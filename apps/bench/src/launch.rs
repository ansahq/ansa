//! Spawns the real launcher's debug-only performance fixture and parses its
//! stdout markers.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

pub const FIRST_FRAME_MARKER: &str = "ANSA_PERF_FIRST_FRAME_US:";
pub const FRAME_MARKER: &str = "ANSA_PERF_FRAME:";
pub const DONE_MARKER: &str = "ANSA_PERF_DONE:";

#[derive(Debug, Clone)]
pub struct PerfLaunch {
    pub internal_first_frame_us: u64,
    pub wall_first_frame_us: u64,
    pub frames_us: Vec<u64>,
    pub done: bool,
    pub exit_ok: bool,
    pub stderr_tail: String,
}

#[derive(Debug, Clone)]
pub struct LaunchFailure {
    pub reason: String,
    pub stderr_tail: String,
    pub stdout_tail: String,
}

impl std::fmt::Display for LaunchFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.reason)?;
        if !self.stderr_tail.is_empty() {
            write!(formatter, "; stderr: {}", self.stderr_tail)?;
        }
        if !self.stdout_tail.is_empty() {
            write!(formatter, "; stdout: {}", self.stdout_tail)?;
        }
        Ok(())
    }
}

/// Parsed marker values, independent of process I/O for testability.
#[derive(Debug, Default, Clone)]
pub struct ParsedMarkers {
    pub first_frame_us: Option<u64>,
    pub frames_us: Vec<u64>,
    pub done: Option<u64>,
    pub malformed: usize,
}

pub fn parse_lines(lines: &[String]) -> ParsedMarkers {
    let mut parsed = ParsedMarkers::default();
    for line in lines {
        let line = line.trim();
        if let Some(value) = line.strip_prefix(FIRST_FRAME_MARKER) {
            match value.trim().parse::<u64>() {
                Ok(micros) if parsed.first_frame_us.is_none() => {
                    parsed.first_frame_us = Some(micros);
                }
                _ => parsed.malformed += 1,
            }
        } else if let Some(value) = line.strip_prefix(FRAME_MARKER) {
            let mut fields = value.split(':');
            let index = fields.next().and_then(|field| field.parse::<u32>().ok());
            let micros = fields.next().and_then(|field| field.parse::<u64>().ok());
            match (index, micros) {
                (Some(_), Some(micros)) if fields.next().is_none() => {
                    parsed.frames_us.push(micros);
                }
                _ => parsed.malformed += 1,
            }
        } else if let Some(value) = line.strip_prefix(DONE_MARKER) {
            match value.trim().parse::<u64>() {
                Ok(frames) => parsed.done = Some(frames),
                Err(_) => parsed.malformed += 1,
            }
        }
    }
    parsed
}

/// Spawns `ansa-desktop --perf-smoke-test` and waits up to `timeout`.
pub fn launch(
    desktop: &Path,
    frames: usize,
    timeout: Duration,
) -> Result<PerfLaunch, LaunchFailure> {
    if !desktop.is_file() {
        return Err(LaunchFailure {
            reason: format!("desktop executable not found at {}", desktop.display()),
            stderr_tail: String::new(),
            stdout_tail: String::new(),
        });
    }
    let started = Instant::now();
    let mut child = Command::new(desktop)
        .arg("--perf-smoke-test")
        .arg(format!("--perf-frames={frames}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| LaunchFailure {
            reason: format!("could not spawn {}: {error}", desktop.display()),
            stderr_tail: String::new(),
            stdout_tail: String::new(),
        })?;

    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let (sender, receiver) = mpsc::channel::<(Instant, String)>();
    let stdout_reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = sender.send((Instant::now(), line));
        }
    });
    let stderr_reader = thread::spawn(move || {
        BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
            .collect::<Vec<_>>()
    });

    let deadline = Instant::now() + timeout;
    let marker_deadline = Instant::now() + Duration::from_secs(15);
    let mut timed_out = false;
    let mut marker_timed_out = false;
    let mut stdout_lines = Vec::new();
    let mut first_frame_wall = None;
    let status = loop {
        while let Ok((seen_at, line)) = receiver.try_recv() {
            if line.trim().starts_with(FIRST_FRAME_MARKER) && first_frame_wall.is_none() {
                first_frame_wall =
                    Some(seen_at.saturating_duration_since(started).as_micros() as u64);
            }
            stdout_lines.push(line);
        }
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if first_frame_wall.is_none() && Instant::now() >= marker_deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    marker_timed_out = true;
                    break None;
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break None;
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };

    let _ = stdout_reader.join();
    let stderr_lines = stderr_reader.join().unwrap_or_default();
    for (seen_at, line) in receiver.try_iter() {
        if line.trim().starts_with(FIRST_FRAME_MARKER) && first_frame_wall.is_none() {
            first_frame_wall = Some(seen_at.saturating_duration_since(started).as_micros() as u64);
        }
        stdout_lines.push(line);
    }

    let stderr_tail = tail(&stderr_lines, 8);
    if marker_timed_out {
        return Err(LaunchFailure {
            reason: "ANSA_PERF_FIRST_FRAME_US did not appear within 15 s; the \
                     --perf-smoke-test fixture is debug-only and release binaries ignore it"
                .to_string(),
            stderr_tail,
            stdout_tail: tail(&stdout_lines, 8),
        });
    }
    if timed_out {
        return Err(LaunchFailure {
            reason: format!("launcher did not exit within {} ms", timeout.as_millis()),
            stderr_tail,
            stdout_tail: tail(&stdout_lines, 8),
        });
    }
    let parsed = parse_lines(&stdout_lines);
    let Some(internal_first_frame_us) = parsed.first_frame_us else {
        return Err(LaunchFailure {
            reason: "launcher exited without ANSA_PERF_FIRST_FRAME_US marker; the \
                     --perf-smoke-test fixture is debug-only and release binaries ignore it"
                .to_string(),
            stderr_tail,
            stdout_tail: tail(&stdout_lines, 8),
        });
    };
    let Some(wall_first_frame_us) = first_frame_wall else {
        return Err(LaunchFailure {
            reason: "first-frame marker was not observed on the pipe".to_string(),
            stderr_tail,
            stdout_tail: tail(&stdout_lines, 8),
        });
    };

    Ok(PerfLaunch {
        internal_first_frame_us,
        wall_first_frame_us,
        frames_us: parsed.frames_us,
        done: parsed.done.is_some(),
        exit_ok: status.is_some_and(|status| status.success()),
        stderr_tail,
    })
}

fn tail(lines: &[String], count: usize) -> String {
    let start = lines.len().saturating_sub(count);
    lines[start..].join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_first_frame_and_frame_markers() {
        let lines = vec![
            "noise".to_string(),
            "ANSA_PERF_FIRST_FRAME_US:12345".to_string(),
            "ANSA_PERF_FRAME:0:16000".to_string(),
            "ANSA_PERF_FRAME:1:16666".to_string(),
            "ANSA_PERF_DONE:2".to_string(),
        ];
        let parsed = parse_lines(&lines);
        assert_eq!(parsed.first_frame_us, Some(12_345));
        assert_eq!(parsed.frames_us, vec![16_000, 16_666]);
        assert_eq!(parsed.done, Some(2));
        assert_eq!(parsed.malformed, 0);
    }

    #[test]
    fn ignores_malformed_and_missing_markers() {
        let parsed = parse_lines(&[
            "ANSA_PERF_FRAME:x:1".to_string(),
            "ANSA_PERF_DONE:no".to_string(),
        ]);
        assert!(parsed.first_frame_us.is_none());
        assert!(parsed.frames_us.is_empty());
        assert!(parsed.done.is_none());
        assert_eq!(parsed.malformed, 2);
    }

    #[test]
    fn missing_desktop_is_a_typed_failure_not_a_panic() {
        let failure = launch(
            Path::new("definitely-not-a-real-desktop-ansa.exe"),
            1,
            Duration::from_millis(50),
        )
        .expect_err("missing executable");
        assert!(failure.reason.contains("not found"));
    }
}
