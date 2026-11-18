//! Reproducible performance measurement harness for the Ansa foundation slice.
//!
//! No crate outside the workspace is linked. Every reported number is a
//! measurement; no latency budget exists in this repository and none is
//! claimed.

#![deny(unsafe_code)]

mod corpus;
mod json;
mod launch;
mod machine;
mod percentile;
mod profiles;
mod report;
mod sha256;
#[cfg(windows)]
#[allow(unsafe_code)]
mod win_metrics;
#[cfg(not(windows))]
#[path = "win_metrics_stub.rs"]
mod win_metrics;

use report::{CorpusSummary, Measurement, RunReport};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
ansa-bench - reproducible performance and resource measurement harness

Usage:
  ansa-bench [PROFILE] [OPTIONS]

Profiles (default: quick):
  quick             bounded: 3 launches, warm 10k sweep, 5 s idle
  full              unrun unless executed: 30 launches, 1M index, 1800 s idle
  soak              unrun unless executed: 72 h (259200 s) idle/soak
  cancel-flood      stale/current apply_batch flood with percentiles
  delayed-provider  simulated sleeping provider, no network
  corpus            verify frozen corpus manifests and hashes

Options:
  --profile=NAME         same as the positional profile
  --idle-seconds=N       override idle CPU sample duration
  --launch-samples=N     override number of launcher spawns (1..=60)
  --query-rounds=N       override warm query sweep rounds (1..=1000)
  --frames=N             no-op frames per launcher spawn (1..=300, default 60)
  --index-entries=N      file-index entries for the full profile (default 1000000)
  --flood-iterations=N   cancel-flood iterations (default 2000)
  --desktop=PATH         ansa-desktop.exe to spawn
  --output-dir=PATH      exact report directory (default docs/benchmarks/runs/<utc>)
  -h, --help             print this help

Unknown profiles or flags return a non-zero exit code. The launcher
--perf-smoke-test fixture is debug-only; a release desktop ignores it and the
launch measurement reports the missing marker as a failure.
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Quick,
    Full,
    Soak,
    CancelFlood,
    DelayedProvider,
    Corpus,
}

impl Mode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "quick" => Some(Self::Quick),
            "full" => Some(Self::Full),
            "soak" => Some(Self::Soak),
            "cancel-flood" => Some(Self::CancelFlood),
            "delayed-provider" => Some(Self::DelayedProvider),
            "corpus" => Some(Self::Corpus),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Quick => "quick",
            Self::Full => "full",
            Self::Soak => "soak",
            Self::CancelFlood => "cancel-flood",
            Self::DelayedProvider => "delayed-provider",
            Self::Corpus => "corpus",
        }
    }
}

#[derive(Debug, Clone)]
struct Options {
    mode: Mode,
    idle_seconds: Option<u64>,
    launch_samples: Option<usize>,
    query_rounds: Option<usize>,
    frames: usize,
    index_entries: Option<usize>,
    flood_iterations: usize,
    desktop: Option<PathBuf>,
    output_dir: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            mode: Mode::Quick,
            idle_seconds: None,
            launch_samples: None,
            query_rounds: None,
            frames: 60,
            index_entries: None,
            flood_iterations: 2000,
            desktop: None,
            output_dir: None,
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ansa-bench: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let raw_args = std::env::args().skip(1).collect::<Vec<_>>();
    let options = match Options::parse(&raw_args) {
        Ok(Some(options)) => options,
        Ok(None) => {
            print!("{USAGE}");
            return Ok(());
        }
        Err(error) => {
            eprintln!("{error}\n");
            print!("{USAGE}");
            return Err(error);
        }
    };

    let root = repo_root();
    let harness_started = std::time::Instant::now();
    let machine = machine::collect_machine();
    let build = machine::collect_build(&root);
    let identity = corpus::verify_command_corpus();
    if !identity.matches() {
        return Err(format!(
            "command corpus hash {} does not match frozen manifest {}; bump the corpus version",
            identity.sha256, identity.manifest_sha256
        ));
    }

    if options.mode == Mode::Corpus {
        println!("corpus: {}", identity.version);
        println!(
            "commands: count={} bytes={} sha256={} manifest={}",
            identity.count, identity.bytes, identity.sha256, identity.manifest_sha256
        );
        let (file_hash, file_bytes) = corpus::hash_file_index(corpus::FILE_INDEX_COUNT);
        let file_manifest = corpus::manifest_string(corpus::file_index_manifest_text(), "sha256")
            .unwrap_or_default();
        println!(
            "file-index: count={} bytes={} ({}) sha256={} manifest={}",
            corpus::FILE_INDEX_COUNT,
            file_bytes,
            machine::format_bytes(file_bytes),
            file_hash,
            file_manifest,
        );
        if file_hash != file_manifest {
            return Err(format!(
                "file-index hash {file_hash} does not match frozen manifest {file_manifest}; bump the version"
            ));
        }
        println!("corpus verification: OK");
        return Ok(());
    }

    let command = if raw_args.is_empty() {
        "ansa-bench quick".to_string()
    } else {
        format!("ansa-bench {}", raw_args.join(" "))
    };
    let run_id = machine::utc_run_id();
    let desktop = resolve_desktop(&root, options.desktop.as_deref());
    let default_idle = match options.mode {
        Mode::Quick => 5,
        Mode::Full => 1800,
        Mode::Soak => 259_200,
        _ => 0,
    };
    let idle_seconds = options.idle_seconds.unwrap_or(default_idle);
    let launch_samples = options.launch_samples.unwrap_or(match options.mode {
        Mode::Quick | Mode::Soak => 3,
        Mode::Full => 30,
        _ => 0,
    });
    let query_rounds = options.query_rounds.unwrap_or(match options.mode {
        Mode::Quick => 20,
        Mode::Full => 100,
        Mode::Soak => 10,
        _ => 0,
    });

    let mut measurements: Vec<Measurement> = Vec::new();
    let mut file_index_hash = None;
    let mut file_index_bytes = None;

    match options.mode {
        Mode::Quick | Mode::Full | Mode::Soak => {
            measurements.extend(profiles::launch_measurements(
                desktop.as_deref(),
                launch_samples,
                options.frames,
            ));
            measurements.extend(profiles::warm_query_measurements(query_rounds));
            if options.mode == Mode::Full {
                let entries = options.index_entries.unwrap_or(corpus::FILE_INDEX_COUNT);
                let outcome = profiles::file_index_measurement(entries)?;
                file_index_hash = Some(outcome.sha256);
                file_index_bytes = Some(outcome.bytes);
                measurements.extend(outcome.measurements);
            }
            measurements.extend(profiles::memory_measurements());
            measurements.extend(profiles::idle_measurements(idle_seconds));
        }
        Mode::CancelFlood => {
            measurements.extend(profiles::cancel_flood(options.flood_iterations)?);
        }
        Mode::DelayedProvider => {
            measurements.extend(profiles::delayed_provider()?);
        }
        Mode::Corpus => unreachable!("corpus mode returned above"),
    }

    let reduced = options.idle_seconds.is_some()
        || options.launch_samples.is_some()
        || options.query_rounds.is_some()
        || options.index_entries.is_some();
    let profile_state = match options.mode {
        Mode::Quick => {
            "run (this report; bounded quick profile, not the full or soak profile)".to_string()
        }
        Mode::Full if reduced => format!(
            "partial validation run (NOT the full profile: options override the 30-launch, 1M-index or 1800 s-idle defaults; command: {command})"
        ),
        Mode::Full => {
            "run (this report; requested 30 launches, 1M index and 1800 s idle)".to_string()
        }
        Mode::Soak if reduced => {
            format!("partial validation run (NOT the 72-hour soak profile; command: {command})")
        }
        Mode::Soak => "run (this report; requested 259200 s idle/soak)".to_string(),
        _ => "run (this report)".to_string(),
    };
    let profile_complete = profile_state.starts_with("run (this report");

    let file_index_count = if options.mode == Mode::Full {
        options.index_entries.unwrap_or(corpus::FILE_INDEX_COUNT)
    } else {
        corpus::FILE_INDEX_COUNT
    };
    let file_index_version = if file_index_count == corpus::FILE_INDEX_COUNT {
        corpus::FILE_INDEX_VERSION.to_string()
    } else {
        format!("{}-custom", corpus::FILE_INDEX_VERSION)
    };
    let summary = CorpusSummary {
        command_version: identity.version.clone(),
        command_count: identity.count,
        command_sha256: identity.sha256.clone(),
        command_verified: identity.matches(),
        file_index_version,
        file_index_count,
        file_index_sha256: file_index_hash,
        file_index_bytes,
    };
    let output_dir = options.output_dir.unwrap_or_else(|| {
        root.join("docs")
            .join("benchmarks")
            .join("runs")
            .join(&run_id)
    });
    let report = RunReport {
        run_id,
        profile: options.mode.label().to_string(),
        profile_state,
        command,
        duration_seconds: harness_started.elapsed().as_secs_f64(),
        machine: &machine,
        build: &build,
        corpus: &summary,
        measurements,
    };
    let written = report.write(&output_dir).map_err(|error| {
        format!(
            "could not write report to {}: {error}",
            output_dir.display()
        )
    })?;
    report.print_human();
    println!("raw: {}", written.join("raw.jsonl").display());
    println!("summary: {}", written.join("summary.md").display());
    if profile_complete && (options.mode == Mode::Full || options.mode == Mode::Soak) {
        println!(
            "note: this {} profile completed; its duration is recorded above. No budget is met.",
            options.mode.label()
        );
    }
    Ok(())
}

impl Options {
    /// Parses arguments. `Ok(None)` means help was requested.
    fn parse(args: &[String]) -> Result<Option<Self>, String> {
        let mut options = Self::default();
        let mut mode_seen = false;
        for argument in args {
            if argument == "-h" || argument == "--help" {
                return Ok(None);
            }
            if let Some(value) = argument.strip_prefix("--profile=") {
                options.mode =
                    Mode::parse(value).ok_or_else(|| format!("unknown profile {value:?}"))?;
                mode_seen = true;
            } else if let Some(value) = argument.strip_prefix("--idle-seconds=") {
                options.idle_seconds = Some(parse_bounded(value, 1, 86_400 * 365)?);
            } else if let Some(value) = argument.strip_prefix("--launch-samples=") {
                options.launch_samples = Some(parse_bounded(value, 1, 60)?);
            } else if let Some(value) = argument.strip_prefix("--query-rounds=") {
                options.query_rounds = Some(parse_bounded(value, 1, 1000)?);
            } else if let Some(value) = argument.strip_prefix("--frames=") {
                options.frames = parse_bounded(value, 1, 300)?;
            } else if let Some(value) = argument.strip_prefix("--index-entries=") {
                options.index_entries = Some(parse_bounded(value, 1, 2_000_000)?);
            } else if let Some(value) = argument.strip_prefix("--flood-iterations=") {
                options.flood_iterations = parse_bounded(value, 1, 1_000_000)?;
            } else if let Some(value) = argument.strip_prefix("--desktop=") {
                if value.is_empty() {
                    return Err("--desktop requires a path".to_string());
                }
                options.desktop = Some(PathBuf::from(value));
            } else if let Some(value) = argument.strip_prefix("--output-dir=") {
                if value.is_empty() {
                    return Err("--output-dir requires a path".to_string());
                }
                options.output_dir = Some(PathBuf::from(value));
            } else if argument.starts_with("--") {
                return Err(format!("unknown flag {argument:?}"));
            } else if mode_seen {
                return Err(format!("unexpected extra argument {argument:?}"));
            } else {
                options.mode =
                    Mode::parse(argument).ok_or_else(|| format!("unknown profile {argument:?}"))?;
                mode_seen = true;
            }
        }
        Ok(Some(options))
    }
}

fn parse_bounded<T>(value: &str, minimum: T, maximum: T) -> Result<T, String>
where
    T: std::str::FromStr + PartialOrd + std::fmt::Display + Copy,
{
    let parsed = value
        .parse::<T>()
        .map_err(|_| format!("invalid numeric value {value:?}"))?;
    if parsed < minimum || parsed > maximum {
        return Err(format!("value {value:?} is outside {minimum}..={maximum}"));
    }
    Ok(parsed)
}

fn repo_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| manifest.to_path_buf())
}

fn resolve_desktop(root: &Path, explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(explicit) = explicit {
        return explicit.is_file().then(|| explicit.to_path_buf());
    }
    let mut candidates = Vec::new();
    if let Ok(current) = std::env::current_exe()
        && let Some(parent) = current.parent()
    {
        candidates.push(parent.join("ansa-desktop.exe"));
    }
    candidates.push(root.join("target").join("debug").join("ansa-desktop.exe"));
    candidates.push(root.join("target").join("release").join("ansa-desktop.exe"));
    candidates.into_iter().find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn quick_is_the_default_and_modes_parse() {
        let options = Options::parse(&[]).unwrap().unwrap();
        assert_eq!(options.mode, Mode::Quick);
        let options = Options::parse(&args(&["soak"])).unwrap().unwrap();
        assert_eq!(options.mode, Mode::Soak);
        let options = Options::parse(&args(&["--profile=cancel-flood"]))
            .unwrap()
            .unwrap();
        assert_eq!(options.mode, Mode::CancelFlood);
    }

    #[test]
    fn unknown_flags_and_modes_are_errors() {
        assert!(Options::parse(&args(&["unknown-mode"])).is_err());
        assert!(Options::parse(&args(&["--nonsense=1"])).is_err());
        assert!(Options::parse(&args(&["--frames=9999"])).is_err());
        assert!(Options::parse(&args(&["--help"])).unwrap().is_none());
    }

    #[test]
    fn frames_are_capped_to_the_documented_bound() {
        let options = Options::parse(&args(&["quick", "--frames=300"]))
            .unwrap()
            .unwrap();
        assert_eq!(options.frames, 300);
        assert!(Options::parse(&args(&["quick", "--frames=301"])).is_err());
    }
}
