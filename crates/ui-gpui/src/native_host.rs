//! Native summon policy for the macOS and Linux GPUI window.
//!
//! Native callbacks send an intent only. Window operations and focus
//! observation run on the GPUI thread; logical text focus is not OS focus.

use ansa_onboarding::ProbeOutcome;
use ansa_platform_api::{
    host_lifecycle::{FocusOutcome, HostBackend, HostLifecycle, PositionOutcome},
    hotkey::{HotkeyBinding, HotkeyError, HotkeyRegistration, HotkeyStatus},
};
use gpui::{App, Context, Focusable, Window, WindowHandle};
use std::time::{Duration, Instant};

use crate::{Launcher, compat};

pub(crate) struct NativeHost {
    pub(crate) lifecycle: HostLifecycle,
    registration: Option<HotkeyRegistration>,
    registration_error: Option<HotkeyError>,
    focus: Option<FocusOutcome>,
    focus_request: u64,
    position: PositionOutcome,
}

impl NativeHost {
    pub(crate) fn new() -> Self {
        let lifecycle = HostLifecycle::current();
        Self {
            lifecycle,
            registration: None,
            registration_error: None,
            focus: None,
            focus_request: 0,
            position: lifecycle.positioning(),
        }
    }

    pub(crate) fn hotkey_registered(&self) -> bool {
        self.registration
            .as_ref()
            .is_some_and(|registration| registration.status() == HotkeyStatus::Registered)
    }

    pub(crate) fn shortcut_probe(&self) -> ProbeOutcome {
        if self.hotkey_registered() {
            ProbeOutcome::Available
        } else {
            match &self.registration_error {
                Some(HotkeyError::Unsupported(_)) => ProbeOutcome::Unsupported,
                Some(HotkeyError::Conflict(_)) => ProbeOutcome::Conflict {
                    source: "The desktop or another application".into(),
                },
                _ => ProbeOutcome::Denied {
                    reason: self
                        .hotkey_notice()
                        .unwrap_or_else(|| "Global shortcut registration has not succeeded".into()),
                },
            }
        }
    }

    pub(crate) fn hotkey_notice(&self) -> Option<String> {
        let chord = HotkeyBinding::DEFAULT.display(self.lifecycle.backend);
        if let Some(error) = &self.registration_error {
            return Some(format!("{chord} unavailable: {error}"));
        }
        match self.registration.as_ref().map(HotkeyRegistration::status) {
            Some(HotkeyStatus::Degraded(reason) | HotkeyStatus::Unsupported(reason)) => {
                Some(format!("{chord} unavailable: {reason}"))
            }
            Some(HotkeyStatus::Suspended) => {
                Some(format!("{chord} suspended — use the desktop window"))
            }
            Some(HotkeyStatus::Registered) | None => None,
        }
    }

    pub(crate) fn dismiss_fallback(&self) -> &'static str {
        if self.hotkey_registered() {
            "This session cannot safely hide the launcher; use the window switcher or Ctrl+Q"
        } else {
            "Global summon unavailable — use the desktop window or Ctrl+Q to quit"
        }
    }

    pub(crate) fn summon_detail(&self) -> String {
        let backend = self.lifecycle.backend;
        let chord = HotkeyBinding::DEFAULT.display(backend);
        let route = self.hotkey_notice().unwrap_or_else(|| {
            if self.hotkey_registered() {
                let use_for = if self.lifecycle.can_hide_retained() {
                    "show/hide"
                } else {
                    "show/focus only"
                };
                format!("{chord} registered ({use_for})")
            } else {
                "No live global shortcut".into()
            }
        });
        let focus = match self.focus {
            Some(FocusOutcome::Verified) => "keyboard focus observed",
            Some(FocusOutcome::Denied) => "keyboard focus not observed before the deadline",
            Some(FocusOutcome::Unsupported) => "programmatic focus unsupported",
            _ if matches!(
                backend,
                HostBackend::Wayland | HostBackend::Headless | HostBackend::Other
            ) =>
            {
                "programmatic focus unsupported"
            }
            _ => "no confirmed programmatic focus result",
        };
        let position = match self.position {
            PositionOutcome::Verified(_) => "launcher observed inside the selected work area",
            PositionOutcome::Unsupported(reason) | PositionOutcome::Degraded(reason) => reason,
            PositionOutcome::Skipped => "no native placement result",
        };
        format!(
            "{} · {route}. Focus: {focus}. Placement: {position}. Use the desktop window if summon is unavailable. Shortcut editing and verified focus restoration are not wired on this desktop.",
            backend.label(),
        )
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn set_macos_visible(
        &self,
        window: &Window,
        visible: bool,
    ) -> Result<(), &'static str> {
        use raw_window_handle::HasWindowHandle;
        // GPUI has an inherent Window::window_handle that returns
        // AnyWindowHandle, so name the raw-window-handle trait explicitly.
        self.lifecycle.set_macos_visible(
            HasWindowHandle::window_handle(window)
                .map_err(|_| "AppKit window handle unavailable")?,
            visible,
        )
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn clear_focus(&mut self) {
        self.focus_request = self.focus_request.wrapping_add(1);
        self.focus = None;
    }

    fn is_focused(&self, window: &Window) -> bool {
        #[cfg(target_os = "macos")]
        {
            use raw_window_handle::HasWindowHandle;
            HasWindowHandle::window_handle(window)
                .ok()
                .and_then(|handle| self.lifecycle.macos_is_focused(handle).ok())
                .unwrap_or(false)
        }
        #[cfg(not(target_os = "macos"))]
        {
            window.is_window_active()
        }
    }
}

fn request_focus(
    this: &mut Launcher,
    window: &mut Window,
    cx: &mut Context<Launcher>,
) -> Option<u64> {
    if !matches!(
        this.native_host.lifecycle.backend,
        HostBackend::MacOs | HostBackend::X11
    ) {
        // No Wayland activation token from a user-mediated portal route.
        this.native_host.focus = Some(FocusOutcome::Unsupported);
        cx.notify();
        return None;
    }
    #[cfg(target_os = "macos")]
    {
        use raw_window_handle::HasWindowHandle;
        if !this.visible {
            if let Err(error) = this.native_host.set_macos_visible(window, true) {
                this.status = format!("Cannot show launcher: {error}");
                cx.notify();
                return None;
            }
            this.visible = true;
        }
        this.native_host.position = match HasWindowHandle::window_handle(window) {
            Ok(handle) => this.native_host.lifecycle.position_macos(handle),
            Err(_) => PositionOutcome::Unsupported("AppKit window handle unavailable"),
        };
        cx.activate(true);
    }
    this.engine.begin_invocation();
    this.native_host.focus_request = this.native_host.focus_request.wrapping_add(1);
    this.native_host.focus = None;
    window.activate_window();
    let focus = if this.setup.is_some() {
        this.setup_focus.clone()
    } else {
        this.input.focus_handle(cx)
    };
    compat::focus(window, &focus, cx);
    cx.notify();
    Some(this.native_host.focus_request)
}

async fn observe_focus(handle: WindowHandle<Launcher>, request: u64, cx: &mut gpui::AsyncApp) {
    // AppKit and X11 can report the change on a later event-loop turn.
    // Never treat the activation request itself or an elapsed timer as proof.
    for attempt in 1..=4 {
        cx.background_executor()
            .timer(Duration::from_millis(75))
            .await;
        let done = handle.update(cx, |this, window, cx| {
            if this.native_host.focus_request != request {
                return true;
            }
            let active = this.native_host.is_focused(window);
            if active || attempt == 4 {
                let outcome = this.native_host.lifecycle.focus_outcome(true, active);
                this.native_host.focus = Some(outcome);
                if outcome != FocusOutcome::Verified {
                    this.status =
                        "Launcher focus was not observed — select its desktop window".into();
                }
                cx.notify();
                true
            } else {
                false
            }
        });
        if !matches!(done, Ok(false)) {
            break;
        }
    }
}

pub(crate) fn open_from_desktop(cx: &mut App, handle: WindowHandle<Launcher>) {
    if let Ok(Some(request)) = handle.update(cx, request_focus) {
        cx.spawn(async move |cx| observe_focus(handle, request, cx).await)
            .detach();
    }
}

pub(crate) fn install_summon(cx: &mut App, handle: WindowHandle<Launcher>) {
    let (sender, receiver) = async_channel::bounded(1);
    let registration = HotkeyRegistration::register_binding(HotkeyBinding::DEFAULT, move |event| {
        let _ = sender.try_send((Instant::now(), event));
    });
    let _ = handle.update(cx, |this, _, cx| {
        match registration {
            Ok(registration) => this.native_host.registration = Some(registration),
            Err(error) => this.native_host.registration_error = Some(error),
        }
        cx.notify();
    });
    cx.spawn(async move |cx| {
        while let Ok((captured_at, event)) = receiver.recv().await {
            if captured_at.elapsed() > Duration::from_secs(1) {
                continue;
            }
            let request = handle.update(cx, |this, window, cx| {
                if !this
                    .native_host
                    .registration
                    .as_ref()
                    .is_some_and(|host| host.accepts(event))
                {
                    return None;
                }
                #[cfg(target_os = "macos")]
                if this.visible && this.native_host.is_focused(window) {
                    this.dismiss(true, window, cx);
                    return None;
                }
                request_focus(this, window, cx)
            });
            match request {
                Ok(Some(request)) => observe_focus(handle, request, cx).await,
                Ok(None) => {}
                Err(_) => break,
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simulated_host(backend: HostBackend) -> NativeHost {
        let lifecycle = HostLifecycle { backend };
        NativeHost {
            lifecycle,
            registration: None,
            registration_error: None,
            focus: None,
            focus_request: 0,
            position: lifecycle.positioning(),
        }
    }

    #[test]
    fn wayland_reports_unsupported_without_registration_or_focus_claim() {
        let mut host = simulated_host(HostBackend::Wayland);
        host.registration_error = Some(HotkeyError::Unsupported("No portal client"));
        assert_eq!(host.shortcut_probe(), ProbeOutcome::Unsupported);
        assert!(!host.hotkey_registered());
        assert!(host.dismiss_fallback().contains("desktop window"));
        let detail = host.summon_detail();
        assert!(detail.contains("programmatic focus unsupported"));
        assert!(detail.contains("No portal client"));
    }

    #[test]
    fn failed_appkit_registration_keeps_desktop_fallback() {
        let mut host = simulated_host(HostBackend::MacOs);
        host.registration_error = Some(HotkeyError::Conflict(HotkeyBinding::DEFAULT));
        assert!(matches!(
            host.shortcut_probe(),
            ProbeOutcome::Conflict { .. }
        ));
        assert!(!host.hotkey_registered());
        assert!(host.dismiss_fallback().contains("desktop window"));
        assert!(!host.summon_detail().contains("keyboard focus observed"));
    }
}
