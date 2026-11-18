//! Monitor identity, selection policy and physical-pixel placement.
//!
//! The policy is deliberately pure: the platform adapter supplies the current
//! monitor list (including each monitor's physical work area and scale factor)
//! and this module decides which monitor should host the launcher and where the
//! launcher should land. A monitor that is no longer in the supplied list is
//! never returned.

use crate::geometry::{Point, Rect, Size};

/// Stable-ish identity of a monitor as reported by the platform adapter.
///
/// Windows builds this from the display device name plus the monitor rectangle,
/// so it survives work-area changes such as a taskbar resize but changes when a
/// monitor is physically replaced. It is an identifier, not a security token.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonitorId(String);

impl MonitorId {
    /// Creates a monitor id from an adapter-supplied string.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Returns the underlying identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for MonitorId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One monitor's physical work area and effective scale.
#[derive(Clone, Debug, PartialEq)]
pub struct MonitorInfo {
    /// Adapter-supplied identity.
    pub id: MonitorId,
    /// Work area in physical pixels, excluding taskbars and docked app bars.
    pub work_area: Rect,
    /// Effective scale factor (1.0 = 96 DPI, 1.5 = 144 DPI, 2.0 = 192 DPI).
    pub scale_factor: f64,
    /// Whether the platform marks this monitor as primary.
    pub primary: bool,
}

/// Why [`select_monitor`] chose a monitor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonitorSelectionReason {
    /// The pointer was inside this monitor's work area.
    Pointer,
    /// The current launcher rectangle intersected this monitor the most.
    CurrentWindow,
    /// No better hint existed, so the previously used monitor was retained.
    Remembered,
    /// The monitor is marked primary.
    Primary,
    /// The platform supplied no primary flag; the first entry was used.
    First,
}

impl MonitorSelectionReason {
    /// Stable label for diagnostics.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Pointer => "pointer",
            Self::CurrentWindow => "current-window",
            Self::Remembered => "remembered",
            Self::Primary => "primary",
            Self::First => "first",
        }
    }
}

/// The monitor selected by [`select_monitor`].
#[derive(Clone, Debug, PartialEq)]
pub struct MonitorSelection {
    /// The selected monitor.
    pub info: MonitorInfo,
    /// Why it was selected.
    pub reason: MonitorSelectionReason,
}

/// Selects the monitor that should host the launcher.
///
/// Priority:
/// 1. the work area containing the pointer, when a pointer is supplied;
/// 2. the work area with the largest intersection with the current launcher
///    rectangle, when one is supplied;
/// 3. the platform's primary monitor;
/// 4. the remembered monitor, but only if it is still present and the platform
///    reported no primary;
/// 5. the first monitor, for platforms that report no primary.
///
/// A remembered monitor that has been removed is ignored, never returned.
/// Returns `None` only when the supplied monitor list is empty.
pub fn select_monitor(
    monitors: &[MonitorInfo],
    pointer: Option<Point>,
    current_window: Option<Rect>,
    remembered: Option<&MonitorId>,
) -> Option<MonitorSelection> {
    if monitors.is_empty() {
        return None;
    }

    let choose = |info: &MonitorInfo, reason| {
        Some(MonitorSelection {
            info: info.clone(),
            reason,
        })
    };

    if let Some(pointer) = pointer
        && let Some(info) = monitors
            .iter()
            .find(|monitor| monitor.work_area.contains_point(pointer))
    {
        return choose(info, MonitorSelectionReason::Pointer);
    }

    if let Some(window) = current_window
        && let Some(info) = monitors
            .iter()
            .filter(|monitor| monitor.work_area.intersects(window))
            .max_by_key(|monitor| monitor.work_area.intersection_area(window))
    {
        return choose(info, MonitorSelectionReason::CurrentWindow);
    }

    if let Some(info) = monitors.iter().find(|monitor| monitor.primary) {
        return choose(info, MonitorSelectionReason::Primary);
    }

    if let Some(remembered) = remembered
        && let Some(info) = monitors.iter().find(|monitor| &monitor.id == remembered)
    {
        return choose(info, MonitorSelectionReason::Remembered);
    }

    choose(&monitors[0], MonitorSelectionReason::First)
}

/// Centers `desired_size` in the monitor work area and clamps it fully inside.
///
/// `desired_size` is already in physical pixels (use [`Size::scaled`] first).
/// The result is never larger than the work area, so a tiny work area yields a
/// tiny rectangle instead of an off-screen one. Negative monitor origins are
/// handled with 64-bit arithmetic.
pub fn place_on_monitor(monitor: &MonitorInfo, desired_size: Size) -> Rect {
    let work = monitor.work_area;
    let width = desired_size.width.max(0).min(work.width.max(0));
    let height = desired_size.height.max(0).min(work.height.max(0));
    Rect::new(
        centered_coordinate(work.x, work.width, width),
        centered_coordinate(work.y, work.height, height),
        width,
        height,
    )
}

fn centered_coordinate(origin: i32, extent: i32, size: i32) -> i32 {
    let offset = (i64::from(extent.max(0)) - i64::from(size.max(0))) / 2;
    let coordinate = i64::from(origin) + offset;
    coordinate.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(
        id: &str,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        scale: f64,
        primary: bool,
    ) -> MonitorInfo {
        MonitorInfo {
            id: MonitorId::new(id),
            work_area: Rect::new(x, y, width, height),
            scale_factor: scale,
            primary,
        }
    }

    fn mixed_dpi_monitors() -> Vec<MonitorInfo> {
        vec![
            monitor("primary-100", -1920, 0, 1920, 1040, 1.0, true),
            monitor("laptop-150", 0, 0, 2560, 1400, 1.5, false),
            monitor("external-200", 2560, -400, 3840, 2160, 2.0, false),
        ]
    }

    #[test]
    fn pointer_wins_over_remembered_and_primary() {
        let monitors = mixed_dpi_monitors();
        let selection = select_monitor(
            &monitors,
            Some(Point::new(3000, 100)),
            None,
            Some(&MonitorId::new("primary-100")),
        )
        .expect("monitor");
        assert_eq!(selection.info.id, MonitorId::new("external-200"));
        assert_eq!(selection.reason, MonitorSelectionReason::Pointer);
    }

    #[test]
    fn current_window_wins_when_no_pointer_is_available() {
        let monitors = mixed_dpi_monitors();
        let selection = select_monitor(
            &monitors,
            None,
            Some(Rect::new(-1800, 100, 400, 300)),
            Some(&MonitorId::new("laptop-150")),
        )
        .expect("monitor");
        assert_eq!(selection.info.id, MonitorId::new("primary-100"));
        assert_eq!(selection.reason, MonitorSelectionReason::CurrentWindow);
    }

    #[test]
    fn removed_remembered_monitor_is_never_returned() {
        let mut monitors = mixed_dpi_monitors();
        monitors.retain(|monitor| monitor.id != MonitorId::new("external-200"));
        let selection =
            select_monitor(&monitors, None, None, Some(&MonitorId::new("external-200")))
                .expect("monitor");
        assert_eq!(selection.info.id, MonitorId::new("primary-100"));
        assert_eq!(selection.reason, MonitorSelectionReason::Primary);
    }

    #[test]
    fn primary_is_the_fallback_and_first_wins_without_a_primary_flag() {
        let monitors = mixed_dpi_monitors();
        let selection = select_monitor(&monitors, None, None, None).expect("monitor");
        assert_eq!(selection.info.id, MonitorId::new("primary-100"));
        assert_eq!(selection.reason, MonitorSelectionReason::Primary);

        let no_primary: Vec<_> = monitors
            .into_iter()
            .map(|monitor| MonitorInfo {
                primary: false,
                ..monitor
            })
            .collect();
        let selection = select_monitor(&no_primary, None, None, None).expect("monitor");
        assert_eq!(selection.info.id, MonitorId::new("primary-100"));
        assert_eq!(selection.reason, MonitorSelectionReason::First);
    }

    #[test]
    fn remembered_monitor_is_used_only_when_no_primary_exists() {
        let no_primary: Vec<_> = mixed_dpi_monitors()
            .into_iter()
            .map(|monitor| MonitorInfo {
                primary: false,
                ..monitor
            })
            .collect();
        let selection = select_monitor(
            &no_primary,
            None,
            None,
            Some(&MonitorId::new("external-200")),
        )
        .expect("monitor");
        assert_eq!(selection.info.id, MonitorId::new("external-200"));
        assert_eq!(selection.reason, MonitorSelectionReason::Remembered);
    }

    #[test]
    fn empty_monitor_list_selects_nothing() {
        assert_eq!(select_monitor(&[], None, None, None), None);
    }

    #[test]
    fn placement_centers_and_stays_inside_each_scale() {
        for (index, monitor) in mixed_dpi_monitors().into_iter().enumerate() {
            let logical = Size::new(760, 500);
            let desired = logical.scaled(monitor.scale_factor);
            let rect = place_on_monitor(&monitor, desired);
            assert!(
                monitor.work_area.contains_rect(rect),
                "monitor {index} scale {} produced {rect:?} outside {:?}",
                monitor.scale_factor,
                monitor.work_area
            );
            assert_eq!(rect.width, desired.width);
            assert_eq!(rect.height, desired.height);
            assert_eq!(
                rect.center(),
                monitor.work_area.center(),
                "launcher must be centered on monitor {index}"
            );
        }
    }

    #[test]
    fn tiny_work_area_shrinks_the_launcher_instead_of_overflowing() {
        let tiny = monitor("tiny", 50, 70, 320, 200, 1.0, true);
        let rect = place_on_monitor(&tiny, Size::new(760, 500));
        assert_eq!(rect, Rect::new(50, 70, 320, 200));
        assert!(tiny.work_area.contains_rect(rect));
    }

    #[test]
    fn negative_origin_monitors_are_placed_without_overflow() {
        let left = monitor("left", -2560, -200, 2560, 1000, 1.0, false);
        let rect = place_on_monitor(&left, Size::new(800, 600));
        assert_eq!(rect, Rect::new(-1680, 0, 800, 600));
        assert!(left.work_area.contains_rect(rect));
    }

    #[test]
    fn empty_desired_size_is_still_inside_the_work_area() {
        let work = monitor("work", -100, -100, 10, 10, 1.0, true);
        let rect = place_on_monitor(&work, Size::new(0, 0));
        assert_eq!(rect.width, 0);
        assert_eq!(rect.height, 0);
        assert!(work.work_area.contains_point(Point::new(rect.x, rect.y)));
    }
}
