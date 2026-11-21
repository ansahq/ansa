//! Public-API tests for the first-run onboarding journey.

use std::fs;

use ansa_domain::CommandId;
use ansa_onboarding::{
    CapabilityNotice, CapabilityStatus, FlowError, IntegrationFeature, JsonFileStore,
    LaunchOutcome, LaunchProbe, MemoryStore, Onboarding, OnboardingStatus, OnboardingStore,
    PermissionDecision, ProbeOutcome, Shortcut, ShortcutProbe, ShortcutVerification, Step,
};

struct ShortcutOutcomes {
    outcomes: Vec<(Shortcut, ProbeOutcome)>,
}

impl ShortcutOutcomes {
    fn new(outcomes: Vec<(Shortcut, ProbeOutcome)>) -> Self {
        Self { outcomes }
    }
}

impl ShortcutProbe for ShortcutOutcomes {
    fn probe(&self, shortcut: &Shortcut) -> ProbeOutcome {
        self.outcomes
            .iter()
            .find(|(candidate, _)| candidate == shortcut)
            .map(|(_, outcome)| outcome.clone())
            .unwrap_or(ProbeOutcome::Unsupported)
    }
}

struct FixedLaunch(LaunchOutcome);

impl LaunchProbe for FixedLaunch {
    fn launch(&self, _command_id: &CommandId) -> LaunchOutcome {
        self.0.clone()
    }
}

fn command() -> CommandId {
    CommandId::parse("app:com.example.Editor").unwrap()
}

fn available_shortcut() -> ShortcutOutcomes {
    ShortcutOutcomes::new(vec![
        (Shortcut::documented_default(), ProbeOutcome::Available),
        (
            Shortcut::parse("Alt+Space").unwrap(),
            ProbeOutcome::Available,
        ),
    ])
}

fn launched() -> FixedLaunch {
    FixedLaunch(LaunchOutcome::Launched)
}

fn reach_shortcut(onboarding: &mut Onboarding) {
    onboarding.next().unwrap();
    assert_eq!(onboarding.step(), Step::Shortcut);
}

fn reach_launch(onboarding: &mut Onboarding) {
    reach_shortcut(onboarding);
    onboarding
        .select_shortcut(&Shortcut::documented_default(), &available_shortcut())
        .unwrap();
    onboarding.next().unwrap();
    assert_eq!(onboarding.step(), Step::TryLaunch);
}

fn reach_integrations(onboarding: &mut Onboarding) {
    reach_launch(onboarding);
    onboarding.try_launch(&command(), &launched()).unwrap();
    onboarding.next().unwrap();
    assert_eq!(onboarding.step(), Step::Integrations);
}

fn complete_happy_path(onboarding: &mut Onboarding) {
    onboarding.next().unwrap();
    onboarding
        .select_shortcut(&Shortcut::documented_default(), &available_shortcut())
        .unwrap();
    onboarding.next().unwrap();
    onboarding.try_launch(&command(), &launched()).unwrap();
    onboarding.next().unwrap();
    onboarding.next().unwrap();
    assert_eq!(onboarding.step(), Step::Done);
    onboarding.complete().unwrap();
}

#[test]
fn fresh_profile_reaches_verified_launch_without_account_or_permissions() {
    let store = MemoryStore::new();
    let mut onboarding = Onboarding::load(store.clone());

    assert_eq!(onboarding.status(), OnboardingStatus::NotStarted);
    assert_eq!(onboarding.step(), Step::Welcome);
    assert!(!onboarding.can_finish());
    assert!(!onboarding.can_go_back());
    assert!(!onboarding.has_completed_before());

    onboarding.next().unwrap();
    assert_eq!(onboarding.status(), OnboardingStatus::InProgress);
    assert_eq!(onboarding.step(), Step::Shortcut);

    assert_eq!(onboarding.next(), Err(FlowError::ShortcutNotVerified));
    let outcome = onboarding
        .select_shortcut(&Shortcut::documented_default(), &available_shortcut())
        .unwrap();
    assert_eq!(outcome, ProbeOutcome::Available);
    assert!(onboarding.shortcut_registered());
    assert_eq!(
        onboarding.shortcut_choice().unwrap().verification(),
        ShortcutVerification::Registered
    );

    onboarding.next().unwrap();
    assert_eq!(onboarding.step(), Step::TryLaunch);
    assert_eq!(onboarding.next(), Err(FlowError::LaunchNotVerified));

    let outcome = onboarding.try_launch(&command(), &launched()).unwrap();
    assert_eq!(outcome, LaunchOutcome::Launched);
    assert_eq!(
        onboarding.verified_launch().unwrap().command_id().as_str(),
        "app:com.example.Editor"
    );

    onboarding.next().unwrap();
    assert_eq!(onboarding.step(), Step::Integrations);
    for feature in IntegrationFeature::ALL {
        assert_eq!(onboarding.permission_request(feature), None);
        assert!(onboarding.integration_choice(feature).is_none());
    }

    onboarding.next().unwrap();
    assert_eq!(onboarding.step(), Step::Done);
    assert!(onboarding.can_finish());
    onboarding.complete().unwrap();
    assert_eq!(onboarding.status(), OnboardingStatus::Completed);
    assert_eq!(onboarding.completion_count(), 1);

    let reloaded = Onboarding::load(store);
    assert_eq!(reloaded.status(), OnboardingStatus::Completed);
    assert_eq!(reloaded.step(), Step::Done);
    assert!(reloaded.shortcut_registered());
    assert_eq!(
        reloaded.chosen_shortcut().unwrap(),
        Shortcut::documented_default()
    );
    assert_eq!(reloaded.verified_launch().unwrap().command_id(), &command());
    assert!(reloaded.skipped_steps().is_empty());
}

#[test]
fn shortcut_conflict_is_recoverable_with_an_alternate() {
    let store = MemoryStore::new();
    let mut onboarding = Onboarding::load(store.clone());
    reach_shortcut(&mut onboarding);

    let probe = ShortcutOutcomes::new(vec![
        (
            Shortcut::documented_default(),
            ProbeOutcome::Conflict {
                source: "Another Launcher".to_owned(),
            },
        ),
        (
            Shortcut::parse("Alt+Space").unwrap(),
            ProbeOutcome::Available,
        ),
    ]);

    let outcome = onboarding
        .select_shortcut(&Shortcut::documented_default(), &probe)
        .unwrap();
    assert!(matches!(outcome, ProbeOutcome::Conflict { .. }));
    assert!(!onboarding.shortcut_registered());
    assert_eq!(onboarding.chosen_shortcut(), None);
    assert_eq!(onboarding.next(), Err(FlowError::ShortcutNotVerified));

    let alternate = Shortcut::parse("Alt+Space").unwrap();
    assert_eq!(
        onboarding.select_shortcut(&alternate, &probe).unwrap(),
        ProbeOutcome::Available
    );
    assert_eq!(onboarding.chosen_shortcut(), Some(alternate));
    assert!(onboarding.shortcut_registered());
    assert_eq!(
        onboarding.last_shortcut_probe(),
        Some(&ProbeOutcome::Available)
    );
    onboarding.next().unwrap();
    assert_eq!(onboarding.step(), Step::TryLaunch);

    let reloaded = Onboarding::load(store);
    assert_eq!(reloaded.chosen_shortcut(), Some(alternate));
    assert!(reloaded.shortcut_registered());
}

#[test]
fn a_conflicting_retry_does_not_replace_a_confirmed_shortcut() {
    let mut onboarding = Onboarding::load(MemoryStore::new());
    reach_shortcut(&mut onboarding);

    let probe = ShortcutOutcomes::new(vec![
        (Shortcut::documented_default(), ProbeOutcome::Available),
        (
            Shortcut::parse("Alt+Space").unwrap(),
            ProbeOutcome::Conflict {
                source: "Desktop".to_owned(),
            },
        ),
    ]);

    onboarding
        .select_shortcut(&Shortcut::documented_default(), &probe)
        .unwrap();
    onboarding
        .select_shortcut(&Shortcut::parse("Alt+Space").unwrap(), &probe)
        .unwrap();

    assert_eq!(
        onboarding.chosen_shortcut(),
        Some(Shortcut::documented_default())
    );
    assert!(onboarding.shortcut_registered());
}

#[test]
fn unsupported_shortcut_can_continue_with_the_documented_default() {
    let store = MemoryStore::new();
    let mut onboarding = Onboarding::load(store.clone());
    reach_shortcut(&mut onboarding);

    let probe = ShortcutOutcomes::new(vec![]);
    let outcome = onboarding
        .select_shortcut(&Shortcut::documented_default(), &probe)
        .unwrap();
    assert_eq!(outcome, ProbeOutcome::Unsupported);
    assert_eq!(onboarding.next(), Err(FlowError::ShortcutNotVerified));
    assert!(!onboarding.shortcut_registered());

    onboarding.continue_with_default_shortcut().unwrap();
    assert_eq!(
        onboarding.chosen_shortcut(),
        Some(Shortcut::documented_default())
    );
    assert!(!onboarding.shortcut_registered());
    assert_eq!(
        onboarding.shortcut_choice().unwrap().verification(),
        ShortcutVerification::UnverifiedDefault
    );
    let warning = onboarding.warning_for(Step::Shortcut).unwrap();
    assert!(warning.contains("without registration confirmation"));

    onboarding.next().unwrap();
    assert_eq!(onboarding.step(), Step::TryLaunch);

    let reloaded = Onboarding::load(store);
    assert!(!reloaded.shortcut_registered());
    assert_eq!(
        reloaded.shortcut_choice().unwrap().verification(),
        ShortcutVerification::UnverifiedDefault
    );
}

#[test]
fn denied_shortcut_is_recoverable_with_another_candidate() {
    let mut onboarding = Onboarding::load(MemoryStore::new());
    reach_shortcut(&mut onboarding);

    let probe = ShortcutOutcomes::new(vec![
        (
            Shortcut::documented_default(),
            ProbeOutcome::Denied {
                reason: "Blocked by policy".to_owned(),
            },
        ),
        (
            Shortcut::parse("Ctrl+Shift+Space").unwrap(),
            ProbeOutcome::Available,
        ),
    ]);

    let outcome = onboarding
        .select_shortcut(&Shortcut::documented_default(), &probe)
        .unwrap();
    assert!(matches!(outcome, ProbeOutcome::Denied { .. }));
    assert_eq!(onboarding.next(), Err(FlowError::ShortcutNotVerified));

    let alternate = Shortcut::parse("Ctrl+Shift+Space").unwrap();
    onboarding.select_shortcut(&alternate, &probe).unwrap();
    assert!(onboarding.shortcut_registered());
    assert_eq!(onboarding.chosen_shortcut(), Some(alternate));
    onboarding.next().unwrap();
}

#[test]
fn launch_failure_can_retry_then_succeed() {
    let mut onboarding = Onboarding::load(MemoryStore::new());
    reach_launch(&mut onboarding);

    let failure = FixedLaunch(LaunchOutcome::Failed {
        reason: "the target disappeared".to_owned(),
    });
    let outcome = onboarding.try_launch(&command(), &failure).unwrap();
    assert!(matches!(outcome, LaunchOutcome::Failed { .. }));
    assert!(onboarding.verified_launch().is_none());
    assert_eq!(onboarding.next(), Err(FlowError::LaunchNotVerified));

    let outcome = onboarding.try_launch(&command(), &launched()).unwrap();
    assert_eq!(outcome, LaunchOutcome::Launched);
    assert_eq!(
        onboarding.verified_launch().unwrap().command_id(),
        &command()
    );
    onboarding.next().unwrap();
    assert_eq!(onboarding.step(), Step::Integrations);
}

#[test]
fn unverified_launch_is_not_recorded_as_success() {
    let mut onboarding = Onboarding::load(MemoryStore::new());
    reach_launch(&mut onboarding);

    let unverified = FixedLaunch(LaunchOutcome::Unverified);
    let outcome = onboarding.try_launch(&command(), &unverified).unwrap();
    assert_eq!(outcome, LaunchOutcome::Unverified);
    assert!(onboarding.verified_launch().is_none());
    assert_eq!(onboarding.next(), Err(FlowError::LaunchNotVerified));
}

#[test]
fn skipped_launch_records_a_warning_without_faking_success() {
    let store = MemoryStore::new();
    let mut onboarding = Onboarding::load(store.clone());
    reach_launch(&mut onboarding);

    let failure = FixedLaunch(LaunchOutcome::Failed {
        reason: "blocked".to_owned(),
    });
    onboarding.try_launch(&command(), &failure).unwrap();
    onboarding.skip().unwrap();

    assert_eq!(onboarding.step(), Step::Integrations);
    assert!(onboarding.verified_launch().is_none());
    assert!(onboarding.is_skipped(Step::TryLaunch));
    assert!(
        onboarding
            .warning_for(Step::TryLaunch)
            .unwrap()
            .contains("No successful application launch has been verified")
    );

    onboarding.next().unwrap();
    onboarding.complete().unwrap();
    assert_eq!(onboarding.status(), OnboardingStatus::Completed);

    let reloaded = Onboarding::load(store);
    assert!(reloaded.verified_launch().is_none());
    assert!(reloaded.is_skipped(Step::TryLaunch));
    assert!(reloaded.warning_for(Step::TryLaunch).is_some());
}

#[test]
fn permission_denial_explains_consequence_and_leaves_the_rest_usable() {
    let store = MemoryStore::new();
    let mut onboarding = Onboarding::load(store.clone());
    reach_integrations(&mut onboarding);

    assert_eq!(
        onboarding.permission_request(IntegrationFeature::ClipboardHistory),
        None
    );

    let request = onboarding
        .enable_integration(IntegrationFeature::ClipboardHistory)
        .unwrap();
    assert_eq!(request.feature, IntegrationFeature::ClipboardHistory);
    assert!(request.data_touched.contains("Clipboard"));
    assert!(
        request
            .denial_consequence
            .contains("Clipboard history stays off")
    );
    assert_eq!(
        onboarding.permission_request(IntegrationFeature::ClipboardHistory),
        Some(request.clone())
    );

    onboarding
        .record_permission_decision(
            IntegrationFeature::ClipboardHistory,
            PermissionDecision::Denied {
                reason: "Not now".to_owned(),
            },
        )
        .unwrap();

    let choice = onboarding
        .integration_choice(IntegrationFeature::ClipboardHistory)
        .unwrap();
    assert!(choice.is_requested());
    assert!(choice.is_denied());
    assert!(!choice.is_active());
    assert_eq!(choice.decision().unwrap().denial_reason(), Some("Not now"));

    // The rest of onboarding still works, and another feature can be enabled.
    let request = onboarding
        .enable_integration(IntegrationFeature::ScreenCapture)
        .unwrap();
    assert_eq!(request.feature, IntegrationFeature::ScreenCapture);
    onboarding
        .record_permission_decision(
            IntegrationFeature::ScreenCapture,
            PermissionDecision::Granted,
        )
        .unwrap();
    assert!(
        onboarding
            .integration_choice(IntegrationFeature::ScreenCapture)
            .unwrap()
            .is_active()
    );

    onboarding.next().unwrap();
    onboarding.complete().unwrap();
    assert_eq!(onboarding.status(), OnboardingStatus::Completed);

    let reloaded = Onboarding::load(store);
    assert!(
        reloaded
            .integration_choice(IntegrationFeature::ClipboardHistory)
            .unwrap()
            .is_denied()
    );
    assert!(
        reloaded
            .integration_choice(IntegrationFeature::ScreenCapture)
            .unwrap()
            .is_active()
    );
}

#[test]
fn permission_decision_requires_an_enabled_feature() {
    let mut onboarding = Onboarding::load(MemoryStore::new());
    reach_integrations(&mut onboarding);

    let result = onboarding.record_permission_decision(
        IntegrationFeature::AutomaticSnippets,
        PermissionDecision::Granted,
    );
    assert_eq!(
        result,
        Err(FlowError::PermissionNotRequested {
            feature: IntegrationFeature::AutomaticSnippets
        })
    );
}

#[test]
fn every_optional_step_can_be_skipped() {
    let store = MemoryStore::new();
    let mut onboarding = Onboarding::load(store.clone());
    assert_eq!(
        onboarding.skip(),
        Err(FlowError::RequiredStep {
            step: Step::Welcome
        })
    );

    onboarding.next().unwrap();
    onboarding.skip().unwrap();
    assert_eq!(onboarding.step(), Step::TryLaunch);
    assert!(!onboarding.shortcut_registered());
    assert!(onboarding.is_skipped(Step::Shortcut));

    onboarding.skip().unwrap();
    assert_eq!(onboarding.step(), Step::Integrations);
    assert!(onboarding.verified_launch().is_none());
    assert!(onboarding.is_skipped(Step::TryLaunch));

    onboarding.skip().unwrap();
    assert_eq!(onboarding.step(), Step::Done);
    assert!(onboarding.is_skipped(Step::Integrations));
    assert_eq!(
        onboarding.skipped_steps(),
        &[Step::Shortcut, Step::TryLaunch, Step::Integrations]
    );
    assert!(onboarding.can_finish());
    assert_eq!(
        onboarding.skip(),
        Err(FlowError::RequiredStep { step: Step::Done })
    );

    onboarding.complete().unwrap();
    assert_eq!(onboarding.status(), OnboardingStatus::Completed);

    let reloaded = Onboarding::load(store);
    assert_eq!(
        reloaded.skipped_steps(),
        &[Step::Shortcut, Step::TryLaunch, Step::Integrations]
    );
    assert_eq!(reloaded.status(), OnboardingStatus::Completed);
}

#[test]
fn completion_and_reset_persist() {
    let store = MemoryStore::new();
    let mut onboarding = Onboarding::load(store.clone());
    complete_happy_path(&mut onboarding);
    assert!(store.document().is_some());

    onboarding.reset().unwrap();
    assert_eq!(store.document(), None);
    assert_eq!(onboarding.status(), OnboardingStatus::NotStarted);
    assert_eq!(onboarding.step(), Step::Welcome);
    assert_eq!(onboarding.chosen_shortcut(), None);
    assert!(onboarding.verified_launch().is_none());
    assert!(onboarding.skipped_steps().is_empty());
    assert_eq!(onboarding.completion_count(), 0);
    assert!(!onboarding.has_completed_before());

    let reloaded = Onboarding::load(store);
    assert_eq!(reloaded.status(), OnboardingStatus::NotStarted);
    assert_eq!(reloaded.step(), Step::Welcome);
    assert_eq!(reloaded.chosen_shortcut(), None);
    assert!(reloaded.verified_launch().is_none());
}

#[test]
fn corrupt_json_falls_back_to_a_pristine_profile_without_touching_the_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("onboarding.json");
    fs::write(&path, "{ this is not valid json").unwrap();

    let onboarding = Onboarding::load(JsonFileStore::new(&path));
    assert_eq!(onboarding.status(), OnboardingStatus::NotStarted);
    assert_eq!(onboarding.step(), Step::Welcome);
    assert!(onboarding.load_warning().is_some());
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "{ this is not valid json"
    );
}

#[test]
fn unsupported_state_version_falls_back_safely() {
    let store = MemoryStore::new();
    store
        .save(r#"{"version":99,"status":"completed","step":"done"}"#)
        .unwrap();

    let onboarding = Onboarding::load(store);
    assert_eq!(onboarding.status(), OnboardingStatus::NotStarted);
    assert_eq!(onboarding.step(), Step::Welcome);
    assert!(
        onboarding
            .load_warning()
            .unwrap()
            .contains("unsupported onboarding state version")
    );
}

#[test]
fn in_progress_state_survives_a_restart() {
    let store = MemoryStore::new();
    let mut onboarding = Onboarding::load(store.clone());
    reach_shortcut(&mut onboarding);
    onboarding
        .select_shortcut(&Shortcut::documented_default(), &available_shortcut())
        .unwrap();
    drop(onboarding);

    let reloaded = Onboarding::load(store);
    assert_eq!(reloaded.status(), OnboardingStatus::InProgress);
    assert_eq!(reloaded.step(), Step::Shortcut);
    assert!(reloaded.shortcut_registered());
    assert_eq!(
        reloaded.chosen_shortcut(),
        Some(Shortcut::documented_default())
    );
}

#[test]
fn restart_from_settings_preserves_completion_history() {
    let store = MemoryStore::new();
    let mut onboarding = Onboarding::load(store.clone());
    complete_happy_path(&mut onboarding);
    assert_eq!(onboarding.completion_count(), 1);

    onboarding.restart_from_settings(Step::Shortcut).unwrap();
    assert_eq!(onboarding.step(), Step::Shortcut);
    assert_eq!(onboarding.status(), OnboardingStatus::InProgress);
    assert!(onboarding.is_revisit());
    assert!(onboarding.has_completed_before());
    assert_eq!(onboarding.completion_count(), 1);
    assert_eq!(
        onboarding.chosen_shortcut(),
        Some(Shortcut::documented_default())
    );
    assert!(onboarding.verified_launch().is_some());

    onboarding.next().unwrap();
    onboarding.next().unwrap();
    onboarding.next().unwrap();
    assert_eq!(onboarding.step(), Step::Done);
    onboarding.complete().unwrap();

    assert_eq!(onboarding.status(), OnboardingStatus::Completed);
    assert!(!onboarding.is_revisit());
    assert_eq!(onboarding.completion_count(), 2);

    let reloaded = Onboarding::load(store);
    assert_eq!(reloaded.status(), OnboardingStatus::Completed);
    assert_eq!(reloaded.completion_count(), 2);
}

#[test]
fn navigation_guards_cover_both_ends_and_completion() {
    let mut onboarding = Onboarding::load(MemoryStore::new());
    assert_eq!(onboarding.back(), Err(FlowError::AtStart));
    assert_eq!(onboarding.complete(), Err(FlowError::NotAtDone));

    onboarding.next().unwrap();
    assert_eq!(onboarding.back().unwrap(), ());
    assert_eq!(onboarding.step(), Step::Welcome);

    complete_happy_path(&mut onboarding);
    assert_eq!(onboarding.next(), Err(FlowError::AtEnd));
    assert_eq!(onboarding.back().unwrap(), ());
    assert_eq!(onboarding.step(), Step::Integrations);
    assert_eq!(onboarding.status(), OnboardingStatus::InProgress);
    assert_eq!(onboarding.completion_count(), 1);
}

#[test]
fn capability_notices_render_unsupported_and_degraded_states() {
    let notices = vec![
        CapabilityNotice::new("Launching", CapabilityStatus::Available),
        CapabilityNotice::new(
            "Clipboard history",
            CapabilityStatus::Unsupported {
                reason: "This session has no clipboard listener".to_owned(),
            },
        ),
        CapabilityNotice::new(
            "Window switching",
            CapabilityStatus::Degraded {
                reason: "Foreground focus can be refused".to_owned(),
                fallback: "Copy the window title instead".to_owned(),
            },
        ),
        CapabilityNotice::new("Screen capture", CapabilityStatus::TemporarilyUnavailable),
    ];

    let onboarding = Onboarding::load(MemoryStore::new()).with_capability_notices(notices.clone());

    assert_eq!(onboarding.capability_notices().len(), 4);
    assert_eq!(onboarding.notices_needing_attention().len(), 3);

    let rendered = onboarding.rendered_capability_notices();
    assert_eq!(rendered.len(), 4);
    assert!(rendered[1].contains("This session has no clipboard listener"));
    assert!(rendered[2].contains("Foreground focus can be refused"));
    assert!(rendered[2].contains("Fallback: Copy the window title instead"));
    assert!(rendered[3].contains("Temporarily unavailable"));
    assert!(rendered[0].contains("available"));
}
