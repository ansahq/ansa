//! Platform-neutral value types returned by the Windows adapter.
//!
//! `ansa-platform-api` defines structurally identical types. It does not depend
//! on this crate's types directly on non-Windows builds, so the Windows adapter
//! stays self-contained and has no dependency on the domain crate.

use std::fmt;

/// A launchable application entry returned by discovery.
///
/// `id` is `app:<hex>`, stable for the same host path across runs and
/// compatible with a namespaced domain command identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AppEntry {
    /// Opaque, stable identity for the discovered entry.
    pub id: String,
    /// Human-readable title supplied to the launcher.
    pub title: String,
    /// Native path that can be passed to `open_application`.
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
