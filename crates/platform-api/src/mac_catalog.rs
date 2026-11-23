//! Bounded discovery of macOS bundles by CFBundleIdentifier.

use crate::catalog::{self, FileStamp};
use crate::AppEntry;
use plist::stream::{Event, Reader};
use std::collections::BTreeMap;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

const MAX_PLIST_BYTES: usize = 256 * 1024;
const MAX_PLIST_EVENTS: usize = 12_288;
const MAX_PLIST_DEPTH: usize = 24;
pub(crate) const MAX_ICON_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Bundle {
    pub entry: AppEntry,
    pub path: PathBuf,
    pub canonical: Option<PathBuf>,
    pub bundle_id: Option<String>,
    alias_stamp: Option<FileStamp>,
    plist_stamp: Option<FileStamp>,
    executable_stamp: Option<FileStamp>,
}

pub(crate) fn discover(roots: &[PathBuf]) -> Result<BTreeMap<String, Bundle>, String> {
    let mut found: BTreeMap<String, (usize, bool, Bundle)> = BTreeMap::new();
    let mut used = 0;
    for (root, path) in catalog::candidates(roots, "app", 4)? {
        let Some(bundle) = inspect(&path, &mut used, catalog::next_selection())? else {
            continue;
        };
        let rank = roots
            .iter()
            .position(|item| item == &root)
            .unwrap_or(roots.len());
        let alias = bundle.alias_stamp.is_some();
        let key = bundle.entry.id.clone();
        if let Some((other_rank, other_alias, other)) = found.get(&key) {
            if (
                other.entry.is_broken(),
                *other_rank,
                *other_alias,
                &other.path,
            ) <= (bundle.entry.is_broken(), rank, alias, &path)
            {
                continue;
            }
        }
        found.insert(key, (rank, alias, bundle));
    }
    Ok(found
        .into_values()
        .map(|(_, _, bundle)| (bundle.entry.path.clone(), bundle))
        .collect())
}

pub(crate) fn recheck(original: &Bundle) -> Result<Bundle, String> {
    // Reuse the retained token so equality below compares bundle content, not
    // the discovery generation. A replaced bundle at the same path and ID still
    // differs in its plist/executable/alias stamps.
    let current = inspect(&original.path, &mut 0, original.entry.selection)?
        .ok_or_else(catalog::stale_target)?;
    if let Some(reason) = &current.entry.broken {
        return Err(format!("{reason}. Refresh applications and try again"));
    }
    if &current != original {
        return Err(
            "Application bundle changed since discovery. Refresh applications and try again"
                .to_owned(),
        );
    }
    Ok(current)
}

fn inspect(path: &Path, budget: &mut usize, selection: u64) -> Result<Option<Bundle>, String> {
    let Some(raw) = path.to_str() else {
        return Ok(None);
    };
    let Some(fallback_title) = path.file_stem().and_then(|name| name.to_str()) else {
        return Ok(None);
    };
    let canonical = fs::canonicalize(path).ok();
    let alias_stamp = fs::symlink_metadata(path)
        .ok()
        .filter(|file| !file.is_dir())
        .as_ref()
        .map(FileStamp::new);
    #[cfg(target_os = "macos")]
    let canonical = canonical.and_then(|alias| {
        // Do not ask Foundation to resolve arbitrary large non-bundle files.
        if fs::metadata(&alias)
            .is_ok_and(|file| file.is_file() && file.len() <= MAX_PLIST_BYTES as u64)
        {
            crate::imp::resolve_bundle_alias(&alias)
        } else {
            Some(alias)
        }
    });
    let mut entry = AppEntry::new(broken_id(raw), fallback_title, raw);
    entry.selection = selection;
    let mut bundle = Bundle {
        entry,
        path: path.to_path_buf(),
        canonical,
        bundle_id: None,
        alias_stamp,
        plist_stamp: None,
        executable_stamp: None,
    };
    let root = match bundle.canonical.as_deref() {
        Some(root) if root.is_dir() => root,
        _ => {
            bundle.entry.broken =
                Some("Application bundle is missing or is not a directory".to_owned());
            return Ok(Some(bundle));
        }
    };
    let plist_path = root.join("Contents/Info.plist");
    let result =
        inside(&plist_path, root).and_then(|path| catalog::bounded_file(&path, MAX_PLIST_BYTES));
    let (bytes, stamp) = match result {
        Ok(result) => result,
        Err(reason) => {
            bundle.entry.broken = Some(format!(
                "Cannot read the application bundle's Info.plist: {reason}"
            ));
            return Ok(Some(bundle));
        }
    };
    bundle.plist_stamp = Some(stamp);
    catalog::spend(budget, bytes.len())?;
    let fields = match metadata(&bytes) {
        Ok(fields) => fields,
        Err(reason) => {
            bundle.entry.broken = Some(format!("Invalid application Info.plist: {reason}"));
            return Ok(Some(bundle));
        }
    };
    if fields
        .get("CFBundlePackageType")
        .is_some_and(|kind| kind != "APPL")
    {
        return Ok(None);
    }
    let identifier = fields
        .get("CFBundleIdentifier")
        .filter(|value| valid_identifier(value));
    let Some(identifier) = identifier else {
        bundle.entry.broken = Some("Application has no valid CFBundleIdentifier".to_owned());
        return Ok(Some(bundle));
    };
    bundle.bundle_id = Some(identifier.clone());
    bundle.entry.id = format!("app:mac-{}", identifier.to_ascii_lowercase());
    if let Some(title) = ["CFBundleDisplayName", "CFBundleName"]
        .into_iter()
        .filter_map(|name| fields.get(name))
        .find(|title| !title.trim().is_empty() && !title.chars().any(char::is_control))
    {
        bundle.entry.title = title.clone();
    }
    if let Some(icon) = fields
        .get("CFBundleIconFile")
        .filter(|value| simple_name(value))
    {
        let mut icon_path = root.join("Contents/Resources").join(icon);
        if icon_path.extension().is_none() {
            icon_path.set_extension("icns");
        }
        if let Ok(icon_path) = inside(&icon_path, root) {
            if is_icon_path(&icon_path) {
                bundle.entry.icon_source = icon_path.to_str().map(str::to_owned);
            }
        }
    }
    let Some(executable) = fields
        .get("CFBundleExecutable")
        .filter(|name| simple_name(name))
    else {
        bundle.entry.broken = Some("Application has no valid CFBundleExecutable".to_owned());
        return Ok(Some(bundle));
    };
    let executable = inside(&root.join("Contents/MacOS").join(executable), root);
    match executable.and_then(|path| {
        let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
        if !catalog::usable_executable(&metadata) {
            return Err("the file is not executable".to_owned());
        }
        Ok((path, metadata))
    }) {
        Ok((path, metadata)) => {
            bundle.entry.target = path.to_str().map(str::to_owned);
            bundle.executable_stamp = Some(FileStamp::new(&metadata));
        }
        Err(reason) => {
            bundle.entry.broken = Some(format!("Application executable is unavailable: {reason}"))
        }
    }
    Ok(Some(bundle))
}

fn inside(path: &Path, root: &Path) -> Result<PathBuf, String> {
    let canonical = fs::canonicalize(path).map_err(|error| error.to_string())?;
    if canonical.starts_with(root) {
        Ok(canonical)
    } else {
        Err("bundle metadata points outside the application bundle".to_owned())
    }
}

fn is_icon_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("icns"))
        && fs::metadata(path)
            .is_ok_and(|file| file.is_file() && file.len() <= MAX_ICON_BYTES as u64)
}

pub(crate) fn icon(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() || !is_icon_path(path) {
        return Err("Application icon is not a bounded local ICNS resource".to_owned());
    }
    let canonical =
        fs::canonicalize(path).map_err(|error| format!("Application icon unavailable: {error}"))?;
    let (bytes, _) = catalog::bounded_file(&canonical, MAX_ICON_BYTES)?;
    let header = bytes
        .get(0..8)
        .ok_or("Application icon has a truncated ICNS header")?;
    let size = u32::from_be_bytes([header[4], header[5], header[6], header[7]]);
    if &header[0..4] != b"icns" || size as usize != bytes.len() || size <= 8 {
        return Err("Application icon has an invalid ICNS container".to_owned());
    }
    Ok(canonical)
}

fn simple_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !matches!(name, "." | "..")
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(char::is_control)
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'.' | b'-'))
        && value.split('.').all(|part| !part.is_empty())
}

fn broken_id(path: &str) -> String {
    use std::fmt::Write as _;
    // `_` cannot appear in a valid CFBundleIdentifier in this module, so this
    // broken-path namespace can never collide with an `app:mac-<bundle-id>`.
    let mut id = String::from("app:mac-broken_");
    for byte in path.bytes() {
        let _ = write!(id, "{byte:02x}");
    }
    id
}

fn metadata(bytes: &[u8]) -> Result<BTreeMap<String, String>, String> {
    if !bytes.starts_with(b"bplist00")
        && !bytes
            .iter()
            .copied()
            .find(|byte| !byte.is_ascii_whitespace())
            .is_some_and(|byte| byte == b'<' || byte == 0xef)
    {
        return Err("expected XML or binary plist".to_owned());
    }
    let mut events = Reader::new(Cursor::new(bytes));
    if !matches!(events.next(), Some(Ok(Event::StartDictionary(_)))) {
        return Err("Info.plist does not start with a dictionary".to_owned());
    }
    let mut depth = 1usize;
    let mut expecting_key = true;
    let mut key: Option<&'static str> = None;
    let mut values = BTreeMap::new();
    for (index, event) in events.enumerate() {
        if index >= MAX_PLIST_EVENTS || depth > MAX_PLIST_DEPTH {
            return Err("Info.plist exceeded collection bounds".to_owned());
        }
        let event = event.map_err(|error| error.to_string())?;
        if depth == 0 {
            return Err("Info.plist contains data after its main dictionary".to_owned());
        }

        if depth == 1 {
            match event {
                Event::String(value) if expecting_key => {
                    // Retain only recognized keys; the empty string is a
                    // sentinel for an unrelated key whose value is skipped.
                    key = Some(match value.as_ref() {
                        "CFBundleIdentifier" => "CFBundleIdentifier",
                        "CFBundleDisplayName" => "CFBundleDisplayName",
                        "CFBundleName" => "CFBundleName",
                        "CFBundleIconFile" => "CFBundleIconFile",
                        "CFBundleExecutable" => "CFBundleExecutable",
                        "CFBundlePackageType" => "CFBundlePackageType",
                        _ => "",
                    });
                    expecting_key = false;
                    continue;
                }
                Event::String(value) => {
                    if let Some(name) = key.filter(|name| !name.is_empty()) {
                        if value.len() > 512
                            || values.insert(name.to_owned(), value.to_string()).is_some()
                        {
                            return Err(
                                "Info.plist contains duplicate or oversized bundle metadata"
                                    .to_owned(),
                            );
                        }
                    }
                    expecting_key = true;
                    key = None;
                    continue;
                }
                Event::StartArray(_) | Event::StartDictionary(_) => {
                    if expecting_key {
                        return Err("Info.plist has a non-string dictionary key".to_owned());
                    }
                    if key.is_some_and(|name| !name.is_empty()) {
                        let name = key.unwrap_or_default();
                        return Err(format!("Info.plist value {name} must be a string"));
                    }
                    // An unrelated key may carry a nested value, which is
                    // skipped while its collection is open.
                    expecting_key = true;
                    key = None;
                    depth += 1;
                    continue;
                }
                Event::EndCollection => {
                    if !expecting_key {
                        return Err("Info.plist is missing a dictionary value".to_owned());
                    }
                    depth = 0;
                    expecting_key = true;
                    key = None;
                    continue;
                }
                _ if expecting_key => {
                    return Err("Info.plist has a non-string dictionary key".to_owned());
                }
                _ => {
                    if key.is_some_and(|name| !name.is_empty()) {
                        let name = key.unwrap_or_default();
                        return Err(format!("Info.plist value {name} must be a string"));
                    }
                    expecting_key = true;
                    key = None;
                    continue;
                }
            }
        }

        match event {
            Event::StartArray(_) | Event::StartDictionary(_) => depth += 1,
            Event::EndCollection => {
                depth = depth.saturating_sub(1);
                if depth == 1 {
                    // A nested value consumed the pending unrelated key.
                    expecting_key = true;
                    key = None;
                }
            }
            _ => {}
        }
    }
    if depth != 0 || !expecting_key || key.is_some() {
        return Err("Info.plist is incomplete".to_owned());
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use plist::{Dictionary, Value};

    fn bundle(root: &Path, name: &str, id: &str, executable: bool, binary: bool) -> PathBuf {
        let root = root.join(name);
        fs::create_dir_all(root.join("Contents/MacOS")).unwrap();
        let mut dictionary = Dictionary::new();
        for (key, value) in [
            ("CFBundleIdentifier", id),
            ("CFBundleDisplayName", "Example & Editor"),
            ("CFBundleExecutable", "app-bin"),
            ("CFBundlePackageType", "APPL"),
        ] {
            dictionary.insert(key.to_owned(), Value::String(value.to_owned()));
        }
        dictionary.insert(
            "Unrelated".to_owned(),
            Value::Array(vec![Value::String("CFBundleIdentifier".to_owned())]),
        );
        let value = Value::Dictionary(dictionary);
        if binary {
            value
                .to_file_binary(root.join("Contents/Info.plist"))
                .unwrap();
        } else {
            value.to_file_xml(root.join("Contents/Info.plist")).unwrap();
        }
        if executable {
            let path = root.join("Contents/MacOS/app-bin");
            fs::write(&path, b"synthetic bundle executable").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        root
    }

    #[test]
    fn healthy_copy_wins_deterministically_and_bundle_id_survives_move() {
        let tmp = tempfile::tempdir().unwrap();
        let user = tmp.path().join("User Applications");
        let system = tmp.path().join("System Applications");
        let broken = bundle(&user, "A.app", "com.example.shared", false, false);
        let preferred = bundle(&user, "Z.app", "com.example.shared", true, true);
        let other = bundle(&system, "A.app", "com.example.shared", true, false);
        let nested = bundle(
            &user.join("Suite"),
            "Other.app",
            "com.example.nested",
            true,
            false,
        );
        let roots = [user.clone(), system];
        let initial = discover(&roots).unwrap();
        assert_eq!(initial.len(), 2);
        let selected = &initial[preferred.to_str().unwrap()];
        assert_eq!(selected.entry.id, "app:mac-com.example.shared");
        assert_eq!(selected.entry.title, "Example & Editor");
        assert!(initial.contains_key(nested.to_str().unwrap()));
        assert!(!initial.contains_key(broken.to_str().unwrap()));
        assert!(!initial.contains_key(other.to_str().unwrap()));
        let moved = user.join("Moved.app");
        fs::rename(preferred, &moved).unwrap();
        assert!(recheck(selected).unwrap_err().contains("Refresh"));
        assert_eq!(
            discover(&roots).unwrap()[moved.to_str().unwrap()].entry.id,
            selected.entry.id
        );
    }

    #[test]
    fn replaced_binary_and_malformed_plist_fail_before_launch() {
        let tmp = tempfile::tempdir().unwrap();
        let app = bundle(tmp.path(), "Example.app", "com.example.app", true, false);
        let malformed = tmp.path().join("Malformed.app");
        fs::create_dir_all(malformed.join("Contents")).unwrap();
        fs::write(
            malformed.join("Contents/Info.plist"),
            b"<plist><dict><key>CFBundleIdentifier",
        )
        .unwrap();
        let catalog = discover(&[tmp.path().to_path_buf()]).unwrap();
        let original = &catalog[app.to_str().unwrap()];
        assert!(recheck(original).is_ok());
        assert!(catalog[malformed.to_str().unwrap()].entry.is_broken());
        fs::write(app.join("Contents/MacOS/app-bin"), b"another build").unwrap();
        assert!(recheck(original)
            .unwrap_err()
            .contains("changed since discovery"));
    }

    #[test]
    fn rediscovery_at_the_same_bundle_path_and_id_rejects_the_retained_selection() {
        let tmp = tempfile::tempdir().unwrap();
        let app = bundle(tmp.path(), "Example.app", "com.example.app", true, false);
        let roots = [tmp.path().to_path_buf()];
        let first = discover(&roots).unwrap();
        let retained = &first[app.to_str().unwrap()];
        let retained_token = retained.entry.selection;
        assert!(recheck(retained).is_ok());

        // Replace the executable without changing the bundle path or
        // CFBundleIdentifier. Rediscovery must issue a new token.
        fs::write(app.join("Contents/MacOS/app-bin"), b"replacement binary").unwrap();
        let second = discover(&roots).unwrap();
        let replacement = &second[app.to_str().unwrap()];
        assert_eq!(replacement.entry.id, retained.entry.id);
        assert_eq!(replacement.entry.path, retained.entry.path);
        assert_ne!(replacement.entry.selection, retained_token);
        assert!(!catalog::selection_matches(
            &replacement.entry,
            &retained.entry
        ));
        assert!(recheck(retained).is_err());
        assert!(recheck(replacement).is_ok());

        // Replacing the manifest at the same identity invalidates the
        // rediscovered row, while the next rediscovery stays launchable.
        let second_token = replacement.entry.selection;
        plist::Value::Dictionary({
            let mut dictionary = plist::Dictionary::new();
            for (key, value) in [
                ("CFBundleIdentifier", "com.example.app"),
                ("CFBundleDisplayName", "Example & Editor"),
                ("CFBundleExecutable", "app-bin"),
                ("CFBundlePackageType", "APPL"),
            ] {
                dictionary.insert(key.to_owned(), plist::Value::String(value.to_owned()));
            }
            dictionary
        })
        .to_file_xml(app.join("Contents/Info.plist"))
        .unwrap();
        let third = discover(&roots).unwrap();
        let refreshed = &third[app.to_str().unwrap()];
        assert_eq!(refreshed.entry.id, retained.entry.id);
        assert_ne!(refreshed.entry.selection, second_token);
        assert!(recheck(replacement).is_err());
        assert!(recheck(refreshed).is_ok());
    }

    #[test]
    fn broken_path_ids_cannot_collide_with_valid_bundle_ids() {
        let broken = broken_id(r"/Applications/Bad Copy.app");
        assert!(broken.starts_with("app:mac-broken_"));
        assert!(!valid_identifier(broken.strip_prefix("app:mac-").unwrap()));
        assert_ne!(
            broken,
            format!("app:mac-{}", "broken_2f4170706c69636174696f6e73")
        );
    }

    #[test]
    fn non_string_top_level_dictionary_key_is_rejected() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
    <array><string>not-a-key</string></array>
    <key>CFBundleIdentifier</key>
    <string>com.example.app</string>
</dict>
</plist>"#;
        let error = metadata(xml).unwrap_err();
        assert!(
            error.contains("non-string dictionary key"),
            "unexpected parser result: {error}"
        );
        // A nested collection under an unrelated string key is still skipped,
        // while recognized keys and duplicate checks keep working.
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
    <key>Unrelated</key>
    <dict><key>CFBundleIdentifier</key><string>ignored</string></dict>
    <key>CFBundleIdentifier</key>
    <string>com.example.app</string>
</dict>
</plist>"#;
        let values = metadata(xml).unwrap();
        assert_eq!(values["CFBundleIdentifier"], "com.example.app");
    }

    #[test]
    fn duplicate_bundle_id_never_substitutes_the_other_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let user_root = tmp.path().join("User Applications");
        let system_root = tmp.path().join("System Applications");
        let user = bundle(&user_root, "User.app", "com.example.duplicate", true, false);
        let system = bundle(
            &system_root,
            "System.app",
            "com.example.duplicate",
            true,
            false,
        );
        let roots = [user_root, system_root];
        let first = discover(&roots).unwrap();
        assert_eq!(first.len(), 1);
        let selected = &first[user.to_str().unwrap()];
        assert_eq!(selected.entry.id, "app:mac-com.example.duplicate");
        assert!(!first.contains_key(system.to_str().unwrap()));

        // Changing the non-selected copy must not change, redirect or break the
        // selected row; recheck only inspects the selected path.
        fs::write(system.join("Contents/MacOS/app-bin"), b"other build").unwrap();
        assert!(recheck(selected).is_ok());
        let second = discover(&roots).unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[user.to_str().unwrap()].entry.id, selected.entry.id);

        // Once the selected copy is gone, a stale row fails before launch and
        // discovery may select the other copy as a new, separately identified
        // row. The old row is never rewritten to point at it.
        fs::remove_dir_all(&user).unwrap();
        assert!(recheck(selected).unwrap_err().contains("Refresh"));
        let third = discover(&roots).unwrap();
        assert_eq!(third.len(), 1);
        assert!(third.contains_key(system.to_str().unwrap()));
    }

    #[test]
    fn large_tree_and_bounded_icon_checks() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..200 {
            bundle(
                tmp.path(),
                &format!("{i}.app"),
                &format!("com.example.app-{i}"),
                true,
                i % 2 == 0,
            );
        }
        assert_eq!(discover(&[tmp.path().to_path_buf()]).unwrap().len(), 200);
        let icon_path = tmp.path().join("app.icns");
        fs::write(&icon_path, b"icns\0\0\0\x0cdata").unwrap();
        assert!(icon(&icon_path).is_ok());
        fs::write(&icon_path, b"icns\0\0\0\x0bdata").unwrap();
        assert!(icon(&icon_path).is_err());
    }
}
