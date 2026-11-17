//! Bounded Start Menu catalog traversal.
//!
//! Design constraints:
//! * Only the per-user and all-users Start Menu `Programs` directories are
//!   scanned; no arbitrary directory walking.
//! * Traversal is iterative and bounded by depth, inspected entries and result
//!   count.
//! * Symbolic links, junctions and other reparse points are never followed, so
//!   a link loop cannot recurse indefinitely. A normalized visited-directory set
//!   is a second line of defense.
//! * Shortcut targets are not resolved. Discovery only catalogs the `.lnk` or
//!   `.exe` entry itself; the shell resolves the shortcut at launch time.

use crate::types::{AppEntry, CapabilityStatus};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

pub(crate) const DEFAULT_MAX_DEPTH: usize = 8;
pub(crate) const DEFAULT_MAX_ENTRIES: usize = 20_000;
pub(crate) const DEFAULT_MAX_RESULTS: usize = 5_000;

#[derive(Clone, Copy, Debug)]
pub(crate) struct DiscoverLimits {
    pub(crate) max_depth: usize,
    pub(crate) max_entries: usize,
    pub(crate) max_results: usize,
    pub(crate) include_exe: bool,
}

impl Default for DiscoverLimits {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            max_entries: DEFAULT_MAX_ENTRIES,
            max_results: DEFAULT_MAX_RESULTS,
            include_exe: true,
        }
    }
}

/// Returns the two standard Start Menu Programs roots.
pub(crate) fn start_menu_roots() -> Result<Vec<PathBuf>, String> {
    let mut roots = Vec::with_capacity(2);

    if let Some(program_data) = env::var_os("ProgramData") {
        roots.push(
            PathBuf::from(program_data)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs"),
        );
    }
    if let Some(app_data) = env::var_os("APPDATA") {
        roots.push(
            PathBuf::from(app_data)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs"),
        );
    }

    if roots.is_empty() {
        return Err(
            "cannot locate the Start Menu: neither ProgramData nor APPDATA is set".to_owned(),
        );
    }

    let mut seen = HashSet::new();
    roots.retain(|root| seen.insert(normalize_path_key(root)));
    Ok(roots)
}

/// Discovers Start Menu entries using the default bounds.
pub(crate) fn discover() -> Result<Vec<AppEntry>, String> {
    let roots = start_menu_roots()?;
    discover_from_roots(&roots, DiscoverLimits::default())
}

/// Probes the catalog capability without caching the result.
pub(crate) fn probe_catalog_status() -> CapabilityStatus {
    let roots = match start_menu_roots() {
        Ok(roots) => roots,
        Err(_) => return CapabilityStatus::TemporarilyUnavailable,
    };

    let mut saw_permission_denial = false;
    for root in &roots {
        match fs::read_dir(root) {
            Ok(_) => return CapabilityStatus::Available,
            Err(error) if error.kind() == ErrorKind::PermissionDenied => {
                saw_permission_denial = true;
            }
            Err(_) => {}
        }
    }

    if saw_permission_denial {
        CapabilityStatus::PermissionRequired
    } else {
        CapabilityStatus::TemporarilyUnavailable
    }
}

/// Discovers entries below explicit roots. Used by tests with bounded limits.
pub(crate) fn discover_from_roots(
    roots: &[PathBuf],
    limits: DiscoverLimits,
) -> Result<Vec<AppEntry>, String> {
    if roots.is_empty() {
        return Err("no Start Menu roots were supplied".to_owned());
    }

    let accessible_roots = roots
        .iter()
        .filter(|root| {
            fs::metadata(root)
                .map(|metadata| metadata.is_dir())
                .unwrap_or(false)
        })
        .count();
    if accessible_roots == 0 {
        return Err("no accessible Start Menu directories were found".to_owned());
    }

    // Reverse so the first root is processed first with a LIFO stack.
    let mut stack: Vec<(PathBuf, usize)> = roots
        .iter()
        .rev()
        .map(|root| (root.clone(), 0usize))
        .collect();
    let mut visited_directories: HashSet<String> = HashSet::new();
    let mut seen_paths: HashSet<String> = HashSet::new();
    let mut entries: Vec<AppEntry> = Vec::new();
    let mut inspected = 0usize;

    while let Some((path, depth)) = stack.pop() {
        if inspected >= limits.max_entries || entries.len() >= limits.max_results {
            break;
        }
        inspected += 1;

        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };

        // Roots are allowed to be reparse points, but anything below them is
        // not traversed. This prevents junction/symlink recursion and loops.
        if depth > 0 && is_reparse_point(&metadata) {
            continue;
        }

        if metadata.is_dir() {
            let key = normalize_path_key(&path);
            if !visited_directories.insert(key) {
                continue;
            }
            if depth >= limits.max_depth {
                continue;
            }

            let read_dir = match fs::read_dir(&path) {
                Ok(read_dir) => read_dir,
                Err(_) => continue,
            };
            let mut children: Vec<PathBuf> = read_dir
                .filter_map(|result| result.ok().map(|entry| entry.path()))
                .collect();
            // Deterministic push order; the final catalog is sorted anyway.
            children.sort();
            for child in children.into_iter().rev() {
                stack.push((child, depth + 1));
            }
            continue;
        }

        if !metadata.is_file() || !is_supported_file(&path, limits.include_exe) {
            continue;
        }

        let path_key = normalize_path_key(&path);
        if !seen_paths.insert(path_key) {
            continue;
        }

        let Some(path_string) = path.to_str() else {
            // The public API uses `&str` paths. Do not substitute a lossy path
            // that would fail or launch a different file later.
            continue;
        };
        let Some(title) = entry_title(&path) else {
            continue;
        };
        if title.is_empty() {
            continue;
        }

        entries.push(AppEntry {
            id: stable_app_id(&path),
            title,
            path: path_string.to_owned(),
        });
    }

    entries.sort_by_cached_key(|entry| (entry.title.to_lowercase(), entry.path.to_lowercase()));
    Ok(entries)
}

fn is_supported_file(path: &Path, include_exe: bool) -> bool {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("lnk") => true,
        Some("exe") => include_exe,
        _ => false,
    }
}

fn entry_title(path: &Path) -> Option<String> {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().trim().to_owned())
}

fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

    metadata.file_type().is_symlink()
        || (metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
}

/// Case-folded, separator-normalized key for path identity.
pub(crate) fn normalize_path_key(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let mut normalized = String::with_capacity(raw.len());
    let mut previous_was_separator = false;

    for character in raw.chars() {
        let character = if character == '/' { '\\' } else { character };
        if character == '\\' {
            if !previous_was_separator {
                normalized.push('\\');
            }
            previous_was_separator = true;
        } else {
            normalized.push(character);
            previous_was_separator = false;
        }
    }

    while normalized.len() > 3 && normalized.ends_with('\\') {
        normalized.pop();
    }

    normalized.to_lowercase()
}

/// Stable v1 identity derived from the normalized path.
///
/// This does not use `DefaultHasher`, whose output is not guaranteed stable
/// across Rust versions. Two FNV-1a passes make accidental collisions
/// negligible for a Start Menu-sized catalog.
pub(crate) fn stable_app_id(path: &Path) -> String {
    let key = normalize_path_key(path);
    let first = fnv1a(key.as_bytes(), 0xcbf2_9ce4_8422_2325);
    let second = fnv1a(key.as_bytes(), 0x9e37_79b9_7f4a_7c15);
    // `app:` matches the domain CommandId namespace convention and lets the
    // caller map this opaque identity directly into `CommandId::new`.
    format!("app:{first:016x}{second:016x}")
}

fn fnv1a(bytes: &[u8], mut hash: u64) -> u64 {
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
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
            let path = env::temp_dir().join(format!(
                "ansa-platform-windows-{}-{label}-{sequence}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp tree");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
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

    fn titles(entries: &[AppEntry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.title.as_str()).collect()
    }

    #[test]
    fn id_is_stable_across_case_and_separator_changes() {
        let canonical = Path::new(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\App.lnk");
        let variant = Path::new("c:/programdata/microsoft/windows/start menu/programs/APP.LNK");

        assert_eq!(stable_app_id(canonical), stable_app_id(canonical));
        assert_eq!(stable_app_id(canonical), stable_app_id(variant));
        assert!(stable_app_id(canonical).starts_with("app:"));
        assert_eq!(stable_app_id(canonical).len(), 4 + 32);
        let id = stable_app_id(canonical);
        let (namespace, local) = id.split_once(':').expect("namespace separator");
        assert_eq!(namespace, "app");
        assert_eq!(local.len(), 32);
    }

    #[test]
    fn different_paths_have_different_ids() {
        let first = stable_app_id(Path::new(r"C:\Start Menu\Programs\A\App.lnk"));
        let second = stable_app_id(Path::new(r"C:\Start Menu\Programs\B\App.lnk"));
        assert_ne!(first, second);
    }

    #[test]
    fn identical_titles_in_different_folders_keep_distinct_identities() {
        let tree = TempTree::new("same-title");
        tree.file("One/Tool.lnk");
        tree.file("Two/Tool.lnk");

        let entries =
            discover_from_roots(&[tree.path().to_path_buf()], DiscoverLimits::default()).unwrap();

        assert_eq!(entries.len(), 2);
        assert_ne!(entries[0].id, entries[1].id);
        assert_eq!(entries[0].title, "Tool");
        assert_eq!(entries[1].title, "Tool");
    }

    #[test]
    fn traversal_respects_depth_boundary() {
        let tree = TempTree::new("depth");
        tree.file("Alpha.lnk");
        tree.file("Tools/Beta.lnk");
        tree.file("Tools/Deep/Gamma.lnk");

        let root = tree.path().to_path_buf();
        let at = |max_depth| {
            discover_from_roots(
                std::slice::from_ref(&root),
                DiscoverLimits {
                    max_depth,
                    ..DiscoverLimits::default()
                },
            )
            .unwrap()
        };

        assert!(at(0).is_empty(), "depth 0 must not read the root");
        assert_eq!(titles(&at(1)), vec!["Alpha"]);
        assert_eq!(titles(&at(2)), vec!["Alpha", "Beta"]);
        assert_eq!(titles(&at(3)), vec!["Alpha", "Beta", "Gamma"]);
    }

    #[test]
    fn traversal_respects_entry_budget() {
        let tree = TempTree::new("budget");
        for index in 0..40 {
            tree.file(&format!("App {index:02}.lnk"));
        }

        let entries = discover_from_roots(
            &[tree.path().to_path_buf()],
            DiscoverLimits {
                max_entries: 10,
                ..DiscoverLimits::default()
            },
        )
        .unwrap();

        assert!(!entries.is_empty());
        assert!(
            entries.len() <= 10,
            "entry budget exceeded: {}",
            entries.len()
        );
    }

    #[test]
    fn traversal_respects_result_budget() {
        let tree = TempTree::new("results");
        for index in 0..10 {
            tree.file(&format!("App {index:02}.lnk"));
        }

        let entries = discover_from_roots(
            &[tree.path().to_path_buf()],
            DiscoverLimits {
                max_results: 3,
                ..DiscoverLimits::default()
            },
        )
        .unwrap();

        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn duplicate_roots_do_not_duplicate_entries() {
        let tree = TempTree::new("duplicate-roots");
        tree.file("Alpha.lnk");
        tree.file("Beta.lnk");

        let root = tree.path().to_path_buf();
        let entries =
            discover_from_roots(&[root.clone(), root], DiscoverLimits::default()).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(titles(&entries), vec!["Alpha", "Beta"]);
    }

    #[test]
    fn ignores_unsupported_extensions() {
        let tree = TempTree::new("extensions");
        tree.file("Readme.txt");
        tree.file("Shortcut.lnk");
        tree.file("Program.exe");

        let entries =
            discover_from_roots(&[tree.path().to_path_buf()], DiscoverLimits::default()).unwrap();
        assert_eq!(titles(&entries), vec!["Program", "Shortcut"]);

        let without_exe = discover_from_roots(
            &[tree.path().to_path_buf()],
            DiscoverLimits {
                include_exe: false,
                ..DiscoverLimits::default()
            },
        )
        .unwrap();
        assert_eq!(titles(&without_exe), vec!["Shortcut"]);
    }

    #[test]
    fn missing_roots_are_an_explicit_error() {
        let tree = TempTree::new("missing");
        let missing = tree.path().join("does-not-exist");
        let error = discover_from_roots(&[missing], DiscoverLimits::default()).unwrap_err();
        assert!(error.contains("no accessible Start Menu"));
    }

    #[test]
    fn symlinks_and_junctions_are_not_followed_when_available() {
        use std::os::windows::fs::{symlink_dir, symlink_file};

        let tree = TempTree::new("links");
        let outside = TempTree::new("outside");
        tree.file("Normal.lnk");
        let outside_file = outside.file("Outside.lnk");

        // Creating symlinks may require Developer Mode or elevation. If it is
        // unavailable, the regular-file assertion still runs.
        let _ = symlink_file(&outside_file, tree.path().join("LinkedFile.lnk"));
        let _ = symlink_dir(outside.path(), tree.path().join("LinkedDirectory"));

        let entries =
            discover_from_roots(&[tree.path().to_path_buf()], DiscoverLimits::default()).unwrap();

        assert!(
            entries.iter().any(|entry| entry.title == "Normal"),
            "regular entries must still be discovered"
        );
        assert!(
            entries.iter().all(|entry| entry.title != "Outside"),
            "entries behind reparse points must not be discovered"
        );
    }
}
