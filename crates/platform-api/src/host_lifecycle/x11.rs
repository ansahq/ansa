use super::x11_transport::{connect, X11_TIMEOUT};
use super::WorkArea;
use std::time::Instant;
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};

pub(super) fn work_area() -> Result<WorkArea, &'static str> {
    let (conn, screen) = connect(Instant::now() + X11_TIMEOUT)
        .map_err(|_| "Could not connect to the X11 server before the deadline")?;
    let root = conn.setup().roots[screen].root;
    let property = |name: &[u8], offset: u32, length: u32| -> Option<Vec<u32>> {
        let atom = conn.intern_atom(true, name).ok()?.reply().ok()?.atom;
        if atom == 0 {
            return None;
        }
        let reply = conn
            .get_property(false, root, atom, AtomEnum::CARDINAL, offset, length)
            .ok()?
            .reply()
            .ok()?;
        let values = reply.value32()?.collect();
        Some(values)
    };
    let desktop = property(b"_NET_CURRENT_DESKTOP", 0, 1)
        .and_then(|value| value.first().copied())
        .ok_or("The X11 window manager does not report the active desktop")?;
    let coordinates = property(
        b"_NET_WORKAREA",
        desktop.checked_mul(4).ok_or("Invalid X11 desktop index")?,
        4,
    )
    .ok_or("The X11 window manager does not report a desktop work area")?;
    let &[x, y, width, height] = coordinates.as_slice() else {
        return Err("Incomplete X11 desktop work area");
    };
    // EWMH uses CARDINAL even though a desktop can have a negative origin.
    let desktop = WorkArea {
        x: f64::from(x as i32),
        y: f64::from(y as i32),
        width: f64::from(width),
        height: f64::from(height),
    };
    let pointer = conn
        .query_pointer(root)
        .map_err(|_| "Could not query the X11 pointer")?
        .reply()
        .map_err(|_| "Could not read the X11 pointer")?;
    let monitors = conn
        .randr_get_monitors(root, true)
        .map_err(|_| "RandR monitor geometry is unavailable")?
        .reply()
        .map_err(|_| "RandR 1.5 monitor geometry is unavailable")?;
    let monitor = monitors
        .monitors
        .iter()
        .find(|monitor| {
            let dx = i32::from(pointer.root_x) - i32::from(monitor.x);
            let dy = i32::from(pointer.root_y) - i32::from(monitor.y);
            pointer.same_screen
                && dx >= 0
                && dx < i32::from(monitor.width)
                && dy >= 0
                && dy < i32::from(monitor.height)
        })
        .or_else(|| monitors.monitors.iter().find(|monitor| monitor.primary))
        .ok_or("No X11 monitor matched the pointer and no primary monitor was reported")?;
    super::intersect_work_area(desktop, monitor.x, monitor.y, monitor.width, monitor.height)
        .ok_or("The X11 desktop work area does not intersect the selected monitor")
}
