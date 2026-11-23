//! Global shortcut registration and transactional lifecycle.
//!
//! Native callbacks must only enqueue a bounded summon intent. The registration
//! belongs to the UI owner, not a background future. No input is injected.

use crate::host_lifecycle::HostBackend;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos;
#[cfg(target_os = "linux")]
mod x11;

const CTRL: u8 = 1;
const ALT: u8 = 2;
const SHIFT: u8 = 4;
const SUPER: u8 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    Character(u8),
    Function(u8),
    Space,
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
}

/// Validated stored/registration chord. Text is portable; labels are native.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HotkeyBinding {
    modifiers: u8,
    key: Key,
}

impl HotkeyBinding {
    /// The established launcher default; may conflict with OS or IME bindings.
    pub const DEFAULT: Self = Self {
        modifiers: CTRL,
        key: Key::Space,
    };

    /// Parses the shortcut grammar shared with the settings store.
    pub fn parse(text: &str) -> Result<Self, HotkeyError> {
        fn invalid() -> HotkeyError {
            HotkeyError::InvalidBinding(
                "Use one to four distinct modifiers followed by a recognised key",
            )
        }
        if text.len() > 64 || !text.is_ascii() || text.bytes().any(|byte| byte.is_ascii_control()) {
            return Err(invalid());
        }
        let mut parts = text.rsplit('+');
        let key_text = parts.next().ok_or_else(invalid)?.to_ascii_lowercase();
        let key = match key_text.as_bytes() {
            [byte] if byte.is_ascii_alphanumeric() => Key::Character(byte.to_ascii_uppercase()),
            _ => match key_text.as_str() {
                "space" => Key::Space,
                "enter" | "return" => Key::Enter,
                "escape" | "esc" => Key::Escape,
                "tab" => Key::Tab,
                "backspace" => Key::Backspace,
                "delete" => Key::Delete,
                "insert" => Key::Insert,
                "home" => Key::Home,
                "end" => Key::End,
                "pageup" => Key::PageUp,
                "pagedown" => Key::PageDown,
                "up" => Key::Up,
                "down" => Key::Down,
                "left" => Key::Left,
                "right" => Key::Right,
                function => {
                    let digits = function.strip_prefix('f').ok_or_else(invalid)?;
                    let n: u8 = digits.parse().map_err(|_| invalid())?;
                    if !(1..=24).contains(&n) {
                        return Err(invalid());
                    }
                    Key::Function(n)
                }
            },
        };
        let mut modifiers = 0;
        for part in parts {
            let modifier = match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => CTRL,
                "alt" | "option" => ALT,
                "shift" => SHIFT,
                "super" | "meta" | "cmd" | "command" | "win" | "windows" => SUPER,
                _ => return Err(invalid()),
            };
            if modifiers & modifier != 0 {
                return Err(invalid());
            }
            modifiers |= modifier;
        }
        if modifiers == 0 {
            return Err(invalid());
        }
        Ok(Self { modifiers, key })
    }

    /// Canonical storage spelling, independent of display backend.
    pub fn canonical(self) -> String {
        self.format(["Ctrl", "Alt", "Shift", "Super"], false)
    }

    /// Native names without changing the persisted spelling.
    pub fn display(self, backend: HostBackend) -> String {
        match backend {
            HostBackend::MacOs => self.format(["Control", "Option", "Shift", "Command"], true),
            HostBackend::Windows => self.format(["Ctrl", "Alt", "Shift", "Win"], false),
            _ => self.canonical(),
        }
    }

    fn format(self, modifiers: [&str; 4], macos: bool) -> String {
        let mut name = String::new();
        for (mask, label) in [CTRL, ALT, SHIFT, SUPER].into_iter().zip(modifiers) {
            if self.modifiers & mask != 0 {
                name.push_str(label);
                name.push('+');
            }
        }
        let label = match self.key {
            Key::Character(byte) => {
                name.push(char::from(byte));
                return name;
            }
            Key::Function(n) => {
                name.push('F');
                name.push_str(&n.to_string());
                return name;
            }
            Key::Space => "Space",
            Key::Enter if macos => "Return",
            Key::Enter => "Enter",
            Key::Escape => "Escape",
            Key::Tab => "Tab",
            Key::Backspace if macos => "Delete",
            Key::Backspace => "Backspace",
            Key::Delete if macos => "Forward Delete",
            Key::Delete => "Delete",
            Key::Insert => "Insert",
            Key::Home => "Home",
            Key::End => "End",
            Key::PageUp => "PageUp",
            Key::PageDown => "PageDown",
            Key::Up => "Up",
            Key::Down => "Down",
            Key::Left => "Left",
            Key::Right => "Right",
        };
        name.push_str(label);
        name
    }
}

/// A rejected registration or lifecycle transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyError {
    InvalidBinding(&'static str),
    Conflict(HotkeyBinding),
    PermissionDenied(&'static str),
    Unsupported(&'static str),
    Unavailable(String),
    Persistence(String),
}

impl fmt::Display for HotkeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBinding(reason) => write!(f, "Invalid shortcut: {reason}"),
            Self::Conflict(binding) => write!(
                f,
                "{} is already reserved by the system or another application",
                binding.display(HostBackend::current())
            ),
            Self::PermissionDenied(reason) | Self::Unsupported(reason) => f.write_str(reason),
            Self::Unavailable(reason) => f.write_str(reason),
            Self::Persistence(reason) => write!(f, "Could not save the shortcut: {reason}"),
        }
    }
}

impl std::error::Error for HotkeyError {}

/// A live snapshot, not a native support/acceptance claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyStatus {
    Registered,
    Suspended,
    Degraded(&'static str),
    Unsupported(&'static str),
}

/// Single-use registration identity. Check via [`HotkeyRegistration::accepts`]
/// when consuming an event queued across a rebind, sleep or session switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HotkeyEvent(u64);

type Callback = Arc<Mutex<Box<dyn Fn(HotkeyEvent) + Send>>>;

#[derive(Clone)]
struct Delivery {
    id: HotkeyEvent,
    enabled: Arc<AtomicBool>,
    /// Native monotonic nanoseconds since boot at which this registration
    /// became deliverable. A queued press with an older or equal event time
    /// belongs to the previous registration.
    not_before: Arc<AtomicU64>,
    callback: Callback,
}

/// Converts Carbon `EventTime` seconds to monotonic nanoseconds since boot.
/// Carbon's `EventTime` is a `double`, not an integer tick. Non-finite and
/// negative values become 0; very large finite values saturate at `u64::MAX`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn event_time_nanos(seconds: f64) -> u64 {
    if !seconds.is_finite() || seconds < 0.0 {
        return 0;
    }
    let nanos = seconds * 1_000_000_000.0;
    if nanos >= u64::MAX as f64 {
        u64::MAX
    } else {
        nanos as u64
    }
}

impl Delivery {
    fn new(callback: Callback) -> Result<Self, HotkeyError> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| HotkeyError::Unavailable("Shortcut event counter exhausted".into()))?;
        Ok(Self {
            id: HotkeyEvent(id),
            enabled: Arc::new(AtomicBool::new(false)),
            not_before: Arc::new(AtomicU64::new(0)),
            callback,
        })
    }

    /// Publishes the commit time before enabling delivery. A queued native
    /// press timestamped at or before this fence is rejected even though the
    /// new registration ID is now current.
    fn commit(&self, fence: u64) {
        self.not_before.store(fence, Ordering::Release);
        self.enabled.store(true, Ordering::Release);
    }

    /// Timestamp-free path for Windows/Linux/tests.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    fn call(&self) {
        self.dispatch(None);
    }

    /// macOS path (and host tests). `press_time` is the native event time
    /// captured when the Carbon event occurred, not the callback dispatch
    /// time.
    #[cfg(any(target_os = "macos", test))]
    fn call_at(&self, press_time: u64) {
        self.dispatch(Some(press_time));
    }

    fn dispatch(&self, press_time: Option<u64>) {
        if let Ok(callback) = self.callback.try_lock() {
            if self.enabled.load(Ordering::Acquire) {
                // Only consult the fence for an enabled registration. A
                // disabled registration must not deliver at all.
                if let Some(press_time) = press_time {
                    // Conservatively reject equality: the commit fence and a
                    // queued pre-commit press can quantize to the same
                    // `EventTime`, and a missed summon is safer than a late
                    // one. This clock does not wrap.
                    if press_time <= self.not_before.load(Ordering::Acquire) {
                        return;
                    }
                }
                // A user callback must not unwind through an OS handler or
                // kill the dedicated message worker.
                let _ =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(self.id)));
            }
        }
    }
}

struct Active<N> {
    native: N,
    delivery: Delivery,
}

impl<N> Drop for Active<N> {
    fn drop(&mut self) {
        self.delivery.enabled.store(false, Ordering::Release);
        // Native unregistration follows after the gate is disabled.
    }
}

struct Lifecycle<N> {
    desired: HotkeyBinding,
    active: Option<Active<N>>,
    callback: Callback,
}

impl<N> Lifecycle<N> {
    fn start(binding: HotkeyBinding, callback: Callback) -> Self {
        Self {
            desired: binding,
            active: None,
            callback,
        }
    }

    fn replace(
        &mut self,
        binding: HotkeyBinding,
        register: impl FnOnce(HotkeyBinding, Delivery) -> Result<N, HotkeyError>,
        save: impl FnOnce(HotkeyBinding) -> Result<(), HotkeyError>,
    ) -> Result<(), HotkeyError> {
        // Read the native clock only at commit, after any slow registration or
        // save work, so a press that physically occurred while that work ran
        // is at or before the new registration's fence.
        self.replace_inner(binding, register, save, current_fence)
    }

    #[cfg(test)]
    fn replace_at(
        &mut self,
        binding: HotkeyBinding,
        register: impl FnOnce(HotkeyBinding, Delivery) -> Result<N, HotkeyError>,
        save: impl FnOnce(HotkeyBinding) -> Result<(), HotkeyError>,
        fence: u64,
    ) -> Result<(), HotkeyError> {
        self.replace_inner(binding, register, save, || fence)
    }

    fn replace_inner(
        &mut self,
        binding: HotkeyBinding,
        register: impl FnOnce(HotkeyBinding, Delivery) -> Result<N, HotkeyError>,
        save: impl FnOnce(HotkeyBinding) -> Result<(), HotkeyError>,
        fence: impl FnOnce() -> u64,
    ) -> Result<(), HotkeyError> {
        // Unchanged same-binding path: persist without a native transaction or
        // a new delivery fence.
        if self.desired == binding && self.active.is_some() {
            return save(binding);
        }
        let delivery = Delivery::new(self.callback.clone())?;
        let native = register(binding, delivery.clone())?;
        let pending = Active { native, delivery };
        // No conflict/save error releases a previously active registration.
        save(binding)?;
        drop(self.active.take());
        pending.delivery.commit(fence());
        self.active = Some(pending);
        self.desired = binding;
        Ok(())
    }

    fn suspend(&mut self) {
        drop(self.active.take());
    }

    fn accepts(&self, event: HotkeyEvent) -> bool {
        self.active
            .as_ref()
            .is_some_and(|active| active.delivery.id == event)
    }
}

#[cfg(target_os = "macos")]
fn current_fence() -> u64 {
    macos::current_fence()
}

#[cfg(not(target_os = "macos"))]
fn current_fence() -> u64 {
    0
}

#[cfg(target_os = "macos")]
type Native = macos::Registration;
#[cfg(target_os = "linux")]
type Native = x11::Registration;
#[cfg(windows)]
type Native = ansa_platform_windows::HotkeyManager;
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
type Native = ();

/// Owns the OS registration. Drop or suspend before destroying the host
/// event loop. On macOS this type stays on the AppKit thread.
pub struct HotkeyRegistration {
    backend: HostBackend,
    lifecycle: Lifecycle<Native>,
}

impl fmt::Debug for HotkeyRegistration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HotkeyRegistration")
            .field("backend", &self.backend)
            .field("binding", &self.binding())
            .field("status", &self.status())
            .finish()
    }
}

impl HotkeyRegistration {
    /// Drop-in default registration for UI without live rebinding.
    pub fn register(callback: impl Fn() + Send + 'static) -> Result<Self, HotkeyError> {
        Self::register_binding(HotkeyBinding::DEFAULT, move |_| callback())
    }

    /// Registers a binding; checks native registration before returning.
    pub fn register_binding(
        binding: HotkeyBinding,
        callback: impl Fn(HotkeyEvent) + Send + 'static,
    ) -> Result<Self, HotkeyError> {
        let backend = HostBackend::current();
        let mut lifecycle = Lifecycle::start(binding, Arc::new(Mutex::new(Box::new(callback))));
        lifecycle.replace(
            binding,
            |binding, delivery| native_register(backend, binding, delivery),
            |_| Ok(()),
        )?;
        Ok(Self { backend, lifecycle })
    }

    /// Validates stored settings. Invalid keys fall back to the default with a
    /// warning and do not overwrite the store. A conflict is surfaced, not
    /// silently replaced with a different chord.
    pub fn register_saved(
        saved: Option<&str>,
        callback: impl Fn(HotkeyEvent) + Send + 'static,
    ) -> Result<(Self, Option<HotkeyError>), HotkeyError> {
        let mut warning = None;
        let binding = match saved.map(HotkeyBinding::parse) {
            Some(Ok(binding)) => binding,
            Some(Err(error)) => {
                warning = Some(error);
                HotkeyBinding::DEFAULT
            }
            None => HotkeyBinding::DEFAULT,
        };
        let backend = HostBackend::current();
        let mut lifecycle = Lifecycle::start(binding, Arc::new(Mutex::new(Box::new(callback))));
        let install = |binding, delivery| native_register(backend, binding, delivery);
        match lifecycle.replace(binding, install, |_| Ok(())) {
            Ok(()) => {}
            Err(error @ HotkeyError::InvalidBinding(_)) if binding != HotkeyBinding::DEFAULT => {
                warning = Some(error);
                lifecycle.replace(HotkeyBinding::DEFAULT, install, |_| Ok(()))?;
            }
            Err(error) => return Err(error),
        }
        Ok((Self { backend, lifecycle }, warning))
    }

    /// The desired binding, retained even if suspend or resume fails.
    pub fn binding(&self) -> HotkeyBinding {
        self.lifecycle.desired
    }

    /// Checks registration health without pretending to exercise the keyboard.
    pub fn status(&self) -> HotkeyStatus {
        let Some(active) = &self.lifecycle.active else {
            return HotkeyStatus::Suspended;
        };
        native_status(&active.native)
    }

    /// Rejects queued events from a replaced or suspended registration.
    pub fn accepts(&self, event: HotkeyEvent) -> bool {
        self.lifecycle.accepts(event) && self.status() == HotkeyStatus::Registered
    }

    /// Attempts real acquisition before saving. Pass the existing settings
    /// store's atomic save callback, using `binding.canonical()`. Native
    /// conflict or persistence failure keeps the old registration intact.
    pub fn rebind(
        &mut self,
        binding: HotkeyBinding,
        save: impl FnOnce(HotkeyBinding) -> Result<(), String>,
    ) -> Result<(), HotkeyError> {
        if self.lifecycle.active.is_some() && self.binding() == binding {
            if let HotkeyStatus::Degraded(reason) | HotkeyStatus::Unsupported(reason) =
                self.status()
            {
                return Err(HotkeyError::Unavailable(reason.into()));
            }
        }
        let backend = self.backend;
        self.lifecycle.replace(
            binding,
            |binding, delivery| native_register(backend, binding, delivery),
            |binding| save(binding).map_err(HotkeyError::Persistence),
        )
    }

    /// Releases the active registration and joins its worker, if any.
    pub fn suspend(&mut self) {
        self.lifecycle.suspend();
    }

    /// Re-establishes the desired registration after the host observes a
    /// session change. Drops the old route first so no duplicate survives a
    /// failure. Host/system notifications are separate from this operation.
    pub fn resume(&mut self) -> Result<(), HotkeyError> {
        self.suspend();
        let backend = self.backend;
        self.lifecycle.replace(
            self.binding(),
            |binding, delivery| native_register(backend, binding, delivery),
            |_| Ok(()),
        )
    }
}

/// Capability boundary for [`crate::HostCapabilities`].
///
/// A compiled native route is not the same thing as a successful reservation:
/// X11 chords can still conflict, and macOS can report a Carbon conflict or
/// Secure Event Input. Report the route as degraded until it is actually
/// registered, and never claim a route on Wayland or headless sessions.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub(crate) fn capability(backend: HostBackend) -> crate::CapabilityStatus {
    use crate::CapabilityStatus::{Degraded, Unsupported};
    match backend {
        HostBackend::X11 => Degraded {
            reason: "The X11 passive-grab adapter is compiled in; a specific chord can still conflict with the window manager or another client"
                .to_owned(),
            alternative: "Choose another chord or use the desktop launcher".to_owned(),
        },
        HostBackend::MacOs => Degraded {
            reason: "HIToolbox registration is implemented on the AppKit thread; conflicts and Secure Event Input are reported at registration or status time"
                .to_owned(),
            alternative: "Choose another chord or use the desktop launcher".to_owned(),
        },
        HostBackend::Windows => Degraded {
            reason: "The native Windows registration adapter is compiled in; a specific chord can still conflict"
                .to_owned(),
            alternative: "Choose another chord or use the desktop launcher".to_owned(),
        },
        HostBackend::Wayland => Unsupported,
        HostBackend::Headless | HostBackend::Other => Unsupported,
    }
}

/// Availability prior to registration. `Registered` is never returned here.
pub fn availability(backend: HostBackend) -> HotkeyStatus {
    match backend {
        HostBackend::Wayland => HotkeyStatus::Unsupported(
            "Wayland global shortcuts require a GlobalShortcuts portal client; none is implemented. Open Ansa from the desktop instead.",
        ),
        HostBackend::Headless | HostBackend::Other => HotkeyStatus::Unsupported(
            "There is no interactive desktop global-shortcut route",
        ),
        _ => HotkeyStatus::Degraded("No working global shortcut has been registered yet"),
    }
}

fn native_register(
    backend: HostBackend,
    binding: HotkeyBinding,
    delivery: Delivery,
) -> Result<Native, HotkeyError> {
    if HostBackend::current() != backend {
        return Err(HotkeyError::Unavailable(
            "The display session changed; restart Ansa from the desktop".into(),
        ));
    }
    if let HotkeyStatus::Unsupported(reason) = availability(backend) {
        return Err(HotkeyError::Unsupported(reason));
    }
    #[cfg(target_os = "macos")]
    return macos::Registration::register(binding, delivery);
    #[cfg(target_os = "linux")]
    return x11::Registration::register(binding, delivery);
    #[cfg(windows)]
    {
        use ansa_platform_windows::{HotkeyError as WindowsError, HotkeyManager, ShortcutChord};
        let chord = ShortcutChord::parse(&binding.canonical())
            .map_err(|_| HotkeyError::InvalidBinding("Windows rejected the shortcut format"))?;
        let mut registration = HotkeyManager::new(move || delivery.call());
        registration.register(chord).map_err(|error| match error {
            WindowsError::Conflict { .. } => HotkeyError::Conflict(binding),
            WindowsError::Registration { code: 5, .. } => {
                HotkeyError::PermissionDenied("Windows denied global shortcut registration")
            }
            _ => HotkeyError::Unavailable(error.to_string()),
        })?;
        Ok(registration)
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = (binding, delivery);
        Err(HotkeyError::Unsupported(
            "No global-shortcut adapter for this platform",
        ))
    }
}

fn native_status(native: &Native) -> HotkeyStatus {
    #[cfg(target_os = "macos")]
    return native.status();
    #[cfg(target_os = "linux")]
    return native.status();
    #[cfg(windows)]
    {
        let _ = native;
        HotkeyStatus::Registered
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = native;
        HotkeyStatus::Unsupported("No global-shortcut adapter for this platform")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    struct Reservation {
        index: usize,
        owned: Rc<RefCell<[bool; 2]>>,
    }

    impl Drop for Reservation {
        fn drop(&mut self) {
            let mut owned = self.owned.borrow_mut();
            assert!(owned[self.index], "double release");
            owned[self.index] = false;
        }
    }

    #[test]
    fn validated_storage_spelling_and_native_display_are_distinct() {
        let binding = HotkeyBinding::parse("command+option+shift+control+Return").unwrap();
        assert_eq!(binding.canonical(), "Ctrl+Alt+Shift+Super+Enter");
        assert_eq!(
            binding.display(HostBackend::MacOs),
            "Control+Option+Shift+Command+Return"
        );
        for invalid in [
            "Space",
            "Ctrl+Ctrl+Space",
            "Ctrl+F25",
            "Ctrl+",
            "Ctrl+X\n",
            "Ctrl+Å",
        ] {
            assert!(matches!(
                HotkeyBinding::parse(invalid),
                Err(HotkeyError::InvalidBinding(_))
            ));
        }
    }

    #[test]
    fn capability_boundary_does_not_hide_compiled_routes_or_claim_wayland() {
        use crate::CapabilityStatus;
        assert!(matches!(
            capability(HostBackend::X11),
            CapabilityStatus::Degraded { .. }
        ));
        assert!(matches!(
            capability(HostBackend::MacOs),
            CapabilityStatus::Degraded { .. }
        ));
        assert_eq!(
            capability(HostBackend::Wayland),
            CapabilityStatus::Unsupported
        );
        assert_eq!(
            capability(HostBackend::Headless),
            CapabilityStatus::Unsupported
        );
        assert_eq!(
            capability(HostBackend::Other),
            CapabilityStatus::Unsupported
        );
    }

    #[test]
    fn no_portal_client_never_claims_wayland_registration() {
        let HotkeyStatus::Unsupported(reason) = availability(HostBackend::Wayland) else {
            panic!("Wayland must not silently fall back to XWayland")
        };
        assert!(reason.contains("portal"));
    }

    fn recording_callback() -> (Callback, Arc<Mutex<Vec<HotkeyEvent>>>) {
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let sink = delivered.clone();
        let callback: Callback = Arc::new(Mutex::new(Box::new(move |event| {
            sink.lock().unwrap().push(event)
        })));
        (callback, delivered)
    }

    #[test]
    fn slow_save_fences_presses_queued_before_the_new_registration_commits() {
        let default = HotkeyBinding::DEFAULT;
        let alt = HotkeyBinding::parse("Alt+K").unwrap();
        let (callback, delivered) = recording_callback();
        let mut lifecycle = Lifecycle::start(default, callback);
        lifecycle
            .replace_at(default, |_, _| Ok(()), |_| Ok(()), 100)
            .unwrap();
        let previous = lifecycle.active.as_ref().unwrap().delivery.clone();
        previous.call_at(101);
        assert_eq!(delivered.lock().unwrap().pop(), Some(previous.id));

        let candidate = Rc::new(RefCell::new(None));
        let slot = candidate.clone();
        lifecycle
            .replace_at(
                alt,
                move |_, delivery| {
                    *slot.borrow_mut() = Some(delivery.clone());
                    Ok(())
                },
                |_| {
                    // A deliberately slow save runs while the candidate is
                    // registered but not yet enabled. The press timestamp is
                    // older than the eventual commit fence.
                    Ok(())
                },
                1_000,
            )
            .unwrap();
        let current = candidate.borrow().clone().unwrap();
        assert!(lifecycle.accepts(current.id));

        // Queued during the save: rejected by the new fence.
        current.call_at(999);
        assert!(delivered.lock().unwrap().is_empty());
        // Equal to the commit fence is also rejected conservatively.
        current.call_at(1_000);
        assert!(delivered.lock().unwrap().is_empty());
        // The old registration is disabled, so an old queued press also does
        // not deliver.
        previous.call_at(999);
        assert!(delivered.lock().unwrap().is_empty());

        // Strictly after the fence is a fresh press.
        current.call_at(1_001);
        assert_eq!(delivered.lock().unwrap().pop(), Some(current.id));
    }

    #[test]
    fn failed_save_keeps_previous_registration_and_rejects_candidate_delivery() {
        let default = HotkeyBinding::DEFAULT;
        let alt = HotkeyBinding::parse("Alt+K").unwrap();
        let (callback, delivered) = recording_callback();
        let mut lifecycle = Lifecycle::start(default, callback);
        lifecycle
            .replace_at(default, |_, _| Ok(()), |_| Ok(()), 10)
            .unwrap();
        let previous = lifecycle.active.as_ref().unwrap().delivery.clone();
        previous.call_at(11);
        assert_eq!(delivered.lock().unwrap().pop(), Some(previous.id));

        let candidate = Rc::new(RefCell::new(None));
        let slot = candidate.clone();
        let result = lifecycle.replace_at(
            alt,
            move |_, delivery| {
                *slot.borrow_mut() = Some(delivery.clone());
                Ok(())
            },
            |_| Err(HotkeyError::Persistence("disk full".into())),
            50,
        );
        assert!(matches!(result, Err(HotkeyError::Persistence(_))));
        assert!(lifecycle.accepts(previous.id));

        // The previous registration still delivers with its own fence.
        previous.call_at(20);
        assert_eq!(delivered.lock().unwrap().pop(), Some(previous.id));

        // The uncommitted candidate never delivers and is not accepted.
        let rejected = candidate.borrow().clone().unwrap();
        assert!(!lifecycle.accepts(rejected.id));
        rejected.call_at(100);
        rejected.call();
        assert!(delivered.lock().unwrap().is_empty());
    }

    #[test]
    fn delivery_fence_rejects_equal_or_older_and_accepts_newer() {
        let (callback, delivered) = recording_callback();
        let delivery = Delivery::new(callback).unwrap();
        delivery.call_at(50);
        assert!(delivered.lock().unwrap().is_empty(), "disabled delivery");

        delivery.commit(100);
        delivery.call_at(99);
        assert!(delivered.lock().unwrap().is_empty());
        // Equal is rejected conservatively; only strictly newer is delivered.
        delivery.call_at(100);
        assert!(delivered.lock().unwrap().is_empty());
        delivery.call_at(101);
        assert_eq!(delivered.lock().unwrap().pop(), Some(delivery.id));
    }

    #[test]
    fn carbon_event_time_conversion_is_finite_non_negative_and_bounded() {
        assert_eq!(event_time_nanos(0.0), 0);
        assert_eq!(event_time_nanos(1.0), 1_000_000_000);
        assert_eq!(event_time_nanos(1.5), 1_500_000_000);
        assert_eq!(event_time_nanos(-1.0), 0);
        assert_eq!(event_time_nanos(f64::NAN), 0);
        assert_eq!(event_time_nanos(f64::INFINITY), 0);
        assert_eq!(event_time_nanos(f64::NEG_INFINITY), 0);
        assert_eq!(event_time_nanos(f64::MAX), u64::MAX);
    }

    #[test]
    fn simulated_thousand_rebind_suspend_resume_cycles_no_stale_delivery_or_leak() {
        let alt = HotkeyBinding::parse("Alt+K").unwrap();
        let default = HotkeyBinding::DEFAULT;
        let owned = Rc::new(RefCell::new([false, false]));
        let conflict = Cell::new(false);
        let last_attempt = RefCell::new(None);
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let sink = delivered.clone();
        let callback: Callback = Arc::new(Mutex::new(Box::new(move |event| {
            sink.lock().unwrap().push(event)
        })));
        let register = |binding, gate: Delivery| {
            let index = usize::from(binding == alt);
            if owned.borrow()[index] || (conflict.get() && binding == alt) {
                return Err(HotkeyError::Conflict(binding));
            }
            gate.call(); // disabled before the candidate is committed
            *last_attempt.borrow_mut() = Some(gate);
            owned.borrow_mut()[index] = true;
            Ok(Reservation {
                index,
                owned: owned.clone(),
            })
        };

        let mut lifecycle = Lifecycle::start(default, callback);
        for _ in 0..1_000 {
            lifecycle.replace(default, register, |_| Ok(())).unwrap();
            let previous = last_attempt.borrow().as_ref().unwrap().clone();
            assert_eq!(*owned.borrow(), [true, false]);
            conflict.set(true);
            assert!(matches!(
                lifecycle.replace(alt, register, |_| panic!("cannot save a conflicting chord")),
                Err(HotkeyError::Conflict(_))
            ));
            assert_eq!(lifecycle.desired, default);
            assert!(lifecycle.accepts(previous.id));
            assert_eq!(*owned.borrow(), [true, false]);
            conflict.set(false);

            let failed_save = lifecycle.replace(alt, register, |_| {
                Err(HotkeyError::Persistence("disk".into()))
            });
            assert!(matches!(failed_save, Err(HotkeyError::Persistence(_))));
            let rejected = last_attempt.borrow().as_ref().unwrap().clone();
            rejected.call();
            assert_eq!(*owned.borrow(), [true, false]);
            assert!(lifecycle.accepts(previous.id));
            previous.call();
            assert_eq!(delivered.lock().unwrap().pop(), Some(previous.id));

            lifecycle.replace(alt, register, |_| Ok(())).unwrap();
            let current = last_attempt.borrow().as_ref().unwrap().clone();
            assert_eq!(lifecycle.desired, alt);
            assert_eq!(*owned.borrow(), [false, true]);
            assert!(!lifecycle.accepts(previous.id));
            previous.call();
            current.call();
            assert_eq!(delivered.lock().unwrap().pop(), Some(current.id));
            lifecycle.suspend();
            assert_eq!(lifecycle.desired, alt);
            assert!(!lifecycle.accepts(current.id));
            assert_eq!(*owned.borrow(), [false, false]);
            current.call();
            assert!(delivered.lock().unwrap().is_empty());
        }
    }
}
