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
    std::fs::canonicalize(&absolute)
        .map(|path| lexical_normalize(&path))
        .unwrap_or(absolute)
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
