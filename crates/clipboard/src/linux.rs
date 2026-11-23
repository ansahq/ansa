//! Session dispatch. Never substitute XWayland for a Wayland clipboard.

mod wayland;
mod x11;

use std::io;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::io::Errno;
use rustix::net::{self, AddressFamily, SocketAddrUnix, SocketFlags, SocketType};

use crate::{
    CaptureCapabilities, CapturePolicy, CaptureSupport, ClipboardError, ClipboardSnapshot,
    ClipboardTransport,
};

const DEADLINE: Duration = Duration::from_secs(1);
const X11_LIMITS: &str = "local X11 with XFixes and standard selection targets";
const WAYLAND_LIMITS: &str =
    "one seat with ext-data-control or wlr-data-control; source app unknown";
const LOCAL_FILES: &str = "only bounded text/uri-list offers with local file URLs";

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

enum Session {
    // Both connection states are large; keep the enum small because it lives
    // inside a global mutex.
    X11(Box<x11::Reader>),
    Wayland(Box<wayland::Reader>),
}

fn transport() -> ClipboardTransport {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    if session.eq_ignore_ascii_case("wayland")
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var_os("WAYLAND_SOCKET").is_some()
    {
        ClipboardTransport::Wayland
    } else if session.eq_ignore_ascii_case("x11") || std::env::var_os("DISPLAY").is_some() {
        ClipboardTransport::X11
    } else {
        ClipboardTransport::Unsupported
    }
}

pub(crate) fn capabilities() -> CaptureCapabilities {
    let transport = transport();
    match with_session(transport, |session| match session {
        Session::X11(reader) => reader.probe(),
        Session::Wayland(reader) => reader.probe(),
    }) {
        Ok(()) => CaptureCapabilities::new(
            transport,
            CaptureSupport::Degraded {
                reason: match transport {
                    ClipboardTransport::X11 => X11_LIMITS,
                    ClipboardTransport::Wayland => WAYLAND_LIMITS,
                    _ => unreachable!(),
                },
            },
            CaptureSupport::Degraded {
                reason: LOCAL_FILES,
            },
        ),
        Err(error) => CaptureCapabilities::unavailable(
            transport,
            match error {
                ClipboardError::CaptureUnavailable { reason } => reason,
                _ => "desktop clipboard control could not be verified",
            },
        ),
    }
}

pub(crate) fn read(policy: Option<&CapturePolicy>) -> Result<ClipboardSnapshot, ClipboardError> {
    with_session(transport(), |session| match session {
        Session::X11(reader) => reader.read(policy),
        Session::Wayland(reader) => reader.read(policy),
    })
}

fn with_session<T>(
    transport: ClipboardTransport,
    action: impl FnOnce(&mut Session) -> Result<T, ClipboardError>,
) -> Result<T, ClipboardError> {
    let mut state = SESSION
        .lock()
        .map_err(|_| failed("desktop capture state"))?;
    let matches = matches!(
        (&*state, transport),
        (Some(Session::X11(_)), ClipboardTransport::X11)
            | (Some(Session::Wayland(_)), ClipboardTransport::Wayland)
    );
    if !matches {
        *state = match transport {
            ClipboardTransport::X11 => Some(Session::X11(Box::new(x11::Reader::open()?))),
            ClipboardTransport::Wayland => {
                Some(Session::Wayland(Box::new(wayland::Reader::open()?)))
            }
            _ => return Err(unavailable("no Linux graphical clipboard session detected")),
        };
    }
    let session = state.as_mut().expect("a matching desktop session");
    let result = action(session);
    // Retry the connection on the next call only for a failed native session.
    // Payload refusal, empty offers, races and timeout do not replace the
    // live generation or turn an unchanged selection into a new item.
    let dead = match session {
        Session::X11(reader) => reader.dead(),
        Session::Wayland(reader) => reader.dead(),
    };
    if dead {
        *state = None;
    }
    result
}

fn failed(operation: &'static str) -> ClipboardError {
    ClipboardError::CaptureFailed { operation }
}

fn unavailable(reason: &'static str) -> ClipboardError {
    ClipboardError::CaptureUnavailable { reason }
}

fn poll_until(fds: &mut [PollFd<'_>], deadline: Instant) -> io::Result<()> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let timeout = Timespec {
            tv_sec: remaining.as_secs().try_into().unwrap_or(i64::MAX),
            tv_nsec: remaining.subsec_nanos().into(),
        };
        match poll(fds, Some(&timeout)) {
            Ok(0) => return Err(io::ErrorKind::TimedOut.into()),
            Ok(_) => return Ok(()),
            Err(Errno::INTR) => continue,
            Err(error) => return Err(error.into()),
        }
    }
}

fn connect_unix(address: &SocketAddrUnix, deadline: Instant) -> io::Result<UnixStream> {
    let socket = net::socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
        None,
    )?;
    match net::connect(&socket, address) {
        Ok(()) => {}
        Err(Errno::INPROGRESS) => {
            poll_until(&mut [PollFd::new(&socket, PollFlags::OUT)], deadline)?;
            net::sockopt::socket_error(&socket)??;
        }
        // AF_UNIX reports EAGAIN instead of waiting on a full accept queue.
        Err(error) => return Err(error.into()),
    }
    Ok(UnixStream::from(socket))
}

fn wait(fd: &impl AsFd, flags: PollFlags, deadline: Instant) -> io::Result<()> {
    poll_until(&mut [PollFd::new(fd, flags)], deadline)
}
