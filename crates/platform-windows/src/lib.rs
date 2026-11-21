//! Windows host adapter for Ansa.
//!
//! Scope of this slice:
//! * Discover `.lnk` and `.exe` entries in the per-user and all-users Start
//!   Menu `Programs` folders with bounded, non-recursive-looping traversal.
//! * Decode `.lnk` targets with a bounded Shell Link parser, expose working
//!   directory/arguments/icon-source metadata, and keep broken shortcuts
//!   visible instead of silently dropping them.
//! * Enumerate packaged/MSIX applications through a fixed `Get-StartApps`
//!   command, validate AUMIDs strictly and launch them through
//!   `shell:AppsFolder\<AUMID>`.
//! * Deduplicate entries that resolve to the same local target with stable,
//!   target-derived identity.
//! * Launch a validated `.lnk`/`.exe` path with `ShellExecuteW`, or switch to a
//!   running window only when foreground ownership is observed.
//! * Open absolute `http`/`https` URLs only with `ShellExecuteW`.
//! * Watch Start Menu roots with bounded polling and debounced change
//!   detection, and expose the real roots for an owned polling example.
//! * Extract a bounded icon image into a deterministic ICO cache file with
//!   checked Win32 icon APIs.
//! * Reveal an existing path with shell item APIs and copy a structured
//!   identity, without building an `explorer.exe` command line.
//! * Register the default `Ctrl+Space` global hotkey on a dedicated,
//!   event-driven message-loop thread with deterministic cleanup, and offer a
//!   typed shortcut/recorder/manager lifecycle for rebinding.
//! * Show/hide the launcher's own HWND on the UI thread, including an explicit
//!   check for denied foreground activation.
//! * Capture the foreground top-level window before showing the launcher and
//!   restore it on dismissal only while the launcher still owns the foreground
//!   (`capture_foreground_target`, `launcher_is_foreground`,
//!   `dismiss_launcher`).
//! * Report what is and is not supported honestly.
//!
//! Not implemented or claimed here: clipboard monitoring, general window
//! activation/positioning, packaged-application window switching and extension
//! isolation. Icon extraction returns one bounded image and falls back to an
//! explicit error, never a fabricated icon. Focus restoration is limited to
//! returning to the window captured before the launcher was shown; it is
//! checked, best-effort and never injects input.

#![deny(missing_debug_implementations)]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(not(windows), forbid(unsafe_code))]

#[cfg(windows)]
mod backdrop;
#[cfg(not(windows))]
use std::path::Path;

mod focus;
#[cfg(windows)]
mod hotkey;
#[cfg(windows)]
mod icons;
#[cfg(windows)]
mod launch;
#[cfg(windows)]
mod packaged;
#[cfg(windows)]
mod reveal;
#[cfg(windows)]
mod shell_link;
mod shortcut;
#[cfg(windows)]
mod start_menu;
#[cfg(windows)]
mod switch;
mod types;
mod url;
#[cfg(windows)]
mod visibility;
#[cfg(windows)]
mod watch;
#[cfg(windows)]
mod win32;

#[cfg(windows)]
pub use backdrop::LauncherBackdrop;
pub use focus::{
    capture_foreground_target, dismiss_launcher, launcher_is_foreground, FocusRestoreOutcome,
    FocusTarget,
};
#[cfg(windows)]
pub use hotkey::{HotkeyError, HotkeyManager, HotkeyRegistration};
#[cfg(windows)]
pub use icons::{
    extract_icon_to_ico, validate_ico, MAX_ICON_DIMENSION, MAX_ICON_INDEX, MAX_ICO_BYTES,
};
#[cfg(windows)]
pub use reveal::reveal_in_file_manager;
pub use shortcut::{
    RecorderEvent, ShortcutChord, ShortcutError, ShortcutKey, ShortcutRecorder,
    MAX_SHORTCUT_TEXT_BYTES,
};
pub use types::{
    AppEntry, CapabilityStatus, HostCapabilities, IconCacheEntry, LaunchOutcome, Platform,
};
#[cfg(windows)]
pub use visibility::{make_launcher_frameless, round_launcher_window, set_launcher_visible};
#[cfg(windows)]
pub use watch::{
    default_watch_roots, CatalogDelta, CatalogPoll, CatalogWatcher, DEFAULT_WATCH_INTERVAL,
};

/// Returns a deterministic, multi-line structured identity for an entry.
///
/// The format is stable for CLI output, diagnostics and clipboard copy. It
/// never constructs a shell command and is available on every platform.
pub fn copy_identity(entry: &AppEntry) -> String {
    entry.identity_summary()
}

/// Non-Windows placeholder for the Windows-only hotkey worker.
///
/// The type exists so callers can compile platform-independent glue, but
/// [`HotkeyRegistration::register`] always returns an explicit unsupported
/// error on this platform.
#[cfg(not(windows))]
#[derive(Debug)]
pub struct HotkeyRegistration {
    _private: (),
}

#[cfg(not(windows))]
impl HotkeyRegistration {
    /// Reports that global hotkeys are unsupported on this platform.
    pub fn register(_callback: impl Fn() + Send + 'static) -> Result<Self, String> {
        Err(unsupported("registering a global hotkey"))
    }
}

/// Reports that native launcher visibility control is unsupported on this
/// platform.
#[cfg(not(windows))]
pub fn set_launcher_visible(_hwnd: isize, _visible: bool) -> Result<(), String> {
    Err(unsupported("changing launcher visibility"))
}

/// Reports that icon pixel extraction is unsupported on this platform.
#[cfg(not(windows))]
pub fn extract_icon_to_ico(
    _icon_source: &str,
    _cache_dir: &Path,
) -> Result<IconCacheEntry, String> {
    Err(unsupported("icon extraction"))
}

/// Reports that ICO validation is unsupported on this platform.
#[cfg(not(windows))]
pub fn validate_ico(_bytes: &[u8]) -> Result<(), String> {
    Err(unsupported("icon validation"))
}

/// Reports that revealing a path in the file manager is unsupported on this
/// platform.
#[cfg(not(windows))]
pub fn reveal_in_file_manager(_path: &str) -> Result<(), String> {
    Err(unsupported("revealing a path in the file manager"))
}

/// Validates that a URL can be handed to [`open_url`].
///
/// Only absolute `http` and `https` URLs with a non-empty host are accepted.
/// This can be used by the UI to reject input before invoking the host.
pub fn validate_url(url: &str) -> Result<(), String> {
    url::validate_http_url(url)
}

/// Discovers launchable applications.
///
/// The catalog combines a bounded Start Menu scan with bounded packaged/MSIX
/// enumeration. Discovery does not follow symbolic links, junctions or other
/// reparse points. Shortcut targets are resolved for identity and metadata; a
/// shortcut whose target is missing or whose file is malformed remains in the
/// catalog with `AppEntry::broken` set. Entries that resolve to the same local
/// target are deduplicated.
///
/// Packaged enumeration is best-effort: if `Get-StartApps` is unavailable the
/// classic catalog is still returned and [`host_capabilities`] reports the
/// packaged capability as degraded.
pub fn discover_applications() -> Result<Vec<AppEntry>, String> {
    #[cfg(windows)]
    {
        let mut entries = start_menu::discover()?;
        if let Ok(mut packaged) = packaged::enumerate() {
            entries.append(&mut packaged);
            entries.sort_by_cached_key(|entry| {
                (
                    entry.title.to_lowercase(),
                    entry.path.to_lowercase(),
                    entry.id.clone(),
                )
            });
        }
        Ok(entries)
    }
    #[cfg(not(windows))]
    {
        Err(unsupported("application discovery"))
    }
}

/// Opens an existing `.lnk`, `.exe` or validated packaged application entry.
///
/// The entry and any resolved shortcut target are re-checked immediately before
/// the call. A missing or moved target returns an actionable error that mentions
/// refreshing the catalog, and `ShellExecuteW` is not called with the stale
/// path. Paths are passed as a single argument; they are never interpolated into
/// a command line.
pub fn open_application(path: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        launch::open_application(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(unsupported("opening an application"))
    }
}

/// Launches an application or switches to an existing verified window.
///
/// Unlike [`open_application`], a resolved executable is first matched against
/// top-level windows by process image path. A switch is reported only when the
/// target window is observed as foreground; otherwise the outcome is
/// [`LaunchOutcome::Denied`] or [`LaunchOutcome::Unverified`]. If no eligible
/// window exists, a new instance is launched through the shell.
///
/// Missing, moved or unreadable targets return [`LaunchOutcome::NoTarget`] with
/// a refresh hint instead of a silent no-op.
pub fn launch_or_switch(path: &str) -> LaunchOutcome {
    #[cfg(windows)]
    {
        launch::launch_or_switch(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        LaunchOutcome::NoTarget {
            reason: format!(
                "launching or switching applications is unsupported on {}",
                Platform::current()
            ),
        }
    }
}

/// Opens an absolute `http`/`https` URL with the registered handler.
///
/// Other schemes, relative URLs and URLs containing whitespace or control
/// characters are rejected before reaching the shell.
pub fn open_url(url: &str) -> Result<(), String> {
    validate_url(url)?;

    #[cfg(windows)]
    {
        win32::open_url(url)
    }
    #[cfg(not(windows))]
    {
        let _ = url;
        Err(unsupported("opening a URL"))
    }
}

/// Probes the host's current capabilities.
///
/// Catalog and launch checks are fresh; the packaged-enumeration probe is
/// cached for a short interval so repeated capability checks cannot block the
/// UI on every call.
pub fn host_capabilities() -> HostCapabilities {
    #[cfg(windows)]
    {
        HostCapabilities {
            platform: Platform::current(),
            adapter: "windows-start-menu-packaged-launch-switch-v5",
            application_catalog: start_menu::probe_catalog_status(),
            application_launch: CapabilityStatus::Available,
            url_open: CapabilityStatus::Available,
            global_hotkey: CapabilityStatus::Available,
            packaged_applications: packaged::probe_status(),
            clipboard_capture: CapabilityStatus::Unsupported,
            window_control: CapabilityStatus::Unsupported,
            notes: vec![
                "Application discovery scans the per-user and all-users Start Menu Programs folders for .lnk and .exe entries and adds packaged applications when Get-StartApps can be validated.",
                "Shortcut targets are decoded with a bounded Shell Link parser. Working directory, arguments and icon location are exposed as metadata; the launcher does not re-parse or interpolate arguments into a command line.",
                "Relative shortcut targets are resolved in trust order: an absolute relative path, the decoded LinkTargetIDList target, a matching executable icon location, the shortcut working directory, then the .lnk parent directory; unresolved candidates are reported in the broken-target reason.",
                "Shortcuts that resolve to the same executable with different arguments are distinct commands. Identity includes the normalized argument string, and direct .exe entries use target-derived identity so adding or removing an alias does not change them.",
                "Icon extraction is bounded: ExtractIconExW/GetIconInfo/GetDIBits produce one 32-bit image capped at 256x256, written to a deterministic ICO cache file capped at 512 KiB and cleaned up on failure. Source paths must be absolute regular files; reparse points are refused and extraction failure is reported as an error, never a fabricated icon.",
                "Reveal-in-file-manager uses SHParseDisplayName and SHOpenFolderAndSelectItems with a validated absolute path and never builds an explorer.exe command line. Structured copy-identity is available through copy_identity/identity_summary. Reparse points are refused.",
                "Entries that resolve to the same local target, arguments and working directory are deduplicated with target-derived identity; shortcuts without a resolved local target keep path-derived identity. Ties are broken by title then path.",
                "Broken shortcuts stay in the catalog with an explicit reason. Launch re-checks the shortcut and target existence and returns an error mentioning refresh instead of calling ShellExecuteW with a missing target.",
                "Packaged applications are enumerated with a fixed Get-StartApps command (no metadata interpolation), strict JSON parsing and an AUMID character allowlist. PowerShell is invoked by absolute System32/SysWOW64 path, never through PATH. They are launched only through shell:AppsFolder\\<AUMID>.",
                "Packaged application switching is Unsupported: this adapter can launch a validated AUMID but cannot map an AUMID to a specific top-level window without Windows package-identity APIs that are not bound here. Alternative: use the packaged application's own Start Menu or taskbar shortcut, or launch_or_switch for classic .exe entries; no packaged switch is claimed.",
                "Launch outcomes remain Launched, Switched, Denied, Failed, Unverified or NoTarget. Failed is reserved for non-target shell failures; an unverified switch is never reported as success.",
                "Minimized top-level windows are eligible for switching and are restored before activation; hidden windows are not candidates.",
                "CatalogWatcher polls Start Menu roots with a bounded interval and requires the same change on two consecutive polls before reporting that a rescan is due; it runs no background thread. The desktop and CLI do not poll it yet; default_watch_roots and the owned watch example demonstrate bounded polling, and desktop wiring is tracked as L11. Packaged installs are not represented by Start Menu files, so those installs currently require a manual refresh.",
                "The packaged capability probe is a real Get-StartApps round trip cached for a short 30-second interval so repeated capability checks cannot block the UI on every call.",
                "Packaged enumeration is timeout-bounded. If Windows cannot terminate a hung PowerShell child within the kill grace period, the call returns an explicit error; its pipe-reader threads end when that child eventually exits.",
                "The default Ctrl+Space hotkey is registered with RegisterHotKey on a dedicated message-loop thread; a conflicting registration is reported as an error instead of silently intercepting another shortcut.",
                "Hotkey lifecycle: HotkeyRegistration preserves the Ctrl+Space API; HotkeyManager adds probe/register/unregister/reconfigure with typed conflict errors, releases the old binding before re-registering and restores it when the new chord is rejected. ShortcutChord parses and normalizes the same modifier/key grammar the settings store validates; the recorder never injects input.",
                "Global hotkey callbacks carry no content and only wake the UI.",
                "Focus restoration is checked and best-effort: capture_foreground_target records the foreground top-level window, its PID, creating thread, process creation time and an open query-only process handle; dismiss_launcher hides the launcher only on its owning UI thread and restores the captured window only while the launcher still owns the foreground and the target still passes verification.",
                "launcher_is_foreground lets the UI distinguish Ctrl+Space while the launcher owns focus (dismiss) from Ctrl+Space after the user switched away (capture a new target and activate the launcher instead of hiding it).",
                "Windows can deny foreground activation even after these checks; dismiss_launcher reports Restored, Skipped (user switched away, no target, or target no longer safe) or Denied instead of claiming success.",
                "If the foreground process cannot be opened for limited query access, no target is captured and dismissal hides without restoring focus.",
                "A FocusTarget is not a generation-proof window token: an HWND destroyed and recreated with the same value in the same process and thread could still match. PID reuse is guarded by the held process handle and the creation-time check.",
                "Focus capture and restoration do not inject input, call AttachThreadInput, request elevation, or attempt to operate on a secure desktop.",
                "The launcher can natively show/hide its own HWND on the UI thread via set_launcher_visible; general window enumeration, activation of arbitrary windows and positioning are not implemented.",
                "Clipboard history is not implemented in this slice.",
                "Application launch and URL handoff use ShellExecuteW without cmd.exe or PowerShell string interpolation.",
                "URL handoff accepts only absolute http and https URLs.",
            ],
        }
    }
    #[cfg(not(windows))]
    {
        HostCapabilities {
            platform: Platform::current(),
            adapter: "none",
            application_catalog: CapabilityStatus::Unsupported,
            application_launch: CapabilityStatus::Unsupported,
            url_open: CapabilityStatus::Unsupported,
            global_hotkey: CapabilityStatus::Unsupported,
            packaged_applications: CapabilityStatus::Unsupported,
            clipboard_capture: CapabilityStatus::Unsupported,
            window_control: CapabilityStatus::Unsupported,
            notes: vec![
                "This adapter supports Windows only.",
                "No host capability is claimed on this platform.",
            ],
        }
    }
}

#[cfg(not(windows))]
fn unsupported(operation: &str) -> String {
    format!(
        "{operation} is unsupported on {}: the Windows adapter cannot run here",
        Platform::current()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_url_rejects_non_web_inputs() {
        assert!(validate_url("https://example.com").is_ok());
        assert!(validate_url("http://localhost:8080/").is_ok());
        assert!(validate_url("file:///C:/Windows/System32/calc.exe").is_err());
        assert!(validate_url("javascript:alert(1)").is_err());
        assert!(validate_url("cmd:/c calc").is_err());
        assert!(validate_url("https://").is_err());
        assert!(validate_url("https://example.com/\nnext").is_err());
        assert!(validate_url("not a url").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn open_url_rejects_non_http_schemes_before_launching() {
        assert!(open_url("file:///C:/Windows/System32/calc.exe").is_err());
        assert!(open_url("javascript:alert(1)").is_err());
        assert!(open_url("ms-settings:privacy").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn open_application_rejects_unsupported_targets_before_launching() {
        assert!(open_application("").is_err());
        assert!(open_application(r"C:\Windows\notepad.txt").is_err());
        assert!(open_application(r"C:\Windows\notepad.bat").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn capabilities_report_unimplemented_features_as_unsupported() {
        let capabilities = host_capabilities();
        assert_eq!(capabilities.platform, Platform::Windows);
        assert_eq!(
            capabilities.adapter,
            "windows-start-menu-packaged-launch-switch-v5"
        );
        assert_eq!(capabilities.application_launch, CapabilityStatus::Available);
        assert_eq!(capabilities.url_open, CapabilityStatus::Available);
        assert_eq!(capabilities.global_hotkey, CapabilityStatus::Available);
        assert!(
            matches!(
                capabilities.packaged_applications,
                CapabilityStatus::Available | CapabilityStatus::Degraded { .. }
            ),
            "packaged enumeration must be probed honestly: {:?}",
            capabilities.packaged_applications
        );
        if let CapabilityStatus::Degraded {
            reason,
            alternative,
        } = &capabilities.packaged_applications
        {
            assert!(reason.contains("packaged application enumeration"));
            assert!(!alternative.is_empty());
        }
        assert_eq!(
            capabilities.clipboard_capture,
            CapabilityStatus::Unsupported
        );
        assert_eq!(capabilities.window_control, CapabilityStatus::Unsupported);
        assert!(
            capabilities
                .notes
                .iter()
                .any(|note| note.contains("ExtractIconExW")
                    && note.contains("256x256")
                    && note.contains("512 KiB")),
            "capability notes must describe the bounded icon extraction"
        );
        assert!(
            capabilities
                .notes
                .iter()
                .any(|note| note.contains("refresh") && note.contains("ShellExecuteW")),
            "capability notes must describe broken-target handling"
        );
        assert!(
            capabilities
                .notes
                .iter()
                .any(|note| note.contains("different arguments") && note.contains("distinct")),
            "capability notes must describe argument-aware identity"
        );
        assert!(
            capabilities
                .notes
                .iter()
                .any(|note| note.contains("do not poll it yet")),
            "capability notes must disclose that the watcher is not wired"
        );
        assert!(
            capabilities
                .notes
                .iter()
                .any(|note| note.contains("Reveal-in-file-manager")
                    && note.contains("SHOpenFolderAndSelectItems")
                    && note.contains("copy_identity")),
            "capability notes must describe the reveal and copy-identity actions"
        );
        assert!(
            capabilities.notes.iter().any(|note| note
                .contains("Packaged application switching is Unsupported")
                && note.contains("Alternative:")),
            "capability notes must report packaged switching honestly"
        );
        assert!(
            capabilities
                .notes
                .iter()
                .any(|note| note.contains("HotkeyManager")
                    && note.contains("reconfigure")
                    && note.contains("conflict")),
            "capability notes must describe the hotkey lifecycle"
        );
        assert!(
            capabilities
                .notes
                .iter()
                .any(|note| note.contains("Focus restoration is checked and best-effort")),
            "focus restoration must be documented as checked and best-effort"
        );
        assert!(
            capabilities
                .notes
                .iter()
                .any(|note| note.contains("Restored, Skipped") && note.contains("Denied")),
            "capability notes must describe the restoration outcomes"
        );
        assert!(
            capabilities
                .notes
                .iter()
                .any(|note| note.contains("AttachThreadInput") && note.contains("secure desktop")),
            "capability notes must disclose the native safety limits"
        );
        assert!(
            capabilities
                .notes
                .iter()
                .any(|note| note.contains("generation-proof")),
            "capability notes must disclose the HWND-reuse residual risk"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_is_explicitly_unsupported() {
        assert!(discover_applications().is_err());
        assert!(open_application("C:\\App.lnk").is_err());
        assert!(open_url("https://example.com").is_err());
        assert_eq!(
            host_capabilities().application_catalog,
            CapabilityStatus::Unsupported
        );
        assert!(HotkeyRegistration::register(|| {}).is_err());
        assert!(set_launcher_visible(1, true).is_err());
        assert!(extract_icon_to_ico("C:\\App.exe", Path::new("C:\\cache")).is_err());
        assert!(validate_ico(&[]).is_err());
        assert!(reveal_in_file_manager("C:\\App.exe").is_err());
        assert!(capture_foreground_target(None).is_none());
        assert!(!launcher_is_foreground(1));
        assert!(dismiss_launcher(1, None).is_err());
        assert!(matches!(
            launch_or_switch("C:\\App.lnk"),
            LaunchOutcome::NoTarget { .. }
        ));
    }
}
