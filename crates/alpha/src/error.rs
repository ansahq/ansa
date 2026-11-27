//! Explicit failures surfaced by the alpha composition root.

use std::fmt;

/// Everything the alpha composition root can fail to do.
#[derive(Debug)]
pub enum AlphaError {
    /// Filesystem setup for the data directory failed.
    Io(std::io::Error),
    /// Durable storage failed to open, read or write.
    Storage(ansa_storage::StorageError),
    /// A quicklink preview, commit or import operation failed.
    Quicklink(String),
    /// A coordinator or provider could not be constructed or used.
    Coordinator(String),
    /// Emoji dataset loading or search failed.
    Emoji(String),
    /// The composed searchable catalog contained an invalid identity.
    Catalog(String),
    /// Onboarding state could not be read.
    Onboarding(String),
    /// A saved user record payload could not be interpreted.
    Record { id: String, reason: String },
    /// The vault could not be opened or a sealed value could not be read.
    Vault(String),
}

impl fmt::Display for AlphaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "data directory I/O failed: {error}"),
            Self::Storage(error) => write!(formatter, "storage failed: {error}"),
            Self::Quicklink(message) => write!(formatter, "quicklink operation failed: {message}"),
            Self::Coordinator(message) => {
                write!(formatter, "search coordinator failed: {message}")
            }
            Self::Emoji(message) => write!(formatter, "emoji dataset failed: {message}"),
            Self::Catalog(message) => write!(formatter, "catalog composition failed: {message}"),
            Self::Onboarding(message) => write!(formatter, "onboarding state failed: {message}"),
            Self::Record { id, reason } => {
                write!(formatter, "record {id} is unusable: {reason}")
            }
            Self::Vault(message) => write!(formatter, "vault operation failed: {message}"),
        }
    }
}

impl std::error::Error for AlphaError {}

impl From<std::io::Error> for AlphaError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<ansa_storage::StorageError> for AlphaError {
    fn from(error: ansa_storage::StorageError) -> Self {
        Self::Storage(error)
    }
}
