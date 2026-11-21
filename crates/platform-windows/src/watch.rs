//! Bounded polling watcher for Start Menu install/uninstall changes.
//!
//! There is deliberately no background thread: the UI calls [`CatalogWatcher::poll`]
//! on its own schedule and decides whether to rescan. The watcher stores a
//! snapshot of supported files (path, size, modification time), compares
//! consecutive polls, and requires the same change to be observed twice before
//! reporting [`CatalogPoll::RescanNeeded`]. That debounce avoids a rescan while
//! an installer is still writing files.
//!
//! Traversal uses the same safety rules as discovery: only `.lnk`/`.exe` files,
//! bounded depth/entries, and reparse points below the roots are never followed.

use crate::start_menu::normalize_path_key;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// Default polling interval.
pub const DEFAULT_WATCH_INTERVAL: Duration = Duration::from_secs(2);
/// Lower bound accepted by [`CatalogWatcher::new`].
pub const MIN_WATCH_INTERVAL: Duration = Duration::from_millis(250);
/// Upper bound accepted by [`CatalogWatcher::new`].
pub const MAX_WATCH_INTERVAL: Duration = Duration::from_secs(3600);

const MAX_WATCH_DEPTH: usize = 8;
const MAX_WATCH_ENTRIES: usize = 20_000;

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    key: String,
    length: u64,
    modified_nanos: u128,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CatalogSnapshot {
    files: Vec<FileStamp>,
}

/// Counts of observed changes since the baseline snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CatalogDelta {
    /// Files that appeared.
    pub added: usize,
    /// Files that disappeared.
    pub removed: usize,
    /// Files whose size or modification time changed.
    pub modified: usize,
}

/// The result of one polling step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogPoll {
    /// The configured interval has not elapsed; nothing was inspected.
    Idle,
    /// The snapshot matches the baseline.
    Unchanged,
    /// A change was observed once. Poll again after the interval to confirm.
    Pending,
    /// The same change was observed on two consecutive polls; a rescan is due.
    RescanNeeded {
        /// What changed relative to the previous baseline.
        delta: CatalogDelta,
    },
}

impl CatalogPoll {
    /// Returns `true` only for [`Self::RescanNeeded`].
    pub fn needs_rescan(&self) -> bool {
        matches!(self, Self::RescanNeeded { .. })
    }
}

/// Returns the standard Start Menu roots observed by discovery.
///
/// The roots are the per-user and all-users `Start Menu\Programs` folders that
/// [`CatalogWatcher`] is designed to poll. Missing environment variables are an
/// explicit error rather than an empty watch set.
pub fn default_watch_roots() -> Result<Vec<PathBuf>, String> {
    crate::start_menu::start_menu_roots()
}

/// A bounded, debounced polling watcher over Start Menu roots.
#[derive(Debug)]
pub struct CatalogWatcher {
    roots: Vec<PathBuf>,
    interval: Duration,
    last_poll: Option<Instant>,
    baseline: Option<CatalogSnapshot>,
    pending: Option<CatalogSnapshot>,
}

impl CatalogWatcher {
    /// Creates a watcher over one or more roots.
    ///
    /// The interval must be between [`MIN_WATCH_INTERVAL`] and
    /// [`MAX_WATCH_INTERVAL`] so callers cannot accidentally spin.
    pub fn new(roots: Vec<PathBuf>, interval: Duration) -> Result<Self, String> {
        if roots.is_empty() {
            return Err("a catalog watcher needs at least one root directory".to_owned());
        }
        if interval < MIN_WATCH_INTERVAL || interval > MAX_WATCH_INTERVAL {
            return Err(format!(
                "watch interval must be between {} ms and {} s",
                MIN_WATCH_INTERVAL.as_millis(),
                MAX_WATCH_INTERVAL.as_secs()
            ));
        }

        Ok(Self {
            roots,
            interval,
            last_poll: None,
            baseline: None,
            pending: None,
        })
    }

    /// Returns the configured interval.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Returns the roots this watcher observes.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Performs one bounded polling step.
    ///
    /// The first call establishes the baseline and returns [`CatalogPoll::Unchanged`].
    /// A change must be seen twice before it is reported as
    /// [`CatalogPoll::RescanNeeded`], at which point the baseline advances so the
    /// same change is not reported again.
    pub fn poll(&mut self, now: Instant) -> CatalogPoll {
        if let Some(last) = self.last_poll {
            if now.saturating_duration_since(last) < self.interval {
                return CatalogPoll::Idle;
            }
        }
        self.last_poll = Some(now);

        let current = snapshot(&self.roots);
        let Some(baseline) = self.baseline.as_ref() else {
            self.baseline = Some(current);
            return CatalogPoll::Unchanged;
        };

        if current == *baseline {
            self.pending = None;
            return CatalogPoll::Unchanged;
        }

        if self.pending.as_ref() == Some(&current) {
            let delta = baseline.delta(&current);
            self.pending = None;
            self.baseline = Some(current);
            return CatalogPoll::RescanNeeded { delta };
        }

        self.pending = Some(current);
        CatalogPoll::Pending
    }

    /// Forces the next poll to re-establish a baseline.
    ///
    /// Call this after an explicit rescan if the caller wants to be certain the
    /// catalog and watcher agree, for example after a failed scan.
    pub fn reset(&mut self) {
        self.last_poll = None;
        self.baseline = None;
        self.pending = None;
    }
}

impl CatalogSnapshot {
    fn delta(&self, next: &Self) -> CatalogDelta {
        let previous: std::collections::BTreeMap<&str, &FileStamp> = self
            .files
            .iter()
            .map(|file| (file.key.as_str(), file))
            .collect();
        let current: std::collections::BTreeMap<&str, &FileStamp> = next
            .files
            .iter()
            .map(|file| (file.key.as_str(), file))
            .collect();

        CatalogDelta {
            added: current
                .keys()
                .filter(|key| !previous.contains_key(*key))
                .count(),
            removed: previous
                .keys()
                .filter(|key| !current.contains_key(*key))
                .count(),
            modified: current
                .iter()
                .filter(|(key, file)| {
                    previous.get(*key).is_some_and(|old| {
                        old.length != file.length || old.modified_nanos != file.modified_nanos
                    })
                })
                .count(),
        }
    }
}

fn snapshot(roots: &[PathBuf]) -> CatalogSnapshot {
    let mut stack: Vec<(PathBuf, usize)> = roots
        .iter()
        .rev()
        .map(|root| (root.clone(), 0usize))
        .collect();
    let mut visited = HashSet::new();
    let mut files = Vec::new();
    let mut inspected = 0usize;

    while let Some((path, depth)) = stack.pop() {
        if inspected >= MAX_WATCH_ENTRIES {
            break;
        }
        inspected += 1;

        let Ok(link_metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if depth > 0 && is_reparse_point(&link_metadata) {
            continue;
        }
        // Follow a redirected root (junction/symlink), matching discovery.
        let metadata = if depth == 0 {
            fs::metadata(&path).unwrap_or(link_metadata)
        } else {
            link_metadata
        };

        if metadata.is_dir() {
            if !visited.insert(normalize_path_key(&path)) || depth >= MAX_WATCH_DEPTH {
                continue;
            }
            let remaining = MAX_WATCH_ENTRIES.saturating_sub(inspected);
            if remaining == 0 {
                break;
            }
            let Ok(read_dir) = fs::read_dir(&path) else {
                continue;
            };
            let mut children: Vec<PathBuf> = read_dir
                .filter_map(|result| result.ok().map(|entry| entry.path()))
                .take(remaining)
                .collect();
            children.sort();
            for child in children.into_iter().rev() {
                stack.push((child, depth + 1));
            }
            continue;
        }

        if !metadata.is_file() || !is_supported(&path) {
            continue;
        }

        files.push(FileStamp {
            key: normalize_path_key(&path),
            length: metadata.len(),
            modified_nanos: metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .unwrap_or(0),
        });
    }

    files.sort_by(|left, right| left.key.cmp(&right.key));
    files.dedup_by(|left, right| left.key == right.key);
    CatalogSnapshot { files }
}

fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("lnk") || extension.eq_ignore_ascii_case("exe")
        })
}

fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_type().is_symlink()
        || (metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempTree {
        path: PathBuf,
    }

    impl TempTree {
        fn new(label: &str) -> Self {
            let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "ansa-platform-watch-{}-{label}-{sequence}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp tree");
            Self { path }
        }

        fn file(&self, relative: &str) -> PathBuf {
            let path = self.path.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(&path, b"").expect("write fixture");
            path
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn watcher(tree: &TempTree) -> CatalogWatcher {
        CatalogWatcher::new(vec![tree.path.clone()], Duration::from_secs(1))
            .expect("create watcher")
    }

    #[test]
    fn interval_bounds_are_enforced() {
        let roots = vec![std::env::temp_dir()];
        assert!(CatalogWatcher::new(Vec::new(), Duration::from_secs(1)).is_err());
        assert!(CatalogWatcher::new(roots.clone(), Duration::ZERO).is_err());
        assert!(CatalogWatcher::new(roots.clone(), Duration::from_millis(10)).is_err());
        assert!(CatalogWatcher::new(roots.clone(), Duration::from_secs(7200)).is_err());
        assert!(CatalogWatcher::new(roots, Duration::from_secs(2)).is_ok());
    }

    #[test]
    fn install_requires_two_polls_before_a_rescan() {
        let tree = TempTree::new("install");
        tree.file("Existing.lnk");
        let mut watcher = watcher(&tree);
        let start = Instant::now();

        assert_eq!(watcher.poll(start), CatalogPoll::Unchanged);
        assert_eq!(
            watcher.poll(start + Duration::from_secs(1)),
            CatalogPoll::Unchanged
        );

        tree.file("New App.lnk");
        assert_eq!(
            watcher.poll(start + Duration::from_secs(2)),
            CatalogPoll::Pending
        );
        let rescan = watcher.poll(start + Duration::from_secs(3));
        assert!(rescan.needs_rescan());
        assert_eq!(
            rescan,
            CatalogPoll::RescanNeeded {
                delta: CatalogDelta {
                    added: 1,
                    removed: 0,
                    modified: 0
                }
            }
        );

        // The baseline advanced, so the same change is not reported again.
        assert_eq!(
            watcher.poll(start + Duration::from_secs(4)),
            CatalogPoll::Unchanged
        );
    }

    #[test]
    fn uninstall_is_detected_and_debounced() {
        let tree = TempTree::new("uninstall");
        tree.file("Gone.lnk");
        tree.file("Kept.exe");
        let mut watcher = watcher(&tree);
        let start = Instant::now();

        assert_eq!(watcher.poll(start), CatalogPoll::Unchanged);
        fs::remove_file(tree.path.join("Gone.lnk")).expect("remove fixture");

        assert_eq!(
            watcher.poll(start + Duration::from_secs(1)),
            CatalogPoll::Pending
        );
        assert_eq!(
            watcher.poll(start + Duration::from_secs(2)),
            CatalogPoll::RescanNeeded {
                delta: CatalogDelta {
                    added: 0,
                    removed: 1,
                    modified: 0
                }
            }
        );
        assert_eq!(
            watcher.poll(start + Duration::from_secs(3)),
            CatalogPoll::Unchanged
        );
    }

    #[test]
    fn modification_is_detected_once_files_change_size() {
        let tree = TempTree::new("modify");
        tree.file("App.lnk");
        let mut watcher = watcher(&tree);
        let start = Instant::now();

        assert_eq!(watcher.poll(start), CatalogPoll::Unchanged);
        fs::write(tree.path.join("App.lnk"), b"changed contents").expect("modify fixture");

        assert_eq!(
            watcher.poll(start + Duration::from_secs(1)),
            CatalogPoll::Pending
        );
        assert_eq!(
            watcher.poll(start + Duration::from_secs(2)),
            CatalogPoll::RescanNeeded {
                delta: CatalogDelta {
                    added: 0,
                    removed: 0,
                    modified: 1
                }
            }
        );
    }

    #[test]
    fn a_change_that_reverts_before_confirmation_does_not_rescan() {
        let tree = TempTree::new("revert");
        let path = tree.file("Temp.lnk");
        let mut watcher = watcher(&tree);
        let start = Instant::now();

        assert_eq!(watcher.poll(start), CatalogPoll::Unchanged);
        tree.file("Temp2.lnk");
        assert_eq!(
            watcher.poll(start + Duration::from_secs(1)),
            CatalogPoll::Pending
        );
        fs::remove_file(tree.path.join("Temp2.lnk")).expect("remove fixture");
        assert_eq!(
            watcher.poll(start + Duration::from_secs(2)),
            CatalogPoll::Unchanged
        );
        assert!(path.exists());
    }

    #[test]
    fn polls_inside_the_interval_are_idle() {
        let tree = TempTree::new("idle");
        tree.file("App.lnk");
        let mut watcher = watcher(&tree);
        let start = Instant::now();

        assert_eq!(watcher.poll(start), CatalogPoll::Unchanged);
        assert_eq!(
            watcher.poll(start + Duration::from_millis(100)),
            CatalogPoll::Idle
        );
    }

    #[test]
    fn unsupported_files_are_ignored() {
        let tree = TempTree::new("unsupported");
        tree.file("Readme.txt");
        let mut watcher = watcher(&tree);
        let start = Instant::now();

        assert_eq!(watcher.poll(start), CatalogPoll::Unchanged);
        tree.file("Another.txt");
        assert_eq!(
            watcher.poll(start + Duration::from_secs(1)),
            CatalogPoll::Unchanged
        );
    }

    #[test]
    fn reset_discards_baseline_and_pending_state() {
        let tree = TempTree::new("reset");
        tree.file("App.lnk");
        let mut watcher = watcher(&tree);
        let start = Instant::now();

        assert_eq!(watcher.poll(start), CatalogPoll::Unchanged);
        tree.file("New.lnk");
        assert_eq!(
            watcher.poll(start + Duration::from_secs(1)),
            CatalogPoll::Pending
        );
        watcher.reset();
        assert_eq!(
            watcher.poll(start + Duration::from_secs(2)),
            CatalogPoll::Unchanged
        );
    }
}
