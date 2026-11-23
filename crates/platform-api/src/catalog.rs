//! Shared bounds for Unix discovery and the synthetic catalog tests.

use crate::AppEntry;
use std::collections::BTreeMap;
use std::fs::{self, Metadata, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;

const MAX_VISITED: usize = 32_768;
const MAX_CANDIDATES: usize = 8_192;
const MAX_METADATA_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_ROOTS: usize = 32;
pub(crate) const MAX_NATIVE_PATH: usize = 4096;

/// Selection tokens are unique within a process. Zero is reserved for entries
/// constructed outside discovery and can never match a native snapshot.
static NEXT_SELECTION: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_selection() -> u64 {
    loop {
        let token = NEXT_SELECTION.fetch_add(1, Ordering::Relaxed);
        if token != 0 {
            return token;
        }
    }
}

/// A walk is deliberately not a symlink traversal or filesystem indexer.
/// Directory listings are sorted before use. A limit aborts the refresh instead
/// of publishing a nondeterministic partial list.
pub(crate) fn candidates(
    roots: &[PathBuf],
    extension: &str,
    max_depth: usize,
) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    if roots.len() > MAX_ROOTS {
        return Err(format!("Catalog exceeds the {MAX_ROOTS}-root limit"));
    }
    let mut output = Vec::new();
    let mut visited = 0;
    let mut readable_roots = 0;
    let mut first_error = None;
    for root in roots.iter().filter(|root| root.is_absolute()) {
        let mut pending = vec![(root.to_path_buf(), 0)];
        let mut matches = Vec::new();
        while let Some((directory, depth)) = pending.pop() {
            let entries = match fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(error) => {
                    if error.kind() != io::ErrorKind::NotFound {
                        first_error
                            .get_or_insert_with(|| format!("{}: {error}", directory.display()));
                    }
                    continue;
                }
            };
            if depth == 0 {
                readable_roots += 1;
            }
            for result in entries {
                visited += 1;
                if visited > MAX_VISITED {
                    return Err(format!("Catalog exceeds the {MAX_VISITED}-item scan limit"));
                }
                let Ok(entry) = result else { continue };
                let path = entry.path();
                if path
                    .to_str()
                    .is_none_or(|path| path.len() > MAX_NATIVE_PATH)
                {
                    continue;
                }
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                let matches_extension =
                    path.extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| {
                            if extension == "app" {
                                ext.eq_ignore_ascii_case(extension)
                            } else {
                                ext == extension
                            }
                        });
                if matches_extension && (kind.is_file() || kind.is_dir() || kind.is_symlink()) {
                    if output.len() + matches.len() >= MAX_CANDIDATES {
                        return Err(format!(
                            "Catalog exceeds the {MAX_CANDIDATES}-application limit"
                        ));
                    }
                    matches.push(path);
                } else if kind.is_dir() && depth < max_depth {
                    pending.push((path, depth + 1));
                }
            }
        }
        matches.sort_unstable();
        output.extend(matches.into_iter().map(|path| (root.clone(), path)));
    }
    if readable_roots == 0 {
        if let Some(error) = first_error {
            return Err(format!(
                "Application directories could not be read: {error}"
            ));
        }
    }
    Ok(output)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

impl FileStamp {
    pub(crate) fn new(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            identity: (
                metadata.dev(),
                metadata.ino(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            ),
        }
    }
}

pub(crate) fn bounded_file(path: &Path, max_bytes: usize) -> Result<(Vec<u8>, FileStamp), String> {
    // Open only regular files and recheck the opened descriptor. Nonblocking on
    // Unix prevents a swapped FIFO from hanging discovery before that recheck.
    let metadata = fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    check_regular(path, &metadata, max_bytes)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    check_regular(path, &metadata, max_bytes)?;
    let mut bytes = Vec::new();
    file.take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() > max_bytes {
        return Err(format!("{} exceeds {max_bytes} bytes", path.display()));
    }
    Ok((bytes, FileStamp::new(&metadata)))
}

fn check_regular(path: &Path, metadata: &Metadata, max_bytes: usize) -> Result<(), String> {
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if metadata.len() > max_bytes as u64 {
        return Err(format!("{} exceeds {max_bytes} bytes", path.display()));
    }
    Ok(())
}

pub(crate) fn spend(used: &mut usize, bytes: usize) -> Result<(), String> {
    *used += bytes;
    if *used > MAX_METADATA_BYTES {
        Err(
            "Catalog hit the 64 MiB metadata read limit; no partial refresh was published"
                .to_owned(),
        )
    } else {
        Ok(())
    }
}

/// A discovered path is only launchable while its prepared metadata still
/// matches. Refresh replaces the complete snapshot, never appends to it.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub(crate) struct Snapshot<T>(Mutex<Option<BTreeMap<String, T>>>);

impl<T: Clone> Snapshot<T> {
    pub(crate) const fn new() -> Self {
        Self(Mutex::new(None))
    }

    pub(crate) fn replace(&self, entries: BTreeMap<String, T>) {
        *self.0.lock().unwrap_or_else(|error| error.into_inner()) = Some(entries);
    }

    pub(crate) fn get(
        &self,
        path: &str,
        populate: impl FnOnce() -> Result<(), String>,
    ) -> Result<T, String> {
        {
            let snapshot = self.0.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(entries) = snapshot.as_ref() {
                return entries.get(path).cloned().ok_or_else(stale_target);
            }
        }
        populate()?;
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .and_then(|entries| entries.get(path))
            .cloned()
            .ok_or_else(stale_target)
    }

    /// Returns the current snapshot object only if it is the exact selection
    /// the caller retained. A rediscovery at the same path and stable ID
    /// receives a new token, so an old selection cannot be relaunched against
    /// the replacement.
    pub(crate) fn get_selected(
        &self,
        retained: &AppEntry,
        entry_of: impl Fn(&T) -> &AppEntry,
        populate: impl FnOnce() -> Result<(), String>,
    ) -> Result<T, String> {
        self.get(&retained.path, populate).and_then(|current| {
            if selection_matches(entry_of(&current), retained) {
                Ok(current)
            } else {
                Err(stale_target())
            }
        })
    }
}

pub(crate) fn selection_matches(current: &AppEntry, retained: &AppEntry) -> bool {
    current.selection == retained.selection
        && current.id == retained.id
        && current.path == retained.path
}

pub(crate) fn stale_target() -> String {
    "Application is not in the current catalog. Refresh applications and try again".to_owned()
}

pub(crate) fn usable_executable(metadata: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        // Windows-hosted synthetic Unix fixtures do not have mode bits.
        metadata.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_replaces_snapshot_without_appending() {
        let catalog = Snapshot::new();
        let initialize = || {
            catalog.replace(BTreeMap::from([("/old".to_owned(), 1)]));
            Ok(())
        };
        assert_eq!(catalog.get("/old", initialize), Ok(1));
        catalog.replace(BTreeMap::from([("/new".to_owned(), 2)]));
        assert_eq!(catalog.get("/new", || unreachable!()), Ok(2));
        assert!(catalog.get("/old", || unreachable!()).is_err());
    }

    #[test]
    fn rediscovered_selection_token_rejects_the_retained_entry() {
        let catalog = Snapshot::new();
        let mut retained = AppEntry::new("app:fixture", "Fixture", "/apps/fixture");
        retained.selection = next_selection();
        let mut replacement = retained.clone();
        replacement.selection = next_selection();
        assert_ne!(retained.selection, replacement.selection);
        assert!(!selection_matches(&replacement, &retained));
        catalog.replace(BTreeMap::from([(
            replacement.path.clone(),
            replacement.clone(),
        )]));
        assert!(catalog
            .get_selected(&retained, |entry| entry, || unreachable!())
            .is_err());
        assert_eq!(
            catalog.get_selected(&replacement, |entry| entry, || unreachable!()),
            Ok(replacement)
        );
        // A manually constructed entry never carries a discovery token, so it
        // cannot match a discovered snapshot entry even at the same identity.
        let mut discovered = AppEntry::new("app:fixture", "Fixture", "/apps/fixture");
        discovered.selection = next_selection();
        assert!(!selection_matches(
            &discovered,
            &AppEntry::new("app:fixture", "Fixture", "/apps/fixture")
        ));
    }
}
