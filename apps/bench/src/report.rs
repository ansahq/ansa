//! Raw JSONL and human-readable report emission.

use crate::json;
use crate::machine::{BuildIdentity, MachineIdentity, NOT_DEFINED, format_bytes};
use crate::percentile::Stats;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Measurement {
    pub name: String,
    pub unit: String,
    pub stats: Option<Stats>,
    pub values: Vec<f64>,
    pub failures: usize,
    pub notes: String,
}

impl Measurement {
    pub fn measured(
        name: impl Into<String>,
        unit: impl Into<String>,
        samples: &mut [f64],
        failures: usize,
        notes: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            unit: unit.into(),
            stats: crate::percentile::summarize(samples),
            values: samples.to_vec(),
            failures,
            notes: notes.into(),
        }
    }

    pub fn unavailable(
        name: impl Into<String>,
        unit: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            unit: unit.into(),
            stats: None,
            values: Vec::new(),
            failures: 0,
            notes: reason.into(),
        }
    }

    pub fn count(name: impl Into<String>, value: u64, notes: impl Into<String>) -> Self {
        let mut samples = [value as f64];
        Self::measured(name, "count", &mut samples, 0, notes)
    }
}

#[derive(Debug, Clone)]
pub struct CorpusSummary {
    pub command_version: String,
    pub command_count: usize,
    pub command_sha256: String,
    pub command_verified: bool,
    pub file_index_version: String,
    pub file_index_count: usize,
    pub file_index_sha256: Option<String>,
    pub file_index_bytes: Option<u64>,
}

impl CorpusSummary {
    pub fn to_json(&self) -> String {
        format!(
            "{{\"commands\":{{\"version\":{},\"count\":{},\"sha256\":{},\"verified\":{}}},\"file_index\":{{\"version\":{},\"count\":{},\"sha256\":{},\"bytes\":{}}}}}",
            json::string(&self.command_version),
            self.command_count,
            json::string(&self.command_sha256),
            self.command_verified,
            json::string(&self.file_index_version),
            self.file_index_count,
            self.file_index_sha256
                .as_deref()
                .map_or_else(|| "null".to_string(), json::string),
            self.file_index_bytes
                .map_or_else(|| "null".to_string(), |value| value.to_string()),
        )
    }

    pub fn human_summary(&self) -> String {
        format!(
            "commands {} ({} entries, sha256 {}, {}) | file-index {} ({} entries, sha256 {})",
            self.command_version,
            self.command_count,
            self.command_sha256,
            if self.command_verified {
                "manifest match"
            } else {
                "MANIFEST MISMATCH"
            },
            self.file_index_version,
            self.file_index_count,
            self.file_index_sha256
                .as_deref()
                .unwrap_or("not hashed in this run"),
        )
    }
}

pub struct RunReport<'a> {
    pub run_id: String,
    pub profile: String,
    pub profile_state: String,
    pub command: String,
    pub duration_seconds: f64,
    pub machine: &'a MachineIdentity,
    pub build: &'a BuildIdentity,
    pub corpus: &'a CorpusSummary,
    pub measurements: Vec<Measurement>,
}

impl RunReport<'_> {
    pub fn write(&self, output_dir: &Path) -> io::Result<PathBuf> {
        fs::create_dir_all(output_dir)?;
        let raw_path = output_dir.join("raw.jsonl");
        let mut raw = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&raw_path)?;
        writeln!(
            raw,
            "{{\"kind\":\"run\",\"run_id\":{},\"profile\":{},\"command\":{},\"duration_seconds\":{},\"label\":\"measurement\",\"target\":{},\"machine\":{},\"corpus\":{},\"build\":{}}}",
            json::string(&self.run_id),
            json::string(&self.profile),
            json::string(&self.command),
            json::number(self.duration_seconds),
            json::string(NOT_DEFINED),
            self.machine.to_json(),
            self.corpus.to_json(),
            self.build.to_json(),
        )?;
        for measurement in &self.measurements {
            writeln!(raw, "{}", self.measurement_json(measurement))?;
        }
        raw.flush()?;

        let summary_path = output_dir.join("summary.md");
        fs::write(&summary_path, self.summary_markdown())?;
        Ok(output_dir.to_path_buf())
    }

    fn measurement_json(&self, measurement: &Measurement) -> String {
        let stats = measurement.stats;
        format!(
            "{{\"kind\":\"measurement\",\"run_id\":{},\"profile\":{},\"name\":{},\"unit\":{},\"label\":\"measurement\",\"target\":{},\"budget\":{},\"available\":{},\"samples\":{},\"p50\":{},\"p95\":{},\"p99\":{},\"min\":{},\"max\":{},\"failures\":{},\"values\":{},\"notes\":{},\"machine\":{},\"corpus\":{},\"build\":{},\"timestamp\":{}}}",
            json::string(&self.run_id),
            json::string(&self.profile),
            json::string(&measurement.name),
            json::string(&measurement.unit),
            json::string(NOT_DEFINED),
            json::string(NOT_DEFINED),
            stats.is_some(),
            stats.map_or(0, |stats| stats.samples),
            stats.map_or_else(|| "null".to_string(), |stats| json::number(stats.p50)),
            stats.map_or_else(|| "null".to_string(), |stats| json::number(stats.p95)),
            stats.map_or_else(|| "null".to_string(), |stats| json::number(stats.p99)),
            stats.map_or_else(|| "null".to_string(), |stats| json::number(stats.min)),
            stats.map_or_else(|| "null".to_string(), |stats| json::number(stats.max)),
            measurement.failures,
            values_json(&measurement.values),
            json::string(&measurement.notes),
            self.machine.to_json(),
            self.corpus.to_json(),
            self.build.to_json(),
            json::string(&self.machine.timestamp_utc),
        )
    }

    pub fn print_human(&self) {
        println!("ansa-bench profile: {}", self.profile);
        println!("profile state: {}", self.profile_state);
        println!("harness duration: {:.3} s", self.duration_seconds);
        println!("label: measurement | target: {NOT_DEFINED}");
        println!("machine: {}", self.machine.human_summary());
        println!("build: {}", self.build.human_summary());
        println!("corpus: {}", self.corpus.human_summary());
        for measurement in &self.measurements {
            match measurement.stats {
                Some(stats) => println!(
                    "{}: p50={} p95={} p99={} samples={} min={} max={} failures={} {} [{}]",
                    measurement.name,
                    number_with_unit(stats.p50, &measurement.unit),
                    number_with_unit(stats.p95, &measurement.unit),
                    number_with_unit(stats.p99, &measurement.unit),
                    stats.samples,
                    number_with_unit(stats.min, &measurement.unit),
                    number_with_unit(stats.max, &measurement.unit),
                    measurement.failures,
                    measurement.unit,
                    measurement.notes,
                ),
                None => {
                    if measurement.name == "memory_pss" {
                        println!("pss=unavailable ({})", measurement.notes);
                    } else if measurement.name == "gpu_allocation" {
                        println!("gpu=unavailable ({})", measurement.notes);
                    } else {
                        println!("{}: unavailable ({})", measurement.name, measurement.notes);
                    }
                }
            }
        }
        println!("budget: not defined; no budget is met or claimed.");
    }

    fn summary_markdown(&self) -> String {
        let mut output = String::new();
        output.push_str(&format!("# Ansa bench run {}\n\n", self.run_id));
        output.push_str(&format!("- Profile: `{}`\n", self.profile));
        output.push_str(&format!("- Profile state: {}\n", self.profile_state));
        output.push_str(&format!("- Command: `{}`\n", self.command));
        output.push_str(&format!(
            "- Harness duration: {:.3} s\n",
            self.duration_seconds
        ));
        output.push_str(&format!("- Machine: {}\n", self.machine.human_summary()));
        output.push_str(&format!("- Build: {}\n", self.build.human_summary()));
        output.push_str(&format!("- Corpus: {}\n", self.corpus.human_summary()));
        output.push_str("- Label: every number below is a **measurement**.\n");
        output.push_str(&format!(
            "- Target: **{NOT_DEFINED}**. No budget is met or claimed.\n\n"
        ));

        output.push_str("## Profile coverage\n\n");
        for (profile, state) in profile_coverage(&self.profile, &self.profile_state) {
            output.push_str(&format!("- `{profile}`: {state}\n"));
        }
        output.push_str("\n## Measurements\n\n");
        output.push_str(
            "| measurement | target | unit | samples | p50 | p95 | p99 | min | max | failures | notes |\n",
        );
        output.push_str("|---|---|---|---:|---:|---:|---:|---:|---:|---:|---|\n");
        for measurement in &self.measurements {
            match measurement.stats {
                Some(stats) => output.push_str(&format!(
                    "| {} | not defined | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                    measurement.name,
                    measurement.unit,
                    stats.samples,
                    json::number(stats.p50),
                    json::number(stats.p95),
                    json::number(stats.p99),
                    json::number(stats.min),
                    json::number(stats.max),
                    measurement.failures,
                    measurement.notes.replace('|', "\\|"),
                )),
                None => output.push_str(&format!(
                    "| {} | not defined | {} | 0 | unavailable | unavailable | unavailable | unavailable | unavailable | {} | {} |\n",
                    measurement.name,
                    measurement.unit,
                    measurement.failures,
                    measurement.notes.replace('|', "\\|"),
                )),
            }
        }

        output.push_str("\n## Limitations\n\n");
        output.push_str(
            "- In-process `Engine::set_query` + `results()` timing is not end-to-end UI input latency, paint, or display latency.\n\
             - First-frame timing comes from a real debug-only launcher window. The internal marker starts at `run_with_application` entry; the wall value includes process creation. OS loader time is not included in the internal value.\n\
             - `cold_launch` labels the first spawn in this harness run. No reboot or filesystem cache purge was performed, so it is not a hardware-cold boot measurement.\n\
             - PSS uses `QueryWorkingSetEx`; GPU allocation is not measured and prints `gpu=unavailable`.\n\
             - Idle CPU is the `ansa-bench` process tree. No indexer, clipboard or provider worker is resident by default, so this is not whole-machine idle power.\n\
             - The one-million-entry file index is a synthetic harness baseline, not the production indexer.\n\
             - The `full` and `soak` profiles are not claimed unless their report says they ran to completion.\n",
        );
        output
    }
}

pub fn values_json(values: &[f64]) -> String {
    let joined = values
        .iter()
        .map(|value| json::number(*value))
        .collect::<Vec<_>>()
        .join(",");
    format!("[{joined}]")
}

pub fn number_with_unit(value: f64, unit: &str) -> String {
    if unit == "bytes" {
        format!("{} ({})", json::number(value), format_bytes(value as u64))
    } else {
        format!("{} {unit}", json::number(value))
    }
}

fn profile_coverage(profile: &str, profile_state: &str) -> Vec<(&'static str, String)> {
    ["quick", "full", "soak", "cancel-flood", "delayed-provider"]
        .iter()
        .map(|name| {
            if *name == profile {
                (*name, profile_state.to_string())
            } else {
                (*name, "not run by this invocation".to_string())
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::{collect_build, collect_machine};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn report_is_written_with_explicit_labels() {
        let machine = collect_machine();
        let build = collect_build(Path::new("."));
        let corpus = CorpusSummary {
            command_version: "test".into(),
            command_count: 2,
            command_sha256: "abc".into(),
            command_verified: true,
            file_index_version: "file-test".into(),
            file_index_count: 1,
            file_index_sha256: None,
            file_index_bytes: None,
        };
        let mut samples = [10.0, 20.0];
        let report = RunReport {
            run_id: "test-run".into(),
            profile: "quick".into(),
            profile_state: "run (this report)".into(),
            command: "ansa-bench quick".into(),
            duration_seconds: 1.0,
            machine: &machine,
            build: &build,
            corpus: &corpus,
            measurements: vec![
                Measurement::measured("query", "microseconds", &mut samples, 0, "fixture"),
                Measurement::unavailable("memory_pss", "bytes", "QueryWorkingSetEx failed"),
            ],
        };
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ansa-bench-report-{unique}"));
        report.write(&path).expect("write report");
        let raw = fs::read_to_string(path.join("raw.jsonl")).expect("raw jsonl");
        assert!(raw.contains("\"label\":\"measurement\""));
        assert!(raw.contains("\"values\":[10.0,20.0]"));
        assert!(raw.contains(NOT_DEFINED));
        let summary = fs::read_to_string(path.join("summary.md")).expect("summary");
        assert!(summary.contains("budget: not defined"));
        assert!(summary.contains("unavailable"));
        let _ = fs::remove_dir_all(path);
    }
}
