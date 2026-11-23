//! Conservative bounded lookup of PNG resources from the XDG icon theme.

use crate::catalog;
use std::collections::{BTreeSet, VecDeque};
use std::path::{Component, Path, PathBuf};

const MAX_ICON_BYTES: usize = 2 * 1024 * 1024;
const MAX_INDEX_BYTES: usize = 128 * 1024;
const MAX_LOOKUPS: usize = 4096;

pub(crate) fn resolve(
    source: &str,
    roots: &[PathBuf],
    requested_theme: &str,
) -> Result<PathBuf, String> {
    if source.is_empty() || source.len() > 1024 || source.chars().any(char::is_control) {
        return Err("Invalid application icon source".to_owned());
    }
    let raw_path = Path::new(source);
    if raw_path.is_absolute() {
        return png(raw_path).ok_or_else(|| "Absolute icon source is not a bounded PNG".to_owned());
    }
    if !simple_component(source) {
        return Err("Icon theme names cannot contain a path".to_owned());
    }
    let filename = if source.ends_with(".png") {
        source.to_owned()
    } else {
        format!("{source}.png")
    };
    let mut queue = VecDeque::new();
    if simple_component(requested_theme) {
        queue.push_back(requested_theme.to_owned());
    }
    let mut seen = BTreeSet::new();
    let mut lookups = 0;
    for _ in 0..8 {
        let theme = match queue.pop_front() {
            Some(theme) => theme,
            None if !seen.contains("hicolor") => "hicolor".to_owned(),
            None => break,
        };
        if !seen.insert(theme.clone()) {
            continue;
        }
        for root in roots.iter().take(catalog::MAX_ROOTS) {
            let path = root.join("icons").join(&theme);
            if !path.is_dir() {
                continue;
            }
            let index = catalog::bounded_file(&path.join("index.theme"), MAX_INDEX_BYTES).ok();
            let index = index
                .as_ref()
                .and_then(|(bytes, _)| std::str::from_utf8(bytes).ok());
            let (directories, inherits) = theme_fields(index.unwrap_or_default());
            for inherited in inherits
                .iter()
                .copied()
                .filter(|name| simple_component(name))
            {
                if queue.len() < 8 && !seen.contains(inherited) {
                    queue.push_back(inherited.to_owned());
                }
            }
            let preferred = [
                "64x64/apps",
                "48x48/apps",
                "128x128/apps",
                "32x32/apps",
                "256x256/apps",
                "24x24/apps",
                "16x16/apps",
            ];
            for directory in preferred.into_iter().chain(directories) {
                if !safe_subdir(directory) {
                    continue;
                }
                lookups += 1;
                if lookups > MAX_LOOKUPS {
                    return Err("Icon theme lookup exceeded its search limit".to_owned());
                }
                if let Some(icon) = png(&path.join(directory).join(&filename)) {
                    return Ok(icon);
                }
            }
        }
    }
    roots
        .iter()
        .take(catalog::MAX_ROOTS)
        .find_map(|root| png(&root.join("pixmaps").join(&filename)))
        .ok_or_else(|| {
            "No bounded PNG was found in the configured icon theme; use a generic icon".to_owned()
        })
}

fn theme_fields(index: &str) -> (Vec<&str>, Vec<&str>) {
    let mut active = false;
    let mut directories = Vec::new();
    let mut inherits = Vec::new();
    for line in index.lines() {
        if line.trim_start().starts_with('[') {
            active = line.trim() == "[Icon Theme]";
        }
        if active {
            if let Some((key, value)) = line.split_once('=') {
                let list = value
                    .split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty());
                match key.trim() {
                    "Directories" | "ScaledDirectories" => {
                        directories.extend(list.take(64_usize.saturating_sub(directories.len())))
                    }
                    "Inherits" => {
                        inherits.extend(list.take(8_usize.saturating_sub(inherits.len())))
                    }
                    _ => {}
                }
            }
        }
    }
    (directories, inherits)
}

fn safe_subdir(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.contains(['\\', ':'])
        && name.split('/').all(simple_component)
        && Path::new(name)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn simple_component(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !matches!(name, "." | "..")
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(char::is_control)
}

fn png(path: &Path) -> Option<PathBuf> {
    let path = std::fs::canonicalize(path).ok()?;
    let (bytes, _) = catalog::bounded_file(&path, MAX_ICON_BYTES).ok()?;
    let header = bytes.get(..24)?;
    if &header[..16] != b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR" {
        return None;
    }
    let width = u32::from_be_bytes(header[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(header[20..24].try_into().ok()?);
    ((1..=512).contains(&width) && (1..=512).contains(&height)).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn lookup_checks_theme_inheritance_and_falls_back_to_hicolor() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = [tmp.path().to_path_buf()];
        let child = tmp.path().join("icons/Child");
        let parent = tmp.path().join("icons/Parent/not-usual");
        let fallback = tmp.path().join("icons/hicolor/48x48/apps");
        for dir in [&child, &parent, &fallback] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(child.join("index.theme"), "[Icon Theme]\nInherits=Parent\n").unwrap();
        fs::write(
            parent.parent().unwrap().join("index.theme"),
            "[Icon Theme]\nDirectories=../Child,not-usual\n",
        )
        .unwrap();
        let image = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x30\0\0\0\x30,not-decoded";
        let theme_icon = parent.join("editor.png");
        let fallback_icon = fallback.join("editor.png");
        fs::write(&theme_icon, image).unwrap();
        fs::write(&fallback_icon, image).unwrap();
        assert_eq!(
            resolve("editor", &roots, "Child"),
            fs::canonicalize(&theme_icon).map_err(|error| error.to_string())
        );
        fs::write(&theme_icon, b"invalid png").unwrap();
        assert_eq!(
            resolve("editor", &roots, "Child"),
            fs::canonicalize(&fallback_icon).map_err(|error| error.to_string())
        );
        assert!(resolve("../editor", &roots, "Child").is_err());
    }
}
