//! The renderer-independent first-run onboarding state machine.
//!
//! A fresh profile can complete onboarding and reach a verified application
//! launch without an account, a network call, or any optional permission. Every
//! optional step can be skipped, but a skip is recorded with a warning instead
//! of being presented as a success.

use std::collections::BTreeMap;
use std::fmt;

use ansa_domain::CommandId;
use serde::{Deserialize, Serialize};

use crate::capability::{CapabilityNotice, render_notices};
use crate::launch::{LaunchOutcome, LaunchProbe, LaunchVerification};
use crate::permissions::{
    IntegrationChoice, IntegrationFeature, PermissionDecision, PermissionRequest,
};
use crate::shortcut::{
    ProbeOutcome, Shortcut, ShortcutChoice, ShortcutProbe, ShortcutVerification,
};
use crate::store::{OnboardingStore, StoreError};

/// Version of the persisted onboarding document.
pub const STATE_VERSION: u32 = 1;

/// The ordered onboarding steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Step {
    Welcome,
    Shortcut,
    TryLaunch,
    Integrations,
    Done,
}

impl Step {
    /// Every step, in order.
    pub const ALL: [Self; 5] = [
        Self::Welcome,
        Self::Shortcut,
        Self::TryLaunch,
        Self::Integrations,
        Self::Done,
    ];

    /// Zero-based position in the flow.
    pub fn index(self) -> usize {
        match self {
            Self::Welcome => 0,
            Self::Shortcut => 1,
            Self::TryLaunch => 2,
            Self::Integrations => 3,
            Self::Done => 4,
        }
    }

    /// The next step, if any.
    pub fn next(self) -> Option<Self> {
        Self::ALL.get(self.index() + 1).copied()
    }

    /// The previous step, if any.
    pub fn previous(self) -> Option<Self> {
        self.index().checked_sub(1).map(|index| Self::ALL[index])
    }

    /// A short heading for the step.
    pub fn title(self) -> &'static str {
        match self {
            Self::Welcome => "Welcome",
            Self::Shortcut => "Choose a shortcut",
            Self::TryLaunch => "Try a launch",
            Self::Integrations => "Optional integrations",
            Self::Done => "Ready",
        }
    }

    /// True for steps the user may skip.
    pub fn is_optional(self) -> bool {
        matches!(self, Self::Shortcut | Self::TryLaunch | Self::Integrations)
    }
}

impl fmt::Display for Step {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.title())
    }
}

/// Overall onboarding progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OnboardingStatus {
    NotStarted,
    InProgress,
    Completed,
}

impl OnboardingStatus {
    /// Stable label for diagnostics and tests.
    pub fn label(self) -> &'static str {
        match self {
            Self::NotStarted => "not-started",
            Self::InProgress => "in-progress",
            Self::Completed => "completed",
        }
    }

    /// True only after [`Onboarding::complete`] succeeded.
    pub fn is_completed(self) -> bool {
        matches!(self, Self::Completed)
    }
}

impl fmt::Display for OnboardingStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// A transition or persistence failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowError {
    AtStart,
    AtEnd,
    NotAtDone,
    ShortcutNotVerified,
    LaunchNotVerified,
    RequiredStep { step: Step },
    PermissionNotRequested { feature: IntegrationFeature },
    Store(StoreError),
}

impl fmt::Display for FlowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AtStart => formatter.write_str("already at the first onboarding step"),
            Self::AtEnd => formatter.write_str("already at the last onboarding step"),
            Self::NotAtDone => formatter.write_str("onboarding can only finish at the Done step"),
            Self::ShortcutNotVerified => formatter.write_str(
                "choose a shortcut the host reports as available, or explicitly continue with the documented default",
            ),
            Self::LaunchNotVerified => formatter.write_str(
                "the launch test must succeed before continuing; retry or skip with the recorded warning",
            ),
            Self::RequiredStep { step } => {
                write!(formatter, "the {step} step cannot be skipped")
            }
            Self::PermissionNotRequested { feature } => write!(
                formatter,
                "no permission request is pending for {}",
                feature.label()
            ),
            Self::Store(error) => write!(formatter, "could not save onboarding state: {error}"),
        }
    }
}

impl std::error::Error for FlowError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            _ => None,
        }
    }
}

impl From<StoreError> for FlowError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PersistedState {
    version: u32,
    status: OnboardingStatus,
    step: Step,
    #[serde(default)]
    skipped_steps: Vec<Step>,
    #[serde(default)]
    shortcut: Option<ShortcutChoice>,
    #[serde(default)]
    verified_launch: Option<LaunchVerification>,
    #[serde(default)]
    integrations: BTreeMap<IntegrationFeature, IntegrationChoice>,
    #[serde(default)]
    completion_count: u32,
    #[serde(default)]
    revisit: bool,
    #[serde(default)]
    warnings: BTreeMap<Step, String>,
}

impl PersistedState {
    fn fresh() -> Self {
        Self {
            version: STATE_VERSION,
            status: OnboardingStatus::NotStarted,
            step: Step::Welcome,
            skipped_steps: Vec::new(),
            shortcut: None,
            verified_launch: None,
            integrations: BTreeMap::new(),
            completion_count: 0,
            revisit: false,
            warnings: BTreeMap::new(),
        }
    }

    fn sanitize(mut self) -> Result<Self, String> {
        if self.version != STATE_VERSION {
            return Err(format!(
                "unsupported onboarding state version {} (expected {STATE_VERSION})",
                self.version
            ));
        }

        if self.status == OnboardingStatus::NotStarted {
            // A not-started profile always resumes at the beginning. This
            // discards inconsistent progress rather than trusting it.
            self = Self::fresh();
        }

        self.skipped_steps.sort_unstable();
        self.skipped_steps.dedup();
        Ok(self)
    }
}

/// First-run onboarding state with persistence.
///
/// The model owns no rendering, no platform calls and no network access. Host
/// behavior arrives through [`ShortcutProbe`] and [`LaunchProbe`], and saved
/// state arrives through an [`OnboardingStore`].
pub struct Onboarding {
    store: Box<dyn OnboardingStore>,
    state: PersistedState,
    notices: Vec<CapabilityNotice>,
    last_probe: Option<ProbeOutcome>,
    load_warning: Option<String>,
}

impl fmt::Debug for Onboarding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Onboarding")
            .field("status", &self.state.status)
            .field("step", &self.state.step)
            .field("completion_count", &self.state.completion_count)
            .field("has_shortcut", &self.state.shortcut.is_some())
            .field("verified_launch", &self.state.verified_launch)
            .finish_non_exhaustive()
    }
}

impl Onboarding {
    /// Loads onboarding state, falling back to a pristine profile when the
    /// document is missing, corrupt or from an unsupported version.
    ///
    /// A load problem is reported through [`Onboarding::load_warning`]; it
    /// never panics and never deletes the original document.
    pub fn load(store: impl OnboardingStore + 'static) -> Self {
        let store: Box<dyn OnboardingStore> = Box::new(store);
        let mut load_warning = None;

        let state = match store.load() {
            Ok(Some(document)) => match serde_json::from_str::<PersistedState>(&document) {
                Ok(state) => match state.sanitize() {
                    Ok(state) => state,
                    Err(warning) => {
                        load_warning = Some(format!(
                            "Saved onboarding state was not used: {warning}. Starting fresh."
                        ));
                        PersistedState::fresh()
                    }
                },
                Err(error) => {
                    load_warning = Some(format!(
                        "Saved onboarding state could not be read ({error}). Starting fresh."
                    ));
                    PersistedState::fresh()
                }
            },
            Ok(None) => PersistedState::fresh(),
            Err(error) => {
                load_warning = Some(format!(
                    "Saved onboarding state could not be opened ({error}). Starting fresh."
                ));
                PersistedState::fresh()
            }
        };

        Self {
            store,
            state,
            notices: Vec::new(),
            last_probe: None,
            load_warning,
        }
    }

    /// Starts a pristine profile without reading or clearing the store.
    pub fn fresh(store: impl OnboardingStore + 'static) -> Self {
        Self {
            store: Box::new(store),
            state: PersistedState::fresh(),
            notices: Vec::new(),
            last_probe: None,
            load_warning: None,
        }
    }

    /// Overall progress.
    pub fn status(&self) -> OnboardingStatus {
        self.state.status
    }

    /// The current step.
    pub fn step(&self) -> Step {
        self.state.step
    }

    /// True when the user can navigate backward.
    pub fn can_go_back(&self) -> bool {
        self.state.step != Step::Welcome
    }

    /// True only at the Done step.
    ///
    /// Reaching Done through [`Onboarding::next`] already required either a
    /// verified launch or an explicit, recorded skip.
    pub fn can_finish(&self) -> bool {
        self.state.step == Step::Done
    }

    /// True while the user is revisiting setup from settings.
    pub fn is_revisit(&self) -> bool {
        self.state.revisit
    }

    /// How many times onboarding has been completed.
    pub fn completion_count(&self) -> u32 {
        self.state.completion_count
    }

    /// True when a completion existed before the current revisit.
    pub fn has_completed_before(&self) -> bool {
        self.state.completion_count > 0
    }

    /// A warning produced by a corrupt or unreadable saved document.
    pub fn load_warning(&self) -> Option<&str> {
        self.load_warning.as_deref()
    }

    /// Steps the user explicitly skipped.
    pub fn skipped_steps(&self) -> &[Step] {
        &self.state.skipped_steps
    }

    /// True when the step was explicitly skipped or is skipped on the current
    /// pass.
    pub fn is_skipped(&self, step: Step) -> bool {
        self.state.skipped_steps.contains(&step)
    }

    /// The warning recorded for a skipped or fallback step.
    pub fn warning_for(&self, step: Step) -> Option<&str> {
        self.state.warnings.get(&step).map(String::as_str)
    }

    /// The selected shortcut, which may be an unverified documented default.
    pub fn chosen_shortcut(&self) -> Option<Shortcut> {
        self.state.shortcut.map(|choice| choice.shortcut())
    }

    /// The selected shortcut with its verification state.
    pub fn shortcut_choice(&self) -> Option<&ShortcutChoice> {
        self.state.shortcut.as_ref()
    }

    /// True only when a probe confirmed the shortcut is registered.
    pub fn shortcut_registered(&self) -> bool {
        self.state
            .shortcut
            .as_ref()
            .is_some_and(ShortcutChoice::is_registered)
    }

    /// True when the flow may leave the Shortcut step: either the probe
    /// confirmed the chord or the user explicitly chose the fallback.
    pub fn shortcut_ready(&self) -> bool {
        self.state.shortcut.is_some()
    }

    /// The most recent probe result, if a shortcut was tested this session.
    pub fn last_shortcut_probe(&self) -> Option<&ProbeOutcome> {
        self.last_probe.as_ref()
    }

    /// The catalog command whose launch was verified, if any.
    pub fn verified_launch(&self) -> Option<&LaunchVerification> {
        self.state.verified_launch.as_ref()
    }

    /// The persisted choice for an optional integration.
    pub fn integration_choice(&self, feature: IntegrationFeature) -> Option<&IntegrationChoice> {
        self.state.integrations.get(&feature)
    }

    /// The pending permission request for a feature the user enabled.
    ///
    /// Returns `None` until the user turns the feature on.
    pub fn permission_request(&self, feature: IntegrationFeature) -> Option<PermissionRequest> {
        self.state
            .integrations
            .get(&feature)
            .filter(|choice| choice.is_requested())
            .map(|_| feature.request())
    }

    /// All capability notices supplied by the host, including available ones.
    pub fn capability_notices(&self) -> &[CapabilityNotice] {
        &self.notices
    }

    /// Notices that need attention (unsupported, degraded, unavailable or
    /// permission-required). The renderer must show these; it cannot remove
    /// them from the model.
    pub fn notices_needing_attention(&self) -> Vec<&CapabilityNotice> {
        self.notices
            .iter()
            .filter(|notice| notice.needs_attention())
            .collect()
    }

    /// User-facing lines for every capability notice, never filtered.
    pub fn rendered_capability_notices(&self) -> Vec<String> {
        render_notices(&self.notices)
    }

    /// Replaces the host-supplied capability notices.
    pub fn set_capability_notices(&mut self, notices: Vec<CapabilityNotice>) {
        self.notices = notices;
    }

    /// Builder form of [`Onboarding::set_capability_notices`].
    pub fn with_capability_notices(mut self, notices: Vec<CapabilityNotice>) -> Self {
        self.notices = notices;
        self
    }

    /// Advances one step.
    ///
    /// The Shortcut step requires a confirmed registration or an explicit
    /// fallback, and the TryLaunch step requires a verified launch.
    // `next` is the ticket-specified flow command; onboarding is not an
    // iterator, so implementing `Iterator` would misrepresent it.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<(), FlowError> {
        let next = match self.state.step {
            Step::Welcome => Step::Shortcut,
            Step::Shortcut => {
                if !self.shortcut_ready() {
                    return Err(FlowError::ShortcutNotVerified);
                }
                Step::TryLaunch
            }
            Step::TryLaunch => {
                if self.state.verified_launch.is_none() {
                    return Err(FlowError::LaunchNotVerified);
                }
                Step::Integrations
            }
            Step::Integrations => Step::Done,
            Step::Done => return Err(FlowError::AtEnd),
        };

        self.state.step = next;
        if self.state.status == OnboardingStatus::NotStarted {
            self.state.status = OnboardingStatus::InProgress;
        }
        self.persist()
    }

    /// Moves back one step. Completion history is preserved, but a completed
    /// flow that moves away from Done becomes in progress again.
    pub fn back(&mut self) -> Result<(), FlowError> {
        let previous = self.state.step.previous().ok_or(FlowError::AtStart)?;
        self.state.step = previous;
        if self.state.status == OnboardingStatus::Completed {
            self.state.status = OnboardingStatus::InProgress;
        }
        self.persist()
    }

    /// Skips the current optional step with a recorded warning and advances.
    ///
    /// Skipping never fabricates a successful launch or a registered shortcut.
    pub fn skip(&mut self) -> Result<(), FlowError> {
        let step = self.state.step;
        if !step.is_optional() {
            return Err(FlowError::RequiredStep { step });
        }

        match step {
            Step::Shortcut => {
                if self.state.shortcut.is_none() {
                    let shortcut = Shortcut::documented_default();
                    self.state.shortcut = Some(ShortcutChoice::new(
                        shortcut,
                        ShortcutVerification::UnverifiedDefault,
                    ));
                    self.record_warning(
                        step,
                        format!(
                            "Shortcut setup was skipped. Using the documented default {}; registration has not been confirmed.",
                            shortcut.display()
                        ),
                    );
                    self.state.skipped_steps.push(step);
                }
            }
            Step::TryLaunch => {
                if self.state.verified_launch.is_none() {
                    self.record_warning(
                        step,
                        "The launch test was skipped. No successful application launch has been verified."
                            .to_owned(),
                    );
                    self.state.skipped_steps.push(step);
                }
            }
            Step::Integrations => {
                self.record_warning(
                    step,
                    "Optional integrations were skipped. No permissions were requested.".to_owned(),
                );
                self.state.skipped_steps.push(step);
            }
            Step::Welcome | Step::Done => {}
        }

        self.state.skipped_steps.sort_unstable();
        self.state.skipped_steps.dedup();
        self.state.step = step.next().ok_or(FlowError::AtEnd)?;
        if self.state.status == OnboardingStatus::NotStarted {
            self.state.status = OnboardingStatus::InProgress;
        }
        self.persist()
    }

    /// Finishes onboarding. Only valid at the Done step.
    pub fn complete(&mut self) -> Result<(), FlowError> {
        if self.state.step != Step::Done {
            return Err(FlowError::NotAtDone);
        }
        self.state.status = OnboardingStatus::Completed;
        self.state.completion_count = self.state.completion_count.saturating_add(1);
        self.state.revisit = false;
        self.persist()
    }

    /// Re-enters onboarding at `step` from settings without discarding the
    /// completion count, chosen shortcut, verified launch or integration
    /// choices.
    pub fn restart_from_settings(&mut self, step: Step) -> Result<(), FlowError> {
        self.state.step = step;
        self.state.revisit = true;
        if self.state.status != OnboardingStatus::InProgress {
            self.state.status = OnboardingStatus::InProgress;
        }
        self.persist()
    }

    /// Wipes onboarding state and clears the store. Capability notices, which
    /// describe the current host rather than onboarding progress, are kept.
    pub fn reset(&mut self) -> Result<(), FlowError> {
        self.state = PersistedState::fresh();
        self.last_probe = None;
        self.load_warning = None;
        self.store.clear().map_err(FlowError::Store)
    }

    /// Probes and records a shortcut.
    ///
    /// The choice is persisted only when the probe reports
    /// [`ProbeOutcome::Available`]. A conflict, unsupported platform or denial
    /// leaves any previously confirmed choice untouched so the user can try
    /// another candidate.
    pub fn select_shortcut(
        &mut self,
        shortcut: &Shortcut,
        probe: &dyn ShortcutProbe,
    ) -> Result<ProbeOutcome, FlowError> {
        let shortcut = *shortcut;
        let outcome = probe.probe(&shortcut);
        self.last_probe = Some(outcome.clone());

        if outcome.is_available() {
            self.state.shortcut = Some(ShortcutChoice::new(
                shortcut,
                ShortcutVerification::Registered,
            ));
            self.state.warnings.remove(&Step::Shortcut);
            self.state
                .skipped_steps
                .retain(|skipped| *skipped != Step::Shortcut);
        }

        self.persist()?;
        Ok(outcome)
    }

    /// Explicitly continues with the documented default without registration
    /// confirmation. The fallback is persisted with a warning and is never
    /// reported as registered.
    pub fn continue_with_default_shortcut(&mut self) -> Result<(), FlowError> {
        let shortcut = Shortcut::documented_default();
        self.state.shortcut = Some(ShortcutChoice::new(
            shortcut,
            ShortcutVerification::UnverifiedDefault,
        ));
        self.record_warning(
            Step::Shortcut,
            format!(
                "Continuing with the documented default {} without registration confirmation.",
                shortcut.display()
            ),
        );
        self.persist()
    }

    /// Attempts a real catalog launch through the host adapter.
    ///
    /// Only [`LaunchOutcome::Launched`] records a verification. A failure or an
    /// unverifiable result clears any earlier verification so the user must
    /// retry or skip with a warning.
    pub fn try_launch(
        &mut self,
        command_id: &CommandId,
        probe: &dyn LaunchProbe,
    ) -> Result<LaunchOutcome, FlowError> {
        let outcome = probe.launch(command_id);
        if outcome.is_launched() {
            self.state.verified_launch = Some(LaunchVerification::new(command_id.clone()));
            self.state.warnings.remove(&Step::TryLaunch);
            self.state
                .skipped_steps
                .retain(|skipped| *skipped != Step::TryLaunch);
        } else {
            self.state.verified_launch = None;
        }
        self.persist()?;
        Ok(outcome)
    }

    /// Enables an optional feature and returns its permission request.
    ///
    /// This is the only path that creates a request; merely visiting the
    /// Integrations step asks for nothing.
    pub fn enable_integration(
        &mut self,
        feature: IntegrationFeature,
    ) -> Result<PermissionRequest, FlowError> {
        let choice = self.state.integrations.entry(feature).or_default();
        choice.requested = true;
        choice.decision = None;
        self.persist()?;
        Ok(feature.request())
    }

    /// Records the user's answer to an outstanding permission request.
    ///
    /// A denial is stored and leaves every other step and feature usable.
    pub fn record_permission_decision(
        &mut self,
        feature: IntegrationFeature,
        decision: PermissionDecision,
    ) -> Result<(), FlowError> {
        let Some(choice) = self.state.integrations.get_mut(&feature) else {
            return Err(FlowError::PermissionNotRequested { feature });
        };
        if !choice.is_requested() {
            return Err(FlowError::PermissionNotRequested { feature });
        }
        choice.decision = Some(decision);
        self.persist()
    }

    /// Turns an integration back off and forgets its choice.
    pub fn disable_integration(&mut self, feature: IntegrationFeature) -> Result<(), FlowError> {
        self.state.integrations.remove(&feature);
        self.persist()
    }

    /// Saves the current state without changing it.
    pub fn save(&self) -> Result<(), FlowError> {
        self.persist()
    }

    fn record_warning(&mut self, step: Step, warning: String) {
        self.state.warnings.insert(step, warning);
    }

    fn persist(&self) -> Result<(), FlowError> {
        let document = serde_json::to_string(&self.state).map_err(|error| {
            FlowError::Store(StoreError::new(format!(
                "could not serialize onboarding state: {error}"
            )))
        })?;
        self.store.save(&document).map_err(FlowError::Store)
    }
}
