//! Individual measurement profiles. Nothing here claims a latency budget.

use crate::corpus;
use crate::launch;
use crate::machine::format_bytes;
use crate::report::Measurement;
use crate::win_metrics;
use ansa_core::Engine;
use std::hint::black_box;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

const INCREMENTAL_PHRASE: &str = "benchmark command";

/// Warm, in-process query and per-keystroke measurements.
pub fn warm_query_measurements(rounds: usize) -> Vec<Measurement> {
    let mut engine = Engine::new(corpus::command_catalog());
    let mut query_times = Vec::with_capacity(rounds * corpus::COMMAND_QUERIES.len());
    let mut keystroke_times = Vec::with_capacity(rounds * INCREMENTAL_PHRASE.len());

    for _ in 0..10 {
        for query in corpus::COMMAND_QUERIES {
            engine.set_query(query);
            black_box(engine.results());
        }
        for end in 1..=INCREMENTAL_PHRASE.len() {
            engine.set_query(&INCREMENTAL_PHRASE[..end]);
            black_box(engine.results());
        }
    }

    for _ in 0..rounds {
        for query in corpus::COMMAND_QUERIES {
            let started = Instant::now();
            engine.set_query(query);
            black_box(engine.results());
            query_times.push(started.elapsed().as_secs_f64() * 1_000_000.0);
        }
        for end in 1..=INCREMENTAL_PHRASE.len() {
            let started = Instant::now();
            engine.set_query(&INCREMENTAL_PHRASE[..end]);
            black_box(engine.results());
            keystroke_times.push(started.elapsed().as_secs_f64() * 1_000_000.0);
        }
    }

    vec![
        Measurement::measured(
            "query_to_results",
            "microseconds",
            &mut query_times,
            0,
            "Engine::set_query + lazy results() on the frozen 10k corpus; includes ranking and result cloning, excludes UI input and paint",
        ),
        Measurement::measured(
            "keystroke_to_list",
            "microseconds",
            &mut keystroke_times,
            0,
            "incremental typing of 'benchmark command' against the 10k corpus; in-process model timing, not physical input latency",
        ),
    ]
}

/// Cold (first spawn) and warm (subsequent spawn) launch markers plus frames.
pub fn launch_measurements(
    desktop: Option<&Path>,
    samples: usize,
    frames: usize,
) -> Vec<Measurement> {
    let mut cold_internal = Vec::new();
    let mut cold_wall = Vec::new();
    let mut warm_internal = Vec::new();
    let mut warm_wall = Vec::new();
    let mut frame_intervals = Vec::new();
    let mut failures = 0usize;
    let mut failure_reasons = Vec::new();
    let mut incomplete_runs = 0usize;

    if let Some(desktop) = desktop {
        for sample in 0..samples {
            match launch::launch(desktop, frames, Duration::from_secs(90)) {
                Ok(run) => {
                    if sample == 0 {
                        cold_internal.push(run.internal_first_frame_us as f64);
                        cold_wall.push(run.wall_first_frame_us as f64);
                    } else {
                        warm_internal.push(run.internal_first_frame_us as f64);
                        warm_wall.push(run.wall_first_frame_us as f64);
                    }
                    frame_intervals.extend(run.frames_us.iter().map(|value| *value as f64));
                    if !run.done || !run.exit_ok {
                        incomplete_runs += 1;
                        if !run.stderr_tail.is_empty() {
                            failure_reasons.push(run.stderr_tail);
                        }
                    }
                }
                Err(failure) => {
                    failures += 1;
                    failure_reasons.push(failure.to_string());
                }
            }
        }
    } else {
        failures = samples;
        failure_reasons.push(
            "no ansa-desktop.exe found next to ansa-bench; pass --desktop=<path>".to_string(),
        );
    }

    let reason = if failure_reasons.is_empty() {
        if samples <= 1 {
            "no warm sample requested (one launch sample)".to_string()
        } else {
            "no successful launch".to_string()
        }
    } else {
        failure_reasons.join(" | ")
    };
    let sample_note = format!(
        "debug-only launcher fixture, so launch samples use target/debug/ansa-desktop.exe even when the harness is release; first spawn labelled cold without reboot/cache purge; internal marker starts at run_with_application, wall includes process creation; incomplete={incomplete_runs}"
    );

    let measurements = vec![
        if cold_internal.is_empty() {
            Measurement::unavailable("cold_launch_first_frame_internal", "microseconds", &reason)
        } else {
            Measurement::measured(
                "cold_launch_first_frame_internal",
                "microseconds",
                &mut cold_internal,
                failures,
                sample_note.clone(),
            )
        },
        if cold_wall.is_empty() {
            Measurement::unavailable("cold_launch_first_frame_wall", "microseconds", &reason)
        } else {
            Measurement::measured(
                "cold_launch_first_frame_wall",
                "microseconds",
                &mut cold_wall,
                failures,
                "spawn to ANSA_PERF_FIRST_FRAME_US pipe read; includes process creation",
            )
        },
        if warm_internal.is_empty() {
            Measurement::unavailable("warm_launch_first_frame_internal", "microseconds", &reason)
        } else {
            Measurement::measured(
                "warm_launch_first_frame_internal",
                "microseconds",
                &mut warm_internal,
                failures,
                sample_note.clone(),
            )
        },
        if warm_wall.is_empty() {
            Measurement::unavailable("warm_launch_first_frame_wall", "microseconds", &reason)
        } else {
            Measurement::measured(
                "warm_launch_first_frame_wall",
                "microseconds",
                &mut warm_wall,
                failures,
                "repeated process spawn, not a hotkey summon of an already-resident launcher",
            )
        },
        if frame_intervals.is_empty() {
            Measurement::unavailable("frame_interval", "microseconds", &reason)
        } else {
            Measurement::measured(
                "frame_interval",
                "microseconds",
                &mut frame_intervals,
                0,
                "delta between consecutive no-op frame callbacks; not CPU frame-work time",
            )
        },
        Measurement::count(
            "launch_failures",
            failures as u64,
            "processes that timed out or omitted the first-frame marker",
        ),
    ];
    measurements
}

/// Working set, private bytes, PSS, GPU and process-tree accounting.
pub fn memory_measurements() -> Vec<Measurement> {
    // Keep one resident 10k-command engine while sampling so the memory number
    // is attributable to the measured workload rather than an empty harness.
    let _resident = Engine::new(corpus::command_catalog());
    let Some(tree) = win_metrics::process_tree_memory() else {
        return vec![
            Measurement::unavailable(
                "memory_working_set",
                "bytes",
                "GetProcessMemoryInfo unavailable",
            ),
            Measurement::unavailable(
                "memory_private_bytes",
                "bytes",
                "GetProcessMemoryInfo unavailable",
            ),
            Measurement::unavailable("memory_pss", "bytes", "QueryWorkingSetEx unavailable"),
            Measurement::unavailable("gpu_allocation", "bytes", "no honest GPU counter available"),
        ];
    };

    let working_set: u64 = tree.processes.iter().map(|entry| entry.working_set).sum();
    let private_bytes: u64 = tree.processes.iter().map(|entry| entry.private_bytes).sum();
    let mut working_set_samples = [working_set as f64];
    let mut private_samples = [private_bytes as f64];
    let tree_names = tree
        .processes
        .iter()
        .map(|entry| format!("{}({})", entry.name, entry.pid))
        .collect::<Vec<_>>()
        .join(",");

    let mut measurements = vec![
        Measurement::measured(
            "memory_working_set",
            "bytes",
            &mut working_set_samples,
            0,
            format!("process tree [{tree_names}]; Win32 working set"),
        ),
        Measurement::measured(
            "memory_rss",
            "bytes",
            &mut [working_set as f64],
            0,
            "Windows has no separate RSS counter; this equals working set",
        ),
        Measurement::measured(
            "memory_private_bytes",
            "bytes",
            &mut private_samples,
            0,
            format!("process tree [{tree_names}]; PROCESS_MEMORY_COUNTERS_EX.PrivateUsage"),
        ),
    ];

    match tree.pss {
        Some(pss) => {
            measurements.push(Measurement::measured(
                "memory_pss",
                "bytes",
                &mut [pss as f64],
                0,
                format!("QueryWorkingSetEx proportional-set aggregation over [{tree_names}]"),
            ));
        }
        None => measurements.push(Measurement::unavailable(
            "memory_pss",
            "bytes",
            if tree.pss_complete {
                "QueryWorkingSetEx returned no resident pages".to_string()
            } else {
                "QueryWorkingSetEx aggregation incomplete".to_string()
            },
        )),
    }
    measurements.push(Measurement::unavailable(
        "gpu_allocation",
        "bytes",
        "no honest per-process GPU allocation counter is wired into this harness",
    ));
    measurements.push(Measurement::count(
        "process_tree_processes",
        tree.processes.len() as u64,
        format!(
            "{}; current process plus direct children; the invoking shell parent is not attributable; named indexer/clipboard/provider workers: none by default",
            tree_names
        ),
    ));
    measurements
}

/// Samples this process's CPU while sleeping. This is not whole-machine power.
pub fn idle_measurements(seconds: u64) -> Vec<Measurement> {
    if seconds == 0 {
        return vec![Measurement::unavailable(
            "idle_cpu_time",
            "seconds",
            "--idle-seconds=0",
        )];
    }
    let Some(start_cpu) = win_metrics::process_cpu_100ns() else {
        return vec![Measurement::unavailable(
            "idle_cpu_time",
            "seconds",
            "GetProcessTimes unavailable",
        )];
    };
    let started = Instant::now();
    let duration = Duration::from_secs(seconds);
    let mut samples = 1u64;
    while started.elapsed() < duration {
        let remaining = duration.saturating_sub(started.elapsed());
        thread::sleep(remaining.min(Duration::from_secs(1)));
        samples += 1;
    }
    let wall_seconds = started.elapsed().as_secs_f64();
    let Some(end_cpu) = win_metrics::process_cpu_100ns() else {
        return vec![Measurement::unavailable(
            "idle_cpu_time",
            "seconds",
            "GetProcessTimes unavailable after sample",
        )];
    };
    let cpu_seconds = end_cpu.saturating_sub(start_cpu) as f64 / 10_000_000.0;
    let percent_one_core = if wall_seconds > 0.0 {
        cpu_seconds / wall_seconds * 100.0
    } else {
        0.0
    };
    vec![
        Measurement::measured(
            "idle_cpu_time",
            "seconds",
            &mut [cpu_seconds],
            0,
            format!(
                "ansa-bench process only; {samples} GetProcessTimes samples over {wall_seconds:.3} s wall; no launcher worker resident"
            ),
        ),
        Measurement::measured(
            "idle_cpu_percent_one_core",
            "percent",
            &mut [percent_one_core],
            0,
            format!(
                "{samples} samples; percentage of one logical core, not whole-machine utilization"
            ),
        ),
    ]
}

/// Measures stale-token rejection latency and throughput.
pub fn cancel_flood(iterations: usize) -> Result<Vec<Measurement>, String> {
    let mut engine = Engine::new(corpus::command_catalog());
    let result = flood(&mut engine, iterations)?;
    let mut reject_nanos = result.reject_nanos.clone();
    let mut throughput = result
        .reject_nanos
        .iter()
        .map(|nanos| {
            if *nanos > 0.0 {
                1_000_000_000.0 / *nanos
            } else {
                0.0
            }
        })
        .collect::<Vec<_>>();
    Ok(vec![
        Measurement::measured(
            "stale_batch_reject",
            "nanoseconds",
            &mut reject_nanos,
            0,
            format!(
                "per-op inside 100-rejection batches; setup outside timing; rejected={} accepted_stale=0 current_accepted={}",
                result.rejected, result.current_accepted
            ),
        ),
        Measurement::measured(
            "stale_batch_reject_throughput",
            "ops/second",
            &mut throughput,
            0,
            "reciprocal of each rejection latency sample",
        ),
    ])
}

/// Runs a stale/current token flood against an engine and asserts the guard.
pub(crate) struct FloodResult {
    pub reject_nanos: Vec<f64>,
    pub rejected: usize,
    pub current_accepted: usize,
}

pub(crate) fn flood(engine: &mut Engine, iterations: usize) -> Result<FloodResult, String> {
    const BATCH: usize = 100;
    let batches = iterations.div_ceil(BATCH);
    let mut reject_nanos = Vec::with_capacity(batches);
    let mut rejected = 0usize;
    let mut current_accepted = 0usize;
    let mut accepted_stale = 0usize;
    let mut rejected_current = 0usize;

    for batch in 0..batches {
        // Prepare stale tokens and invalidate them before timing the guard.
        let mut stale_tokens = Vec::with_capacity(BATCH);
        let count = (iterations - batch * BATCH).min(BATCH);
        for iteration in 0..count {
            let stale = engine.set_query(&format!("stale-{batch}-{iteration}"));
            let _current = engine.set_query("flood-current");
            stale_tokens.push(stale);
        }

        let started = Instant::now();
        for stale in &stale_tokens {
            if engine.apply_batch(*stale, Vec::new()) {
                accepted_stale += 1;
            } else {
                rejected += 1;
            }
        }
        let elapsed_nanos = started.elapsed().as_nanos() as f64;
        reject_nanos.push(elapsed_nanos / count as f64);

        // Also prove that a token obtained before a fresh invocation is dead.
        let before_invocation = engine.query_token();
        let fresh = engine.begin_invocation();
        if engine.apply_batch(before_invocation, Vec::new()) {
            accepted_stale += 1;
        }
        if engine.apply_batch(fresh, Vec::new()) {
            current_accepted += 1;
        } else {
            rejected_current += 1;
        }
        if engine.apply_batch(stale_tokens[0], Vec::new()) {
            accepted_stale += 1;
        }
    }

    if accepted_stale != 0 {
        return Err(format!(
            "{accepted_stale} stale batches were accepted; the token guard failed"
        ));
    }
    if rejected_current != 0 {
        return Err(format!(
            "{rejected_current} current batches were rejected; the token guard over-rejected"
        ));
    }
    if rejected != iterations {
        return Err(format!(
            "expected {iterations} stale rejections, observed {rejected}"
        ));
    }
    Ok(FloodResult {
        reject_nanos,
        rejected,
        current_accepted,
    })
}

/// Simulated delayed provider: sleeps, then submits a stale token after the
/// query or invocation changed. No network is used.
pub fn delayed_provider() -> Result<Vec<Measurement>, String> {
    const DELAYS_MS: [u64; 5] = [5, 25, 100, 250, 500];
    let mut engine = Engine::new(corpus::command_catalog());
    let mut observed = Vec::with_capacity(DELAYS_MS.len());
    let mut rejections = 0u64;

    for (index, delay_ms) in DELAYS_MS.into_iter().enumerate() {
        let token = engine.set_query("provider-base");
        let started = Instant::now();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(delay_ms));
            (Instant::now(), token)
        });
        if index % 2 == 0 {
            engine.set_query("user-typed-override");
        } else {
            engine.begin_invocation();
        }
        let (finished_at, stale) = handle
            .join()
            .map_err(|_| "delayed provider thread panicked".to_string())?;
        observed.push(finished_at.saturating_duration_since(started).as_secs_f64() * 1000.0);
        if engine.apply_batch(stale, Vec::new()) {
            return Err(format!(
                "delayed provider batch was accepted after a query/invocation change (delay {delay_ms} ms)"
            ));
        }
        rejections += 1;
    }

    Ok(vec![
        Measurement::measured(
            "delayed_provider_observed_delay",
            "milliseconds",
            &mut observed,
            0,
            "simulated sleeping provider, no network; measured from spawn to thread completion, then the stale batch was rejected",
        ),
        Measurement::count(
            "delayed_provider_rejections",
            rejections,
            "all delayed batches were rejected after query/invocation changes",
        ),
    ])
}

pub struct FileIndexOutcome {
    pub measurements: Vec<Measurement>,
    pub sha256: String,
    pub bytes: u64,
}

/// Builds and scans a synthetic in-memory file index, verifying the frozen hash.
pub fn file_index_measurement(count: usize) -> Result<FileIndexOutcome, String> {
    let build_started = Instant::now();
    let (entries, sha256, bytes) = corpus::build_file_index(count);
    let build_micros = build_started.elapsed().as_secs_f64() * 1_000_000.0;
    let manifest =
        corpus::manifest_string(corpus::file_index_manifest_text(), "sha256").unwrap_or_default();
    if count == corpus::FILE_INDEX_COUNT && sha256 != manifest {
        return Err(format!(
            "file-index hash {sha256} does not match frozen manifest {manifest}; bump the version"
        ));
    }

    let query = "incident-report";
    let scan_started = Instant::now();
    let matches = entries
        .iter()
        .filter(|entry| entry.title.contains(query) || entry.path.contains(query))
        .count();
    let scan_micros = scan_started.elapsed().as_secs_f64() * 1_000_000.0;

    let measurements = vec![
        Measurement::measured(
            "fixture_index_build",
            "microseconds",
            &mut [build_micros],
            0,
            format!("synthetic in-memory index of {count} entries; not the production indexer"),
        ),
        Measurement::measured(
            "fixture_index_scan",
            "microseconds",
            &mut [scan_micros],
            0,
            format!("substring scan for {query:?} over {count} entries; matches={matches}"),
        ),
        Measurement::count(
            "fixture_index_entries",
            count as u64,
            "entries built for this measurement",
        ),
        Measurement::count(
            "fixture_index_serialized_bytes",
            bytes,
            format!("canonical JSONL bytes, {}", format_bytes(bytes)),
        ),
    ];
    Ok(FileIndexOutcome {
        measurements,
        sha256,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ansa_domain::{BuiltinCommand, Command, CommandId, CommandKind};

    fn command(id: &str, title: &str) -> Command {
        Command::new(
            CommandId::parse(id).unwrap(),
            title,
            CommandKind::Builtin(BuiltinCommand::About),
        )
    }

    #[test]
    fn flood_rejects_every_stale_token() {
        let mut engine = Engine::new(vec![command("app:a", "Alpha"), command("app:b", "Beta")]);
        let result = flood(&mut engine, 200).expect("flood guard");
        assert_eq!(result.rejected, 200);
        assert_eq!(result.reject_nanos.len(), 2);
        assert!(result.reject_nanos.iter().all(|value| value.is_finite()));
        assert!(result.current_accepted > 0);
    }

    #[test]
    fn delayed_provider_samples_are_rejected() {
        let measurements = delayed_provider().expect("delayed provider");
        let delay = measurements
            .iter()
            .find(|measurement| measurement.name == "delayed_provider_observed_delay")
            .unwrap();
        assert_eq!(delay.stats.unwrap().samples, 5);
        assert!(delay.stats.unwrap().min >= 4.0);
    }

    #[test]
    fn file_index_hash_matches_manifest_for_small_prefix_only_in_manifest_test() {
        // A 1000-entry prefix is not the frozen profile; only determinism is
        // asserted here. The full 1M hash check runs in `corpus` and `full`.
        let (first, _, _) = corpus::build_file_index(1000);
        let (second, _, _) = corpus::build_file_index(1000);
        assert_eq!(first.len(), second.len());
        assert!(first[999].path.contains("0000999"));
    }
}
