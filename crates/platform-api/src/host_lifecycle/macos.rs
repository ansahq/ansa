//! Borrow the GPUI NSView on the AppKit thread.

use super::{PositionOutcome, WorkArea};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSEvent, NSScreen, NSView, NSWindow};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use raw_window_handle::{RawWindowHandle, WindowHandle};

impl From<NSRect> for WorkArea {
    fn from(rect: NSRect) -> Self {
        Self {
            x: rect.origin.x,
            y: rect.origin.y,
            width: rect.size.width,
            height: rect.size.height,
        }
    }
}

fn with_window<T>(
    handle: WindowHandle<'_>,
    operation: impl FnOnce(MainThreadMarker, &NSWindow) -> T,
) -> Result<T, &'static str> {
    let mtm =
        MainThreadMarker::new().ok_or("AppKit launcher operations must run on the main thread")?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return Err("The window does not expose a live AppKit view");
    };
    // SAFETY: the borrowed AppKit handle is a live NSView valid for this call;
    // MainThreadMarker verified thread affinity before touching it.
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let window = view
        .window()
        .ok_or("The launcher view is detached from its AppKit window")?;
    Ok(operation(mtm, &window))
}

pub(super) fn set_visible(handle: WindowHandle<'_>, visible: bool) -> Result<(), &'static str> {
    with_window(handle, |_, window| {
        if visible {
            window.orderFront(None);
        } else {
            window.orderOut(None);
        }
        if window.isVisible() == visible {
            Ok(())
        } else {
            Err("AppKit did not report the requested window visibility")
        }
    })?
}

pub(super) fn is_focused(handle: WindowHandle<'_>) -> Result<bool, &'static str> {
    with_window(handle, |mtm, window| {
        window.isVisible()
            && window.isKeyWindow()
            && NSApplication::sharedApplication(mtm).isActive()
    })
}

pub(super) fn position(handle: WindowHandle<'_>) -> PositionOutcome {
    with_window(handle, |mtm, window| {
        let mouse = NSEvent::mouseLocation();
        let screens = NSScreen::screens(mtm);
        let screen = (0..screens.count().min(64))
            .map(|i| screens.objectAtIndex(i))
            .find(|candidate| {
                let area = WorkArea::from(candidate.frame());
                mouse.x >= area.x
                    && mouse.x < area.x + area.width
                    && mouse.y >= area.y
                    && mouse.y < area.y + area.height
            })
            .or_else(|| window.screen())
            .or_else(|| NSScreen::mainScreen(mtm));
        let Some(screen) = screen else {
            return PositionOutcome::Degraded("AppKit did not report a screen");
        };
        let work = WorkArea::from(screen.visibleFrame());
        let frame = window.frame();
        let minimum = window.minSize();
        if minimum.width > work.width || minimum.height > work.height {
            return PositionOutcome::Degraded(
                "The screen work area is smaller than the launcher minimum",
            );
        }
        let Some(target) = work.fit_centered(frame.size.width, frame.size.height) else {
            return PositionOutcome::Degraded("AppKit reported invalid screen or window bounds");
        };
        window.setFrame_display(
            NSRect::new(
                NSPoint::new(target.x, target.y),
                NSSize::new(target.width, target.height),
            ),
            true,
        );
        let actual = WorkArea::from(window.frame());
        if actual.valid()
            && (actual.x - target.x).abs() <= 0.5
            && (actual.y - target.y).abs() <= 0.5
            && (actual.width - target.width).abs() <= 0.5
            && (actual.height - target.height).abs() <= 0.5
        {
            PositionOutcome::Verified(actual)
        } else {
            PositionOutcome::Degraded("AppKit did not report the requested centered launcher frame")
        }
    })
    .unwrap_or_else(PositionOutcome::Unsupported)
}
