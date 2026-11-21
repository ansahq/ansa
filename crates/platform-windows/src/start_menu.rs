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
//! * `.lnk` files are decoded with the bounded parser in
//!   [`crate::shell_link`]. A target that is missing or unreadable is kept in
//!   the catalog with an explicit broken reason instead of being dropped.
//! * Entries that resolve to the same local target are deduplicated with a
//!   deterministic winner (title, then path).

use crate::shell_link;
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
///
/// Packaged applications are added by [`crate::discover_applications`] after
/// this scan; this function covers classic Start Menu files only.
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
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut inspected = 0usize;

    while let Some((path, depth)) = stack.pop() {
        if inspected >= limits.max_entries || candidates.len() >= limits.max_results {
            break;
        }
        inspected += 1;

        let link_metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };

        // Roots are allowed to be reparse points, but anything below them is
        // not traversed. This prevents junction/symlink recursion and loops.
        if depth > 0 && is_reparse_point(&link_metadata) {
            continue;
        }
        // Follow a redirected root (junction/symlink) so an enterprise profile
        // with a redirected Start Menu is not silently empty. The visited set
        // still prevents loops.
        let metadata = if depth == 0 {
            fs::metadata(&path).unwrap_or(link_metadata)
        } else {
            link_metadata
        };

        if metadata.is_dir() {
            let key = normalize_path_key(&path);
            if !visited_directories.insert(key) {
                continue;
            }
            if depth >= limits.max_depth {
                continue;
            }

            let remaining = limits
                .max_entries
                .saturating_sub(inspected)
                .min(limits.max_results.saturating_sub(candidates.len()));
            if remaining == 0 {
                break;
            }

            let read_dir = match fs::read_dir(&path) {
                Ok(read_dir) => read_dir,
                Err(_) => continue,
            };
            // Bound the child vector by the remaining traversal/result budget
            // instead of collecting an unbounded directory.
            let mut children: Vec<PathBuf> = read_dir
                .filter_map(|result| result.ok().map(|entry| entry.path()))
                .take(remaining)
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

        let Some(title) = entry_title(&path) else {
            continue;
        };
        if title.is_empty() {
            continue;
        }

        let Some(path_string) = path.to_str() else {
            // Keep the entry visible with an explicit broken reason rather than
            // silently dropping a discovered file with a non-Unicode path.
            candidates.push(Candidate {
                entry: AppEntry {
                    id: stable_app_id(&path),
                    title,
                    path: path.to_string_lossy().into_owned(),
                    target: None,
                    arguments: None,
                    working_directory: None,
                    icon_source: None,
                    packaged: false,
                    broken: Some(
                        "application path is not valid Unicode; refresh the application catalog"
                            .to_owned(),
                    ),
                },
                dedupe_key: normalize_path_key(&path),
                target_id: None,
            });
            continue;
        };

        candidates.push(build_candidate(&path, path_string.to_owned(), title));
    }

    Ok(finalize_candidates(candidates))
}

/// One discovered file before target-aware deduplication.
#[derive(Clone, Debug)]
struct Candidate {
    entry: AppEntry,
    /// Normalized local target for `.lnk` files that resolve one, otherwise the
    /// normalized link path.
    dedupe_key: String,
    /// Target-derived identity, only for shortcuts with a resolved local target.
    target_id: Option<String>,
}

fn build_candidate(path: &Path, path_string: String, title: String) -> Candidate {
    if is_lnk(path) {
        return build_shortcut_candidate(path, path_string, title);
    }

    // A direct executable's target is itself, so it uses the same
    // target-derived identity as a shortcut to it. Adding or removing an alias
    // therefore cannot change the executable's identity.
    let dedupe_key = target_identity_key(path, None, None);
    Candidate {
        entry: AppEntry {
            id: stable_target_app_id(path, None, None),
            title,
            path: path_string.clone(),
            target: Some(path_string.clone()),
            arguments: None,
            working_directory: None,
            icon_source: Some(format!("{path_string},0")),
            packaged: false,
            broken: None,
        },
        dedupe_key,
        target_id: Some(stable_target_app_id(path, None, None)),
    }
}

fn build_shortcut_candidate(path: &Path, path_string: String, title: String) -> Candidate {
    let path_key = normalize_path_key(path);

    match shell_link::resolve_shell_link_file(path) {
        Ok(resolved) => {
            let info = resolved.info;
            let target_path = resolved.target_path;
            let target_string = target_path
                .as_deref()
                .and_then(|target| target.to_str())
                .map(str::to_owned);
            let arguments = normalized_arguments(info.arguments.as_deref());
            let working_directory = info
                .working_directory
                .as_deref()
                .and_then(|directory| directory.to_str())
                .map(str::to_owned);

            let broken = match (&target_path, &target_string) {
                (Some(target), None) => Some(format!(
                    "shortcut target '{}' is not valid Unicode; refresh the application catalog after reinstalling the application",
                    target.display()
                )),
                (Some(target), Some(_)) if !target.is_file() => Some(format!(
                    "shortcut target '{}' no longer exists; refresh the application catalog after reinstalling or moving the application",
                    target.display()
                )),
                (None, _) if !resolved.attempted.is_empty() => Some(format!(
                    "shortcut target could not be resolved; tried {}; refresh the application catalog after reinstalling or moving the application",
                    describe_attempts(&resolved.attempted)
                )),
                _ => None,
            };

            let (dedupe_key, target_id) = match &target_path {
                Some(target) => (
                    target_identity_key(
                        target,
                        arguments.as_deref(),
                        working_directory.as_deref(),
                    ),
                    broken.is_none().then(|| {
                        stable_target_app_id(
                            target,
                            arguments.as_deref(),
                            working_directory.as_deref(),
                        )
                    }),
                ),
                None => (path_key.clone(), None),
            };

            let icon_source = info
                .icon_location
                .clone()
                .or_else(|| target_string.as_ref().map(|target| format!("{target},0")));

            Candidate {
                entry: AppEntry {
                    id: stable_app_id(path),
                    title,
                    path: path_string,
                    target: target_string,
                    arguments,
                    working_directory,
                    icon_source,
                    packaged: false,
                    broken,
                },
                dedupe_key,
                target_id,
            }
        }
        Err(error) => Candidate {
            entry: AppEntry {
                id: stable_app_id(path),
                title,
                path: path_string,
                target: None,
                arguments: None,
                working_directory: None,
                icon_source: None,
                packaged: false,
                broken: Some(format!(
                    "shortcut could not be resolved ({error}); refresh the application catalog after reinstalling the application"
                )),
            },
            dedupe_key: path_key,
            target_id: None,
        },
    }
}

/// Normalizes shortcut arguments for identity purposes without changing their
/// meaning: only surrounding whitespace is removed. Interior quoting and
/// spacing are preserved because they are argument-significant.
fn normalized_arguments(arguments: Option<&str>) -> Option<String> {
    arguments
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// Identity/dedupe key for a resolved target, arguments and working directory.
///
/// Working directory is included because identical target/arguments can still
/// be distinct commands when they run from different directories.
fn target_identity_key(
    target: &Path,
    arguments: Option<&str>,
    working_directory: Option<&str>,
) -> String {
    let working_directory = working_directory
        .map(|directory| normalize_path_key(Path::new(directory)))
        .unwrap_or_default();
    format!(
        "target:{}\u{1f}args:{}\u{1f}wd:{working_directory}",
        normalize_path_key(target),
        arguments.unwrap_or("")
    )
}

fn describe_attempts(attempts: &[PathBuf]) -> String {
    attempts
        .iter()
        .map(|path| format!("'{}'", path.display()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Collapses candidates that resolve to the same target **and** arguments.
///
/// The winner is the lexicographically first (title, path) pair, so refresh
/// order and filesystem enumeration order cannot change the result. A group
/// containing a resolved candidate uses the target+argument-derived identity;
/// otherwise the winner keeps its path-derived identity.
fn finalize_candidates(mut candidates: Vec<Candidate>) -> Vec<AppEntry> {
    candidates.sort_by(|left, right| {
        left.dedupe_key
            .cmp(&right.dedupe_key)
            .then_with(|| {
                left.entry
                    .title
                    .to_lowercase()
                    .cmp(&right.entry.title.to_lowercase())
            })
            .then_with(|| {
                left.entry
                    .path
                    .to_lowercase()
                    .cmp(&right.entry.path.to_lowercase())
            })
    });

    let mut entries = Vec::with_capacity(candidates.len());
    let mut start = 0;
    while start < candidates.len() {
        let mut end = start + 1;
        while end < candidates.len() && candidates[end].dedupe_key == candidates[start].dedupe_key {
            end += 1;
        }

        let winner = &candidates[start];
        let mut entry = winner.entry.clone();
        if let Some(target_id) = candidates[start..end]
            .iter()
            .find_map(|candidate| candidate.target_id.clone())
        {
            entry.id = target_id;
        }
        entries.push(entry);
        start = end;
    }

    entries.sort_by_cached_key(|entry| {
        (
            entry.title.to_lowercase(),
            entry.path.to_lowercase(),
            entry.id.clone(),
        )
    });
    entries
}

fn is_lnk(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
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
    const MAX_TITLE_CHARS: usize = 200;
    path.file_stem().map(|stem| {
        let trimmed = stem.to_string_lossy();
        let trimmed = trimmed.trim();
        if trimmed.chars().count() > MAX_TITLE_CHARS {
            trimmed.chars().take(MAX_TITLE_CHARS).collect()
        } else {
            trimmed.to_owned()
        }
    })
}

fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

    metadata.file_type().is_symlink()
        || (metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
}

/// Case-folded, separator-normalized key for path identity.
///
/// A leading `\\` (UNC/extended prefix) and a leading `\` (root-relative path)
/// are preserved and remain distinct. Duplicate separators after the prefix,
/// `.` components, `..` components and trailing separators are normalized.
pub(crate) fn normalize_path_key(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('/', "\\");
    let components: Vec<&str> = raw.split('\\').collect();
    let leading = components
        .iter()
        .take_while(|component| component.is_empty())
        .count()
        .min(2);

    let mut output: Vec<&str> = Vec::with_capacity(components.len());
    for component in &components[leading..] {
        match *component {
            "" | "." => {}
            ".." => {
                let floor = match leading {
                    2 => 2,
                    1 => 1,
                    _ => usize::from(output.first().is_some_and(|first| first.ends_with(':'))),
                };
                if output.len() > floor && output.last().is_some_and(|last| *last != "..") {
                    output.pop();
                } else {
                    output.push("..");
                }
            }
            value => output.push(value),
        }
    }

    let body = output.join("\\");
    let mut normalized = match leading {
        0 => String::new(),
        1 => "\\".to_owned(),
        _ => "\\\\".to_owned(),
    };
    normalized.push_str(&body);
    normalized.to_lowercase()
}

/// Stable v1 identity derived from the normalized path.
///
/// This does not use `DefaultHasher`, whose output is not guaranteed stable
/// across Rust versions. Two FNV-1a passes make accidental collisions
/// negligible for a Start Menu-sized catalog.
pub(crate) fn stable_app_id(path: &Path) -> String {
    stable_id_for_key(&normalize_path_key(path))
}

/// Stable identity derived from a resolved target, arguments and working
/// directory.
pub(crate) fn stable_target_app_id(
    target: &Path,
    arguments: Option<&str>,
    working_directory: Option<&str>,
) -> String {
    stable_id_for_key(&target_identity_key(target, arguments, working_directory))
}

/// Stable identity for an already-normalized key in any namespace.
pub(crate) fn stable_id_for_key(key: &str) -> String {
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
            self.file_bytes(relative, b"")
        }

        fn file_bytes(&self, relative: &str, bytes: &[u8]) -> PathBuf {
            let path = self.path.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(&path, bytes).expect("write fixture");
            path
        }

        fn shortcut(&self, relative: &str, target: &Path) -> PathBuf {
            self.file_bytes(
                relative,
                &crate::shell_link::minimal_link_fixture(&target.to_string_lossy()),
            )
        }

        fn shortcut_with_args(&self, relative: &str, target: &Path, arguments: &str) -> PathBuf {
            self.file_bytes(
                relative,
                &crate::shell_link::minimal_link_with_args_fixture(
                    &target.to_string_lossy(),
                    arguments,
                ),
            )
        }

        fn shortcut_with_args_and_working(
            &self,
            relative: &str,
            target: &Path,
            arguments: &str,
            working_directory: &str,
        ) -> PathBuf {
            self.file_bytes(
                relative,
                &crate::shell_link::link_with_args_and_working_fixture(
                    &target.to_string_lossy(),
                    arguments,
                    working_directory,
                ),
            )
        }

        fn relative_shortcut(
            &self,
            relative: &str,
            relative_target: &str,
            working_directory: &str,
        ) -> PathBuf {
            self.file_bytes(
                relative,
                &crate::shell_link::relative_link_fixture(relative_target, working_directory),
            )
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
    fn shortcuts_to_the_same_target_collapse_to_one_deterministic_entry() {
        let tree = TempTree::new("duplicate-target");
        let target = tree.file("Install/Example.exe");
        tree.shortcut("Programs/Zed Example.lnk", &target);
        tree.shortcut("Programs/Alpha Example.lnk", &target);

        let first =
            discover_from_roots(&[tree.path().to_path_buf()], DiscoverLimits::default()).unwrap();
        let second =
            discover_from_roots(&[tree.path().to_path_buf()], DiscoverLimits::default()).unwrap();

        assert_eq!(first.len(), 1, "same target must appear once: {first:?}");
        assert_eq!(first, second, "repeated scans must be idempotent");

        let entry = &first[0];
        assert_eq!(
            normalize_path_key(Path::new(entry.target.as_deref().expect("target"))),
            normalize_path_key(&target)
        );
        // The lexicographically smaller title wins regardless of discovery order.
        assert_eq!(entry.title, "Alpha Example");
        assert_eq!(
            entry.id,
            stable_target_app_id(&target, None, None),
            "identity must be target-derived"
        );
        assert!(!entry.is_broken());
    }

    #[test]
    fn a_direct_executable_and_a_shortcut_to_it_dedupe() {
        let tree = TempTree::new("exe-and-lnk");
        let target = tree.file("Example.exe");
        tree.shortcut("Programs/Example.lnk", &target);

        let entries =
            discover_from_roots(&[tree.path().to_path_buf()], DiscoverLimits::default()).unwrap();

        assert_eq!(entries.len(), 1, "duplicate rows: {entries:?}");
        assert_eq!(entries[0].path, target.to_str().unwrap());
        assert_eq!(entries[0].id, stable_target_app_id(&target, None, None));
    }

    #[test]
    fn shortcuts_with_different_targets_remain_distinct() {
        let tree = TempTree::new("distinct-targets");
        let first = tree.file("One.exe");
        let second = tree.file("Two.exe");
        tree.shortcut("Programs/Shared Name.lnk", &first);
        tree.file_bytes(
            "Programs/Shared Name (other).lnk",
            &crate::shell_link::minimal_link_fixture(&second.to_string_lossy()),
        );

        let entries =
            discover_from_roots(&[tree.path().to_path_buf()], DiscoverLimits::default()).unwrap();

        assert_eq!(entries.len(), 2);
        assert_ne!(entries[0].id, entries[1].id);
    }

    #[test]
    fn a_shortcut_with_a_missing_target_is_kept_and_marked_broken() {
        let tree = TempTree::new("broken-target");
        let missing = tree.path().join("Moved").join("Gone.exe");
        tree.shortcut("Programs/Gone.lnk", &missing);

        let entries =
            discover_from_roots(&[tree.path().to_path_buf()], DiscoverLimits::default()).unwrap();

        assert_eq!(entries.len(), 1, "broken entries must not be dropped");
        assert!(entries[0].is_broken());
        assert!(
            entries[0].broken.as_deref().is_some_and(
                |reason| reason.contains("no longer exists") && reason.contains("refresh")
            ),
            "unexpected broken reason: {:?}",
            entries[0].broken
        );
        // No target identity can be trusted for a missing target.
        assert_eq!(entries[0].id, stable_app_id(Path::new(&entries[0].path)));
    }

    #[test]
    fn a_malformed_shortcut_is_kept_and_marked_broken() {
        let tree = TempTree::new("malformed-lnk");
        tree.file_bytes("Programs/Broken.lnk", b"not a shell link");

        let entries =
            discover_from_roots(&[tree.path().to_path_buf()], DiscoverLimits::default()).unwrap();

        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_broken());
        assert!(
            entries[0]
                .broken
                .as_deref()
                .is_some_and(|reason| reason.contains("could not be resolved")),
            "unexpected broken reason: {:?}",
            entries[0].broken
        );
    }

    #[test]
    fn icon_source_metadata_is_exposed_for_extraction() {
        let tree = TempTree::new("icon-metadata");
        let target = tree.file("Example.exe");
        tree.shortcut("Programs/Example.lnk", &target);

        let entries =
            discover_from_roots(&[tree.path().to_path_buf()], DiscoverLimits::default()).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].icon_source.as_deref(),
            Some(format!("{},0", target.display()).as_str())
        );
    }

    #[test]
    fn same_target_with_different_arguments_stays_distinct() {
        let tree = TempTree::new("argument-corpus");
        let target = tree.file("cmd.exe");
        tree.shortcut("Programs/Plain.lnk", &target);
        tree.shortcut_with_args("Programs/Node prompt.lnk", &target, "/k node");
        tree.shortcut_with_args("Programs/VS prompt.lnk", &target, "/k vs");
        tree.shortcut_with_args("Programs/Node duplicate.lnk", &target, "/k node");

        let entries =
            discover_from_roots(&[tree.path().to_path_buf()], DiscoverLimits::default()).unwrap();

        assert_eq!(
            entries.len(),
            3,
            "same target with three distinct argument sets must produce three entries: {entries:?}"
        );
        let mut arguments: Vec<Option<&str>> = entries
            .iter()
            .map(|entry| entry.arguments.as_deref())
            .collect();
        arguments.sort_by(|left, right| left.unwrap_or("").cmp(right.unwrap_or("")));
        assert_eq!(arguments, vec![None, Some("/k node"), Some("/k vs")]);

        let ids: HashSet<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(ids.len(), entries.len(), "argument sets need distinct ids");
        for entry in &entries {
            assert_eq!(
                entry.id,
                stable_target_app_id(
                    &target,
                    entry.arguments.as_deref(),
                    entry.working_directory.as_deref()
                )
            );
        }
    }

    #[test]
    fn same_target_and_arguments_with_different_working_dirs_stay_distinct() {
        let tree = TempTree::new("working-dir-corpus");
        let target = tree.file("tool.exe");
        tree.shortcut_with_args_and_working(
            "Programs/From One.lnk",
            &target,
            "--same",
            r"C:\Working\One",
        );
        tree.shortcut_with_args_and_working(
            "Programs/From Two.lnk",
            &target,
            "--same",
            r"C:\Working\Two",
        );

        let entries =
            discover_from_roots(&[tree.path().join("Programs")], DiscoverLimits::default())
                .unwrap();

        assert_eq!(entries.len(), 2, "{entries:?}");
        let mut working_dirs: Vec<&str> = entries
            .iter()
            .filter_map(|entry| entry.working_directory.as_deref())
            .collect();
        working_dirs.sort_unstable();
        assert_eq!(working_dirs, vec![r"C:\Working\One", r"C:\Working\Two"]);
        assert_ne!(entries[0].id, entries[1].id);
        for entry in &entries {
            assert_eq!(entry.arguments.as_deref(), Some("--same"));
            assert_eq!(
                entry.id,
                stable_target_app_id(
                    &target,
                    entry.arguments.as_deref(),
                    entry.working_directory.as_deref()
                )
            );
        }
    }

    #[test]
    fn direct_executable_identity_is_stable_when_an_alias_is_added_or_removed() {
        let tree = TempTree::new("alias-identity");
        let target = tree.file("Example.exe");

        let before =
            discover_from_roots(&[tree.path().to_path_buf()], DiscoverLimits::default()).unwrap();
        let id_before = before[0].id.clone();
        assert_eq!(id_before, stable_target_app_id(&target, None, None));

        tree.shortcut("Programs/Example.lnk", &target);
        let with_alias =
            discover_from_roots(&[tree.path().to_path_buf()], DiscoverLimits::default()).unwrap();
        assert_eq!(with_alias.len(), 1);
        assert_eq!(with_alias[0].id, id_before);

        fs::remove_file(tree.path().join("Programs/Example.lnk")).expect("remove alias");
        let after =
            discover_from_roots(&[tree.path().to_path_buf()], DiscoverLimits::default()).unwrap();
        assert_eq!(after[0].id, id_before);
    }

    #[test]
    fn relative_shortcut_resolves_via_the_link_parent_when_working_dir_does_not() {
        let tree = TempTree::new("riot-relative");
        let target = tree.file("Riot Games/Riot Client/RiotClientServices.exe");
        tree.relative_shortcut(
            "Riot Games/Riot Client.lnk",
            r"..\Riot Games\Riot Client\RiotClientServices.exe",
            r"C:/Riot Games/Riot Client",
        );

        let entries =
            discover_from_roots(&[tree.path().to_path_buf()], DiscoverLimits::default()).unwrap();

        // The direct executable and the shortcut remain distinct commands
        // because the shortcut carries a working directory.
        assert_eq!(entries.len(), 2, "{entries:?}");
        let entry = entries
            .iter()
            .find(|entry| entry.path.ends_with("Riot Client.lnk"))
            .expect("shortcut entry");
        assert!(!entry.is_broken(), "broken: {:?}", entry.broken);
        assert_eq!(
            normalize_path_key(Path::new(entry.target.as_deref().expect("target"))),
            normalize_path_key(&target)
        );
        assert_eq!(
            entry.id,
            stable_target_app_id(&target, None, Some(r"C:/Riot Games/Riot Client"))
        );
        assert_eq!(
            entry.working_directory.as_deref(),
            Some(r"C:/Riot Games/Riot Client")
        );
    }

    #[test]
    fn unresolved_relative_shortcut_lists_every_attempted_path() {
        let tree = TempTree::new("relative-missing");
        tree.relative_shortcut(
            "Programs/Missing.lnk",
            r"..\Gone\Missing.exe",
            r"C:\Does\Not\Exist",
        );

        let entries =
            discover_from_roots(&[tree.path().to_path_buf()], DiscoverLimits::default()).unwrap();

        assert_eq!(entries.len(), 1);
        let reason = entries[0].broken.as_deref().expect("broken reason");
        assert!(reason.contains("tried"), "{reason}");
        assert!(reason.contains("Does"), "{reason}");
        assert!(reason.contains("refresh"), "{reason}");
    }

    #[test]
    fn unc_and_root_relative_paths_do_not_collide() {
        let unc = normalize_path_key(Path::new(r"\\server\share\app.exe"));
        let root_relative = normalize_path_key(Path::new(r"\server\share\app.exe"));
        let extended = normalize_path_key(Path::new(r"\\?\C:\app.exe"));

        assert_ne!(unc, root_relative);
        assert!(unc.starts_with(r"\\server\share"));
        assert!(root_relative.starts_with(r"\server\share"));
        assert!(extended.starts_with(r"\\?\c:"));
        assert_ne!(
            stable_app_id(Path::new(r"\\server\share\app.exe")),
            stable_app_id(Path::new(r"\server\share\app.exe"))
        );
    }

    #[test]
    fn dot_components_are_normalized_for_identity() {
        assert_eq!(
            normalize_path_key(Path::new(r"C:\one\..\two\.\app.exe")),
            r"c:\two\app.exe"
        );
        assert_ne!(
            normalize_path_key(Path::new(r"C:\one\..\two\app.exe")),
            normalize_path_key(Path::new(r"C:\other\two\app.exe"))
        );
    }

    #[test]
    fn junction_roots_are_traversed_when_available() {
        use std::os::windows::fs::symlink_dir;

        let tree = TempTree::new("junction-root");
        let target = TempTree::new("junction-target");
        target.file("Programs/App.lnk");
        let linked = tree.path().join("LinkedStartMenu");
        if symlink_dir(target.path(), &linked).is_err() {
            return; // Developer Mode or elevation is unavailable.
        }

        let entries = discover_from_roots(&[linked], DiscoverLimits::default()).unwrap();

        assert!(
            entries.iter().any(|entry| entry.title == "App"),
            "redirected root must be traversed: {entries:?}"
        );
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
