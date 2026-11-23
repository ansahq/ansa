//! Native LaunchServices/Finder handoff through typed AppKit bindings.

use crate::catalog::{self, Snapshot};
use crate::host_lifecycle::HostBackend;
use crate::mac_catalog::{self, Bundle};
use crate::{
    opener, AppEntry, CapabilityStatus, HostCapabilities, LaunchOutcome, OpenerCapabilities,
    Platform,
};
use objc2::rc::{autoreleasepool, Retained};
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplicationActivationOptions, NSRunningApplication, NSWorkspace, NSWorkspaceLaunchOptions,
};
use objc2_foundation::{
    NSArray, NSDictionary, NSNumber, NSString, NSURLBookmarkResolutionOptions, NSURLIsAliasFileKey,
    NSURLIsPackageKey, NSURL,
};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

static CATALOG: Snapshot<Bundle> = Snapshot::new();

pub(super) fn catalog_watch_roots() -> Result<Vec<PathBuf>, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    Ok(home
        .into_iter()
        .map(|path| path.join("Applications"))
        .chain(
            [
                "/Applications",
                "/System/Applications",
                "/System/Library/CoreServices/Applications",
            ]
            .into_iter()
            .map(PathBuf::from),
        )
        .collect())
}

pub(super) fn discover_applications() -> Result<Vec<AppEntry>, String> {
    let snapshot = mac_catalog::discover(&catalog_watch_roots()?)?;
    let mut entries: Vec<_> = snapshot
        .values()
        .map(|bundle| bundle.entry.clone())
        .collect();
    entries.sort_by(|a, b| (&a.title, &a.id).cmp(&(&b.title, &b.id)));
    CATALOG.replace(snapshot);
    Ok(entries)
}

pub(super) fn resolve_bundle_alias(path: &Path) -> Option<PathBuf> {
    autoreleasepool(|_| {
        let alias = NSURL::fileURLWithPath(&NSString::from_str(path.to_str()?));
        let target = NSURL::URLByResolvingAliasFileAtURL_options_error(
            &alias,
            NSURLBookmarkResolutionOptions::WithoutUI
                | NSURLBookmarkResolutionOptions::WithoutMounting,
        )
        .ok()?;
        if !target.isFileURL() {
            return None;
        }
        let canonical = fs::canonicalize(target.path()?.to_string()).ok()?;
        (canonical.is_dir()
            && canonical
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("app")))
        .then_some(canonical)
    })
}

pub(super) fn launch_or_switch(path: &str, expected_id: Option<&str>) -> LaunchOutcome {
    let result = CATALOG
        .get(path, || discover_applications().map(|_| ()))
        .and_then(|selected| {
            if expected_id.is_some_and(|id| id != selected.entry.id) {
                return Err(catalog::stale_target());
            }
            mac_catalog::recheck(&selected)
        });
    finish_launch(result)
}

pub(super) fn launch_or_switch_selected(entry: &AppEntry) -> LaunchOutcome {
    let result = CATALOG
        .get_selected(
            entry,
            |selected| &selected.entry,
            || discover_applications().map(|_| ()),
        )
        .and_then(|selected| mac_catalog::recheck(&selected));
    finish_launch(result)
}

fn finish_launch(result: Result<Bundle, String>) -> LaunchOutcome {
    let (canonical, identifier) = match result {
        Ok(selected) => match (selected.canonical, selected.bundle_id) {
            (Some(path), Some(id)) => (path, id),
            _ => {
                return LaunchOutcome::NoTarget {
                    reason: catalog::stale_target(),
                }
            }
        },
        Err(reason) => return LaunchOutcome::NoTarget { reason },
    };
    autoreleasepool(|_| launch_exact(&canonical, &identifier))
}

fn running_at(app: &NSRunningApplication, path: &Path) -> bool {
    app.bundleURL()
        .and_then(|url| url.path())
        .and_then(|path| fs::canonicalize(path.to_string()).ok())
        .is_some_and(|actual| actual == path)
}

fn same_id(app: &NSRunningApplication, identifier: &str) -> bool {
    app.bundleIdentifier()
        .is_some_and(|id| id.to_string().eq_ignore_ascii_case(identifier))
}

#[allow(deprecated)]
fn launch_exact(path: &Path, identifier: &str) -> LaunchOutcome {
    let Some(raw_path) = path.to_str() else {
        return LaunchOutcome::NoTarget {
            reason: "Application bundle path is not UTF-8. Refresh applications".to_owned(),
        };
    };
    let workspace = NSWorkspace::sharedWorkspace();
    let running = NSRunningApplication::runningApplicationsWithBundleIdentifier(
        &NSString::from_str(identifier),
    );
    if let Some(app) = (0..running.count().min(256))
        .map(|index| running.objectAtIndex(index))
        .filter(|app| running_at(app, path))
        .min_by_key(|app| (!app.isActive(), app.processIdentifier()))
    {
        let pid = app.processIdentifier();
        let foreground = || {
            workspace.frontmostApplication().is_some_and(|front| {
                front.processIdentifier() == pid
                    && same_id(&front, identifier)
                    && running_at(&front, path)
            })
        };
        if pid > 0 && foreground() {
            return LaunchOutcome::SwitchedApplication { process_id: pid };
        }
        if !app.activateWithOptions(NSApplicationActivationOptions::empty()) {
            return LaunchOutcome::Denied { reason: "macOS declined activation of the selected running application; use the Dock or Command-Tab".to_owned() };
        }
        let can_wait = MainThreadMarker::new().is_none();
        for observation in 0..=if can_wait { 6 } else { 0 } {
            if observation > 0 {
                std::thread::sleep(Duration::from_millis(15));
            }
            if pid > 0 && foreground() {
                return LaunchOutcome::SwitchedApplication { process_id: pid };
            }
        }
        return LaunchOutcome::Unverified {
            reason: "macOS accepted activation but the selected application was not observed in front. Use the Dock or Command-Tab".to_owned(),
        };
    }
    let url = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(raw_path), true);
    // Use an exact bundle URL plus NewInstance: a running duplicate at a
    // different path must not be activated as if it were the selected copy.
    // An empty, correctly typed configuration imposes no argv or environment.
    // SAFETY: The dictionary cannot contain incorrectly typed option values.
    let result = unsafe {
        workspace.launchApplicationAtURL_options_configuration_error(
            &url,
            NSWorkspaceLaunchOptions::NewInstance,
            &NSDictionary::new(),
        )
    };
    match result {
        Ok(app) if same_id(&app, identifier) && running_at(&app, path) => LaunchOutcome::Launched,
        Ok(_) => LaunchOutcome::Unverified {
            reason: "LaunchServices did not confirm the selected bundle path. Refresh applications and check the installed copy".to_owned(),
        },
        Err(error) => LaunchOutcome::Failed {
            reason: format!("LaunchServices could not launch the selected application: {}. Check installation or Gatekeeper, then refresh applications", error.localizedDescription()),
        },
    }
}

fn web_nsurl(target: &str) -> Result<Retained<NSURL>, String> {
    let url = NSURL::URLWithString(&NSString::from_str(target))
        .ok_or("macOS could not parse the validated web URL")?;
    let scheme = url
        .scheme()
        .ok_or("macOS did not recognize the URL scheme")?;
    if !ansa_quicklinks::is_web_scheme(&scheme.to_string()) {
        return Err("macOS did not recognize an HTTP(S) URL".to_owned());
    }
    Ok(url)
}

fn handler_exists(workspace: &NSWorkspace, url: &NSURL) -> bool {
    workspace
        .URLForApplicationToOpenURL(url)
        .is_some_and(|app| {
            app.path()
                .is_some_and(|path| Path::new(&path.to_string()).is_dir())
        })
}

fn finder_exists(workspace: &NSWorkspace) -> bool {
    workspace
        .URLForApplicationWithBundleIdentifier(&NSString::from_str("com.apple.finder"))
        .and_then(|url| url.path())
        .is_some_and(|path| Path::new(&path.to_string()).is_dir())
}

pub(super) fn open_url(url: &str) -> Result<(), String> {
    opener::web_url(url)?;
    autoreleasepool(|_| {
        let workspace = NSWorkspace::sharedWorkspace();
        let url = web_nsurl(url)?;
        if !handler_exists(&workspace, &url) {
            return Err("No default web browser is available for this URL. Set one in System Settings and try again".to_owned());
        }
        if workspace.openURL(&url) {
            Ok(())
        } else {
            Err("NSWorkspace could not open the browser. Check your default browser in System Settings".to_owned())
        }
    })
}

pub(super) fn open_document(target: &opener::LocalTarget) -> Result<(), String> {
    autoreleasepool(|_| {
        let workspace = NSWorkspace::sharedWorkspace();
        let path = target.handoff_str()?;
        let url = NSURL::fileURLWithPath(&NSString::from_str(path));
        // SAFETY: Foundation exports both immutable NSString resource-key
        // constants for the lifetime of the process.
        let keys = unsafe { [NSURLIsAliasFileKey, NSURLIsPackageKey] };
        let flags = url
            .resourceValuesForKeys_error(&NSArray::from_slice(&keys))
            .map_err(|error| {
                format!(
                    "Cannot verify that the local target is not an alias or package: {}",
                    error.localizedDescription()
                )
            })?;
        for key in keys {
            if !flags
                .objectForKey(key)
                .and_then(|value| value.downcast::<NSNumber>().ok())
                .is_some_and(|flag| !flag.boolValue())
            {
                return Err("Finder aliases, application packages, or unverified file types cannot be opened as documents; reveal the path instead".to_owned());
            }
        }
        if !handler_exists(&workspace, &url) {
            return Err("No default application can open this file. Set a default handler or reveal it in Finder".to_owned());
        }
        if workspace.openURL(&url) {
            Ok(())
        } else {
            Err("NSWorkspace could not open this document. Check its permissions and default application".to_owned())
        }
    })
}

pub(super) fn reveal_in_file_manager(path: &str) -> Result<(), String> {
    let path = opener::reveal_path(path)?;
    autoreleasepool(|_| {
        let workspace = NSWorkspace::sharedWorkspace();
        if !finder_exists(&workspace) {
            return Err("Finder is not registered with LaunchServices. Relaunch Finder or copy the path and open it manually".to_owned());
        }
        let parent = path.parent().unwrap_or(path.as_path());
        let path = NSString::from_str(path.to_str().ok_or("Reveal path is not UTF-8")?);
        let root = NSString::from_str(parent.to_str().ok_or("Containing folder is not UTF-8")?);
        if workspace.selectFile_inFileViewerRootedAtPath(Some(&path), &root) {
            Ok(())
        } else {
            Err("Finder declined to reveal the path. Check its permissions or open the containing folder manually".to_owned())
        }
    })
}

pub(super) fn opener_capabilities() -> OpenerCapabilities {
    use CapabilityStatus::{Available, Degraded, TemporarilyUnavailable};
    autoreleasepool(|_| {
        let workspace = NSWorkspace::sharedWorkspace();
        let supports = |target| web_nsurl(target).is_ok_and(|url| handler_exists(&workspace, &url));
        let browser = match (
            supports("http://example.invalid/"),
            supports("https://example.invalid/"),
        ) {
            (true, true) => Available,
            (true, false) | (false, true) => Degraded {
                reason: "Only one web scheme has a default handler".to_owned(),
                alternative: "Set a default web browser in System Settings".to_owned(),
            },
            (false, false) => TemporarilyUnavailable,
        };
        let file_manager = if finder_exists(&workspace) {
            Available
        } else {
            TemporarilyUnavailable
        };
        OpenerCapabilities {
            backend: HostBackend::MacOs,
            browser,
            file_manager,
            default_application: Degraded {
                reason: "LaunchServices resolves a default handler for each particular document"
                    .to_owned(),
                alternative:
                    "Reveal it in Finder or choose a default application for its file type"
                        .to_owned(),
            },
        }
    })
}

pub(super) fn host_capabilities() -> HostCapabilities {
    let roots = catalog_watch_roots().unwrap_or_default();
    let application_catalog = if roots.iter().any(|root| fs::read_dir(root).is_ok()) {
        CapabilityStatus::Available
    } else if roots.iter().any(|root| {
        fs::read_dir(root).is_err_and(|err| err.kind() == io::ErrorKind::PermissionDenied)
    }) {
        CapabilityStatus::PermissionRequired
    } else {
        CapabilityStatus::TemporarilyUnavailable
    };
    HostCapabilities {
        platform: Platform::MacOs,
        adapter: "macos-launchservices",
        application_catalog,
        application_launch: CapabilityStatus::Available,
        url_open: opener_capabilities().browser,
        // HIToolbox registration is implemented but is not a reservation until
        // a chord is acquired; conflicts and Secure Event Input are observed
        // later. Do not report this route as unavailable or unconditionally
        // registered.
        global_hotkey: crate::hotkey::capability(HostBackend::MacOs),
        packaged_applications: CapabilityStatus::Unsupported,
        clipboard_capture: CapabilityStatus::Unsupported,
        window_control: CapabilityStatus::Degraded {
            reason: "Foreground application activation can be observed; individual windows are not enumerated or controlled".to_owned(),
            alternative: "Use Mission Control or the Dock for individual windows".to_owned(),
        },
        notes: vec![
            "LaunchServices receives the selected bundle URL; activation is verified only at the application level.",
            "Gatekeeper policy is enforced by macOS at launch.",
            "HIToolbox registration is implemented on the AppKit thread; conflicts and Secure Event Input are reported at registration or status time.",
            "Clipboard capture is owned by a separate adapter and is not probed by this platform capability.",
        ],
    }
}
