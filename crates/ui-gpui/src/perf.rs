//! Debug-only frame cadence fixture for `ansa-bench`.
//!
//! This creates the real launcher window, prints a process-relative
//! first-frame marker, runs a bounded chain of no-op frame callbacks, prints
//! each interval, and quits. It does not register the summon hotkey, discover
//! the application catalog, start external apps, or write the clipboard.

use super::Launcher;
use gpui::{App, Entity, Window};
use std::time::Instant;

pub(super) const DEFAULT_FRAMES: usize = 60;
pub(super) const MAX_FRAMES: usize = 300;

/// Parses `--perf-frames=N`, defaulting to 60 and clamping to 1..=300.
pub(super) fn frame_count() -> Result<usize, String> {
    let mut frames = DEFAULT_FRAMES;
    for argument in std::env::args() {
        if let Some(value) = argument.strip_prefix("--perf-frames=") {
            let parsed = value.parse::<usize>().map_err(|_| {
                format!("invalid --perf-frames value {value:?}; expected an integer")
            })?;
            frames = clamp_frames(parsed);
        }
    }
    Ok(frames)
}

fn clamp_frames(value: usize) -> usize {
    value.clamp(1, MAX_FRAMES)
}

/// Starts the fixture on the next presented frame.
pub(super) fn start(
    launcher: Entity<Launcher>,
    window: &mut Window,
    process_start: Instant,
    frames: usize,
) {
    window.on_next_frame(move |window, cx| {
        use std::io::Write;
        let first_frame_us = process_start.elapsed().as_micros() as u64;
        println!("ANSA_PERF_FIRST_FRAME_US:{first_frame_us}");
        let _ = std::io::stdout().flush();
        // `on_next_frame` only queues a callback; a notification keeps the next
        // frame coming without touching the launcher view's state.
        cx.notify(launcher.entity_id());
        window.on_next_frame(move |window, cx| {
            tick(launcher, 0, frames, Instant::now(), window, cx);
        });
    });
}

fn tick(
    launcher: Entity<Launcher>,
    index: usize,
    frames: usize,
    previous: Instant,
    window: &mut Window,
    cx: &mut App,
) {
    use std::io::Write;
    let now = Instant::now();
    let micros = now.saturating_duration_since(previous).as_micros() as u64;
    if index == 0 && micros == 0 {
        // `on_next_frame` callbacks registered from a frame callback can run
        // again in the same frame. Skip that zero-length artifact so the
        // reported intervals are real frame cadence.
        cx.notify(launcher.entity_id());
        window.on_next_frame(move |window, cx| {
            tick(launcher, index, frames, now, window, cx);
        });
        return;
    }
    println!("ANSA_PERF_FRAME:{index}:{micros}");
    let _ = std::io::stdout().flush();
    if index + 1 >= frames {
        println!("ANSA_PERF_DONE:{frames}");
        let _ = std::io::stdout().flush();
        cx.quit();
        return;
    }
    cx.notify(launcher.entity_id());
    window.on_next_frame(move |window, cx| {
        tick(launcher, index + 1, frames, now, window, cx);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_count_has_a_hard_cap() {
        assert_eq!(clamp_frames(0), 1);
        assert_eq!(clamp_frames(1), 1);
        assert_eq!(clamp_frames(60), 60);
        assert_eq!(clamp_frames(MAX_FRAMES), MAX_FRAMES);
        assert_eq!(clamp_frames(usize::MAX), MAX_FRAMES);
    }
}
