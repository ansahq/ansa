//! Dedicated-thread global hotkey registration, lifecycle and message loop.
//!
//! `RegisterHotKey` with a null window delivers `WM_HOTKEY` to the message queue
//! of the thread that registered it. Registration and the message loop must
//! therefore live on the same dedicated thread.
//!
//! Lifecycle:
//! 1. The worker creates its thread message queue.
//! 2. It registers the requested chord, then reports success (or the exact
//!    failure) to the caller through a bounded handshake channel.
//! 3. `register` does not return `Ok` until step 2 succeeds, so dropping a
//!    `HotkeyRegistration` can always post `WM_QUIT` to a live queue.
//! 4. On `WM_QUIT`, the worker unregisters the hotkey and exits; `Drop` joins
//!    the worker. The join is the synchronization point that guarantees the
//!    hotkey is released before `register` can succeed again.
//!
//! Two layers are exposed:
//! * [`HotkeyRegistration`] preserves the original single-shot API and the
//!   default `Ctrl+Space` binding used by the desktop UI.
//! * [`HotkeyManager`] adds probe/register/unregister/reconfigure with typed
//!   conflict errors. Reconfiguration releases the previous registration
//!   before attempting the new chord and restores the previous working binding
//!   when the new one is rejected, so a failed re-register never leaves a
//!   half-installed hotkey.
//!
//! The callback receives no content and runs on the worker thread. It should
//! only wake the UI (for example by sending on an `async_channel`); it must not
//! block.

use crate::shortcut::{ShortcutChord, ShortcutError};
use std::ffi::c_void;
use std::fmt;
use std::mem::MaybeUninit;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

const HOTKEY_ID: i32 = 1;
const PM_NOREMOVE: u32 = 0x0000;
const WM_HOTKEY: u32 = 0x0312;
const WM_QUIT: u32 = 0x0012;

#[repr(C)]
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct Msg {
    hwnd: *mut c_void,
    message: u32,
    w_param: usize,
    l_param: isize,
    time: u32,
    point: Point,
    // Present in the modern Win32 MSG layout. It must be included so
    // GetMessageW cannot write past this allocation.
    private: u32,
}

#[link(name = "user32")]
extern "system" {
    fn RegisterHotKey(hwnd: *mut c_void, id: i32, modifiers: u32, virtual_key: u32) -> i32;
    fn UnregisterHotKey(hwnd: *mut c_void, id: i32) -> i32;
    fn GetMessageW(message: *mut Msg, hwnd: *mut c_void, filter_min: u32, filter_max: u32) -> i32;
    fn PeekMessageW(
        message: *mut Msg,
        hwnd: *mut c_void,
        filter_min: u32,
        filter_max: u32,
        remove: u32,
    ) -> i32;
    fn PostThreadMessageW(thread_id: u32, message: u32, w_param: usize, l_param: isize) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentThreadId() -> u32;
    fn GetLastError() -> u32;
}

/// Why a hotkey could not be registered, probed or reconfigured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyError {
    /// The shortcut text or value is not a valid global chord.
    InvalidShortcut(String),
    /// A registration is already active on this manager.
    AlreadyRegistered {
        /// The currently registered chord.
        shortcut: String,
    },
    /// Another application or the system already owns the combination.
    Conflict {
        /// The requested chord.
        shortcut: String,
    },
    /// Windows rejected the registration for a non-conflict reason.
    Registration {
        /// The requested chord.
        shortcut: String,
        /// Raw Win32 error code.
        code: u32,
        /// Human-readable explanation.
        detail: String,
    },
    /// The worker thread could not be started or exited unexpectedly.
    Worker(String),
    /// A reconfigure attempt failed; the previous binding may have been restored.
    Reconfigure {
        /// The chord that was requested.
        requested: String,
        /// The previous chord, when one was active.
        previous: Option<String>,
        /// Whether the previous chord was successfully re-registered.
        previous_restored: bool,
        /// The underlying failure.
        cause: Box<HotkeyError>,
    },
}

impl HotkeyError {
    /// Returns `true` when the failure is an explicit registration conflict.
    pub fn is_conflict(&self) -> bool {
        match self {
            Self::Conflict { .. } => true,
            Self::Reconfigure { cause, .. } => cause.is_conflict(),
            _ => false,
        }
    }

    /// Returns the requested chord text, when the error refers to one.
    pub fn shortcut(&self) -> Option<&str> {
        match self {
            Self::AlreadyRegistered { shortcut }
            | Self::Conflict { shortcut }
            | Self::Registration { shortcut, .. } => Some(shortcut),
            Self::Reconfigure { requested, .. } => Some(requested),
            Self::InvalidShortcut(_) | Self::Worker(_) => None,
        }
    }
}

impl fmt::Display for HotkeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidShortcut(reason) => write!(formatter, "invalid shortcut: {reason}"),
            Self::AlreadyRegistered { shortcut } => {
                write!(
                    formatter,
                    "{shortcut} is already registered by this manager"
                )
            }
            Self::Conflict { shortcut } => write!(
                formatter,
                "{shortcut} is already registered by another application or by the system"
            ),
            Self::Registration {
                shortcut,
                code,
                detail,
            } => write!(
                formatter,
                "Windows rejected {shortcut} (Win32 error {code}: {detail})"
            ),
            Self::Worker(reason) => write!(formatter, "hotkey worker failed: {reason}"),
            Self::Reconfigure {
                requested,
                previous,
                previous_restored,
                cause,
            } => {
                let previous = previous.as_deref().unwrap_or("none");
                let restored = if *previous_restored {
                    "the previous binding was restored"
                } else {
                    "no binding is active"
                };
                write!(
                    formatter,
                    "could not reconfigure the hotkey to {requested} (previous: {previous}; {restored}): {cause}"
                )
            }
        }
    }
}

impl std::error::Error for HotkeyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Reconfigure { cause, .. } => Some(cause.as_ref()),
            _ => None,
        }
    }
}

impl From<ShortcutError> for HotkeyError {
    fn from(error: ShortcutError) -> Self {
        Self::InvalidShortcut(error.to_string())
    }
}

#[derive(Clone, Copy, Debug)]
struct WorkerFailure {
    code: u32,
    shortcut: ShortcutChord,
}

fn failure_to_error(failure: WorkerFailure) -> HotkeyError {
    let shortcut = failure.shortcut.to_string();
    match failure.code {
        1409 => HotkeyError::Conflict { shortcut },
        87 => HotkeyError::Registration {
            shortcut,
            code: failure.code,
            detail: "the key combination is invalid".to_owned(),
        },
        code => HotkeyError::Registration {
            shortcut,
            code,
            detail: registration_detail(code).to_owned(),
        },
    }
}

/// Owns a registered global hotkey and its dedicated message-loop thread.
///
/// Construct with [`HotkeyRegistration::register`] for the original
/// `Ctrl+Space` behaviour or [`HotkeyRegistration::register_chord`] for an
/// explicit chord. Dropping this value posts `WM_QUIT`, waits for the worker to
/// unregister the hotkey and exit, then returns. There is no detached worker.
#[derive(Debug)]
pub struct HotkeyRegistration {
    thread_id: u32,
    shortcut: ShortcutChord,
    worker: Option<JoinHandle<()>>,
}

impl HotkeyRegistration {
    /// Registers the default `Ctrl+Space` hotkey and starts the worker.
    ///
    /// The callback is invoked on the worker thread for each `WM_HOTKEY` and
    /// receives no content. Keep it non-blocking; waking the UI through a
    /// channel is the intended use.
    ///
    /// Returns only after the worker has successfully registered the hotkey.
    /// If the registration fails (for example because another application or
    /// the IME already owns `Ctrl+Space`), the exact Win32 error is returned and
    /// no thread is left running.
    pub fn register(callback: impl Fn() + Send + 'static) -> Result<Self, String> {
        Self::register_chord(ShortcutChord::default_summon(), callback)
    }

    /// Registers an explicit chord and starts the worker.
    ///
    /// The error string preserves the original [`Self::register`] behaviour for
    /// callers that do not use the typed [`HotkeyManager`] API.
    pub fn register_chord(
        chord: ShortcutChord,
        callback: impl Fn() + Send + 'static,
    ) -> Result<Self, String> {
        Self::register_chord_typed(chord, callback).map_err(|error| error.to_string())
    }

    /// Registers and immediately releases a chord, reporting whether it is free.
    ///
    /// This is a real `RegisterHotKey` round trip with a no-op callback. The
    /// worker is joined before returning, so no registration is left behind.
    pub fn probe_chord(chord: ShortcutChord) -> Result<(), HotkeyError> {
        let registration = Self::register_chord_typed(chord, || {})?;
        drop(registration);
        Ok(())
    }

    fn register_chord_typed(
        chord: ShortcutChord,
        callback: impl Fn() + Send + 'static,
    ) -> Result<Self, HotkeyError> {
        let (ready_sender, ready_receiver) = mpsc::sync_channel::<Result<u32, WorkerFailure>>(1);
        let worker_name = format!("ansa-hotkey-{}", chord.canonical());
        let worker = thread::Builder::new()
            .name(worker_name)
            .spawn(move || worker_main(Box::new(callback), chord, ready_sender))
            .map_err(|error| {
                HotkeyError::Worker(format!("failed to start the hotkey worker: {error}"))
            })?;

        match ready_receiver.recv() {
            Ok(Ok(thread_id)) => Ok(Self {
                thread_id,
                shortcut: chord,
                worker: Some(worker),
            }),
            Ok(Err(failure)) => {
                let _ = worker.join();
                Err(failure_to_error(failure))
            }
            Err(_) => {
                let _ = worker.join();
                Err(HotkeyError::Worker(
                    "the hotkey worker exited before reporting registration".to_owned(),
                ))
            }
        }
    }

    /// Returns the chord this registration owns.
    pub fn shortcut(&self) -> ShortcutChord {
        self.shortcut
    }

    /// Releases the hotkey and joins the worker.
    ///
    /// Equivalent to dropping the value, but explicit and idempotent by
    /// construction (consumes `self`).
    pub fn unregister(self) {}

    #[cfg(test)]
    pub(crate) fn thread_id_for_test(&self) -> u32 {
        self.thread_id
    }
}

impl Drop for HotkeyRegistration {
    fn drop(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };

        // The registration handshake guarantees the queue exists and the
        // worker is inside GetMessageW or about to enter it, so this post is
        // delivered and cannot race queue creation. If the worker already
        // exited, the post fails and join returns immediately.
        // SAFETY: WM_QUIT needs no pointer payload and the thread id came from
        // the live worker.
        unsafe {
            PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }

        // Join synchronizes with the worker's UnregisterHotKey call. A
        // callback that blocks forever will also block this drop; that is why
        // callbacks are documented as wake-up-only.
        let _ = worker.join();
    }
}

type SharedCallback = Arc<Mutex<Box<dyn Fn() + Send + 'static>>>;

/// Lifecycle owner for a single global hotkey.
///
/// The manager wraps [`HotkeyRegistration`] and keeps the callback alive across
/// reconfiguration. It never registers two chords at once on this manager, and
/// [`Self::reconfigure`] releases the old chord before requesting the new one.
pub struct HotkeyManager {
    callback: SharedCallback,
    registration: Option<HotkeyRegistration>,
    shortcut: Option<ShortcutChord>,
}

impl fmt::Debug for HotkeyManager {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HotkeyManager")
            .field("shortcut", &self.shortcut)
            .field("registered", &self.registration.is_some())
            .finish()
    }
}

impl HotkeyManager {
    /// Creates a manager with no active registration.
    pub fn new(callback: impl Fn() + Send + 'static) -> Self {
        Self {
            callback: Arc::new(Mutex::new(Box::new(callback))),
            registration: None,
            shortcut: None,
        }
    }

    /// Returns the active chord, if any.
    pub fn active(&self) -> Option<ShortcutChord> {
        self.shortcut
    }

    /// Registers a chord.
    ///
    /// Fails with [`HotkeyError::AlreadyRegistered`] when this manager already
    /// owns a chord; use [`Self::reconfigure`] for rebinding.
    pub fn register(&mut self, chord: ShortcutChord) -> Result<(), HotkeyError> {
        if let Some(active) = self.shortcut {
            return Err(HotkeyError::AlreadyRegistered {
                shortcut: active.to_string(),
            });
        }
        self.install(chord)
    }

    /// Parses and registers a textual shortcut.
    pub fn register_text(&mut self, text: &str) -> Result<(), HotkeyError> {
        let chord = ShortcutChord::parse(text)?;
        self.register(chord)
    }

    /// Releases the active registration. Calling it when idle succeeds.
    pub fn unregister(&mut self) -> Result<(), HotkeyError> {
        // Taking the registration drops it, which posts WM_QUIT and joins the
        // worker before returning.
        self.registration.take();
        self.shortcut = None;
        Ok(())
    }

    /// Probes whether a chord can be registered right now.
    ///
    /// If the manager already owns the same chord, the probe succeeds without
    /// touching the live registration. A different chord is registered and
    /// immediately released on a separate worker. This is a real round trip;
    /// it briefly owns the combination and does not inject input.
    pub fn probe(&self, chord: ShortcutChord) -> Result<(), HotkeyError> {
        if self.shortcut == Some(chord) {
            return Ok(());
        }
        HotkeyRegistration::probe_chord(chord)
    }

    /// Rebinds to a new chord.
    ///
    /// The previous registration is released first. If the new chord is
    /// rejected, the previous chord is re-registered when possible; the
    /// returned error states whether that restore succeeded. A failed
    /// re-register therefore never leaves a half-installed or stuck hotkey.
    pub fn reconfigure(&mut self, chord: ShortcutChord) -> Result<(), HotkeyError> {
        let previous = self.shortcut;
        if previous == Some(chord) {
            return Ok(());
        }

        self.registration.take();
        self.shortcut = None;

        match self.install(chord) {
            Ok(()) => Ok(()),
            Err(cause) => {
                let previous_restored = match previous {
                    Some(old) if old != chord => self.install(old).is_ok(),
                    _ => false,
                };
                Err(HotkeyError::Reconfigure {
                    requested: chord.to_string(),
                    previous: previous.map(|value| value.to_string()),
                    previous_restored,
                    cause: Box::new(cause),
                })
            }
        }
    }

    /// Parses and reconfigures to a textual shortcut.
    pub fn reconfigure_text(&mut self, text: &str) -> Result<(), HotkeyError> {
        let chord = ShortcutChord::parse(text)?;
        self.reconfigure(chord)
    }

    /// Re-establishes the active chord after a suspend, lock or session change.
    ///
    /// The adapter has no global session-notification subscription yet, so the
    /// host lifecycle must call this when it observes a resume. The old worker
    /// is joined before the new registration is attempted; a failure leaves no
    /// duplicate or dead registration behind. Calling it while idle succeeds.
    pub fn reapply(&mut self) -> Result<(), HotkeyError> {
        let Some(chord) = self.shortcut else {
            return Ok(());
        };
        self.registration.take();
        self.shortcut = None;
        self.install(chord)
    }

    fn install(&mut self, chord: ShortcutChord) -> Result<(), HotkeyError> {
        let callback = Arc::clone(&self.callback);
        let registration = HotkeyRegistration::register_chord_typed(chord, move || {
            let callback = match callback.lock() {
                Ok(callback) => callback,
                Err(poisoned) => poisoned.into_inner(),
            };
            callback();
        })?;
        self.registration = Some(registration);
        self.shortcut = Some(chord);
        Ok(())
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        // The registration's Drop joins the worker, so no hotkey survives the
        // manager.
        self.registration.take();
        self.shortcut = None;
    }
}

fn worker_main(
    callback: Box<dyn Fn() + Send>,
    chord: ShortcutChord,
    ready: SyncSender<Result<u32, WorkerFailure>>,
) {
    let mut message = zeroed_message();

    // Force creation of this thread's message queue before registering.
    // SAFETY: the message buffer is valid and the optional HWND is null.
    unsafe {
        PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
    }

    // SAFETY: null HWND directs WM_HOTKEY to this thread's queue.
    let registered = unsafe {
        RegisterHotKey(
            std::ptr::null_mut(),
            HOTKEY_ID,
            chord.win32_modifiers(),
            chord.win32_virtual_key(),
        )
    };
    if registered == 0 {
        // SAFETY: GetLastError has no preconditions and is read immediately.
        let error = unsafe { GetLastError() };
        let _ = ready.send(Err(WorkerFailure {
            code: error,
            shortcut: chord,
        }));
        return;
    }

    // SAFETY: returns the calling thread's id.
    let thread_id = unsafe { GetCurrentThreadId() };
    if ready.send(Ok(thread_id)).is_err() {
        // The caller disappeared before observing success. Do not leave the
        // hotkey registered.
        // SAFETY: this thread registered the hotkey and is still alive.
        unsafe {
            UnregisterHotKey(std::ptr::null_mut(), HOTKEY_ID);
        }
        return;
    }

    loop {
        // SAFETY: the message buffer is valid for writes; filters 0,0 mean all
        // messages; a null HWND requests thread messages.
        let result = unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) };
        if result <= 0 {
            // 0 is WM_QUIT; -1 is a message-loop error. Both end the worker.
            break;
        }

        if message.message == WM_HOTKEY {
            // A panicking callback must not kill the hotkey worker. The panic
            // was already reported by the default panic hook.
            let _ = catch_unwind(AssertUnwindSafe(&callback));
        }
    }

    // SAFETY: this thread registered the hotkey, and it is still alive.
    unsafe {
        UnregisterHotKey(std::ptr::null_mut(), HOTKEY_ID);
    }
}

fn zeroed_message() -> Msg {
    // SAFETY: Msg is a plain C-layout struct of integers and raw pointers, for
    // which an all-zero bit pattern is valid and represents an empty message.
    unsafe { MaybeUninit::<Msg>::zeroed().assume_init() }
}

fn registration_detail(code: u32) -> &'static str {
    match code {
        87 => "the key combination is invalid",
        1409 => "the key combination is already registered by another application",
        _ => "the system rejected the hotkey registration",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    // Hotkey registration is process-global. Serialize tests that acquire it.
    static HOTKEY_TEST_LOCK: StdMutex<()> = StdMutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        HOTKEY_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn chord(text: &str) -> ShortcutChord {
        ShortcutChord::parse(text).expect("valid test chord")
    }

    #[test]
    fn handshake_dispatches_callback_reports_conflict_and_cleans_up() {
        let _guard = lock();

        let (sender, receiver) = mpsc::channel();
        let first = match HotkeyRegistration::register(move || {
            let _ = sender.send(());
        }) {
            Ok(registration) => registration,
            Err(error) => {
                // Ctrl+Space is commonly owned by an IME on some systems.
                // A conflict is a correct, reported failure, not a test bug.
                eprintln!("skipping global hotkey test: {error}");
                return;
            }
        };

        // Simulate the WM_HOTKEY that RegisterHotKey would post. This verifies
        // the worker loop and callback without injecting system input.
        // SAFETY: posting a synthetic thread message needs no payload pointers.
        let posted = unsafe {
            PostThreadMessageW(first.thread_id_for_test(), WM_HOTKEY, HOTKEY_ID as usize, 0)
        };
        assert_ne!(posted, 0, "failed to post the synthetic hotkey message");
        receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("hotkey callback was not invoked");

        let second = HotkeyRegistration::register(|| {});
        assert!(
            second.is_err(),
            "a second registration must fail while the first is active"
        );
        let second_error = second.unwrap_err();
        assert!(
            second_error.contains("1409") || second_error.contains("already registered"),
            "unexpected conflict error: {second_error}"
        );

        // Drop must join the worker and release the hotkey before returning.
        drop(first);
        let third = match HotkeyRegistration::register(|| {}) {
            Ok(registration) => registration,
            Err(error) => panic!("hotkey remained registered after drop: {error}"),
        };
        drop(third);
    }

    #[test]
    fn short_cut_chords_round_trip_through_the_worker() {
        let _guard = lock();
        let requested = chord("Ctrl+Alt+Shift+F24");
        let registration = match HotkeyRegistration::register_chord(requested, || {}) {
            Ok(registration) => registration,
            Err(error) => {
                eprintln!("skipping global hotkey registration test: {error}");
                return;
            }
        };
        assert_eq!(registration.shortcut(), requested);
        registration.unregister();

        // The pick-up after unregister proves the worker released the chord.
        let again = match HotkeyRegistration::register_chord(requested, || {}) {
            Ok(registration) => registration,
            Err(error) => panic!("hotkey remained registered after unregister: {error}"),
        };
        again.unregister();
    }

    #[test]
    fn probe_is_a_real_round_trip_and_leaves_no_registration() {
        let _guard = lock();
        let requested = chord("Ctrl+Alt+Shift+F23");
        if let Err(error) = HotkeyRegistration::probe_chord(requested) {
            eprintln!("skipping hotkey probe test: {error}");
            return;
        }
        let registration = match HotkeyRegistration::register_chord(requested, || {}) {
            Ok(registration) => registration,
            Err(error) => panic!("probe left the chord registered: {error}"),
        };
        registration.unregister();
    }

    #[test]
    fn manager_reports_conflicts_explicitly_and_recovers() {
        let _guard = lock();
        let requested = chord("Ctrl+Alt+Shift+F22");

        let mut manager = HotkeyManager::new(|| {});
        match manager.register(requested) {
            Ok(()) => {}
            Err(error) => {
                eprintln!("skipping hotkey manager conflict test: {error}");
                return;
            }
        }

        let conflict_text = HotkeyRegistration::register_chord(requested, || {})
            .expect_err("a second registration must conflict");
        assert!(
            conflict_text.contains("1409") || conflict_text.contains("already registered"),
            "unexpected conflict error: {conflict_text}"
        );
        let conflict = HotkeyError::Conflict {
            shortcut: requested.to_string(),
        };
        assert!(conflict.is_conflict());
        assert!(conflict.to_string().contains("already registered"));
        assert!(conflict.shortcut().is_some());
        assert!(conflict.to_string().contains(&requested.to_string()));

        // A manager-level second registration is rejected before touching the
        // OS, and the original stays live.
        let already = manager
            .register(chord("Ctrl+Alt+Shift+F21"))
            .expect_err("an active manager must reject register");
        assert!(matches!(already, HotkeyError::AlreadyRegistered { .. }));
        assert_eq!(manager.active(), Some(requested));

        manager.unregister().expect("unregister is infallible");
        assert_eq!(manager.active(), None);
        assert!(HotkeyRegistration::register_chord(requested, || {}).is_ok());
    }

    #[test]
    fn reconfigure_releases_the_old_chord() {
        let _guard = lock();
        let first = chord("Ctrl+Alt+Shift+F20");
        let second = chord("Ctrl+Alt+Shift+F19");

        let mut manager = HotkeyManager::new(|| {});
        if let Err(error) = manager.register(first) {
            eprintln!("skipping hotkey reconfigure test: {error}");
            return;
        }
        if let Err(error) = manager.reconfigure(second) {
            eprintln!("skipping hotkey reconfigure test: {error}");
            manager.unregister().ok();
            return;
        }
        assert_eq!(manager.active(), Some(second));

        // The first chord must now be free because reconfigure released it.
        let reclaimed = HotkeyRegistration::register_chord(first, || {})
            .expect("the previous chord must be released by reconfigure");
        reclaimed.unregister();

        // The new chord is still owned by the manager.
        assert!(HotkeyRegistration::register_chord(second, || {}).is_err());
        manager.unregister().expect("unregister is infallible");
    }

    #[test]
    fn a_failed_reconfigure_restores_the_previous_binding() {
        let _guard = lock();
        let working = chord("Ctrl+Alt+Shift+F18");
        let blocked = chord("Ctrl+Alt+Shift+F17");

        let mut blocker = HotkeyManager::new(|| {});
        if let Err(error) = blocker.register(blocked) {
            eprintln!("skipping failed-reconfigure test: {error}");
            return;
        }

        let mut manager = HotkeyManager::new(|| {});
        if let Err(error) = manager.register(working) {
            eprintln!("skipping failed-reconfigure test: {error}");
            blocker.unregister().ok();
            return;
        }

        let error = manager
            .reconfigure(blocked)
            .expect_err("reconfiguring onto an owned chord must fail");
        assert!(matches!(error, HotkeyError::Reconfigure { .. }));
        assert!(error.is_conflict(), "{error}");
        match &error {
            HotkeyError::Reconfigure {
                requested,
                previous,
                previous_restored,
                ..
            } => {
                assert_eq!(requested, &blocked.to_string());
                assert_eq!(previous.as_deref(), Some(working.to_string().as_str()));
                assert!(*previous_restored, "the previous binding must be restored");
            }
            _ => unreachable!(),
        }
        assert_eq!(manager.active(), Some(working));

        // The restored binding is real, not just bookkeeping.
        assert!(HotkeyRegistration::register_chord(working, || {}).is_err());

        manager.unregister().expect("unregister is infallible");
        blocker.unregister().expect("unregister is infallible");
    }

    #[test]
    fn unregister_and_drop_are_idempotent() {
        let _guard = lock();
        let requested = chord("Ctrl+Alt+Shift+F16");
        let mut manager = HotkeyManager::new(|| {});
        manager.unregister().expect("idle unregister succeeds");
        if let Err(error) = manager.register(requested) {
            eprintln!("skipping hotkey idempotence test: {error}");
            return;
        }
        manager.unregister().expect("unregister succeeds");
        manager.unregister().expect("second unregister succeeds");
        assert_eq!(manager.active(), None);

        // Re-register after teardown, then drop without an explicit unregister.
        if let Err(error) = manager.register(requested) {
            eprintln!("skipping hotkey idempotence test: {error}");
            return;
        }
        drop(manager);

        let reclaimed = HotkeyRegistration::register_chord(requested, || {})
            .expect("drop must release the registration");
        reclaimed.unregister();
    }

    #[test]
    fn reapply_reestablishes_the_active_chord_without_duplicates() {
        let _guard = lock();
        let requested = chord("Ctrl+Alt+Shift+F15");

        let mut manager = HotkeyManager::new(|| {});
        if let Err(error) = manager.register(requested) {
            eprintln!("skipping hotkey reapply test: {error}");
            return;
        }
        manager
            .reapply()
            .expect("reapply succeeds for a free chord");
        assert_eq!(manager.active(), Some(requested));
        // Exactly one registration exists: a foreign registration conflicts.
        assert!(HotkeyRegistration::register_chord(requested, || {}).is_err());
        manager.unregister().expect("unregister is infallible");
        assert!(HotkeyRegistration::register_chord(requested, || {}).is_ok());

        // Idle reapply is a no-op.
        manager.reapply().expect("idle reapply succeeds");
        assert_eq!(manager.active(), None);
    }

    #[test]
    fn invalid_shortcuts_are_rejected_before_registration() {
        let mut manager = HotkeyManager::new(|| {});
        for text in [
            "",
            "Space",
            "Ctrl+",
            "Ctrl+Ctrl+K",
            "Ctrl+F25",
            "Ctrl+Space\n",
        ] {
            let error = manager
                .register_text(text)
                .expect_err("invalid shortcut must be rejected");
            assert!(matches!(error, HotkeyError::InvalidShortcut(_)), "{text:?}");
        }
        assert_eq!(manager.active(), None);
    }
}
