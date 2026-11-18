//! Focus outcomes recorded by every show/activation path.
//!
//! A launcher can be visible without owning the foreground. The type here makes
//! that distinction explicit: only [`FocusOutcome::Succeeded`] is success and
//! every other variant keeps the human-readable reason Windows or an adapter
//! supplied. No path is allowed to claim activation silently.

/// The observed result of a show or activation request.
///
/// The variant set is intentionally closed: `Succeeded`, `Denied`, `Skipped`
/// and `Unsupported`. A future adapter must extend the enum deliberately rather
/// than inventing a free-form status string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FocusOutcome {
    /// The window is foreground, or activation was observed after the request.
    Succeeded,
    /// Windows (or another platform) rejected the activation request.
    Denied {
        /// Human-readable reason suitable for diagnostics or a status line.
        reason: String,
    },
    /// Activation was deliberately not attempted because the request was
    /// unsafe, redundant or arrived on the wrong thread.
    Skipped {
        /// Human-readable reason suitable for diagnostics or a status line.
        reason: String,
    },
    /// This adapter cannot request activation at all.
    Unsupported {
        /// Human-readable reason suitable for diagnostics or a status line.
        reason: String,
    },
}

impl FocusOutcome {
    /// Returns `true` only for [`Self::Succeeded`].
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Succeeded)
    }

    /// Returns a stable, lowercase label for logs and counters.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Denied { .. } => "denied",
            Self::Skipped { .. } => "skipped",
            Self::Unsupported { .. } => "unsupported",
        }
    }

    /// Returns the adapter's reason for a non-success outcome.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Succeeded => None,
            Self::Denied { reason } | Self::Skipped { reason } | Self::Unsupported { reason } => {
                Some(reason)
            }
        }
    }
}

impl std::fmt::Display for FocusOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Succeeded => formatter.write_str("succeeded"),
            Self::Denied { reason } => write!(formatter, "denied: {reason}"),
            Self::Skipped { reason } => write!(formatter, "skipped: {reason}"),
            Self::Unsupported { reason } => write!(formatter, "unsupported: {reason}"),
        }
    }
}

/// Visibility plus the focus outcome of one native show call.
///
/// `visible` is separate from `focus` on purpose: `visible == true` with a
/// non-success focus is a valid, honestly reported state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShowOutcome {
    /// Whether the native window became visible.
    pub visible: bool,
    /// What happened to foreground activation.
    pub focus: FocusOutcome,
}

impl ShowOutcome {
    /// Returns `true` only when the window is visible *and* focus succeeded.
    pub fn is_success(&self) -> bool {
        self.visible && self.focus.is_success()
    }
}

/// Counts used by automation to prove that focus outcomes are not faked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FocusOutcomeCounts {
    /// Number of [`FocusOutcome::Succeeded`] observations.
    pub succeeded: usize,
    /// Number of [`FocusOutcome::Denied`] observations.
    pub denied: usize,
    /// Number of [`FocusOutcome::Skipped`] observations.
    pub skipped: usize,
    /// Number of [`FocusOutcome::Unsupported`] observations.
    pub unsupported: usize,
}

impl FocusOutcomeCounts {
    /// Records one outcome.
    pub fn record(&mut self, outcome: &FocusOutcome) {
        match outcome {
            FocusOutcome::Succeeded => self.succeeded += 1,
            FocusOutcome::Denied { .. } => self.denied += 1,
            FocusOutcome::Skipped { .. } => self.skipped += 1,
            FocusOutcome::Unsupported { .. } => self.unsupported += 1,
        }
    }

    /// Total number of recorded observations.
    pub fn total(&self) -> usize {
        self.succeeded + self.denied + self.skipped + self.unsupported
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcomes() -> Vec<FocusOutcome> {
        vec![
            FocusOutcome::Succeeded,
            FocusOutcome::Denied {
                reason: "foreground lock".into(),
            },
            FocusOutcome::Skipped {
                reason: "wrong thread".into(),
            },
            FocusOutcome::Unsupported {
                reason: "no adapter".into(),
            },
        ]
    }

    #[test]
    fn only_success_is_success() {
        for outcome in outcomes() {
            assert_eq!(
                outcome.is_success(),
                matches!(outcome, FocusOutcome::Succeeded)
            );
            assert_eq!(outcome.reason().is_none(), outcome.is_success());
        }
        assert!(
            !ShowOutcome {
                visible: true,
                focus: FocusOutcome::Denied {
                    reason: "denied".into()
                }
            }
            .is_success()
        );
    }

    #[test]
    fn labels_are_stable_and_display_includes_the_reason() {
        let outcomes = outcomes();
        let labels: Vec<_> = outcomes.iter().map(FocusOutcome::label).collect();
        assert_eq!(labels, ["succeeded", "denied", "skipped", "unsupported"]);
        for blocked in &outcomes[1..] {
            assert!(blocked.to_string().starts_with(blocked.label()));
            assert!(blocked.to_string().contains(blocked.reason().unwrap()));
        }
        assert_eq!(outcomes[0].to_string(), "succeeded");
    }

    #[test]
    fn counters_record_every_variant_exactly_once() {
        let mut counts = FocusOutcomeCounts::default();
        for outcome in outcomes() {
            counts.record(&outcome);
        }
        assert_eq!(counts.succeeded, 1);
        assert_eq!(counts.denied, 1);
        assert_eq!(counts.skipped, 1);
        assert_eq!(counts.unsupported, 1);
        assert_eq!(counts.total(), 4);
    }
}
