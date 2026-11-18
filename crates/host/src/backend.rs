//! Native effects interface and shutdown plan.

use crate::focus::{FocusOutcome, ShowOutcome};
use crate::geometry::Rect;
use crate::monitor::MonitorInfo;

/// System appearance observed from the OS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeAppearance {
    /// Light application theme.
    Light,
    /// Dark application theme.
    Dark,
}

impl ThemeAppearance {
    /// Stable label for diagnostics and status text.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

impl std::fmt::Display for ThemeAppearance {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.label())
    }
}

/// One step of the ordered shutdown release plan.
///
/// The order matters: the hotkey worker must be joined before its sender clone
/// disappears, and worker channels must be closed after native handles so a
/// late wake-up cannot touch a destroyed window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReleaseStep {
    /// Unregister the global hotkey and join its worker thread.
    Hotkeys,
    /// Hide/forget the launcher's native window handles.
    NativeHandles,
    /// Close the summon worker channel so its task can exit.
    WorkerChannels,
}

impl ReleaseStep {
    /// Stable label for logs.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Hotkeys => "hotkeys",
            Self::NativeHandles => "native-handles",
            Self::WorkerChannels => "worker-channels",
        }
    }
}

/// The fixed release order used by [`crate::HostController::shutdown`].
pub const RELEASE_PLAN: [ReleaseStep; 3] = [
    ReleaseStep::Hotkeys,
    ReleaseStep::NativeHandles,
    ReleaseStep::WorkerChannels,
];

/// One failed release step. Failures do not skip later steps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseFailure {
    /// The step that failed.
    pub step: ReleaseStep,
    /// Adapter-supplied reason.
    pub message: String,
}

/// Result of one shutdown call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShutdownReport {
    /// `true` when shutdown had already completed; no steps ran.
    pub already_shut_down: bool,
    /// Steps that completed, in execution order.
    pub released: Vec<ReleaseStep>,
    /// Steps that returned an error, in execution order.
    pub failures: Vec<ReleaseFailure>,
}

impl ShutdownReport {
    /// Returns `true` when every planned step completed.
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty()
            && (self.already_shut_down || self.released.len() == RELEASE_PLAN.len())
    }
}

/// Typed controller/adapter error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostError {
    /// The controller has already shut down and cannot show or hide.
    ShutDown,
    /// No invocation has been issued yet.
    NoActiveInvocation,
    /// The caller presented an invocation that is no longer current.
    StaleInvocation {
        /// The invocation that is currently active.
        expected: crate::InvocationId,
        /// The invocation supplied by the caller.
        received: crate::InvocationId,
    },
    /// The platform reported no monitors on which to place the launcher.
    NoMonitors,
    /// A native/backend operation failed.
    Native {
        /// Operation name such as `show`, `hide` or `list_monitors`.
        operation: &'static str,
        /// Adapter-supplied detail.
        message: String,
    },
    /// The system appearance could not be observed and must not be guessed.
    ThemeUnavailable {
        /// Adapter-supplied detail.
        reason: String,
    },
}

impl HostError {
    /// Builds a [`Self::Native`] error.
    pub fn native(operation: &'static str, message: impl Into<String>) -> Self {
        Self::Native {
            operation,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for HostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ShutDown => formatter.write_str("the launcher host has already shut down"),
            Self::NoActiveInvocation => formatter.write_str("no launcher invocation is active"),
            Self::StaleInvocation { expected, received } => write!(
                formatter,
                "ignored invocation {received}: {expected} is the current invocation"
            ),
            Self::NoMonitors => formatter.write_str("no active monitor is available for placement"),
            Self::Native { operation, message } => {
                write!(formatter, "native {operation} failed: {message}")
            }
            Self::ThemeUnavailable { reason } => {
                write!(formatter, "system appearance is unavailable: {reason}")
            }
        }
    }
}

impl std::error::Error for HostError {}

/// Native effects used by [`crate::HostController`].
///
/// Implementations live in platform adapters and must perform every call on
/// the UI thread that owns the native window. `Debug` is required so the
/// controller can derive `Debug` without hiding its backend state.
pub trait HostBackend: std::fmt::Debug {
    /// Enumerates active monitors in physical pixels.
    fn list_monitors(&mut self) -> Result<Vec<MonitorInfo>, HostError>;

    /// Places and shows the launcher at `rect`, returning focus separately
    /// from visibility. An implementation must never claim success silently.
    fn show(&mut self, rect: Rect) -> Result<ShowOutcome, HostError>;

    /// Moves the existing launcher to `rect` without showing or changing
    /// focus. Used to reapply placement after a DPI or size change.
    fn place(&mut self, rect: Rect) -> Result<(), HostError>;

    /// Hides the launcher.
    fn hide(&mut self) -> Result<(), HostError>;

    /// Requests foreground activation without changing visibility.
    fn request_focus(&mut self) -> FocusOutcome;

    /// Reads the OS application theme. A missing value is an error, not a
    /// guess.
    fn observe_theme(&mut self) -> Result<ThemeAppearance, HostError>;

    /// Releases one shutdown step. The default is a no-op for adapters that do
    /// not own that kind of resource.
    fn release(&mut self, step: ReleaseStep) -> Result<(), HostError> {
        let _ = step;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_plan_has_the_documented_order() {
        assert_eq!(
            RELEASE_PLAN.map(|step| step.label()),
            ["hotkeys", "native-handles", "worker-channels"]
        );
    }

    #[test]
    fn theme_labels_are_stable() {
        assert_eq!(ThemeAppearance::Light.label(), "light");
        assert_eq!(ThemeAppearance::Dark.label(), "dark");
    }

    #[test]
    fn shutdown_completeness_requires_every_step() {
        let complete = ShutdownReport {
            already_shut_down: false,
            released: RELEASE_PLAN.to_vec(),
            failures: Vec::new(),
        };
        assert!(complete.is_complete());

        let partial = ShutdownReport {
            already_shut_down: false,
            released: vec![ReleaseStep::Hotkeys],
            failures: Vec::new(),
        };
        assert!(!partial.is_complete());

        let repeated = ShutdownReport {
            already_shut_down: true,
            released: Vec::new(),
            failures: Vec::new(),
        };
        assert!(repeated.is_complete());
    }
}
