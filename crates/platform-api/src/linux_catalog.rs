//! XDG application metadata, kept independent of the native Linux launcher.

use crate::catalog::{self, FileStamp, MAX_ROOTS};
use crate::AppEntry;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const MAX_DESKTOP_BYTES: usize = 64 * 1024;
const MAX_EXEC_BYTES: usize = 16 * 1024;
const MAX_ARGC: usize = 128;
const MAX_PATH_ENTRIES: usize = 128;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Environment {
    pub roots: Vec<PathBuf>,
    pub search_path: Vec<PathBuf>,
    pub desktops: Vec<String>,
}

impl Environment {
    #[cfg(target_os = "linux")]
    pub(crate) fn current() -> Self {
        let variables = ["XDG_DATA_HOME", "HOME", "XDG_DATA_DIRS", "PATH"];
        let [home, user, dirs, path] = variables.map(std::env::var_os);
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        Self {
            roots: data_roots(home.as_deref(), user.as_deref(), dirs.as_deref())
                .into_iter()
                .map(|dir| dir.join("applications"))
                .collect(),
            search_path: executable_roots(path.as_deref()),
            desktops: desktops(&desktop),
        }
    }
}

pub(crate) fn data_roots(
    data_home: Option<&OsStr>,
    home: Option<&OsStr>,
    data_dirs: Option<&OsStr>,
) -> Vec<PathBuf> {
    let user = data_home
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| {
            home.map(PathBuf::from)
                .filter(|dir| dir.is_absolute())
                .map(|dir| dir.join(".local/share"))
        });
    let dirs = data_dirs
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsStr::new("/usr/local/share:/usr/share"));
    absolute_unique(
        user.into_iter().chain(std::env::split_paths(dirs)),
        MAX_ROOTS,
    )
}

pub(crate) fn executable_roots(path: Option<&OsStr>) -> Vec<PathBuf> {
    let fallback = if cfg!(windows) {
        ""
    } else {
        "/usr/local/bin:/usr/bin:/bin"
    };
    absolute_unique(
        std::env::split_paths(path.unwrap_or_else(|| OsStr::new(fallback))),
        MAX_PATH_ENTRIES,
    )
}

fn absolute_unique(paths: impl Iterator<Item = PathBuf>, limit: usize) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    paths
        .take(limit * 4)
        .filter(|dir| dir.is_absolute() && seen.insert(dir.clone()))
        .take(limit)
        .collect()
}

pub(crate) fn desktops(value: &str) -> Vec<String> {
    value
        .split(':')
        .take(32)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Application {
    pub entry: AppEntry,
    pub path: PathBuf,
    canonical_manifest: Option<PathBuf>,
    manifest_stamp: Option<FileStamp>,
    pub launch: Option<Exec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Exec {
    /// Keep the original absolute path for multicall binaries and script argv0.
    pub executable: PathBuf,
    canonical_executable: PathBuf,
    stamp: FileStamp,
    pub arguments: Vec<String>,
    pub working_directory: Option<PathBuf>,
    pub terminal: bool,
}

impl Exec {
    pub(crate) fn command(&self, terminal: Option<(&Path, &str)>) -> Command {
        let mut command = if let Some((program, separator)) = terminal {
            let mut command = Command::new(program);
            command.arg(separator).arg(&self.executable);
            command
        } else {
            Command::new(&self.executable)
        };
        command.args(&self.arguments);
        if let Some(directory) = &self.working_directory {
            command.current_dir(directory);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }
}

#[derive(Debug)]
struct Executable {
    path: PathBuf,
    canonical: PathBuf,
    stamp: FileStamp,
}

#[cfg(target_os = "linux")]
pub(crate) fn find_program(program: &str, search_path: &[PathBuf]) -> Option<PathBuf> {
    executable(program, search_path).map(|item| item.path)
}

fn executable(program: &str, search_path: &[PathBuf]) -> Option<Executable> {
    if program.is_empty()
        || program.len() > 4096
        || program.chars().any(char::is_control)
        || program.starts_with('-')
        || program.contains('=')
    {
        return None;
    }
    let path = Path::new(program);
    if path.is_absolute() {
        return executable_at(path.to_path_buf());
    }
    if program.contains(['/', '\\']) || matches!(program, "." | "..") {
        return None;
    }
    search_path
        .iter()
        .find_map(|root| executable_at(root.join(path)))
}

fn executable_at(path: PathBuf) -> Option<Executable> {
    let canonical = fs::canonicalize(&path).ok()?;
    let metadata = fs::metadata(&canonical).ok()?;
    catalog::usable_executable(&metadata).then(|| Executable {
        path,
        canonical,
        stamp: FileStamp::new(&metadata),
    })
}

pub(crate) fn discover(environment: &Environment) -> Result<BTreeMap<String, Application>, String> {
    let mut desktop_ids = BTreeSet::new();
    let mut targets = BTreeSet::new();
    let mut output = BTreeMap::new();
    let mut used = 0;
    for (root, path) in catalog::candidates(&environment.roots, "desktop", 8)? {
        let Some(desktop_id) = desktop_id(&root, &path) else {
            continue;
        };
        // XDG precedence uses the lossy flattened desktop ID. The first
        // manifest with that ID wins, including Hidden=true and unlaunchable
        // user overrides. Never revive a masked lower entry. The launcher's
        // stable identity below remains lossless even when two paths flatten
        // to the same desktop ID.
        if !desktop_ids.insert(desktop_id.clone()) {
            continue;
        }
        let Some(stable_id) = stable_id(&root, &path) else {
            continue;
        };
        let Some(app) = inspect(
            &path,
            &desktop_id,
            &stable_id,
            environment,
            &mut used,
            catalog::next_selection(),
        )?
        else {
            continue;
        };
        if let Some(exec) = &app.launch {
            // The invoked executable path is argv0 identity. Two differently
            // named symlinks to one multicall binary can behave differently,
            // so they must not collapse to the first row.
            let key = (
                &exec.executable,
                &exec.canonical_executable,
                &exec.arguments,
                &exec.working_directory,
                exec.terminal,
            );
            if !targets.insert((
                key.0.clone(),
                key.1.clone(),
                key.2.clone(),
                key.3.clone(),
                key.4,
            )) {
                continue;
            }
        }
        output.insert(app.entry.path.clone(), app);
    }
    Ok(output)
}

/// The XDG desktop-file ID. This is intentionally lossy: relative path
/// separators become `-`, so `a-b/c.desktop` and `a/b-c.desktop` both produce
/// `a-b-c.desktop`. Precedence/masking must use this value.
fn desktop_id(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let parts: Option<Vec<&str>> = relative.iter().map(OsStr::to_str).collect();
    Some(parts?.join("-"))
}

/// The launcher identity, derived losslessly from every relative path
/// component. Components are percent-encoded and joined by encoded `%2F`, so a
/// literal `%2F` in a name cannot collide with a real separator. Flattened XDG
/// desktop-ID collisions therefore keep distinct, stable CommandIds even though
/// only the first path is exposed by precedence.
fn stable_id(root: &Path, path: &Path) -> Option<String> {
    use std::fmt::Write as _;
    let relative = path.strip_prefix(root).ok()?;
    let parts: Option<Vec<&str>> = relative.iter().map(OsStr::to_str).collect();
    let mut id = String::from("app:xdg-");
    for (index, part) in parts?.into_iter().enumerate() {
        if index != 0 {
            id.push_str("%2F");
        }
        for byte in part.bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_') {
                id.push(char::from(byte));
            } else {
                let _ = write!(id, "%{byte:02X}");
            }
        }
    }
    Some(id)
}

/// Resolve the currently winning path for a flattened desktop ID, using the
/// same root order and sorted candidate order as discovery.
fn effective_path(id: &str, environment: &Environment) -> Result<Option<PathBuf>, String> {
    // Use the same whole-root-set walk and limits as discovery. Scanning roots
    // separately would let a launch succeed after the global scan budget had
    // been exceeded, which would publish a result discovery itself refuses.
    Ok(catalog::candidates(&environment.roots, "desktop", 8)?
        .into_iter()
        .find(|(root, path)| desktop_id(root, path).as_deref() == Some(id))
        .map(|(_, path)| path))
}

pub(crate) fn recheck(original: &Application, environment: &Environment) -> Result<Exec, String> {
    let Some((id, stable)) = environment.roots.iter().find_map(|root| {
        let id = desktop_id(root, &original.path)?;
        let stable = stable_id(root, &original.path)?;
        Some((id, stable))
    }) else {
        return Err(catalog::stale_target());
    };
    // Re-resolve the flattened desktop ID before touching the selected file.
    // A newly added higher-priority override, or a Hidden/NoDisplay/TryExec
    // mask, makes the old selected row stale instead of launching a lower copy.
    match effective_path(&id, environment)? {
        Some(winner) if winner == original.path => {}
        Some(_) => {
            return Err("A different or higher-priority desktop entry now owns this desktop ID. Refresh applications and try again".to_owned())
        }
        None => return Err(catalog::stale_target()),
    }
    // Reuse the retained token so metadata equality is not defeated by a new
    // discovery token. A replacement at the same path and ID still differs in
    // its manifest/executable stamps and is rejected below.
    let current = inspect(
        &original.path,
        &id,
        &stable,
        environment,
        &mut 0,
        original.entry.selection,
    )?;
    let Some(current) = current else {
        return Err(catalog::stale_target());
    };
    if let Some(reason) = &current.entry.broken {
        return Err(format!("{reason}. Refresh applications and try again"));
    }
    if &current != original {
        return Err("Desktop entry or executable changed since discovery. Refresh applications and try again".to_owned());
    }
    current.launch.ok_or_else(catalog::stale_target)
}

fn inspect(
    path: &Path,
    desktop_id: &str,
    stable_id: &str,
    environment: &Environment,
    budget: &mut usize,
    selection: u64,
) -> Result<Option<Application>, String> {
    let Some(raw_path) = path.to_str() else {
        return Ok(None);
    };
    let mut entry = AppEntry::new(stable_id, desktop_id, raw_path);
    entry.selection = selection;
    let mut app = Application {
        entry,
        path: path.to_path_buf(),
        canonical_manifest: fs::canonicalize(path).ok(),
        manifest_stamp: None,
        launch: None,
    };
    let (bytes, stamp) = match catalog::bounded_file(path, MAX_DESKTOP_BYTES) {
        Ok(data) => data,
        Err(reason) => {
            app.entry.broken = Some(format!("Cannot read desktop entry: {reason}"));
            return Ok(Some(app));
        }
    };
    app.manifest_stamp = Some(stamp);
    catalog::spend(budget, bytes.len())?;
    let fields = match std::str::from_utf8(&bytes)
        .map_err(|_| "desktop entry is not UTF-8")
        .and_then(fields)
    {
        Ok(fields) => fields,
        Err(reason) => {
            app.entry.broken = Some(reason.to_owned());
            return Ok(Some(app));
        }
    };
    let get = |key: &str| fields.get(key).copied();
    if let Err(reason) = parse_entry(&mut app, &get, environment) {
        app.entry.broken = Some(reason);
    } else if app.launch.is_none() {
        return Ok(None);
    }
    Ok(Some(app))
}

fn parse_entry<'a>(
    app: &mut Application,
    get: &impl Fn(&str) -> Option<&'a str>,
    environment: &Environment,
) -> Result<(), String> {
    let boolean = |key| match get(key) {
        Some("true") => Ok(true),
        None | Some("false") => Ok(false),
        Some(_) => Err(format!("Invalid desktop entry boolean {key}")),
    };
    if boolean("Hidden")? || boolean("NoDisplay")? {
        return Ok(());
    }
    if let Some(kind) = get("Type") {
        if kind != "Application" {
            return Ok(());
        }
    } else {
        return Err("Desktop entry lacks Type=Application".to_owned());
    }
    let matches_desktop = |list: &str| {
        list.split(';')
            .filter(|item| !item.is_empty())
            .any(|item| environment.desktops.iter().any(|desk| desk == item))
    };
    if get("OnlyShowIn").is_some_and(|list| !matches_desktop(list))
        || get("NotShowIn").is_some_and(matches_desktop)
    {
        return Ok(());
    }
    if let Some(try_exec) = get("TryExec") {
        if executable(&unescape(try_exec)?, &environment.search_path).is_none() {
            return Ok(());
        }
    }
    let title = get("Name")
        .ok_or("Desktop entry lacks Name")
        .and_then(|raw| unescape(raw).map_err(|_| "Desktop entry has invalid Name escapes"))?;
    if title.trim().is_empty() || title.len() > 512 || title.chars().any(char::is_control) {
        return Err("Desktop entry Name is empty, invalid or too long".to_owned());
    }
    app.entry.title = title;
    app.entry.icon_source = get("Icon")
        .and_then(|raw| unescape(raw).ok())
        .filter(|icon| {
            !icon.is_empty() && icon.len() <= 1024 && !icon.chars().any(char::is_control)
        });
    let terminal = boolean("Terminal")?;
    let working_directory = get("Path")
        .filter(|raw| !raw.is_empty())
        .map(|raw| {
            let path = PathBuf::from(unescape(raw)?);
            if !path.is_absolute() || !path.is_dir() {
                return Err("Desktop entry working directory is missing or not absolute".to_owned());
            }
            fs::canonicalize(path)
                .map_err(|error| format!("Cannot resolve working directory: {error}"))
        })
        .transpose()?;
    let raw_exec = get("Exec").ok_or("Desktop entry lacks an Exec fallback for launching")?;
    app.entry.arguments = Some(raw_exec.to_owned());
    let argv = parse_exec(
        raw_exec,
        &app.entry.title,
        &app.entry.path,
        app.entry.icon_source.as_deref(),
    )?;
    let (program, args) = argv
        .split_first()
        .ok_or("Desktop entry Exec has no executable")?;
    let exec = executable(program, &environment.search_path).ok_or_else(|| {
        format!("Desktop entry executable not found or not executable: {program}")
    })?;
    if is_shell(&exec.path) || is_shell(&exec.canonical) {
        return Err("Desktop entry invokes a shell or command wrapper directly; this launcher does not evaluate shell commands".to_owned());
    }
    app.entry.target = exec.path.to_str().map(str::to_owned);
    app.entry.working_directory = working_directory
        .as_ref()
        .and_then(|path| path.to_str())
        .map(str::to_owned);
    app.launch = Some(Exec {
        executable: exec.path,
        canonical_executable: exec.canonical,
        stamp: exec.stamp,
        arguments: args.to_vec(),
        working_directory,
        terminal,
    });
    Ok(())
}

fn is_shell(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| {
            matches!(
                name,
                "sh" | "bash"
                    | "dash"
                    | "ash"
                    | "zsh"
                    | "fish"
                    | "ksh"
                    | "mksh"
                    | "csh"
                    | "tcsh"
                    | "busybox"
                    | "env"
                    | "eval"
                    | "command"
                    | "sudo"
                    | "pkexec"
                    | "setsid"
                    | "nohup"
                    | "xargs"
            )
        })
}

fn fields(source: &str) -> Result<BTreeMap<&str, &str>, &'static str> {
    let mut in_main = false;
    let mut found = false;
    let mut fields = BTreeMap::new();
    for source_line in source.trim_start_matches('\u{feff}').lines() {
        let line = source_line.trim_start_matches([' ', '\t']);
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            in_main = line.trim_end() == "[Desktop Entry]";
            if in_main && std::mem::replace(&mut found, true) {
                return Err("Desktop entry contains duplicate [Desktop Entry] groups");
            }
        } else if in_main {
            let (key, value) = line
                .split_once('=')
                .ok_or("Malformed desktop entry key/value")?;
            if key.is_empty() || fields.insert(key.trim_end(), value).is_some() {
                return Err("Desktop entry has an empty or duplicated key");
            }
        }
    }
    if found {
        Ok(fields)
    } else {
        Err("Missing [Desktop Entry] group")
    }
}

fn unescape(input: &str) -> Result<String, String> {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            result.push(match chars.next() {
                Some('s') => ' ',
                Some('n') => '\n',
                Some('t') => '\t',
                Some('r') => '\r',
                Some('\\') => '\\',
                _ => return Err("Invalid desktop entry escape sequence".to_owned()),
            });
        } else {
            result.push(ch);
        }
    }
    Ok(result)
}

#[derive(Debug)]
struct Word {
    text: String,
    quoted: bool,
}

fn tokenize(exec: &str) -> Result<Vec<Word>, String> {
    let mut words = Vec::new();
    let mut current: Option<Word> = None;
    let mut inside = false;
    let mut chars = exec.chars();
    while let Some(ch) = chars.next() {
        if ch.is_control() && (ch != '\t' || inside) {
            return Err("Desktop entry Exec contains a control character".to_owned());
        }
        if !inside && matches!(ch, ' ' | '\t') {
            if let Some(word) = current.take() {
                words.push(word);
            }
            if words.len() >= MAX_ARGC {
                return Err("Desktop entry Exec has too many arguments".to_owned());
            }
            continue;
        }
        let word = current.get_or_insert_with(|| Word {
            text: String::new(),
            quoted: false,
        });
        if ch == '"' {
            if !inside && (!word.text.is_empty() || word.quoted) {
                return Err("Desktop entry Exec requires quoting the entire argument".to_owned());
            }
            word.quoted = true;
            inside = !inside;
        } else if inside {
            if ch == '\\' {
                match chars.next() {
                    Some(escaped @ ('"' | '\\' | '`' | '$')) => word.text.push(escaped),
                    _ => return Err("Invalid escaped character in quoted desktop Exec".to_owned()),
                }
            } else if matches!(ch, '`' | '$') {
                return Err(
                    "Desktop entry Exec contains an unescaped reserved character".to_owned(),
                );
            } else {
                word.text.push(ch);
            }
        } else if word.quoted || "'\\><~|&;$*?#()`".contains(ch) {
            return Err(
                "Desktop entry Exec contains invalid quoting or an unquoted reserved character"
                    .to_owned(),
            );
        } else {
            word.text.push(ch);
        }
    }
    if inside {
        return Err("Desktop entry Exec has an unterminated quote".to_owned());
    }
    if let Some(word) = current {
        words.push(word);
    }
    Ok(words)
}

fn parse_exec(
    raw: &str,
    name: &str,
    desktop_path: &str,
    icon: Option<&str>,
) -> Result<Vec<String>, String> {
    if raw.len() > MAX_EXEC_BYTES {
        return Err("Desktop entry Exec is too long".to_owned());
    }
    let mut words = tokenize(&unescape(raw)?)?.into_iter();
    let program = words.next().ok_or("Desktop entry Exec is empty")?;
    if program.text.is_empty() || program.text.contains('%') {
        return Err("Desktop entry Exec cannot derive its executable from a field code".to_owned());
    }
    let mut argv = vec![program.text];
    let (mut file_codes, mut expanded_bytes) = (0, argv[0].len());
    for word in words {
        // No files or URLs are supplied by the application-launch action.
        let expansion = match word.text.as_str() {
            "%i" if !word.quoted => icon
                .into_iter()
                .flat_map(|icon| ["--icon", icon])
                .map(str::to_owned)
                .collect(),
            "%f" | "%F" | "%u" | "%U" if !word.quoted => {
                file_codes += 1;
                if file_codes > 1 {
                    return Err("Desktop entry Exec has multiple file/URL field codes".to_owned());
                }
                Vec::new()
            }
            "%d" | "%D" | "%n" | "%N" | "%v" | "%m" if !word.quoted => Vec::new(),
            _ => {
                let mut expanded = String::new();
                let mut chars = word.text.chars();
                while let Some(ch) = chars.next() {
                    if ch != '%' {
                        expanded.push(ch);
                        continue;
                    }
                    match chars.next() {
                        Some('%') => expanded.push('%'),
                        Some('c') if !word.quoted => expanded.push_str(name),
                        Some('k') if !word.quoted => expanded.push_str(desktop_path),
                        _ => {
                            return Err(
                                "Unknown, quoted or malformed desktop Exec field code".to_owned()
                            )
                        }
                    }
                    if expanded.len() > MAX_EXEC_BYTES {
                        return Err("Expanded desktop Exec argument is too long".to_owned());
                    }
                }
                vec![expanded]
            }
        };
        expanded_bytes += expansion.iter().map(String::len).sum::<usize>();
        argv.extend(expansion);
        if argv.len() > MAX_ARGC || expanded_bytes > MAX_EXEC_BYTES {
            return Err("Expanded desktop Exec exceeds argument limits".to_owned());
        }
    }
    Ok(argv)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, Environment) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let system = tmp.path().join("system");
        let bin = tmp.path().join("bin");
        for dir in [&home, &system, &bin] {
            fs::create_dir(dir).unwrap();
        }
        for name in ["viewer", "editor", "sh"] {
            let path = bin.join(name);
            fs::write(&path, b"fixture").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        (
            tmp,
            Environment {
                roots: vec![home, system],
                search_path: vec![bin],
                desktops: desktops("GNOME:GNOME-Classic"),
            },
        )
    }

    fn entry(root: &Path, path: &str, extra: &str) -> PathBuf {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            format!("[Desktop Entry]\nType=Application\nName=Example\n{extra}\n"),
        )
        .unwrap();
        path
    }

    #[test]
    fn xdg_roots_honor_precedence_and_ignore_relative_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let declared = tmp.path().join("flatpak/exports/share");
        let dirs = std::env::join_paths([Path::new("relative"), &declared, &declared]).unwrap();
        assert_eq!(
            data_roots(
                Some(OsStr::new("relative")),
                Some(home.as_os_str()),
                Some(&dirs)
            ),
            [home.join(".local/share"), declared.clone()]
        );
        assert_eq!(executable_roots(Some(&dirs)), [declared]);
    }

    #[test]
    fn desktop_visibility_precedence_and_target_dedup_are_deterministic() {
        let (_tmp, env) = fixture();
        let home = &env.roots[0];
        let system = &env.roots[1];
        entry(home, "nested/blocked.desktop", "Hidden=true");
        entry(system, "nested-blocked.desktop", "Exec=editor --hidden");
        entry(home, "nodisplay.desktop", "NoDisplay=true");
        entry(system, "nodisplay.desktop", "Exec=editor --nodisplay");
        entry(
            system,
            "not-here.desktop",
            "NotShowIn=KDE;GNOME;\nExec=editor --wrong",
        );
        entry(
            home,
            "missing-try.desktop",
            "TryExec=missing\nExec=editor --wrong",
        );
        entry(
            system,
            "missing-try.desktop",
            "Exec=editor --should-be-masked",
        );
        entry(system, "a-system.desktop", "Exec=viewer %U");
        let user = entry(
            home,
            "chosen.desktop",
            "OnlyShowIn=GNOME;\nTryExec=viewer\nExec=viewer %u",
        );
        let distinct = entry(system, "different.desktop", "Exec=viewer --another");
        let entries = discover(&env).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[user.to_str().unwrap()].entry.id,
            "app:xdg-chosen.desktop"
        );
        assert_eq!(
            entries[distinct.to_str().unwrap()]
                .launch
                .as_ref()
                .unwrap()
                .arguments,
            ["--another"]
        );
    }

    #[test]
    fn exec_parsing_preserves_literal_argv_and_expands_only_once() {
        let args = parse_exec(
            r#"viewer "space and ; | text" --title=%c %i %k %U 85%% "literal \\$HOME" """#,
            "$(literal) %U",
            "/file name.deck",
            Some("icon with spaces"),
        )
        .unwrap();
        assert_eq!(
            args,
            [
                "viewer",
                "space and ; | text",
                "--title=$(literal) %U",
                "--icon",
                "icon with spaces",
                "/file name.deck",
                "85%",
                "literal $HOME",
                ""
            ]
        );
        // A bare %i is removed when the entry has no Icon, and a quoted whole
        // program path stays a single literal argv element.
        assert_eq!(
            parse_exec("viewer %i --flag", "name", "/file", None).unwrap(),
            ["viewer", "--flag"]
        );
        assert_eq!(
            parse_exec("\"/opt/My App/bin\" --flag", "name", "/file", None).unwrap(),
            ["/opt/My App/bin", "--flag"]
        );
        for invalid in [
            "viewer x;attack",
            "viewer %Q",
            "viewer %u %F",
            "viewer prefix%F",
            "%c flag",
            "viewer \"unterminated",
            "viewer \"$HOME\"",
            "viewer \"a\"x",
            "viewer \"%k\"",
            "viewer arg\\q",
            "viewer first\0second",
        ] {
            assert!(
                parse_exec(invalid, "test", "/file", None).is_err(),
                "{invalid:?}"
            );
        }
    }

    #[test]
    fn stale_replaced_removed_and_malformed_appear_actionably() {
        let (_tmp, env) = fixture();
        let root = &env.roots[0];
        let path = entry(root, "good.desktop", "Exec=viewer --initial\nTerminal=true");
        let shell = entry(root, "shell.desktop", "Exec=sh -c \"malicious command\"");
        let malformed = entry(root, "bad.desktop", "Exec=view %Q");
        let entries = discover(&env).unwrap();
        let selected = &entries[path.to_str().unwrap()];
        assert!(recheck(selected, &env).unwrap().terminal);
        assert!(entries[shell.to_str().unwrap()].entry.is_broken());
        assert!(entries[malformed.to_str().unwrap()].entry.is_broken());
        entry(root, "good.desktop", "Exec=viewer --replacement");
        assert!(recheck(selected, &env)
            .unwrap_err()
            .contains("changed since discovery"));
        fs::remove_file(path).unwrap();
        assert!(recheck(selected, &env).unwrap_err().contains("Refresh"));
        assert_eq!(discover(&env).unwrap().len(), 2);
    }

    #[test]
    fn rediscovery_at_the_same_path_and_id_rejects_the_retained_selection() {
        let (_tmp, env) = fixture();
        let root = &env.roots[0];
        let path = entry(root, "selected.desktop", "Exec=viewer --initial");
        let first = discover(&env).unwrap();
        let retained = &first[path.to_str().unwrap()];
        let retained_token = retained.entry.selection;
        assert!(recheck(retained, &env).is_ok());

        // Replace the manifest without changing its path, desktop ID or
        // launcher ID. Rediscovery must issue a new selection token.
        entry(root, "selected.desktop", "Exec=viewer --replacement");
        let second = discover(&env).unwrap();
        let replacement = &second[path.to_str().unwrap()];
        assert_eq!(replacement.entry.id, retained.entry.id);
        assert_eq!(replacement.entry.path, retained.entry.path);
        assert_ne!(replacement.entry.selection, retained_token);
        assert!(!catalog::selection_matches(
            &replacement.entry,
            &retained.entry
        ));
        assert!(recheck(retained, &env)
            .unwrap_err()
            .contains("changed since discovery"));
        assert!(recheck(replacement, &env).is_ok());

        // Replacing the executable behind the same path also invalidates the
        // retained row; the rediscovered row remains launchable.
        let third_token = replacement.entry.selection;
        fs::write(env.search_path[0].join("viewer"), b"replacement executable").unwrap();
        let third = discover(&env).unwrap();
        let refreshed = &third[path.to_str().unwrap()];
        assert_eq!(refreshed.entry.id, retained.entry.id);
        assert_ne!(refreshed.entry.selection, third_token);
        assert!(recheck(replacement, &env).is_err());
        assert!(recheck(refreshed, &env).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn distinct_argv0_symlinks_to_one_multicall_binary_survive_dedup() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let (_tmp, env) = fixture();
        let bin = &env.search_path[0];
        let multicall = bin.join("multicall");
        fs::write(&multicall, b"fixture multicall").unwrap();
        fs::set_permissions(&multicall, fs::Permissions::from_mode(0o700)).unwrap();
        let first_name = bin.join("first-caller");
        let second_name = bin.join("second-caller");
        symlink(&multicall, &first_name).unwrap();
        symlink(&multicall, &second_name).unwrap();
        assert_eq!(
            fs::canonicalize(&first_name).unwrap(),
            fs::canonicalize(&second_name).unwrap()
        );
        let root = &env.roots[0];
        let first = entry(root, "first-caller.desktop", "Exec=first-caller --shared");
        let second = entry(root, "second-caller.desktop", "Exec=second-caller --shared");
        let entries = discover(&env).unwrap();
        assert_eq!(entries.len(), 2, "distinct argv0 launchers were collapsed");
        assert_eq!(
            entries[first.to_str().unwrap()]
                .launch
                .as_ref()
                .unwrap()
                .executable,
            first_name
        );
        assert_eq!(
            entries[second.to_str().unwrap()]
                .launch
                .as_ref()
                .unwrap()
                .executable,
            second_name
        );
    }

    #[test]
    fn flattened_ids_keep_lossless_identity_and_deterministic_precedence() {
        let (_tmp, env) = fixture();
        let home = &env.roots[0];
        let system = &env.roots[1];
        // Both relative paths flatten to the same XDG desktop ID.
        let nested = entry(home, "a-b/c.desktop", "Exec=viewer --nested");
        let sibling = entry(home, "a/b-c.desktop", "Exec=viewer --sibling");
        entry(system, "a-b-c.desktop", "Exec=viewer --system");
        assert_eq!(
            desktop_id(home, &nested).as_deref(),
            desktop_id(home, &sibling).as_deref()
        );
        assert_ne!(stable_id(home, &nested), stable_id(home, &sibling));
        // Discovery's per-root candidate order decides ties deterministically;
        // derive the expected winner from that same bounded walk.
        let flattened: Vec<_> = catalog::candidates(std::slice::from_ref(home), "desktop", 8)
            .unwrap()
            .into_iter()
            .map(|(_, path)| path)
            .filter(|path| desktop_id(home, path).as_deref() == Some("a-b-c.desktop"))
            .collect();
        assert_eq!(flattened.len(), 2);
        let winner = flattened[0].clone();
        let loser = flattened[1].clone();
        let entries = discover(&env).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries.contains_key(winner.to_str().unwrap()));
        assert!(!entries.contains_key(loser.to_str().unwrap()));
        assert!(!entries.contains_key(system.join("a-b-c.desktop").to_str().unwrap()));
        assert_eq!(
            entries[winner.to_str().unwrap()].entry.id,
            stable_id(home, &winner).unwrap()
        );
        let expected_arguments = if winner == nested {
            "--nested"
        } else {
            "--sibling"
        };
        assert_eq!(
            entries[winner.to_str().unwrap()]
                .launch
                .as_ref()
                .unwrap()
                .arguments,
            [expected_arguments]
        );
        // Removing the winner reveals the next path under the same flattened
        // ID, but with a different stable identity rather than a recycled one.
        fs::remove_file(&winner).unwrap();
        let next = discover(&env).unwrap();
        assert_eq!(next.len(), 1);
        assert!(next.contains_key(loser.to_str().unwrap()));
        assert!(!next.contains_key(winner.to_str().unwrap()));
        assert_eq!(
            next[loser.to_str().unwrap()].entry.id,
            stable_id(home, &loser).unwrap()
        );
        assert_ne!(
            next[loser.to_str().unwrap()].entry.id,
            stable_id(home, &winner).unwrap()
        );
    }

    #[test]
    fn later_higher_priority_override_or_hidden_mask_invalidates_selected_row() {
        let (_tmp, env) = fixture();
        let home = &env.roots[0];
        let system = &env.roots[1];
        let selected_path = entry(system, "selected.desktop", "Exec=viewer --system");
        let first = discover(&env).unwrap();
        let selected = &first[selected_path.to_str().unwrap()];
        assert!(recheck(selected, &env).is_ok());

        let user_path = entry(home, "selected.desktop", "Exec=viewer --user");
        assert!(recheck(selected, &env)
            .unwrap_err()
            .contains("higher-priority"));
        let user_catalog = discover(&env).unwrap();
        assert_eq!(user_catalog.len(), 1);
        assert!(user_catalog.contains_key(user_path.to_str().unwrap()));
        assert!(!user_catalog.contains_key(selected_path.to_str().unwrap()));
        assert!(recheck(&user_catalog[user_path.to_str().unwrap()], &env).is_ok());

        // A Hidden user override masks the lower copy and also makes the old
        // system selection stale before any executable can be inspected.
        entry(home, "selected.desktop", "Hidden=true");
        assert!(recheck(selected, &env).is_err());
        assert!(discover(&env).unwrap().is_empty());
    }

    #[test]
    fn terminal_command_records_literal_argv_and_working_directory() {
        let (tmp, env) = fixture();
        let path = &env.roots[0];
        let first_directory = tmp.path().join("first directory");
        let second_directory = tmp.path().join("other directory");
        let escape = |path: &Path| path.to_str().unwrap().replace('\\', "\\\\");
        for dir in [&first_directory, &second_directory] {
            fs::create_dir(dir).unwrap();
        }
        let spec = |dir: &Path| {
            format!(
                "Terminal=true\nPath={}\nExec=viewer \"one ; | argument\" %k --percent=100%% %U",
                escape(dir)
            )
        };
        let first = entry(path, "first.desktop", &spec(&first_directory));
        let second = entry(path, "second.desktop", &spec(&second_directory));
        let entries = discover(&env).unwrap();
        let exec = entries[first.to_str().unwrap()].launch.as_ref().unwrap();
        let terminal = env.search_path[0].join("synthetic-terminal");
        let command = exec.command(Some((&terminal, "--")));
        let actual_args: Vec<_> = command.get_args().collect();
        assert_eq!(command.get_program(), terminal);
        assert_eq!(
            command.get_current_dir(),
            fs::canonicalize(&first_directory).ok().as_deref()
        );
        assert_eq!(
            actual_args,
            [
                OsStr::new("--"),
                exec.executable.as_os_str(),
                OsStr::new("one ; | argument"),
                first.as_os_str(),
                OsStr::new("--percent=100%"),
            ]
        );
        assert!(entries.contains_key(second.to_str().unwrap()));
        // Without per-file %k, the working directory alone must keep both rows.
        for (entry_path, dir) in [
            ("first.desktop", first_directory),
            ("second.desktop", second_directory),
        ] {
            entry(
                path,
                entry_path,
                &format!("Path={}\nExec=viewer --literal", escape(&dir)),
            );
        }
        assert_eq!(discover(&env).unwrap().len(), 2);
    }
}
