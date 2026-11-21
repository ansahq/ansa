//! Path normalization used by the configuration and backup guards.
//!
//! Paths may not exist yet on a fresh install, so `canonicalize` alone is not
//! sufficient. We make the path absolute, resolve `.`/`..` lexically, then use
//! the canonical path when the filesystem can provide one. Comparison is
//! case-insensitive on Windows.

use std::path::{Component, Path, PathBuf};

/// A path made absolute and lexically normalized. If the path exists, the
/// canonical (symlink-resolved) form is returned instead.
pub(crate) fn normalized(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path)
        .map(|path| lexical_normalize(&path))
        .unwrap_or_else(|_| lexical_normalize(path));
    if let Ok(canonical) = std::fs::canonicalize(&absolute) {
        return lexical_normalize(&canonical);
    }

    // The path may not exist yet (for example a backup destination). Resolve
    // the nearest existing ancestor and append the missing suffix. This keeps
    // short/long path aliases and junctions consistent for missing children.
    let mut ancestor = absolute.as_path();
    let mut missing: Vec<PathBuf> = Vec::new();
    loop {
        if ancestor.exists() {
            break;
        }
        if let Some(name) = ancestor.file_name() {
            missing.push(PathBuf::from(name));
        }
        match ancestor.parent() {
            Some(parent) if parent != ancestor => ancestor = parent,
            _ => break,
        }
    }
    let mut resolved = std::fs::canonicalize(ancestor)
        .map(|path| lexical_normalize(&path))
        .unwrap_or_else(|_| ancestor.to_path_buf());
    for name in missing.iter().rev() {
        resolved.push(name);
    }
    lexical_normalize(&resolved)
}

/// Compare two paths after normalization, case-insensitively on Windows.
pub(crate) fn equivalent(left: &Path, right: &Path) -> bool {
    comparison_key(&normalized(left)) == comparison_key(&normalized(right))
}

/// True when `child` is `parent` or lives below it.
pub(crate) fn is_within(child: &Path, parent: &Path) -> bool {
    let child = comparison_key(&normalized(child));
    let mut prefix = comparison_key(&normalized(parent));
    if !prefix.ends_with('/') {
        prefix.push('/');
    }
    child == comparison_key(&normalized(parent)) || child.starts_with(&prefix)
}

/// A SQLite sidecar path such as `ansa.sqlite-wal`.
pub(crate) fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(suffix);
    PathBuf::from(sidecar)
}

fn comparison_key(path: &Path) -> String {
    let key = path.to_string_lossy().replace('\\', "/");
    let key = if let Some(rest) = key.strip_prefix("//?/unc/") {
        format!("//{rest}")
    } else if let Some(rest) = key.strip_prefix("//?/") {
        rest.to_owned()
    } else {
        key
    };
    if cfg!(windows) {
        key.to_lowercase()
    } else {
        key
    }
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let can_pop = matches!(output.components().next_back(), Some(Component::Normal(_)));
                if can_pop {
                    output.pop();
                }
            }
            other => output.push(other.as_os_str()),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn missing_children_resolve_through_an_existing_ancestor() {
        let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("ansa-paths-{}-{sequence}", std::process::id()));
        let snapshots = root.join("snapshots");
        std::fs::create_dir_all(&snapshots).expect("create fixture");
        let child = snapshots.join("missing-backup.sqlite");

        assert!(is_within(&child, &snapshots));
        assert!(!is_within(&root.join("other").join("x.sqlite"), &snapshots));
        assert!(equivalent(&child, &child));

        std::fs::remove_dir_all(root).expect("cleanup fixture");
    }
}
