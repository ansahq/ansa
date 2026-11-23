//! Bounded ICCCM selection reader with an XFixes generation monitor.

use std::cell::Cell;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rustix::event::PollFlags;
use rustix::net::SocketAddrUnix;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, GetPropertyReply, Property,
    Window, WindowClass,
};
use x11rb::rust_connection::{DefaultStream, PollMode, RustConnection, Stream};
use x11rb_protocol::{RawFdContainer, parse_display, xauth};

use super::{DEADLINE, connect_unix, failed, unavailable, wait};
use crate::native::offer::{self, Dialect, Format, MAX_BYTES, MAX_OFFER_TYPES, Scratch};
use crate::native::{next_generation, unchanged};
use crate::{CapturePolicy, ClipboardError, ClipboardSnapshot, SourceApp, SourceConfidence};

const MAX_EVENTS: usize = 4096;
const MAX_CHUNKS: usize = 1024;
/// Largest `XAUTHORITY` file read during setup. A larger or non-regular file
/// is treated as unauthenticated instead of being opened, so a FIFO or huge
/// file cannot block startup without a bound.
const MAX_XAUTHORITY_BYTES: u64 = 64 * 1024;

struct DeadlineStream {
    stream: DefaultStream,
    deadline: Cell<Instant>,
    io_failed: Cell<bool>,
}

impl DeadlineStream {
    fn checked<T>(&self, result: io::Result<T>) -> io::Result<T> {
        if let Err(error) = &result
            && error.kind() != io::ErrorKind::WouldBlock
        {
            self.io_failed.set(true);
        }
        result
    }

    fn check_deadline(&self) -> io::Result<()> {
        if Instant::now() >= self.deadline.get() {
            self.io_failed.set(true);
            return Err(io::ErrorKind::TimedOut.into());
        }
        Ok(())
    }
}

impl Stream for DeadlineStream {
    fn poll(&self, mode: PollMode) -> io::Result<()> {
        let mut flags = PollFlags::empty();
        if mode.readable() {
            flags |= PollFlags::IN;
        }
        if mode.writable() {
            flags |= PollFlags::OUT;
        }
        self.checked(wait(&self.stream, flags, self.deadline.get()))
    }

    fn read(&self, buffer: &mut [u8], fds: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        // An always-readable transport can satisfy an unbounded sequence of
        // reads without ever polling. Check the deadline on both sides so
        // successful reads cannot extend the session past it.
        self.check_deadline()?;
        let read = self.checked(self.stream.read(buffer, fds))?;
        if read == 0 {
            self.io_failed.set(true);
        }
        self.check_deadline()?;
        Ok(read)
    }

    fn write(&self, buffer: &[u8], fds: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        self.check_deadline()?;
        let written = self.checked(self.stream.write(buffer, fds))?;
        self.check_deadline()?;
        Ok(written)
    }
}

type XConnection = RustConnection<DeadlineStream>;

/// The server operations needed for one ownership-checked conversion.
trait SelectionServer {
    fn grab(&mut self) -> Result<(), ClipboardError>;
    fn selection_owner(&mut self, selection: Atom) -> Result<Window, ClipboardError>;
    /// Drain already-queued XFixes ownership notifications for `monitor` and
    /// `selection`. Returns `true` when a reassertion is found, including one by
    /// the same window. The scan stops at the first match so later events stay
    /// queued for the next `sync_selection`; the caller must advance its local
    /// generation. Reaching the event bound without a match returns
    /// [`ClipboardError::ClipboardChanged`] because the absence of a match
    /// cannot be proven.
    fn drain_selection_events(
        &mut self,
        monitor: Window,
        selection: Atom,
    ) -> Result<bool, ClipboardError>;
    fn convert(
        &mut self,
        requestor: Window,
        selection: Atom,
        target: Atom,
        property: Atom,
    ) -> Result<(), ClipboardError>;
    fn flush(&mut self) -> Result<(), ClipboardError>;
    fn ungrab(&mut self) -> Result<(), ClipboardError>;
}

struct XServer<'a>(&'a XConnection);

impl SelectionServer for XServer<'_> {
    fn grab(&mut self) -> Result<(), ClipboardError> {
        // XGrabServer has no reply in the core protocol. Checking the void
        // cookie surfaces an immediate protocol error, and the GetSelectionOwner
        // round trip in `request_conversion` proves the server processed this
        // grab before that query. There is no separate grab acknowledgement.
        self.0
            .grab_server()
            .map_err(|_| failed("grab the X11 server"))?
            .check()
            .map_err(|_| failed("grab the X11 server"))?;
        self.0.flush().map_err(|_| failed("flush X11 server grab"))
    }

    fn selection_owner(&mut self, selection: Atom) -> Result<Window, ClipboardError> {
        Ok(self
            .0
            .get_selection_owner(selection)
            .map_err(|_| failed("query X11 clipboard ownership"))?
            .reply()
            .map_err(|_| failed("query X11 clipboard ownership"))?
            .owner)
    }

    fn drain_selection_events(
        &mut self,
        monitor: Window,
        selection: Atom,
    ) -> Result<bool, ClipboardError> {
        for _ in 0..MAX_EVENTS {
            let event = self.0.poll_for_event().map_err(|_| {
                self.0.stream().io_failed.set(true);
                failed("read X11 events")
            })?;
            match event {
                Some(Event::XfixesSelectionNotify(event))
                    if event.window == monitor
                        && event.selection == selection
                        && event.response_type & 0x80 == 0 =>
                {
                    // Any queued ownership event, including one for the same
                    // window ID, invalidates the offer that was screened. Stop
                    // here so later events remain queued for `sync_selection`,
                    // which advances the generation from the notification.
                    return Ok(true);
                }
                Some(_) => {}
                None => return Ok(false),
            }
        }
        // The queue exceeded the application drain bound with no match. We
        // cannot prove that a queued reassertion is absent, so invalidate the
        // offer instead of returning an unclassified limit. The caller treats
        // `ClipboardChanged` as a generation-advancing rejection and sends no
        // conversion.
        Err(ClipboardError::ClipboardChanged)
    }

    fn convert(
        &mut self,
        requestor: Window,
        selection: Atom,
        target: Atom,
        property: Atom,
    ) -> Result<(), ClipboardError> {
        self.0
            .convert_selection(requestor, selection, target, property, x11rb::CURRENT_TIME)
            .map(|_| ())
            .map_err(|_| failed("request X11 selection conversion"))
    }

    fn flush(&mut self) -> Result<(), ClipboardError> {
        self.0
            .flush()
            .map_err(|_| failed("flush X11 selection request"))
    }

    fn ungrab(&mut self) -> Result<(), ClipboardError> {
        self.0
            .ungrab_server()
            .map_err(|_| failed("release the X11 server grab"))?;
        self.0
            .flush()
            .map_err(|_| failed("flush X11 server grab release"))
    }
}

/// Request `target` from `expected_owner` under a server grab.
///
/// `XGrabServer` prevents other clients from processing requests while the
/// ownership check, queued-event drain and `ConvertSelection` are handled. The
/// core protocol has no grab reply, so the grab's void cookie is checked and
/// the `GetSelectionOwner` round trip is what proves the server processed the
/// grab before that query. Requests are server-ordered: an ownership change the
/// server processed before the grab took effect has already generated an XFixes
/// event by the time the owner reply arrives; a peer request merely queued on
/// another connection is not covered. Draining those events under the
/// grab catches a reassertion even when it reuses the same window ID, which the
/// XID comparison alone cannot distinguish. The server therefore resolves the
/// conversion against the same offer whose `TARGETS` were screened; a rival
/// that becomes the owner after the grab is released cannot receive a payload
/// request made on the old offer. The grab is released before any payload is
/// read, so a slow or malicious owner cannot freeze the display.
fn request_conversion(
    server: &mut impl SelectionServer,
    monitor: Window,
    selection: Atom,
    expected_owner: Window,
    requestor: Window,
    target: Atom,
    property: Atom,
) -> Result<(), ClipboardError> {
    server.grab()?;
    let outcome = (|| {
        if server.selection_owner(selection)? != expected_owner {
            return Err(ClipboardError::ClipboardChanged);
        }
        // The owner reply confirms prior XFixes events are queued. Scan them
        // under the grab. The scan stops at the first match, so events that
        // follow it remain queued for `sync_selection`; none can belong to this
        // request because ConvertSelection has not been sent yet.
        if server.drain_selection_events(monitor, selection)? {
            return Err(ClipboardError::ClipboardChanged);
        }
        server.convert(requestor, selection, target, property)?;
        server.flush()
    })();
    let ungrab = server.ungrab();
    match outcome {
        Ok(()) => ungrab,
        Err(error) => {
            // Best effort: a failed release is reported only when the request
            // itself succeeded. A dead connection drops the grab on close.
            let _ = ungrab;
            Err(error)
        }
    }
}

/// Whether a refused conversion must invalidate the current ownership
/// generation.
///
/// The under-grab drain stops at the first matching XFixes event and converts
/// an event-bound overflow into `ClipboardChanged`; both must advance the local
/// generation even though the owner XID is unchanged. `ClipboardChanged` is
/// the only conversion failure that carries that meaning.
fn conversion_invalidates_generation(result: &Result<(), ClipboardError>) -> bool {
    matches!(result, Err(ClipboardError::ClipboardChanged))
}

pub(super) struct Reader {
    conn: XConnection,
    monitor: Window,
    root: Window,
    clipboard: Atom,
    targets: Atom,
    property: Atom,
    incr: Atom,
    active_window: Atom,
    pid: Atom,
    owner: Window,
    generation: u64,
}

impl Reader {
    pub(super) fn open() -> Result<Self, ClipboardError> {
        let no_display = || unavailable("no usable local X11 display");
        let display = parse_display::parse_display(None).map_err(|_| no_display())?;
        if display.protocol.as_deref().is_some_and(|p| p != "unix")
            || !(display.host.is_empty() || display.host == "unix" || display.host.starts_with('/'))
        {
            return Err(unavailable(
                "remote or non-Unix X11 displays are not supported",
            ));
        }
        let path = if display.host.starts_with('/') {
            display.host
        } else {
            format!("/tmp/.X11-unix/X{}", display.display)
        };
        let deadline = Instant::now() + DEADLINE;
        let stream = (|| {
            connect_unix(
                &SocketAddrUnix::new_abstract_name(path.as_bytes())?,
                deadline,
            )
        })()
        .or_else(|_| connect_unix(&SocketAddrUnix::new(Path::new(&path))?, deadline))
        .map_err(|_| no_display())?;
        let (stream, (family, address)) =
            DefaultStream::from_unix_stream(stream).map_err(|_| no_display())?;
        let (name, auth) = bounded_authority().map_or_else(
            || (Vec::new(), Vec::new()),
            |_| {
                xauth::get_auth(family, &address, display.display)
                    .ok()
                    .flatten()
                    .unwrap_or_default()
            },
        );
        if Instant::now() >= deadline {
            return Err(unavailable("X11 clipboard setup exceeded its deadline"));
        }
        let screen = usize::from(display.screen);
        let conn = XConnection::connect_to_stream_with_auth_info(
            DeadlineStream {
                stream,
                deadline: Cell::new(deadline),
                io_failed: Cell::new(false),
            },
            screen,
            name,
            auth,
        )
        .map_err(|_| no_display())?;
        if conn
            .extension_information(x11rb::protocol::xfixes::X11_EXTENSION_NAME)
            .map_err(|_| failed("probe XFixes"))?
            .is_none()
        {
            return Err(unavailable("the X11 server does not advertise XFixes"));
        }
        let version = conn
            .xfixes_query_version(2, 0)
            .map_err(|_| failed("query XFixes version"))?
            .reply()
            .map_err(|_| failed("query XFixes version"))?;
        if version.major_version < 2 {
            return Err(unavailable("XFixes version 2 or newer is required"));
        }
        let root = conn.setup().roots[screen].root;
        let monitor = make_window(&conn, root)?;
        let clipboard = atom(&conn, b"CLIPBOARD")?;
        conn.xfixes_select_selection_input(
            monitor,
            clipboard,
            SelectionEventMask::SET_SELECTION_OWNER
                | SelectionEventMask::SELECTION_WINDOW_DESTROY
                | SelectionEventMask::SELECTION_CLIENT_CLOSE,
        )
        .map_err(|_| failed("subscribe to X11 clipboard changes"))?
        .check()
        .map_err(|_| failed("subscribe to X11 clipboard changes"))?;
        Ok(Self {
            targets: atom(&conn, b"TARGETS")?,
            property: atom(&conn, b"_ANSA_CLIPBOARD_READ")?,
            incr: atom(&conn, b"INCR")?,
            active_window: atom(&conn, b"_NET_ACTIVE_WINDOW")?,
            pid: atom(&conn, b"_NET_WM_PID")?,
            conn,
            monitor,
            root,
            clipboard,
            owner: 0,
            generation: 0,
        })
    }

    pub(super) fn dead(&self) -> bool {
        self.conn.stream().io_failed.get()
    }

    pub(super) fn probe(&mut self) -> Result<(), ClipboardError> {
        self.begin();
        self.sync_selection()
    }

    pub(super) fn read(
        &mut self,
        policy: Option<&CapturePolicy>,
    ) -> Result<ClipboardSnapshot, ClipboardError> {
        self.begin();
        self.sync_selection()?;
        if self.owner == 0 {
            return Err(ClipboardError::NoSupportedFormat);
        }
        let generation = self.generation;
        let focus = self.focus();
        let source = focus.and_then(|window| self.source(window));
        let window = make_window(&self.conn, self.root)?;
        // A fresh requestor window isolates late replies from previous timeouts.
        let result = self.read_offer(window, generation, source, policy);
        let _ = self.conn.destroy_window(window);
        let _ = self.conn.flush();
        self.sync_selection()?;
        let mut snapshot = unchanged(generation, self.generation, result)?;
        if focus != self.focus() {
            snapshot.source = None;
            snapshot.confidence = SourceConfidence::Unknown;
        }
        self.sync_selection()?;
        unchanged(generation, self.generation, Ok(snapshot))
    }

    fn begin(&self) {
        self.conn.stream().deadline.set(Instant::now() + DEADLINE);
    }

    fn sync_selection(&mut self) -> Result<(), ClipboardError> {
        let error = || failed("query X11 clipboard ownership");
        let owner = self
            .conn
            .get_selection_owner(self.clipboard)
            .map_err(|_| error())?
            .reply()
            .map_err(|_| error())?
            .owner;
        for _ in 0..MAX_EVENTS {
            let Some(_) = self.poll_event()? else {
                if self.owner != owner {
                    self.set_owner(owner);
                }
                return Ok(());
            };
        }
        Err(offer::limit("X11 pending events"))
    }

    fn set_owner(&mut self, owner: Window) {
        self.owner = owner;
        self.generation = next_generation();
    }

    fn poll_event(&mut self) -> Result<Option<Event>, ClipboardError> {
        let event = self.conn.poll_for_event().map_err(|_| {
            self.conn.stream().io_failed.set(true);
            failed("read X11 events")
        })?;
        if let Some(Event::XfixesSelectionNotify(event)) = &event
            && event.window == self.monitor
            && event.selection == self.clipboard
            && event.response_type & 0x80 == 0
        {
            // Notify is also emitted when the owner reclaims CLIPBOARD using
            // the same window. Comparing just an XID would miss that race.
            self.set_owner(event.owner);
        }
        Ok(event)
    }

    fn read_offer(
        &mut self,
        window: Window,
        generation: u64,
        source: Option<SourceApp>,
        policy: Option<&CapturePolicy>,
    ) -> Result<ClipboardSnapshot, ClipboardError> {
        let raw = self.transfer(
            window,
            generation,
            self.targets,
            AtomEnum::ATOM.into(),
            32,
            MAX_OFFER_TYPES * 4,
        )?;
        let mut types = Vec::new();
        let mut atoms = Vec::new();
        let (chunks, remainder) = raw.0.as_chunks::<4>();
        if !remainder.is_empty() {
            return Err(offer::malformed("misaligned X11 selection target list"));
        }
        for bytes in chunks {
            let target = Atom::from_ne_bytes(*bytes);
            if target == 0 {
                return Err(offer::malformed("null X11 selection target"));
            }
            let name = self
                .conn
                .get_atom_name(target)
                .map_err(|_| failed("get X11 target name"))?
                .reply()
                .map_err(|_| failed("get X11 target name"))?
                .name;
            let name = std::str::from_utf8(&name)
                .map_err(|_| offer::malformed("non-UTF-8 X11 selection target"))?;
            let old = types.len();
            offer::add_type(&mut types, name)?;
            if types.len() != old {
                atoms.push(target);
            }
        }
        offer::snapshot(
            Dialect::Linux,
            &types,
            generation,
            source,
            policy,
            |format, name| {
                self.sync_selection()?;
                if self.generation != generation {
                    return Err(ClipboardError::ClipboardChanged);
                }
                let target = types
                    .iter()
                    .position(|item| item == name)
                    .map(|index| atoms[index])
                    .ok_or_else(|| failed("resolve X11 transfer type"))?;
                let data = self.transfer(window, generation, target, target, 8, MAX_BYTES)?;
                match format {
                    Format::Text => offer::text(&data.0),
                    Format::Files => offer::uri_list(&data.0),
                    Format::Image => unreachable!("the offer layer never requests image bytes"),
                }
            },
        )
    }

    fn transfer(
        &mut self,
        window: Window,
        generation: u64,
        target: Atom,
        expected_type: Atom,
        width: u8,
        limit: usize,
    ) -> Result<Scratch, ClipboardError> {
        let conversion = request_conversion(
            &mut XServer(&self.conn),
            self.monitor,
            self.clipboard,
            self.owner,
            window,
            target,
            self.property,
        );
        if conversion_invalidates_generation(&conversion) {
            // The drain can stop at a consumed XFixes event or report the
            // event-bound overflow without passing through `poll_event`, which
            // normally advances the generation. Advance it here too, otherwise
            // a same-XID reassert could reuse the previous generation and be
            // suppressed as a duplicate capture on the next read.
            self.set_owner(self.owner);
        }
        conversion?;
        let Event::SelectionNotify(notify) = self.await_event(generation, |event, reader| {
            matches!(event, Event::SelectionNotify(event)
                if event.requestor == window && event.selection == reader.clipboard && event.target == target)
        })? else {
            unreachable!("event matched above")
        };
        if notify.property != self.property {
            return Err(failed("X11 selection owner refused the conversion"));
        }
        let first = self.get_property(window, limit.max(4))?;
        if first.type_ != self.incr {
            let bytes = property_bytes(first, expected_type, width, limit)?;
            self.delete_property(window)?;
            return Ok(bytes);
        }
        let header = property_bytes(first, self.incr, 32, 4)?;
        let size = u32::from_ne_bytes(
            header
                .0
                .as_slice()
                .try_into()
                .map_err(|_| offer::malformed("invalid X11 INCR header"))?,
        ) as usize;
        if size > limit {
            return Err(offer::limit("X11 incremental payload bytes"));
        }
        self.delete_property(window)?;
        collect_incr_chunks(
            |request| {
                self.await_event(generation, |event, reader| {
                    matches!(event, Event::PropertyNotify(event)
                        if event.window == window && event.atom == reader.property && event.state == Property::NEW_VALUE)
                })?;
                let reply = self.get_property(window, request)?;
                self.delete_property(window)?;
                Ok(reply)
            },
            expected_type,
            width,
            limit,
            size,
        )
    }

    fn await_event(
        &mut self,
        generation: u64,
        predicate: impl Fn(&Event, &Self) -> bool,
    ) -> Result<Event, ClipboardError> {
        let deadline = self.conn.stream().deadline.get();
        for _ in 0..MAX_EVENTS {
            if Instant::now() >= deadline {
                break;
            }
            match self.poll_event()? {
                Some(event) => {
                    if self.generation != generation {
                        return Err(ClipboardError::ClipboardChanged);
                    }
                    if predicate(&event, self) {
                        return Ok(event);
                    }
                }
                None => wait(&self.conn.stream().stream, PollFlags::IN, deadline)
                    .map_err(|_| failed("X11 selection transfer timed out"))?,
            }
        }
        Err(failed(
            "X11 selection transfer timed out or exceeded the event bound",
        ))
    }

    fn get_property(
        &self,
        window: Window,
        limit: usize,
    ) -> Result<GetPropertyReply, ClipboardError> {
        self.conn
            .get_property(
                false,
                window,
                self.property,
                AtomEnum::ANY,
                0,
                limit.div_ceil(4) as u32,
            )
            .map_err(|_| failed("get X11 selection property"))?
            .reply()
            .map_err(|_| failed("get X11 selection property"))
    }

    fn delete_property(&self, window: Window) -> Result<(), ClipboardError> {
        self.conn
            .delete_property(window, self.property)
            .map_err(|_| failed("release X11 selection property"))?;
        self.conn
            .flush()
            .map_err(|_| failed("flush X11 selection property release"))
    }

    fn focus(&self) -> Option<Window> {
        self.scalar_property(self.root, self.active_window, AtomEnum::WINDOW)
            .filter(|window| *window > 1)
            .or_else(|| {
                Some(self.conn.get_input_focus().ok()?.reply().ok()?.focus).filter(|id| *id > 1)
            })
    }

    fn source(&self, window: Window) -> Option<SourceApp> {
        let pid = self.scalar_property(window, self.pid, AtomEnum::CARDINAL)?;
        if pid == 0 {
            return None;
        }
        // WM_CLASS, window titles and command lines are arbitrary client
        // strings. A process basename is a less revealing attribution hint.
        let executable = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
        SourceApp::new(executable.file_name()?.to_str()?)
    }

    fn scalar_property(&self, window: Window, property: Atom, kind: AtomEnum) -> Option<u32> {
        let reply = self
            .conn
            .get_property(false, window, property, kind, 0, 1)
            .ok()?
            .reply()
            .ok()?;
        if reply.bytes_after != 0 || reply.type_ != Atom::from(kind) || reply.value_len != 1 {
            return None;
        }
        reply.value32()?.next()
    }
}

fn authority_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("XAUTHORITY") {
        return Some(path.into());
    }
    std::env::var_os("HOME").map(|home| Path::new(&home).join(".Xauthority"))
}

/// Preflight predicate for the authority path passed to [`xauth::get_auth`].
///
/// This is not an I/O guarantee: the pinned reader re-resolves, opens and reads
/// the path after this metadata check, so a same-user swap to a FIFO, a file
/// that grows past the cap, or a slow filesystem can still block or exceed the
/// size hint. The check only avoids the ordinary FIFO/oversized path.
fn authority_is_bounded(path: &Path) -> bool {
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= MAX_XAUTHORITY_BYTES)
}

fn bounded_authority() -> Option<PathBuf> {
    let path = authority_path()?;
    authority_is_bounded(&path).then_some(path)
}

fn atom(conn: &XConnection, name: &[u8]) -> Result<Atom, ClipboardError> {
    let error = || failed("resolve X11 selection atoms");
    Ok(conn
        .intern_atom(false, name)
        .map_err(|_| error())?
        .reply()
        .map_err(|_| error())?
        .atom)
}

fn make_window(conn: &XConnection, root: Window) -> Result<Window, ClipboardError> {
    let error = || failed("create X11 selection requestor");
    let window = conn.generate_id().map_err(|_| error())?;
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )
    .map_err(|_| error())?
    .check()
    .map_err(|_| error())?;
    Ok(window)
}

/// Collect an INCR payload from successive property replies.
///
/// `fetch` receives the requested byte count and must return the reply for
/// that request. It must request at least one 32-bit word: a zero-length
/// `GetProperty` can produce an empty reply that is indistinguishable from the
/// INCR end marker, hiding data beyond `limit`.
fn collect_incr_chunks(
    mut fetch: impl FnMut(usize) -> Result<GetPropertyReply, ClipboardError>,
    expected_type: Atom,
    width: u8,
    limit: usize,
    declared: usize,
) -> Result<Scratch, ClipboardError> {
    // Fixed max capacity avoids reallocating buffers containing plaintext.
    let mut result = Scratch(Vec::with_capacity(limit));
    for _ in 0..MAX_CHUNKS {
        let remaining = limit.saturating_sub(result.0.len());
        let data = property_bytes(fetch(remaining.max(4))?, expected_type, width, remaining)?;
        if data.0.is_empty() {
            if result.0.len() < declared {
                return Err(offer::malformed("truncated X11 incremental payload"));
            }
            return Ok(result);
        }
        result.0.extend_from_slice(&data.0);
    }
    Err(offer::limit("X11 incremental chunk count"))
}

fn property_bytes(
    reply: GetPropertyReply,
    expected_type: Atom,
    width: u8,
    limit: usize,
) -> Result<Scratch, ClipboardError> {
    let value = Scratch(reply.value);
    if reply.bytes_after != 0 || value.0.len() > limit {
        return Err(offer::limit("X11 selection property bytes"));
    }
    if reply.type_ != expected_type || reply.format != width {
        return Err(offer::malformed("unexpected X11 selection property type"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    use super::*;

    #[test]
    fn x11_property_rejects_prefix_of_overrun_or_wrong_native_format() {
        let property = |width, bytes_after| GetPropertyReply {
            format: width,
            bytes_after,
            type_: 10,
            value_len: 4,
            value: b"text".to_vec(),
            ..GetPropertyReply::default()
        };
        assert!(matches!(
            property_bytes(property(8, 1), 10, 8, 4),
            Err(ClipboardError::CaptureLimit { .. })
        ));
        assert!(matches!(
            property_bytes(property(32, 0), 10, 8, 4),
            Err(ClipboardError::MalformedPayload { .. })
        ));
        assert_eq!(
            property_bytes(property(8, 0), 10, 8, 4).ok().unwrap().0,
            b"text"
        );
    }

    fn incr_reply(value: &[u8]) -> GetPropertyReply {
        GetPropertyReply {
            format: 8,
            bytes_after: 0,
            type_: 10,
            value_len: value.len() as u32,
            value: value.to_vec(),
            ..GetPropertyReply::default()
        }
    }

    #[test]
    fn x11_incr_rejects_extra_data_after_a_full_limit_chunk() {
        let mut replies = [incr_reply(b"abcd"), incr_reply(b"efgh")].into_iter();
        let mut requests = Vec::new();
        let result = collect_incr_chunks(
            |request| {
                requests.push(request);
                // A compliant server returns no bytes for a zero-length
                // request. The old code could mistake that for INCR EOF.
                if request == 0 {
                    return Ok(incr_reply(b""));
                }
                Ok(replies.next().expect("synthetic chunk"))
            },
            10,
            8,
            4,
            4,
        );
        assert!(matches!(result, Err(ClipboardError::CaptureLimit { .. })));
        assert_eq!(requests, vec![4, 4]);
    }

    #[test]
    fn x11_incr_accepts_exact_limit_and_rejects_a_truncated_prefix() {
        let mut replies = [incr_reply(b"abcd"), incr_reply(b"")].into_iter();
        let mut requests = Vec::new();
        let result = collect_incr_chunks(
            |request| {
                requests.push(request);
                Ok(replies.next().expect("synthetic chunk"))
            },
            10,
            8,
            4,
            4,
        );
        assert_eq!(result.expect("exact limit").0, b"abcd");
        assert_eq!(requests, vec![4, 4]);

        let mut replies = [incr_reply(b"ab"), incr_reply(b"")].into_iter();
        assert!(matches!(
            collect_incr_chunks(
                |_| Ok(replies.next().expect("synthetic chunk")),
                10,
                8,
                4,
                4
            ),
            Err(ClipboardError::MalformedPayload { .. })
        ));
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum FakeEvent {
        /// A non-matching event (another monitor/selection or a non-notify).
        Unrelated,
        /// A matching XFixes ownership notification.
        Reassert,
    }

    struct FakeServer {
        owner: Window,
        honor_grab: bool,
        grabbed: bool,
        /// A rival that asks for ownership immediately after the owner query.
        rival: Option<Window>,
        /// XFixes events already queued when the grab is processed.
        queued: VecDeque<FakeEvent>,
        /// An ownership event delivered when the grab is released.
        on_release: Option<Window>,
        drained: Vec<(Window, Atom)>,
        conversions: Vec<(Window, Atom)>,
    }

    impl FakeServer {
        fn new(owner: Window, honor_grab: bool) -> Self {
            Self {
                owner,
                honor_grab,
                grabbed: false,
                rival: Some(owner.saturating_add(1)),
                queued: VecDeque::new(),
                on_release: None,
                drained: Vec::new(),
                conversions: Vec::new(),
            }
        }

        /// Queue a reassertion that arrived before the grab was processed. The
        /// owner window may be the same XID as the observed owner.
        fn queued_reassertion(mut self) -> Self {
            self.rival = None;
            self.queued.push_back(FakeEvent::Reassert);
            self
        }

        /// Queue an explicit event sequence before the grab is processed.
        fn with_queued(mut self, events: impl IntoIterator<Item = FakeEvent>) -> Self {
            self.rival = None;
            self.queued.extend(events);
            self
        }

        /// Deliver a reassertion only after the grab is released.
        fn reassertion_on_release(mut self, owner: Window) -> Self {
            self.rival = None;
            self.on_release = Some(owner);
            self
        }
    }

    impl SelectionServer for FakeServer {
        fn grab(&mut self) -> Result<(), ClipboardError> {
            self.grabbed = self.honor_grab;
            Ok(())
        }

        fn selection_owner(&mut self, _: Atom) -> Result<Window, ClipboardError> {
            let owner = self.owner;
            if !self.grabbed
                && let Some(rival) = self.rival.take()
            {
                self.owner = rival;
            }
            Ok(owner)
        }

        fn drain_selection_events(
            &mut self,
            monitor: Window,
            selection: Atom,
        ) -> Result<bool, ClipboardError> {
            self.drained.push((monitor, selection));
            for _ in 0..MAX_EVENTS {
                match self.queued.pop_front() {
                    Some(FakeEvent::Unrelated) => {}
                    // Stop at the first match; later events stay queued for the
                    // next sync_selection, exactly like the real drain.
                    Some(FakeEvent::Reassert) => return Ok(true),
                    None => return Ok(false),
                }
            }
            Err(ClipboardError::ClipboardChanged)
        }

        fn convert(
            &mut self,
            _: Window,
            _: Atom,
            target: Atom,
            _: Atom,
        ) -> Result<(), ClipboardError> {
            // Record the owner that would receive the conversion when the
            // server processes this request.
            self.conversions.push((self.owner, target));
            Ok(())
        }

        fn flush(&mut self) -> Result<(), ClipboardError> {
            Ok(())
        }

        fn ungrab(&mut self) -> Result<(), ClipboardError> {
            if self.grabbed {
                self.grabbed = false;
                if let Some(owner) = self.rival.take() {
                    self.owner = owner;
                }
                if let Some(owner) = self.on_release.take() {
                    self.queued.push_back(FakeEvent::Reassert);
                    self.owner = owner;
                }
            }
            Ok(())
        }
    }

    #[test]
    fn x11_conversion_under_a_grab_cannot_reach_a_new_owner() {
        let mut server = FakeServer::new(10, true);
        request_conversion(&mut server, 7, 1, 10, 20, 2, 3).expect("checked conversion");
        assert_eq!(server.conversions, vec![(10, 2)]);
        assert_eq!(server.drained, vec![(7, 1)]);
        assert_eq!(
            server.owner, 11,
            "the rival takes ownership only after the grab is released"
        );
        assert!(!server.grabbed);

        // Negative control: the same interleaving without the grab delivers the
        // payload request to the new owner. This is the defect the grab removes.
        let mut server = FakeServer::new(10, false);
        request_conversion(&mut server, 7, 1, 10, 20, 2, 3).expect("unchecked conversion");
        assert_eq!(server.conversions, vec![(11, 2)]);
    }

    #[test]
    fn x11_conversion_rejects_an_owner_that_already_changed() {
        let mut server = FakeServer::new(10, true);
        assert!(matches!(
            request_conversion(&mut server, 7, 1, 9, 20, 2, 3),
            Err(ClipboardError::ClipboardChanged)
        ));
        assert!(server.conversions.is_empty());
        assert!(!server.grabbed, "the server grab must always be released");
    }

    #[test]
    fn x11_conversion_rejects_a_same_xid_reassertion_queued_before_the_grab() {
        // The owner query still returns the observed window, so the XID check
        // alone passes. The queued XFixes event proves the offer was reasserted
        // after the last snapshot and must abort the conversion.
        let mut server = FakeServer::new(10, true).queued_reassertion();
        assert!(matches!(
            request_conversion(&mut server, 7, 1, 10, 20, 2, 3),
            Err(ClipboardError::ClipboardChanged)
        ));
        assert!(server.conversions.is_empty());
        assert_eq!(server.drained, vec![(7, 1)]);
        assert_eq!(server.owner, 10);
        assert!(!server.grabbed, "the server grab must always be released");
    }

    #[test]
    fn x11_late_reassertion_is_not_routed_to_the_new_offer() {
        // The conversion is queued while the grab is held, so it reaches the
        // old offer. A reassertion delivered after the release remains queued
        // for the next capture and receives no payload request from this one.
        let mut server = FakeServer::new(10, true).reassertion_on_release(11);
        request_conversion(&mut server, 7, 1, 10, 20, 2, 3).expect("checked conversion");
        assert_eq!(server.conversions, vec![(10, 2)]);
        assert!(
            server.conversions.iter().all(|(owner, _)| *owner != 11),
            "the new offer must receive zero conversions"
        );
        assert_eq!(server.owner, 11);
        assert_eq!(
            server.drained,
            vec![(7, 1)],
            "the late event was not drained"
        );
        assert_eq!(
            server.queued,
            VecDeque::from([FakeEvent::Reassert]),
            "the late reassertion remains queued for the next capture"
        );
        assert!(!server.grabbed);
    }

    #[test]
    fn x11_conversion_rejects_a_match_at_the_event_bound() {
        // 4,095 non-matching events precede the only reassertion. The drain
        // must stop at the match, abort, and leave later events for
        // `sync_selection` instead of losing the invalidation at the bound.
        let mut events = vec![FakeEvent::Unrelated; MAX_EVENTS - 1];
        events.push(FakeEvent::Reassert);
        events.extend([FakeEvent::Unrelated; 2]);
        let mut server = FakeServer::new(10, true).with_queued(events);

        let result = request_conversion(&mut server, 7, 1, 10, 20, 2, 3);
        assert!(matches!(result, Err(ClipboardError::ClipboardChanged)));
        assert!(
            server.conversions.is_empty(),
            "no conversion may escape the rejected offer"
        );
        assert!(
            conversion_invalidates_generation(&result),
            "the rejection must advance the ownership generation"
        );
        assert_eq!(
            server.queued,
            VecDeque::from([FakeEvent::Unrelated, FakeEvent::Unrelated]),
            "events after the match stay queued for sync_selection"
        );
        assert!(!server.grabbed);
    }

    #[test]
    fn x11_conversion_rejects_an_event_bound_without_a_match() {
        // The queue is longer than the application drain bound and contains no
        // matching notification. Absence cannot be proven, so the conversion
        // must be rejected as a clipboard change, not an unclassified limit.
        let mut server =
            FakeServer::new(10, true).with_queued(vec![FakeEvent::Unrelated; MAX_EVENTS]);

        let result = request_conversion(&mut server, 7, 1, 10, 20, 2, 3);
        assert!(matches!(result, Err(ClipboardError::ClipboardChanged)));
        assert!(server.conversions.is_empty());
        assert!(
            conversion_invalidates_generation(&result),
            "the capped rejection must advance the ownership generation"
        );
        assert!(!server.grabbed);
    }

    #[test]
    fn x11_only_clipboard_changes_advance_the_generation() {
        assert!(conversion_invalidates_generation(&Err(
            ClipboardError::ClipboardChanged
        )));
        assert!(!conversion_invalidates_generation(&Ok(())));
        assert!(!conversion_invalidates_generation(&Err(
            ClipboardError::CaptureLimit {
                reason: "synthetic",
            }
        )));
    }

    #[test]
    fn deadline_stream_rejects_data_available_after_the_deadline() {
        let (ours, mut peer) = UnixStream::pair().expect("socket pair");
        ours.set_nonblocking(true).expect("nonblocking");
        peer.write_all(b"ready").expect("write fixture");
        let (stream, _) = DefaultStream::from_unix_stream(ours).expect("default stream");
        let stream = DeadlineStream {
            stream,
            deadline: Cell::new(Instant::now() - Duration::from_millis(1)),
            io_failed: Cell::new(false),
        };

        let mut buffer = [0u8; 8];
        let mut fds = Vec::new();
        let error = stream
            .read(&mut buffer, &mut fds)
            .expect_err("read must honor the expired deadline");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(stream.io_failed.get());
        assert!(
            stream
                .write(b"x", &mut Vec::new())
                .expect_err("write must honor the expired deadline")
                .kind()
                == io::ErrorKind::TimedOut
        );
    }

    #[test]
    fn deadline_stream_reads_before_the_deadline() {
        let (ours, mut peer) = UnixStream::pair().expect("socket pair");
        ours.set_nonblocking(true).expect("nonblocking");
        peer.write_all(b"ok").expect("write fixture");
        let (stream, _) = DefaultStream::from_unix_stream(ours).expect("default stream");
        let stream = DeadlineStream {
            stream,
            deadline: Cell::new(Instant::now() + Duration::from_secs(1)),
            io_failed: Cell::new(false),
        };

        let mut buffer = [0u8; 8];
        let mut fds = Vec::new();
        assert_eq!(stream.read(&mut buffer, &mut fds).expect("read"), 2);
        assert_eq!(&buffer[..2], b"ok");
    }

    #[test]
    fn xauthority_check_rejects_missing_and_oversized_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let regular = dir.path().join("regular");
        std::fs::write(&regular, b"entry").expect("write fixture");
        assert!(authority_is_bounded(&regular));
        assert!(!authority_is_bounded(&dir.path().join("missing")));

        let oversized = dir.path().join("oversized");
        std::fs::write(&oversized, vec![0u8; MAX_XAUTHORITY_BYTES as usize + 1])
            .expect("write fixture");
        assert!(!authority_is_bounded(&oversized));
    }
}
