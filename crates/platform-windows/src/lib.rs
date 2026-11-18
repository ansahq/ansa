//! Initial Windows host adapter for Ansa.
//!
//! Scope of this slice:
//! * Discover `.lnk` and `.exe` entries in the per-user and all-users Start
//!   Menu `Programs` folders with bounded, non-recursive-looping traversal.
//! * Launch a validated `.lnk`/`.exe` path with `ShellExecuteW`.
//! * Open absolute `http`/`https` URLs only with `ShellExecuteW`.
//! * Register a validated global-hotkey chord (`Ctrl+Space` by default) on a
//!   dedicated, event-driven message-loop thread with deterministic cleanup,
//!   typed conflict/denial/invalid-chord errors and explicit
//!   suspend/resume/rearm lifecycle support.
//! * Show/hide the launcher's own HWND on the UI thread, including an explicit
//!   check for denied foreground activation.
//! * Capture the foreground top-level window before showing the launcher and
//!   restore it on dismissal only while the launcher still owns the foreground
//!   (`capture_foreground_target`, `launcher_is_foreground`,
//!   `dismiss_launcher`).
//! * Report what is and is not supported honestly.
//!
//! Not implemented or claimed here: packaged/MSIX application enumeration,
//! clipboard monitoring, general window activation/positioning and extension
//! isolation. Focus restoration is limited to returning to the window captured
//! before the launcher was shown; it is checked, best-effort and never injects
//! input.

#![deny(missing_debug_implementations)]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(not(windows), forbid(unsafe_code))]

mod focus;
#[cfg(windows)]
mod hotkey;
mod lifecycle;
#[cfg(windows)]
mod start_menu;
mod types;
mod url;
#[cfg(windows)]
mod visibility;
#[cfg(windows)]
mod win32;

pub use focus::{
    capture_foreground_target, dismiss_launcher, launcher_is_foreground, FocusRestoreOutcome,
    FocusTarget,
};
#[cfg(windows)]
pub use hotkey::{global_hotkey_capability, probe_chord};
#[cfg(windows)]
pub use hotkey::{
    HotkeyParseError, HotkeyRegistration, HotkeyRegistrationError, KeyChord, LifecycleOutcome,
    ProbeOutcome,
};
pub use lifecycle::{
    cursor_position, enumerate_monitors, hide_launcher, launcher_window_live, launcher_window_rect,
    place_and_show_launcher, place_launcher_window, read_apps_theme, request_launcher_focus,
    show_launcher_checked, ThemeError,
};
pub use types::{AppEntry, CapabilityStatus, HostCapabilities, Platform};
#[cfg(windows)]
pub use visibility::{
    enable_launcher_alpha, make_launcher_frameless, round_launcher_window, set_launcher_visible,
};

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

/// Validates that a URL can be handed to [`open_url`].
///
/// Only absolute `http` and `https` URLs with a non-empty host are accepted.
/// This can be used by the UI to reject input before invoking the host.
pub fn validate_url(url: &str) -> Result<(), String> {
    url::validate_http_url(url)
}

/// Discovers Start Menu `.lnk` and `.exe` entries.
///
/// Discovery is bounded and does not follow symbolic links, junctions or other
/// reparse points. Shortcut targets are not resolved; the shell resolves them
/// when the entry is launched.
pub fn discover_applications() -> Result<Vec<AppEntry>, String> {
    #[cfg(windows)]
    {
        start_menu::discover()
    }
    #[cfg(not(windows))]
    {
        Err(unsupported("application discovery"))
    }
}

/// Opens an existing `.lnk` or `.exe` application entry.
///
/// The path is passed as a single argument to `ShellExecuteW`; it is never
/// interpolated into a command line.
pub fn open_application(path: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        win32::open_application(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(unsupported("opening an application"))
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

/// Probes the host's current capabilities. The result is not cached.
pub fn host_capabilities() -> HostCapabilities {
    #[cfg(windows)]
    {
        HostCapabilities {
            platform: Platform::current(),
            adapter: "windows-start-menu-shellexecute-hotkey-v2",
            application_catalog: start_menu::probe_catalog_status(),
            application_launch: CapabilityStatus::Available,
            url_open: CapabilityStatus::Available,
            global_hotkey: hotkey::global_hotkey_capability(),
            packaged_applications: CapabilityStatus::Unsupported,
            clipboard_capture: CapabilityStatus::Unsupported,
            window_control: CapabilityStatus::Unsupported,
            notes: vec![
                "Application discovery scans only the per-user and all-users Start Menu Programs folders for .lnk and .exe entries.",
                "Shortcut targets are not resolved during discovery; the shell resolves them at launch.",
                "Packaged/MSIX application enumeration is not implemented and is reported as unsupported.",
                "The global hotkey is registered with RegisterHotKey on a dedicated message-loop thread; the adapter accepts a validated chord and reports conflict, denial, invalid-chord and worker failures as typed errors instead of silently intercepting another shortcut.",
                "Global hotkey support is probed with a transient RegisterHotKey call on each capability query; if the development-default Ctrl+Space is owned by an IME or another application, global summon is reported as degraded and a rebind or desktop-shortcut fallback is offered.",
                "The hotkey lifecycle exposes explicit suspend/resume/rearm operations. Automatic sleep/lock/wake re-registration is not wired: no message-only WM_POWERBROADCAST/WTSSESSION_CHANGE observer is created, so the host must call rearm after its own lifecycle event.",
                "The rebinding, persistence and recorder contracts live in ansa-platform-api; no rebinding UI is wired in this slice.",
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
                "No global-hotkey adapter exists for this platform; a desktop shortcut or direct launch is the documented fallback.",
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
            "windows-start-menu-shellexecute-hotkey-v2"
        );
        assert_eq!(capabilities.application_launch, CapabilityStatus::Available);
        assert_eq!(capabilities.url_open, CapabilityStatus::Available);
        assert!(
            matches!(
                capabilities.global_hotkey,
                CapabilityStatus::Available
                    | CapabilityStatus::Degraded { .. }
                    | CapabilityStatus::PermissionRequired
            ),
            "global hotkeys must be probed and reported honestly: {:?}",
            capabilities.global_hotkey
        );
        assert!(
            capabilities
                .notes
                .iter()
                .any(|note| note.contains("probed with a transient RegisterHotKey")),
            "capability notes must disclose that hotkey support is probed"
        );
        assert!(
            capabilities
                .notes
                .iter()
                .any(|note| note.contains("Automatic sleep/lock/wake re-registration is not wired")),
            "capability notes must not claim automatic sleep/lock handling"
        );
        assert_eq!(
            capabilities.packaged_applications,
            CapabilityStatus::Unsupported
        );
        assert_eq!(
            capabilities.clipboard_capture,
            CapabilityStatus::Unsupported
        );
        assert_eq!(capabilities.window_control, CapabilityStatus::Unsupported);
        assert!(
            capabilities
                .notes
                .iter()
                .any(|note| note.contains("not implemented")),
            "capability notes must disclose unimplemented features"
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
        assert!(capture_foreground_target(None).is_none());
        assert!(!launcher_is_foreground(1));
        assert!(dismiss_launcher(1, None).is_err());
    }
}
