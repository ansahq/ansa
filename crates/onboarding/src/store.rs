//! Persistence for onboarding state.
//!
//! [`MemoryStore`] is for tests and previews. [`JsonFileStore`] writes a single
//! JSON document with a temporary file in the same directory followed by a
//! rename, so a crash cannot leave a half-written state file. A missing or
//! unreadable (corrupt) file loads as `None`; onboarding then starts from a
//! safe default without touching any other file.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// Persistence operations required by the onboarding model.
pub trait OnboardingStore {
    /// Returns the last saved document, or `None` when no usable document
    /// exists. Corrupt documents are reported as `None`, not as an error that
    /// would block startup.
    fn load(&self) -> Result<Option<String>, StoreError>;

    /// Replaces the saved document.
    fn save(&self, contents: &str) -> Result<(), StoreError>;

    /// Removes the saved document. Clearing an already-empty store succeeds.
    fn clear(&self) -> Result<(), StoreError>;
}

/// A store failure with a plain-language message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreError {
    message: String,
}

impl StoreError {
    /// Creates an error from any displayable description.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The message shown to the user or logged by the host.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for StoreError {}

impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        Self::new(error.to_string())
    }
}

/// An in-process store. Clones share the same backing document.
#[derive(Debug, Clone, Default)]
pub struct MemoryStore {
    document: Arc<Mutex<Option<String>>>,
}

impl MemoryStore {
    /// Creates an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a copy of the stored document, if any.
    pub fn document(&self) -> Option<String> {
        self.lock().clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.document.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl OnboardingStore for MemoryStore {
    fn load(&self) -> Result<Option<String>, StoreError> {
        Ok(self.document())
    }

    fn save(&self, contents: &str) -> Result<(), StoreError> {
        *self.lock() = Some(contents.to_owned());
        Ok(())
    }

    fn clear(&self) -> Result<(), StoreError> {
        *self.lock() = None;
        Ok(())
    }
}

/// A JSON document stored in one file, written atomically.
#[derive(Debug, Clone)]
pub struct JsonFileStore {
    path: PathBuf,
}

impl JsonFileStore {
    /// Creates a store for `path`. The file is not created until the first
    /// save.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The document path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl OnboardingStore for JsonFileStore {
    fn load(&self) -> Result<Option<String>, StoreError> {
        match fs::read_to_string(&self.path) {
            Ok(contents) => Ok(Some(contents)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            // Invalid UTF-8 is a corrupt state file, not a startup failure.
            Err(error) if error.kind() == io::ErrorKind::InvalidData => Ok(None),
            Err(error) => Err(StoreError::new(format!(
                "could not read {}: {error}",
                self.path.display()
            ))),
        }
    }

    fn save(&self, contents: &str) -> Result<(), StoreError> {
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|error| {
                StoreError::new(format!("could not create {}: {error}", parent.display()))
            })?;
        }

        let temporary = self.temporary_path();
        let write_result = (|| -> io::Result<()> {
            let mut file = fs::File::create(&temporary)?;
            file.write_all(contents.as_bytes())?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)
        })();

        if let Err(error) = write_result {
            // Best-effort cleanup; the original document, if any, is intact.
            let _ = fs::remove_file(&temporary);
            return Err(StoreError::new(format!(
                "could not save {}: {error}",
                self.path.display()
            )));
        }

        Ok(())
    }

    fn clear(&self) -> Result<(), StoreError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(StoreError::new(format!(
                "could not clear {}: {error}",
                self.path.display()
            ))),
        }
    }
}

impl JsonFileStore {
    fn temporary_path(&self) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
        let file_name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "onboarding".to_owned());
        let temporary_name = format!(".{file_name}.{}.{}.tmp", std::process::id(), sequence);
        self.path.with_file_name(temporary_name)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn memory_store_round_trips_and_clears() {
        let store = MemoryStore::new();
        assert_eq!(store.load().unwrap(), None);
        store.save("{\"a\":1}").unwrap();
        assert_eq!(store.load().unwrap().as_deref(), Some("{\"a\":1}"));
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
        store.clear().unwrap();
    }

    #[test]
    fn memory_store_clones_share_state() {
        let store = MemoryStore::new();
        let clone = store.clone();
        store.save("state").unwrap();
        assert_eq!(clone.document().as_deref(), Some("state"));
    }

    #[test]
    fn file_store_round_trips_missing_and_clear() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        let store = JsonFileStore::new(&path);

        assert_eq!(store.load().unwrap(), None);
        store.save("{\"version\":1}").unwrap();
        assert_eq!(store.load().unwrap().as_deref(), Some("{\"version\":1}"));
        store.save("{\"version\":2}").unwrap();
        assert_eq!(store.load().unwrap().as_deref(), Some("{\"version\":2}"));
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
        store.clear().unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn file_store_creates_parent_directories() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory
            .path()
            .join("nested")
            .join("deeper")
            .join("state.json");
        let store = JsonFileStore::new(&path);
        store.save("{}").unwrap();
        assert!(path.exists());
    }

    #[test]
    fn corrupt_utf8_is_a_safe_default_not_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        let store = JsonFileStore::new(&path);
        assert_eq!(store.load().unwrap(), None);
        // Loading a corrupt document does not delete or rewrite it.
        assert!(path.exists());
    }

    #[test]
    fn saving_and_clearing_leave_siblings_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let sibling = directory.path().join("other.json");
        fs::write(&sibling, "keep me").unwrap();
        let path = directory.path().join("state.json");
        let store = JsonFileStore::new(&path);

        store.save("{}").unwrap();
        store.clear().unwrap();

        assert_eq!(fs::read_to_string(&sibling).unwrap(), "keep me");
        let leftovers: Vec<_> = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }
}
