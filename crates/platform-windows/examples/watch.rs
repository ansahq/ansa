//! Runs `CatalogWatcher` against the real Start Menu roots for bounded polls.
//!
//! Run with:
//!
//! ```text
//! cargo run -p ansa-platform-windows --example watch --offline
//! ```
//!
//! This is wiring evidence for the platform crate only. The desktop and CLI do
//! not poll the watcher yet; desktop integration is tracked as L11. The example
//! performs a fixed number of polls, prints each outcome and reports whether a
//! rescan is due. It never mutates the Start Menu.

#[cfg(windows)]
fn main() {
    use ansa_platform_windows::CatalogWatcher;
    use std::time::{Duration, Instant};

    const POLLS: usize = 6;
    const INTERVAL: Duration = Duration::from_millis(300);

    let roots = match ansa_platform_windows::default_watch_roots() {
        Ok(roots) => roots,
        Err(error) => {
            println!("watch_roots_error={error:?}");
            std::process::exit(1);
        }
    };
    for root in &roots {
        println!("watch_root={:?} exists={}", root, root.is_dir());
    }

    let mut watcher = match CatalogWatcher::new(roots.clone(), INTERVAL) {
        Ok(watcher) => watcher,
        Err(error) => {
            println!("watch_setup_error={error:?}");
            std::process::exit(1);
        }
    };

    let mut rescan_due = false;
    for poll in 1..=POLLS {
        let started = Instant::now();
        let outcome = watcher.poll(Instant::now());
        rescan_due |= outcome.needs_rescan();
        println!(
            "poll={poll} outcome={outcome:?} rescan_due={} elapsed_ms={}",
            outcome.needs_rescan(),
            started.elapsed().as_millis()
        );
        if poll < POLLS {
            std::thread::sleep(INTERVAL + Duration::from_millis(25));
        }
    }

    println!(
        "polls={POLLS} interval_ms={} rescan_due={rescan_due} desktop_wiring=L11",
        INTERVAL.as_millis()
    );
}

#[cfg(not(windows))]
fn main() {
    println!("watch_example=unsupported platform");
}
