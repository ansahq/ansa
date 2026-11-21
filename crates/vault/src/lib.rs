//! Windows DPAPI-backed secret sealing and encrypted payload storage.
//!
//! # What this crate is
//!
//! `ansa-vault` stores sensitive bytes as opaque, user-bound ciphertext. On
//! Windows the only sealing backend is DPAPI through `CryptProtectData` and
//! `CryptUnprotectData`, called with `CRYPTPROTECT_UI_FORBIDDEN` and a
//! per-purpose entropy blob derived from a validated context label. No
//! home-grown cipher, no key file and no plaintext fallback exists.
//!
//! A vault is a directory:
//!
//! ```text
//! <vault>/vault.json                 non-sensitive format/backend metadata
//! <vault>/secrets/<alias>.sealed     JSON envelope with DPAPI ciphertext
//! ```
//!
//! Files contain only the metadata and the hex-encoded ciphertext. Secret file
//! names are derived from validated aliases; temporary files are ignored and
//! replaced atomically with `write + sync_all + rename`.
//!
//! # Honest security boundaries
//!
//! * **DPAPI is user-bound, not passphrase-bound.** Any process running as the
//!   same Windows user can call DPAPI and unprotect the same data. [`Vault::lock`]
//!   is a session gate for this API; it does not add a password, erase the OS
//!   key, or prevent another same-user process from reading the ciphertext.
//! * **No backup/migration support.** DPAPI protects data for the current user
//!   profile. Copying the vault to another user account or machine normally
//!   makes decryption fail; the payload then remains an unrecoverable opaque
//!   blob rather than turning into plaintext. Roaming-profile/credential backup
//!   scenarios are an explicit future design, not a claim of this crate.
//! * **Key rotation is OS-managed.** Windows owns the DPAPI master keys.
//!   [`Vault::rotate`] re-protects every stored secret under the current DPAPI
//!   key, but it cannot rotate Windows' own master keys.
//! * **Non-Windows is unsupported and visible.** [`Vault::open`] returns
//!   [`VaultError::Unsupported`] before creating files. No plaintext fallback
//!   exists, and no other cipher is substituted.
//! * **Tampering and wrong context fail closed.** DPAPI authenticates its
//!   output, and the context is bound through the entropy blob. A modified
//!   payload, a different context, another user or another machine yields
//!   [`VaultError::DecryptionFailed`]; no plaintext is returned.
//! * **Context labels are not secrets.** A context is a bounded ASCII label
//!   such as `clipboard.text`; it is stored in the payload's `context_tag`.
//!   Callers must not put secret material in a context.
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

mod error;
mod hex;

#[cfg(windows)]
#[path = "dpapi_windows.rs"]
mod dpapi;
#[cfg(not(windows))]
#[path = "dpapi_unsupported.rs"]
mod dpapi;

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

pub use crate::error::VaultError;

/// Version written into every [`SealedPayload`] produced by this build.
pub const PAYLOAD_VERSION: u16 = 1;

/// Version written into the vault metadata file.
pub const VAULT_FORMAT_VERSION: u32 = 1;

/// Backend identifier stored in vault metadata.
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

#[cfg(windows)]
const METADATA_FILE: &str = "vault.json";
#[cfg(windows)]
const MAX_METADATA_BYTES: usize = 4096;

const SECRETS_DIR: &str = "secrets";
const SECRET_FILE_SUFFIX: &str = ".sealed";
const TEMP_PREFIX: &str = ".tmp-";
const CONTEXT_DOMAIN: &[u8] = b"ansa-vault.dpapi.v1\0";

/// Bound on raw directory entries inspected by one list/lookup operation.
/// This keeps a directory filled with unrelated files from making an
/// individual operation unbounded.
const MAX_DIRECTORY_ENTRIES: usize = MAX_SECRETS + 1024;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A serde-serializable sealed payload.
///
/// The ciphertext is opaque DPAPI output; `context_tag` is the validated,
/// non-secret context label used for binding and diagnostics. `Debug` omits
/// the ciphertext body.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedPayload {
    /// Payload format version. This build writes [`PAYLOAD_VERSION`].
    pub version: u16,
    /// The validated, non-secret context label the payload was bound to.
    pub context_tag: String,
    /// Lowercase hexadecimal encoding of the DPAPI ciphertext.
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
    /// Number of secrets successfully re-protected with the current DPAPI key.
    pub secrets_resealed: usize,
    /// Whether Windows DPAPI master keys were rotated. Always `false`: those
    /// keys are OS-managed, and this routine only re-seals payloads.
    pub os_key_rotation_performed: bool,
}

/// A DPAPI-backed vault directory.
///
/// `Vault` is safe to share behind a reference; the lock state is atomic.
/// Cloning is intentionally not implemented so a lock cannot be bypassed with
/// stale copies.
pub struct Vault {
    root: PathBuf,
    locked: AtomicBool,
}

impl fmt::Debug for Vault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Vault")
            .field("root", &self.root)
            .field("backend", &BACKEND_ID)
            .field("locked", &self.is_locked())
            .finish()
    }
}

impl Vault {
    /// Open (or create) a vault directory.
    ///
    /// On Windows this creates the directory, the `secrets` subdirectory and a
    /// bounded, non-sensitive `vault.json` metadata file if they are absent.
    /// On other platforms this returns [`VaultError::Unsupported`] before
    /// touching the filesystem. An existing incompatible metadata file is
    /// rejected rather than rewritten.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, VaultError> {
        let root = dir.as_ref();
        #[cfg(windows)]
        {
            Self::open_windows(root)
        }
        #[cfg(not(windows))]
        {
            let _ = root;
            Err(VaultError::Unsupported {
                platform: std::env::consts::OS,
                reason: "only Windows DPAPI is implemented; no plaintext fallback exists",
            })
        }
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
            let metadata: VaultMetadata =
                serde_json::from_slice(&bytes).map_err(|_| VaultError::InvalidVault {
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
            root: root.to_path_buf(),
            locked: AtomicBool::new(false),
        })
    }

    /// Lock the vault for this API session.
    ///
    /// This is only a session gate. It is not a passphrase prompt, does not
    /// clear DPAPI keys and cannot stop another process running as the same
    /// Windows user from decrypting the same files.
    pub fn lock(&self) {
        self.locked.store(true, Ordering::SeqCst);
    }

    /// Unlock the vault for this API session.
    ///
    /// No credential is requested or verified because DPAPI is already bound
    /// to the Windows user. See [`Vault::lock`] for the honest boundary.
    pub fn unlock(&self) {
        self.locked.store(false, Ordering::SeqCst);
    }

    /// Whether locked operations will be rejected.
    #[must_use]
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::SeqCst)
    }

    /// Seal `plaintext` to the current Windows user, bound to `context`.
    ///
    /// The returned payload contains no plaintext and can be serialized by the
    /// caller. The context must be a non-secret label; see the crate docs for
    /// the grammar.
    pub fn seal(&self, plaintext: &[u8], context: &str) -> Result<SealedPayload, VaultError> {
        self.ensure_unlocked()?;
        validate_context(context)?;
        if plaintext.len() > MAX_PLAINTEXT_BYTES {
            return Err(VaultError::LimitExceeded {
                kind: "plaintext",
                limit: MAX_PLAINTEXT_BYTES,
            });
        }

        let entropy = entropy_for(context);
        let ciphertext = dpapi::protect(plaintext, &entropy)?;
        if ciphertext.len().saturating_mul(2) > MAX_SEALED_FILE_BYTES {
            return Err(VaultError::LimitExceeded {
                kind: "sealed payload",
                limit: MAX_SEALED_FILE_BYTES,
            });
        }

        Ok(SealedPayload {
            version: PAYLOAD_VERSION,
            context_tag: context.to_owned(),
            ciphertext_hex: hex::encode(&ciphertext),
        })
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
        self.ensure_unlocked()?;
        validate_context(context)?;

        if payload.version != PAYLOAD_VERSION {
            return Err(VaultError::UnsupportedPayloadVersion {
                found: payload.version,
                supported: PAYLOAD_VERSION,
            });
        }
        if payload.context_tag != context {
            return Err(VaultError::WrongContext);
        }
        if payload.ciphertext_hex.len() > MAX_SEALED_FILE_BYTES.saturating_mul(2) {
            return Err(VaultError::LimitExceeded {
                kind: "sealed payload",
                limit: MAX_SEALED_FILE_BYTES,
            });
        }

        let ciphertext =
            hex::decode(&payload.ciphertext_hex).ok_or(VaultError::InvalidPayload {
                reason: "ciphertext is not valid hexadecimal",
            })?;
        if ciphertext.len() > MAX_SEALED_FILE_BYTES {
            return Err(VaultError::LimitExceeded {
                kind: "sealed payload",
                limit: MAX_SEALED_FILE_BYTES,
            });
        }

        let entropy = entropy_for(context);
        let plaintext = dpapi::unprotect(&ciphertext, &entropy)?;
        if plaintext.len() > MAX_PLAINTEXT_BYTES {
            return Err(VaultError::LimitExceeded {
                kind: "plaintext",
                limit: MAX_PLAINTEXT_BYTES,
            });
        }
        Ok(plaintext)
    }

    /// Store or replace a secret under `alias`.
    ///
    /// Aliases match `[A-Za-z0-9._-]{1,128}`. Windows reserved device names,
    /// `.`, `..` and a trailing `.` are rejected. Resolution is case-insensitive
    /// on Windows, so `Token` replaces an existing `token` atomically. The
    /// replacement is written to a temporary file, synced, then renamed over
    /// the previous ciphertext.
    pub fn put_secret(&self, alias: &str, secret: &[u8]) -> Result<(), VaultError> {
        self.ensure_unlocked()?;
        validate_alias(alias)?;
        if secret.len() > MAX_PLAINTEXT_BYTES {
            return Err(VaultError::LimitExceeded {
                kind: "plaintext",
                limit: MAX_PLAINTEXT_BYTES,
            });
        }

        let existing = self.find_secret_file(alias)?;
        if existing.is_none() && self.list_secrets()?.len() >= MAX_SECRETS {
            return Err(VaultError::LimitExceeded {
                kind: "stored secrets",
                limit: MAX_SECRETS,
            });
        }

        let payload = self.seal(secret, &secret_context(alias))?;
        let encoded = serde_json::to_vec(&payload).map_err(|_| VaultError::Internal {
            operation: "encoding sealed payload",
        })?;
        if encoded.len() > MAX_SEALED_FILE_BYTES {
            return Err(VaultError::LimitExceeded {
                kind: "sealed payload",
                limit: MAX_SEALED_FILE_BYTES,
            });
        }

        let target = existing.unwrap_or_else(|| self.secret_path(alias));
        atomic_write(&self.secrets_dir(), &target, &encoded)
    }

    /// Read a stored secret. Missing aliases yield
    /// [`VaultError::SecretNotFound`].
    pub fn get_secret(&self, alias: &str) -> Result<Vec<u8>, VaultError> {
        self.ensure_unlocked()?;
        validate_alias(alias)?;

        let path = self
            .find_secret_file(alias)?
            .ok_or(VaultError::SecretNotFound)?;
        let bytes = read_bounded(&path, MAX_SEALED_FILE_BYTES)?;
        let payload: SealedPayload =
            serde_json::from_slice(&bytes).map_err(|_| VaultError::InvalidPayload {
                reason: "stored secret is not a valid sealed payload",
            })?;
        self.open_payload(&payload, &secret_context(alias))
    }

    /// Delete a stored secret, returning `true` if a file was removed.
    ///
    /// Deleting a missing alias is not an error, which makes cleanup
    /// idempotent.
    pub fn delete_secret(&self, alias: &str) -> Result<bool, VaultError> {
        self.ensure_unlocked()?;
        validate_alias(alias)?;

        let Some(path) = self.find_secret_file(alias)? else {
            return Ok(false);
        };
        fs::remove_file(path)?;
        Ok(true)
    }

    /// List stored aliases in deterministic sorted order.
    ///
    /// Only files ending in `.sealed` with a valid alias stem are returned;
    /// temporary and unrelated files are ignored.
    pub fn list_secrets(&self) -> Result<Vec<String>, VaultError> {
        self.ensure_unlocked()?;

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
                    return Err(VaultError::LimitExceeded {
                        kind: "stored secrets",
                        limit: MAX_SECRETS,
                    });
                }
            }
        }
        aliases.sort();
        Ok(aliases)
    }

    /// Re-seal every stored secret under the current DPAPI key.
    ///
    /// Windows owns the DPAPI master keys, so this cannot rotate them. What it
    /// does do is decrypt each secret and protect it again with fresh DPAPI
    /// output, writing each replacement atomically. If rotation stops on an
    /// error, earlier secrets are already re-sealed, the failing secret is
    /// unchanged and later secrets keep their previous ciphertext; no secret is
    /// deleted or replaced with plaintext.
    pub fn rotate(&self) -> Result<RotationReport, VaultError> {
        self.ensure_unlocked()?;

        let aliases = self.list_secrets()?;
        let mut resealed = 0usize;
        for alias in aliases {
            let path = self
                .find_secret_file(&alias)?
                .ok_or(VaultError::SecretNotFound)?;
            let bytes = read_bounded(&path, MAX_SEALED_FILE_BYTES)?;
            let payload: SealedPayload =
                serde_json::from_slice(&bytes).map_err(|_| VaultError::InvalidPayload {
                    reason: "stored secret is not a valid sealed payload",
                })?;
            let context = secret_context(&alias);

            let mut plaintext = self.open_payload(&payload, &context)?;
            let replacement = self.seal(&plaintext, &context)?;
            wipe(&mut plaintext);

            let encoded = serde_json::to_vec(&replacement).map_err(|_| VaultError::Internal {
                operation: "encoding sealed payload",
            })?;
            if encoded.len() > MAX_SEALED_FILE_BYTES {
                return Err(VaultError::LimitExceeded {
                    kind: "sealed payload",
                    limit: MAX_SEALED_FILE_BYTES,
                });
            }
            atomic_write(&self.secrets_dir(), &path, &encoded)?;
            resealed += 1;
        }

        Ok(RotationReport {
            secrets_resealed: resealed,
            os_key_rotation_performed: false,
        })
    }

    fn ensure_unlocked(&self) -> Result<(), VaultError> {
        if self.is_locked() {
            Err(VaultError::Locked)
        } else {
            Ok(())
        }
    }

    fn secrets_dir(&self) -> PathBuf {
        self.root.join(SECRETS_DIR)
    }

    fn secret_path(&self, alias: &str) -> PathBuf {
        self.secrets_dir()
            .join(format!("{}{}", alias, SECRET_FILE_SUFFIX))
    }

    /// Find an existing secret file without assuming the case Windows uses.
    fn find_secret_file(&self, alias: &str) -> Result<Option<PathBuf>, VaultError> {
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
            let Some(stem) = file_name.strip_suffix(SECRET_FILE_SUFFIX) else {
                continue;
            };
            if stem.eq_ignore_ascii_case(alias) {
                return Ok(Some(entry.path()));
            }
        }
        Ok(None)
    }
}

/// Payload opening is exposed as a trait method because Rust does not permit an
/// inherent associated function (`Vault::open`) and an inherent method with the
/// same name. Import this trait to call `vault.open(&payload, context)`; the
/// inherent [`Vault::open_payload`] method is an equivalent convenience.
pub trait OpenPayload {
    /// Open a sealed payload with the context it was sealed with.
    ///
    /// A different context, a modified payload or a payload sealed for another
    /// Windows user fails without returning plaintext.
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

fn entropy_for(context: &str) -> Vec<u8> {
    let mut entropy = Vec::with_capacity(CONTEXT_DOMAIN.len() + context.len());
    entropy.extend_from_slice(CONTEXT_DOMAIN);
    entropy.extend_from_slice(context.as_bytes());
    entropy
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, VaultError> {
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
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = directory.join(format!("{}{}-{}", TEMP_PREFIX, std::process::id(), counter));

    if let Err(error) = write_temp_file(&temp, bytes) {
        let _ = fs::remove_file(&temp);
        return Err(error.into());
    }
    if let Err(error) = fs::rename(&temp, target) {
        let _ = fs::remove_file(&temp);
        return Err(error.into());
    }
    Ok(())
}

fn write_temp_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Best-effort wipe of a temporary plaintext buffer.
///
/// This uses volatile writes so the compiler cannot elide the clearing. It does
/// not claim to erase copies the allocator or the operating system may retain.
fn wipe(bytes: &mut [u8]) {
    for byte in bytes {
        // SAFETY: `byte` is a valid, exclusive reference into `bytes`.
        unsafe {
            std::ptr::write_volatile(byte, 0);
        }
    }
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
