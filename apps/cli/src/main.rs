use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use ansa_alpha::{
    AlphaError, DEFAULT_SMOKE_CYCLES, DailyDriver, alpha_smoke, emoji_info, search_emoji,
};
use ansa_core::{Engine, calculator};
use ansa_domain::{Command, CommandId, CommandKind};
use ansa_onboarding::{
    JsonFileStore, LaunchOutcome, LaunchProbe, Onboarding, ProbeOutcome, Shortcut, ShortcutProbe,
};
use ansa_platform_windows::LaunchOutcome as AppLaunchOutcome;
use ansa_quicklinks::{
    ImportOptions, ImportReport, Library, OpenPolicy, Preview, Quicklink, QuicklinkId,
    StaticOpenerProbe,
};
use ansa_storage::{SettingKey, SettingValue};

const HELP: &str = "Ansa — local-first Rust launcher

  ansa search <query>             Search Windows Start Menu applications
  ansa apps                       List discovered applications
  ansa calc <expression>          Evaluate local arithmetic and bounded units
  ansa open <stable-id>           Launch or switch to an application truthfully
  ansa doctor [--data-dir <dir>]  Report platform, catalog, storage and dataset status
  ansa emoji <query> [--limit N]  Search the licensed local emoji dataset
  ansa quicklink list    (--library <file> | --data-dir <dir>)
  ansa quicklink preview <id> (--library <file> | --data-dir <dir>) [--arg name=value]...
  ansa quicklink import <file> (--library <file> | --data-dir <dir>) [--replace]
  ansa quicklink export (--library <file> | --data-dir <dir>) [--out <path>]
    --library reads/writes a portable versioned JSON file.
    --data-dir reads/writes the persisted SQLite quicklink record used by the desktop.
    Close a running Ansa before using --data-dir; the durable store is single-instance locked.
  ansa storage [--data-dir <dir>] [--demo]
  ansa onboard --state <path> [--demo]
  ansa alpha-smoke [--cycles N]   Run the deterministic 1,000-cycle harness

Desktop: cargo run -p ansa-desktop
No IPC show command exists yet; use the desktop's Ctrl+Space shortcut.";

const DATA_DIR_LOCKED: &str = "Ansa is running and holds the data directory; close it before importing/exporting quicklinks or running doctor.";

/// Opens the durable alpha store, translating the single-instance lock into an
/// actionable message instead of a raw storage error.
fn open_driver(directory: impl Into<PathBuf>) -> Result<DailyDriver, String> {
    DailyDriver::open(directory).map_err(|error| match error {
        AlphaError::Storage(ansa_storage::StorageError::AlreadyLocked { .. }) => {
            DATA_DIR_LOCKED.to_owned()
        }
        other => other.to_string(),
    })
}

fn run(args: &[String]) -> Result<bool, String> {
    match args.first().map(String::as_str) {
        None | Some("help" | "--help" | "-h") => {
            println!("{HELP}");
            Ok(true)
        }
        Some("calc") => {
            let expression = args[1..].join(" ");
            let expression = expression.trim();
            let expression = expression
                .strip_prefix('=')
                .map(str::trim_start)
                .unwrap_or(expression);
            let value =
                calculator::evaluate_extended(expression).map_err(|error| error.to_string())?;
            println!("{}", value.format());
            Ok(true)
        }
        Some("search" | "apps" | "open") => run_catalog(&args[0], args),
        Some("doctor") => run_doctor(args),
        Some("emoji") => run_emoji(args),
        Some("quicklink") => run_quicklink(args),
        Some("storage") => run_storage(args),
        Some("onboard") => run_onboard(args),
        Some("alpha-smoke") => run_alpha_smoke(args),
        Some(other) => Err(format!("Unknown command: {other}. Run `ansa --help`.")),
    }
}

fn run_catalog(command: &str, args: &[String]) -> Result<bool, String> {
    let apps = ansa_platform_windows::discover_applications()?;
    match command {
        "open" => {
            if args.len() != 2 {
                return Err("Usage: ansa open <stable-id>".into());
            }
            let app = apps
                .iter()
                .find(|app| app.id == args[1])
                .ok_or("Application not found; refresh the catalog with `ansa apps`")?;
            let outcome = ansa_platform_windows::launch_or_switch(&app.path);
            match &outcome {
                AppLaunchOutcome::Launched => println!("Launched: {}", app.title),
                AppLaunchOutcome::Switched { window } => {
                    println!("Switched to {} (window 0x{window:x})", app.title);
                }
                AppLaunchOutcome::Denied { reason } => {
                    println!("Denied: {} — {reason}", app.title);
                }
                AppLaunchOutcome::Failed { reason } => {
                    println!("Failed: {} — {reason}", app.title);
                }
                AppLaunchOutcome::Unverified { reason } => {
                    println!("Unverified: {} — {reason}", app.title);
                }
                AppLaunchOutcome::NoTarget { reason } => {
                    println!("NoTarget: {} — {reason}", app.title);
                }
            }
            Ok(outcome.is_success())
        }
        "apps" => {
            for app in &apps {
                println!("{}\t{}\t{}", app.id, app.title, app.path);
            }
            println!("{} applications", apps.len());
            Ok(true)
        }
        _ => {
            let commands = apps
                .into_iter()
                .map(|app| {
                    Ok(Command {
                        id: CommandId::new(&app.id)
                            .map_err(|error| format!("Invalid app identity: {error:?}"))?,
                        title: app.title,
                        subtitle: Some(app.path.clone()),
                        keywords: vec![],
                        alias: None,
                        kind: CommandKind::Application { path: app.path },
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let mut engine = Engine::new(commands);
            engine.set_query(&args[1..].join(" "));
            for command in engine.results().iter().take(100) {
                println!(
                    "{}\t{}\t{}",
                    command.id.as_str(),
                    command.title,
                    command.subtitle.as_deref().unwrap_or_default()
                );
            }
            println!(
                "{} matches (display limited to 100)",
                engine.results().len()
            );
            Ok(true)
        }
    }
}

fn run_doctor(args: &[String]) -> Result<bool, String> {
    let data_dir = option_value(args, "--data-dir");
    let capabilities = ansa_platform_windows::host_capabilities();
    println!("Ansa doctor");
    println!(
        "platform: {} (adapter {})",
        capabilities.platform, capabilities.adapter
    );

    let discovered = ansa_platform_windows::discover_applications();
    match &discovered {
        Ok(apps) => {
            let packaged = apps.iter().filter(|app| app.packaged).count();
            let broken = apps.iter().filter(|app| app.is_broken()).count();
            let classic = apps.len().saturating_sub(packaged);
            println!(
                "applications: {} total · {} classic · {} packaged · {} broken",
                apps.len(),
                classic,
                packaged,
                broken
            );
        }
        Err(error) => println!("applications: discovery failed — {error}"),
    }

    print_capability("catalog", &capabilities.application_catalog);
    print_capability("packaged apps", &capabilities.packaged_applications);
    print_capability("application launch", &capabilities.application_launch);
    print_capability("url open", &capabilities.url_open);
    print_capability("global hotkey", &capabilities.global_hotkey);
    println!("  hotkey probe only: no shortcut was registered by doctor");
    print_capability("clipboard capture", &capabilities.clipboard_capture);
    print_capability("window control", &capabilities.window_control);

    let info = emoji_info().map_err(|error| error.to_string())?;
    println!(
        "emoji dataset: {} · Unicode {} · {} entries · license {} · checksum {}",
        info.version, info.unicode_version, info.entry_count, info.license, info.checksum
    );
    println!(
        "ranking corpus: v{} (ansa-search) · calculator corpus v{} (ansa-core)",
        ansa_alpha::RANKING_CORPUS_VERSION,
        ansa_alpha::CALCULATOR_CORPUS_VERSION
    );
    println!("calculator trigger: {}", ansa_alpha::CALCULATOR_TRIGGER);

    match data_dir {
        Some(directory) => {
            let driver = open_driver(&directory)?;
            println!(
                "storage: {} · schema v{}",
                driver.main_database_path().display(),
                driver.schema_version()
            );
            let pragma = driver.pragma_report().map_err(|error| error.to_string())?;
            println!(
                "  pragmas: foreign_keys={} journal_mode={} synchronous={} busy_timeout={}ms",
                pragma.foreign_keys, pragma.journal_mode, pragma.synchronous, pragma.busy_timeout
            );
            if let Some(warning) = driver.quicklink_warning() {
                println!("  quicklink warning: {warning}");
            }
            let apps_for_catalog = discovered.as_ref().map(Vec::as_slice).unwrap_or(&[]);
            let composition = driver
                .compose_catalog(apps_for_catalog)
                .map_err(|error| error.to_string())?;
            println!(
                "  quicklinks: {} persisted · {} launchable · {} omitted from the catalog (record revision {:?})",
                driver.quicklinks().len(),
                composition.launchable_quicklinks(),
                composition.skipped_quicklinks.len(),
                driver.quicklink_record_revision()
            );
            for skipped in &composition.skipped_quicklinks {
                println!(
                    "    omitted {} ({}): {}",
                    skipped.id, skipped.title, skipped.reason
                );
            }
            match driver
                .onboarding_info()
                .map_err(|error| error.to_string())?
            {
                Some(info) => println!(
                    "  onboarding: {} at step {:?} · completions {} · verified launch {:?}{}",
                    info.status.label(),
                    info.step,
                    info.completion_count,
                    info.verified_launch,
                    info.warning
                        .map(|warning| format!(" · warning: {warning}"))
                        .unwrap_or_default()
                ),
                None => println!("  onboarding: no state file"),
            }
        }
        None => println!("storage: skipped (pass --data-dir <dir>)"),
    }

    println!("No network access was used.");
    Ok(true)
}

fn print_capability(name: &str, status: &ansa_platform_windows::CapabilityStatus) {
    println!("{name}: {}", status.label());
    if let ansa_platform_windows::CapabilityStatus::Degraded {
        reason,
        alternative,
    } = status
    {
        println!("  reason: {reason}");
        println!("  alternative: {alternative}");
    }
}

fn run_emoji(args: &[String]) -> Result<bool, String> {
    let limit = match option_value(args, "--limit") {
        Some(raw) => raw
            .parse::<usize>()
            .map_err(|_| format!("--limit must be a positive integer, got {raw:?}"))?,
        None => 10,
    };
    let terms = positional_args(args, 1, &["--limit"]);
    let query = terms.join(" ");
    let hits = search_emoji(&query, limit).map_err(|error| error.to_string())?;
    let info = emoji_info().map_err(|error| error.to_string())?;

    println!(
        "emoji query {:?} · dataset {} · license {} · checksum {}",
        query, info.version, info.license, info.checksum
    );
    if hits.is_empty() {
        println!("no matches");
    }
    for hit in &hits {
        println!("{}  {}  [{}]", hit.grapheme, hit.name, hit.group);
        println!("  accessible: {}", hit.accessible_name);
        if !hit.variants.is_empty() {
            let variants = hit
                .variants
                .iter()
                .map(|(grapheme, name)| format!("{grapheme} {name}"))
                .collect::<Vec<_>>()
                .join(", ");
            println!("  variants: {variants}");
        }
    }
    println!("{} result(s)", hits.len());
    Ok(true)
}

/// Where `ansa quicklink` reads and writes quicklinks.
enum QuicklinkStore {
    /// Portable versioned JSON file, independent of the desktop storage record.
    JsonFile(PathBuf),
    /// Persisted `DailyDriver` storage record used by the desktop.
    Storage(Box<DailyDriver>),
}

impl QuicklinkStore {
    fn open(args: &[String]) -> Result<Self, String> {
        match (
            option_value(args, "--library"),
            option_value(args, "--data-dir"),
        ) {
            (Some(_), Some(_)) => Err(
                "Use either --library <file> (portable JSON) or --data-dir <dir> (persisted alpha storage), not both."
                    .into(),
            ),
            (Some(path), None) => Ok(Self::JsonFile(PathBuf::from(path))),
            (None, Some(directory)) => Ok(Self::Storage(Box::new(open_driver(directory)?))),
            (None, None) => Err(
                "quicklink commands require --library <file> (portable JSON) or --data-dir <dir> (persisted alpha storage)"
                    .into(),
            ),
        }
    }

    fn label(&self) -> String {
        match self {
            Self::JsonFile(path) => path.display().to_string(),
            Self::Storage(driver) => driver.data_dir().display().to_string(),
        }
    }

    fn with_library<T>(
        &self,
        operation: impl FnOnce(&Library) -> Result<T, String>,
    ) -> Result<T, String> {
        match self {
            Self::JsonFile(path) => {
                let library = load_library(path)?;
                operation(&library)
            }
            Self::Storage(driver) => operation(driver.quicklinks()),
        }
    }

    fn list(&self) -> Result<Vec<Quicklink>, String> {
        self.with_library(|library| Ok(library.list().into_iter().cloned().collect()))
    }

    fn revision(&self) -> Result<u64, String> {
        self.with_library(|library| Ok(library.revision()))
    }

    fn export_json(&self) -> Result<String, String> {
        self.with_library(|library| library.export_json().map_err(|error| error.to_string()))
    }

    fn preview(
        &self,
        id: &QuicklinkId,
        values: &BTreeMap<String, String>,
    ) -> Result<(String, Preview), String> {
        let label = self.label();
        self.with_library(|library| {
            let link = library
                .get(id)
                .ok_or_else(|| format!("quicklink {id} is not in {label}"))?;
            let preview = library
                .preview(
                    link,
                    values,
                    OpenPolicy::with_local_paths(),
                    &StaticOpenerProbe::all_available(),
                )
                .map_err(|error| error.to_string())?;
            Ok((link.title().to_owned(), preview))
        })
    }

    fn import(
        &mut self,
        contents: &str,
        options: ImportOptions,
    ) -> Result<(ImportReport, usize, String), String> {
        match self {
            Self::JsonFile(path) => {
                let mut library = load_library(path)?;
                let report = library.import_json(contents, options);
                if report.replaced || report.applied > 0 {
                    save_library(path, &library)?;
                }
                let detail = format!("Wrote {} quicklinks to {}", library.len(), path.display());
                Ok((report, library.len(), detail))
            }
            Self::Storage(driver) => {
                let report = driver
                    .import_quicklinks(contents, options)
                    .map_err(|error| error.to_string())?;
                let detail = format!(
                    "Persisted {} quicklinks in {} (record revision {:?})",
                    driver.quicklinks().len(),
                    driver.data_dir().display(),
                    driver.quicklink_record_revision()
                );
                Ok((report, driver.quicklinks().len(), detail))
            }
        }
    }
}

fn run_quicklink(args: &[String]) -> Result<bool, String> {
    let Some(subcommand) = args.get(1).map(String::as_str) else {
        return Err(
            "Usage: ansa quicklink <list|preview|import|export> (--library <path> | --data-dir <dir>)"
                .into(),
        );
    };
    let mut store = QuicklinkStore::open(args)?;

    match subcommand {
        "list" => {
            let links = store.list()?;
            let revision = store.revision()?;
            println!("{:<28} {:<24} {:<10} TARGET", "ID", "TITLE", "OPENER");
            for link in &links {
                println!(
                    "{:<28} {:<24} {:<10} {}",
                    link.id().as_str(),
                    link.title(),
                    link.opener().label(),
                    link.template().source()
                );
            }
            println!(
                "{} quicklinks · library revision {} · source {}",
                links.len(),
                revision,
                store.label()
            );
            if matches!(&store, QuicklinkStore::Storage(_)) {
                println!(
                    "Persisted records may include entries the catalog omits; run `ansa doctor --data-dir <dir>` for launchability reasons."
                );
            }
            Ok(true)
        }
        "preview" => {
            let terms = positional_args(args, 2, &["--library", "--data-dir", "--arg"]);
            let id_raw = terms.first().ok_or(
                "Usage: ansa quicklink preview <id> (--library <path> | --data-dir <dir>) [--arg name=value]...",
            )?;
            let id = QuicklinkId::parse(id_raw).map_err(|error| error.to_string())?;
            let mut values = BTreeMap::new();
            for pair in multi_values(args, "--arg") {
                let (name, value) = pair
                    .split_once('=')
                    .ok_or_else(|| format!("--arg expects name=value, got {pair:?}"))?;
                values.insert(name.to_owned(), value.to_owned());
            }
            let (title, preview) = store.preview(&id, &values)?;
            println!("id: {}", id);
            println!("title: {title}");
            println!("library: {}", store.label());
            println!("target: {}", preview.target());
            println!("opener: {}", preview.opener().label());
            println!("target kind: {}", preview.target_kind().label());
            println!("effect: {}", preview.effect());
            println!("fingerprint: {}", preview.fingerprint());
            println!("Note: the CLI uses a static opener probe and does not open anything.");
            Ok(true)
        }
        "import" => {
            let terms = positional_args(args, 2, &["--library", "--data-dir"]);
            let input_path = terms.first().ok_or(
                "Usage: ansa quicklink import <file> (--library <path> | --data-dir <dir>) [--replace]",
            )?;
            let replace = has_flag(args, "--replace");
            let contents = fs::read_to_string(input_path)
                .map_err(|error| format!("could not read {input_path}: {error}"))?;
            let options = if replace {
                ImportOptions::replace_confirmed()
            } else {
                ImportOptions::merge()
            };
            let (report, count, detail) = store.import(&contents, options)?;
            println!("{report}");
            for error in &report.errors {
                println!("  error: {error}");
            }
            if !replace && !report.errors.is_empty() {
                println!("  merge mode never overwrites; pass --replace to replace the library");
            }
            if report.replaced || report.applied > 0 {
                println!("{detail}");
            } else {
                println!("No quicklinks were written ({} existing)", count);
            }
            Ok(!report.has_errors())
        }
        "export" => {
            let json = store.export_json()?;
            let count = store.list()?.len();
            match option_value(args, "--out") {
                Some(out) => {
                    fs::write(&out, json)
                        .map_err(|error| format!("could not write {out}: {error}"))?;
                    println!("Wrote {count} quicklinks (schema v1) to {out}");
                }
                None => println!("{json}"),
            }
            Ok(true)
        }
        other => Err(format!(
            "Unknown quicklink command: {other}. Use list, preview, import or export."
        )),
    }
}

fn run_storage(args: &[String]) -> Result<bool, String> {
    let demo = has_flag(args, "--demo");
    let directory = if demo {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let path =
            env::temp_dir().join(format!("ansa-storage-demo-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&path).map_err(|error| error.to_string())?;
        println!("Demo data directory: {}", path.display());
        path
    } else {
        PathBuf::from(
            option_value(args, "--data-dir")
                .ok_or("Usage: ansa storage --data-dir <dir> [--demo]")?,
        )
    };

    let driver = open_driver(&directory)?;
    println!("opened: {}", driver.main_database_path().display());
    println!("schema version: {}", driver.schema_version());
    let pragma = driver.pragma_report().map_err(|error| error.to_string())?;
    println!(
        "pragmas: foreign_keys={} journal_mode={} synchronous={} busy_timeout={}ms",
        pragma.foreign_keys, pragma.journal_mode, pragma.synchronous, pragma.busy_timeout
    );

    let revision = driver
        .set_setting(
            SettingKey::MaximumResults,
            &SettingValue::MaximumResults(42),
            None,
        )
        .map_err(|error| error.to_string())?;
    let snapshot = driver
        .get_setting(SettingKey::MaximumResults)
        .map_err(|error| error.to_string())?
        .ok_or("test setting disappeared after write")?;
    println!(
        "test setting: maximum_results={} (revision {})",
        setting_label(&snapshot.value),
        revision
    );

    let stale = driver.set_setting(
        SettingKey::MaximumResults,
        &SettingValue::MaximumResults(7),
        Some(revision + 1),
    );
    match stale {
        Err(_) => println!("compare-and-swap: stale revision rejected as expected"),
        Ok(_) => return Err("compare-and-swap guard accepted a stale revision".into()),
    }

    let backup = driver
        .backup_to(driver.backup_path())
        .map_err(|error| error.to_string())?;
    println!(
        "backup verified: {} · integrity {} · schema v{} · {} settings · {} records",
        backup.path.display(),
        backup.integrity,
        backup.schema_version,
        backup.settings_count,
        backup.user_records
    );
    if demo {
        println!("Demo files were left in place for inspection.");
    }
    Ok(true)
}

fn setting_label(value: &SettingValue) -> String {
    match value {
        SettingValue::Hotkey(text) => text.clone(),
        SettingValue::AliasPrefix(text) => text.clone(),
        SettingValue::Theme(theme) => format!("{theme:?}").to_lowercase(),
        SettingValue::LaunchMode(mode) => format!("{mode:?}").to_lowercase(),
        SettingValue::MaximumResults(limit) => limit.to_string(),
    }
}

struct DemoShortcutProbe;
impl ShortcutProbe for DemoShortcutProbe {
    fn probe(&self, _shortcut: &Shortcut) -> ProbeOutcome {
        ProbeOutcome::Available
    }
}

struct DemoLaunchProbe;
impl LaunchProbe for DemoLaunchProbe {
    fn launch(&self, _command_id: &CommandId) -> LaunchOutcome {
        LaunchOutcome::Launched
    }
}

fn run_onboard(args: &[String]) -> Result<bool, String> {
    let state_path =
        option_value(args, "--state").ok_or("Usage: ansa onboard --state <path> [--demo]")?;
    let demo = has_flag(args, "--demo");
    if !demo {
        let onboarding = Onboarding::load(JsonFileStore::new(state_path.as_str()));
        println!("state file: {state_path}");
        println!(
            "status: {} · step: {}",
            onboarding.status().label(),
            onboarding.step()
        );
        println!("completions: {}", onboarding.completion_count());
        if let Some(warning) = onboarding.load_warning() {
            println!("warning: {warning}");
        }
        for step in onboarding.skipped_steps() {
            println!("skipped: {step}");
            if let Some(warning) = onboarding.warning_for(*step) {
                println!("  warning: {warning}");
            }
        }
        println!(
            "verified launch: {:?}",
            onboarding
                .verified_launch()
                .map(|verification| verification.command_id().to_string())
        );
        println!("Run with --demo to execute the scripted fake-probe journey.");
        return Ok(true);
    }

    println!("Ansa onboarding demo (fake probes)");
    println!("- This demo does NOT register a real global hotkey.");
    println!("- This demo does NOT launch an application.");
    println!("state file: {state_path}");
    let mut onboarding = Onboarding::fresh(JsonFileStore::new(state_path.as_str()));
    println!(
        "[start] step {} · status {}",
        onboarding.step(),
        onboarding.status().label()
    );

    onboarding.next().map_err(|error| error.to_string())?;
    println!(
        "[next] step {} · status {}",
        onboarding.step(),
        onboarding.status().label()
    );

    let shortcut = Shortcut::documented_default();
    let probe = onboarding
        .select_shortcut(&shortcut, &DemoShortcutProbe)
        .map_err(|error| error.to_string())?;
    println!(
        "[shortcut] {} · probe {} (fake) · registered {}",
        shortcut.display(),
        if probe.is_available() {
            "available"
        } else {
            "unavailable"
        },
        onboarding.shortcut_registered()
    );

    onboarding.next().map_err(|error| error.to_string())?;
    println!(
        "[next] step {} · status {}",
        onboarding.step(),
        onboarding.status().label()
    );

    let demo_command = CommandId::parse("app:demo").map_err(|error| error.to_string())?;
    let launch = onboarding
        .try_launch(&demo_command, &DemoLaunchProbe)
        .map_err(|error| error.to_string())?;
    println!(
        "[launch] {} for {} (fake probe; no process started)",
        launch, demo_command
    );

    onboarding.next().map_err(|error| error.to_string())?;
    println!(
        "[next] step {} · status {}",
        onboarding.step(),
        onboarding.status().label()
    );
    onboarding.next().map_err(|error| error.to_string())?;
    println!(
        "[next] step {} · status {}",
        onboarding.step(),
        onboarding.status().label()
    );
    onboarding.complete().map_err(|error| error.to_string())?;
    println!(
        "[complete] status {} · completions {}",
        onboarding.status().label(),
        onboarding.completion_count()
    );
    for step in onboarding.skipped_steps() {
        println!(
            "[warning] skipped {step}: {:?}",
            onboarding.warning_for(*step)
        );
    }
    println!("Demo probes do not register a real hotkey or launch an app.");
    Ok(true)
}

fn run_alpha_smoke(args: &[String]) -> Result<bool, String> {
    let cycles = match option_value(args, "--cycles") {
        Some(raw) => raw
            .parse::<usize>()
            .map_err(|_| format!("--cycles must be a positive integer, got {raw:?}"))?,
        None => DEFAULT_SMOKE_CYCLES,
    };
    match alpha_smoke(cycles) {
        Ok(report) => {
            println!("Alpha smoke: PASS");
            println!("cycles: {}", report.cycles);
            println!("accepted batches: {}", report.accepted_batches);
            println!("rejected batches: {}", report.rejected_batches);
            println!(
                "stable selection checks: {}",
                report.stable_selection_checks
            );
            println!("max pending: {}", report.max_pending);
            println!("max results: {}", report.max_results);
            println!("catalog size: {}", report.catalog_size);
            println!("assertions: {}", report.assertions);
            println!("elapsed: {:.1} ms", report.elapsed.as_secs_f64() * 1_000.0);
            println!("fingerprint: 0x{:016x}", report.fingerprint);
            Ok(true)
        }
        Err(error) => {
            println!("Alpha smoke: FAIL");
            println!("{error}");
            Ok(false)
        }
    }
}

fn load_library(path: &Path) -> Result<Library, String> {
    if !path.exists() {
        return Ok(Library::new());
    }
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let mut library = Library::new();
    let report = library.import_json(&contents, ImportOptions::replace_confirmed());
    if report.has_errors() {
        let details = report
            .errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!(
            "{} is not a valid versioned quicklink library: {details}",
            path.display()
        ));
    }
    Ok(library)
}

fn save_library(path: &Path, library: &Library) -> Result<(), String> {
    let json = library.export_json().map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| format!("could not write {}: {error}", path.display()))
}

fn option_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|argument| argument == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn multi_values(args: &[String], name: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == name
            && let Some(value) = args.get(index + 1)
        {
            values.push(value.clone());
            index += 2;
            continue;
        }
        index += 1;
    }
    values
}

fn has_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|argument| argument == name)
}

fn positional_args(args: &[String], start: usize, value_flags: &[&str]) -> Vec<String> {
    let mut values = Vec::new();
    let mut index = start;
    while index < args.len() {
        let argument = &args[index];
        if value_flags.contains(&argument.as_str()) {
            index += 2;
            continue;
        }
        if argument.starts_with("--") {
            index += 1;
            continue;
        }
        values.push(argument.clone());
        index += 1;
    }
    values
}

fn main() -> ExitCode {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("Ansa: {error}");
            ExitCode::FAILURE
        }
    }
}
