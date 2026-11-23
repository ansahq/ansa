//! Read-only Wayland data control. No focus surfaces, portal simulation, shell
//! helpers, primary selection or fallback to an XWayland shadow clipboard.

use std::fs::File;
use std::io::{ErrorKind, Read};
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rustix::event::PollFlags;
use rustix::fs::{OFlags, fcntl_setfl};
use rustix::net::SocketAddrUnix;
use rustix::pipe::{PipeFlags, pipe_with};
use wayland_client::backend::WaylandError;
use wayland_client::protocol::{wl_callback, wl_registry, wl_seat};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop, event_created_child,
};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1 as ext_device, ext_data_control_manager_v1 as ext_manager,
    ext_data_control_offer_v1 as ext_offer,
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1 as wlr_device, zwlr_data_control_manager_v1 as wlr_manager,
    zwlr_data_control_offer_v1 as wlr_offer,
};

use super::{DEADLINE, connect_unix, failed, unavailable, wait};
use crate::native::offer::{self, Dialect, Format, MAX_BYTES, MAX_OFFER_TYPES, Scratch};
use crate::native::{next_generation, unchanged};
use crate::{CapturePolicy, ClipboardError, ClipboardSnapshot};

const NO_PROTOCOL: &str = "the compositor does not expose ext-data-control or wlr-data-control";
const NO_SOCKET: &str = "no usable Wayland socket for a dedicated clipboard connection";
const MAX_GLOBALS: usize = 1024;
const MAX_PENDING_OFFERS: usize = 8;

pub(super) struct Reader {
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
    // Retain the registry proxy for the life of the session. Dropping it would
    // stop `GlobalRemove` delivery, so a withdrawn seat or data-control global
    // would not invalidate the cached reader.
    _registry: wl_registry::WlRegistry,
    globals: Option<(u32, u32)>,
    bad_connection: bool,
}

#[derive(Default)]
struct State {
    // Only record names and versions for interfaces relevant to capture.
    seats: Vec<u32>,
    ext: Option<u32>,
    wlr: Option<u32>,
    global_count: usize,
    selection: Option<Offer>,
    pending: Vec<Offer>,
    generation: u64,
    invalid: bool,
    finished: bool,
}

#[derive(Clone, PartialEq, Eq)]
enum Offer {
    Ext(ext_offer::ExtDataControlOfferV1),
    Wlr(wlr_offer::ZwlrDataControlOfferV1),
}

impl Offer {
    fn types(&self) -> Result<Vec<String>, ClipboardError> {
        let data = match self {
            Self::Ext(offer) => offer.data::<OfferData>(),
            Self::Wlr(offer) => offer.data::<OfferData>(),
        }
        .ok_or_else(|| failed("missing Wayland offer metadata"))?;
        let state = data
            .lock()
            .map_err(|_| failed("read Wayland offer types"))?;
        if state.invalid {
            return Err(offer::limit("invalid or excessive Wayland offer types"));
        }
        Ok(state.types.clone())
    }

    fn receive(&self, name: &str, fd: &impl AsFd) {
        match self {
            Self::Ext(offer) => offer.receive(name.to_owned(), fd.as_fd()),
            Self::Wlr(offer) => offer.receive(name.to_owned(), fd.as_fd()),
        }
    }

    fn destroy(self) {
        match self {
            Self::Ext(offer) => offer.destroy(),
            Self::Wlr(offer) => offer.destroy(),
        }
    }
}

#[derive(Default)]
struct Types {
    types: Vec<String>,
    announced: usize,
    invalid: bool,
}

type OfferData = Mutex<Types>;

impl Reader {
    pub(super) fn open() -> Result<Self, ClipboardError> {
        let deadline = Instant::now() + DEADLINE;
        let path = socket_path()?;
        let address = SocketAddrUnix::new(&path).map_err(|_| unavailable(NO_SOCKET))?;
        let stream = connect_unix(&address, deadline).map_err(|_| unavailable(NO_SOCKET))?;
        let connection = Connection::from_socket(stream).map_err(|_| unavailable(NO_SOCKET))?;
        let queue = connection.new_event_queue();
        let handle = queue.handle();
        let registry = connection.display().get_registry(&handle, ());
        let mut reader = Self {
            connection,
            queue,
            state: State::default(),
            _registry: registry.clone(),
            globals: None,
            bad_connection: false,
        };
        reader.sync(deadline)?;
        let seat_id = match reader.state.seats.as_slice() {
            [] => return Err(unavailable("the Wayland compositor did not expose a seat")),
            [seat_id] => *seat_id,
            _ => {
                return Err(unavailable(
                    "multiple Wayland seats require explicit seat selection",
                ));
            }
        };
        let seat = registry.bind::<wl_seat::WlSeat, _, _>(seat_id, 1, &handle, ());
        let manager_id = if let Some(id) = reader.state.ext {
            let manager =
                registry.bind::<ext_manager::ExtDataControlManagerV1, _, _>(id, 1, &handle, ());
            manager.get_data_device(&seat, &handle, ());
            manager.destroy();
            id
        } else if let Some(id) = reader.state.wlr {
            // Version 1 is sufficient and does not expose the primary selection.
            let manager =
                registry.bind::<wlr_manager::ZwlrDataControlManagerV1, _, _>(id, 1, &handle, ());
            manager.get_data_device(&seat, &handle, ());
            manager.destroy();
            id
        } else {
            return Err(unavailable(NO_PROTOCOL));
        };
        reader.globals = Some((seat_id, manager_id));
        reader.sync(deadline)?;
        if reader.state.generation == 0 {
            return Err(unavailable(
                "the Wayland data device did not report its initial selection",
            ));
        }
        Ok(reader)
    }

    pub(super) fn probe(&mut self) -> Result<(), ClipboardError> {
        self.sync(Instant::now() + DEADLINE)
    }

    pub(super) fn dead(&self) -> bool {
        self.bad_connection || self.state.invalid || self.state.finished
    }

    pub(super) fn read(
        &mut self,
        policy: Option<&CapturePolicy>,
    ) -> Result<ClipboardSnapshot, ClipboardError> {
        let deadline = Instant::now() + DEADLINE;
        self.sync(deadline)?;
        let generation = self.state.generation;
        let selected = self
            .state
            .selection
            .clone()
            .ok_or(ClipboardError::NoSupportedFormat)?;
        let types = selected.types()?;
        let result = offer::snapshot(
            Dialect::Linux,
            &types,
            generation,
            None,
            policy,
            |format, name| {
                self.sync(deadline)?;
                if self.state.generation != generation || selected.types()? != types {
                    return Err(ClipboardError::ClipboardChanged);
                }
                let data = self.receive(&selected, name, deadline)?;
                match format {
                    Format::Text => offer::text(&data.0),
                    Format::Files => offer::uri_list(&data.0),
                    Format::Image => unreachable!("the offer layer never requests image bytes"),
                }
            },
        );
        // The compositor owns offer assignment. A final barrier discards data
        // if a different selection (or mutated offer metadata) arrived.
        if Instant::now() < deadline {
            self.sync(deadline)?;
            if self.state.generation == generation && selected.types()? != types {
                return Err(ClipboardError::ClipboardChanged);
            }
        } else {
            return Err(failed("Wayland clipboard transfer timed out"));
        }
        unchanged(generation, self.state.generation, result)
    }

    fn receive(
        &mut self,
        offer: &Offer,
        name: &str,
        deadline: Instant,
    ) -> Result<Scratch, ClipboardError> {
        let (read_fd, write_fd) =
            pipe_with(PipeFlags::CLOEXEC).map_err(|_| failed("open Wayland transfer pipe"))?;
        fcntl_setfl(&read_fd, OFlags::NONBLOCK)
            .map_err(|_| failed("configure Wayland transfer pipe"))?;
        offer.receive(name, &write_fd);
        drop(write_fd);
        self.flush(deadline)?;
        read_pipe(File::from(read_fd), deadline)
    }

    fn sync(&mut self, deadline: Instant) -> Result<(), ClipboardError> {
        // Wayland's blocking roundtrip has no deadline. Send wl_display.sync
        // explicitly and wait on the connection's fd with one shared budget.
        let complete = Arc::new(AtomicBool::new(false));
        self.connection
            .display()
            .sync(&self.queue.handle(), Arc::clone(&complete));
        self.flush(deadline)?;
        loop {
            if Instant::now() >= deadline {
                self.bad_connection = true;
                return Err(failed(
                    "Wayland compositor did not answer before the deadline",
                ));
            }
            self.queue.dispatch_pending(&mut self.state).map_err(|_| {
                self.bad_connection = true;
                failed("dispatch Wayland clipboard events")
            })?;
            self.check_state()?;
            if complete.load(Ordering::Relaxed) {
                return Ok(());
            }
            let Some(guard) = self.connection.prepare_read() else {
                continue;
            };
            if wait(&guard.connection_fd(), PollFlags::IN, deadline).is_err() {
                self.bad_connection = true;
                return Err(failed(
                    "Wayland compositor did not answer before the deadline",
                ));
            }
            match guard.read() {
                Ok(_) => {}
                Err(WaylandError::Io(error)) if error.kind() == ErrorKind::WouldBlock => {}
                Err(_) => {
                    self.bad_connection = true;
                    return Err(failed("read Wayland compositor events"));
                }
            }
        }
    }

    fn flush(&mut self, deadline: Instant) -> Result<(), ClipboardError> {
        let backend = self.connection.backend();
        let fd = backend.poll_fd();
        loop {
            match self.connection.flush() {
                Ok(()) => return Ok(()),
                Err(WaylandError::Io(error)) if error.kind() == ErrorKind::WouldBlock => {
                    if wait(&fd, PollFlags::OUT, deadline).is_ok() {
                        continue;
                    }
                }
                Err(_) => {}
            }
            self.bad_connection = true;
            return Err(failed("flush Wayland clipboard requests"));
        }
    }

    fn check_state(&mut self) -> Result<(), ClipboardError> {
        let state = &self.state;
        if state.finished {
            return Err(unavailable(
                "the compositor withdrew the Wayland data device",
            ));
        }
        if state.invalid {
            return Err(failed("invalid or excessive Wayland clipboard events"));
        }
        if let Some((seat, manager)) = self.globals
            && (state.seats.as_slice() != [seat]
                || (state.ext != Some(manager) && state.wlr != Some(manager)))
        {
            self.bad_connection = true;
            return Err(unavailable(
                "the Wayland clipboard seat or control protocol changed",
            ));
        }
        Ok(())
    }
}

fn socket_path() -> Result<PathBuf, ClipboardError> {
    // Do not take ownership of the host application's inherited WAYLAND_SOCKET
    // fd or attempt a second protocol connection over a shared byte stream.
    let display = match std::env::var_os("WAYLAND_DISPLAY") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        None if std::env::var_os("WAYLAND_SOCKET").is_none() => PathBuf::from("wayland-0"),
        _ => return Err(unavailable(NO_SOCKET)),
    };
    if display.is_absolute() {
        return Ok(display);
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| unavailable(NO_SOCKET))?;
    if display.components().count() != 1 || display.file_name().is_none() {
        return Err(unavailable(NO_SOCKET));
    }
    Ok(runtime.join(display))
}

fn read_pipe(mut file: File, deadline: Instant) -> Result<Scratch, ClipboardError> {
    // A complete transfer requires EOF and at most MAX_BYTES. Read one extra
    // byte to distinguish exact-size offers from truncated oversized ones.
    let mut data = Scratch(vec![0; MAX_BYTES + 1]);
    let mut used = 0;
    loop {
        if Instant::now() >= deadline {
            return Err(failed("Wayland clipboard transfer timed out"));
        }
        match file.read(&mut data.0[used..]) {
            Ok(0) => {
                data.0.truncate(used);
                return Ok(data);
            }
            Ok(n) => {
                used += n;
                if used > MAX_BYTES {
                    return Err(offer::limit("Wayland clipboard transfer bytes"));
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                wait(&file, PollFlags::IN, deadline)
                    .map_err(|_| failed("Wayland clipboard transfer timed out"))?;
            }
            Err(_) => return Err(failed("read Wayland clipboard transfer")),
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => {
                if state.global_count == MAX_GLOBALS {
                    state.invalid = true;
                    return;
                }
                state.global_count += 1;
                if version == 0 {
                    state.invalid = true;
                    return;
                }
                match interface.as_str() {
                    "wl_seat" => state.seats.push(name),
                    "ext_data_control_manager_v1" => state.ext = Some(name),
                    "zwlr_data_control_manager_v1" => state.wlr = Some(name),
                    _ => {}
                }
            }
            wl_registry::Event::GlobalRemove { name } => {
                state.seats.retain(|id| *id != name);
                if state.ext == Some(name) {
                    state.ext = None;
                }
                if state.wlr == Some(name) {
                    state.wlr = None;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, Arc<AtomicBool>> for State {
    fn event(
        _: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        data: &Arc<AtomicBool>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        data.store(true, Ordering::Relaxed);
    }
}

delegate_noop!(State: ignore wl_seat::WlSeat);
delegate_noop!(State: ignore ext_manager::ExtDataControlManagerV1);
delegate_noop!(State: ignore wlr_manager::ZwlrDataControlManagerV1);

impl State {
    fn pending(&mut self, offer: Offer) {
        if self.pending.len() == MAX_PENDING_OFFERS {
            self.invalid = true;
            offer.destroy();
        } else {
            self.pending.push(offer);
        }
    }

    fn assign(&mut self, offer: Option<Offer>, primary: bool) {
        if let Some(offer) = &offer {
            let Some(index) = self.pending.iter().position(|pending| pending == offer) else {
                self.invalid = true;
                return;
            };
            self.pending.swap_remove(index);
        }
        if primary {
            if let Some(offer) = offer {
                offer.destroy();
            }
        } else {
            if let Some(previous) = std::mem::replace(&mut self.selection, offer) {
                previous.destroy();
            }
            self.generation = next_generation();
        }
    }
}

// The ext and wlr event shapes used for read-only capture are identical.
macro_rules! data_control {
    ($device:ident, $device_type:ident, $offer:ident, $offer_type:ident, $variant:ident) => {
        impl Dispatch<$device::$device_type, ()> for State {
            event_created_child!(State, $device::$device_type, [
                $device::EVT_DATA_OFFER_OPCODE => ($offer::$offer_type, OfferData::default())
            ]);
            fn event(
                state: &mut Self,
                device: &$device::$device_type,
                event: $device::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                match event {
                    $device::Event::DataOffer { id } => state.pending(Offer::$variant(id)),
                    $device::Event::Selection { id } => {
                        state.assign(id.map(Offer::$variant), false);
                    }
                    $device::Event::PrimarySelection { id } => {
                        state.assign(id.map(Offer::$variant), true);
                    }
                    $device::Event::Finished => {
                        state.finished = true;
                        device.destroy();
                    }
                    _ => {}
                }
            }
        }

        impl Dispatch<$offer::$offer_type, OfferData> for State {
            fn event(
                _: &mut Self,
                _: &$offer::$offer_type,
                event: $offer::Event,
                data: &OfferData,
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                if let $offer::Event::Offer { mime_type } = event
                    && let Ok(mut state) = data.lock()
                    && !state.invalid
                {
                    state.announced += 1;
                    if state.announced > MAX_OFFER_TYPES
                        || offer::add_type(&mut state.types, &mime_type).is_err()
                    {
                        state.invalid = true;
                    }
                }
            }
        }
    };
}

data_control!(
    ext_device,
    ExtDataControlDeviceV1,
    ext_offer,
    ExtDataControlOfferV1,
    Ext
);

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::Duration;

    fn pipe() -> (File, File) {
        let (rx, tx) = pipe_with(PipeFlags::CLOEXEC).expect("pipe");
        fcntl_setfl(&rx, OFlags::NONBLOCK).expect("non-blocking receive");
        (File::from(rx), File::from(tx))
    }

    #[test]
    fn wayland_pipe_requires_complete_bounded_contents() {
        for size in [MAX_BYTES, MAX_BYTES + 1] {
            let (read, mut write) = pipe();
            let handle = std::thread::spawn(move || write.write_all(&vec![b'a'; size]));
            let result = read_pipe(read, Instant::now() + DEADLINE);
            handle.join().unwrap().unwrap();
            if size == MAX_BYTES {
                assert_eq!(result.ok().unwrap().0.len(), size);
            } else {
                assert!(matches!(result, Err(ClipboardError::CaptureLimit { .. })));
            }
        }
    }

    #[test]
    fn wayland_pipe_rejects_prefix_when_the_writer_never_closes() {
        let (read, mut write) = pipe();
        write
            .write_all(b"prefix which must not be accepted")
            .unwrap();
        assert!(matches!(
            read_pipe(read, Instant::now() + Duration::from_millis(20)),
            Err(ClipboardError::CaptureFailed { .. })
        ));
    }
}
data_control!(
    wlr_device,
    ZwlrDataControlDeviceV1,
    wlr_offer,
    ZwlrDataControlOfferV1,
    Wlr
);
