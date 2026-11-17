//! Dedicated-thread global hotkey (`Ctrl+Space`) registration and message loop.
//!
//! `RegisterHotKey` with a null window delivers `WM_HOTKEY` to the message queue
//! of the thread that registered it. Registration and the message loop must
//! therefore live on the same dedicated thread.
//!
//! Lifecycle:
//! 1. The worker creates its thread message queue.
//! 2. It registers the hotkey, then reports success (or the exact failure) to
//!    the caller through a bounded handshake channel.
//! 3. `register` does not return `Ok` until step 2 succeeds, so dropping a
//!    `HotkeyRegistration` can always post `WM_QUIT` to a live queue.
//! 4. On `WM_QUIT`, the worker unregisters the hotkey and exits; `Drop` joins
//!    the worker. The join is the synchronization point that guarantees the
//!    hotkey is released before `register` can succeed again.
//!
//! The callback receives no content and runs on the worker thread. It should
//! only wake the UI (for example by sending on an `async_channel`); it must not
//! block.

use std::ffi::c_void;
use std::mem::MaybeUninit;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, SyncSender};
use std::thread::{self, JoinHandle};

const HOTKEY_ID: i32 = 1;
const MOD_CONTROL: u32 = 0x0002;
const MOD_NOREPEAT: u32 = 0x4000;
const VK_SPACE: u32 = 0x20;
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

/// Owns a registered global hotkey and its dedicated message-loop thread.
///
/// Construct with [`HotkeyRegistration::register`]. Dropping this value posts
/// `WM_QUIT`, waits for the worker to unregister the hotkey and exit, then
/// returns. There is no detached worker.
#[derive(Debug)]
pub struct HotkeyRegistration {
    thread_id: u32,
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
        let (ready_sender, ready_receiver) = mpsc::sync_channel::<Result<u32, String>>(1);
        let worker = thread::Builder::new()
            .name("ansa-hotkey-ctrl-space".to_owned())
            .spawn(move || worker_main(Box::new(callback), ready_sender))
            .map_err(|error| format!("failed to start the hotkey worker: {error}"))?;

        match ready_receiver.recv() {
            Ok(Ok(thread_id)) => Ok(Self {
                thread_id,
                worker: Some(worker),
            }),
            Ok(Err(error)) => {
                let _ = worker.join();
                Err(error)
            }
            Err(_) => {
                let _ = worker.join();
                Err("the hotkey worker exited before reporting registration".to_owned())
            }
        }
    }

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

fn worker_main(callback: Box<dyn Fn() + Send>, ready: SyncSender<Result<u32, String>>) {
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
            MOD_CONTROL | MOD_NOREPEAT,
            VK_SPACE,
        )
    };
    if registered == 0 {
        // SAFETY: GetLastError has no preconditions and is read immediately.
        let error = unsafe { GetLastError() };
        let _ = ready.send(Err(registration_error(error)));
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

fn registration_error(code: u32) -> String {
    let detail = match code {
        1409 => "the key combination is already registered by another application",
        87 => "the key combination is invalid",
        _ => "the system rejected the hotkey registration",
    };
    format!("failed to register the default Ctrl+Space hotkey (Win32 error {code}: {detail})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    // Hotkey registration is process-global. Serialize tests that acquire it.
    static HOTKEY_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn handshake_dispatches_callback_reports_conflict_and_cleans_up() {
        let _guard = HOTKEY_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

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
}
