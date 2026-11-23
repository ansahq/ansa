//! Carbon/HIToolbox registered hotkeys on the AppKit thread. This is not an
//! event tap or keyboard monitor and does not request Input Monitoring.

use super::{Delivery, HotkeyBinding, HotkeyError, HotkeyStatus, Key, ALT, CTRL, SHIFT, SUPER};
use objc2::MainThreadMarker;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::ptr::{null_mut, NonNull};

type EventRef = *mut c_void;
type EventTargetRef = *mut c_void;
type EventHandlerRef = *mut c_void;
type EventHotKeyRef = *mut c_void;
type EventHandler = extern "C" fn(*mut c_void, EventRef, *mut c_void) -> i32;

const SIGNATURE: u32 = u32::from_be_bytes(*b"AnsA");
const EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const EVENT_HOTKEY_PRESSED: u32 = 5;
const EVENT_HOTKEY_RELEASED: u32 = 6;
const EVENT_PARAM_DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
const TYPE_EVENT_HOTKEY_ID: u32 = u32::from_be_bytes(*b"hkid");
const EVENT_NOT_HANDLED: i32 = -9874;
const EVENT_HOTKEY_EXISTS: i32 = -9878;
const EVENT_HOTKEY_EXCLUSIVE: u32 = 1;
const SECURE_INPUT: &str =
    "macOS Secure Event Input is active; use the Ansa desktop window until it ends";

#[repr(C)]
struct EventTypeSpec {
    class: u32,
    kind: u32,
}

#[repr(C)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn GetApplicationEventTarget() -> EventTargetRef;
    fn InstallEventHandler(
        target: EventTargetRef,
        handler: EventHandler,
        count: usize,
        types: *const EventTypeSpec,
        context: *mut c_void,
        installed: *mut EventHandlerRef,
    ) -> i32;
    fn RemoveEventHandler(handler: EventHandlerRef) -> i32;
    fn RegisterEventHotKey(
        code: u32,
        modifiers: u32,
        id: EventHotKeyId,
        target: EventTargetRef,
        options: u32,
        hotkey: *mut EventHotKeyRef,
    ) -> i32;
    fn UnregisterEventHotKey(hotkey: EventHotKeyRef) -> i32;
    fn GetEventKind(event: EventRef) -> u32;
    // CarbonEventsCore.h declares `typedef double EventTime;` and returns it
    // from these functions in the floating-point register. Both are declared
    // under the existing Carbon umbrella link. This Windows host cannot link a
    // macOS binary, so the linkage is compile-checked only; see
    // HOST-VALIDATION.md.
    fn GetEventTime(event: EventRef) -> f64;
    fn GetCurrentEventTime() -> f64;
    fn GetEventParameter(
        event: EventRef,
        name: u32,
        parameter_type: u32,
        actual_type: *mut u32,
        size: usize,
        actual_size: *mut usize,
        data: *mut c_void,
    ) -> i32;
    fn IsSecureEventInputEnabled() -> u8;
}

// Callback state is owned by the AppKit thread. No heap pointer is passed to
// Carbon, so a failed native teardown cannot later dereference freed Rust data.
struct HandlerState {
    handler: Option<NonNull<c_void>>,
    callbacks: BTreeMap<u32, Route>,
}

struct Route {
    delivery: Delivery,
    down: bool,
}

thread_local! {
    static STATE: RefCell<HandlerState> = const { RefCell::new(HandlerState {
        handler: None, callbacks: BTreeMap::new(),
    }) };
}

fn secure_input() -> bool {
    // SAFETY: read-only HIToolbox query, called on the AppKit thread.
    unsafe { IsSecureEventInputEnabled() != 0 }
}

/// Current Carbon event time used to fence a newly committed registration.
/// Called at commit, after slow registration/save work, so earlier queued
/// presses are rejected by the new delivery.
pub(super) fn current_fence() -> u64 {
    // SAFETY: GetCurrentEventTime has no preconditions and uses the same
    // seconds-since-boot domain as GetEventTime.
    super::event_time_nanos(unsafe { GetCurrentEventTime() })
}

extern "C" fn handle_hotkey(_: *mut c_void, event: EventRef, _: *mut c_void) -> i32 {
    if MainThreadMarker::new().is_none() || event.is_null() {
        return EVENT_NOT_HANDLED;
    }
    let mut id = EventHotKeyId {
        signature: 0,
        id: 0,
    };
    // SAFETY: Carbon supplies a live EventRef for this callback. The
    // output is a writable, correctly sized repr(C) EventHotKeyId.
    let status = unsafe {
        GetEventParameter(
            event,
            EVENT_PARAM_DIRECT_OBJECT,
            TYPE_EVENT_HOTKEY_ID,
            null_mut(),
            std::mem::size_of::<EventHotKeyId>(),
            null_mut(),
            (&mut id as *mut EventHotKeyId).cast(),
        )
    };
    if status != 0 || id.signature != SIGNATURE {
        return EVENT_NOT_HANDLED;
    }
    // SAFETY: Carbon guarantees the event lives through this handler.
    let kind = unsafe { GetEventKind(event) };
    // The timestamp records when Carbon created the event, not when this
    // callback runs. A press queued during a slow rebind carries the older
    // time and is rejected by the new delivery fence.
    // SAFETY: the same live EventRef also supports GetEventTime, which returns
    // EventTime (double seconds since boot).
    let event_time = super::event_time_nanos(unsafe { GetEventTime(event) });
    let secure = secure_input();
    let callback = STATE
        .try_with(|state| {
            let mut state = state.try_borrow_mut().ok()?;
            let route = state.callbacks.get_mut(&id.id)?;
            let delivery = match kind {
                EVENT_HOTKEY_PRESSED if !route.down && !secure => {
                    route.down = true;
                    Some(route.delivery.clone())
                }
                EVENT_HOTKEY_RELEASED => {
                    route.down = false;
                    None
                }
                _ => None,
            };
            Some(delivery)
        })
        .ok()
        .flatten();
    let Some(callback) = callback else {
        return EVENT_NOT_HANDLED;
    };
    if let Some(callback) = callback {
        // route.down was already latched for a pre-fence press, so its release
        // still resets state; only callback delivery is suppressed.
        callback.call_at(event_time);
    }
    0
}

pub(super) struct Registration {
    id: u32,
    hotkey: NonNull<c_void>,
    _main_thread: MainThreadMarker,
}

fn install_handler(target: EventTargetRef) -> Result<(), HotkeyError> {
    if STATE.with(|state| state.borrow().handler.is_some()) {
        return Ok(());
    }
    let events = [EVENT_HOTKEY_PRESSED, EVENT_HOTKEY_RELEASED].map(|kind| EventTypeSpec {
        class: EVENT_CLASS_KEYBOARD,
        kind,
    });
    let mut handler = null_mut();
    // SAFETY: valid target, callback with Carbon ABI, two type specs, no
    // user data and valid output storage.
    let status = unsafe {
        InstallEventHandler(
            target,
            handle_hotkey,
            events.len(),
            events.as_ptr(),
            null_mut(),
            &mut handler,
        )
    };
    let handler = NonNull::new(handler)
        .filter(|_| status == 0)
        .ok_or_else(|| {
            HotkeyError::Unavailable(format!(
                "HIToolbox could not install the shortcut handler ({status})"
            ))
        })?;
    STATE.with(|state| state.borrow_mut().handler = Some(handler));
    Ok(())
}

fn remove_unused_handler() {
    let handler = STATE
        .try_with(|state| {
            let state = state.borrow();
            if state.callbacks.is_empty() {
                state.handler
            } else {
                None
            }
        })
        .ok()
        .flatten();
    if let Some(handler) = handler {
        // SAFETY: called on AppKit thread and owns this installed handler.
        let status = unsafe { RemoveEventHandler(handler.as_ptr()) };
        if status == 0 {
            let _ = STATE.try_with(|state| state.borrow_mut().handler = None);
        } else {
            eprintln!("Ansa: macOS shortcut handler removal failed ({status})");
        }
    }
}

impl Registration {
    pub(super) fn register(
        binding: HotkeyBinding,
        delivery: Delivery,
    ) -> Result<Self, HotkeyError> {
        let mtm = MainThreadMarker::new().ok_or_else(|| {
            HotkeyError::Unavailable("macOS shortcuts must register on the AppKit thread".into())
        })?;
        let key = key_code(binding.key).ok_or(HotkeyError::InvalidBinding(
            "This key has no supported macOS hotkey code; use a letter, digit, or supported special key",
        ))?;
        let id = u32::try_from(delivery.id.0)
            .map_err(|_| HotkeyError::Unavailable("macOS shortcut event IDs exhausted".into()))?;
        // SAFETY: all Carbon registration/teardown stays on the AppKit thread.
        let target = unsafe { GetApplicationEventTarget() };
        if target.is_null() {
            return Err(HotkeyError::Unavailable(
                "HIToolbox has no application event target".into(),
            ));
        }
        install_handler(target)?;
        let mut hotkey = null_mut();
        // SAFETY: live target, properly encoded chord and valid output slot.
        // Only this signature and id are consumed by handle_hotkey.
        let status = unsafe {
            RegisterEventHotKey(
                key,
                modifiers(binding),
                EventHotKeyId {
                    signature: SIGNATURE,
                    id,
                },
                target,
                EVENT_HOTKEY_EXCLUSIVE,
                &mut hotkey,
            )
        };
        let Some(hotkey) = NonNull::new(hotkey).filter(|_| status == 0) else {
            remove_unused_handler();
            return Err(match status {
                EVENT_HOTKEY_EXISTS => HotkeyError::Conflict(binding),
                -54 => HotkeyError::PermissionDenied("macOS denied registration of this shortcut"),
                _ => {
                    HotkeyError::Unavailable(format!("HIToolbox rejected the shortcut ({status})"))
                }
            });
        };
        STATE.with(|state| {
            state.borrow_mut().callbacks.insert(
                id,
                Route {
                    delivery,
                    down: false,
                },
            );
        });
        Ok(Self {
            id,
            hotkey,
            _main_thread: mtm,
        })
    }

    pub(super) fn status(&self) -> HotkeyStatus {
        if secure_input() {
            // The matching release might be withheld during Secure Event
            // Input. Do not permanently latch a previously held hotkey.
            let _ = STATE.try_with(|state| {
                if let Some(route) = state.borrow_mut().callbacks.get_mut(&self.id) {
                    route.down = false;
                }
            });
            HotkeyStatus::Degraded(SECURE_INPUT)
        } else {
            HotkeyStatus::Registered
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        let _ = STATE.try_with(|state| {
            state.borrow_mut().callbacks.remove(&self.id);
        });
        // SAFETY: these successful native registrations are owned uniquely,
        // and !Send MainThreadMarker keeps teardown on the AppKit thread.
        let status = unsafe { UnregisterEventHotKey(self.hotkey.as_ptr()) };
        if status != 0 {
            eprintln!(
                "Ansa: macOS shortcut teardown error ({status}); restart Ansa before rebinding"
            );
        }
        remove_unused_handler();
    }
}

fn modifiers(binding: HotkeyBinding) -> u32 {
    [
        (CTRL, 1 << 12),
        (ALT, 1 << 11),
        (SHIFT, 1 << 9),
        (SUPER, 1 << 8),
    ]
    .into_iter()
    .filter(|(flag, _)| binding.modifiers & flag != 0)
    .fold(0, |mask, (_, native)| mask | native)
}

// Carbon virtual key codes describe physical ANSI positions, not glyphs
// translated through the active input source.
fn key_code(key: Key) -> Option<u32> {
    Some(match key {
        Key::Character(c @ b'A'..=b'Z') => {
            const LETTERS: [u32; 26] = [
                0, 11, 8, 2, 14, 3, 5, 4, 34, 38, 40, 37, 46, 45, 31, 35, 12, 15, 1, 17, 32, 9, 13,
                7, 16, 6,
            ];
            LETTERS[usize::from(c - b'A')]
        }
        Key::Character(c @ b'0'..=b'9') => {
            [29, 18, 19, 20, 21, 23, 22, 26, 28, 25][usize::from(c - b'0')]
        }
        Key::Function(n @ 1..=20) => [
            122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111, 105, 107, 113, 106, 64, 79, 80,
            90,
        ][usize::from(n - 1)],
        Key::Space => 49,
        Key::Enter => 36,
        Key::Escape => 53,
        Key::Tab => 48,
        Key::Backspace => 51,
        Key::Delete => 117,
        Key::Home => 115,
        Key::End => 119,
        Key::PageUp => 116,
        Key::PageDown => 121,
        Key::Up => 126,
        Key::Down => 125,
        Key::Left => 123,
        Key::Right => 124,
        Key::Insert | Key::Character(_) | Key::Function(_) => return None,
    })
}
