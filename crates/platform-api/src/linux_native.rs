//! Linux native handoff: only argv, never a command-language string.

use crate::catalog::{self, Snapshot};
use crate::host_lifecycle::HostBackend;
use crate::linux_catalog::{self, Application, Environment, Exec};
use crate::{
    opener, AppEntry, CapabilityStatus, HostCapabilities, LaunchOutcome, OpenerCapabilities,
    Platform,
};
use ansa_quicklinks::{OpenPolicy, Opener};
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

static CATALOG: Snapshot<Application> = Snapshot::new();

pub(super) fn catalog_watch_roots() -> Result<Vec<PathBuf>, String> {
    Ok(Environment::current().roots)
}

pub(super) fn icon(source: &str) -> Result<PathBuf, String> {
    let mut roots: Vec<_> = Environment::current()
        .roots
        .into_iter()
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .collect();
    let default = if std::env::var("XDG_CURRENT_DESKTOP")
        .is_ok_and(|desktop| desktop.split(':').any(|name| name == "KDE"))
    {
        "breeze"
    } else {
        "Adwaita"
    };
    let name = std::env::var("XDG_ICON_THEME").unwrap_or_else(|_| default.to_owned());
    if let Some(home) = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
    {
        roots.insert(0, home);
    }
    crate::linux_icons::resolve(source, &roots, &name)
}

pub(super) fn discover_applications() -> Result<Vec<AppEntry>, String> {
    let snapshot = linux_catalog::discover(&Environment::current())?;
    let mut entries: Vec<_> = snapshot.values().map(|app| app.entry.clone()).collect();
    entries.sort_by(|a, b| (&a.title, &a.id).cmp(&(&b.title, &b.id)));
    CATALOG.replace(snapshot);
    Ok(entries)
}

pub(super) fn launch_or_switch(path: &str, expected_id: Option<&str>) -> LaunchOutcome {
    let environment = Environment::current();
    let result = CATALOG
        .get(path, || discover_applications().map(|_| ()))
        .and_then(|selected| {
            if expected_id.is_some_and(|id| id != selected.entry.id) {
                return Err(catalog::stale_target());
            }
            linux_catalog::recheck(&selected, &environment)
        });
    finish_launch(result, &environment)
}

pub(super) fn launch_or_switch_selected(entry: &AppEntry) -> LaunchOutcome {
    let environment = Environment::current();
    let result = CATALOG
        .get_selected(
            entry,
            |selected| &selected.entry,
            || discover_applications().map(|_| ()),
        )
        .and_then(|selected| linux_catalog::recheck(&selected, &environment));
    finish_launch(result, &environment)
}

fn finish_launch(result: Result<Exec, String>, environment: &Environment) -> LaunchOutcome {
    let exec = match result {
        Ok(exec) => exec,
        Err(reason) => return LaunchOutcome::NoTarget { reason },
    };
    match launch(&exec, environment) {
        Ok(()) => LaunchOutcome::Launched,
        Err(error) => LaunchOutcome::Failed { reason: error },
    }
}

fn launch(exec: &Exec, environment: &Environment) -> Result<(), String> {
    let terminal = if exec.terminal {
        Some([("xdg-terminal-exec", "--"), ("gnome-terminal", "--"),
            ("konsole", "-e"), ("xterm", "-e")]
            .into_iter()
            .find_map(|(name, separator)| {
                linux_catalog::find_program(name, &environment.search_path)
                    .map(|terminal| (terminal, separator))
            })
            .ok_or("This application requires a terminal. Install xdg-terminal-exec or a supported terminal emulator")?)
    } else {
        None
    };
    let mut command = exec.command(
        terminal
            .as_ref()
            .map(|(path, separator)| (path.as_path(), *separator)),
    );
    run_detachable(
        &mut command,
        "Application launch",
        Duration::from_millis(35),
        true,
    )
}

pub(super) fn open_url(url: &str) -> Result<(), String> {
    opener::web_url(url)?;
    let scheme = if url
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http:"))
    {
        "x-scheme-handler/http"
    } else {
        "x-scheme-handler/https"
    };
    let helpers = Helpers::current()?;
    helpers.handler(scheme)?;
    helpers.open(url)
}

pub(super) fn open_document(target: &opener::LocalTarget) -> Result<(), String> {
    let helpers = Helpers::current()?;
    open_document_with(target, &helpers, query, open_target)
}

/// Runs the document queries with an injectable helper boundary so a test can
/// replace the target during the default-handler query without spawning a
/// helper process. Production always passes the bounded process helpers above.
fn open_document_with(
    target: &opener::LocalTarget,
    helpers: &Helpers,
    query: impl Fn(&Path, &[&str]) -> Result<String, String>,
    open: impl Fn(&Path, &opener::LocalTarget) -> Result<(), String>,
) -> Result<(), String> {
    let handoff = target.handoff_str()?.to_owned();
    let utility = helpers
        .xdg_mime
        .as_deref()
        .ok_or("Install xdg-utils (xdg-mime) to locate a desktop handler")?;
    let mime = query_mime(utility, &handoff, &query)?;
    // Reject a replacement that happened during the file-type query before
    // asking for a handler.
    target.revalidate()?;
    query_handler(utility, &mime, &query)?;
    // Revalidate after the default-handler query. `open_target` repeats this
    // after the shared child lock wait and immediately before spawn.
    target.revalidate()?;
    let utility = helpers
        .xdg_open
        .as_deref()
        .ok_or("Install xdg-utils (xdg-open), or open the target manually")?;
    open(utility, target)
}

pub(super) fn reveal_in_file_manager(path: &str) -> Result<(), String> {
    let path = opener::reveal_path(path)?;
    let path = path.to_str().ok_or("Reveal path is not UTF-8")?;
    let target =
        opener::LocalTarget::capture(path, Opener::FileManager, OpenPolicy::with_local_paths())?;
    // Capture the fallback folder before any helper. If FileManager1 fails
    // after a parent retarget, the fallback must not adopt a fresh stamp.
    let folder = containing_folder(&target)?;
    let helpers = Helpers::current()?;
    reveal_with_helpers(&target, &folder, &helpers, show_items, query, open_target)
}

fn containing_folder(target: &opener::LocalTarget) -> Result<opener::LocalTarget, String> {
    let folder = target
        .handoff()
        .parent()
        .unwrap_or_else(|| target.handoff());
    let folder = folder.to_str().ok_or("Containing folder is not UTF-8")?;
    let folder =
        opener::LocalTarget::capture(folder, Opener::FileManager, OpenPolicy::with_local_paths())?;
    if !folder.is_directory() {
        return Err("The containing folder no longer exists. Refresh and try again".to_owned());
    }
    Ok(folder)
}

fn reveal_with_helpers(
    target: &opener::LocalTarget,
    folder: &opener::LocalTarget,
    helpers: &Helpers,
    show_items: impl FnOnce(&Path, &opener::LocalTarget) -> bool,
    query: impl Fn(&Path, &[&str]) -> Result<String, String>,
    open: impl Fn(&Path, &opener::LocalTarget) -> Result<(), String>,
) -> Result<(), String> {
    if let Some(gdbus) = &helpers.gdbus {
        // Revalidate the exact selected object before asking FileManager1 to
        // select it; the ShowItems request itself is not a payload handoff.
        target.revalidate()?;
        if show_items(gdbus, target) {
            return Ok(());
        }
    }
    // On failure, use the folder captured before ShowItems. The first check
    // detects a parent retargeted during that helper.
    reveal_directory_with(folder, helpers, query, open)
}

fn show_items(gdbus: &Path, target: &opener::LocalTarget) -> bool {
    let Ok(items) = file_uri(target.handoff()).map(|uri| format!("['{uri}']")) else {
        return false;
    };
    let mut command = quiet(gdbus);
    command.args([
        "call",
        "--session",
        "--timeout",
        "1",
        "--dest",
        "org.freedesktop.FileManager1",
        "--object-path",
        "/org/freedesktop/FileManager1",
        "--method",
        "org.freedesktop.FileManager1.ShowItems",
        &items,
        "",
    ]);
    run_bounded(
        &mut command,
        "File manager reveal",
        Duration::from_millis(1250),
    )
    .is_ok()
}

/// Degrades to displaying a directory, never opening the file. The folder was
/// captured before the FileManager1 attempt and is revalidated before and after
/// the handler query, then passed to `open_target` for the final post-lock
/// revalidation.
fn reveal_directory_with(
    folder: &opener::LocalTarget,
    helpers: &Helpers,
    query: impl Fn(&Path, &[&str]) -> Result<String, String>,
    open: impl Fn(&Path, &opener::LocalTarget) -> Result<(), String>,
) -> Result<(), String> {
    folder.revalidate()?;
    let utility = helpers
        .xdg_mime
        .as_deref()
        .ok_or("Install xdg-utils (xdg-mime) to locate a desktop handler")?;
    query_handler(utility, "inode/directory", &query)?;
    folder.revalidate()?;
    let utility = helpers
        .xdg_open
        .as_deref()
        .ok_or("Install xdg-utils (xdg-open), or open the target manually")?;
    open(utility, folder)
}

fn file_uri(path: &Path) -> Result<String, String> {
    use std::fmt::Write as _;
    let path = path.to_str().ok_or("Reveal path is not UTF-8")?;
    let mut uri = String::from("file://");
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'.' | b'_' | b'~') {
            uri.push(char::from(byte));
        } else {
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    Ok(uri)
}

#[derive(Debug)]
struct Helpers {
    xdg_open: Option<PathBuf>,
    xdg_mime: Option<PathBuf>,
    gdbus: Option<PathBuf>,
}

impl Helpers {
    fn current() -> Result<Self, String> {
        if std::env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").is_file() {
            return Err("Host openers are unsupported inside this Flatpak build: no OpenURI portal client is configured. Copy the target and open it from the host desktop".to_owned());
        }
        if !matches!(
            HostBackend::current(),
            HostBackend::X11 | HostBackend::Wayland
        ) {
            return Err("No X11 or Wayland desktop session is available. Open the target manually from a desktop session".to_owned());
        }
        let environment = Environment::current();
        let find = |name| linux_catalog::find_program(name, &environment.search_path);
        Ok(Self {
            xdg_open: find("xdg-open"),
            xdg_mime: find("xdg-mime"),
            gdbus: find("gdbus"),
        })
    }

    fn handler(&self, mime: &str) -> Result<(), String> {
        let utility = self
            .xdg_mime
            .as_ref()
            .ok_or("Install xdg-utils (xdg-mime) to locate a desktop handler")?;
        query_handler(utility, mime, &query)
    }

    fn open(&self, target: &str) -> Result<(), String> {
        let utility = self
            .xdg_open
            .as_ref()
            .ok_or("Install xdg-utils (xdg-open), or open the target manually")?;
        open_target_path(utility, target)
    }
}

fn query_mime(
    utility: &Path,
    path: &str,
    query: &impl Fn(&Path, &[&str]) -> Result<String, String>,
) -> Result<String, String> {
    let mime = query(utility, &["query", "filetype", path])?;
    if !mime.contains('/')
        || !mime
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/.+-_".contains(&byte))
        || matches!(
            mime.as_str(),
            "application/x-desktop"
                | "application/x-executable"
                | "application/x-shellscript"
                | "application/x-ms-dos-executable"
                | "application/x-pie-executable"
        )
    {
        return Err(
            "File type is unsafe or could not be determined. Reveal it in the file manager instead"
                .to_owned(),
        );
    }
    Ok(mime)
}

fn query_handler(
    utility: &Path,
    mime: &str,
    query: &impl Fn(&Path, &[&str]) -> Result<String, String>,
) -> Result<(), String> {
    let output = query(utility, &["query", "default", mime])?;
    if output.ends_with(".desktop")
        && !output
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        Ok(())
    } else {
        Err(format!("No default desktop handler for {mime}. Set a default application in desktop settings and try again"))
    }
}

fn open_target(program: &Path, target: &opener::LocalTarget) -> Result<(), String> {
    let mut command = quiet(program);
    command.arg(target.handoff_str()?);
    // Revalidate after the shared child lock is acquired, immediately before
    // spawn. An in-process opener can hold that lock for its observation
    // window while the pathname is replaced.
    run_detachable_checked(
        &mut command,
        "xdg-open",
        Duration::from_millis(750),
        false,
        || target.revalidate(),
    )
}

/// URL handoff has no local pathname identity to revalidate.
fn open_target_path(program: &Path, target: &str) -> Result<(), String> {
    let mut command = quiet(program);
    command.arg(target);
    run_detachable(&mut command, "xdg-open", Duration::from_millis(750), false)
}

pub(super) fn opener_capabilities() -> OpenerCapabilities {
    use CapabilityStatus::{Available, Degraded, TemporarilyUnavailable, Unsupported};
    let backend = HostBackend::current();
    let Ok(helpers) = Helpers::current() else {
        // Either no X11/Wayland desktop session exists or this is a Flatpak
        // build with no portal client. Neither is a temporarily missing tool.
        return OpenerCapabilities {
            backend,
            browser: Unsupported,
            file_manager: Unsupported,
            default_application: Unsupported,
        };
    };
    let xdg = helpers.xdg_open.is_some() && helpers.xdg_mime.is_some();
    let browser = if xdg {
        match (
            helpers.handler("x-scheme-handler/http").is_ok(),
            helpers.handler("x-scheme-handler/https").is_ok(),
        ) {
            (true, true) => Available,
            (true, false) | (false, true) => Degraded {
                reason: "Only one web scheme has a default handler".to_owned(),
                alternative: "Set a default browser for both HTTP and HTTPS".to_owned(),
            },
            (false, false) => TemporarilyUnavailable,
        }
    } else {
        TemporarilyUnavailable
    };
    // An installed gdbus binary does not prove that FileManager1 is reachable.
    // Report only the independently probed directory-handler fallback here.
    let file_manager = if xdg && helpers.handler("inode/directory").is_ok() {
        Degraded {
            reason: "Item selection depends on FileManager1; a directory handler was found"
                .to_owned(),
            alternative: "Open the containing folder and select the item manually".to_owned(),
        }
    } else {
        TemporarilyUnavailable
    };
    let default_application = if xdg {
        Degraded {
            reason: "A default handler must be found for the specific document type".to_owned(),
            alternative: "Reveal the file or configure a handler in desktop settings".to_owned(),
        }
    } else {
        TemporarilyUnavailable
    };
    OpenerCapabilities {
        backend,
        browser,
        file_manager,
        default_application,
    }
}

pub(super) fn host_capabilities() -> HostCapabilities {
    let backend = HostBackend::current();
    let roots = Environment::current().roots;
    let application_catalog = if roots.iter().any(|root| fs::read_dir(root).is_ok()) {
        CapabilityStatus::Available
    } else if roots.iter().any(|root| {
        fs::read_dir(root).is_err_and(|err| err.kind() == io::ErrorKind::PermissionDenied)
    }) {
        CapabilityStatus::PermissionRequired
    } else {
        CapabilityStatus::TemporarilyUnavailable
    };
    let application_launch = if matches!(backend, HostBackend::X11 | HostBackend::Wayland) {
        CapabilityStatus::Degraded {
            reason: "Exec launch is supported; no verified application switching is implemented on Linux".to_owned(),
            alternative: "Use the desktop task switcher to focus running applications".to_owned(),
        }
    } else {
        CapabilityStatus::TemporarilyUnavailable
    };
    let window_control = match backend {
        HostBackend::X11 => CapabilityStatus::Degraded {
            reason: "Only an advisory EWMH/RandR work-area hint is implemented; there is no window enumeration, activation or positioning route".to_owned(),
            alternative: "Use the desktop task switcher and window manager".to_owned(),
        },
        _ => CapabilityStatus::Unsupported,
    };
    HostCapabilities {
        platform: Platform::Linux,
        adapter: "linux-xdg",
        application_catalog,
        application_launch,
        url_open: opener_capabilities().browser,
        // A compiled X11 grab route is not a reservation. This stays degraded
        // until registration succeeds; Wayland has no portal client here.
        global_hotkey: crate::hotkey::capability(backend),
        packaged_applications: CapabilityStatus::Unsupported,
        clipboard_capture: CapabilityStatus::Unsupported,
        window_control,
        notes: vec![
            "Catalog includes desktop entries in the declared XDG roots, including declared exports.",
            "X11 and Wayland use literal Exec argv; this adapter does not switch existing windows.",
            "X11 global shortcuts use checked passive grabs; conflicts are reported at registration.",
            "Wayland global shortcuts require a GlobalShortcuts portal client; none is implemented.",
            "Clipboard capture is owned by a separate adapter and is not probed by this platform capability.",
            "Flatpak portal routing is not implemented.",
        ],
    }
}

fn quiet(program: &Path) -> Command {
    let mut child = Command::new(program);
    child
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    child
}

fn query(program: &Path, args: &[&str]) -> Result<String, String> {
    // A file avoids a pipe that could block on a descendant that inherited its
    // write end. Only a bounded result is brought into memory.
    let mut output = tempfile::tempfile()
        .map_err(|error| format!("Cannot capture desktop handler query: {error}"))?;
    let writer = output
        .try_clone()
        .map_err(|error| format!("Cannot capture desktop handler query: {error}"))?;
    let mut command = quiet(program);
    command.args(args).stdout(writer);
    run_bounded(
        &mut command,
        "Desktop handler query",
        Duration::from_millis(500),
    )?;
    output
        .seek(SeekFrom::Start(0))
        .map_err(|error| format!("Cannot read desktop handler query: {error}"))?;
    let mut contents = String::new();
    output
        .take(4097)
        .read_to_string(&mut contents)
        .map_err(|error| format!("Cannot read desktop handler query: {error}"))?;
    if contents.len() > 4096 {
        return Err("Desktop handler returned an oversized response".to_owned());
    }
    Ok(contents.trim().to_owned())
}

fn run_bounded(command: &mut Command, label: &str, timeout: Duration) -> Result<(), String> {
    let mut child = command
        .spawn()
        .map_err(|error| format!("{label} could not start: {error}"))?;
    match observe_exit(&mut child, timeout) {
        Ok(Some(status)) => exit_result(status, label),
        result => {
            let _ = child.kill();
            let _ = child.wait();
            Err(match result {
                Ok(None) => {
                    format!("{label} timed out. Check the default desktop handler and try again")
                }
                Err(error) => format!("{label} could not be verified: {error}"),
                Ok(Some(_)) => unreachable!(),
            })
        }
    }
}

fn exit_result(status: ExitStatus, label: &str) -> Result<(), String> {
    if status.success() {
        Ok(())
    } else {
        Err(format!("{label} failed ({status}). Check the installed application or desktop handler and try again"))
    }
}

fn observe_exit(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

type ChildQueue = Arc<(Mutex<Vec<Child>>, Condvar)>;
static CHILD_QUEUE: OnceLock<Result<ChildQueue, String>> = OnceLock::new();

fn child_queue() -> Result<&'static ChildQueue, String> {
    CHILD_QUEUE
        .get_or_init(|| {
            let queue = Arc::new((Mutex::new(Vec::<Child>::new()), Condvar::new()));
            let worker = queue.clone();
            thread::Builder::new()
                .name("ansa-xdg-child-reaper".to_owned())
                .spawn(move || {
                    let (children, changed) = &*worker;
                    loop {
                        let mut tracked =
                            children.lock().unwrap_or_else(|error| error.into_inner());
                        tracked = changed
                            .wait_while(tracked, |list| list.is_empty())
                            .unwrap_or_else(|error| error.into_inner());
                        tracked.retain_mut(|child| matches!(child.try_wait(), Ok(None)));
                        drop(tracked);
                        thread::sleep(Duration::from_millis(200));
                    }
                })
                .map_err(|error| format!("Cannot start child process supervisor: {error}"))?;
            Ok(queue)
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn run_detachable(
    command: &mut Command,
    label: &str,
    timeout: Duration,
    allow_running: bool,
) -> Result<(), String> {
    run_detachable_checked(command, label, timeout, allow_running, || Ok(()))
}

/// The validation closure runs only after the shared child lock is acquired
/// and immediately before spawn, so an in-process opener wait cannot hide a
/// replaced pathname.
fn run_detachable_checked(
    command: &mut Command,
    label: &str,
    timeout: Duration,
    allow_running: bool,
    validate: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let (children, changed) = &**child_queue()?;
    let mut tracked = children.lock().unwrap_or_else(|error| error.into_inner());
    // The lock reserves one slot during startup, so concurrent callers cannot
    // exceed the child cap. A bounded process-creation observation follows.
    if tracked.len() >= 256 {
        return Err("Too many applications or opener requests are still running".to_owned());
    }
    validate()?;
    let mut child = command
        .spawn()
        .map_err(|error| format!("{label} could not start: {error}"))?;
    match observe_exit(&mut child, timeout) {
        Ok(Some(status)) => exit_result(status, label),
        Ok(None) => {
            tracked.push(child);
            changed.notify_one();
            if allow_running {
                Ok(())
            } else {
                Err(format!("{label} has not confirmed handoff and is still running. The target may already be open; check before retrying"))
            }
        }
        Err(error) => {
            tracked.push(child);
            changed.notify_one();
            Err(format!("{label} could not be verified: {error}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn helpers() -> Helpers {
        Helpers {
            xdg_open: Some(PathBuf::from("/inert/xdg-open")),
            xdg_mime: Some(PathBuf::from("/inert/xdg-mime")),
            gdbus: None,
        }
    }

    #[test]
    fn document_replaced_during_handler_query_never_reaches_the_opener() {
        let tmp = tempfile::tempdir().unwrap();
        let approved = tmp.path().join("approved.txt");
        let disallowed = tmp.path().join("no-extension");
        fs::write(&approved, b"approved").unwrap();
        fs::write(&disallowed, b"not an allowlisted document").unwrap();
        let target = opener::LocalTarget::capture(
            approved.to_str().unwrap(),
            Opener::DefaultApp,
            OpenPolicy::with_local_paths(),
        )
        .unwrap();
        let queries = Cell::new(0);
        let opened = Cell::new(false);
        let result = open_document_with(
            &target,
            &helpers(),
            |_, args| {
                queries.set(queries.get() + 1);
                match args {
                    ["query", "filetype", _] => Ok("text/plain".to_owned()),
                    ["query", "default", "text/plain"] => {
                        // The approved document is replaced by a symlink to an
                        // object the document policy rejects while the default
                        // handler is being queried.
                        fs::remove_file(&approved).unwrap();
                        std::os::unix::fs::symlink(&disallowed, &approved).unwrap();
                        Ok("text.desktop".to_owned())
                    }
                    _ => panic!("unexpected inert helper query: {args:?}"),
                }
            },
            |_, _| {
                opened.set(true);
                Ok(())
            },
        );
        // The replacement can fail either the document policy or the identity
        // comparison; only rejection and no handoff are required.
        assert!(result.is_err());
        assert_eq!(queries.get(), 2);
        assert!(!opened.get(), "xdg-open must not be invoked");
    }

    #[test]
    fn directory_replaced_during_handler_query_never_reaches_the_opener() {
        let tmp = tempfile::tempdir().unwrap();
        let folder_path = tmp.path().join("folder");
        let replacement = tmp.path().join("replacement");
        fs::create_dir(&folder_path).unwrap();
        fs::create_dir(&replacement).unwrap();
        let document = folder_path.join("document.txt");
        fs::write(&document, b"approved").unwrap();
        let target = opener::LocalTarget::capture(
            document.to_str().unwrap(),
            Opener::FileManager,
            OpenPolicy::with_local_paths(),
        )
        .unwrap();
        let folder = containing_folder(&target).unwrap();
        let opened = Cell::new(false);
        let result = reveal_directory_with(
            &folder,
            &helpers(),
            |_, args| {
                assert_eq!(args, &["query", "default", "inode/directory"]);
                fs::rename(&folder_path, tmp.path().join("folder-moved")).unwrap();
                std::os::unix::fs::symlink(&replacement, &folder_path).unwrap();
                Ok("inode-directory.desktop".to_owned())
            },
            |_, _| {
                opened.set(true);
                Ok(())
            },
        );
        assert!(result.unwrap_err().contains("changed after policy"));
        assert!(!opened.get(), "xdg-open must not be invoked");
    }

    #[test]
    fn fallback_does_not_adopt_a_folder_retargeted_during_show_items() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("folder");
        let replacement = tmp.path().join("replacement");
        fs::create_dir(&folder).unwrap();
        fs::create_dir(&replacement).unwrap();
        let document = folder.join("document.txt");
        fs::write(&document, b"approved").unwrap();
        let target = opener::LocalTarget::capture(
            document.to_str().unwrap(),
            Opener::FileManager,
            OpenPolicy::with_local_paths(),
        )
        .unwrap();
        // The fallback folder is captured before the FileManager1 attempt.
        let folder = containing_folder(&target).unwrap();
        let folder_path = folder.handoff().to_path_buf();
        let mut helpers = helpers();
        helpers.gdbus = Some(PathBuf::from("/inert/gdbus"));
        let queries = Cell::new(0);
        let opened = Cell::new(false);
        let result = reveal_with_helpers(
            &target,
            &folder,
            &helpers,
            |_, _| {
                // Parent is retargeted while ShowItems runs, then ShowItems
                // reports failure. The fallback must not take a fresh stamp.
                fs::rename(&folder_path, tmp.path().join("folder-moved")).unwrap();
                std::os::unix::fs::symlink(&replacement, &folder_path).unwrap();
                false
            },
            |_, _| {
                queries.set(queries.get() + 1);
                Ok("inode-directory.desktop".to_owned())
            },
            |_, _| {
                opened.set(true);
                Ok(())
            },
        );
        assert!(result.unwrap_err().contains("changed after policy"));
        assert_eq!(queries.get(), 0, "handler query must not run");
        assert!(!opened.get(), "xdg-open must not be invoked");
    }

    #[test]
    fn pre_spawn_validation_runs_while_the_child_lock_is_held() {
        let (queue, _) = &**child_queue().unwrap();
        let checked = Cell::new(false);
        let mut command = Command::new("/inert/never-spawned");
        let result = run_detachable_checked(
            &mut command,
            "test opener",
            Duration::from_millis(10),
            true,
            || {
                assert!(
                    queue.try_lock().is_err(),
                    "validation must run under the child lock"
                );
                checked.set(true);
                Err("changed after policy".to_owned())
            },
        );
        assert!(checked.get());
        assert!(result.unwrap_err().contains("changed after policy"));
    }
}
