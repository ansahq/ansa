//! Errors surfaced by the vault.
//!
//! Error messages deliberately never contain plaintext secrets, ciphertext,
//! DPAPI blobs or secret file contents. Only non-sensitive identifiers such as
//! an alias, a context label, a limit and a Windows error code may appear.

use std::fmt;

/// Every failure surfaced by [`crate::Vault`].
///
/// The enum is `#[non_exhaustive]` so that future platform backends can add
/// variants without breaking downstream matching.
#[derive(Debug)]
#[non_exhaustive]
pub enum VaultError {
    /// The platform has no supported OS-backed sealing backend. No files are
    /// written and no plaintext fallback exists.
    Unsupported {
        /// Operating system reported by `std::env::consts::OS`.
        platform: &'static str,
        /// Short, non-sensitive explanation.
        reason: &'static str,
    },
    /// A filesystem operation failed.
    Io(std::io::Error),
    /// The vault is locked. Call [`crate::Vault::unlock`] first.
    Locked,
    /// No stored secret exists for the requested alias.
    SecretNotFound,
    /// The alias does not satisfy the documented grammar.
    InvalidAlias {
        /// Short, non-sensitive explanation.
        reason: &'static str,
    },
    /// The context label does not satisfy the documented grammar.
    InvalidContext {
        /// Short, non-sensitive explanation.
        reason: &'static str,
    },
    /// A sealed payload is malformed, truncated or uses unsupported fields.
    InvalidPayload {
        /// Short, non-sensitive explanation.
        reason: &'static str,
    },
    /// The payload format version is newer or older than this build supports.
    UnsupportedPayloadVersion {
        /// Version found in the payload.
        found: u16,
        /// Version supported by this crate.
        supported: u16,
    },
    /// The caller supplied a different context than the one the payload was
    /// sealed with.
    WrongContext,
    /// DPAPI refused to unprotect the payload. This is the expected result for
    /// tampered ciphertext, wrong entropy, a different Windows user or a
    /// restored vault from another machine.
    DecryptionFailed {
        /// Raw Windows error code, without any payload data.
        os_code: u32,
    },
    /// DPAPI protection failed before any data was written.
    Dpapi {
        /// `"protect"` or `"unprotect"`.
        operation: &'static str,
        /// Raw Windows error code.
        os_code: u32,
    },
    /// The vault directory or its metadata is invalid.
    InvalidVault {
        /// Short, non-sensitive explanation.
        reason: &'static str,
    },
    /// The vault metadata was written by an incompatible format version.
    UnsupportedVaultFormat {
        /// Format version found on disk.
        found: u32,
        /// Format version supported by this crate.
        supported: u32,
    },
    /// A bounded resource limit was exceeded.
    LimitExceeded {
        /// Resource name, for example `"plaintext"`.
        kind: &'static str,
        /// Maximum accepted size or count.
        limit: usize,
    },
    /// An internal invariant failed. No user data is included.
    Internal {
        /// Operation that failed.
        operation: &'static str,
    },
}

impl fmt::Display for VaultError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { platform, reason } => {
                write!(formatter, "vault is unsupported on {platform}: {reason}")
            }
            Self::Io(error) => write!(formatter, "vault I/O error: {error}"),
            Self::Locked => write!(formatter, "vault is locked"),
            Self::SecretNotFound => write!(formatter, "vault secret was not found"),
            Self::InvalidAlias { reason } => write!(formatter, "invalid vault alias: {reason}"),
            Self::InvalidContext { reason } => write!(formatter, "invalid vault context: {reason}"),
            Self::InvalidPayload { reason } => {
                write!(formatter, "invalid sealed payload: {reason}")
            }
            Self::UnsupportedPayloadVersion { found, supported } => write!(
                formatter,
                "sealed payload version {found} is not supported (expected {supported})"
            ),
            Self::WrongContext => {
                write!(formatter, "sealed payload is bound to a different context")
            }
            Self::DecryptionFailed { os_code } => write!(
                formatter,
                "vault could not decrypt the payload (Windows error {os_code}); \
                 it may be tampered with, bound to another context, or sealed for another user"
            ),
            Self::Dpapi { operation, os_code } => {
                write!(
                    formatter,
                    "DPAPI {operation} failed (Windows error {os_code})"
                )
            }
            Self::InvalidVault { reason } => write!(formatter, "invalid vault: {reason}"),
            Self::UnsupportedVaultFormat { found, supported } => write!(
                formatter,
                "vault format {found} is not supported (expected {supported})"
            ),
            Self::LimitExceeded { kind, limit } => {
                write!(formatter, "vault {kind} limit exceeded (maximum {limit})")
            }
            Self::Internal { operation } => {
                write!(formatter, "vault internal failure during {operation}")
            }
        }
    }
}

impl std::error::Error for VaultError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for VaultError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
