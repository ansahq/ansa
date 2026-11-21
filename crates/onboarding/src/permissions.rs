//! Optional integrations and their permission requests.
//!
//! Every request names the feature, the data touched, and what stops working
//! if the user declines. Requests exist only after the user enables a feature;
//! onboarding never asks for optional access up front.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Optional features that may need a host permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IntegrationFeature {
    /// Clipboard change observation for history.
    ClipboardHistory,
    /// Automatic expansion of saved snippets while typing elsewhere.
    AutomaticSnippets,
    /// Enumerating and activating other windows.
    WindowSwitching,
    /// Capturing a screen region.
    ScreenCapture,
}

impl IntegrationFeature {
    /// Every optional integration, in onboarding order.
    pub const ALL: [Self; 4] = [
        Self::ClipboardHistory,
        Self::AutomaticSnippets,
        Self::WindowSwitching,
        Self::ScreenCapture,
    ];

    /// Human-readable feature name.
    pub fn label(self) -> &'static str {
        match self {
            Self::ClipboardHistory => "Clipboard history",
            Self::AutomaticSnippets => "Automatic snippets",
            Self::WindowSwitching => "Window switching",
            Self::ScreenCapture => "Screen capture",
        }
    }

    /// What the feature does, without promising more than it can deliver.
    pub fn summary(self) -> &'static str {
        match self {
            Self::ClipboardHistory => "Keep a short, local history of copied text.",
            Self::AutomaticSnippets => {
                "Expand saved snippets when their trigger is typed in other applications."
            }
            Self::WindowSwitching => "Find and focus open windows from the launcher.",
            Self::ScreenCapture => "Capture a screen region for a quick action.",
        }
    }

    /// The data this feature would touch if enabled.
    pub fn data_touched(self) -> &'static str {
        match self {
            Self::ClipboardHistory => "Clipboard contents copied after the feature is enabled.",
            Self::AutomaticSnippets => {
                "A short in-memory trigger buffer from the focused text field; general keystrokes are not stored."
            }
            Self::WindowSwitching => {
                "Window titles, process names and window geometry for running applications."
            }
            Self::ScreenCapture => "Pixels inside the region the user explicitly captures.",
        }
    }

    /// What the user loses by declining, stated plainly.
    pub fn denial_consequence(self) -> &'static str {
        match self {
            Self::ClipboardHistory => {
                "Clipboard history stays off. Copying and pasting still work normally."
            }
            Self::AutomaticSnippets => {
                "Snippets must be inserted manually from the launcher; automatic expansion stays off."
            }
            Self::WindowSwitching => {
                "Window results stay out of search; applications can still be launched."
            }
            Self::ScreenCapture => {
                "Screen capture actions stay unavailable; the rest of the launcher is unaffected."
            }
        }
    }

    /// Builds the request shown only after the user enables this feature.
    pub fn request(self) -> PermissionRequest {
        PermissionRequest {
            feature: self,
            data_touched: self.data_touched().to_owned(),
            denial_consequence: self.denial_consequence().to_owned(),
        }
    }
}

impl fmt::Display for IntegrationFeature {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// A named permission request with its data scope and denial consequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub feature: IntegrationFeature,
    pub data_touched: String,
    pub denial_consequence: String,
}

impl PermissionRequest {
    /// A single accessible sentence for a permission notice.
    pub fn explanation(&self) -> String {
        format!(
            "{}: touches {}. If denied: {}",
            self.feature.label(),
            self.data_touched,
            self.denial_consequence
        )
    }
}

/// The user's answer to a permission request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionDecision {
    Granted,
    Denied { reason: String },
}

impl PermissionDecision {
    /// True only when the user granted access.
    pub fn is_granted(&self) -> bool {
        matches!(self, Self::Granted)
    }

    /// The recorded denial reason, when present.
    pub fn denial_reason(&self) -> Option<&str> {
        match self {
            Self::Granted => None,
            Self::Denied { reason } => Some(reason),
        }
    }
}

/// A persisted integration decision.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationChoice {
    pub(crate) requested: bool,
    #[serde(default)]
    pub(crate) decision: Option<PermissionDecision>,
}

impl IntegrationChoice {
    /// True when the user turned the feature on (and therefore saw a request).
    pub fn is_requested(&self) -> bool {
        self.requested
    }

    /// The recorded answer, if the user answered.
    pub fn decision(&self) -> Option<&PermissionDecision> {
        self.decision.as_ref()
    }

    /// True only when the feature was enabled and access was granted.
    pub fn is_active(&self) -> bool {
        self.requested
            && self
                .decision
                .as_ref()
                .is_some_and(PermissionDecision::is_granted)
    }

    /// True when the user enabled the feature but denied the permission.
    pub fn is_denied(&self) -> bool {
        self.requested
            && self
                .decision
                .as_ref()
                .is_some_and(|decision| !decision.is_granted())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_feature_request_names_feature_data_and_consequence() {
        for feature in IntegrationFeature::ALL {
            let request = feature.request();
            assert_eq!(request.feature, feature);
            assert!(!request.data_touched.is_empty());
            assert!(!request.denial_consequence.is_empty());
            assert!(request.explanation().contains(feature.label()));
            assert!(request.explanation().contains("If denied"));
        }
    }

    #[test]
    fn denial_reason_is_preserved() {
        let decision = PermissionDecision::Denied {
            reason: "User declined".to_owned(),
        };
        assert!(!decision.is_granted());
        assert_eq!(decision.denial_reason(), Some("User declined"));
        assert_eq!(PermissionDecision::Granted.denial_reason(), None);
    }
}
