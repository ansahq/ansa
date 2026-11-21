//! Platform-neutral value types returned by the Windows adapter.
//!
//! `ansa-platform-api` defines structurally identical types. It does not depend
//! on this crate's types directly on non-Windows builds, so the Windows adapter
//! stays self-contained and has no dependency on the domain crate.

use std::fmt;
use std::path::PathBuf;

/// A launchable application entry returned by discovery.
///
/// `id` is `app:<hex>` and stable across runs. Entries whose `.lnk` resolves to
/// a local target derive their identity from that target, so several shortcuts
/// to the same executable collapse into one entry. Entries with no resolved
/// local target keep the path-derived identity used by earlier releases.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AppEntry {
    /// Opaque, stable identity for the discovered entry.
    pub id: String,
    /// Human-readable title supplied to the launcher.
    pub title: String,
    /// Launchable path: a local `.lnk`/`.exe` file or `shell:AppsFolder\<AUMID>`.
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
    /// `C:\App\app.exe,0`. [`crate::extract_icon_to_ico`] can turn this bounded
    /// metadata into an ICO cache file on Windows.
    pub icon_source: Option<String>,
    /// Whether this entry is a packaged/MSIX application launched by AUMID.
    pub packaged: bool,
    /// Why the entry is known to be broken, if it is. Broken entries stay in
    /// the catalog so the user can see and refresh them instead of silently
    /// losing the application.
    pub broken: Option<String>,
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
        }
    }

    /// Returns `true` when discovery knows this entry cannot be launched.
    pub fn is_broken(&self) -> bool {
        self.broken.is_some()
    }

    /// Returns a deterministic, multi-line structured identity.
    ///
    /// The format is stable for CLI output, diagnostics and clipboard copy.
    /// Empty optional fields are rendered explicitly so a consumer can
    /// distinguish an absent value from a formatting error.
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

/// A successfully written icon cache entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconCacheEntry {
    /// Absolute path of the written `.ico` file.
    pub cache_path: PathBuf,
    /// Canonical description of the source, for example `C:\App\app.exe,0`.
    pub source: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Size of the written ICO file in bytes.
    pub bytes: u64,
}

impl fmt::Display for IconCacheEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}x{} {} ({} bytes) from {}",
            self.width,
            self.height,
            self.cache_path.display(),
            self.bytes,
            self.source
        )
    }
}

/// What happened when the caller asked to launch or switch to an application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchOutcome {
    /// A new process/instance was handed to the shell.
    Launched,
    /// An existing top-level window was observed as the foreground window.
    Switched {
        /// Native window handle that was verified as foreground.
        window: isize,
    },
    /// Windows denied the activation request or the launch was rejected.
    Denied {
        /// Human-readable reason suitable for diagnostics or a status line.
        reason: String,
    },
    /// The shell reported a non-target launch failure (for example a missing
    /// handler or a sharing violation). This is distinct from `NoTarget`.
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

    /// Returns `true` only for a verified [`Self::Switched`] outcome.
    pub fn is_switched(&self) -> bool {
        matches!(self, Self::Switched { .. })
    }

    /// Returns `true` only when the caller has a positive, observed result.
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Launched | Self::Switched { .. })
    }

    /// Returns a stable label for UI and logs.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Launched => "launched",
            Self::Switched { .. } => "switched",
            Self::Denied { .. } => "denied",
            Self::Failed { .. } => "failed",
            Self::Unverified { .. } => "unverified",
            Self::NoTarget { .. } => "no-target",
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
