//! Platform-neutral host contract for the first Ansa platform slice.
//!
//! The contract is deliberately small and dependency-free:
//!
//! * [`AppEntry`] describes a launchable application discovered on the host.
//! * [`CapabilityStatus`] reports honestly whether a host feature works.
//! * [`HostCapabilities`] is a fresh capability probe, not a cached promise.
//! * [`hotkey`] defines the platform-neutral chord grammar, registration
//!   outcomes, persisted configuration, manager state machine and recorder.
//!
//! The caller maps [`AppEntry`] into its own domain command type (for example
//! `ansa_domain::Command`) at the boundary. This crate intentionally does not
//! depend on `ansa-domain`, which avoids coupling the native adapter to domain
//! ID and ranking policy and avoids a circular dependency with the Windows
//! adapter.
//!
//! On Windows these functions delegate to `ansa-platform-windows`. On every
//! other operating system they return an explicit unsupported error instead of
//! pretending a native adapter exists.

#![forbid(unsafe_code)]
#![deny(missing_debug_implementations)]

use std::fmt;

pub mod hotkey;

pub use hotkey::{
    ChordParseError, ConflictCheck, Hotkey, HotkeyBackend, HotkeyBinding, HotkeyConfig,
    HotkeyConfigError, HotkeyConfigLoad, HotkeyError, HotkeyManager, HotkeyRecovery, HotkeyStatus,
    HotkeyStore, HotkeyStoreError, HotkeyWake, IgnoreReason, Key, KeyChord, Modifier, Modifiers,
    RearmOutcome, RebindOutcome, RecordOutcome, RecorderCancel, RecorderError, RecorderEvent,
    RecorderKey, RecorderPhase, RegistrationOutcome, ResumeOutcome, ShortcutRecorder,
    ShutdownOutcome, SuspendOutcome, DEFAULT_SUMMON_CHORD, HOTKEY_CONFIG_HEADER,
    HOTKEY_CONFIG_VERSION, SUMMON_ACTION,
};

#[cfg(windows)]
pub use hotkey::WindowsHotkeyBackend;

/// A launchable application entry returned by discovery.
///
/// `id` is stable for the same host path across runs. The Windows adapter
/// derives it from a case-folded, separator-normalized path and returns
/// `app:<hex>`, which is compatible with a namespaced `CommandId`. Callers
/// should otherwise treat it as an opaque identifier.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AppEntry {
    /// Opaque, stable identity for the discovered entry.
    pub id: String,
    /// Human-readable title supplied to the launcher.
    pub title: String,
    /// Native path that can be passed to [`open_application`].
    pub path: String,
}

impl AppEntry {
    /// Creates an application entry.
    pub fn new(id: impl Into<String>, title: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            path: path.into(),
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
    /// Start Menu application catalog.
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
/// On Windows this is a bounded Start Menu catalog scan. On all other
/// platforms it returns an explicit unsupported error.
pub fn discover_applications() -> Result<Vec<AppEntry>, String> {
    imp::discover_applications()
}

/// Opens an application entry previously returned by [`discover_applications`].
///
/// The Windows adapter accepts only existing `.lnk` and `.exe` files. It never
/// builds a command line or interpolates the path into a shell string.
pub fn open_application(path: &str) -> Result<(), String> {
    imp::open_application(path)
}

/// Opens an `http` or `https` URL using the host's registered handler.
///
/// Any other scheme is rejected before the host is called.
pub fn open_url(url: &str) -> Result<(), String> {
    imp::open_url(url)
}

/// Probes the host's current capabilities.
///
/// The result is intentionally not cached: permission and availability can
/// change at runtime.
pub fn host_capabilities() -> HostCapabilities {
    imp::host_capabilities()
}

#[cfg(windows)]
mod imp {
    use super::{AppEntry, CapabilityStatus, HostCapabilities, Platform};
    use ansa_platform_windows as host;

    pub(super) fn discover_applications() -> Result<Vec<AppEntry>, String> {
        host::discover_applications().map(|entries| {
            entries
                .into_iter()
                .map(|entry| AppEntry {
                    id: entry.id,
                    title: entry.title,
                    path: entry.path,
                })
                .collect()
        })
    }

    pub(super) fn open_application(path: &str) -> Result<(), String> {
        host::open_application(path)
    }

    pub(super) fn open_url(url: &str) -> Result<(), String> {
        host::open_url(url)
    }

    pub(super) fn host_capabilities() -> HostCapabilities {
        let mut capabilities = host::host_capabilities();
        capabilities.notes.push(
            "The hotkey chord, persistence, manager and recorder contracts are implemented in ansa-platform-api; no desktop rebinding UI is wired in this slice.",
        );
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

#[cfg(not(windows))]
mod imp {
    use super::{AppEntry, CapabilityStatus, HostCapabilities, Platform};

    fn unsupported(operation: &str) -> String {
        format!(
            "{operation} is unsupported on {}: no native platform adapter is implemented in this slice",
            Platform::current()
        )
    }

    pub(super) fn discover_applications() -> Result<Vec<AppEntry>, String> {
        Err(unsupported("application discovery"))
    }

    pub(super) fn open_application(_path: &str) -> Result<(), String> {
        Err(unsupported("opening an application"))
    }

    pub(super) fn open_url(_url: &str) -> Result<(), String> {
        Err(unsupported("opening a URL"))
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
                "This slice implements the Windows adapter only.",
                "No application discovery, launch or URL handler is claimed on this platform.",
                "No global-hotkey adapter exists on this platform; global_hotkey is unsupported and the fallback is a desktop shortcut or direct launch.",
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_entry_keeps_identity_fields() {
        let entry = AppEntry::new("app-1", "Example", r"C:\Example.lnk");
        assert_eq!(entry.id, "app-1");
        assert_eq!(entry.title, "Example");
        assert_eq!(entry.path, r"C:\Example.lnk");
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

    #[cfg(not(windows))]
    #[test]
    fn non_windows_api_is_explicitly_unsupported() {
        assert!(discover_applications().is_err());
        assert!(open_application("anything").is_err());
        assert!(open_url("https://example.com").is_err());
        assert_eq!(
            host_capabilities().application_catalog,
            CapabilityStatus::Unsupported
        );
    }

    #[cfg(windows)]
    #[test]
    fn capabilities_do_not_claim_unimplemented_features() {
        let capabilities = host_capabilities();
        assert_eq!(capabilities.platform, Platform::Windows);
        assert_eq!(
            capabilities.packaged_applications,
            CapabilityStatus::Unsupported
        );
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
        assert_eq!(
            capabilities.clipboard_capture,
            CapabilityStatus::Unsupported
        );
        assert_eq!(capabilities.window_control, CapabilityStatus::Unsupported);
        assert_eq!(capabilities.url_open, CapabilityStatus::Available);
    }
}
