//! Isolated `user32`/`kernel32` implementation for focus capture and
//! restoration.
//!
//! All native calls and `unsafe` blocks live here. The pure decisions are in
//! [`super::policy`] so they can be tested without touching foreground state.
//!
//! This module never:
//! * injects input or calls `SendInput`;
//! * calls `AttachThreadInput`;
//! * requests elevation or UIAccess;
//! * attempts to reach a secure desktop;
//! * reads memory from another process.
//!
//! It only performs documented `user32`/`kernel32` queries, hides the caller's
//! own launcher window on its owning thread, and requests foreground activation
//! of a previously captured, re-verified window.

use std::ffi::c_void;
use std::sync::Arc;

use super::policy::{
    capture_target_snapshot, dismiss_action, post_hide_action, verify_target_snapshot,
    CaptureSnapshot, DismissAction, DismissPolicyInput, PostHideAction, TargetIssue,
    TargetSnapshot,
};
use super::{FocusRestoreOutcome, FocusTarget};

const GA_PARENT: u32 = 1;
const GA_ROOT: u32 = 2;
const SW_HIDE: i32 = 0;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const STILL_ACTIVE: u32 = 259;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FileTime {
    low_date_time: u32,
    high_date_time: u32,
}

impl FileTime {
    fn to_u64(self) -> u64 {
        ((self.high_date_time as u64) << 32) | self.low_date_time as u64
    }
}

#[link(name = "user32")]
extern "system" {
    fn GetForegroundWindow() -> *mut c_void;
    fn GetWindowThreadProcessId(hwnd: *mut c_void, process_id: *mut u32) -> u32;
    fn IsWindow(hwnd: *mut c_void) -> i32;
    fn IsWindowVisible(hwnd: *mut c_void) -> i32;
    fn IsIconic(hwnd: *mut c_void) -> i32;
    fn GetAncestor(hwnd: *mut c_void, flags: u32) -> *mut c_void;
    fn GetDesktopWindow() -> *mut c_void;
    fn ShowWindow(hwnd: *mut c_void, command: i32) -> i32;
    fn SetForegroundWindow(hwnd: *mut c_void) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcessId() -> u32;
    fn GetCurrentThreadId() -> u32;
    fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> *mut c_void;
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn GetExitCodeProcess(process: *mut c_void, exit_code: *mut u32) -> i32;
    fn GetProcessTimes(
        process: *mut c_void,
        creation_time: *mut FileTime,
        exit_time: *mut FileTime,
        kernel_time: *mut FileTime,
        user_time: *mut FileTime,
    ) -> i32;
}

/// A query-only process handle held by a [`FocusTarget`].
///
/// Keeping the handle open prevents the process id from being recycled while
/// the target is alive. The handle is process-wide, not thread-affine, so the
/// target can be captured on the hotkey worker and used on the UI thread.
#[derive(Debug)]
pub(crate) struct OwnedProcessHandle {
    raw: *mut c_void,
}

// SAFETY: A process handle is a process-wide kernel handle. `GetExitCodeProcess`
// and `GetProcessTimes` are thread-agnostic read-only queries, and `CloseHandle`
// may be called from any thread in the process. `FocusTarget` is intentionally
// transferable from the hotkey worker to the UI thread.
unsafe impl Send for OwnedProcessHandle {}
// SAFETY: See the `Send` implementation. The handle is only used through
// `&self` for read-only identity queries, and no interior mutability is exposed.
unsafe impl Sync for OwnedProcessHandle {}

impl OwnedProcessHandle {
    fn open(process_id: u32) -> Option<Self> {
        // SAFETY: OpenProcess takes plain scalar arguments. PROCESS_QUERY_LIMITED_INFORMATION
        // requests only the access needed for GetExitCodeProcess/GetProcessTimes.
        let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
        if raw.is_null() {
            None
        } else {
            Some(Self { raw })
        }
    }

    fn is_alive(&self) -> bool {
        let mut exit_code = 0_u32;
        // SAFETY: the handle was returned by OpenProcess and is still owned by
        // this value; the out-parameter points to a live local.
        let queried = unsafe { GetExitCodeProcess(self.raw, &mut exit_code) } != 0;
        queried && exit_code == STILL_ACTIVE
    }

    fn creation_time(&self) -> Option<u64> {
        let mut creation = FileTime::default();
        let mut exit = FileTime::default();
        let mut kernel = FileTime::default();
        let mut user = FileTime::default();
        // SAFETY: the handle is live and all four out-parameters point to
        // distinct, valid FileTime values for the duration of the call.
        let queried =
            unsafe { GetProcessTimes(self.raw, &mut creation, &mut exit, &mut kernel, &mut user) }
                != 0;
        if queried {
            Some(creation.to_u64())
        } else {
            None
        }
    }

    fn creation_time_matches(&self, expected: u64) -> bool {
        self.creation_time() == Some(expected)
    }
}

impl Drop for OwnedProcessHandle {
    fn drop(&mut self) {
        // SAFETY: this value owns the handle and closes it exactly once.
        unsafe {
            CloseHandle(self.raw);
        }
    }
}

/// Native identity of a captured foreground window.
#[derive(Clone, Debug)]
pub(crate) struct TargetData {
    pub(crate) hwnd: isize,
    pub(crate) pid: u32,
    pub(crate) thread_id: u32,
    pub(crate) creation_time: u64,
    pub(crate) process: Arc<OwnedProcessHandle>,
}

pub(crate) fn capture(excluded: Option<isize>) -> Option<FocusTarget> {
    // SAFETY: GetForegroundWindow has no preconditions and returns a borrowed
    // handle or null. The handle is only inspected with documented queries.
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return None;
    }

    // Resolve the root window so a focused child cannot become the target. The
    // foreground window is normally top-level already.
    // SAFETY: the foreground handle is a live window while we inspect it; a
    // failed GetAncestor returns null and is handled below.
    let root = unsafe { GetAncestor(foreground, GA_ROOT) };
    let window = if root.is_null() { foreground } else { root };
    let hwnd = window as isize;

    // SAFETY: arbitrary integer handles are validated with IsWindow first.
    let is_window = unsafe { IsWindow(window) } != 0;
    if !is_window {
        return None;
    }

    let mut pid = 0_u32;
    // SAFETY: the window handle is live; the out-parameter points to a local.
    let thread_id = unsafe { GetWindowThreadProcessId(window, &mut pid) };
    if thread_id == 0 || pid == 0 {
        return None;
    }

    // SAFETY: GetCurrentProcessId has no preconditions.
    let current_pid = unsafe { GetCurrentProcessId() };
    let process = OwnedProcessHandle::open(pid);
    let process_opened = process.is_some();
    let process_alive = process
        .as_ref()
        .map(|process| process.is_alive())
        .unwrap_or(false);

    let snapshot = CaptureSnapshot {
        is_window,
        excluded: excluded == Some(hwnd),
        own_process: pid == current_pid,
        // SAFETY: the window handle is live; these queries have no side effects.
        visible: unsafe { IsWindowVisible(window) } != 0,
        // SAFETY: see above.
        minimized: unsafe { IsIconic(window) } != 0,
        // SAFETY: both calls are read-only; a top-level window's GA_PARENT is
        // the desktop window. Owned dialogs still pass this check.
        top_level: unsafe { GetAncestor(window, GA_PARENT) == GetDesktopWindow() },
        process_opened,
        process_alive,
    };
    if capture_target_snapshot(snapshot).is_err() {
        return None;
    }

    let process = process?;
    let creation_time = process.creation_time()?;

    Some(FocusTarget {
        data: TargetData {
            hwnd,
            pid,
            thread_id,
            creation_time,
            process: Arc::new(process),
        },
    })
}

/// Re-validates a captured target against live Win32 state.
pub(crate) fn verify(target: &FocusTarget, launcher_hwnd: isize) -> Result<(), TargetIssue> {
    let data = &target.data;
    let window = data.hwnd as *mut c_void;

    // SAFETY: arbitrary integer handles are validated with IsWindow first.
    let is_window = unsafe { IsWindow(window) } != 0;

    let mut snapshot = TargetSnapshot {
        is_window,
        same_as_launcher: data.hwnd == launcher_hwnd,
        // SAFETY: GetCurrentProcessId has no preconditions.
        own_process: data.pid == unsafe { GetCurrentProcessId() },
        ..TargetSnapshot::default()
    };

    if is_window {
        let mut pid = 0_u32;
        // SAFETY: the window handle is live; the out-parameter points to a local.
        let thread_id = unsafe { GetWindowThreadProcessId(window, &mut pid) };
        snapshot.same_pid = pid == data.pid;
        snapshot.same_thread = thread_id == data.thread_id;
        // SAFETY: the window handle is live; these queries have no side effects.
        snapshot.visible = unsafe { IsWindowVisible(window) } != 0;
        // SAFETY: see above.
        snapshot.minimized = unsafe { IsIconic(window) } != 0;
        // SAFETY: read-only top-level check.
        snapshot.top_level = unsafe { GetAncestor(window, GA_PARENT) == GetDesktopWindow() };
    }

    snapshot.alive = data.process.is_alive();
    snapshot.creation_time_matches = data.process.creation_time_matches(data.creation_time);

    verify_target_snapshot(snapshot)
}

pub(crate) fn launcher_is_foreground(launcher_hwnd: isize) -> bool {
    if launcher_hwnd == 0 {
        return false;
    }

    let launcher = launcher_hwnd as *mut c_void;

    // SAFETY: arbitrary integer handles are validated with IsWindow first.
    if unsafe { IsWindow(launcher) } == 0 {
        return false;
    }

    let mut owner_pid = 0_u32;
    // SAFETY: the window handle is live; the out-parameter points to a local.
    let owner_thread = unsafe { GetWindowThreadProcessId(launcher, &mut owner_pid) };
    // SAFETY: both queries have no preconditions.
    let current_thread = unsafe { GetCurrentThreadId() };
    let current_pid = unsafe { GetCurrentProcessId() };

    // A live window whose creating thread is the calling thread belongs to the
    // calling process. Check both so a stale or recycled thread id cannot make
    // a foreign window look like the launcher.
    if owner_thread != current_thread || owner_pid != current_pid {
        return false;
    }

    // SAFETY: GetForegroundWindow has no preconditions.
    let foreground = unsafe { GetForegroundWindow() };
    !foreground.is_null() && foreground == launcher
}

pub(crate) fn dismiss(
    launcher_hwnd: isize,
    target: Option<&FocusTarget>,
) -> Result<FocusRestoreOutcome, String> {
    if launcher_hwnd == 0 {
        return Err("cannot dismiss the launcher: null window handle".to_owned());
    }

    let launcher = launcher_hwnd as *mut c_void;

    // SAFETY: arbitrary integer handles are validated with IsWindow first.
    if unsafe { IsWindow(launcher) } == 0 {
        return Err("cannot dismiss the launcher: window handle is not a live window".to_owned());
    }

    let mut owner_pid = 0_u32;
    // SAFETY: the window handle is live; the out-parameter points to a local.
    let owner_thread = unsafe { GetWindowThreadProcessId(launcher, &mut owner_pid) };
    // SAFETY: both queries have no preconditions.
    let current_thread = unsafe { GetCurrentThreadId() };
    let current_pid = unsafe { GetCurrentProcessId() };

    if owner_thread != current_thread {
        return Err(format!(
            "dismiss_launcher must run on the launcher window's owning UI thread \
             (owner thread {owner_thread}, current thread {current_thread})"
        ));
    }
    if owner_pid != current_pid {
        return Err(
            "cannot dismiss the launcher: window does not belong to this process".to_owned(),
        );
    }

    // SAFETY: both calls are read-only.
    let desktop = unsafe { GetDesktopWindow() };
    if unsafe { GetAncestor(launcher, GA_PARENT) } != desktop {
        return Err(
            "cannot dismiss the launcher: window handle is not a top-level window".to_owned(),
        );
    }

    let launcher_owns_foreground = is_launcher_foreground(launcher);
    let target_check = if launcher_owns_foreground {
        target.map(|target| verify(target, launcher_hwnd))
    } else {
        None
    };
    let target_valid = matches!(&target_check, Some(Ok(())));

    // Process/window identity queries take time. Do not restore if the user
    // switched away while those checks were running.
    let launcher_owns_foreground = launcher_owns_foreground && is_launcher_foreground(launcher);

    let action = dismiss_action(DismissPolicyInput {
        launcher_owns_foreground,
        target_supplied: target.is_some(),
        target_valid,
    });

    hide_launcher(launcher)?;

    match action {
        DismissAction::HideOnly => {
            let reason = if !launcher_owns_foreground {
                "the launcher no longer owned the foreground window; the user's current focus was left unchanged".to_owned()
            } else if target.is_none() {
                "no foreground target was captured; the launcher was hidden without restoring focus"
                    .to_owned()
            } else {
                let issue = match target_check {
                    Some(Err(issue)) => issue.reason(),
                    _ => "the captured target was not verified",
                };
                format!("the captured foreground target could not be restored: {issue}")
            };
            Ok(FocusRestoreOutcome::Skipped { reason })
        }
        DismissAction::HideAndRestore => {
            let target = target.expect("restore action requires a target");

            // Re-verify after hiding: the window can disappear or change while
            // ShowWindow runs.
            if let Err(issue) = verify(target, launcher_hwnd) {
                return Ok(FocusRestoreOutcome::Denied {
                    reason: format!(
                        "the captured foreground target changed after the launcher was hidden: {}",
                        issue.reason()
                    ),
                });
            }

            let window = target.data.hwnd as *mut c_void;

            // Hiding may expose the target, or a different app. A different
            // foreground app is not evidence that we may steal focus from it.
            // SAFETY: read-only foreground/root-window queries with no retained pointers.
            let foreground = unsafe { GetForegroundWindow() };
            let root = if foreground.is_null() {
                foreground
            } else {
                let root = unsafe { GetAncestor(foreground, GA_ROOT) };
                if root.is_null() {
                    foreground
                } else {
                    root
                }
            };
            match post_hide_action(root == window, root == launcher, !root.is_null()) {
                PostHideAction::AlreadyRestored => return Ok(FocusRestoreOutcome::Restored),
                PostHideAction::LeaveUnchanged => return Ok(FocusRestoreOutcome::Skipped {
                    reason: "another window became foreground while dismissing; its focus was left unchanged".into(),
                }),
                PostHideAction::RequestActivation => {},
            }

            // SAFETY: the target was re-verified as a live window. Windows may
            // still deny activation; success is decided by observation, not by
            // this return value.
            let _ = unsafe { SetForegroundWindow(window) };

            if is_foreground(window) {
                Ok(FocusRestoreOutcome::Restored)
            } else {
                Ok(FocusRestoreOutcome::Denied {
                    reason: "Windows denied foreground activation to the captured target; the launcher is hidden and focus was not restored".to_owned(),
                })
            }
        }
    }
}

fn is_launcher_foreground(launcher: *mut c_void) -> bool {
    // SAFETY: GetForegroundWindow has no preconditions; GetAncestor is only
    // called on a non-null foreground handle.
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return false;
    }
    // Treat focus inside a launcher child window as the launcher owning focus.
    foreground == launcher || unsafe { GetAncestor(foreground, GA_ROOT) } == launcher
}

fn is_foreground(window: *mut c_void) -> bool {
    // SAFETY: GetForegroundWindow has no preconditions; GetAncestor is only
    // called on a non-null foreground handle.
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return false;
    }
    foreground == window || unsafe { GetAncestor(foreground, GA_ROOT) } == window
}

fn hide_launcher(launcher: *mut c_void) -> Result<(), String> {
    // SAFETY: the caller already verified that this thread owns the live
    // launcher window. ShowWindow has no pointer payload beyond the handle.
    unsafe {
        ShowWindow(launcher, SW_HIDE);
    }

    // SAFETY: the handle was live before the call and remains owned by this
    // thread.
    if unsafe { IsWindowVisible(launcher) } != 0 {
        return Err(
            "cannot dismiss the launcher: the window remained visible after ShowWindow(SW_HIDE)"
                .to_owned(),
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[link(name = "user32")]
    extern "system" {
        fn CreateWindowExW(
            ex_style: u32,
            class_name: *const u16,
            window_name: *const u16,
            style: u32,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            parent: *mut c_void,
            menu: *mut c_void,
            instance: *mut c_void,
            param: *mut c_void,
        ) -> *mut c_void;
        fn DestroyWindow(hwnd: *mut c_void) -> i32;
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn hidden_test_window() -> *mut c_void {
        let class = wide("STATIC");
        let title = wide("Ansa focus test window");
        // SAFETY: both strings are NUL-terminated and alive for the call; the
        // optional parent/menu/instance/param pointers are null. A system class
        // needs no registration.
        unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        }
    }

    #[test]
    fn capture_is_safe_without_a_ui_thread() {
        let _ = capture(None);
        let _ = capture(Some(0));
    }

    #[test]
    fn capture_excludes_the_requested_root_window() {
        // SAFETY: GetForegroundWindow has no preconditions.
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.is_null() {
            return;
        }
        // SAFETY: the handle is non-null; GetAncestor is a read-only query.
        let root = unsafe { GetAncestor(foreground, GA_ROOT) };
        let excluded = if root.is_null() { foreground } else { root };

        // The foreground may change between the two calls on a busy desktop.
        // Whenever a target is returned, it must never be the excluded handle.
        if let Some(target) = capture(Some(excluded as isize)) {
            assert_ne!(target.data.hwnd, excluded as isize);
        }
    }

    #[test]
    fn launcher_is_foreground_rejects_invalid_and_foreign_handles() {
        assert!(!launcher_is_foreground(0));
        assert!(!launcher_is_foreground(1));

        // The desktop window is live but is not owned by this thread.
        // SAFETY: GetDesktopWindow has no preconditions.
        let desktop = unsafe { GetDesktopWindow() };
        assert!(!desktop.is_null());
        assert!(!launcher_is_foreground(desktop as isize));
    }

    #[test]
    fn launcher_is_foreground_rejects_an_owned_hidden_window() {
        // SAFETY: see hidden_test_window.
        let window = hidden_test_window();
        if window.is_null() {
            return; // No desktop/window station in this test environment.
        }
        assert!(!launcher_is_foreground(window as isize));
        // SAFETY: the window was created by this thread and is destroyed once.
        unsafe {
            DestroyWindow(window);
        }
    }

    #[test]
    fn dismiss_rejects_null_and_stale_handles() {
        assert!(dismiss(0, None).is_err());
        assert!(dismiss(1, None).is_err());
    }

    #[test]
    fn dismiss_rejects_windows_owned_by_another_thread() {
        // SAFETY: GetDesktopWindow has no preconditions.
        let desktop = unsafe { GetDesktopWindow() };
        assert!(!desktop.is_null());
        assert!(dismiss(desktop as isize, None).is_err());
    }

    #[test]
    fn dismiss_hides_an_owned_hidden_window_without_stealing_focus() {
        // SAFETY: see hidden_test_window.
        let window = hidden_test_window();
        if window.is_null() {
            return; // No desktop/window station in this test environment.
        }

        let result = dismiss(window as isize, None);
        // SAFETY: the window was created by this thread and is destroyed once.
        unsafe {
            DestroyWindow(window);
        }

        match result {
            Ok(FocusRestoreOutcome::Skipped { reason }) => {
                assert!(reason.contains("foreground") || reason.contains("focus"));
            }
            other => panic!("unexpected dismissal result: {other:?}"),
        }
    }

    #[test]
    fn process_identity_checks_the_creation_time() {
        // SAFETY: GetCurrentProcessId has no preconditions.
        let pid = unsafe { GetCurrentProcessId() };
        let process = OwnedProcessHandle::open(pid).expect("open current process");
        assert!(process.is_alive());

        let creation = process.creation_time().expect("read creation time");
        assert!(process.creation_time_matches(creation));
        assert!(!process.creation_time_matches(creation ^ 1));
    }
}
