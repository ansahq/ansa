//! Black-box CLI tests driven through the real `ansa` binary.

use std::fs;
use std::process::{Command, Output};

fn ansa(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ansa"))
        .args(args)
        .output()
        .expect("run ansa binary")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn help_lists_alpha_commands() {
    let output = ansa(&["--help"]);
    assert!(output.status.success(), "help exited nonzero");
    let text = stdout(&output);
    for command in [
        "search",
        "apps",
        "calc",
        "open",
        "doctor",
        "emoji",
        "quicklink",
        "storage",
        "onboard",
        "alpha-smoke",
    ] {
        assert!(text.contains(command), "help is missing {command}");
    }
}

#[test]
fn calc_keeps_local_arithmetic_behavior() {
    let output = ansa(&["calc", "2+3*4"]);
    assert!(output.status.success());
    assert_eq!(stdout(&output).trim(), "14");
}

#[test]
fn calc_preserves_bounded_unit_results() {
    let output = ansa(&["calc", "10 km to mi"]);
    assert!(output.status.success());
    assert_eq!(stdout(&output).trim(), "6.21371192237 mi");

    let explicit = ansa(&["calc", "=20 C to F"]);
    assert!(explicit.status.success());
    assert_eq!(stdout(&explicit).trim(), "68 F");
}

#[test]
fn emoji_prints_grapheme_dataset_and_license() {
    let output = ansa(&["emoji", "rocket", "--limit", "3"]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.to_lowercase().contains("rocket"));
    assert!(text.contains("emoji-1.0.0"));
    assert!(text.contains("Unicode License v3"));
    assert!(text.contains("checksum"));
    assert!(text.contains("result(s)"));
}

#[test]
fn doctor_reports_platform_and_is_offline() {
    let output = ansa(&["doctor"]);
    assert!(output.status.success(), "doctor exited nonzero");
    let text = stdout(&output);
    assert!(text.contains("Ansa doctor"));
    assert!(text.contains("platform:"));
    assert!(text.contains("emoji dataset:"));
    assert!(text.contains("ranking corpus:"));
    assert!(text.contains("No network access was used."));
}

#[test]
fn doctor_with_data_dir_reports_quicklink_launchability() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let data_dir = directory.path().to_str().expect("utf-8 path");
    let output = ansa(&["doctor", "--data-dir", data_dir]);
    assert!(output.status.success(), "doctor exited nonzero");
    let text = stdout(&output);
    assert!(text.contains("schema v"));
    assert!(text.contains("quicklinks: 0 persisted · 0 launchable · 0 omitted"));
}

#[test]
fn data_dir_lock_is_reported_actionably() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let data_dir = directory.path().join("data");
    let data_arg = data_dir.to_str().expect("utf-8 path");
    let held = ansa_alpha::DailyDriver::open(&data_dir).expect("hold the durable store");

    for args in [
        vec!["quicklink", "list", "--data-dir", data_arg],
        vec!["doctor", "--data-dir", data_arg],
        vec!["storage", "--data-dir", data_arg],
    ] {
        let output = ansa(&args);
        assert!(!output.status.success(), "{args:?} unexpectedly succeeded");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(
                "Ansa is running and holds the data directory; close it before importing/exporting quicklinks or running doctor."
            ),
            "{args:?} did not explain the lock: {stderr}"
        );
    }

    drop(held);
}

#[test]
fn quicklink_library_file_mode_still_works() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let seed = directory.path().join("seed.json");
    fs::write(
        &seed,
        r#"{"schema_version":1,"quicklinks":[{"id":"quicklink:00000000000000000000000000000002","title":"Portable","alias":null,"tags":[],"template":{"source":"https://example.test/portable","arguments":[]},"opener":"browser","enabled":true}]}"#,
    )
    .expect("write seed");
    let library = directory.path().join("library.json");
    let library_arg = library.to_str().expect("utf-8 path");

    let import = ansa(&[
        "quicklink",
        "import",
        seed.to_str().expect("utf-8 path"),
        "--library",
        library_arg,
    ]);
    assert!(import.status.success());
    assert!(stdout(&import).contains("Wrote 1 quicklinks"));

    let list = ansa(&["quicklink", "list", "--library", library_arg]);
    assert!(list.status.success());
    let text = stdout(&list);
    assert!(text.contains("Portable"));
    assert!(text.contains("https://example.test/portable"));
}

#[test]
fn quicklink_data_dir_import_is_persisted_and_visible_to_doctor() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let seed = directory.path().join("seed.json");
    fs::write(
        &seed,
        r#"{"schema_version":1,"quicklinks":[{"id":"quicklink:00000000000000000000000000000001","title":"Docs","alias":"docs","tags":[],"template":{"source":"https://example.test/docs","arguments":[]},"opener":"browser","enabled":true}]}"#,
    )
    .expect("write seed");
    let data_dir = directory.path().join("data");
    let data_arg = data_dir.to_str().expect("utf-8 path");
    let seed_arg = seed.to_str().expect("utf-8 path");

    let import = ansa(&["quicklink", "import", seed_arg, "--data-dir", data_arg]);
    assert!(import.status.success(), "data-dir import failed");
    let import_text = stdout(&import);
    assert!(import_text.contains("applied 1 of 1 quicklinks"));
    assert!(import_text.contains("Persisted 1 quicklinks"));

    let list = ansa(&["quicklink", "list", "--data-dir", data_arg]);
    assert!(list.status.success());
    let list_text = stdout(&list);
    assert!(list_text.contains("Docs"));
    assert!(list_text.contains("https://example.test/docs"));
    assert!(list_text.contains("1 quicklinks"));

    let preview = ansa(&[
        "quicklink",
        "preview",
        "quicklink:00000000000000000000000000000001",
        "--data-dir",
        data_arg,
    ]);
    assert!(preview.status.success());
    assert!(stdout(&preview).contains("target: https://example.test/docs"));

    let doctor = ansa(&["doctor", "--data-dir", data_arg]);
    assert!(doctor.status.success());
    let doctor_text = stdout(&doctor);
    assert!(
        doctor_text.contains("quicklinks: 1 persisted · 1 launchable · 0 omitted"),
        "doctor must see the persisted quicklink: {doctor_text}"
    );

    let export = ansa(&["quicklink", "export", "--data-dir", data_arg]);
    assert!(export.status.success());
    let export_text = stdout(&export);
    assert!(export_text.contains("\"schema_version\": 1"));
    assert!(export_text.contains("https://example.test/docs"));

    let mixed = ansa(&[
        "quicklink",
        "list",
        "--library",
        seed_arg,
        "--data-dir",
        data_arg,
    ]);
    assert!(!mixed.status.success());
    assert!(
        String::from_utf8_lossy(&mixed.stderr).contains("not both"),
        "mixing --library and --data-dir must be rejected"
    );

    // The desktop path reads the same SQLite record through DailyDriver.
    let driver = ansa_alpha::DailyDriver::open(&data_dir).expect("open driver");
    assert_eq!(driver.quicklinks().len(), 1);
    let composition = driver.compose_catalog(&[]).expect("compose catalog");
    assert_eq!(composition.launchable_quicklinks(), 1);
    let command = composition
        .commands
        .iter()
        .find(|command| matches!(command.kind, ansa_domain::CommandKind::Quicklink { .. }))
        .expect("launchable quicklink command");
    match &command.kind {
        ansa_domain::CommandKind::Quicklink { url } => {
            assert_eq!(url, "https://example.test/docs");
        }
        other => panic!("expected quicklink command, got {other:?}"),
    }
}

#[test]
fn alpha_smoke_short_run_passes() {
    let output = ansa(&["alpha-smoke", "--cycles", "50"]);
    assert!(output.status.success(), "alpha-smoke exited nonzero");
    let text = stdout(&output);
    assert!(text.contains("Alpha smoke: PASS"));
    assert!(text.contains("cycles: 50"));
    assert!(text.contains("rejected batches:"));
    assert!(text.contains("fingerprint:"));
}
