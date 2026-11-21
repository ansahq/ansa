//! Truthful capability notices for degraded or unsupported features.
//!
//! This is a local mirror of the launcher's capability vocabulary so the
//! onboarding crate stays independent of the platform and domain crates. A
//! notice is never filtered out of [`CapabilityNotice::render`] input by the
//! model: unsupported and degraded states are always shown with a reason and,
//! where one exists, a fallback.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Availability of a capability, matching the launcher's public vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityStatus {
    Available,
    PermissionRequired,
    TemporarilyUnavailable,
    Unsupported {
        reason: String,
    },
    /// Usable, but with a disclosed limitation and an explicit fallback.
    Degraded {
        reason: String,
        fallback: String,
    },
}

impl CapabilityStatus {
    /// True only when the full capability works now.
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    /// True when the capability can be used at all right now.
    pub fn is_usable(&self) -> bool {
        matches!(self, Self::Available | Self::Degraded { .. })
    }

    /// A stable machine-readable label for rendering and tests.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::PermissionRequired => "permission-required",
            Self::TemporarilyUnavailable => "temporarily-unavailable",
            Self::Unsupported { .. } => "unsupported",
            Self::Degraded { .. } => "degraded",
        }
    }

    /// Why the capability is not fully available.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Available => None,
            Self::PermissionRequired => Some("Permission required"),
            Self::TemporarilyUnavailable => Some("Temporarily unavailable"),
            Self::Unsupported { reason } => Some(reason),
            Self::Degraded { reason, .. } => Some(reason),
        }
    }

    /// A working alternative, when one exists.
    pub fn fallback(&self) -> Option<&str> {
        match self {
            Self::Degraded { fallback, .. } => Some(fallback),
            _ => None,
        }
    }
}

impl fmt::Display for CapabilityStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// A named capability and its current status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityNotice {
    pub capability: String,
    pub status: CapabilityStatus,
}

impl CapabilityNotice {
    /// Creates a notice for a named capability.
    pub fn new(capability: impl Into<String>, status: CapabilityStatus) -> Self {
        Self {
            capability: capability.into(),
            status,
        }
    }

    /// True when the notice needs the user's attention (anything but fully
    /// available).
    pub fn needs_attention(&self) -> bool {
        !self.status.is_available()
    }

    /// A one-line label such as `Clipboard history — unsupported`.
    pub fn headline(&self) -> String {
        format!("{} \u{2014} {}", self.capability, self.status.label())
    }

    /// Complete user-facing text, always including the reason and fallback.
    ///
    /// No notice is ever rendered as an empty string.
    pub fn render(&self) -> String {
        let mut text = self.headline();
        if let Some(reason) = self.status.reason() {
            text.push_str(": ");
            text.push_str(reason);
        }
        if let Some(fallback) = self.status.fallback() {
            text.push_str(" Fallback: ");
            text.push_str(fallback);
        }
        text.push('.');
        text
    }
}

/// Every rendered notice, including fully available capabilities.
///
/// The onboarding model uses this helper so a caller cannot accidentally hide
/// a degraded or unsupported state by filtering the list.
pub fn render_notices(notices: &[CapabilityNotice]) -> Vec<String> {
    notices.iter().map(CapabilityNotice::render).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_and_degraded_notices_render_reason_and_fallback() {
        let unsupported = CapabilityNotice::new(
            "Clipboard history",
            CapabilityStatus::Unsupported {
                reason: "No supported compositor".to_owned(),
            },
        );
        assert!(unsupported.needs_attention());
        assert!(unsupported.render().contains("No supported compositor"));
        assert!(unsupported.render().contains("unsupported"));

        let degraded = CapabilityNotice::new(
            "Window switching",
            CapabilityStatus::Degraded {
                reason: "Focus changes can be refused".to_owned(),
                fallback: "Copy the window title instead".to_owned(),
            },
        );
        assert!(degraded.status.is_usable());
        assert!(!degraded.status.is_available());
        assert!(degraded.render().contains("Focus changes can be refused"));
        assert!(degraded.render().contains("Copy the window title instead"));
    }

    #[test]
    fn render_notices_never_drops_an_entry() {
        let notices = vec![
            CapabilityNotice::new("Launching", CapabilityStatus::Available),
            CapabilityNotice::new(
                "Clipboard history",
                CapabilityStatus::TemporarilyUnavailable,
            ),
        ];
        let rendered = render_notices(&notices);
        assert_eq!(rendered.len(), 2);
        assert!(rendered[1].contains("Temporarily unavailable"));
    }
}
