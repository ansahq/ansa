use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::crypto::{self, ID_BYTES, KEY_BYTES, Key, OVERHEAD, ProfileId};
use crate::{
    Backend, MAX_METADATA_BYTES, METADATA_FILE, MemoryStore, SECRETS_DIR, SecureStorageIssue,
    Vault, VaultError, VaultMode, VaultState, atomic_write, create_directory, create_directory_all,
    hex, read_bounded, sync_directory,
};

#[cfg(target_os = "linux")]
use crate::native_linux::LinuxKeyStore;
#[cfg(target_os = "macos")]
use crate::native_macos::MacKeyStore;

pub(crate) const FORMAT: u32 = 2;
const NATIVE_CHECK_DOMAIN: &[u8] = b"ansa-vault.native-key-check.v2\0";
const WRAP_DOMAIN: &[u8] = b"ansa-vault.passphrase-wrapped-key.v2\0";

pub(crate) trait NativeKeyStore: Send + Sync {
    fn add(&self, id: &str, key: &Key) -> Result<(), SecureStorageIssue>;
    fn load(&self, id: &str, interactive: bool) -> Result<Key, SecureStorageIssue>;
}

pub(crate) struct Portable {
    pub(crate) id: ProfileId,
    pub(crate) mode: VaultMode,
    source: Source,
    session: Mutex<Session>,
}

enum Source {
    Native {
        store: Arc<dyn NativeKeyStore>,
        check: Vec<u8>,
    },
    Passphrase,
    Memory,
}

struct Session {
    state: VaultState,
    key: Option<Key>,
    memory: MemoryStore,
    /// Exact bytes of `vault.json` observed when this handle opened or last
    /// legitimately rewrote it. Non-secret; used to fail closed if the profile
    /// metadata is replaced underneath an open handle.
    header: Option<Vec<u8>>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    format: u32,
    backend: String,
    profile_id: String,
    key_check: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    salt: Option<String>,
}

struct ParsedHeader {
    id: ProfileId,
    check: Vec<u8>,
    salt: Option<ProfileId>,
    raw: Vec<u8>,
}

impl Portable {
    fn new(
        mode: VaultMode,
        id: ProfileId,
        source: Source,
        key: Option<Key>,
        header: Option<Vec<u8>>,
    ) -> Self {
        Self {
            id,
            mode,
            source,
            session: Mutex::new(Session {
                state: VaultState::Ready,
                key,
                memory: MemoryStore::default(),
                header,
            }),
        }
    }

    pub(crate) fn memory_only() -> Result<Self, VaultError> {
        Ok(Self::new(
            VaultMode::MemoryOnly,
            crypto::random()?,
            Source::Memory,
            Some(crypto::random_key()?),
            None,
        ))
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(crate) fn open_os(
        root: &Path,
        create: bool,
        interactive: bool,
    ) -> Result<Vault, VaultError> {
        #[cfg(target_os = "linux")]
        let (mode, store): (_, Arc<dyn NativeKeyStore>) =
            (VaultMode::LinuxSecretService, Arc::new(LinuxKeyStore));
        #[cfg(target_os = "macos")]
        let (mode, store): (_, Arc<dyn NativeKeyStore>) =
            (VaultMode::MacosKeychain, Arc::new(MacKeyStore));
        Self::open_native(root, mode, store, create, interactive)
    }

    #[cfg_attr(
        not(any(test, target_os = "linux", target_os = "macos")),
        allow(dead_code)
    )]
    fn open_native(
        root: &Path,
        mode: VaultMode,
        store: Arc<dyn NativeKeyStore>,
        create: bool,
        interactive: bool,
    ) -> Result<Vault, VaultError> {
        let root = std::path::absolute(root)?;
        let header = read_header(&root, mode)?;
        let (id, check, header_bytes) = match header {
            Some(header) => {
                drop(native_key(
                    mode,
                    store.as_ref(),
                    &header.id,
                    &header.check,
                    interactive,
                )?);
                (header.id, header.check, header.raw)
            }
            None if create => {
                require_empty_root(&root)?;
                let id = crypto::random()?;
                let key = crypto::random_key()?;
                let check = crypto::seal(&key, b"", &native_aad(mode, &id))?;
                store.add(&hex::encode(&id), &key).map_err(native_error)?;
                // Verify the OS stored exactly the intended key before publishing
                // the header. Normally this will never prompt for a new item.
                drop(key);
                native_key(mode, store.as_ref(), &id, &check, false)?;
                let encoded = encode_header(mode, &id, &check, None)?;
                publish_header(&root, &encoded)?;
                (id, check, encoded)
            }
            None => return Err(VaultError::MetadataMissing),
        };
        let root = prepare_directory(&root)?;
        Ok(Vault {
            root: Some(root),
            backend: Backend::Portable(Self::new(
                mode,
                id,
                Source::Native { store, check },
                None,
                Some(header_bytes),
            )),
        })
    }

    pub(crate) fn open_passphrase(
        root: &Path,
        passphrase: &[u8],
        create: bool,
    ) -> Result<Vault, VaultError> {
        crypto::validate_passphrase(passphrase)?;
        let root = std::path::absolute(root)?;
        let mode = VaultMode::Passphrase;
        let (id, key, header_bytes) = match read_header(&root, mode)? {
            Some(header) => {
                let key = unwrap_key(&header, passphrase)?;
                (header.id, key, header.raw)
            }
            None if create => {
                require_empty_root(&root)?;
                let id = crypto::random()?;
                let key = crypto::random_key()?;
                let encoded = wrap_key(&id, &key, passphrase)?;
                publish_header(&root, &encoded)?;
                (id, key, encoded)
            }
            None => return Err(VaultError::MetadataMissing),
        };
        let root = prepare_directory(&root)?;
        Ok(Vault {
            root: Some(root),
            backend: Backend::Portable(Self::new(
                mode,
                id,
                Source::Passphrase,
                Some(key),
                Some(header_bytes),
            )),
        })
    }

    pub(crate) fn state(&self) -> VaultState {
        self.session
            .lock()
            .map_or(VaultState::Locked, |guard| guard.state)
    }

    pub(crate) fn lock(&self) {
        let mut guard = self
            .session
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        guard.state = VaultState::Locked;
        guard.key = None;
        guard.memory = MemoryStore::default();
    }

    pub(crate) fn unlock(&self, interactive: bool) -> Result<(), VaultError> {
        let mut guard = self.session.lock().map_err(|_| VaultError::Locked)?;
        match &self.source {
            Source::Native { store, check } => {
                let result = native_key(self.mode, store.as_ref(), &self.id, check, interactive);
                guard.state = match &result {
                    Ok(_) => VaultState::Ready,
                    Err(VaultError::SecureStorage { issue }) => VaultState::NativeLocked(*issue),
                    Err(_) => VaultState::Locked,
                };
                result.map(drop)
            }
            Source::Passphrase if guard.state == VaultState::Ready => Ok(()),
            Source::Passphrase => Err(VaultError::PassphraseRequired),
            Source::Memory => {
                if guard.state != VaultState::Ready {
                    guard.key = Some(crypto::random_key()?);
                    guard.state = VaultState::Ready;
                }
                Ok(())
            }
        }
    }

    pub(crate) fn unlock_passphrase(
        &self,
        root: &Path,
        passphrase: &[u8],
    ) -> Result<(), VaultError> {
        self.require_passphrase_mode()?;
        let mut guard = self.session.lock().map_err(|_| VaultError::Locked)?;
        guard.state = VaultState::Locked;
        guard.key = None;
        // Bind the accepted passphrase to the exact header this handle opened
        // (or last legitimately rewrote). Without this check a same-ID wrapper
        // for another key left on disk could be adopted by the locked handle.
        let expected = guard.header.as_deref().ok_or(VaultError::InvalidVault {
            reason: "vault metadata snapshot is missing; reopen this vault",
        })?;
        let header = self.read_matching_header(root, expected)?;
        let key = unwrap_key(&header, passphrase)?;
        guard.key = Some(key);
        guard.state = VaultState::Ready;
        Ok(())
    }

    pub(crate) fn change_passphrase(
        &self,
        root: &Path,
        old_passphrase: &[u8],
        new_passphrase: &[u8],
    ) -> Result<(), VaultError> {
        self.require_passphrase_mode()?;
        crypto::validate_passphrase(old_passphrase)?;
        crypto::validate_passphrase(new_passphrase)?;
        let mut guard = self.session.lock().map_err(|_| VaultError::Locked)?;
        match guard.state {
            VaultState::Ready => {}
            VaultState::Locked => return Err(VaultError::Locked),
            VaultState::NativeLocked(issue) => return Err(native_error(issue)),
        }
        // Do not allow a cached unlocked session to replace a passphrase that
        // has since changed, or a wrapper other than the one this handle
        // opened. Authenticate the exact on-disk wrapper, then make sure it
        // wraps the same data key this session is using.
        let header = {
            let expected = guard.header.as_deref().ok_or(VaultError::InvalidVault {
                reason: "vault metadata snapshot is missing; reopen this vault",
            })?;
            self.read_matching_header(root, expected)?
        };
        let key = unwrap_key(&header, old_passphrase)?;
        let same_key = {
            let cached = guard.key.as_ref().ok_or(VaultError::Locked)?;
            key[..] == cached[..]
        };
        if !same_key {
            return Err(VaultError::InvalidVault {
                reason: "vault header key does not match the open session; reopen this vault",
            });
        }
        let replacement = wrap_key(&self.id, &key, new_passphrase)?;
        atomic_write(root, &root.join(METADATA_FILE), &replacement)?;
        guard.header = Some(replacement);
        Ok(())
    }

    fn require_passphrase_mode(&self) -> Result<(), VaultError> {
        if matches!(self.source, Source::Passphrase) {
            Ok(())
        } else {
            Err(VaultError::WrongMode)
        }
    }

    /// Read `vault.json` and require it to be exactly the header this handle
    /// opened or last legitimately rewrote. Callers hold the session lock.
    fn read_matching_header(
        &self,
        root: &Path,
        expected: &[u8],
    ) -> Result<ParsedHeader, VaultError> {
        let header = read_header(root, self.mode)?.ok_or(VaultError::MetadataMissing)?;
        if header.id != self.id {
            return Err(VaultError::InvalidVault {
                reason: "vault profile identifier changed; reopen this vault",
            });
        }
        if header.raw.as_slice() != expected {
            return Err(VaultError::InvalidVault {
                reason: "vault metadata changed since this vault was opened; reopen it",
            });
        }
        Ok(header)
    }

    /// Verify that `vault.json` is still exactly the metadata this handle
    /// opened. A replaced/removed header would otherwise let the handle keep
    /// writing ciphertext under a profile that cannot be reopened.
    pub(crate) fn verify_header(&self, root: &Path) -> Result<(), VaultError> {
        let guard = self.session.lock().map_err(|_| VaultError::Locked)?;
        let expected = guard.header.as_deref().ok_or(VaultError::InvalidVault {
            reason: "vault metadata snapshot is missing; reopen this vault",
        })?;
        let current = read_header(root, self.mode)?.ok_or(VaultError::MetadataMissing)?;
        if current.raw.as_slice() != expected {
            return Err(VaultError::InvalidVault {
                reason: "vault metadata changed since this vault was opened; reopen it",
            });
        }
        Ok(())
    }

    pub(crate) fn with_key<T>(
        &self,
        f: impl FnOnce(&Key, Option<&mut MemoryStore>) -> Result<T, VaultError>,
    ) -> Result<T, VaultError> {
        let mut guard = self.session.lock().map_err(|_| VaultError::Locked)?;
        match guard.state {
            VaultState::Ready => {}
            VaultState::Locked => return Err(VaultError::Locked),
            VaultState::NativeLocked(issue) => return Err(native_error(issue)),
        }
        let Session {
            state,
            key,
            memory,
            header: _,
        } = &mut *guard;
        match &self.source {
            Source::Native { store, check } => {
                // Deliberately do not cache the OS secret across operations.
                let key = match native_key(self.mode, store.as_ref(), &self.id, check, false) {
                    Ok(key) => key,
                    Err(error) => {
                        *state = match &error {
                            VaultError::SecureStorage { issue } => VaultState::NativeLocked(*issue),
                            _ => VaultState::Locked,
                        };
                        return Err(error);
                    }
                };
                f(&key, None)
            }
            Source::Passphrase => f(key.as_ref().ok_or(VaultError::Locked)?, None),
            Source::Memory => f(key.as_ref().ok_or(VaultError::Locked)?, Some(memory)),
        }
    }
}

fn native_error(issue: SecureStorageIssue) -> VaultError {
    VaultError::SecureStorage { issue }
}

fn native_aad(mode: VaultMode, id: &ProfileId) -> Vec<u8> {
    crypto::aad(NATIVE_CHECK_DOMAIN, id, mode.backend_id().as_bytes())
}

fn native_key(
    mode: VaultMode,
    store: &dyn NativeKeyStore,
    id: &ProfileId,
    check: &[u8],
    interactive: bool,
) -> Result<Key, VaultError> {
    let key = store
        .load(&hex::encode(id), interactive)
        .map_err(native_error)?;
    crypto::open(&key, check, &native_aad(mode, id))
        .map_err(|_| native_error(SecureStorageIssue::InvalidKey))?;
    Ok(key)
}

fn wrap_key(id: &ProfileId, key: &Key, passphrase: &[u8]) -> Result<Vec<u8>, VaultError> {
    let salt = crypto::random()?;
    let derived_key = crypto::derive(passphrase, &salt)?;
    let check = crypto::seal(&derived_key, key, &crypto::aad(WRAP_DOMAIN, id, &salt))?;
    encode_header(VaultMode::Passphrase, id, &check, Some(&salt))
}

fn unwrap_key(header: &ParsedHeader, passphrase: &[u8]) -> Result<Key, VaultError> {
    let salt = header.salt.as_ref().ok_or(VaultError::WrongMode)?;
    let derived_key = crypto::derive(passphrase, salt)?;
    let bytes = crypto::open(
        &derived_key,
        &header.check,
        &crypto::aad(WRAP_DOMAIN, &header.id, salt),
    )
    .map_err(|_| VaultError::InvalidPassphrase)?;
    crypto::key_from_bytes(&bytes).ok_or(VaultError::InvalidPassphrase)
}

fn read_header(root: &Path, mode: VaultMode) -> Result<Option<ParsedHeader>, VaultError> {
    let path = root.join(METADATA_FILE);
    let bytes = match read_bounded(&path, MAX_METADATA_BYTES) {
        Ok(bytes) => bytes,
        Err(VaultError::Io(error)) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    // Inspect format first so callers get a typed mode/format error even if
    // the selected backend uses a different JSON schema (e.g. DPAPI).
    #[derive(Deserialize)]
    struct Discriminator {
        format: u32,
        backend: String,
    }
    let base: Discriminator = serde_json::from_slice(&bytes).map_err(|_| invalid_header())?;
    if base.backend != mode.backend_id() {
        return Err(VaultError::WrongMode);
    }
    if base.format != FORMAT {
        return Err(VaultError::UnsupportedVaultFormat {
            found: base.format,
            supported: FORMAT,
        });
    }
    let header: Header = serde_json::from_slice(&bytes).map_err(|_| invalid_header())?;
    let id = decode_field(&header.profile_id, ID_BYTES)?
        .try_into()
        .map_err(|_| invalid_header())?;
    let passphrase = mode == VaultMode::Passphrase;
    let salt = match (&header.salt, passphrase) {
        (Some(salt), true) => Some(
            decode_field(salt, ID_BYTES)?
                .try_into()
                .map_err(|_| invalid_header())?,
        ),
        (None, false) => None,
        _ => return Err(invalid_header()),
    };
    let check = decode_field(
        &header.key_check,
        OVERHEAD + if passphrase { KEY_BYTES } else { 0 },
    )?;
    Ok(Some(ParsedHeader {
        id,
        check,
        salt,
        raw: bytes,
    }))
}

fn decode_field(encoded: &str, size: usize) -> Result<Vec<u8>, VaultError> {
    if encoded.len() != size * 2
        || !encoded
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(invalid_header());
    }
    hex::decode(encoded).ok_or_else(invalid_header)
}

fn invalid_header() -> VaultError {
    VaultError::InvalidVault {
        reason: "vault header is malformed",
    }
}

fn encode_header(
    mode: VaultMode,
    id: &ProfileId,
    check: &[u8],
    salt: Option<&ProfileId>,
) -> Result<Vec<u8>, VaultError> {
    serde_json::to_vec(&Header {
        format: FORMAT,
        backend: mode.backend_id().to_owned(),
        profile_id: hex::encode(id),
        key_check: hex::encode(check),
        salt: salt.map(|salt| hex::encode(salt)),
    })
    .map_err(|_| VaultError::Internal {
        operation: "encoding vault header",
    })
}

fn require_empty_root(root: &Path) -> Result<(), VaultError> {
    // A provisioned vault always has `secrets/` even if all payloads are stored
    // in the caller's database. Its presence without metadata is not a fresh
    // profile. Reject unrelated data too; profiles have dedicated directories.
    match fs::read_dir(root) {
        Ok(mut entries) => {
            if entries.next().transpose()?.is_some() {
                Err(VaultError::MetadataMissing)
            } else {
                Ok(())
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn publish_header(root: &Path, bytes: &[u8]) -> Result<(), VaultError> {
    create_directory_all(root)?;
    crate::atomic_create(root, &root.join(METADATA_FILE), bytes)
}

fn prepare_directory(root: &Path) -> Result<PathBuf, VaultError> {
    let root = fs::canonicalize(root)?;
    let secrets = root.join(SECRETS_DIR);
    match create_directory(&secrets) {
        Ok(()) => {
            sync_directory(&root);
            Ok(root)
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            if fs::symlink_metadata(secrets)?.is_dir() {
                Ok(root)
            } else {
                Err(VaultError::InvalidVault {
                    reason: "vault secrets path is not a regular directory",
                })
            }
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeStore {
        inner: Mutex<FakeState>,
    }

    #[derive(Default)]
    struct FakeState {
        registered: Vec<(String, Key)>,
        issue: Option<SecureStorageIssue>,
        load_issue: Option<SecureStorageIssue>,
        return_different_key: bool,
        adds: usize,
        loads: usize,
        allow_interactive: bool,
    }

    impl NativeKeyStore for FakeStore {
        fn add(&self, id: &str, key: &Key) -> Result<(), SecureStorageIssue> {
            let mut state = self.inner.lock().unwrap();
            if let Some(issue) = state.issue {
                return Err(issue);
            }
            assert!(
                !state.registered.iter().any(|(stored, _)| stored == id),
                "unexpected replacement key"
            );
            state.adds += 1;
            state
                .registered
                .push((id.to_owned(), crypto::key_from_bytes(key).unwrap()));
            Ok(())
        }

        fn load(&self, id: &str, interactive: bool) -> Result<Key, SecureStorageIssue> {
            let mut state = self.inner.lock().unwrap();
            state.loads += 1;
            if let Some(issue) = state.load_issue {
                return Err(issue);
            }
            if let Some(issue) = state.issue
                && !(interactive && state.allow_interactive)
            {
                return Err(issue);
            }
            let (_, key) = state
                .registered
                .iter()
                .find(|(stored, _)| stored == id)
                .ok_or(SecureStorageIssue::KeyMissing)?;
            if state.return_different_key {
                Ok(crypto::empty_key())
            } else {
                Ok(crypto::key_from_bytes(key).unwrap())
            }
        }
    }

    fn open(root: &Path, store: &Arc<FakeStore>, create: bool) -> Result<Vault, VaultError> {
        Portable::open_native(
            root,
            VaultMode::LinuxSecretService,
            store.clone(),
            create,
            false,
        )
    }

    fn expect_issue<T>(result: Result<T, VaultError>, expected: SecureStorageIssue) {
        assert!(matches!(result, Err(VaultError::SecureStorage { issue }) if issue == expected));
    }

    #[test]
    fn denied_locked_and_lost_key_leave_unchanged_ciphertext_and_recover() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let store = Arc::new(FakeStore::default());
        let vault = open(root, &store, true).unwrap();
        vault.put_secret("alpha", b"private").unwrap();
        let external = vault.seal(b"external", "clipboard.text").unwrap();
        let header_path = root.join(METADATA_FILE);
        let sealed_path = root.join(SECRETS_DIR).join("alpha.sealed");
        let original_header = fs::read(&header_path).unwrap();
        let original_ciphertext = fs::read(&sealed_path).unwrap();

        for issue in [
            SecureStorageIssue::AccessDenied,
            SecureStorageIssue::Locked,
            SecureStorageIssue::KeyMissing,
            SecureStorageIssue::Unavailable,
            SecureStorageIssue::TimedOut,
        ] {
            let removed = {
                let mut provider = store.inner.lock().unwrap();
                if issue == SecureStorageIssue::KeyMissing {
                    std::mem::take(&mut provider.registered)
                } else {
                    provider.issue = Some(issue);
                    Vec::new()
                }
            };
            expect_issue(vault.get_secret("alpha"), issue);
            assert_eq!(vault.status().state, VaultState::NativeLocked(issue));
            expect_issue(vault.open_payload(&external, "clipboard.text"), issue);
            expect_issue(vault.put_secret("alpha", b"replacement"), issue);
            expect_issue(vault.list_secrets(), issue);
            expect_issue(vault.delete_secret("alpha"), issue);
            expect_issue(vault.rotate(), issue);
            expect_issue(vault.verify_backup(), issue);
            expect_issue(open(root, &store, true), issue);
            expect_issue(vault.try_unlock(), issue);
            assert_eq!(fs::read(&header_path).unwrap(), original_header);
            assert_eq!(fs::read(&sealed_path).unwrap(), original_ciphertext);

            let mut provider = store.inner.lock().unwrap();
            provider.issue = None;
            if !removed.is_empty() {
                provider.registered = removed;
            }
            drop(provider);
            vault.try_unlock().unwrap();
            assert_eq!(vault.status().state, VaultState::Ready);
            assert_eq!(vault.get_secret("alpha").unwrap(), b"private");
        }
        assert_eq!(store.inner.lock().unwrap().adds, 1);
    }

    #[test]
    fn unexpected_replacement_key_does_not_authenticate_the_vault() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(FakeStore::default());
        let vault = open(temp.path(), &store, true).unwrap();
        vault.put_secret("alpha", b"original").unwrap();
        let path = temp.path().join("secrets/alpha.sealed");
        let original_ciphertext = fs::read(&path).unwrap();
        store.inner.lock().unwrap().return_different_key = true;
        expect_issue(vault.seal(b"new", "ctx"), SecureStorageIssue::InvalidKey);
        expect_issue(
            open(temp.path(), &store, true),
            SecureStorageIssue::InvalidKey,
        );
        assert_eq!(fs::read(&path).unwrap(), original_ciphertext);
        assert_eq!(store.inner.lock().unwrap().adds, 1);
        store.inner.lock().unwrap().return_different_key = false;
        vault.try_unlock().unwrap();
        assert_eq!(vault.get_secret("alpha").unwrap(), b"original");
    }

    #[test]
    fn only_explicit_unlock_or_reopen_may_request_an_os_prompt() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(FakeStore::default());
        let vault = open(temp.path(), &store, true).unwrap();
        {
            let mut state = store.inner.lock().unwrap();
            state.issue = Some(SecureStorageIssue::Locked);
            state.allow_interactive = true;
        }
        expect_issue(vault.seal(b"data", "context"), SecureStorageIssue::Locked);
        vault.unlock();
        assert!(vault.is_locked());
        vault.try_unlock().unwrap();
        assert!(!vault.is_locked());
        expect_issue(open(temp.path(), &store, false), SecureStorageIssue::Locked);
        assert!(
            Portable::open_native(
                temp.path(),
                VaultMode::LinuxSecretService,
                store,
                false,
                true,
            )
            .is_ok()
        );
    }

    #[test]
    fn lost_metadata_never_registers_a_replacement_key() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(FakeStore::default());
        let root = temp.path();
        let vault = open(root, &store, true).unwrap();
        vault.put_secret("alpha", b"private").unwrap();
        let path = root.join("secrets/alpha.sealed");
        let original = fs::read(&path).unwrap();
        fs::remove_file(root.join(METADATA_FILE)).unwrap();
        assert!(matches!(
            open(root, &store, true),
            Err(VaultError::MetadataMissing)
        ));
        assert!(matches!(
            open(root, &store, false),
            Err(VaultError::MetadataMissing)
        ));
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(store.inner.lock().unwrap().adds, 1);
        assert!(!root.join(METADATA_FILE).exists());
        // Even if all application ciphertext is in an external database, a
        // previously initialized vault directory is distinguishable from fresh.
        fs::remove_file(&path).unwrap();
        assert!(matches!(
            open(root, &store, true),
            Err(VaultError::MetadataMissing)
        ));
    }

    #[test]
    fn unavailable_provider_and_existing_only_open_do_not_write_a_new_profile() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("absent");
        let store = Arc::new(FakeStore::default());
        store.inner.lock().unwrap().issue = Some(SecureStorageIssue::Unavailable);
        expect_issue(open(&root, &store, true), SecureStorageIssue::Unavailable);
        assert!(!root.exists());
        assert!(matches!(
            open(&root, &store, false),
            Err(VaultError::MetadataMissing)
        ));
        assert!(!root.exists());
        assert_eq!(store.inner.lock().unwrap().adds, 0);
    }

    #[test]
    fn registration_verification_failure_never_publishes_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let store = Arc::new(FakeStore::default());
        store.inner.lock().unwrap().load_issue = Some(SecureStorageIssue::TimedOut);

        expect_issue(open(root, &store, true), SecureStorageIssue::TimedOut);
        assert!(!root.join(METADATA_FILE).exists());
        assert_eq!(fs::read_dir(root).unwrap().count(), 0);
        assert_eq!(store.inner.lock().unwrap().adds, 1);
        assert_eq!(store.inner.lock().unwrap().loads, 1);
        assert_eq!(store.inner.lock().unwrap().registered.len(), 1);

        // The existing-only path must not contact the provider at all.
        assert!(matches!(
            open(root, &store, false),
            Err(VaultError::MetadataMissing)
        ));
        assert_eq!(store.inner.lock().unwrap().loads, 1);

        // A later fresh attempt creates a new profile. The unverified orphan is
        // deliberately retained, never treated as a replacement for a profile.
        store.inner.lock().unwrap().load_issue = None;
        let vault = open(root, &store, true).unwrap();
        assert!(root.join(METADATA_FILE).exists());
        assert_eq!(store.inner.lock().unwrap().adds, 2);
        assert_eq!(store.inner.lock().unwrap().registered.len(), 2);
        vault.put_secret("alpha", b"private").unwrap();
        assert_eq!(vault.get_secret("alpha").unwrap(), b"private");
    }

    #[test]
    fn missing_or_replaced_header_fails_closed_until_restored() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let store = Arc::new(FakeStore::default());
        let vault = open(root, &store, true).unwrap();
        vault.put_secret("alpha", b"private").unwrap();
        let external = vault.seal(b"external", "clipboard.text").unwrap();
        let header_path = root.join(METADATA_FILE);
        let sealed_path = root.join(SECRETS_DIR).join("alpha.sealed");
        let original_header = fs::read(&header_path).unwrap();
        let original_ciphertext = fs::read(&sealed_path).unwrap();

        fs::remove_file(&header_path).unwrap();
        assert!(matches!(
            vault.get_secret("alpha"),
            Err(VaultError::MetadataMissing)
        ));
        assert!(matches!(
            vault.put_secret("alpha", b"replacement"),
            Err(VaultError::MetadataMissing)
        ));
        assert!(matches!(
            vault.open_payload(&external, "clipboard.text"),
            Err(VaultError::MetadataMissing)
        ));
        assert!(matches!(vault.rotate(), Err(VaultError::MetadataMissing)));
        assert!(matches!(
            vault.verify_backup(),
            Err(VaultError::MetadataMissing)
        ));
        assert!(matches!(
            vault.try_unlock(),
            Err(VaultError::MetadataMissing)
        ));
        assert!(matches!(
            open(root, &store, true),
            Err(VaultError::MetadataMissing)
        ));
        assert!(matches!(
            open(root, &store, false),
            Err(VaultError::MetadataMissing)
        ));
        assert_eq!(fs::read(&sealed_path).unwrap(), original_ciphertext);
        assert_eq!(store.inner.lock().unwrap().adds, 1);
        assert!(!header_path.exists());

        fs::write(&header_path, &original_header).unwrap();
        assert_eq!(vault.get_secret("alpha").unwrap(), b"private");
        vault.try_unlock().unwrap();
        assert_eq!(vault.verify_backup().unwrap().secrets_verified, 1);

        // A different valid profile header is not accepted by the open handle.
        let other_store = Arc::new(FakeStore::default());
        let other_root = temp.path().join("other");
        let other = open(&other_root, &other_store, true).unwrap();
        drop(other);
        let other_header = fs::read(other_root.join(METADATA_FILE)).unwrap();
        fs::write(&header_path, &other_header).unwrap();
        assert!(matches!(
            vault.get_secret("alpha"),
            Err(VaultError::InvalidVault { .. })
        ));
        assert!(matches!(
            vault.put_secret("alpha", b"replacement"),
            Err(VaultError::InvalidVault { .. })
        ));
        assert!(matches!(
            vault.rotate(),
            Err(VaultError::InvalidVault { .. })
        ));
        assert!(matches!(
            vault.verify_backup(),
            Err(VaultError::InvalidVault { .. })
        ));
        assert!(matches!(
            vault.try_unlock(),
            Err(VaultError::InvalidVault { .. })
        ));
        assert_eq!(fs::read(&sealed_path).unwrap(), original_ciphertext);
        assert_eq!(store.inner.lock().unwrap().adds, 1);
        assert_eq!(other_store.inner.lock().unwrap().adds, 1);

        fs::write(&header_path, original_header).unwrap();
        assert_eq!(vault.get_secret("alpha").unwrap(), b"private");
        assert_eq!(vault.verify_backup().unwrap().secrets_verified, 1);
    }

    #[test]
    fn invalid_input_is_rejected_before_provider_access() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let store = Arc::new(FakeStore::default());
        let vault = open(root, &store, true).unwrap();
        let payload = vault.seal(b"external", "clipboard.text").unwrap();
        store.inner.lock().unwrap().issue = Some(SecureStorageIssue::AccessDenied);
        let loads_before = store.inner.lock().unwrap().loads;

        assert!(matches!(
            vault.seal(b"value", "bad context"),
            Err(VaultError::InvalidContext { .. })
        ));
        assert!(matches!(
            vault.seal(&vec![0; crate::MAX_PLAINTEXT_BYTES + 1], "clipboard.text"),
            Err(VaultError::LimitExceeded { .. })
        ));
        assert!(matches!(
            vault.put_secret("../escape", b"value"),
            Err(VaultError::InvalidAlias { .. })
        ));
        assert!(matches!(
            vault.put_secret("oversized", &vec![0; crate::MAX_PLAINTEXT_BYTES + 1]),
            Err(VaultError::LimitExceeded { .. })
        ));
        assert!(matches!(
            vault.get_secret("bad alias"),
            Err(VaultError::InvalidAlias { .. })
        ));
        assert!(matches!(
            vault.delete_secret("bad/alias"),
            Err(VaultError::InvalidAlias { .. })
        ));
        assert!(matches!(
            vault.open_payload(&payload, "bad context"),
            Err(VaultError::InvalidContext { .. })
        ));
        assert!(matches!(
            vault.verify_payload(&payload, "bad context"),
            Err(VaultError::InvalidContext { .. })
        ));
        assert_eq!(store.inner.lock().unwrap().loads, loads_before);
        assert_eq!(vault.status().state, VaultState::Ready);

        // A valid request observes the denial and gates this handle.
        expect_issue(vault.get_secret("valid"), SecureStorageIssue::AccessDenied);
        assert_eq!(
            vault.status().state,
            VaultState::NativeLocked(SecureStorageIssue::AccessDenied)
        );
        assert_eq!(store.inner.lock().unwrap().adds, 1);
    }

    #[test]
    fn wrong_mode_malformed_and_oversized_headers_do_not_contact_provider() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let store = Arc::new(FakeStore::default());
        let header_path = root.join(METADATA_FILE);
        let linux_backend = VaultMode::LinuxSecretService.backend_id();

        let passphrase_header =
            br#"{"format":2,"backend":"passphrase-argon2id-xchacha20poly1305-v1"}"#;
        fs::write(&header_path, passphrase_header).unwrap();
        assert!(matches!(
            open(root, &store, true),
            Err(VaultError::WrongMode)
        ));
        assert_eq!(fs::read(&header_path).unwrap(), passphrase_header);

        let malformed = format!(r#"{{"format":2,"backend":"{linux_backend}"}}"#);
        fs::write(&header_path, malformed.as_bytes()).unwrap();
        assert!(matches!(
            open(root, &store, true),
            Err(VaultError::InvalidVault { .. })
        ));

        let unsupported = format!(r#"{{"format":99,"backend":"{linux_backend}"}}"#);
        fs::write(&header_path, unsupported.as_bytes()).unwrap();
        assert!(matches!(
            open(root, &store, true),
            Err(VaultError::UnsupportedVaultFormat {
                found: 99,
                supported: 2
            })
        ));

        fs::write(&header_path, vec![b'{'; crate::MAX_METADATA_BYTES + 1]).unwrap();
        assert!(matches!(
            open(root, &store, true),
            Err(VaultError::LimitExceeded { .. })
        ));

        let state = store.inner.lock().unwrap();
        assert_eq!(state.adds, 0);
        assert_eq!(state.loads, 0);
        drop(state);

        fs::remove_file(&header_path).unwrap();
        assert!(matches!(
            open(root, &store, false),
            Err(VaultError::MetadataMissing)
        ));
        assert_eq!(store.inner.lock().unwrap().loads, 0);
    }

    #[test]
    fn atomic_metadata_publication_does_not_clobber_a_concurrent_winner() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        publish_header(root, b"first").unwrap();
        assert!(matches!(
            publish_header(root, b"second"),
            Err(VaultError::Busy)
        ));
        assert_eq!(fs::read(root.join(METADATA_FILE)).unwrap(), b"first");
        assert_eq!(fs::read_dir(root).unwrap().count(), 1);
    }

    fn portable(vault: &Vault) -> &Portable {
        match &vault.backend {
            #[cfg(windows)]
            Backend::Dpapi(_) => panic!("expected a portable backend"),
            Backend::Portable(portable) => portable,
        }
    }

    #[test]
    fn passphrase_unlock_rejects_a_same_id_wrapper_for_another_key() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("vault");
        let passphrase = b"vault-unit-fixture-passphrase";
        let new_passphrase = b"vault-unit-fixture-replacement";
        let vault = Portable::open_passphrase(&root, passphrase, true).unwrap();
        vault.put_secret("alpha", b"private").unwrap();
        let header_path = root.join(METADATA_FILE);
        let sealed_path = root.join(SECRETS_DIR).join("alpha.sealed");
        let opened_header = fs::read(&header_path).unwrap();
        let opened_ciphertext = fs::read(&sealed_path).unwrap();

        // Same public profile ID, but a wrapper around another data key. The
        // old passphrase would unwrap this successfully if only the ID/salt
        // were checked.
        let replacement = wrap_key(
            &portable(&vault).id,
            &crypto::random_key().unwrap(),
            passphrase,
        )
        .unwrap();
        assert_ne!(replacement, opened_header);
        fs::write(&header_path, &replacement).unwrap();
        assert!(matches!(
            vault.put_secret("alpha", b"replacement"),
            Err(VaultError::InvalidVault { .. })
        ));

        vault.lock();
        assert!(matches!(
            vault.unlock_with_passphrase(passphrase),
            Err(VaultError::InvalidVault { .. })
        ));
        assert!(vault.is_locked());
        assert_eq!(vault.status().state, VaultState::Locked);
        assert!(matches!(vault.get_secret("alpha"), Err(VaultError::Locked)));
        assert!(matches!(
            vault.change_passphrase(passphrase, new_passphrase),
            Err(VaultError::Locked)
        ));
        assert_eq!(fs::read(&header_path).unwrap(), replacement);
        assert_eq!(fs::read(&sealed_path).unwrap(), opened_ciphertext);

        // Restoring the exact opened bytes is enough to recover this handle.
        fs::write(&header_path, &opened_header).unwrap();
        vault.unlock_with_passphrase(passphrase).unwrap();
        assert_eq!(vault.get_secret("alpha").unwrap(), b"private");
    }

    #[test]
    fn passphrase_change_never_overwrites_a_replaced_same_id_wrapper() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("vault");
        let passphrase = b"vault-unit-fixture-passphrase";
        let new_passphrase = b"vault-unit-fixture-replacement";
        let vault = Portable::open_passphrase(&root, passphrase, true).unwrap();
        vault.put_secret("alpha", b"private").unwrap();
        let header_path = root.join(METADATA_FILE);
        let sealed_path = root.join(SECRETS_DIR).join("alpha.sealed");
        let opened_header = fs::read(&header_path).unwrap();
        let opened_ciphertext = fs::read(&sealed_path).unwrap();

        vault.lock();
        fs::write(
            &header_path,
            wrap_key(
                &portable(&vault).id,
                &crypto::random_key().unwrap(),
                passphrase,
            )
            .unwrap(),
        )
        .unwrap();
        let replaced = fs::read(&header_path).unwrap();
        assert!(matches!(
            vault.unlock_with_passphrase(passphrase),
            Err(VaultError::InvalidVault { .. })
        ));
        assert!(vault.is_locked());
        assert_eq!(fs::read(&header_path).unwrap(), replaced);

        fs::write(&header_path, &opened_header).unwrap();
        vault.unlock_with_passphrase(passphrase).unwrap();
        fs::write(&header_path, &replaced).unwrap();
        assert!(matches!(
            vault.change_passphrase(passphrase, new_passphrase),
            Err(VaultError::InvalidVault { .. })
        ));
        assert_eq!(fs::read(&header_path).unwrap(), replaced);
        assert_eq!(fs::read(&sealed_path).unwrap(), opened_ciphertext);
        assert!(matches!(
            vault.put_secret("alpha", b"replacement"),
            Err(VaultError::InvalidVault { .. })
        ));

        // Normal change still works after the exact opened metadata returns.
        fs::write(&header_path, &opened_header).unwrap();
        vault.change_passphrase(passphrase, new_passphrase).unwrap();
        vault.lock();
        assert!(matches!(
            vault.unlock_with_passphrase(passphrase),
            Err(VaultError::InvalidPassphrase)
        ));
        assert!(vault.is_locked());
        vault.unlock_with_passphrase(new_passphrase).unwrap();
        assert_eq!(vault.get_secret("alpha").unwrap(), b"private");
        assert_eq!(fs::read(&sealed_path).unwrap(), opened_ciphertext);
    }
}
