//! Local X11 passive grabs. All lock variants are acquired with checked X11
//! requests; closing this dedicated connection rolls back partial success.

use super::{Delivery, HotkeyBinding, HotkeyError, HotkeyStatus, Key, ALT, CTRL, SHIFT, SUPER};
use crate::host_lifecycle::x11_transport::{connect, DeadlineStream, X11_TIMEOUT};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::xkb::{BoolCtrl, ConnectionExt as _, PerClientFlag, ID};
use x11rb::protocol::xproto::{ConnectionExt as _, GrabMode, Mapping, ModMask};
use x11rb::protocol::{ErrorKind, Event};
use x11rb::rust_connection::RustConnection;

pub(super) struct Registration {
    stop: Option<SyncSender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl Registration {
    pub(super) fn register(
        binding: HotkeyBinding,
        delivery: Delivery,
    ) -> Result<Self, HotkeyError> {
        let deadline = Instant::now() + X11_TIMEOUT;
        let (stop, stopped) = mpsc::sync_channel(1);
        let (ready, initialized) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("ansa-hotkey-x11".into())
            .spawn(move || {
                let (connection, keycode) = match reserve(binding, deadline) {
                    Ok(route) => route,
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                if ready.send(Ok(())).is_err() {
                    return;
                }
                let mut down = false;
                loop {
                    match stopped.recv_timeout(Duration::from_millis(16)) {
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                    connection
                        .stream()
                        .set_deadline(Instant::now() + Duration::from_millis(100));
                    // Only bounded work per iteration so stop isn't starved by
                    // unrelated events, server failure or busy clients.
                    for _ in 0..64 {
                        match connection.poll_for_event() {
                            Ok(Some(Event::KeyPress(event)))
                                if event.detail == keycode && event.response_type & 0x80 == 0 =>
                            {
                                if !down {
                                    down = true;
                                    delivery.call();
                                }
                            }
                            Ok(Some(Event::KeyRelease(event)))
                                if event.detail == keycode && event.response_type & 0x80 == 0 =>
                            {
                                down = false;
                            }
                            Ok(Some(Event::MappingNotify(event)))
                                if event.request != Mapping::POINTER =>
                            {
                                return
                            }
                            Ok(Some(_)) => {}
                            Ok(None) => break,
                            Err(_) => return,
                        }
                    }
                }
            })
            .map_err(|error| {
                HotkeyError::Unavailable(format!(
                    "Could not start the X11 shortcut worker: {error}"
                ))
            })?;
        let registration = Self {
            stop: Some(stop),
            worker: Some(worker),
        };
        match initialized.recv_timeout(X11_TIMEOUT + Duration::from_millis(100)) {
            Ok(Ok(())) => Ok(registration),
            Ok(Err(error)) => Err(error), // Drop joins before returning.
            Err(_) => Err(HotkeyError::Unavailable(
                "The X11 shortcut worker did not finish registration before the deadline".into(),
            )),
        }
    }

    pub(super) fn status(&self) -> HotkeyStatus {
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
        {
            HotkeyStatus::Registered
        } else {
            HotkeyStatus::Degraded(
                "The X11 server or keymap changed; retry the shortcut registration",
            )
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.stop.take(); // Disconnect stop channel, without blocking.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

type XConnection = RustConnection<DeadlineStream>;

fn unavailable(error: impl std::fmt::Display) -> HotkeyError {
    HotkeyError::Unavailable(format!("X11 shortcut unavailable: {error}"))
}

fn reserve(binding: HotkeyBinding, deadline: Instant) -> Result<(XConnection, u8), HotkeyError> {
    let (conn, screen) = connect(deadline).map_err(unavailable)?;
    let setup = conn.setup();
    let root = setup.roots[screen].root;
    // A held shortcut should deliver one summon. Refuse the route if XKB
    // cannot give us real release events (not legacy repeat release/press pairs).
    if !conn
        .xkb_use_extension(1, 0)
        .map_err(unavailable)?
        .reply()
        .map_err(unavailable)?
        .supported
    {
        return Err(HotkeyError::Unsupported(
            "X11 global summon requires XKB auto-repeat detection",
        ));
    }
    let repeat = PerClientFlag::DETECTABLE_AUTO_REPEAT;
    let empty = BoolCtrl::from(0u32);
    let flags = conn
        .xkb_per_client_flags(
            u16::from(ID::USE_CORE_KBD),
            repeat,
            repeat,
            empty,
            empty,
            empty,
        )
        .map_err(unavailable)?
        .reply()
        .map_err(unavailable)?;
    if !flags.value.contains(repeat) {
        return Err(HotkeyError::Unsupported(
            "X11 did not enable XKB auto-repeat detection",
        ));
    }
    let first = setup.min_keycode;
    let count = setup
        .max_keycode
        .checked_sub(first)
        .and_then(|width| width.checked_add(1))
        .ok_or_else(|| unavailable("invalid X11 keymap range"))?;
    let keyboard = conn
        .get_keyboard_mapping(first, count)
        .map_err(unavailable)?
        .reply()
        .map_err(unavailable)?;
    let stride = usize::from(keyboard.keysyms_per_keycode);
    if stride == 0 || keyboard.keysyms.len() != usize::from(count) * stride {
        return Err(unavailable("invalid X11 keymap"));
    }
    let contains = |code: u8, symbols: &[u32]| -> bool {
        code != 0
            && code
                .checked_sub(first)
                .and_then(|relative| {
                    let index = usize::from(relative) * stride;
                    keyboard.keysyms.get(index..index + stride)
                })
                .is_some_and(|key_symbols| {
                    key_symbols.iter().any(|symbol| symbols.contains(symbol))
                })
    };
    let wanted = keysym(binding.key);
    // Prefer the first (unshifted) group. Upper/lower Latin symbols are
    // equivalent for key lookup; modifiers are matched separately by X11.
    let matches = |symbol: u32| -> bool {
        symbol == wanted || ((0x41..=0x5a).contains(&wanted) && symbol == wanted + 32)
    };
    let keycode = keyboard
        .keysyms
        .chunks_exact(stride)
        .position(|keys| matches(keys[0]))
        .or_else(|| {
            keyboard
                .keysyms
                .chunks_exact(stride)
                .position(|keys| keys.iter().copied().any(matches))
        })
        .map(|index| first + index as u8)
        .ok_or(HotkeyError::InvalidBinding(
            "The shortcut key is absent from the X11 keymap",
        ))?;
    let modifiers = conn
        .get_modifier_mapping()
        .map_err(unavailable)?
        .reply()
        .map_err(unavailable)?;
    let width = modifiers.keycodes.len() / 8;
    if width == 0 || modifiers.keycodes.len() != 8 * width {
        return Err(unavailable("invalid X11 modifier map"));
    }
    let bindings = modifiers.keycodes.chunks_exact(width);
    let mask_for = |symbols: &[u32]| -> Option<u16> {
        bindings
            .clone()
            .position(|row| row.iter().any(|code| contains(*code, symbols)))
            .map(|i| 1u16 << i)
    };
    let mut mask = 0;
    if binding.modifiers & CTRL != 0 {
        mask |= 1 << 2;
    }
    if binding.modifiers & SHIFT != 0 {
        mask |= 1;
    }
    if binding.modifiers & ALT != 0 {
        mask |= mask_for(&[0xffe9, 0xffea]).ok_or(HotkeyError::InvalidBinding(
            "The X11 keymap has no Alt modifier",
        ))?;
    }
    if binding.modifiers & SUPER != 0 {
        mask |= mask_for(&[0xffeb, 0xffec]).ok_or(HotkeyError::InvalidBinding(
            "The X11 keymap has no Super modifier",
        ))?;
    }
    if mask.count_ones() != binding.modifiers.count_ones() {
        return Err(HotkeyError::InvalidBinding(
            "The X11 keymap maps two requested modifiers to the same mask",
        ));
    }
    let mut locks = 1 << 1; // X11's standard Lock modifier.
    for (index, row) in bindings.enumerate() {
        if row
            .iter()
            .any(|code| contains(*code, &[0xffe5, 0xffe6, 0xff7f, 0xff14]))
        {
            locks |= 1 << index;
        }
    }
    if locks & mask != 0 {
        return Err(HotkeyError::InvalidBinding(
            "The X11 keymap overlaps a requested modifier with a lock modifier",
        ));
    }
    // Enumerate only the lock subsets. No AnyModifier grab: it would also
    // seize unrelated shortcuts.
    let mut subset = locks;
    loop {
        let grabbed = conn
            .grab_key(
                false,
                root,
                ModMask::from(mask | subset),
                keycode,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )
            .map_err(unavailable)?
            .check();
        if let Err(error) = grabbed {
            return Err(match &error {
                ReplyError::X11Error(error) if error.error_kind == ErrorKind::Access => {
                    HotkeyError::Conflict(binding)
                }
                _ => unavailable(error),
            });
        }
        if subset == 0 {
            break;
        }
        subset = (subset - 1) & locks;
    }
    Ok((conn, keycode))
}

fn keysym(key: Key) -> u32 {
    match key {
        Key::Character(c) => u32::from(c),
        Key::Function(f) => 0xffbe + u32::from(f) - 1,
        Key::Space => 0x20,
        Key::Enter => 0xff0d,
        Key::Escape => 0xff1b,
        Key::Tab => 0xff09,
        Key::Backspace => 0xff08,
        Key::Delete => 0xffff,
        Key::Insert => 0xff63,
        Key::Home => 0xff50,
        Key::End => 0xff57,
        Key::PageUp => 0xff55,
        Key::PageDown => 0xff56,
        Key::Up => 0xff52,
        Key::Down => 0xff54,
        Key::Left => 0xff51,
        Key::Right => 0xff53,
    }
}
