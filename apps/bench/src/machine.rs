//! Machine, toolchain and build identity recorded with every measurement.
//!
//! Every probe is optional: a missing `git`, `rustc`, `powercfg` or registry
//! value becomes an explicit `"unavailable"` string instead of failing a run.

use crate::{json, win_metrics};
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub const NOT_DEFINED: &str = "budget: not defined";

#[derive(Debug, Clone)]
pub struct BuildIdentity {
    pub commit: String,
    pub dirty: bool,
    pub dirty_detail: String,
    pub rustc: String,
    pub profile: &'static str,
    pub debug_assertions: bool,
}

#[derive(Debug, Clone)]
pub struct MachineIdentity {
    pub os: String,
    pub os_version: String,
    pub arch: String,
    pub cpu_model: String,
    pub logical_cores: usize,
    pub physical_cores: Option<u32>,
    pub total_ram_bytes: Option<u64>,
    pub power_scheme: String,
    pub timestamp_utc: String,
}

impl MachineIdentity {
    pub fn to_json(&self) -> String {
        format!(
            "{{\"os\":{},\"os_version\":{},\"arch\":{},\"cpu_model\":{},\"logical_cores\":{},\"physical_cores\":{},\"total_ram_bytes\":{},\"power_scheme\":{},\"timestamp_utc\":{}}}",
            json::string(&self.os),
            json::string(&self.os_version),
            json::string(&self.arch),
            json::string(&self.cpu_model),
            self.logical_cores,
            self.physical_cores
                .map_or_else(|| "null".to_string(), |value| value.to_string()),
            self.total_ram_bytes
                .map_or_else(|| "null".to_string(), |value| value.to_string()),
            json::string(&self.power_scheme),
            json::string(&self.timestamp_utc),
        )
    }

    pub fn human_summary(&self) -> String {
        format!(
            "{} {} ({}) | CPU: {} | cores: {} logical / {} physical | RAM: {} | power: {} | {}",
            self.os,
            self.os_version,
            self.arch,
            self.cpu_model,
            self.logical_cores,
            self.physical_cores
                .map_or_else(|| "unavailable".to_string(), |value| value.to_string()),
            self.total_ram_bytes
                .map_or_else(|| "unavailable".to_string(), format_bytes),
            self.power_scheme,
            self.timestamp_utc,
        )
    }
}

impl BuildIdentity {
    pub fn to_json(&self) -> String {
        format!(
            "{{\"commit\":{},\"dirty\":{},\"dirty_detail\":{},\"rustc\":{},\"profile\":{},\"debug_assertions\":{}}}",
            json::string(&self.commit),
            self.dirty,
            json::string(&self.dirty_detail),
            json::string(&self.rustc),
            json::string(self.profile),
            self.debug_assertions,
        )
    }

    pub fn human_summary(&self) -> String {
        format!(
            "commit {} ({}) | {} | profile {}",
            self.commit,
            if self.dirty { "dirty" } else { "clean" },
            self.rustc,
            self.profile,
        )
    }
}

pub fn collect_machine() -> MachineIdentity {
    MachineIdentity {
        os: std::env::consts::OS.to_string(),
        os_version: command_stdout("cmd", &["/c", "ver"])
            .unwrap_or_else(|| "unavailable".to_string()),
        arch: std::env::consts::ARCH.to_string(),
        cpu_model: cpu_model(),
        logical_cores: win_metrics::logical_processor_count(),
        physical_cores: win_metrics::physical_core_count(),
        total_ram_bytes: win_metrics::total_physical_memory_bytes(),
        power_scheme: command_stdout("powercfg", &["/getactivescheme"])
            .unwrap_or_else(|| "unknown (powercfg unavailable)".to_string()),
        timestamp_utc: utc_timestamp(),
    }
}

pub fn collect_build(root: &Path) -> BuildIdentity {
    let commit = command_stdout_in(root, "git", &["rev-parse", "HEAD"])
        .unwrap_or_else(|| "unavailable".to_string());
    let status = command_stdout_in(root, "git", &["status", "--porcelain"]);
    let dirty_detail = status.clone().unwrap_or_else(|| "unavailable".to_string());
    let dirty = status.is_some_and(|value| !value.trim().is_empty());
    BuildIdentity {
        commit,
        dirty,
        dirty_detail,
        rustc: command_stdout("rustc", &["--version"]).unwrap_or_else(|| "unavailable".to_string()),
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        debug_assertions: cfg!(debug_assertions),
    }
}

/// Runs a command and returns trimmed stdout. Missing programs and non-zero
/// exits return `None`; this function never panics or fails a benchmark run.
pub fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    command_stdout_in(Path::new("."), program, args)
}

pub fn command_stdout_in(workdir: &Path, program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(workdir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_string();
    if text.is_empty() { None } else { Some(text) }
}

fn cpu_model() -> String {
    if let Some(output) = command_stdout(
        "reg",
        &[
            "query",
            r"HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0",
            "/v",
            "ProcessorNameString",
        ],
    ) && let Some(line) = output
        .lines()
        .find(|line| line.contains("ProcessorNameString"))
        && let Some((_, value)) = line.split_once("REG_SZ")
    {
        let value = value.trim();
        if !value.is_empty() {
            return value.to_string();
        }
    }
    std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "unavailable".to_string())
}

/// UTC timestamp in ISO-8601 form, derived without a date-time dependency.
pub fn utc_timestamp() -> String {
    let (year, month, day, hour, minute, second) = utc_parts();
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Filesystem-safe UTC run identifier, e.g. `20261118T143012123Z`.
pub fn utc_run_id() -> String {
    let (year, month, day, hour, minute, second) = utc_parts();
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_millis())
        .unwrap_or(0);
    format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}{millis:03}Z")
}

fn utc_parts() -> (i32, u32, u32, u32, u32, u32) {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = (of_day / 3600) as u32;
    let minute = ((of_day % 3600) / 60) as u32;
    let second = (of_day % 60) as u32;
    (year, month, day, hour, minute, second)
}

/// Howard Hinnant's civil_from_days algorithm.
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    (
        (year + i64::from(month <= 2)) as i32,
        month as u32,
        day as u32,
    )
}

/// Human-readable byte count using binary units.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_commands_are_optional() {
        assert!(command_stdout("ansa-bench-no-such-program-xyz", &[]).is_none());
        assert!(command_stdout("cmd", &["/c", "exit", "7"]).is_none());
        // The build/summary path must not panic when git is unavailable.
        let build = collect_build(Path::new("."));
        assert!(!build.rustc.is_empty());
        assert!(!build.commit.is_empty());
    }

    #[test]
    fn utc_formatting_is_stable_and_parseable() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(20_089), (2025, 1, 1));
        let timestamp = utc_timestamp();
        assert_eq!(timestamp.len(), 20);
        assert!(timestamp.ends_with('Z'));
        assert_eq!(timestamp.as_bytes()[4], b'-');
        assert_eq!(timestamp.as_bytes()[10], b'T');
        let run_id = utc_run_id();
        assert_eq!(run_id.len(), 19);
        assert!(run_id.ends_with('Z'));
    }

    #[test]
    fn byte_formatting_is_explicit() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MiB");
    }
}
