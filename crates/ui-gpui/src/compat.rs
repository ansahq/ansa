//! Narrow adapters for GPUI API changes between the crates.io 0.2.2 pin and
//! the immutable upstream accessibility candidate.
//!
//! Only signatures that actually moved are wrapped here. Everything else in
//! the launcher stays on the common API so both renderers run the same shell
//! logic and the same interaction smoke test.

use gpui::{App, FocusHandle, Subscription, Window};

/// Move keyboard focus, with the platform argument required by the candidate.
#[cfg(not(feature = "upstream-candidate"))]
pub(crate) fn focus(window: &mut Window, handle: &FocusHandle, _cx: &mut App) {
    window.focus(handle);
}

/// Move keyboard focus, with the platform argument required by the candidate.
#[cfg(feature = "upstream-candidate")]
pub(crate) fn focus(window: &mut Window, handle: &FocusHandle, cx: &mut App) {
    window.focus(handle, cx);
}

/// Quit once the last launcher window has closed.
#[cfg(not(feature = "upstream-candidate"))]
pub(crate) fn quit_when_last_window_closes(cx: &App) -> Subscription {
    cx.on_window_closed(|cx| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
}

/// Quit once the last launcher window has closed.
#[cfg(feature = "upstream-candidate")]
pub(crate) fn quit_when_last_window_closes(cx: &App) -> Subscription {
    cx.on_window_closed(|cx, _window_id| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
}
