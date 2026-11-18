//! The launcher host state machine.

use crate::backend::{
    HostBackend, HostError, RELEASE_PLAN, ReleaseFailure, ShutdownReport, ThemeAppearance,
};
use crate::focus::FocusOutcome;
use crate::geometry::{Point, Rect, Size};
use crate::invocation::{InvocationId, InvocationIssuer};
use crate::monitor::{MonitorId, MonitorSelectionReason, place_on_monitor, select_monitor};

/// Coarse lifecycle state used for assertions and status text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleState {
    /// No launcher window is visible.
    Hidden,
    /// A show is in progress.
    Showing,
    /// The launcher is visible under one active invocation.
    Visible,
    /// A hide is in progress.
    Hiding,
    /// `shutdown()` has started or completed.
    ShuttingDown,
}

impl LifecycleState {
    /// Stable label for logs.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Hidden => "hidden",
            Self::Showing => "showing",
            Self::Visible => "visible",
            Self::Hiding => "hiding",
            Self::ShuttingDown => "shutting-down",
        }
    }

    /// Returns `true` only for [`Self::Visible`].
    pub fn is_visible(&self) -> bool {
        matches!(self, Self::Visible)
    }
}

/// Inputs for one summon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShowRequest {
    /// Cursor position in physical pixels, used to pick the pointer monitor.
    pub pointer: Option<Point>,
    /// Current launcher rectangle in physical pixels, used when no pointer
    /// hint is available.
    pub current_window: Option<Rect>,
    /// Desired launcher size in physical pixels.
    pub desired_size: Size,
}

impl ShowRequest {
    /// Creates a request with no pointer/window hint.
    pub fn new(desired_size: Size) -> Self {
        Self {
            pointer: None,
            current_window: None,
            desired_size,
        }
    }
}

/// Result of a successful show.
#[derive(Clone, Debug, PartialEq)]
pub struct ShowReport {
    /// The new, unique invocation.
    pub invocation: InvocationId,
    /// The monitor that was selected.
    pub monitor: MonitorId,
    /// Why the monitor was selected.
    pub selection: MonitorSelectionReason,
    /// Physical-pixel placement that was requested.
    pub rect: Rect,
    /// What happened to foreground activation. Never omitted.
    pub focus: FocusOutcome,
}

/// Result of a hide request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HideReport {
    /// The invocation that performed the hide.
    pub invocation: InvocationId,
    /// `true` when the launcher was already hidden and no native call ran.
    pub already_hidden: bool,
}

/// One monitor-change observation.
#[derive(Clone, Debug, PartialEq)]
pub enum MonitorChange {
    /// No monitor has been recorded for the launcher yet.
    NotPlaced,
    /// The monitor still exists; no native call ran.
    Retained {
        /// The recorded monitor.
        monitor: MonitorId,
    },
    /// The recorded monitor disappeared while the launcher was hidden.
    RemovedWhileHidden {
        /// The removed monitor.
        monitor: MonitorId,
    },
    /// The recorded monitor disappeared while visible; the launcher was
    /// re-placed and re-shown on a surviving monitor.
    Replaced {
        /// The removed monitor.
        from: MonitorId,
        /// The surviving monitor.
        to: MonitorId,
        /// The new physical-pixel placement.
        rect: Rect,
        /// Focus result of the re-show.
        focus: FocusOutcome,
    },
}

/// One appearance observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeObservation {
    /// The appearance currently reported by the OS.
    pub appearance: ThemeAppearance,
    /// `true` when this differs from the previous observation.
    pub changed: bool,
}

/// Owns the launcher lifecycle and exactly one active invocation.
///
/// The controller never exposes a way to construct an [`InvocationId`] itself;
/// every successful show issues a fresh one. `hide`/`hide_with` only accept the
/// current invocation, so a late or injected dismiss from an earlier summon is
/// rejected instead of closing the newer window.
#[derive(Debug)]
pub struct HostController<B: HostBackend> {
    backend: B,
    issuer: InvocationIssuer,
    state: LifecycleState,
    last_invocation: Option<InvocationId>,
    monitor: Option<MonitorId>,
    placed: Option<Rect>,
    theme: Option<ThemeAppearance>,
    shutdown: bool,
}

impl<B: HostBackend> HostController<B> {
    /// Creates a controller in the hidden state.
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            issuer: InvocationIssuer::new(),
            state: LifecycleState::Hidden,
            last_invocation: None,
            monitor: None,
            placed: None,
            theme: None,
            shutdown: false,
        }
    }

    /// Borrows the backend.
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Mutably borrows the backend for adapter configuration (for example
    /// installing a hotkey registration that shutdown must release).
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Current lifecycle state.
    pub fn state(&self) -> LifecycleState {
        self.state
    }

    /// The active invocation, if the launcher is visible.
    pub fn active_invocation(&self) -> Option<InvocationId> {
        match self.state {
            LifecycleState::Showing | LifecycleState::Visible | LifecycleState::Hiding => {
                self.last_invocation
            }
            LifecycleState::Hidden | LifecycleState::ShuttingDown => None,
        }
    }

    /// The monitor recorded by the last successful show or adoption.
    pub fn current_monitor(&self) -> Option<&MonitorId> {
        self.monitor.as_ref()
    }

    /// The placement recorded by the last successful show.
    pub fn placed_rect(&self) -> Option<Rect> {
        self.placed
    }

    /// The most recently observed system appearance.
    pub fn theme(&self) -> Option<ThemeAppearance> {
        self.theme
    }

    /// Adopts an already-visible native window at startup.
    ///
    /// This issues the first invocation without moving or showing anything, so
    /// the window created by the UI keeps its initial placement. The same
    /// stale-invocation rules apply afterwards.
    pub fn adopt_visible(&mut self) -> Result<InvocationId, HostError> {
        if self.shutdown {
            return Err(HostError::ShutDown);
        }
        let invocation = self.issuer.issue();
        self.last_invocation = Some(invocation);
        self.state = LifecycleState::Visible;
        Ok(invocation)
    }

    /// Selects a monitor, places the launcher and shows it.
    ///
    /// Every show issues a new invocation and records exactly one
    /// [`FocusOutcome`]. Showing while already visible is safe and idempotent
    /// in effect, but it still issues a new invocation so a stale dismiss from
    /// the earlier summon cannot close the new one.
    pub fn show(&mut self, request: ShowRequest) -> Result<ShowReport, HostError> {
        if self.shutdown {
            return Err(HostError::ShutDown);
        }

        let monitors = self.backend.list_monitors()?;
        let selection = select_monitor(
            &monitors,
            request.pointer,
            request.current_window,
            self.monitor.as_ref(),
        )
        .ok_or(HostError::NoMonitors)?;
        let rect = place_on_monitor(&selection.info, request.desired_size);

        let previous_state = self.state;
        self.state = LifecycleState::Showing;
        let outcome = match self.backend.show(rect) {
            Ok(outcome) => outcome,
            Err(error) => {
                self.state = previous_state;
                return Err(error);
            }
        };
        if !outcome.visible {
            self.state = previous_state;
            return Err(HostError::native(
                "show",
                format!(
                    "the native window did not become visible (focus {})",
                    outcome.focus.label()
                ),
            ));
        }

        let invocation = self.issuer.issue();
        self.last_invocation = Some(invocation);
        self.monitor = Some(selection.info.id.clone());
        self.placed = Some(rect);
        self.state = LifecycleState::Visible;

        Ok(ShowReport {
            invocation,
            monitor: selection.info.id,
            selection: selection.reason,
            rect,
            focus: outcome.focus,
        })
    }

    /// Hides the launcher through the backend.
    ///
    /// Returns [`HostError::StaleInvocation`] without calling the backend when
    /// `invocation` is not current. Repeated hides of the current invocation
    /// are idempotent and report `already_hidden`.
    pub fn hide(&mut self, invocation: &InvocationId) -> Result<HideReport, HostError> {
        if self.shutdown {
            return Err(HostError::ShutDown);
        }
        let Some(expected) = self.last_invocation else {
            return Err(HostError::NoActiveInvocation);
        };
        if expected != *invocation {
            return Err(HostError::StaleInvocation {
                expected,
                received: *invocation,
            });
        }
        if self.state == LifecycleState::Hidden {
            return Ok(HideReport {
                invocation: *invocation,
                already_hidden: true,
            });
        }

        let previous_state = self.state;
        self.state = LifecycleState::Hiding;
        if let Err(error) = self.backend.hide() {
            self.state = previous_state;
            return Err(error);
        }
        self.state = LifecycleState::Hidden;
        Ok(HideReport {
            invocation: *invocation,
            already_hidden: false,
        })
    }

    /// Hides the launcher through a caller-supplied native effect.
    ///
    /// This is for the checked dismiss path that must hide *and* restore the
    /// previously captured foreground window in one thread-affine sequence.
    /// The invocation check and state transitions are identical to [`Self::hide`]
    /// and happen before `effect` runs.
    pub fn hide_with<T>(
        &mut self,
        invocation: &InvocationId,
        effect: impl FnOnce() -> Result<T, HostError>,
    ) -> Result<(HideReport, Option<T>), HostError> {
        if self.shutdown {
            return Err(HostError::ShutDown);
        }
        let Some(expected) = self.last_invocation else {
            return Err(HostError::NoActiveInvocation);
        };
        if expected != *invocation {
            return Err(HostError::StaleInvocation {
                expected,
                received: *invocation,
            });
        }
        if self.state == LifecycleState::Hidden {
            return Ok((
                HideReport {
                    invocation: *invocation,
                    already_hidden: true,
                },
                None,
            ));
        }

        let previous_state = self.state;
        self.state = LifecycleState::Hiding;
        let value = match effect() {
            Ok(value) => value,
            Err(error) => {
                self.state = previous_state;
                return Err(error);
            }
        };
        self.state = LifecycleState::Hidden;
        Ok((
            HideReport {
                invocation: *invocation,
                already_hidden: false,
            },
            Some(value),
        ))
    }

    /// Requests activation for the current invocation without showing.
    pub fn request_focus(&mut self, invocation: &InvocationId) -> Result<FocusOutcome, HostError> {
        self.require_current(invocation)?;
        Ok(self.backend.request_focus())
    }

    /// Reapplies the current monitor's placement at `desired_size` without
    /// showing, focusing or issuing a new invocation.
    ///
    /// This is the DPI/size-change path. If the recorded monitor disappeared,
    /// the launcher is kept visible under the same invocation but moved to a
    /// surviving monitor; the periodic monitor watcher performs the
    /// re-show/focus bookkeeping separately.
    pub fn reapply_placement(&mut self, desired_size: Size) -> Result<Option<Rect>, HostError> {
        if self.shutdown {
            return Err(HostError::ShutDown);
        }
        if self.state != LifecycleState::Visible {
            return Ok(None);
        }
        let Some(current) = self.monitor.clone() else {
            return Ok(None);
        };
        let monitors = self.backend.list_monitors()?;
        let info = if let Some(info) = monitors.iter().find(|monitor| monitor.id == current) {
            info.clone()
        } else {
            let selection =
                select_monitor(&monitors, None, self.placed, None).ok_or(HostError::NoMonitors)?;
            self.monitor = Some(selection.info.id.clone());
            selection.info
        };
        let rect = place_on_monitor(&info, desired_size);
        self.backend.place(rect)?;
        self.placed = Some(rect);
        Ok(Some(rect))
    }

    /// Re-checks the monitor list and re-places a visible launcher when its
    /// monitor disappeared. Returns the observation without changing a hidden
    /// launcher.
    pub fn monitors_changed(&mut self, pointer: Option<Point>) -> Result<MonitorChange, HostError> {
        if self.shutdown {
            return Err(HostError::ShutDown);
        }
        let monitors = self.backend.list_monitors()?;
        let Some(current) = self.monitor.clone() else {
            return Ok(MonitorChange::NotPlaced);
        };
        if monitors.iter().any(|monitor| monitor.id == current) {
            return Ok(MonitorChange::Retained { monitor: current });
        }

        if self.state != LifecycleState::Visible {
            self.monitor = None;
            self.placed = None;
            return Ok(MonitorChange::RemovedWhileHidden { monitor: current });
        }

        let selection =
            select_monitor(&monitors, pointer, self.placed, None).ok_or(HostError::NoMonitors)?;
        let desired_size = self
            .placed
            .map(|rect| Size::new(rect.width, rect.height))
            .unwrap_or_else(|| Size::new(760, 500));
        let rect = place_on_monitor(&selection.info, desired_size);
        let outcome = self.backend.show(rect)?;
        if !outcome.visible {
            return Err(HostError::native(
                "show",
                "the launcher could not be re-shown on a surviving monitor",
            ));
        }
        self.monitor = Some(selection.info.id.clone());
        self.placed = Some(rect);

        Ok(MonitorChange::Replaced {
            from: current,
            to: selection.info.id,
            rect,
            focus: outcome.focus,
        })
    }

    /// Observes the OS appearance and reports whether it changed.
    ///
    /// An unavailable value is returned as [`HostError::ThemeUnavailable`]; the
    /// controller does not invent a theme.
    pub fn observe_theme(&mut self) -> Result<ThemeObservation, HostError> {
        let appearance = self.backend.observe_theme()?;
        let changed = self.theme != Some(appearance);
        self.theme = Some(appearance);
        Ok(ThemeObservation {
            appearance,
            changed,
        })
    }

    /// Alias used by appearance-change notifications.
    pub fn appearance_changed(&mut self) -> Result<ThemeObservation, HostError> {
        self.observe_theme()
    }

    /// Runs the ordered release plan exactly once.
    ///
    /// Hotkeys are released first so no new summon can arrive, then native
    /// handles, then worker channels. A failing step does not skip later
    /// steps. The second call is a no-op with `already_shut_down = true`.
    pub fn shutdown(&mut self) -> ShutdownReport {
        if self.shutdown {
            return ShutdownReport {
                already_shut_down: true,
                released: Vec::new(),
                failures: Vec::new(),
            };
        }
        self.shutdown = true;
        self.state = LifecycleState::ShuttingDown;

        let mut released = Vec::new();
        let mut failures = Vec::new();
        for step in RELEASE_PLAN {
            match self.backend.release(step) {
                Ok(()) => released.push(step),
                Err(error) => failures.push(ReleaseFailure {
                    step,
                    message: error.to_string(),
                }),
            }
        }

        ShutdownReport {
            already_shut_down: false,
            released,
            failures,
        }
    }

    fn require_current(&self, invocation: &InvocationId) -> Result<(), HostError> {
        if self.shutdown {
            return Err(HostError::ShutDown);
        }
        let Some(expected) = self.last_invocation else {
            return Err(HostError::NoActiveInvocation);
        };
        if expected != *invocation {
            return Err(HostError::StaleInvocation {
                expected,
                received: *invocation,
            });
        }
        Ok(())
    }
}

/// Dropping the controller (for example when the launcher window closes or the
/// app quits) performs the same idempotent shutdown, so a UI path that forgets
/// an explicit hook cannot orphan the hotkey or worker channel.
impl<B: HostBackend> Drop for HostController<B> {
    fn drop(&mut self) {
        let report = self.shutdown();
        if !report.failures.is_empty() {
            eprintln!(
                "Ansa host shutdown during drop had {} failed release step(s)",
                report.failures.len()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ReleaseStep;
    use crate::focus::{FocusOutcomeCounts, ShowOutcome};
    use crate::monitor::MonitorInfo;
    use std::collections::{HashSet, VecDeque};

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Call {
        ListMonitors,
        Show(Rect),
        Place(Rect),
        Hide,
        Focus,
        Theme,
        Release(ReleaseStep),
    }

    #[derive(Debug)]
    struct FakeBackend {
        monitors: Vec<MonitorInfo>,
        show_results: VecDeque<ShowOutcome>,
        hide_error: Option<HostError>,
        theme: Result<ThemeAppearance, HostError>,
        calls: Vec<Call>,
        release_error: Option<ReleaseStep>,
    }

    impl FakeBackend {
        fn new(monitors: Vec<MonitorInfo>) -> Self {
            Self {
                monitors,
                show_results: VecDeque::new(),
                hide_error: None,
                theme: Ok(ThemeAppearance::Dark),
                calls: Vec::new(),
                release_error: None,
            }
        }

        fn success() -> ShowOutcome {
            ShowOutcome {
                visible: true,
                focus: FocusOutcome::Succeeded,
            }
        }
    }

    impl HostBackend for FakeBackend {
        fn list_monitors(&mut self) -> Result<Vec<MonitorInfo>, HostError> {
            self.calls.push(Call::ListMonitors);
            Ok(self.monitors.clone())
        }

        fn show(&mut self, rect: Rect) -> Result<ShowOutcome, HostError> {
            self.calls.push(Call::Show(rect));
            Ok(self.show_results.pop_front().unwrap_or_else(Self::success))
        }

        fn place(&mut self, rect: Rect) -> Result<(), HostError> {
            self.calls.push(Call::Place(rect));
            Ok(())
        }

        fn hide(&mut self) -> Result<(), HostError> {
            self.calls.push(Call::Hide);
            match self.hide_error.clone() {
                Some(error) => Err(error),
                None => Ok(()),
            }
        }

        fn request_focus(&mut self) -> FocusOutcome {
            self.calls.push(Call::Focus);
            FocusOutcome::Succeeded
        }

        fn observe_theme(&mut self) -> Result<ThemeAppearance, HostError> {
            self.calls.push(Call::Theme);
            self.theme.clone()
        }

        fn release(&mut self, step: ReleaseStep) -> Result<(), HostError> {
            self.calls.push(Call::Release(step));
            if self.release_error == Some(step) {
                Err(HostError::native("release", step.label()))
            } else {
                Ok(())
            }
        }
    }

    fn monitor(id: &str, x: i32, primary: bool) -> MonitorInfo {
        MonitorInfo {
            id: MonitorId::new(id),
            work_area: Rect::new(x, 0, 1920, 1040),
            scale_factor: 1.0,
            primary,
        }
    }

    fn two_monitors() -> Vec<MonitorInfo> {
        vec![
            monitor("primary", 0, true),
            monitor("secondary", 1920, false),
        ]
    }

    fn request() -> ShowRequest {
        ShowRequest {
            pointer: None,
            current_window: None,
            desired_size: Size::new(760, 500),
        }
    }

    #[test]
    fn one_thousand_cycles_issue_unique_invocations_and_end_hidden() {
        let mut controller = HostController::new(FakeBackend::new(two_monitors()));
        let mut seen = HashSet::new();

        for _ in 0..1000 {
            let report = controller.show(request()).expect("show");
            assert_eq!(controller.state(), LifecycleState::Visible);
            assert_eq!(
                controller.active_invocation(),
                Some(report.invocation),
                "the show report and controller must agree"
            );
            assert!(seen.insert(report.invocation), "invocation was reused");
            assert!(report.focus.is_success());
            assert_eq!(report.monitor, MonitorId::new("primary"));

            let hide = controller.hide(&report.invocation).expect("hide");
            assert!(!hide.already_hidden);
            assert_eq!(controller.state(), LifecycleState::Hidden);
            assert_eq!(controller.active_invocation(), None);
        }

        assert_eq!(seen.len(), 1000);
        assert_eq!(
            controller
                .backend()
                .calls
                .iter()
                .filter(|call| matches!(call, Call::Show(_)))
                .count(),
            1000
        );
        assert_eq!(
            controller
                .backend()
                .calls
                .iter()
                .filter(|call| matches!(call, Call::Hide))
                .count(),
            1000
        );
    }

    #[test]
    fn focus_outcomes_are_preserved_and_never_silently_promoted() {
        let mut backend = FakeBackend::new(two_monitors());
        let outcomes = [
            FocusOutcome::Denied {
                reason: "foreground lock".into(),
            },
            FocusOutcome::Skipped {
                reason: "wrong thread".into(),
            },
            FocusOutcome::Unsupported {
                reason: "no adapter".into(),
            },
        ];
        for outcome in &outcomes {
            backend.show_results.push_back(ShowOutcome {
                visible: true,
                focus: outcome.clone(),
            });
        }
        let mut controller = HostController::new(backend);
        let mut counts = FocusOutcomeCounts::default();

        for expected in &outcomes {
            let report = controller.show(request()).expect("visible without focus");
            assert_eq!(&report.focus, expected);
            assert!(!report.focus.is_success());
            assert_eq!(controller.state(), LifecycleState::Visible);
            counts.record(&report.focus);
            controller.hide(&report.invocation).expect("hide");
        }
        assert_eq!(counts.denied, 1);
        assert_eq!(counts.skipped, 1);
        assert_eq!(counts.unsupported, 1);
        assert_eq!(counts.succeeded, 0);
    }

    #[test]
    fn a_native_show_that_stays_invisible_is_a_typed_failure() {
        let mut backend = FakeBackend::new(two_monitors());
        backend.show_results.push_back(ShowOutcome {
            visible: false,
            focus: FocusOutcome::Skipped {
                reason: "wrong thread".into(),
            },
        });
        let mut controller = HostController::new(backend);

        let error = controller.show(request()).expect_err("show must fail");
        assert!(matches!(
            error,
            HostError::Native {
                operation: "show",
                ..
            }
        ));
        assert_eq!(controller.state(), LifecycleState::Hidden);
    }

    #[test]
    fn monitor_removal_replaces_the_visible_launcher_and_readd_is_retained() {
        let mut controller = HostController::new(FakeBackend::new(two_monitors()));
        let report = controller.show(request()).expect("show");
        assert_eq!(report.monitor, MonitorId::new("primary"));

        controller.backend_mut().monitors = vec![monitor("secondary", 1920, true)];
        let change = controller.monitors_changed(None).expect("monitor check");
        match change {
            MonitorChange::Replaced {
                from,
                to,
                rect,
                focus,
            } => {
                assert_eq!(from, MonitorId::new("primary"));
                assert_eq!(to, MonitorId::new("secondary"));
                assert_eq!(
                    rect,
                    Rect::new(1920 + (1920 - 760) / 2, (1040 - 500) / 2, 760, 500)
                );
                assert!(focus.is_success());
            }
            other => panic!("expected replacement, got {other:?}"),
        }
        assert_eq!(
            controller.current_monitor(),
            Some(&MonitorId::new("secondary"))
        );
        assert_eq!(controller.active_invocation(), Some(report.invocation));

        // The original monitor comes back: the current monitor still exists,
        // so no flicker/re-show happens.
        controller.backend_mut().monitors = two_monitors();
        assert_eq!(
            controller.monitors_changed(None).expect("monitor check"),
            MonitorChange::Retained {
                monitor: MonitorId::new("secondary")
            }
        );

        // A later summon with the pointer on the re-added monitor selects it.
        let request = ShowRequest {
            pointer: Some(Point::new(100, 100)),
            ..request()
        };
        let report = controller.show(request).expect("show");
        assert_eq!(report.monitor, MonitorId::new("primary"));
    }

    #[test]
    fn monitor_removal_while_hidden_does_not_show_the_launcher() {
        let mut controller = HostController::new(FakeBackend::new(two_monitors()));
        let report = controller.show(request()).expect("show");
        controller.hide(&report.invocation).expect("hide");
        controller.backend_mut().monitors = vec![monitor("secondary", 1920, true)];

        assert_eq!(
            controller.monitors_changed(None).expect("monitor check"),
            MonitorChange::RemovedWhileHidden {
                monitor: MonitorId::new("primary")
            }
        );
        assert_eq!(controller.state(), LifecycleState::Hidden);
    }

    #[test]
    fn appearance_changes_are_reported_only_when_they_change() {
        let mut controller = HostController::new(FakeBackend::new(two_monitors()));
        let first = controller.observe_theme().expect("theme");
        assert_eq!(first.appearance, ThemeAppearance::Dark);
        assert!(first.changed);

        assert!(!controller.observe_theme().expect("theme").changed);

        controller.backend_mut().theme = Ok(ThemeAppearance::Light);
        let changed = controller.appearance_changed().expect("theme");
        assert_eq!(changed.appearance, ThemeAppearance::Light);
        assert!(changed.changed);
        assert_eq!(controller.theme(), Some(ThemeAppearance::Light));
    }

    #[test]
    fn an_unavailable_theme_is_a_typed_error_not_a_guess() {
        let mut backend = FakeBackend::new(two_monitors());
        backend.theme = Err(HostError::ThemeUnavailable {
            reason: "registry value missing".into(),
        });
        let mut controller = HostController::new(backend);
        assert!(matches!(
            controller.observe_theme(),
            Err(HostError::ThemeUnavailable { .. })
        ));
        assert_eq!(controller.theme(), None);
    }

    #[test]
    fn shutdown_releases_in_order_exactly_once_and_blocks_actions() {
        let mut controller = HostController::new(FakeBackend::new(two_monitors()));
        let report = controller.show(request()).expect("show");

        let first = controller.shutdown();
        assert!(!first.already_shut_down);
        assert_eq!(first.released, RELEASE_PLAN.to_vec());
        assert!(first.failures.is_empty());
        assert!(first.is_complete());
        assert_eq!(controller.state(), LifecycleState::ShuttingDown);

        let second = controller.shutdown();
        assert!(second.already_shut_down);
        assert!(second.released.is_empty());
        assert_eq!(
            controller
                .backend()
                .calls
                .iter()
                .filter(|call| matches!(call, Call::Release(_)))
                .count(),
            3,
            "the second shutdown must not touch the backend"
        );

        assert_eq!(controller.show(request()), Err(HostError::ShutDown));
        assert_eq!(
            controller.hide(&report.invocation),
            Err(HostError::ShutDown)
        );
        assert_eq!(
            controller.request_focus(&report.invocation),
            Err(HostError::ShutDown)
        );
        assert_eq!(controller.monitors_changed(None), Err(HostError::ShutDown));
    }

    #[test]
    fn a_failed_release_step_does_not_skip_later_steps() {
        let mut backend = FakeBackend::new(two_monitors());
        backend.release_error = Some(ReleaseStep::Hotkeys);
        let mut controller = HostController::new(backend);

        let report = controller.shutdown();
        assert_eq!(
            report.released,
            vec![ReleaseStep::NativeHandles, ReleaseStep::WorkerChannels]
        );
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].step, ReleaseStep::Hotkeys);
        assert!(!report.is_complete());
    }

    #[test]
    fn showing_while_visible_remains_visible_and_replaces_the_invocation() {
        let mut controller = HostController::new(FakeBackend::new(two_monitors()));
        let first = controller.show(request()).expect("first show");
        let second = controller.show(request()).expect("second show");

        assert_eq!(controller.state(), LifecycleState::Visible);
        assert_ne!(first.invocation, second.invocation);
        assert_eq!(controller.active_invocation(), Some(second.invocation));
        assert!(matches!(
            controller.hide(&first.invocation),
            Err(HostError::StaleInvocation { .. })
        ));
        assert_eq!(controller.state(), LifecycleState::Visible);
        controller.hide(&second.invocation).expect("current hide");
        assert_eq!(controller.state(), LifecycleState::Hidden);
    }

    #[test]
    fn an_old_invocation_cannot_dismiss_the_current_window() {
        let mut controller = HostController::new(FakeBackend::new(two_monitors()));
        let first = controller.show(request()).expect("first show");
        controller.hide(&first.invocation).expect("first hide");
        let second = controller.show(request()).expect("second show");
        assert_ne!(first.invocation, second.invocation);

        let error = controller
            .hide(&first.invocation)
            .expect_err("old invocation must not dismiss");
        match error {
            HostError::StaleInvocation { expected, received } => {
                assert_eq!(expected, second.invocation);
                assert_eq!(received, first.invocation);
            }
            other => panic!("expected stale invocation, got {other:?}"),
        }
        assert_eq!(controller.state(), LifecycleState::Visible);
        assert_eq!(
            controller.request_focus(&first.invocation),
            Err(HostError::StaleInvocation {
                expected: second.invocation,
                received: first.invocation
            })
        );

        controller.hide(&second.invocation).expect("current hide");
        assert_eq!(controller.state(), LifecycleState::Hidden);
    }

    #[test]
    fn hide_is_idempotent_for_the_current_invocation() {
        let mut controller = HostController::new(FakeBackend::new(two_monitors()));
        let report = controller.show(request()).expect("show");
        let first = controller.hide(&report.invocation).expect("hide");
        let second = controller.hide(&report.invocation).expect("hide again");
        assert!(!first.already_hidden);
        assert!(second.already_hidden);
        assert_eq!(
            controller
                .backend()
                .calls
                .iter()
                .filter(|call| matches!(call, Call::Hide))
                .count(),
            1,
            "the second hide must not call the backend"
        );
    }

    #[test]
    fn hide_with_runs_the_caller_effect_only_after_the_invocation_check() {
        let mut controller = HostController::new(FakeBackend::new(two_monitors()));
        let first = controller.show(request()).expect("first");
        let second = controller.show(request()).expect("second");
        let mut ran = false;

        let error = controller
            .hide_with(&first.invocation, || {
                ran = true;
                Ok::<_, HostError>(())
            })
            .expect_err("stale invocation");
        assert!(matches!(error, HostError::StaleInvocation { .. }));
        assert!(
            !ran,
            "the native effect must not run for a stale invocation"
        );
        assert_eq!(controller.state(), LifecycleState::Visible);

        let (_report, value) = controller
            .hide_with(&second.invocation, || {
                ran = true;
                Ok::<_, HostError>(42)
            })
            .expect("current hide");
        assert_eq!(value, Some(42));
        assert!(ran);
        assert_eq!(controller.state(), LifecycleState::Hidden);
    }

    #[test]
    fn reapplying_placement_keeps_the_invocation_and_does_not_refocus() {
        let mut controller = HostController::new(FakeBackend::new(two_monitors()));
        let report = controller.show(request()).expect("show");
        let calls_before = controller.backend().calls.len();

        let rect = controller
            .reapply_placement(Size::new(900, 700))
            .expect("placement")
            .expect("visible launcher");
        assert_eq!(
            rect,
            Rect::new((1920 - 900) / 2, (1040 - 700) / 2, 900, 700)
        );
        assert_eq!(controller.active_invocation(), Some(report.invocation));
        assert_eq!(controller.placed_rect(), Some(rect));
        let new_calls = &controller.backend().calls[calls_before..];
        assert!(matches!(new_calls.last(), Some(Call::Place(_))));
        assert!(
            !new_calls.iter().any(|call| matches!(call, Call::Show(_))),
            "reapplying placement must not re-show or refocus"
        );
    }

    #[test]
    fn reapplying_placement_survives_a_removed_monitor_without_a_new_invocation() {
        let mut controller = HostController::new(FakeBackend::new(two_monitors()));
        let report = controller.show(request()).expect("show");

        controller.backend_mut().monitors = vec![monitor("secondary", 1920, true)];
        let rect = controller
            .reapply_placement(Size::new(800, 600))
            .expect("placement")
            .expect("visible launcher");
        assert_eq!(
            controller.current_monitor(),
            Some(&MonitorId::new("secondary"))
        );
        assert_eq!(controller.active_invocation(), Some(report.invocation));
        assert!(
            monitor("secondary", 1920, true)
                .work_area
                .contains_rect(rect)
        );
    }

    #[test]
    fn adopting_an_existing_window_issues_an_invocation_that_can_be_used() {
        let mut controller = HostController::new(FakeBackend::new(two_monitors()));
        let invocation = controller.adopt_visible().expect("adopt");
        assert_eq!(controller.state(), LifecycleState::Visible);
        assert_eq!(controller.active_invocation(), Some(invocation));
        assert!(!controller.hide(&invocation).expect("hide").already_hidden);
    }
}
