//! Bounded local X11 socket setup and reply reads. Never connects over TCP.

use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::fd::AsFd;
use rustix::io::Errno;
use rustix::net::{self, AddressFamily, SocketAddrUnix, SocketFlags, SocketType};
use std::cell::Cell;
use std::io::{Error, ErrorKind, IoSlice, Result};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use x11rb::rust_connection::{DefaultStream, PollMode, RustConnection, Stream};
use x11rb::utils::RawFdContainer;

pub(crate) const X11_TIMEOUT: Duration = Duration::from_millis(500);

pub(crate) struct DeadlineStream {
    inner: DefaultStream,
    deadline: Cell<Instant>,
}

impl DeadlineStream {
    pub(crate) fn set_deadline(&self, deadline: Instant) {
        self.deadline.set(deadline);
    }
}

fn remaining(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|time| !time.is_zero())
        .ok_or_else(|| Error::new(ErrorKind::TimedOut, "Timed out waiting for the X11 server"))
}

fn ready(fd: &impl AsFd, flags: PollFlags, deadline: Instant) -> Result<()> {
    let mut fds = [PollFd::new(fd, flags)];
    loop {
        let time = remaining(deadline)?;
        let timeout = Timespec {
            tv_sec: time.as_secs() as _,
            tv_nsec: time.subsec_nanos().into(),
        };
        match poll(&mut fds, Some(&timeout)) {
            Ok(0) | Err(Errno::INTR) => continue,
            Ok(_) => return Ok(()), // Required actual read/write reports HUP/error.
            Err(other) => return Err(other.into()),
        }
    }
}

impl Stream for DeadlineStream {
    fn poll(&self, mode: PollMode) -> Result<()> {
        let mut flags = PollFlags::empty();
        if mode.readable() {
            flags |= PollFlags::IN;
        }
        if mode.writable() {
            flags |= PollFlags::OUT;
        }
        ready(&self.inner, flags, self.deadline.get())
    }

    fn read(&self, buf: &mut [u8], fds: &mut Vec<RawFdContainer>) -> Result<usize> {
        remaining(self.deadline.get())?;
        self.inner.read(buf, fds)
    }

    fn write(&self, buf: &[u8], fds: &mut Vec<RawFdContainer>) -> Result<usize> {
        remaining(self.deadline.get())?;
        self.inner.write(buf, fds)
    }

    fn write_vectored(&self, bufs: &[IoSlice<'_>], fds: &mut Vec<RawFdContainer>) -> Result<usize> {
        remaining(self.deadline.get())?;
        self.inner.write_vectored(bufs, fds)
    }
}

fn connect_socket(address: &SocketAddrUnix, deadline: Instant) -> Result<UnixStream> {
    remaining(deadline)?;
    let fd = net::socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
        None,
    )?;
    match net::connect(&fd, address) {
        Ok(()) => {}
        Err(Errno::INPROGRESS) => {
            ready(&fd, PollFlags::OUT, deadline)?;
            net::sockopt::socket_error(&fd)??;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(UnixStream::from(fd))
}

fn local_display(display: &str) -> Option<(u16, usize)> {
    let local = display
        .strip_prefix(':')
        .or_else(|| display.strip_prefix("unix:"))?;
    let (display, screen) = local.split_once('.').unwrap_or((local, "0"));
    Some((
        display.parse().ok()?,
        usize::from(screen.parse::<u16>().ok()?),
    ))
}

pub(crate) fn connect(deadline: Instant) -> Result<(RustConnection<DeadlineStream>, usize)> {
    let display = std::env::var("DISPLAY")
        .ok()
        .and_then(|s| local_display(&s));
    let (num, screen) = display.ok_or_else(|| {
        Error::new(
            ErrorKind::Unsupported,
            "Only local Unix-socket X11 displays (:N or unix:N) are supported",
        )
    })?;
    let path = format!("/tmp/.X11-unix/X{num}");
    let unix = connect_socket(
        &SocketAddrUnix::new_abstract_name(path.as_bytes())?,
        deadline,
    )
    .or_else(|_| connect_socket(&SocketAddrUnix::new(path)?, deadline))?;
    let (inner, (family, peer)) = DefaultStream::from_unix_stream(unix)?;
    // Do not knowingly open pipes, sockets, or huge files as authority data.
    // X11 authentication is read from the user's conventional local file.
    let authority = std::env::var_os("XAUTHORITY")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".Xauthority")));
    let (name, data) = if authority
        .and_then(|path| path.metadata().ok())
        .is_some_and(|metadata| metadata.is_file() && metadata.len() <= 1024 * 1024)
    {
        x11rb_protocol::xauth::get_auth(family, &peer, num)
            .ok()
            .flatten()
            .unwrap_or_default()
    } else {
        (Vec::new(), Vec::new())
    };
    let stream = DeadlineStream {
        inner,
        deadline: Cell::new(deadline),
    };
    let connection = RustConnection::connect_to_stream_with_auth_info(stream, screen, name, data)
        .map_err(|e| Error::other(format!("X11 connection failed: {e}")))?;
    Ok((connection, screen))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_remote_displays_without_trying_dns() {
        assert_eq!(local_display(":2.1"), Some((2, 1)));
        assert_eq!(local_display("unix:0"), Some((0, 0)));
        assert_eq!(local_display("remote:0"), None);
        assert_eq!(local_display("localhost:10.0"), None);
        assert_eq!(local_display(":0.0.0"), None);
    }

    #[test]
    fn unresponsive_authenticated_transport_times_out() {
        let (client, _unresponsive_peer) = UnixStream::pair().unwrap();
        let (inner, _) = DefaultStream::from_unix_stream(client).unwrap();
        let stream = DeadlineStream {
            inner,
            deadline: Cell::new(Instant::now() + Duration::from_millis(10)),
        };
        let result = RustConnection::connect_to_stream(stream, 0);
        assert!(
            matches!(result, Err(x11rb::errors::ConnectError::IoError(ref e))
            if e.kind() == ErrorKind::TimedOut)
        );
    }

    #[test]
    fn expired_deadline_read_returns_timeout_instead_of_escaping() {
        let (client, _unresponsive_peer) = UnixStream::pair().unwrap();
        let (inner, _) = DefaultStream::from_unix_stream(client).unwrap();
        let stream = DeadlineStream {
            inner,
            deadline: Cell::new(Instant::now()),
        };
        let mut buffer = [0u8; 1];
        let mut descriptors = Vec::new();
        let result = stream.read(&mut buffer, &mut descriptors);
        assert!(
            matches!(result, Err(ref error) if error.kind() == ErrorKind::TimedOut),
            "unexpected read result: {result:?}"
        );
    }
}
