//! Native secret sealing and encrypted payload storage for Windows, macOS and Linux.
//!
//! # What this crate is
//!
//! `ansa-vault` stores sensitive bytes as opaque, user-bound ciphertext. On
//! Windows the default sealing backend is DPAPI through `CryptProtectData` and
//! `CryptUnprotectData`, called with `CRYPTPROTECT_UI_FORBIDDEN` and a
//! per-purpose entropy blob derived from a validated context label. No
//! home-grown cipher, plaintext key file or plaintext fallback exists.
//! On macOS, `Vault::open` uses the user's Keychain. On Linux, it uses the
//! session's Secret Service. These implementations have not yet been exercised
//! on native hosts. Explicit passphrase and memory-only modes are also
//! available; neither is automatically selected on native failure.
//!
//! A vault is a directory:
//!
//! ```text
//! <vault>/vault.json                 format/backend metadata and wrapped key check
//! <vault>/secrets/<alias>.sealed     JSON envelope with authenticated ciphertext
//! ```
//!
//! Files contain only non-plaintext metadata and hex-encoded ciphertext.
//! Format-2 headers include the public profile identifier and a key-check or
//! passphrase-wrapped-key ciphertext. Secret file names are derived from
//! validated aliases; temporary files are ignored and replaced atomically with
//! `write + sync_all + rename`. New Unix directories are mode `0700` and new
//! files are mode `0600`. Directory-entry sync is best-effort because some
//! supported filesystems reject directory `fsync`.
//!
//! A format-2 handle is bound to the exact `vault.json` bytes it opened. If the
//! header is removed or replaced while the handle is alive, secret and payload
//! operations fail closed until the original metadata is restored or the vault
//! is reopened. This prevents writes under a stale profile after a backup
//! restore or concurrent credential change.
//!
//! # Honest security boundaries
//!
//! * **DPAPI is user-bound, not passphrase-bound.** Any process running as the
//!   same Windows user can call DPAPI and unprotect the same data. [`Vault::lock`]
//!   is a session gate for this API; it does not add a password, erase the OS
//!   key, or prevent another same-user process from reading the ciphertext.
//! * **DPAPI backup is profile-bound.** DPAPI protects data for the current user
//!   profile. Copying the vault to another user account or machine normally
//!   makes decryption fail; the payload then remains an unrecoverable opaque
//!   blob rather than turning into plaintext. Roaming-profile/credential backup
//!   scenarios are an explicit future design, not a claim of this crate.
//!   Legacy `Vault::open` may recreate the non-secret format-1 metadata if it
//!   is missing, because DPAPI ciphertext is user-bound rather than bound to
//!   that metadata; `Vault::open_existing` and `verify_backup` still require it.
//!   Format-2 backends never recreate a missing header.
//! * **Key rotation is OS-managed.** Windows owns the DPAPI master keys.
//!   [`Vault::rotate`] re-protects every stored secret under the current DPAPI
//!   key, but it cannot rotate Windows' own master keys.
//! * **Native revocation is live.** The Keychain/Secret Service key is fetched
//!   per operation and authenticated against the profile header. On denial,
//!   lock, loss or mismatch, ciphertext is not replaced; restore native
//!   access then retry. There is no silent fallback. The open handle also
//!   checks that `vault.json` is unchanged before each secret/payload
//!   operation; a removed or replaced header fails closed until the vault is
//!   reopened.
//! * **Tampering and wrong context fail closed.** DPAPI authenticates its
//!   output, and the context is bound through the entropy blob. A modified
//!   payload, a different context, another user or another machine yields
//!   [`VaultError::DecryptionFailed`]. In the Keychain, Secret Service,
//!   passphrase and memory-only modes, XChaCha20-Poly1305 authenticates
//!   both the context and a random profile identifier.
//! * **Context labels are not secrets.** A context is a bounded ASCII label
//!   such as `clipboard.text`; it is stored in the payload's `context_tag`.
//!   Callers must not put secret material in a context.
//! * **Aliases resolve exact-case first.** A non-exact lookup is
//!   case-insensitive (matching Windows), but if a case-sensitive filesystem
//!   contains multiple case variants for the same alias, the operation fails
//!   closed instead of deleting or replacing an arbitrary file.
//!
//! # Bounds
//!
//! Plaintext is limited to 1 MiB, a sealed file to 3 MiB, a context label to
//! 160 bytes, an alias to 128 bytes and a vault to 4096 stored secrets. Vault
//! metadata is limited to 4 KiB. Directory scans are bounded as well. These
//! bounds are enforced on reads as well as writes.
//!
//! # Payload API
//!
//! [`Vault::open`] is the directory constructor. Because Rust does not allow an
//! inherent associated function and an inherent method with the same name,
//! opening a [`SealedPayload`] is exposed both as the [`OpenPayload::open`]
//! trait method (import the trait to call `vault.open(&payload, context)`) and
//! as the inherent [`Vault::open_payload`] method.

#![deny(unsafe_op_in_unsafe_fn)]

mod crypto;
mod error;
mod hex;
mod portable;

#[cfg(windows)]
#[path = "dpapi_windows.rs"]
mod dpapi;
#[cfg(target_os = "linux")]
mod native_linux;
#[cfg(target_os = "macos")]
mod native_macos;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::ops::Deref;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::crypto::{Key, ProfileId};
pub use crate::error::{SecureStorageIssue, VaultError};
use crate::portable::Portable;
pub use zeroize::Zeroizing;

/// Legacy Windows DPAPI [`SealedPayload`] version (unchanged).
pub const PAYLOAD_VERSION: u16 = 1;

/// Keychain, Secret Service, passphrase and memory-only payload version.
pub const AEAD_PAYLOAD_VERSION: u16 = 2;

/// Legacy Windows DPAPI vault metadata version (unchanged).
pub const VAULT_FORMAT_VERSION: u32 = 1;

/// Legacy Windows DPAPI backend identifier.
pub const BACKEND_ID: &str = "windows-dpapi-v1";

/// Maximum plaintext accepted by [`Vault::seal`] and [`Vault::put_secret`]:
/// 1 MiB.
pub const MAX_PLAINTEXT_BYTES: usize = 1024 * 1024;

/// Maximum encoded sealed file accepted from disk: 3 MiB.
///
/// A 1 MiB plaintext becomes roughly 2.2 MiB of hexadecimal ciphertext plus
/// the JSON envelope, so this leaves headroom without permitting unbounded
/// reads.
pub const MAX_SEALED_FILE_BYTES: usize = 3 * 1024 * 1024;

/// Maximum context label length in bytes.
pub const MAX_CONTEXT_BYTES: usize = 160;

/// Maximum number of secrets in one vault.
pub const MAX_SECRETS: usize = 4096;

/// Maximum alias length in bytes.
pub const MAX_ALIAS_BYTES: usize = 128;

/// Maximum accepted passphrase length in bytes; passphrases must be nonempty.
pub const MAX_PASSPHRASE_BYTES: usize = 1024;

/// Maximum encoded in-memory secret bytes (in addition to the alias-count limit).
pub const MAX_MEMORY_SECRET_BYTES: usize = 16 * 1024 * 1024;

const METADATA_FILE: &str = "vault.json";
const MAX_METADATA_BYTES: usize = 4096;

const SECRETS_DIR: &str = "secrets";
const SECRET_FILE_SUFFIX: &str = ".sealed";
const TEMP_PREFIX: &str = ".tmp-";
#[cfg(windows)]
const CONTEXT_DOMAIN: &[u8] = b"ansa-vault.dpapi.v1\0";
const AEAD_CONTEXT_DOMAIN: &[u8] = b"ansa-vault.payload.v2\0";

/// Bound on raw directory entries inspected by one list/lookup operation.
/// This keeps a directory filled with unrelated files from making an
/// individual operation unbounded.
const MAX_DIRECTORY_ENTRIES: usize = MAX_SECRETS + 1024;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A serde-serializable sealed payload.
///
/// `context_tag` is the validated,
/// non-secret context label used for binding and diagnostics. `Debug` omits
/// the ciphertext body.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedPayload {
    /// Payload format version determined by the active backend.
    pub version: u16,
    /// The validated, non-secret context label the payload was bound to.
    pub context_tag: String,
    /// Lowercase hexadecimal encoding of the opaque backend ciphertext.
    pub ciphertext_hex: String,
}

impl fmt::Debug for SealedPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SealedPayload")
            .field("version", &self.version)
            .field("context_tag", &self.context_tag)
            .field("ciphertext_bytes", &(self.ciphertext_hex.len() / 2))
            .finish()
    }
}

/// Summary of [`Vault::rotate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct RotationReport {
    /// Number of secrets successfully re-protected with the current vault key.
    pub secrets_resealed: usize,
    /// Whether OS-managed keys were rotated. Always `false`: this routine
    /// only re-seals payloads.
    pub os_key_rotation_performed: bool,
}

/// Active vault storage mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum VaultMode {
    WindowsDpapi,
    MacosKeychain,
    LinuxSecretService,
    Passphrase,
    MemoryOnly,
}

impl VaultMode {
    /// Stable identifier for diagnostics and, for persistent modes, on-disk metadata.
    #[must_use]
    pub fn backend_id(self) -> &'static str {
        match self {
            Self::WindowsDpapi => BACKEND_ID,
            Self::MacosKeychain => "macos-keychain-xchacha20poly1305-v1",
            Self::LinuxSecretService => "linux-secret-service-xchacha20poly1305-v1",
            Self::Passphrase => "passphrase-argon2id-xchacha20poly1305-v1",
            Self::MemoryOnly => "memory-only-xchacha20poly1305-v1",
        }
    }

    /// Whether callers may persist sealed payloads. In memory-only mode callers
    /// must keep sensitive history and its metadata entirely in memory.
    #[must_use]
    pub fn is_persistent(self) -> bool {
        self != Self::MemoryOnly
    }
}

/// Last known state for a successfully opened vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum VaultState {
    Ready,
    Locked,
    NativeLocked(SecureStorageIssue),
}

/// Evidence available for this storage mode, independent of current access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum VaultValidation {
    /// Windows DPAPI exercised on Windows.
    NativeTested,
    /// Portable crypto/storage tested; no native provider is involved.
    PortableTested,
    /// A native implementation exists, but has no recorded real-host evidence.
    NativeUnverified,
}

/// Last known capability and access outcome. Reading this value does not
/// contact the OS; a subsequent operation can observe access changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct VaultStatus {
    pub mode: VaultMode,
    pub state: VaultState,
    pub validation: VaultValidation,
}

/// Authenticated verification of every named secret found in a backup.
/// Does not assert completeness: verify the count against the backup inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct VerificationReport {
    pub secrets_verified: usize,
}

/// Plaintext buffer that is wiped on drop and redacted in `Debug` output.
/// Exposing or copying the bytes transfers responsibility to the caller.
pub struct SecretBytes(Zeroizing<Vec<u8>>);

impl SecretBytes {
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    fn into_legacy_vec(mut self) -> Vec<u8> {
        std::mem::take(&mut *self.0)
    }
}

impl Deref for SecretBytes {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretBytes([redacted])")
    }
}

/// A persistent or explicitly memory-only vault.
///
/// `Vault` is safe to share behind a reference.
/// Cloning is intentionally not implemented so a lock cannot be bypassed with
/// stale copies.
pub struct Vault {
    root: Option<PathBuf>,
    backend: Backend,
}

enum Backend {
    #[cfg(windows)]
    Dpapi(AtomicBool),
    Portable(Portable),
}

#[derive(Clone, Copy)]
enum Cipher<'a> {
    #[cfg(windows)]
    Dpapi,
    Aead(&'a Key, &'a ProfileId),
}

impl fmt::Debug for Vault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Vault")
            .field("root", &self.root)
            .field("status", &self.status())
            .finish()
    }
}

impl Vault {
    /// Open (or create) a vault using the platform's native secure storage.
    ///
    /// On Windows this creates the directory, the `secrets` subdirectory and a
    /// bounded, non-sensitive `vault.json` metadata file if they are absent.
    /// On macOS/Linux an inaccessible keyring or missing key yields a typed
    /// failure even when ciphertext is present. No fallback is selected.
    /// Format-2 metadata is bound to the returned handle; if it is removed or
    /// replaced, secret and payload operations fail until the original
    /// metadata is restored or the vault is reopened. Unsupported operating systems return
    /// [`VaultError::Unsupported`].
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, VaultError> {
        Self::open_native(dir.as_ref(), true, false)
    }

    /// Reopen an existing native vault without allowing a fresh profile/key.
    /// Use this when the caller already has sealed data (e.g. in a database),
    /// so that losing the entire vault directory is distinguishable from a new
    /// profile. `allow_prompt` opts into platform authentication during reopen.
    pub fn open_existing(dir: impl AsRef<Path>, allow_prompt: bool) -> Result<Self, VaultError> {
        Self::open_native(dir.as_ref(), false, allow_prompt)
    }

    fn open_native(root: &Path, create: bool, allow_prompt: bool) -> Result<Self, VaultError> {
        #[cfg(windows)]
        {
            let _ = allow_prompt;
            if !create && !root.join(METADATA_FILE).try_exists()? {
                return Err(VaultError::MetadataMissing);
            }
            Self::open_windows(root)
        }
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            Portable::open_os(root, create, allow_prompt)
        }
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            let _ = (root, create, allow_prompt);
            Err(VaultError::Unsupported {
                platform: std::env::consts::OS,
                reason: "no native secure storage is implemented for this platform",
            })
        }
    }

    /// Explicitly choose persistent passphrase storage for a dedicated vault
    /// directory. Do not put passphrases in logs, command arguments or
    /// environment variables. The caller owns and must wipe its input.
    /// Does not open or overwrite a native vault.
    pub fn open_passphrase(dir: impl AsRef<Path>, passphrase: &[u8]) -> Result<Self, VaultError> {
        Portable::open_passphrase(dir.as_ref(), passphrase, true)
    }

    /// Reopen existing passphrase storage; never creates metadata or a key.
    pub fn open_existing_passphrase(
        dir: impl AsRef<Path>,
        passphrase: &[u8],
    ) -> Result<Self, VaultError> {
        Portable::open_passphrase(dir.as_ref(), passphrase, false)
    }

    /// Explicitly choose a new volatile vault. It never takes a path or touches
    /// disk. Locking it discards all named secrets and destroys this session's
    /// key; unlocking starts an empty session. Callers must also clear any
    /// volatile history and never send it to their persistent stores.
    pub fn memory_only() -> Result<Self, VaultError> {
        Ok(Self {
            root: None,
            backend: Backend::Portable(Portable::memory_only()?),
        })
    }

    #[cfg(windows)]
    fn open_windows(root: &Path) -> Result<Self, VaultError> {
        if root.exists() && !root.is_dir() {
            return Err(VaultError::InvalidVault {
                reason: "vault path exists but is not a directory",
            });
        }
        fs::create_dir_all(root)?;
        // Store an absolute, canonical root so a later process-wide working
        // directory change cannot redirect secret reads or writes.
        let root = fs::canonicalize(root)?;
        let root = root.as_path();

        let secrets = root.join(SECRETS_DIR);
        if secrets.exists() && !secrets.is_dir() {
            return Err(VaultError::InvalidVault {
                reason: "vault secrets path exists but is not a directory",
            });
        }
        fs::create_dir_all(&secrets)?;

        let metadata_path = root.join(METADATA_FILE);
        if metadata_path.exists() {
            let bytes = read_bounded(&metadata_path, MAX_METADATA_BYTES)?;
            parse_windows_metadata(&bytes)?;
        } else {
            let metadata = VaultMetadata {
                format: VAULT_FORMAT_VERSION,
                backend: BACKEND_ID.to_owned(),
            };
            let bytes = serde_json::to_vec(&metadata).map_err(|_| VaultError::Internal {
                operation: "encoding vault metadata",
            })?;
            atomic_write(root, &metadata_path, &bytes)?;
        }

        Ok(Self {
            root: Some(root.to_path_buf()),
            backend: Backend::Dpapi(AtomicBool::new(false)),
        })
    }

    /// Lock the vault for this API session.
    ///
    /// Windows locking remains an API session gate, not an OS lock. Portable
    /// modes discard the cached passphrase or memory-only key; memory-only
    /// named secrets are discarded. This does not lock the native keyring
    /// or erase plaintext the caller already obtained.
    pub fn lock(&self) {
        match &self.backend {
            #[cfg(windows)]
            Backend::Dpapi(locked) => locked.store(true, Ordering::SeqCst),
            Backend::Portable(portable) => portable.lock(),
        }
    }

    /// Unlock the vault for this API session.
    ///
    /// Compatibility method for Windows callers. It does not opt into native
    /// prompts or bypass a passphrase. New integrations should call the
    /// fallible [`Self::try_unlock`] or [`Self::unlock_with_passphrase`].
    pub fn unlock(&self) {
        let _ = self.unlock_inner(false);
    }

    /// Explicit unlock action. May request native OS user authorization
    /// (perform it off the UI thread). For passphrase mode, use
    /// [`Self::unlock_with_passphrase`] instead.
    pub fn try_unlock(&self) -> Result<(), VaultError> {
        self.unlock_inner(true)
    }

    fn unlock_inner(&self, allow_prompt: bool) -> Result<(), VaultError> {
        match &self.backend {
            #[cfg(windows)]
            Backend::Dpapi(locked) => {
                locked.store(false, Ordering::SeqCst);
                Ok(())
            }
            Backend::Portable(portable) => {
                if let Some(root) = self.root.as_deref() {
                    portable.verify_header(root)?;
                }
                portable.unlock(allow_prompt)
            }
        }
    }

    /// Verify passphrase and unlock an explicitly passphrase-backed vault.
    ///
    /// The on-disk `vault.json` must byte-for-byte match the header this
    /// handle opened or last rewrote. A replaced header (even with the same
    /// public profile ID) is rejected and the handle stays locked until the
    /// original metadata is restored or the vault is explicitly reopened.
    pub fn unlock_with_passphrase(&self, passphrase: &[u8]) -> Result<(), VaultError> {
        match (&self.backend, self.root.as_deref()) {
            (Backend::Portable(portable), Some(root)) => {
                portable.unlock_passphrase(root, passphrase)
            }
            _ => Err(VaultError::WrongMode),
        }
    }

    /// Authenticate the old passphrase and atomically wrap the existing profile
    /// key with a new passphrase and salt. Payloads remain compatible. The
    /// current on-disk header must byte-for-byte match the one this handle
    /// opened or last rewrote and must wrap the same data key the session is
    /// using; a missing, replaced or same-ID-for-another-key header fails
    /// closed without overwriting it. Old backups still require their original
    /// passphrase; this does not revoke other already-unlocked sessions. Use
    /// with a single writer.
    pub fn change_passphrase(
        &self,
        old_passphrase: &[u8],
        new_passphrase: &[u8],
    ) -> Result<(), VaultError> {
        match (&self.backend, self.root.as_deref()) {
            (Backend::Portable(portable), Some(root)) => {
                portable.change_passphrase(root, old_passphrase, new_passphrase)
            }
            _ => Err(VaultError::WrongMode),
        }
    }

    /// Last-known access and validation status, without probing a keyring.
    #[must_use]
    pub fn status(&self) -> VaultStatus {
        match &self.backend {
            #[cfg(windows)]
            Backend::Dpapi(locked) => VaultStatus {
                mode: VaultMode::WindowsDpapi,
                state: if locked.load(Ordering::SeqCst) {
                    VaultState::Locked
                } else {
                    VaultState::Ready
                },
                validation: VaultValidation::NativeTested,
            },
            Backend::Portable(portable) => VaultStatus {
                mode: portable.mode,
                state: portable.state(),
                validation: match portable.mode {
                    VaultMode::MemoryOnly | VaultMode::Passphrase => {
                        VaultValidation::PortableTested
                    }
                    _ => VaultValidation::NativeUnverified,
                },
            },
        }
    }

    /// Whether locked operations will be rejected.
    #[must_use]
    pub fn is_locked(&self) -> bool {
        self.status().state != VaultState::Ready
    }

    /// Seal `plaintext` to this vault and `context`.
    ///
    /// The returned payload contains no plaintext and can be serialized by the
    /// caller. The context must be a non-secret label; see the crate docs for
    /// the grammar.
    pub fn seal(&self, plaintext: &[u8], context: &str) -> Result<SealedPayload, VaultError> {
        // Reject invalid input before touching the native keyring or cached key.
        validate_context(context)?;
        check_plaintext_size(plaintext.len())?;
        self.with_access(|cipher, _| cipher.seal(plaintext, context))
    }

    /// Open a sealed payload with the context it was sealed with.
    ///
    /// This is an inherent alias for [`OpenPayload::open`], provided so callers
    /// that do not import the trait can still open payloads.
    pub fn open_payload(
        &self,
        payload: &SealedPayload,
        context: &str,
    ) -> Result<Vec<u8>, VaultError> {
        self.open_payload_zeroizing(payload, context)
            .map(SecretBytes::into_legacy_vec)
    }

    /// Like [`Self::open_payload`], but wipes the returned buffer on drop.
    pub fn open_payload_zeroizing(
        &self,
        payload: &SealedPayload,
        context: &str,
    ) -> Result<SecretBytes, VaultError> {
        // Reject invalid input before touching the native keyring or cached key.
        validate_context(context)?;
        self.with_access(|cipher, _| cipher.open(payload, context))
            .map(SecretBytes)
    }

    /// Authenticate an externally stored payload without returning plaintext.
    pub fn verify_payload(&self, payload: &SealedPayload, context: &str) -> Result<(), VaultError> {
        self.open_payload_zeroizing(payload, context).map(drop)
    }

    /// Store or replace a secret under `alias`.
    ///
    /// Aliases match `[A-Za-z0-9._-]{1,128}`. Windows reserved device names,
    /// `.`, `..` and a trailing `.` are rejected. Resolution is case-insensitive,
    /// so `Token` replaces an existing `token` atomically. An exact-case file is
    /// preferred; if a case-sensitive filesystem contains multiple case
    /// variants, ambiguous lookups, deletes and replacements fail closed. The
    /// replacement is written to a temporary file, synced, then renamed over
    /// the previous ciphertext.
    pub fn put_secret(&self, alias: &str, secret: &[u8]) -> Result<(), VaultError> {
        // Reject invalid input before touching the native keyring or cached key.
        validate_alias(alias)?;
        check_plaintext_size(secret.len())?;
        self.with_access(|cipher, memory| {
            validate_alias(alias)?;
            check_plaintext_size(secret.len())?;
            if let Some(memory) = memory {
                return memory.put(alias, cipher.seal(secret, &secret_context(alias))?);
            }
            let existing = self.find_secret_file(alias)?;
            if existing.is_none() && self.list_secret_files()?.len() >= MAX_SECRETS {
                return Err(secret_count_limit());
            }
            let payload = cipher.seal(secret, &secret_context(alias))?;
            let target = existing.unwrap_or_else(|| self.secret_path(alias));
            write_payload(&self.secrets_dir(), &target, &payload)
        })
    }

    /// Store or replace a secret while atomically removing other aliases.
    ///
    /// This is the transactional replacement used by volatile consumers that
    /// budget the backend's encoded representation (for example a clipboard
    /// history that must evict old ciphertext to make room for a new payload).
    /// The new payload is sealed first. Only if it fits the encoded byte and
    /// secret-count budget after subtracting `remove_aliases` are the removals
    /// and the insertion committed. A quota, count, alias or cryptographic
    /// failure leaves every existing secret untouched — in particular, it does
    /// not delete the payloads named in `remove_aliases`.
    ///
    /// This operation is only available for memory-only vaults. File-backed
    /// vaults return [`VaultError::WrongMode`]; their storage is not consumed by
    /// this volatile budget and callers should use [`Self::put_secret`] plus
    /// [`Self::delete_secret`].
    pub fn put_secret_replacing(
        &self,
        alias: &str,
        secret: &[u8],
        remove_aliases: &[&str],
    ) -> Result<(), VaultError> {
        // Reject invalid input before touching the cached key or any payload.
        validate_alias(alias)?;
        check_plaintext_size(secret.len())?;
        for remove in remove_aliases {
            validate_alias(remove)?;
        }
        self.with_access(|cipher, memory| {
            validate_alias(alias)?;
            check_plaintext_size(secret.len())?;
            let Some(memory) = memory else {
                return Err(VaultError::WrongMode);
            };
            // Seal before planning the replacement so a crypto or size failure
            // cannot remove an existing alias.
            let payload = cipher.seal(secret, &secret_context(alias))?;
            memory.put_replacing(alias, payload, remove_aliases)
        })
    }

    /// Read a stored secret. Missing aliases yield
    /// [`VaultError::SecretNotFound`].
    pub fn get_secret(&self, alias: &str) -> Result<Vec<u8>, VaultError> {
        self.get_secret_zeroizing(alias)
            .map(SecretBytes::into_legacy_vec)
    }

    /// Like [`Self::get_secret`], but wipes the returned buffer on drop.
    pub fn get_secret_zeroizing(&self, alias: &str) -> Result<SecretBytes, VaultError> {
        // Reject invalid input before touching the native keyring or cached key.
        validate_alias(alias)?;
        self.with_access(|cipher, memory| {
            validate_alias(alias)?;
            let context = secret_context(alias);
            if let Some(memory) = memory {
                return cipher.open(memory.get(alias)?, &context);
            }
            let path = self
                .find_secret_file(alias)?
                .ok_or(VaultError::SecretNotFound)?;
            cipher.open(&read_payload(&path)?, &context)
        })
        .map(SecretBytes)
    }

    /// Delete a stored secret, returning `true` if a file was removed.
    ///
    /// Deleting a missing alias is not an error, which makes cleanup
    /// idempotent.
    pub fn delete_secret(&self, alias: &str) -> Result<bool, VaultError> {
        // Reject invalid input before touching the native keyring or cached key.
        validate_alias(alias)?;
        let directory = self.root.as_ref().map(|_| self.secrets_dir());
        self.with_access(|_, memory| {
            validate_alias(alias)?;
            if let Some(memory) = memory {
                return Ok(memory.delete(alias));
            }
            let Some(path) = self.find_secret_file(alias)? else {
                return Ok(false);
            };
            fs::remove_file(path)?;
            if let Some(directory) = directory {
                sync_directory(&directory);
            }
            Ok(true)
        })
    }

    /// List stored aliases in deterministic sorted order.
    ///
    /// Only files ending in `.sealed` with a valid alias stem are returned;
    /// temporary and unrelated files are ignored.
    pub fn list_secrets(&self) -> Result<Vec<String>, VaultError> {
        self.with_access(|_, memory| match memory {
            Some(memory) => Ok(memory.aliases()),
            None => self.list_secret_files(),
        })
    }

    fn list_secret_files(&self) -> Result<Vec<String>, VaultError> {
        let mut aliases = Vec::new();
        let mut scanned = 0usize;
        for entry in fs::read_dir(self.secrets_dir())? {
            scanned += 1;
            if scanned > MAX_DIRECTORY_ENTRIES {
                return Err(VaultError::LimitExceeded {
                    kind: "vault directory entries",
                    limit: MAX_DIRECTORY_ENTRIES,
                });
            }
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            let Some(alias) = file_name.strip_suffix(SECRET_FILE_SUFFIX) else {
                continue;
            };
            if is_valid_alias(alias) {
                aliases.push(alias.to_owned());
                if aliases.len() > MAX_SECRETS {
                    return Err(secret_count_limit());
                }
            }
        }
        aliases.sort();
        Ok(aliases)
    }

    /// Re-seal every named secret under the current vault key.
    ///
    /// This verifies that the on-disk header is still the one this handle
    /// opened, then does not replace the profile key: externally stored
    /// payloads and older backups remain accessible. If interrupted, each file
    /// is still authenticated ciphertext under the same key, old or re-sealed.
    pub fn rotate(&self) -> Result<RotationReport, VaultError> {
        self.with_access(|cipher, memory| {
            let count;
            if let Some(memory) = memory {
                let aliases = memory.aliases();
                count = aliases.len();
                for alias in aliases {
                    let context = secret_context(&alias);
                    let plaintext = cipher.open(memory.get(&alias)?, &context)?;
                    memory.put(&alias, cipher.seal(&plaintext, &context)?)?;
                }
            } else {
                let aliases = self.list_secret_files()?;
                count = aliases.len();
                for alias in aliases {
                    let path = self.secret_path(&alias);
                    let context = secret_context(&alias);
                    let plaintext = cipher.open(&read_payload(&path)?, &context)?;
                    let replacement = cipher.seal(&plaintext, &context)?;
                    write_payload(&self.secrets_dir(), &path, &replacement)?;
                }
            }
            Ok(RotationReport {
                secrets_resealed: count,
                os_key_rotation_performed: false,
            })
        })
    }

    /// Authenticate each named secret after making or restoring a consistent
    /// vault directory copy. Verify external ciphertext with [`Self::verify_payload`].
    /// Memory-only storage has no backup.
    ///
    /// For format-2 (Keychain, Secret Service and passphrase) vaults this also
    /// requires the current `vault.json` to be byte-for-byte identical to the
    /// metadata this handle opened, so a missing or replaced header is not
    /// reported as a good backup. For Windows DPAPI it validates the legacy
    /// format/backend metadata. Reopen a vault after replacing its directory
    /// in place. This does not detect missing secret files; the backup owner
    /// must independently check its expected inventory.
    pub fn verify_backup(&self) -> Result<VerificationReport, VaultError> {
        self.with_access(|cipher, memory| {
            if memory.is_some() {
                return Err(VaultError::WrongMode);
            }
            #[cfg(windows)]
            {
                if let Backend::Dpapi(_) = &self.backend {
                    let root = self.root.as_deref().ok_or(VaultError::WrongMode)?;
                    verify_windows_metadata(root)?;
                }
            }
            let aliases = self.list_secret_files()?;
            for alias in &aliases {
                let payload = read_payload(&self.secret_path(alias))?;
                cipher.open(&payload, &secret_context(alias))?;
            }
            Ok(VerificationReport {
                secrets_verified: aliases.len(),
            })
        })
    }

    fn with_access<T>(
        &self,
        f: impl FnOnce(Cipher<'_>, Option<&mut MemoryStore>) -> Result<T, VaultError>,
    ) -> Result<T, VaultError> {
        match &self.backend {
            #[cfg(windows)]
            Backend::Dpapi(locked) => {
                if locked.load(Ordering::SeqCst) {
                    return Err(VaultError::Locked);
                }
                f(Cipher::Dpapi, None)
            }
            Backend::Portable(portable) => {
                // A format-2 handle is bound to the metadata it opened. Do not
                // keep writing ciphertext under a stale key/profile after the
                // header was replaced or removed; the caller must reopen.
                if portable.state() == VaultState::Ready
                    && let Some(root) = self.root.as_deref()
                {
                    portable.verify_header(root)?;
                }
                portable.with_key(|key, memory| f(Cipher::Aead(key, &portable.id), memory))
            }
        }
    }

    fn secrets_dir(&self) -> PathBuf {
        self.root
            .as_ref()
            .expect("file-backed vault")
            .join(SECRETS_DIR)
    }

    fn secret_path(&self, alias: &str) -> PathBuf {
        self.secrets_dir()
            .join(format!("{}{}", alias, SECRET_FILE_SUFFIX))
    }

    /// Find an existing secret file without assuming the case Windows uses.
    ///
    /// An exact-case file always wins. If only one case-insensitive match
    /// exists it is used, preserving Windows-like replace semantics on
    /// case-sensitive filesystems. Multiple case-variant files are ambiguous
    /// and fail closed so a delete or replace cannot silently pick the wrong
    /// alias.
    fn find_secret_file(&self, alias: &str) -> Result<Option<PathBuf>, VaultError> {
        let mut scanned = 0usize;
        let mut fallback = None;
        let mut fallback_count = 0usize;
        for entry in fs::read_dir(self.secrets_dir())? {
            scanned += 1;
            if scanned > MAX_DIRECTORY_ENTRIES {
                return Err(VaultError::LimitExceeded {
                    kind: "vault directory entries",
                    limit: MAX_DIRECTORY_ENTRIES,
                });
            }
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            let Some(stem) = file_name.strip_suffix(SECRET_FILE_SUFFIX) else {
                continue;
            };
            if stem == alias {
                return Ok(Some(entry.path()));
            }
            if stem.eq_ignore_ascii_case(alias) {
                fallback_count += 1;
                if fallback.is_none() {
                    fallback = Some(entry.path());
                }
            }
        }
        match fallback_count {
            0 => Ok(None),
            1 => Ok(fallback),
            _ => Err(VaultError::InvalidVault {
                reason: "multiple case-variant secret files exist for this alias",
            }),
        }
    }
}

impl Cipher<'_> {
    fn version(&self) -> u16 {
        match self {
            #[cfg(windows)]
            Self::Dpapi => PAYLOAD_VERSION,
            Self::Aead(..) => AEAD_PAYLOAD_VERSION,
        }
    }

    fn seal(&self, plaintext: &[u8], context: &str) -> Result<SealedPayload, VaultError> {
        validate_context(context)?;
        check_plaintext_size(plaintext.len())?;
        let ciphertext = match self {
            #[cfg(windows)]
            Self::Dpapi => dpapi::protect(plaintext, &entropy_for(context))?,
            Self::Aead(key, id) => crypto::seal(
                key,
                plaintext,
                &crypto::aad(AEAD_CONTEXT_DOMAIN, id, context.as_bytes()),
            )?,
        };
        if ciphertext.len().saturating_mul(2) > MAX_SEALED_FILE_BYTES {
            return Err(sealed_limit(MAX_SEALED_FILE_BYTES));
        }
        Ok(SealedPayload {
            version: self.version(),
            context_tag: context.to_owned(),
            ciphertext_hex: hex::encode(&ciphertext),
        })
    }

    fn open(
        &self,
        payload: &SealedPayload,
        context: &str,
    ) -> Result<Zeroizing<Vec<u8>>, VaultError> {
        validate_context(context)?;
        if payload.version != self.version() {
            return Err(VaultError::UnsupportedPayloadVersion {
                found: payload.version,
                supported: self.version(),
            });
        }
        if payload.context_tag != context {
            return Err(VaultError::WrongContext);
        }
        let frame_limit = match self {
            #[cfg(windows)]
            Self::Dpapi => MAX_SEALED_FILE_BYTES,
            Self::Aead(..) => MAX_PLAINTEXT_BYTES + crypto::OVERHEAD,
        };
        if payload.ciphertext_hex.len() > 2 * frame_limit {
            return Err(sealed_limit(frame_limit));
        }
        let ciphertext =
            hex::decode(&payload.ciphertext_hex).ok_or(VaultError::InvalidPayload {
                reason: "ciphertext is not valid hexadecimal",
            })?;
        let plaintext = match self {
            #[cfg(windows)]
            Self::Dpapi => Zeroizing::new(dpapi::unprotect(&ciphertext, &entropy_for(context))?),
            Self::Aead(key, id) => crypto::open(
                key,
                &ciphertext,
                &crypto::aad(AEAD_CONTEXT_DOMAIN, id, context.as_bytes()),
            )?,
        };
        check_plaintext_size(plaintext.len())?;
        Ok(plaintext)
    }
}

#[derive(Default)]
struct MemoryStore {
    secrets: BTreeMap<String, (String, SealedPayload)>,
    bytes: usize,
}

impl MemoryStore {
    fn put(&mut self, alias: &str, payload: SealedPayload) -> Result<(), VaultError> {
        self.put_replacing(alias, payload, &[])
    }

    /// Insert or replace one payload while removing other aliases, with no
    /// partial mutation.
    ///
    /// The post-operation byte total and entry count are computed from the
    /// existing map before anything is changed. A rejection therefore leaves
    /// every stored payload and the byte counter exactly as they were.
    fn put_replacing(
        &mut self,
        alias: &str,
        payload: SealedPayload,
        remove_aliases: &[&str],
    ) -> Result<(), VaultError> {
        let target = alias.to_ascii_lowercase();
        let prior_alias = self.secrets.get(&target).map(|(stored, _)| stored.clone());

        // Count each distinct existing removal key once. The target alias is
        // skipped here because its old payload is replaced by the branch below;
        // this also means repeating or case-varying a removal is harmless.
        let mut removed = BTreeSet::new();
        let mut bytes = self.bytes;
        let mut count = self.secrets.len();
        for remove in remove_aliases {
            let key = remove.to_ascii_lowercase();
            if key == target || !removed.insert(key.clone()) {
                continue;
            }
            if let Some((stored_alias, payload)) = self.secrets.get(&key) {
                bytes -= Self::size(stored_alias, payload);
                count -= 1;
            }
        }
        match self.secrets.get(&target) {
            Some((stored_alias, previous)) => {
                bytes = bytes - Self::size(stored_alias, previous) + Self::size(alias, &payload);
            }
            None => {
                bytes += Self::size(alias, &payload);
                count += 1;
            }
        }
        if count > MAX_SECRETS {
            return Err(secret_count_limit());
        }
        if bytes > MAX_MEMORY_SECRET_BYTES {
            return Err(VaultError::LimitExceeded {
                kind: "memory-only encoded secret bytes",
                limit: MAX_MEMORY_SECRET_BYTES,
            });
        }

        for key in removed {
            self.secrets.remove(&key);
        }
        let stored_alias = prior_alias.unwrap_or_else(|| alias.to_owned());
        self.secrets.insert(target, (stored_alias, payload));
        self.bytes = bytes;
        Ok(())
    }

    fn size(alias: &str, payload: &SealedPayload) -> usize {
        2 * alias.len() + payload.context_tag.len() + payload.ciphertext_hex.len()
    }

    fn get(&self, alias: &str) -> Result<&SealedPayload, VaultError> {
        self.secrets
            .get(&alias.to_ascii_lowercase())
            .map(|(_, payload)| payload)
            .ok_or(VaultError::SecretNotFound)
    }

    fn delete(&mut self, alias: &str) -> bool {
        if let Some((alias, payload)) = self.secrets.remove(&alias.to_ascii_lowercase()) {
            self.bytes -= Self::size(&alias, &payload);
            true
        } else {
            false
        }
    }

    fn aliases(&self) -> Vec<String> {
        let mut aliases: Vec<_> = self
            .secrets
            .values()
            .map(|(alias, _)| alias.clone())
            .collect();
        aliases.sort();
        aliases
    }
}

fn check_plaintext_size(size: usize) -> Result<(), VaultError> {
    if size > MAX_PLAINTEXT_BYTES {
        return Err(VaultError::LimitExceeded {
            kind: "plaintext",
            limit: MAX_PLAINTEXT_BYTES,
        });
    }
    Ok(())
}

fn secret_count_limit() -> VaultError {
    VaultError::LimitExceeded {
        kind: "stored secrets",
        limit: MAX_SECRETS,
    }
}

fn sealed_limit(limit: usize) -> VaultError {
    VaultError::LimitExceeded {
        kind: "sealed payload",
        limit,
    }
}

fn read_payload(path: &Path) -> Result<SealedPayload, VaultError> {
    let bytes = read_bounded(path, MAX_SEALED_FILE_BYTES)?;
    serde_json::from_slice(&bytes).map_err(|_| VaultError::InvalidPayload {
        reason: "stored secret is not a valid sealed payload",
    })
}

fn write_payload(
    directory: &Path,
    target: &Path,
    payload: &SealedPayload,
) -> Result<(), VaultError> {
    let encoded = serde_json::to_vec(payload).map_err(|_| VaultError::Internal {
        operation: "encoding sealed payload",
    })?;
    if encoded.len() > MAX_SEALED_FILE_BYTES {
        return Err(sealed_limit(MAX_SEALED_FILE_BYTES));
    }
    atomic_write(directory, target, &encoded)
}

/// Payload opening is exposed as a trait method because Rust does not permit an
/// inherent associated function (`Vault::open`) and an inherent method with the
/// same name. Import this trait to call `vault.open(&payload, context)`; the
/// inherent [`Vault::open_payload`] method is an equivalent convenience.
pub trait OpenPayload {
    /// Open a sealed payload with the context it was sealed with.
    ///
    /// A different context, a modified payload or a payload sealed for another
    /// user/profile fails without returning plaintext.
    fn open(&self, payload: &SealedPayload, context: &str) -> Result<Vec<u8>, VaultError>;
}

impl OpenPayload for Vault {
    fn open(&self, payload: &SealedPayload, context: &str) -> Result<Vec<u8>, VaultError> {
        self.open_payload(payload, context)
    }
}

#[cfg(windows)]
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VaultMetadata {
    format: u32,
    backend: String,
}

#[cfg(windows)]
fn parse_windows_metadata(bytes: &[u8]) -> Result<VaultMetadata, VaultError> {
    let metadata: VaultMetadata =
        serde_json::from_slice(bytes).map_err(|_| VaultError::InvalidVault {
            reason: "vault metadata is not valid JSON",
        })?;
    if metadata.format != VAULT_FORMAT_VERSION {
        return Err(VaultError::UnsupportedVaultFormat {
            found: metadata.format,
            supported: VAULT_FORMAT_VERSION,
        });
    }
    if metadata.backend != BACKEND_ID {
        return Err(VaultError::InvalidVault {
            reason: "vault metadata names an unsupported backend",
        });
    }
    Ok(metadata)
}

#[cfg(windows)]
fn verify_windows_metadata(root: &Path) -> Result<(), VaultError> {
    let path = root.join(METADATA_FILE);
    let bytes = match read_bounded(&path, MAX_METADATA_BYTES) {
        Ok(bytes) => bytes,
        Err(VaultError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(VaultError::MetadataMissing);
        }
        Err(error) => return Err(error),
    };
    parse_windows_metadata(&bytes)?;
    Ok(())
}

fn validate_alias(alias: &str) -> Result<(), VaultError> {
    if !is_valid_alias(alias) {
        return Err(VaultError::InvalidAlias {
            reason: "alias must be 1..=128 bytes of A-Z, a-z, 0-9, '.', '_' or '-', \
                     must not be '.' or '..', must not end with '.', and must not be a \
                     Windows reserved device name",
        });
    }
    Ok(())
}

fn is_valid_alias(alias: &str) -> bool {
    let bytes = alias.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_ALIAS_BYTES {
        return false;
    }
    if !bytes
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return false;
    }
    if alias == "." || alias == ".." || alias.ends_with('.') {
        return false;
    }
    !is_reserved_device_name(alias)
}

fn is_reserved_device_name(alias: &str) -> bool {
    let base = alias.split('.').next().unwrap_or(alias);
    let upper = base.to_ascii_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    let bytes = upper.as_bytes();
    bytes.len() == 4
        && (upper.starts_with("COM") || upper.starts_with("LPT"))
        && bytes[3].is_ascii_digit()
        && bytes[3] != b'0'
}

fn validate_context(context: &str) -> Result<(), VaultError> {
    if context.is_empty() || context.len() > MAX_CONTEXT_BYTES {
        return Err(VaultError::InvalidContext {
            reason: "context must be 1..=160 bytes",
        });
    }
    if !context.as_bytes().iter().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(byte, b'.' | b'_' | b'-' | b':' | b'/' | b'+' | b'@')
    }) {
        return Err(VaultError::InvalidContext {
            reason: "context must be printable ASCII letters, digits, '.', '_', '-', ':', '/', '+' or '@'",
        });
    }
    Ok(())
}

fn secret_context(alias: &str) -> String {
    format!("secret:{}", alias.to_ascii_lowercase())
}

#[cfg(windows)]
fn entropy_for(context: &str) -> Vec<u8> {
    let mut entropy = Vec::with_capacity(CONTEXT_DOMAIN.len() + context.len());
    entropy.extend_from_slice(CONTEXT_DOMAIN);
    entropy.extend_from_slice(context.as_bytes());
    entropy
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, VaultError> {
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(VaultError::InvalidVault {
            reason: "vault entry is not a regular file",
        });
    }
    let file = File::open(path)?;
    let length = file.metadata()?.len();
    if length > limit as u64 {
        return Err(VaultError::LimitExceeded {
            kind: "file size",
            limit,
        });
    }

    let mut bytes = Vec::with_capacity(length as usize);
    let mut limited = file.take(limit as u64 + 1);
    limited.read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(VaultError::LimitExceeded {
            kind: "file size",
            limit,
        });
    }
    Ok(bytes)
}

fn atomic_write(directory: &Path, target: &Path, bytes: &[u8]) -> Result<(), VaultError> {
    commit_file(directory, target, bytes, false)
}

fn atomic_create(directory: &Path, target: &Path, bytes: &[u8]) -> Result<(), VaultError> {
    commit_file(directory, target, bytes, true)
}

fn commit_file(
    directory: &Path,
    target: &Path,
    bytes: &[u8],
    create: bool,
) -> Result<(), VaultError> {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nonce = hex::encode(&crypto::random::<8>()?);
    let temp = directory.join(format!(
        "{TEMP_PREFIX}{}-{counter}-{nonce}",
        std::process::id()
    ));

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    let published = match (written, create) {
        (Ok(()), true) => fs::hard_link(&temp, target),
        (Ok(()), false) => fs::rename(&temp, target),
        (Err(error), _) => Err(error),
    };
    if create || published.is_err() {
        let _ = fs::remove_file(&temp);
    }
    if let Err(error) = published {
        return Err(
            if create && error.kind() == std::io::ErrorKind::AlreadyExists {
                VaultError::Busy
            } else {
                error.into()
            },
        );
    }
    // The file is already published. Directory sync is a best-effort
    // durability hint; macOS and some filesystems reject it (EINVAL/ENOTSUP).
    // Never turn a successful commit into an error at this point.
    sync_directory(directory);
    Ok(())
}

fn directory_builder(recursive: bool) -> fs::DirBuilder {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(recursive);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
}

pub(crate) fn create_directory_all(path: &Path) -> Result<(), VaultError> {
    directory_builder(true).create(path)?;
    Ok(())
}

pub(crate) fn create_directory(path: &Path) -> std::io::Result<()> {
    directory_builder(false).create(path)
}

/// Best-effort directory-entry sync. Errors are intentionally ignored: the
/// payload or metadata file is already committed and some supported systems
/// do not implement directory fsync.
pub(crate) fn sync_directory(directory: &Path) {
    #[cfg(unix)]
    {
        if let Ok(directory) = File::open(directory) {
            let _ = directory.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = directory;
}

#[cfg(test)]
mod tests {
    use super::{is_valid_alias, secret_context, validate_context};

    #[test]
    fn alias_grammar_rejects_traversal_and_devices() {
        assert!(is_valid_alias("token"));
        assert!(is_valid_alias("a.b_c-d"));
        assert!(is_valid_alias(".hidden"));
        assert!(!is_valid_alias(""));
        assert!(!is_valid_alias("."));
        assert!(!is_valid_alias(".."));
        assert!(!is_valid_alias("../escape"));
        assert!(!is_valid_alias("a/b"));
        assert!(!is_valid_alias("a\\b"));
        assert!(!is_valid_alias("bad alias"));
        assert!(!is_valid_alias("trailing."));
        assert!(!is_valid_alias("CON"));
        assert!(!is_valid_alias("nul.txt"));
        assert!(!is_valid_alias("COM1"));
        assert!(!is_valid_alias("lpt9.log"));
    }

    #[test]
    fn context_grammar_is_bounded_and_ascii() {
        assert!(validate_context("clipboard.text").is_ok());
        assert!(validate_context("secret:a-b_c.d").is_ok());
        assert!(validate_context("").is_err());
        assert!(validate_context("bad context").is_err());
        assert!(validate_context("bad\ncontext").is_err());
        assert!(validate_context(&"a".repeat(161)).is_err());
        assert_eq!(secret_context("Token"), "secret:token");
    }
}
