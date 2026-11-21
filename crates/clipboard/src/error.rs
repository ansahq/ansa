//! Errors surfaced by clipboard capture, storage and insertion.
//!
//! Error values never contain clipboard text, file paths, ciphertext or DPAPI
//! blobs. They carry static operation names, bounds and OS error codes only, so
//! they are safe to log and display.

use std::fmt;

use ansa_vault::VaultError;

use crate::model::HistoryError;

/// A clipboard capture, storage or adapter failure.
#[derive(Debug)]
#[non_exhaustive]
pub enum ClipboardError {
    /// The store or its sealing backend is locked.
    Locked,
    /// A filesystem operation failed.
    Io(std::io::Error),
    /// The DPAPI vault rejected an operation.
    Vault(VaultError),
    /// The metadata model rejected an operation.
    History(HistoryError),
    /// The store is already open in this process or another process.
    AlreadyLocked,
    /// A pre-namespace clipboard store could not be safely opened or migrated.
    LegacyLayout {
        /// Short, non-sensitive reason.
        reason: &'static str,
    },
    /// No stored item has the requested identifier.
    ItemNotFound,
    /// `manifest.json` did not satisfy the documented format.
    CorruptManifest {
        /// Short, non-sensitive reason.
        reason: &'static str,
    },
    /// `history.json` did not satisfy the documented format.
    CorruptHistory {
        /// Short, non-sensitive reason.
        reason: &'static str,
    },
    /// `history.json` exceeded the bounded read limit.
    MetadataTooLarge {
        /// Maximum accepted bytes.
        limit: usize,
    },
    /// A payload envelope exceeded the sealing limit.
    EnvelopeTooLarge {
        /// Observed bytes.
        bytes: usize,
        /// Maximum accepted bytes.
        limit: usize,
    },
    /// A sealed payload was not a valid clipboard envelope.
    MalformedPayload {
        /// Short, non-sensitive reason.
        reason: &'static str,
    },
    /// A sealing backend failed.
    Backend {
        /// Operation name, for example `"seal"`.
        operation: &'static str,
    },
    /// This platform has no clipboard adapter.
    UnsupportedPlatform {
        /// Operating system reported by `std::env::consts::OS`.
        platform: &'static str,
    },
    /// The clipboard could not be opened after bounded retries.
    ClipboardBusy {
        /// Number of attempts made.
        attempts: u32,
    },
    /// A Windows API call failed.
    Native {
        /// Operation name.
        operation: &'static str,
        /// Raw Windows error code, or `0` if unavailable.
        os_code: u32,
    },
    /// The clipboard exposed no format this build understands.
    NoSupportedFormat,
    /// A caller-supplied value was invalid.
    InvalidArgument {
        /// Short, non-sensitive reason.
        reason: &'static str,
    },
}

impl fmt::Display for ClipboardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Locked => formatter.write_str("clipboard store is locked"),
            Self::AlreadyLocked => {
                formatter.write_str("clipboard store is already open in another instance")
            }
            Self::LegacyLayout { reason } => {
                write!(
                    formatter,
                    "legacy clipboard store cannot be opened: {reason}"
                )
            }
            Self::Io(error) => write!(formatter, "clipboard I/O error: {error}"),
            Self::Vault(error) => write!(formatter, "clipboard vault error: {error}"),
            Self::History(error) => write!(formatter, "clipboard history error: {error}"),
            Self::ItemNotFound => formatter.write_str("clipboard item was not found"),
            Self::CorruptHistory { reason } => {
                write!(formatter, "corrupt clipboard history: {reason}")
            }
            Self::CorruptManifest { reason } => {
                write!(formatter, "corrupt clipboard manifest: {reason}")
            }
            Self::MetadataTooLarge { limit } => {
                write!(formatter, "clipboard metadata exceeds {limit} bytes")
            }
            Self::EnvelopeTooLarge { bytes, limit } => {
                write!(
                    formatter,
                    "clipboard payload is {bytes} bytes (limit {limit})"
                )
            }
            Self::MalformedPayload { reason } => {
                write!(formatter, "malformed clipboard payload: {reason}")
            }
            Self::Backend { operation } => {
                write!(
                    formatter,
                    "clipboard sealing backend failed during {operation}"
                )
            }
            Self::UnsupportedPlatform { platform } => {
                write!(formatter, "clipboard capture is unsupported on {platform}")
            }
            Self::ClipboardBusy { attempts } => {
                write!(formatter, "clipboard was busy after {attempts} attempts")
            }
            Self::Native { operation, os_code } => {
                write!(
                    formatter,
                    "clipboard {operation} failed (Windows error {os_code})"
                )
            }
            Self::NoSupportedFormat => {
                formatter.write_str("clipboard has no supported text, file-list or image format")
            }
            Self::InvalidArgument { reason } => {
                write!(formatter, "invalid clipboard argument: {reason}")
            }
        }
    }
}

impl std::error::Error for ClipboardError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Vault(error) => Some(error),
            Self::History(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ClipboardError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<VaultError> for ClipboardError {
    fn from(value: VaultError) -> Self {
        Self::Vault(value)
    }
}

impl From<HistoryError> for ClipboardError {
    fn from(value: HistoryError) -> Self {
        Self::History(value)
    }
}
