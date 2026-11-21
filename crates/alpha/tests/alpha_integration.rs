//! End-to-end integration coverage for the alpha composition root.
//!
//! Every test drives the real prerequisite crates. Storage tests use a real
//! SQLite database in a temporary directory; the harness uses the real engine
//! and coordinator. No test launches an application or touches the network.

use std::collections::BTreeMap;
use std::time::Duration;

use ansa_alpha::{
    AlphaError, CALCULATOR_CORPUS_VERSION, DailyDriver, RANKING_CORPUS_VERSION, alpha_smoke,
    compose_catalog, emoji_info, search_catalog, search_emoji,
};
use ansa_core::calculator::CalcResult;
use ansa_core::query::{QueryRoute, route_query};
use ansa_domain::{Command, CommandId, CommandKind};
use ansa_emoji::{ActivationOutcome, CopyFallbackReason, activation, is_single_grapheme};
use ansa_onboarding::{
    JsonFileStore, LaunchOutcome, LaunchProbe, Onboarding, ProbeOutcome, Shortcut, ShortcutProbe,
    Step,
};
use ansa_platform_windows::AppEntry;
use ansa_quicklinks::{
    ArgKind, ArgSpec, Encoding, ImportOptions, Library, OpenPolicy, Quicklink, QuicklinkId,
    StaticOpenerProbe, Template,
};
use ansa_registry::{
    ActionError, ActionInput, CommandKind as RegistryKind, CommandSpec, Confirmation,
    ExecutionRequest, GrantSnapshot, Host, HostReconcile, HostReport, ReconcileOutcome,
    ReconcileRequest, Registry, TargetProbe, capability_names,
};
use ansa_storage::{SettingKey, SettingValue, StorageError};

fn data_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("temporary directory")
}

fn query_template() -> Template {
    Template::parse(
        "https://example.test/search?q={query}",
        vec![ArgSpec::new(
            "query",
            ArgKind::Text,
            Encoding::UrlQueryValue,
        )],
    )
    .expect("valid template")
}

fn search_link() -> Quicklink {
    Quicklink::new(
        QuicklinkId::from_slug("search").expect("valid slug"),
        "Search",
        query_template(),
    )
    .with_alias("s")
}

fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

#[test]
fn storage_settings_cas_backup_and_restore_round_trip() {
    let directory = data_dir();
    let mut driver = DailyDriver::open(directory.path()).expect("open driver");
    assert!(driver.schema_version() >= 1);
    assert!(driver.quicklink_warning().is_none());

    let revision = driver
        .set_setting(
            SettingKey::MaximumResults,
            &SettingValue::MaximumResults(42),
            None,
        )
        .expect("create setting");
    let snapshot = driver
        .get_setting(SettingKey::MaximumResults)
        .expect("read setting")
        .expect("setting exists");
    assert_eq!(snapshot.value, SettingValue::MaximumResults(42));
    assert_eq!(snapshot.revision, Some(revision));

    let conflict = driver
        .set_setting(
            SettingKey::MaximumResults,
            &SettingValue::MaximumResults(7),
            Some(revision + 1),
        )
        .expect_err("stale revision must be rejected");
    assert!(matches!(
        conflict,
        AlphaError::Storage(StorageError::Conflict { .. })
    ));
    assert_eq!(
        driver
            .get_setting(SettingKey::MaximumResults)
            .unwrap()
            .unwrap()
            .value,
        SettingValue::MaximumResults(42)
    );

    let pragma = driver.pragma_report().expect("pragma report");
    assert_eq!(pragma.foreign_keys, 1);
    assert_eq!(pragma.journal_mode, "wal");
    assert_eq!(pragma.synchronous, 2);

    let backup_path = directory.path().join("backup.sqlite");
    let backup = driver.backup_to(&backup_path).expect("backup");
    assert_eq!(backup.integrity, "ok");
    assert_eq!(backup.schema_version, driver.schema_version());

    let newer = driver
        .set_setting(
            SettingKey::MaximumResults,
            &SettingValue::MaximumResults(7),
            Some(revision),
        )
        .expect("replace setting");
    assert!(newer > revision);

    let restored = driver.restore_from(&backup_path).expect("restore");
    assert_eq!(restored.integrity, "ok");
    assert_eq!(
        driver
            .get_setting(SettingKey::MaximumResults)
            .unwrap()
            .unwrap()
            .value,
        SettingValue::MaximumResults(42)
    );
}

#[test]
fn quicklinks_preview_commit_persist_import_and_export() {
    let directory = data_dir();
    let mut driver = DailyDriver::open(directory.path()).expect("open driver");
    let link = search_link();
    let preview = driver
        .preview_quicklink(
            &link,
            &values(&[("query", "rust lang")]),
            OpenPolicy::web_only(),
            &StaticOpenerProbe::all_available(),
        )
        .expect("preview");
    assert_eq!(
        preview.target(),
        "https://example.test/search?q=rust%20lang"
    );
    assert!(preview.effect().contains(preview.target()));

    driver.commit_quicklink(&preview).expect("commit");
    assert_eq!(driver.quicklinks().len(), 1);
    assert_eq!(driver.quicklink_record_revision(), Some(1));

    let exported = driver.export_quicklinks().expect("export");
    assert!(exported.contains("\"schema_version\": 1"));
    assert!(exported.contains("\"title\": \"Search\""));

    drop(driver);
    let mut reopened = DailyDriver::open(directory.path()).expect("reopen driver");
    assert_eq!(reopened.quicklinks().len(), 1);
    assert!(reopened.quicklink_warning().is_none());

    let replaced = reopened
        .import_quicklinks(&exported, ImportOptions::replace_confirmed())
        .expect("replace import");
    assert!(replaced.replaced);
    assert_eq!(replaced.applied, 1);

    let mut document: serde_json::Value =
        serde_json::from_str(&exported).expect("valid export JSON");
    let duplicate = document["quicklinks"][0].clone();
    document["quicklinks"]
        .as_array_mut()
        .expect("quicklinks array")
        .push(duplicate);
    let report = reopened
        .import_quicklinks(&document.to_string(), ImportOptions::merge())
        .expect("merge import");
    assert!(report.has_errors(), "duplicate identity must be reported");
    assert_eq!(report.total, 2);

    let composition = compose_catalog(
        reopened.quicklinks(),
        &[AppEntry::new(
            "app:notepad",
            "Notepad",
            "C:\\Windows\\notepad.exe",
        )],
    )
    .expect("compose catalog");
    assert!(
        composition
            .commands
            .iter()
            .all(|command| !matches!(command.kind, CommandKind::Quicklink { .. })),
        "a required-argument quicklink must not become a launch command"
    );
    assert!(
        composition
            .commands
            .iter()
            .any(|command| matches!(command.kind, CommandKind::Application { .. }))
    );
    assert_eq!(composition.skipped_quicklinks.len(), 1);
    assert_eq!(
        composition.skipped_quicklinks[0].reason,
        ansa_alpha::QuicklinkSkipReason::RequiredArguments
    );
}

#[test]
fn emoji_search_is_grapheme_safe_and_activation_is_truthful() {
    let hits = search_emoji("rocket", 5).expect("emoji search");
    assert!(!hits.is_empty());
    assert!(
        hits.iter().all(|hit| is_single_grapheme(&hit.grapheme)),
        "every base must be one extended grapheme"
    );
    assert!(
        hits.iter()
            .all(|hit| hit.variants.iter().all(|(g, _)| is_single_grapheme(g)))
    );
    assert!(
        hits.iter()
            .any(|hit| hit.name.to_lowercase().contains("rocket"))
    );

    let info = emoji_info().expect("dataset info");
    assert_eq!(info.version, "emoji-1.0.0");
    assert_eq!(info.unicode_version, "16.0");
    assert_eq!(info.license, "Unicode License v3");
    assert_eq!(info.checksum.len(), 16);
    assert!(info.entry_count >= 100);

    assert_eq!(activation(true, true), ActivationOutcome::Inserted);
    assert_eq!(
        activation(true, false),
        ActivationOutcome::CopyFallback {
            reason: CopyFallbackReason::TargetUnverified
        }
    );
    assert_eq!(activation(false, false), ActivationOutcome::Copied);
}

#[test]
fn onboarding_fresh_profile_reaches_verified_launch_and_persists() {
    struct AvailableShortcut;
    impl ShortcutProbe for AvailableShortcut {
        fn probe(&self, _shortcut: &Shortcut) -> ProbeOutcome {
            ProbeOutcome::Available
        }
    }

    struct ConfirmedLaunch;
    impl LaunchProbe for ConfirmedLaunch {
        fn launch(&self, _command_id: &CommandId) -> LaunchOutcome {
            LaunchOutcome::Launched
        }
    }

    let directory = data_dir();
    let path = directory.path().join("onboarding.json");
    let mut onboarding = Onboarding::fresh(JsonFileStore::new(&path));
    assert_eq!(onboarding.status().label(), "not-started");

    onboarding.next().expect("welcome to shortcut");
    assert_eq!(onboarding.step(), Step::Shortcut);
    let probe = onboarding
        .select_shortcut(&Shortcut::documented_default(), &AvailableShortcut)
        .expect("select shortcut");
    assert_eq!(probe, ProbeOutcome::Available);
    assert!(onboarding.shortcut_registered());

    onboarding.next().expect("shortcut to launch");
    assert_eq!(onboarding.step(), Step::TryLaunch);
    let command = CommandId::parse("app:demo").expect("command id");
    let outcome = onboarding
        .try_launch(&command, &ConfirmedLaunch)
        .expect("launch");
    assert_eq!(outcome, LaunchOutcome::Launched);
    assert_eq!(
        onboarding
            .verified_launch()
            .expect("verified launch")
            .command_id(),
        &command
    );

    onboarding.next().expect("launch to integrations");
    onboarding.next().expect("integrations to done");
    assert_eq!(onboarding.step(), Step::Done);
    onboarding.complete().expect("complete");
    assert!(onboarding.status().is_completed());
    assert!(path.exists());

    let reloaded = Onboarding::load(JsonFileStore::new(&path));
    assert!(reloaded.status().is_completed());
    assert_eq!(
        reloaded
            .verified_launch()
            .expect("persisted launch")
            .command_id(),
        &command
    );
}

#[test]
fn calculator_routing_and_unit_evaluation_are_local() {
    assert_eq!(route_query("7-Zip"), QueryRoute::Search);
    assert!(matches!(route_query("1Password"), QueryRoute::Search));

    let QueryRoute::Calculator(Ok(result)) = route_query("= 2*3") else {
        panic!("expected calculator result");
    };
    assert_eq!(result, CalcResult::Number(6.0));
    assert_eq!(result.format(), "6");

    let QueryRoute::Calculator(Ok(result)) = route_query("(24+8)/2") else {
        panic!("expected calculator result");
    };
    assert_eq!(result, CalcResult::Number(16.0));
    assert_eq!(result.format(), "16");

    let QueryRoute::Calculator(Ok(result)) = route_query("2^3") else {
        panic!("expected implicit calculator result");
    };
    assert_eq!(result, CalcResult::Number(8.0));

    let QueryRoute::Calculator(Ok(result)) = route_query("=10 km to mi") else {
        panic!("expected unit result");
    };
    assert!(matches!(result, CalcResult::Quantity(_)));
    assert_eq!(result.format(), "6.21371192237 mi");

    assert!(matches!(route_query("1+"), QueryRoute::Calculator(Err(_))));

    let catalog = vec![Command::new(
        CommandId::parse("app:7zip").expect("id"),
        "7-Zip",
        CommandKind::Application {
            path: "C:\\7-Zip\\7zFM.exe".to_owned(),
        },
    )];
    let search = search_catalog(&catalog, "7-Zip", 5).expect("search");
    assert_eq!(search.kind, ansa_alpha::QueryKind::Search);
    assert!(!search.commands.is_empty());

    let calculator = search_catalog(&catalog, "=2*3", 5).expect("calculator");
    assert_eq!(calculator.kind, ansa_alpha::QueryKind::Calculator);
    assert!(calculator.commands.is_empty());
    assert_eq!(calculator.calculator, Some(Ok("6".to_owned())));
    assert!(calculator.emoji.is_empty());

    let unit = search_catalog(&catalog, "=10 km to mi", 5).expect("unit calculator");
    assert_eq!(
        unit.calculator,
        Some(Ok("6.21371192237 mi".to_owned())),
        "unit results must survive calculator composition"
    );
}

/** Fake host for one registry lifecycle. */
struct FakeHost;

impl Host for FakeHost {
    fn probe_target(&self, _spec: &CommandSpec) -> TargetProbe {
        TargetProbe::Present {
            identity: "https://example.test/".to_owned(),
            revision: 1,
        }
    }

    fn perform(&mut self, _request: ExecutionRequest) -> HostReport {
        HostReport::Completed
    }

    fn reconcile(&mut self, _request: ReconcileRequest) -> HostReconcile {
        HostReconcile::Completed
    }
}

#[test]
fn registry_executes_through_a_fake_host_and_refuses_replay() {
    let id = CommandId::parse("quicklink:example").expect("id");
    let mut registry = Registry::new();
    registry
        .register(CommandSpec::new(
            id.clone(),
            "Example",
            RegistryKind::Quicklink {
                url: "https://example.test/".to_owned(),
            },
        ))
        .expect("register");

    let mut grants = GrantSnapshot::none();
    grants.grant(capability_names::OPEN_URL);
    let mut host = FakeHost;
    let prepared = registry
        .prepare(&id, &ActionInput::new(), &host)
        .expect("prepare");
    assert!(
        prepared
            .effect()
            .summary()
            .contains("https://example.test/")
    );
    let denied = registry
        .authorize(&prepared, &GrantSnapshot::none(), Confirmation::Confirmed)
        .expect_err("an ungranted quicklink must not authorize");
    assert!(matches!(denied, ActionError::DeniedPermission { .. }));
    // Quicklink policy is hardened; always confirm explicitly rather than
    // depending on whether this kind is currently classified as consequential.
    let authorization = registry
        .authorize(&prepared, &grants, Confirmation::Confirmed)
        .expect("authorize");
    let outcome = registry
        .execute(&id, &authorization, &grants, &mut host)
        .expect("execute");
    assert!(outcome.is_completed());

    let replay = registry
        .execute(&id, &authorization, &grants, &mut host)
        .expect_err("replay must be refused");
    assert!(matches!(replay, ActionError::Conflict { .. }));
}

/** Host that reports an indeterminate first effect and confirms it on reconcile. */
struct UncertainHost;

impl Host for UncertainHost {
    fn probe_target(&self, _spec: &CommandSpec) -> TargetProbe {
        TargetProbe::Present {
            identity: "C:\\App.exe".to_owned(),
            revision: 3,
        }
    }

    fn perform(&mut self, _request: ExecutionRequest) -> HostReport {
        HostReport::Uncertain {
            reason: "the process did not report a start event".to_owned(),
        }
    }

    fn reconcile(&mut self, _request: ReconcileRequest) -> HostReconcile {
        HostReconcile::Completed
    }
}

#[test]
fn registry_reconciles_an_uncertain_outcome_instead_of_retrying() {
    let id = CommandId::parse("app:uncertain").expect("id");
    let mut registry = Registry::new();
    registry
        .register(CommandSpec::new(
            id.clone(),
            "Uncertain App",
            RegistryKind::Application {
                path: "C:\\App.exe".to_owned(),
            },
        ))
        .expect("register");

    let mut grants = GrantSnapshot::none();
    grants.grant(capability_names::LAUNCH_APPLICATION);
    let mut host = UncertainHost;
    let prepared = registry
        .prepare(&id, &ActionInput::new(), &host)
        .expect("prepare");
    let authorization = registry
        .authorize(&prepared, &grants, Confirmation::Confirmed)
        .expect("authorize");
    let first = registry
        .execute(&id, &authorization, &grants, &mut host)
        .expect("execute");
    assert!(first.is_uncertain());

    let retry = registry
        .execute(&id, &authorization, &grants, &mut host)
        .expect_err("an uncertain action must not be retried blindly");
    assert!(matches!(retry, ActionError::Uncertain { .. }));

    assert_eq!(
        registry.reconcile(first.operation_id(), &mut host),
        ReconcileOutcome::Completed
    );
}

#[test]
fn one_thousand_cycle_harness_is_deterministic_and_bounded() {
    let report = alpha_smoke(1_000).expect("harness");
    assert_eq!(report.cycles, 1_000);
    assert_eq!(report.stable_selection_checks, 1_000);
    assert!(report.accepted_batches >= 1_000);
    assert!(report.rejected_batches >= 3_000);
    assert_eq!(report.max_pending, 1);
    assert!(report.max_results <= report.catalog_size);
    assert!(report.assertions >= 9_000);
    assert!(
        report.elapsed < Duration::from_secs(30),
        "debug harness took {:?}",
        report.elapsed
    );

    let short_a = alpha_smoke(50).expect("short harness");
    let short_b = alpha_smoke(50).expect("short harness");
    assert_eq!(short_a.fingerprint, short_b.fingerprint);
    assert_eq!(short_a.accepted_batches, short_b.accepted_batches);
    assert_eq!(short_a.rejected_batches, short_b.rejected_batches);
    assert_eq!(short_a.assertions, short_b.assertions);
}

#[test]
fn corpus_version_mirrors_match_the_shipped_files() {
    let relevance = include_str!("../../search/data/relevance_corpus.json");
    assert!(
        relevance.contains(&format!("\"corpus_version\": \"{RANKING_CORPUS_VERSION}\"")),
        "alpha ranking corpus mirror drifted"
    );
    let calculator = include_str!("../../core/data/calculator_corpus.json");
    assert!(
        calculator.contains(&format!("\"version\": \"{CALCULATOR_CORPUS_VERSION}\"")),
        "alpha calculator corpus mirror drifted"
    );
}

#[test]
fn library_preview_is_required_before_commit() {
    let mut library = Library::new();
    let link = search_link();
    let preview = library
        .preview(
            &link,
            &values(&[("query", "x")]),
            OpenPolicy::web_only(),
            &StaticOpenerProbe::all_available(),
        )
        .expect("preview");
    library.commit(&preview).expect("commit");
    assert!(library.commit(&preview).is_err(), "stale preview must fail");
}
