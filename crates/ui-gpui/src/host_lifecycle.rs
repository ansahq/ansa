//! GPUI integration for the platform-neutral host lifecycle.
//!
//! The controller owns invocation identity and state. This module supplies the
//! real Windows backend, drives summon/dismiss through the controller, observes
//! the system theme, watches for monitor removal and owns the debug-only native
//! lifecycle smoke test. The launcher's HWND is wrapped in
//! [`MainThreadOnly`], so it cannot be moved into a worker task.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use ansa_host::{
    FocusOutcome, HostBackend, HostController, HostError, MainThreadDispatcher, MainThreadOnly,
    MonitorChange, MonitorInfo, Rect, ReleaseStep, ShowOutcome, ShowRequest, Size, ThemeAppearance,
};
#[cfg(debug_assertions)]
use ansa_host::{FocusOutcomeCounts, LifecycleState};
#[cfg(debug_assertions)]
use gpui::Entity;
use gpui::{App, Focusable, Window, WindowHandle};

use crate::Launcher;

/// One wake-up delivered by the global hotkey worker.
pub(crate) type Summon = (
    std::time::Instant,
    Option<ansa_platform_windows::FocusTarget>,
);

/// Real native effects for the launcher window.
pub(crate) struct LauncherBackend {
    hwnd: MainThreadOnly<Option<isize>>,
    dispatcher: MainThreadDispatcher,
    hotkey: Option<ansa_platform_windows::HotkeyRegistration>,
    summon_sender: Option<async_channel::Sender<Summon>>,
    native_released: bool,
}

impl LauncherBackend {
    fn new(hwnd: Option<isize>) -> Self {
        let dispatcher = MainThreadDispatcher::new();
        let token = dispatcher
            .token()
            .expect("a fresh main-thread dispatcher is owned by the calling thread");
        let hwnd = MainThreadOnly::bind(&token, hwnd)
            .expect("the launcher backend is created on its UI thread");
        Self {
            hwnd,
            dispatcher,
            hotkey: None,
            summon_sender: None,
            native_released: false,
        }
    }

    fn hwnd(&self) -> Option<isize> {
        self.hwnd.with(|hwnd| *hwnd)
    }

    fn ensure_owner(&self, operation: &'static str) -> Result<(), HostError> {
        self.dispatcher
            .token()
            .map(|_| ())
            .map_err(|error| HostError::native(operation, error.to_string()))
    }

    fn set_hotkey(
        &mut self,
        hotkey: ansa_platform_windows::HotkeyRegistration,
        sender: async_channel::Sender<Summon>,
    ) {
        self.hotkey = Some(hotkey);
        self.summon_sender = Some(sender);
    }

    fn hotkey_registered(&self) -> bool {
        self.hotkey.is_some()
    }

    fn work_area_for(&mut self, rect: Rect) -> Result<Rect, HostError> {
        let monitors = self.list_monitors()?;
        monitors
            .iter()
            .find(|monitor| monitor.work_area.contains_rect(rect))
            .or_else(|| {
                monitors
                    .iter()
                    .filter(|monitor| monitor.work_area.intersects(rect))
                    .max_by_key(|monitor| monitor.work_area.intersection_area(rect))
            })
            .map(|monitor| monitor.work_area)
            .ok_or_else(|| {
                HostError::native(
                    "show",
                    "no monitor work area contains the requested launcher placement",
                )
            })
    }
}

impl std::fmt::Debug for LauncherBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LauncherBackend")
            .field("hwnd", &self.hwnd)
            .field("hotkey_registered", &self.hotkey.is_some())
            .field("summon_channel", &self.summon_sender.is_some())
            .field("native_released", &self.native_released)
            .finish()
    }
}

impl HostBackend for LauncherBackend {
    fn list_monitors(&mut self) -> Result<Vec<MonitorInfo>, HostError> {
        self.ensure_owner("list_monitors")?;
        ansa_platform_windows::enumerate_monitors()
            .map_err(|error| HostError::native("list_monitors", error))
    }

    fn show(&mut self, rect: Rect) -> Result<ShowOutcome, HostError> {
        self.ensure_owner("show")?;
        if self.native_released {
            return Err(HostError::native(
                "show",
                "the launcher's native handles were already released",
            ));
        }
        let hwnd = self
            .hwnd()
            .ok_or_else(|| HostError::native("show", "the launcher HWND is unavailable"))?;
        let work_area = self.work_area_for(rect)?;
        ansa_platform_windows::place_and_show_launcher(hwnd, rect, work_area)
            .map_err(|error| HostError::native("show", error))
    }

    fn hide(&mut self) -> Result<(), HostError> {
        self.ensure_owner("hide")?;
        let hwnd = self
            .hwnd()
            .ok_or_else(|| HostError::native("hide", "the launcher HWND is unavailable"))?;
        ansa_platform_windows::hide_launcher(hwnd).map_err(|error| HostError::native("hide", error))
    }

    fn place(&mut self, rect: Rect) -> Result<(), HostError> {
        self.ensure_owner("place")?;
        let hwnd = self
            .hwnd()
            .ok_or_else(|| HostError::native("place", "the launcher HWND is unavailable"))?;
        let work_area = self.work_area_for(rect)?;
        ansa_platform_windows::place_launcher_window(hwnd, rect, work_area)
            .map_err(|error| HostError::native("place", error))
    }

    fn request_focus(&mut self) -> FocusOutcome {
        if let Err(error) = self.ensure_owner("request_focus") {
            return FocusOutcome::Skipped {
                reason: error.to_string(),
            };
        }
        match self.hwnd() {
            Some(hwnd) => ansa_platform_windows::request_launcher_focus(hwnd),
            None => FocusOutcome::Unsupported {
                reason: "the launcher HWND is unavailable".to_owned(),
            },
        }
    }

    fn observe_theme(&mut self) -> Result<ThemeAppearance, HostError> {
        self.ensure_owner("observe_theme")?;
        ansa_platform_windows::read_apps_theme().map_err(|error| HostError::ThemeUnavailable {
            reason: error.to_string(),
        })
    }

    fn release(&mut self, step: ReleaseStep) -> Result<(), HostError> {
        // Cleanup must not depend on a runtime thread check: the backend is
        // `!Send`, so a `&mut self` already proves the owner thread.
        match step {
            ReleaseStep::Hotkeys => {
                // Dropping the registration joins the worker and unregisters
                // the hotkey before any later step runs.
                self.hotkey.take();
            }
            ReleaseStep::NativeHandles => {
                if let Some(hwnd) = self.hwnd() {
                    // The HWND itself is owned by GPUI and is destroyed with
                    // the window; hiding here makes the release observable and
                    // safe even during an explicit quit shortcut.
                    let _ = ansa_platform_windows::hide_launcher(hwnd);
                }
                self.native_released = true;
            }
            ReleaseStep::WorkerChannels => {
                self.summon_sender.take();
            }
        }
        Ok(())
    }
}

pub(crate) type LauncherHost = Rc<RefCell<HostController<LauncherBackend>>>;

pub(crate) fn create_host(hwnd: Option<isize>) -> LauncherHost {
    Rc::new(RefCell::new(HostController::new(LauncherBackend::new(
        hwnd,
    ))))
}

fn focus_status(outcome: &FocusOutcome) -> Option<String> {
    match outcome {
        FocusOutcome::Succeeded => None,
        FocusOutcome::Denied { reason } => {
            Some(format!("Launcher visible; focus denied: {reason}"))
        }
        FocusOutcome::Skipped { reason } => {
            Some(format!("Launcher visible; activation skipped: {reason}"))
        }
        FocusOutcome::Unsupported { reason } => Some(format!(
            "Launcher visible; activation unsupported: {reason}"
        )),
    }
}

impl Launcher {
    pub(crate) fn host_hotkey_registered(&self) -> bool {
        self.host.borrow().backend().hotkey_registered()
    }

    pub(crate) fn install_hotkey(
        &mut self,
        hotkey: ansa_platform_windows::HotkeyRegistration,
        sender: async_channel::Sender<Summon>,
    ) {
        self.host
            .borrow_mut()
            .backend_mut()
            .set_hotkey(hotkey, sender);
    }

    pub(crate) fn summon_from_host(
        &mut self,
        target: Option<ansa_platform_windows::FocusTarget>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.is_composing(cx) {
            return;
        }
        let scale = window.scale_factor();
        let viewport = window.viewport_size();
        let desired_size = Size::new(
            (f32::from(viewport.width) * scale).round() as i32,
            (f32::from(viewport.height) * scale).round() as i32,
        );
        let request = ShowRequest {
            pointer: ansa_platform_windows::cursor_position(),
            current_window: self
                .native_handle
                .and_then(ansa_platform_windows::launcher_window_rect),
            desired_size,
        };
        match self.host.borrow_mut().show(request) {
            Ok(report) => {
                self.previous_target = target;
                self.engine.begin_invocation();
                window.activate_window();
                crate::compat::focus(window, &self.input.focus_handle(cx), cx);
                if let Some(status) = focus_status(&report.focus) {
                    self.status = status;
                }
                cx.notify();
            }
            Err(error) => {
                self.status = format!("Cannot show launcher: {error}");
                cx.notify();
            }
        }
    }

    pub(crate) fn dismiss_from_host(
        &mut self,
        restore: bool,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.is_composing(cx) {
            return;
        }
        let summon_unavailable = !self.host_hotkey_registered();
        let Some(invocation) = self.host.borrow().active_invocation() else {
            self.status = "Launcher is already hidden".into();
            cx.notify();
            return;
        };
        let Some(handle) = self.native_handle else {
            self.status = "Cannot hide launcher: native HWND unavailable".into();
            cx.notify();
            return;
        };

        if restore {
            let target = self.previous_target.take();
            let result = self.host.borrow_mut().hide_with(&invocation, || {
                ansa_platform_windows::dismiss_launcher(handle, target.as_ref())
                    .map_err(|error| HostError::native("dismiss", error))
            });
            match result {
                Ok((_report, Some(outcome))) => {
                    match outcome {
                        ansa_platform_windows::FocusRestoreOutcome::Restored => {}
                        ansa_platform_windows::FocusRestoreOutcome::Skipped { reason }
                        | ansa_platform_windows::FocusRestoreOutcome::Denied { reason } => {
                            self.status = reason;
                        }
                    }
                    self.after_hide(window, cx);
                    if summon_unavailable {
                        self.status = "Launcher hidden. Global summon is unavailable — relaunch from the desktop shortcut or quit with Ctrl+Q.".into();
                        cx.notify();
                    }
                }
                Ok((_report, None)) => {
                    // The controller was already hidden; nothing native ran.
                    self.after_hide(window, cx);
                }
                Err(error) => {
                    self.previous_target = target;
                    self.report_hide_error(error, cx);
                }
            }
        } else {
            let result = self.host.borrow_mut().hide(&invocation);
            match result {
                Ok(_) => {
                    self.previous_target = None;
                    self.after_hide(window, cx);
                    if summon_unavailable {
                        self.status = "Launcher hidden. Global summon is unavailable — relaunch from the desktop shortcut or quit with Ctrl+Q.".into();
                        cx.notify();
                    }
                }
                Err(error) => self.report_hide_error(error, cx),
            }
        }
    }

    fn after_hide(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.engine.hide_root();
        self.close_actions(window, cx);
        cx.notify();
    }

    fn report_hide_error(&mut self, error: HostError, cx: &mut gpui::Context<Self>) {
        self.status = match error {
            HostError::StaleInvocation { .. } => {
                "Dismissal ignored: a newer launcher invocation is active".to_owned()
            }
            HostError::ShutDown => "Cannot hide launcher: the host is shutting down".to_owned(),
            other => format!("Cannot hide launcher: {other}"),
        };
        cx.notify();
    }

    pub(crate) fn shutdown_host(&mut self) {
        let report = self.host.borrow_mut().shutdown();
        for failure in report.failures {
            eprintln!(
                "Ansa lifecycle release step {} failed: {}",
                failure.step.label(),
                failure.message
            );
        }
    }

    pub(crate) fn poll_environment(&mut self, cx: &mut gpui::Context<Self>) {
        match self.host.borrow_mut().observe_theme() {
            Ok(observation) if observation.changed => {
                self.light = observation.appearance == ThemeAppearance::Light;
                let theme = self.theme();
                self.input
                    .update(cx, |input, cx| input.set_theme(theme, cx));
                self.action_input
                    .update(cx, |input, cx| input.set_theme(theme, cx));
                self.theme_notice = None;
                cx.notify();
            }
            Ok(_) => {
                if self.theme_notice.take().is_some() {
                    cx.notify();
                }
            }
            Err(error) => {
                let notice = format!("System appearance unavailable: {error}");
                if self.theme_notice.as_deref() != Some(notice.as_str()) {
                    self.theme_notice = Some(notice);
                    cx.notify();
                }
            }
        }

        if !self.host.borrow().state().is_visible() {
            return;
        }
        let pointer = ansa_platform_windows::cursor_position();
        match self.host.borrow_mut().monitors_changed(pointer) {
            Ok(MonitorChange::Replaced { to, focus, .. }) => {
                self.status = match focus_status(&focus) {
                    Some(status) => format!("Moved to monitor {to}; {status}"),
                    None => format!("Moved to monitor {to}"),
                };
                cx.notify();
            }
            Ok(_) => {}
            Err(error) => {
                self.status = format!("Monitor check failed: {error}");
                cx.notify();
            }
        }
    }
}

/// Checks the environment (theme and monitor topology) at a low frequency.
///
/// GPUI 0.2.2 does not expose a display-change or theme-change callback, so this
/// is deliberately a poll rather than a claimed native event subscription.
pub(crate) fn watch_environment(handle: WindowHandle<Launcher>, cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            if handle
                .update(cx, |this, _window, cx| this.poll_environment(cx))
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
}

#[cfg(debug_assertions)]
pub(crate) fn lifecycle_cycles() -> Result<usize, String> {
    let mut cycles = 1000usize;
    for argument in std::env::args() {
        if let Some(value) = argument.strip_prefix("--lifecycle-cycles=") {
            let parsed = value.parse::<usize>().map_err(|_| {
                format!("invalid --lifecycle-cycles value {value:?}; expected an integer")
            })?;
            cycles = parsed.clamp(1, 2000);
        }
    }
    Ok(cycles)
}

#[cfg(debug_assertions)]
pub(crate) fn run_lifecycle_smoke(
    launcher: Entity<Launcher>,
    window: &mut Window,
    cycles: usize,
    failure: Rc<RefCell<Option<String>>>,
) {
    window.on_next_frame(move |window, cx| {
        let scale = window.scale_factor();
        let viewport = window.viewport_size();
        let desired_size = Size::new(
            (f32::from(viewport.width) * scale).round() as i32,
            (f32::from(viewport.height) * scale).round() as i32,
        );
        let result = launcher.update(cx, |this, _cx| {
            let hwnd = this
                .native_handle
                .ok_or_else(|| "the launcher HWND is unavailable".to_owned())?;
            let request = ShowRequest {
                pointer: ansa_platform_windows::cursor_position(),
                current_window: ansa_platform_windows::launcher_window_rect(hwnd),
                desired_size,
            };
            let mut counts = FocusOutcomeCounts::default();
            for cycle in 0..cycles {
                let report = this
                    .host
                    .borrow_mut()
                    .show(request.clone())
                    .map_err(|error| format!("cycle {cycle}: show failed: {error}"))?;
                counts.record(&report.focus);
                if !ansa_platform_windows::launcher_window_live(hwnd) {
                    return Err(format!("cycle {cycle}: the HWND is not live after show"));
                }
                this.host
                    .borrow_mut()
                    .hide(&report.invocation)
                    .map_err(|error| format!("cycle {cycle}: hide failed: {error}"))?;
                let state = this.host.borrow().state();
                if state != LifecycleState::Hidden {
                    return Err(format!(
                        "cycle {cycle}: launcher stuck in state {}",
                        state.label()
                    ));
                }
                if !ansa_platform_windows::launcher_window_live(hwnd) {
                    return Err(format!("cycle {cycle}: the HWND is not live after hide"));
                }
            }
            if this.host.borrow().state() != LifecycleState::Hidden {
                return Err("the launcher did not finish hidden".to_owned());
            }
            Ok(counts)
        });

        match result {
            Ok(counts) => {
                println!(
                    "ANSA_LIFECYCLE_FOCUS succeeded={} denied={} skipped={} unsupported={}",
                    counts.succeeded, counts.denied, counts.skipped, counts.unsupported
                );
                println!("ANSA_LIFECYCLE_OK cycles={cycles}");
                cx.quit();
            }
            Err(error) => {
                eprintln!("ANSA_LIFECYCLE_FAILED cycles={cycles}: {error}");
                *failure.borrow_mut() = Some(format!("lifecycle smoke failed: {error}"));
                cx.quit();
            }
        }
    });
}
