//! Platform-neutral host contract for the first Ansa platform slice.
//!
//! The contract is deliberately small:
//!
//! * [`AppEntry`] describes a launchable application discovered on the host.
//! * [`IconCacheEntry`] describes a bounded ICO produced by
//!   [`extract_icon_to_ico`].
//! * [`CapabilityStatus`] reports honestly whether a host feature works.
//! * [`HostCapabilities`] is a boundary capability probe. Catalog availability
//!   is fresh; the Windows packaged-app probe caches its result for a short
//!   30-second interval so repeated UI probes do not spawn PowerShell each
//!   time. The returned value is still a snapshot, not a permanent promise.
//!
//! The caller maps [`AppEntry`] into its own domain command type (for example
//! `ansa_domain::Command`) at the boundary. This crate intentionally does not
//! depend on `ansa-domain`, which avoids coupling the native adapter to domain
//! ID and ranking policy and avoids a circular dependency with the Windows
//! adapter.
//!
//! On Windows these functions delegate to `ansa-platform-windows`. macOS uses
//! bundle identity and LaunchServices; Linux uses XDG desktop entries.

#![deny(unsafe_code)]
#![deny(missing_debug_implementations)]

pub mod host_lifecycle;
pub mod hotkey;

mod opener;
pub use opener::{NativeOpenerProbe, OpenerCapabilities};

#[cfg(any(target_os = "macos", target_os = "linux", test))]
mod catalog;
#[cfg(any(target_os = "linux", test))]
mod linux_catalog;
#[cfg(any(target_os = "linux", test))]
mod linux_icons;
#[cfg(any(target_os = "macos", test))]
mod mac_catalog;

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
#[path = "mac_native.rs"]
mod imp;
#[cfg(target_os = "linux")]
#[path = "linux_native.rs"]
mod imp;

use std::fmt;
use std::path::{Path, PathBuf};

/// A launchable application entry returned by discovery.
///
/// `id` is stable across runs. The Windows adapter derives it from a resolved
/// shortcut target when one exists, otherwise from a case-folded,
/// separator-normalized path, and returns `app:<hex>`, which is compatible with
/// a namespaced `CommandId`. Callers should otherwise treat it as an opaque
/// identifier.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AppEntry {
    /// Opaque, stable identity for the discovered entry.
    pub id: String,
    /// Human-readable title supplied to the launcher.
    pub title: String,
    /// Native path that can be passed to [`open_application`].
    pub path: String,
    /// Resolved local target when the entry is a shortcut (or the executable
    /// itself for direct `.exe` entries). `None` for packaged applications and
    /// shortcuts that only store a shell ID list.
    pub target: Option<String>,
    /// Shortcut command-line arguments as a single opaque string. Shortcuts to
    /// the same target with different arguments are distinct commands.
    pub arguments: Option<String>,
    /// Working directory requested by the shortcut, when present.
    pub working_directory: Option<String>,
    /// Icon source path/string and optional index, for example
    /// `C:\App\app.exe,0`. Feed this to [`extract_icon_to_ico`] on Windows.
    pub icon_source: Option<String>,
    /// Whether this entry is a packaged/MSIX application launched by AUMID.
    pub packaged: bool,
    /// Why the entry is known to be broken, if it is. Broken entries remain
    /// visible so the user can refresh instead of silently losing them.
    pub broken: Option<String>,
    /// Per-discovery selection token for macOS and Linux.
    ///
    /// This is not part of the determined application identity. It lets a
    /// caller prove that it retained the exact entry from the current native
    /// snapshot rather than a rediscovered row at the same path and stable ID.
    /// Keep this value with the entry; do not construct it or compare it across
    /// processes. It is always zero on Windows and for manually built entries.
    #[doc(hidden)]
    pub selection: u64,
}

impl AppEntry {
    /// Creates an application entry with no resolved target or icon metadata.
    pub fn new(id: impl Into<String>, title: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            path: path.into(),
            target: None,
            arguments: None,
            working_directory: None,
            icon_source: None,
            packaged: false,
            broken: None,
            selection: 0,
        }
    }

    /// Returns `true` when discovery knows this entry cannot be launched.
    pub fn is_broken(&self) -> bool {
        self.broken.is_some()
    }

    /// Returns a deterministic, multi-line structured identity.
    ///
    /// The format is stable for CLI output, diagnostics and clipboard copy.
    /// Empty optional fields are rendered explicitly.
    pub fn identity_summary(&self) -> String {
        let mut output = String::with_capacity(512);
        output.push_str("id: ");
        output.push_str(&self.id);
        output.push('\n');
        output.push_str("title: ");
        output.push_str(&self.title);
        output.push('\n');
        output.push_str("path: ");
        output.push_str(&self.path);
        output.push('\n');
        output.push_str("target: ");
        output.push_str(self.target.as_deref().unwrap_or(""));
        output.push('\n');
        output.push_str("arguments: ");
        output.push_str(self.arguments.as_deref().unwrap_or(""));
        output.push('\n');
        output.push_str("working_directory: ");
        output.push_str(self.working_directory.as_deref().unwrap_or(""));
        output.push('\n');
        output.push_str("icon_source: ");
        output.push_str(self.icon_source.as_deref().unwrap_or(""));
        output.push('\n');
        output.push_str("packaged: ");
        output.push_str(if self.packaged { "true" } else { "false" });
        output.push('\n');
        output.push_str("broken: ");
        output.push_str(self.broken.as_deref().unwrap_or(""));
        output.push('\n');
        output
    }
}

/// A written icon cache entry returned by [`extract_icon_to_ico`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconCacheEntry {
    /// Absolute path of the written `.ico` file.
    pub path: String,
    /// Canonical source description, for example `C:\App\app.exe,0`.
    pub source: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Size of the written ICO file in bytes.
    pub bytes: u64,
}

/// What happened when the caller asked to launch or switch to an application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchOutcome {
    /// The native host accepted a launch or a new executable was started.
    Launched,
    /// An existing top-level window was observed as the foreground window.
    Switched {
        /// Native window handle that was verified as foreground.
        window: isize,
    },
    /// macOS reported the selected existing application as foreground.
    /// This does not assert focus of a particular window.
    SwitchedApplication { process_id: i32 },
    /// The host denied the activation request or the launch was rejected.
    Denied {
        /// Human-readable reason suitable for diagnostics or a status line.
        reason: String,
    },
    /// The shell reported a non-target launch failure. This is distinct from
    /// `NoTarget`, which is reserved for missing or moved targets.
    Failed {
        /// Human-readable reason suitable for diagnostics or a status line.
        reason: String,
    },
    /// Activation was requested but could not be verified. This is not success.
    Unverified {
        /// Human-readable reason suitable for diagnostics or a status line.
        reason: String,
    },
    /// There was no valid target to launch or switch to.
    NoTarget {
        /// Human-readable reason, including a refresh hint where applicable.
        reason: String,
    },
}

impl LaunchOutcome {
    /// Returns `true` only for [`Self::Launched`].
    pub fn is_launched(&self) -> bool {
        matches!(self, Self::Launched)
    }

    /// Returns `true` only for a verified window or application switch.
    pub fn is_switched(&self) -> bool {
        matches!(
            self,
            Self::Switched { .. } | Self::SwitchedApplication { .. }
        )
    }

    /// Returns `true` only when the caller has a positive, observed result.
    pub fn is_success(&self) -> bool {
        self.is_launched() || self.is_switched()
    }

    /// Returns a stable label for UI and logs.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Launched => "launched",
            Self::Switched { .. } | Self::SwitchedApplication { .. } => "switched",
            Self::Denied { .. } => "denied",
            Self::Failed { .. } => "failed",
            Self::Unverified { .. } => "unverified",
            Self::NoTarget { .. } => "no-target",
        }
    }

    /// An actionable error or explanation when the operation did not succeed.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Launched | Self::Switched { .. } | Self::SwitchedApplication { .. } => None,
            Self::Denied { reason }
            | Self::Failed { reason }
            | Self::Unverified { reason }
            | Self::NoTarget { reason } => Some(reason),
        }
    }
}

/// The operating-system family this process is running on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    Windows,
    MacOs,
    Linux,
    Other,
}

impl Platform {
    /// Returns the current process platform.
    pub fn current() -> Self {
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(target_os = "macos")]
        {
            Self::MacOs
        }
        #[cfg(target_os = "linux")]
        {
            Self::Linux
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            Self::Other
        }
    }

    /// Returns a short lowercase name suitable for logs and messages.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::MacOs => "macos",
            Self::Linux => "linux",
            Self::Other => "other",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Honest state of one host capability.
///
/// This mirrors the design contract in `work/platform-security.md`. A probe may
/// change between calls (for example, a directory can become readable), so
/// callers must not cache `PermissionRequired` or `TemporarilyUnavailable`
/// indefinitely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CapabilityStatus {
    /// The feature was probed and is usable.
    Available,
    /// The feature exists but the current user must grant permission.
    PermissionRequired,
    /// This host/platform has no implementation for the feature.
    Unsupported,
    /// The feature exists but cannot be used right now.
    TemporarilyUnavailable,
    /// The feature works with a documented limitation and alternative.
    Degraded {
        /// Why the capability is degraded.
        reason: String,
        /// What the caller or user can do instead.
        alternative: String,
    },
}

impl CapabilityStatus {
    /// Returns `true` only when the capability is fully available.
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    /// Returns a stable label for UI and diagnostics.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::PermissionRequired => "permission-required",
            Self::Unsupported => "unsupported",
            Self::TemporarilyUnavailable => "temporarily-unavailable",
            Self::Degraded { .. } => "degraded",
        }
    }
}

impl fmt::Display for CapabilityStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// Capabilities of the current host, as observed by a fresh probe.
///
/// Fields that are not implemented in this slice are reported as
/// [`CapabilityStatus::Unsupported`]; they are not silently omitted or claimed
/// to work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostCapabilities {
    /// Current platform.
    pub platform: Platform,
    /// Identifier/version of the adapter that answered the probe.
    pub adapter: &'static str,
    /// Native application catalog.
    pub application_catalog: CapabilityStatus,
    /// Launching a discovered application entry.
    pub application_launch: CapabilityStatus,
    /// Opening an `http`/`https` URL in the host handler.
    pub url_open: CapabilityStatus,
    /// System-wide launcher hotkey.
    pub global_hotkey: CapabilityStatus,
    /// Packaged/MSIX application enumeration.
    pub packaged_applications: CapabilityStatus,
    /// Clipboard history capture.
    pub clipboard_capture: CapabilityStatus,
    /// Window enumeration and activation/positioning.
    pub window_control: CapabilityStatus,
    /// Human-readable scope notes and known limitations.
    pub notes: Vec<&'static str>,
}

/// Discovers launchable applications known to the host.
///
/// A new bounded scan replaces the Unix adapter's discovery snapshot.
pub fn discover_applications() -> Result<Vec<AppEntry>, String> {
    imp::discover_applications()
}

/// Opens an application entry previously returned by [`discover_applications`].
///
/// The Windows adapter accepts only existing `.lnk` and `.exe` files. It never
/// builds a command line or interpolates the path into a shell string.
pub fn open_application(path: &str) -> Result<(), String> {
    #[cfg(windows)]
    return imp::open_application(path);
    #[cfg(not(windows))]
    match launch_or_switch(path) {
        outcome if outcome.is_success() => Ok(()),
        outcome => Err(outcome.reason().unwrap_or("No launch target").to_owned()),
    }
}

/// Launches an application or switches to an existing verified window.
///
/// On Windows a resolved executable is matched against top-level windows by
/// process image path. A switch is reported only when the target window is
/// observed as foreground. Missing, moved or unreadable targets return
/// [`LaunchOutcome::NoTarget`] with a refresh hint instead of a silent no-op.
/// macOS verifies the foreground application, not an individual window.
/// Linux launches literal argv and does not attempt switch.
pub fn launch_or_switch(path: &str) -> LaunchOutcome {
    imp::launch_or_switch(path, None)
}

/// Launches the selected application only when its identity matches the
/// current discovery snapshot on macOS and Linux.
///
/// This checks path and stable ID against the *current* snapshot. It cannot
/// distinguish an old selection from a newly discovered entry at the same path
/// and ID; use [`launch_or_switch_selected`] with the retained [`AppEntry`]
/// when the selection outlives a refresh. Windows keeps its existing path
/// validation and launch behavior.
pub fn launch_or_switch_identified(path: &str, id: &str) -> LaunchOutcome {
    imp::launch_or_switch(path, Some(id))
}

/// Launches or switches using the exact application entry the caller retained.
///
/// On macOS and Linux, the current native snapshot must still contain the same
/// path, stable ID and per-discovery selection token before the entry is
/// rechecked and launched. A rediscovery at the same path and ID receives a new
/// token, so an old row cannot launch its replacement. This is the API a
/// launcher UI should use after rendering a catalog. Windows retains its
/// existing path validation because its adapter has no per-discovery token.
pub fn launch_or_switch_selected(entry: &AppEntry) -> LaunchOutcome {
    imp::launch_or_switch_selected(entry)
}

/// Opens an `http` or `https` URL using the host's registered handler.
///
/// Any other scheme is rejected before the host is called.
pub fn open_url(url: &str) -> Result<(), String> {
    imp::open_url(url)
}

/// Executes an inert quicklink request under the caller's preview policy.
/// Always revalidates scheme, native path and actual file type before handoff.
/// This is not a substitute for the quicklink preview/commit gate.
pub fn execute_open_request(
    request: &ansa_quicklinks::OpenRequest,
    policy: ansa_quicklinks::OpenPolicy,
) -> Result<(), String> {
    opener::execute(request, policy)
}

/// Reports browser, reveal and default-app capability for the current backend.
pub fn opener_capabilities() -> OpenerCapabilities {
    imp::opener_capabilities()
}

/// Probes the host's current capabilities.
///
/// Permission and availability can change at runtime, so callers must not treat
/// the result as permanent. The Windows catalog probe is fresh on each call,
/// while packaged enumeration is cached by the adapter for up to 30 seconds to
/// avoid a PowerShell round trip on every UI probe.
pub fn host_capabilities() -> HostCapabilities {
    imp::host_capabilities()
}

/// Extracts one bounded icon image into a deterministic ICO cache file.
///
/// `icon_source` is the metadata string produced by discovery (for example
/// `C:\App\app.exe,0`) or an absolute executable path. The Windows adapter
/// validates the source, refuses reparse points, caps the image at 256x256 and
/// the file at 512 KiB, and cleans up temporary files on failure. The file name
/// is derived only from a normalized source key, so repeated calls are
/// deterministic.
///
/// An extraction failure is returned as an explicit error; callers should show
/// a generic icon instead of claiming a real one.
pub fn extract_icon_to_ico(icon_source: &str, cache_dir: &Path) -> Result<IconCacheEntry, String> {
    #[cfg(windows)]
    return imp::extract_icon_to_ico(icon_source, cache_dir);
    #[cfg(not(windows))]
    {
        let _ = (icon_source, cache_dir);
        Err("ICO extraction is only available on Windows; use a native icon resource or a generic icon".to_owned())
    }
}

/// Resolves a discovery icon source to a bounded native image (ICNS on macOS,
/// PNG from the XDG theme on Linux). Does not decode the image into a bitmap.
pub fn resolve_application_icon(icon_source: &str) -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    return mac_catalog::icon(Path::new(icon_source));
    #[cfg(target_os = "linux")]
    return imp::icon(icon_source);
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = icon_source;
        Err("Native icon resource lookup is unsupported on this platform".to_owned())
    }
}

/// Reveals an existing absolute path in the registered file manager.
///
/// The Windows adapter uses `SHParseDisplayName` and
/// `SHOpenFolderAndSelectItems`; it never builds an `explorer.exe` command line
/// from metadata. Relative paths, missing paths, NUL-containing paths and
/// reparse points are rejected before the shell is called.
pub fn reveal_in_file_manager(path: &str) -> Result<(), String> {
    imp::reveal_in_file_manager(path)
}

/// Returns a deterministic, multi-line structured identity for an entry.
///
/// This is a pure formatter available on every platform. It is the safe
/// fallback when reveal-in-file-manager is unavailable.
pub fn copy_identity(entry: &AppEntry) -> String {
    entry.identity_summary()
}

/// Returns the standard roots used by discovery and watching.
pub fn catalog_watch_roots() -> Result<Vec<PathBuf>, String> {
    imp::catalog_watch_roots()
}

#[cfg(windows)]
mod imp {
    use super::{
        AppEntry, CapabilityStatus, HostCapabilities, IconCacheEntry, LaunchOutcome,
        OpenerCapabilities, Platform,
    };
    use ansa_platform_windows as host;
    use std::path::{Path, PathBuf};

    pub(super) fn discover_applications() -> Result<Vec<AppEntry>, String> {
        host::discover_applications().map(|entries| {
            entries
                .into_iter()
                .map(|entry| AppEntry {
                    id: entry.id,
                    title: entry.title,
                    path: entry.path,
                    target: entry.target,
                    arguments: entry.arguments,
                    working_directory: entry.working_directory,
                    icon_source: entry.icon_source,
                    packaged: entry.packaged,
                    broken: entry.broken,
                    selection: 0,
                })
                .collect()
        })
    }

    pub(super) fn open_application(path: &str) -> Result<(), String> {
        host::open_application(path)
    }

    pub(super) fn launch_or_switch(path: &str, _expected_id: Option<&str>) -> LaunchOutcome {
        map_launch_outcome(host::launch_or_switch(path))
    }

    pub(super) fn launch_or_switch_selected(entry: &AppEntry) -> LaunchOutcome {
        // The Windows adapter validates the entry path and has no snapshot
        // token. Keep its existing behavior rather than inventing one.
        launch_or_switch(&entry.path, Some(&entry.id))
    }

    fn map_launch_outcome(outcome: host::LaunchOutcome) -> LaunchOutcome {
        match outcome {
            host::LaunchOutcome::Launched => LaunchOutcome::Launched,
            host::LaunchOutcome::Switched { window } => LaunchOutcome::Switched { window },
            host::LaunchOutcome::Denied { reason } => LaunchOutcome::Denied { reason },
            host::LaunchOutcome::Failed { reason } => LaunchOutcome::Failed { reason },
            host::LaunchOutcome::Unverified { reason } => LaunchOutcome::Unverified { reason },
            host::LaunchOutcome::NoTarget { reason } => LaunchOutcome::NoTarget { reason },
        }
    }

    pub(super) fn open_url(url: &str) -> Result<(), String> {
        host::open_url(url)
    }

    pub(super) fn open_document(_target: &crate::opener::LocalTarget) -> Result<(), String> {
        Err("Default application document opening is not implemented on Windows; reveal it in Explorer instead".to_owned())
    }

    pub(super) fn opener_capabilities() -> OpenerCapabilities {
        OpenerCapabilities {
            backend: crate::host_lifecycle::HostBackend::Windows,
            browser: CapabilityStatus::Available,
            file_manager: CapabilityStatus::Available,
            default_application: CapabilityStatus::Degraded {
                reason: "Default application routing supports web URLs only".to_owned(),
                alternative: "Reveal a local item in Explorer".to_owned(),
            },
        }
    }

    pub(super) fn extract_icon_to_ico(
        icon_source: &str,
        cache_dir: &Path,
    ) -> Result<IconCacheEntry, String> {
        host::extract_icon_to_ico(icon_source, cache_dir).map(|entry| IconCacheEntry {
            path: entry.cache_path.to_string_lossy().into_owned(),
            source: entry.source,
            width: entry.width,
            height: entry.height,
            bytes: entry.bytes,
        })
    }

    pub(super) fn reveal_in_file_manager(path: &str) -> Result<(), String> {
        host::reveal_in_file_manager(path)
    }

    pub(super) fn catalog_watch_roots() -> Result<Vec<PathBuf>, String> {
        host::default_watch_roots()
    }

    pub(super) fn host_capabilities() -> HostCapabilities {
        let capabilities = host::host_capabilities();
        HostCapabilities {
            platform: map_platform(capabilities.platform),
            adapter: capabilities.adapter,
            application_catalog: map_status(capabilities.application_catalog),
            application_launch: map_status(capabilities.application_launch),
            url_open: map_status(capabilities.url_open),
            global_hotkey: map_status(capabilities.global_hotkey),
            packaged_applications: map_status(capabilities.packaged_applications),
            clipboard_capture: map_status(capabilities.clipboard_capture),
            window_control: map_status(capabilities.window_control),
            notes: capabilities.notes,
        }
    }

    fn map_platform(platform: host::Platform) -> Platform {
        match platform {
            host::Platform::Windows => Platform::Windows,
            host::Platform::MacOs => Platform::MacOs,
            host::Platform::Linux => Platform::Linux,
            host::Platform::Other => Platform::Other,
        }
    }

    fn map_status(status: host::CapabilityStatus) -> CapabilityStatus {
        match status {
            host::CapabilityStatus::Available => CapabilityStatus::Available,
            host::CapabilityStatus::PermissionRequired => CapabilityStatus::PermissionRequired,
            host::CapabilityStatus::Unsupported => CapabilityStatus::Unsupported,
            host::CapabilityStatus::TemporarilyUnavailable => {
                CapabilityStatus::TemporarilyUnavailable
            }
            host::CapabilityStatus::Degraded {
                reason,
                alternative,
            } => CapabilityStatus::Degraded {
                reason,
                alternative,
            },
        }
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod imp {
    use super::{
        AppEntry, CapabilityStatus, HostCapabilities, LaunchOutcome, OpenerCapabilities, Platform,
    };
    use std::path::PathBuf;

    fn unsupported(operation: &str) -> String {
        format!(
            "{operation} is unsupported on {}: no native platform adapter is implemented in this slice",
            Platform::current()
        )
    }

    pub(super) fn discover_applications() -> Result<Vec<AppEntry>, String> {
        Err(unsupported("application discovery"))
    }

    pub(super) fn launch_or_switch(_path: &str, _expected_id: Option<&str>) -> LaunchOutcome {
        LaunchOutcome::NoTarget {
            reason: unsupported("launching or switching applications"),
        }
    }

    pub(super) fn launch_or_switch_selected(_entry: &AppEntry) -> LaunchOutcome {
        launch_or_switch("", None)
    }

    pub(super) fn open_url(_url: &str) -> Result<(), String> {
        Err(unsupported("opening a URL"))
    }

    pub(super) fn open_document(_target: &crate::opener::LocalTarget) -> Result<(), String> {
        Err(unsupported("opening a document"))
    }

    pub(super) fn opener_capabilities() -> OpenerCapabilities {
        OpenerCapabilities {
            backend: crate::host_lifecycle::HostBackend::Other,
            browser: CapabilityStatus::Unsupported,
            file_manager: CapabilityStatus::Unsupported,
            default_application: CapabilityStatus::Unsupported,
        }
    }

    pub(super) fn reveal_in_file_manager(_path: &str) -> Result<(), String> {
        Err(unsupported("revealing a path in the file manager"))
    }

    pub(super) fn catalog_watch_roots() -> Result<Vec<PathBuf>, String> {
        Err(unsupported("watching the application catalog"))
    }

    pub(super) fn host_capabilities() -> HostCapabilities {
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
                "No application discovery, launch or URL handler is claimed on this platform.",
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_entry_keeps_identity_and_metadata_fields() {
        let entry = AppEntry::new("app-1", "Example", r"C:\Example.lnk");
        assert_eq!(entry.id, "app-1");
        assert_eq!(entry.title, "Example");
        assert_eq!(entry.path, r"C:\Example.lnk");
        assert_eq!(entry.target, None);
        assert_eq!(entry.arguments, None);
        assert_eq!(entry.working_directory, None);
        assert_eq!(entry.icon_source, None);
        assert!(!entry.packaged);
        assert!(!entry.is_broken());
    }

    #[test]
    fn copy_identity_is_deterministic_and_complete() {
        let mut entry = AppEntry::new("app-1", "Example", r"C:\Example.lnk");
        entry.target = Some(r"C:\Example.exe".to_owned());
        entry.arguments = Some("--flag".to_owned());
        entry.working_directory = Some(r"C:\".to_owned());
        entry.icon_source = Some(r"C:\Example.exe,0".to_owned());
        entry.broken = Some("moved; refresh".to_owned());

        let identity = copy_identity(&entry);
        assert_eq!(identity, entry.identity_summary());
        assert!(identity.starts_with("id: app-1\ntitle: Example\npath: C:\\Example.lnk\n"));
        assert!(identity.contains("target: C:\\Example.exe\n"));
        assert!(identity.contains("arguments: --flag\n"));
        assert!(identity.contains("working_directory: C:\\\n"));
        assert!(identity.contains("icon_source: C:\\Example.exe,0\n"));
        assert!(identity.contains("packaged: false\n"));
        assert!(identity.contains("broken: moved; refresh\n"));

        let bare = AppEntry::new("app-2", "Bare", r"C:\Bare.exe");
        let identity = copy_identity(&bare);
        assert!(identity.contains("target: \n"));
        assert!(identity.contains("broken: \n"));
    }

    #[test]
    fn launch_outcome_success_is_conservative() {
        assert!(LaunchOutcome::Launched.is_success());
        assert!(LaunchOutcome::Switched { window: 3 }.is_success());
        assert!(LaunchOutcome::SwitchedApplication { process_id: 42 }.is_switched());
        assert!(!LaunchOutcome::Denied {
            reason: "denied".to_owned()
        }
        .is_success());
        assert!(!LaunchOutcome::Unverified {
            reason: "not observed".to_owned()
        }
        .is_success());
        assert!(!LaunchOutcome::NoTarget {
            reason: "missing".to_owned()
        }
        .is_success());
        assert!(!LaunchOutcome::Failed {
            reason: "shell failure".to_owned()
        }
        .is_success());
        assert_eq!(
            LaunchOutcome::Failed {
                reason: "shell failure".to_owned()
            }
            .label(),
            "failed"
        );
        assert_eq!(LaunchOutcome::Launched.label(), "launched");
        assert_eq!(LaunchOutcome::Switched { window: 3 }.label(), "switched");
    }

    #[test]
    fn capability_labels_are_stable() {
        assert_eq!(CapabilityStatus::Available.label(), "available");
        assert_eq!(
            CapabilityStatus::PermissionRequired.label(),
            "permission-required"
        );
        assert_eq!(CapabilityStatus::Unsupported.label(), "unsupported");
        assert_eq!(
            CapabilityStatus::TemporarilyUnavailable.label(),
            "temporarily-unavailable"
        );
        assert_eq!(
            CapabilityStatus::Degraded {
                reason: "no packaged catalog".to_owned(),
                alternative: "use classic entries".to_owned(),
            }
            .label(),
            "degraded"
        );
    }

    #[test]
    fn only_available_counts_as_available() {
        assert!(CapabilityStatus::Available.is_available());
        assert!(!CapabilityStatus::PermissionRequired.is_available());
        assert!(!CapabilityStatus::Unsupported.is_available());
        assert!(!CapabilityStatus::TemporarilyUnavailable.is_available());
        assert!(!CapabilityStatus::Degraded {
            reason: "x".to_owned(),
            alternative: "y".to_owned(),
        }
        .is_available());
    }

    #[test]
    fn platform_names_are_stable() {
        assert_eq!(Platform::Windows.as_str(), "windows");
        assert_eq!(Platform::MacOs.as_str(), "macos");
        assert_eq!(Platform::Linux.as_str(), "linux");
        assert_eq!(Platform::Other.as_str(), "other");
    }

    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    #[test]
    fn non_windows_api_is_explicitly_unsupported() {
        assert!(discover_applications().is_err());
        assert!(open_application("anything").is_err());
        assert!(open_url("https://example.com").is_err());
        assert!(extract_icon_to_ico("anything", std::path::Path::new("cache")).is_err());
        assert!(reveal_in_file_manager("anything").is_err());
        assert!(catalog_watch_roots().is_err());
        assert!(matches!(
            launch_or_switch("anything"),
            LaunchOutcome::NoTarget { .. }
        ));
        assert_eq!(
            host_capabilities().application_catalog,
            CapabilityStatus::Unsupported
        );
    }

    #[cfg(windows)]
    #[test]
    fn icon_extraction_returns_a_real_non_empty_ico_or_reports_an_error() {
        let root = std::env::var_os("SystemRoot")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"));
        let notepad = root.join("System32").join("notepad.exe");
        if !notepad.is_file() {
            eprintln!("skipping platform-api icon test: notepad.exe is absent");
            return;
        }
        let cache =
            std::env::temp_dir().join(format!("ansa-platform-api-icon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache);
        let source = format!("{},0", notepad.display());
        let entry = match extract_icon_to_ico(&source, &cache) {
            Ok(entry) => entry,
            Err(error) => {
                eprintln!("skipping platform-api icon test: {error}");
                return;
            }
        };
        let bytes = std::fs::read(&entry.path).expect("read ICO cache file");
        assert!(!bytes.is_empty());
        assert_eq!(entry.bytes, bytes.len() as u64);
        assert_eq!(&bytes[..4], &[0, 0, 1, 0]);
        assert_eq!(entry.width.min(256), entry.width);
        assert_eq!(entry.height.min(256), entry.height);
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[cfg(windows)]
    #[test]
    fn capabilities_do_not_claim_unimplemented_features() {
        let capabilities = host_capabilities();
        assert_eq!(capabilities.platform, Platform::Windows);
        assert!(
            matches!(
                capabilities.packaged_applications,
                CapabilityStatus::Available | CapabilityStatus::Degraded { .. }
            ),
            "packaged enumeration must be probed honestly: {:?}",
            capabilities.packaged_applications
        );
        assert_eq!(capabilities.global_hotkey, CapabilityStatus::Available);
        assert_eq!(
            capabilities.clipboard_capture,
            CapabilityStatus::Unsupported
        );
        assert_eq!(capabilities.window_control, CapabilityStatus::Unsupported);
        assert_eq!(capabilities.url_open, CapabilityStatus::Available);
    }
}
