//! Host window policy and observations. GPUI owns the launcher window, theme
//! subscription, and run loop; this module never transfers a window to a worker.

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos;
#[cfg(target_os = "linux")]
mod x11;
#[cfg(target_os = "linux")]
pub(crate) mod x11_transport;

/// The display backend, not the desktop environment or a support claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostBackend {
    Windows,
    MacOs,
    X11,
    Wayland,
    Headless,
    Other,
}

impl HostBackend {
    /// Detects the session conservatively: Wayland takes precedence over an
    /// XWayland `DISPLAY`, and headless gets no global grabs.
    pub fn current() -> Self {
        #[cfg(target_os = "linux")]
        {
            let value = |key| std::env::var(key).ok();
            linux_backend(
                std::env::var_os("ZED_HEADLESS").is_some(),
                value("XDG_SESSION_TYPE").as_deref(),
                value("WAYLAND_DISPLAY").as_deref(),
                value("DISPLAY").as_deref(),
            )
        }
        #[cfg(target_os = "macos")]
        return Self::MacOs;
        #[cfg(target_os = "windows")]
        return Self::Windows;
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        return Self::Other;
    }

    /// Machine-readable display backend name.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::MacOs => "macos",
            Self::X11 => "x11",
            Self::Wayland => "wayland",
            Self::Headless => "headless",
            Self::Other => "other",
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_backend(
    headless: bool,
    session: Option<&str>,
    wayland_display: Option<&str>,
    x_display: Option<&str>,
) -> HostBackend {
    if headless {
        HostBackend::Headless
    } else if session.is_some_and(|value| value.eq_ignore_ascii_case("wayland"))
        || wayland_display.is_some_and(|display| !display.is_empty())
    {
        HostBackend::Wayland
    } else if x_display.is_some_and(|display| !display.is_empty()) {
        HostBackend::X11
    } else {
        HostBackend::Headless
    }
}

/// Result of a focus request, not of appearance or query-field selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusOutcome {
    /// The launcher was observed to own the native keyboard route.
    Verified,
    /// Focus was requested but not observed by the verification deadline.
    /// This does not distinguish a policy denial from a late compositor reply.
    Denied,
    /// No focus request was intended (for example, passive show).
    Skipped,
    /// This route cannot request focus through the active backend.
    Unsupported,
}

impl FocusOutcome {
    /// Stable label for UI and cycle logs.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Denied => "denied",
            Self::Skipped => "skipped",
            Self::Unsupported => "unsupported",
        }
    }
}

/// Host geometry in one coordinate space; do not combine physical X11 pixels
/// with GPUI logical pixels. macOS AppKit geometry is in logical points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorkArea {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl WorkArea {
    fn valid(self) -> bool {
        [
            self.x,
            self.y,
            self.width,
            self.height,
            self.x + self.width,
            self.y + self.height,
        ]
        .into_iter()
        .all(f64::is_finite)
            && self.width > 0.0
            && self.height > 0.0
    }

    /// Fits the requested window within the work area; rejects invalid
    /// geometry. The caller must separately respect the platform minimum size.
    pub fn fit_centered(self, width: f64, height: f64) -> Option<Self> {
        if !self.valid()
            || !width.is_finite()
            || !height.is_finite()
            || width <= 0.0
            || height <= 0.0
        {
            return None;
        }
        let width = width.min(self.width);
        let height = height.min(self.height);
        Some(Self {
            x: self.x + (self.width - width) / 2.0,
            y: self.y + (self.height - height) / 2.0,
            width,
            height,
        })
    }
}

/// Clip an X11 monitor's original rectangle to the current desktop work area.
/// Kept separate from the socket path so clipping can be tested on any host.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn intersect_work_area(
    desktop: WorkArea,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
) -> Option<WorkArea> {
    let left = f64::from(x).max(desktop.x);
    let top = f64::from(y).max(desktop.y);
    let right = (f64::from(x) + f64::from(width)).min(desktop.x + desktop.width);
    let bottom = (f64::from(y) + f64::from(height)).min(desktop.y + desktop.height);
    let usable = WorkArea {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    };
    usable.valid().then_some(usable)
}

/// Outcome of positioning, separate from focus and visibility.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PositionOutcome {
    /// The resulting window frame was read back immediately after the request.
    /// This proves the AppKit frame matched the requested fit at that moment; it
    /// does not promise later Spaces/fullscreen placement, on-screen visibility,
    /// or that a compositor will keep the frame.
    Verified(WorkArea),
    /// Native work-area information was advisory or the result differed.
    Degraded(&'static str),
    /// The user/window manager chose positioning.
    Skipped,
    /// No usable client-side positioning route exists.
    Unsupported(&'static str),
}

/// Snapshot of window-level policy. Sample native geometry again on each
/// summon rather than caching a screen across hot-plug/scale changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostLifecycle {
    pub backend: HostBackend,
}

impl HostLifecycle {
    /// Detects the current display backend.
    pub fn current() -> Self {
        Self {
            backend: HostBackend::current(),
        }
    }

    /// Whether the pinned GPUI integration has a retained hide/show route.
    /// Hiding without a working summon route is never advised.
    pub const fn can_hide_retained(self) -> bool {
        matches!(self.backend, HostBackend::Windows | HostBackend::MacOs)
    }

    /// Classifies an observation after a bounded native focus check. An
    /// activation request, logical text focus, or timer alone is not evidence.
    pub const fn focus_outcome(self, requested: bool, observed_active: bool) -> FocusOutcome {
        if !requested {
            FocusOutcome::Skipped
        } else if observed_active {
            FocusOutcome::Verified
        } else if matches!(
            self.backend,
            HostBackend::Wayland | HostBackend::Headless | HostBackend::Other
        ) {
            FocusOutcome::Unsupported
        } else {
            FocusOutcome::Denied
        }
    }

    /// Default placement disposition when no native resulting bounds have
    /// been read back.
    pub const fn positioning(self) -> PositionOutcome {
        match self.backend {
            HostBackend::Wayland => {
                PositionOutcome::Unsupported("The Wayland compositor places toplevel windows")
            }
            HostBackend::X11 => PositionOutcome::Degraded(
                "X11 work area is advisory; GPUI owns placement and does not expose its window ID",
            ),
            HostBackend::Headless | HostBackend::Other => {
                PositionOutcome::Unsupported("No supported display backend")
            }
            HostBackend::MacOs | HostBackend::Windows => PositionOutcome::Skipped,
        }
    }

    /// Current monitor under the X11 pointer, intersected with the current
    /// desktop's EWMH work area. This is an advisory physical-pixel hint, not
    /// a placement result. Does not connect to XWayland in a Wayland session.
    #[cfg(target_os = "linux")]
    pub fn x11_work_area(self) -> Result<WorkArea, &'static str> {
        if self.backend != HostBackend::X11 || HostBackend::current() != HostBackend::X11 {
            return Err("X11 work area is unavailable outside an X11 session");
        }
        x11::work_area()
    }

    /// Orders the retained launcher in/out, reading visibility back
    /// synchronously on the AppKit thread. Does not activate another
    /// application. Call only with this process's live launcher window handle.
    #[cfg(target_os = "macos")]
    pub fn set_macos_visible(
        self,
        handle: raw_window_handle::WindowHandle<'_>,
        visible: bool,
    ) -> Result<(), &'static str> {
        macos::set_visible(handle, visible)
    }

    /// Centers the retained window on the screen under the pointer (fallback:
    /// current or main screen), inside its current `visibleFrame`. AppKit
    /// decides Spaces/fullscreen behavior. Does not activate the window.
    #[cfg(target_os = "macos")]
    pub fn position_macos(self, handle: raw_window_handle::WindowHandle<'_>) -> PositionOutcome {
        macos::position(handle)
    }

    /// Reads both AppKit key-window and foreground-application state.
    #[cfg(target_os = "macos")]
    pub fn macos_is_focused(
        self,
        handle: raw_window_handle::WindowHandle<'_>,
    ) -> Result<bool, &'static str> {
        macos::is_focused(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wayland_never_becomes_x11_because_xwayland_sets_display() {
        assert_eq!(
            linux_backend(false, Some("wayland"), None, Some(":0")),
            HostBackend::Wayland
        );
        assert_eq!(
            linux_backend(false, None, Some("wayland-0"), Some(":0")),
            HostBackend::Wayland
        );
        assert_eq!(
            linux_backend(false, Some("x11"), None, Some(":0")),
            HostBackend::X11
        );
        assert_eq!(
            linux_backend(true, Some("wayland"), None, Some(":0")),
            HostBackend::Headless
        );
    }

    #[test]
    fn focus_success_requires_observation() {
        let native = HostLifecycle {
            backend: HostBackend::MacOs,
        };
        let wayland = HostLifecycle {
            backend: HostBackend::Wayland,
        };
        assert_eq!(native.focus_outcome(true, true), FocusOutcome::Verified);
        assert_eq!(native.focus_outcome(true, false), FocusOutcome::Denied);
        assert_eq!(native.focus_outcome(false, true), FocusOutcome::Skipped);
        assert_eq!(
            wayland.focus_outcome(true, false),
            FocusOutcome::Unsupported
        );
        assert!(!wayland.can_hide_retained());
        assert!(matches!(
            wayland.positioning(),
            PositionOutcome::Unsupported(_)
        ));
    }

    #[test]
    fn monitor_intersection_is_bounded_and_rejects_non_overlap() {
        let desktop = WorkArea {
            x: -1920.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        assert_eq!(
            intersect_work_area(desktop, -1920, 0, 800, 600),
            Some(WorkArea {
                x: -1920.0,
                y: 0.0,
                width: 800.0,
                height: 600.0
            })
        );
        // A monitor partly outside the desktop work area is clipped, never
        // allowed to produce zero/negative extents.
        assert_eq!(
            intersect_work_area(desktop, -2000, -100, 200, 200),
            Some(WorkArea {
                x: -1920.0,
                y: 0.0,
                width: 120.0,
                height: 100.0
            })
        );
        assert_eq!(intersect_work_area(desktop, 0, 0, 800, 600), None);
        assert_eq!(intersect_work_area(desktop, -1920, 0, 0, 600), None);
    }

    #[test]
    fn fits_negative_and_fractional_coordinates_without_crossing_edges() {
        let monitor = WorkArea {
            x: -1250.5,
            y: 80.25,
            width: 1024.5,
            height: 700.0,
        };
        let frame = monitor.fit_centered(2000.0, 500.0).unwrap();
        assert_eq!(
            frame,
            WorkArea {
                y: 180.25,
                height: 500.0,
                ..monitor
            }
        );
        assert_eq!(monitor.fit_centered(f64::NAN, 500.0), None);
    }
}
