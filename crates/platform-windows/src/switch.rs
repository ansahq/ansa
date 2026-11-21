//! Deterministic best-effort switch to an already running application window.
//!
//! Candidate selection is pure and unit-tested with synthetic window
//! descriptors. Only the final enumeration and activation are native. Windows
//! can deny foreground activation, so [`SwitchResult::Unverified`] is returned
//! unless the target window is *observed* as foreground after the request.
//!
//! No input is injected, no thread input is attached and no elevation is
//! requested.

use crate::start_menu::normalize_path_key;
use std::ffi::c_void;
use std::path::Path;

const GA_ROOT: u32 = 2;
const SW_RESTORE: i32 = 9;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const MAX_ENUMERATED_WINDOWS: usize = 4096;
const WINDOW_PATH_UNITS: usize = 32_768;

/// The observable result of a switch attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SwitchResult {
    /// The requested window was observed as the foreground window.
    Verified(isize),
    /// Windows reported that activation failed.
    Denied(String),
    /// Activation was requested but the foreground window did not match.
    Unverified(String),
}

/// One top-level window observation used by the pure candidate policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WindowDescriptor {
    pub(crate) hwnd: isize,
    pub(crate) image_path: String,
    pub(crate) visible: bool,
    pub(crate) minimized: bool,
    pub(crate) top_level: bool,
    pub(crate) own_process: bool,
}

/// Picks the lowest-handle eligible visible window whose image path equals the
/// requested executable.
///
/// Sorting by handle gives a stable choice for repeated calls and for any input
/// order. Minimized windows remain eligible and are restored before activation.
/// Hidden, non-top-level and same-process windows are never candidates.
pub(crate) fn select_candidate(target: &Path, windows: &[WindowDescriptor]) -> Option<isize> {
    let target_key = normalize_path_key(target);
    windows
        .iter()
        .filter(|window| {
            window.visible
                && window.top_level
                && !window.own_process
                && !window.image_path.is_empty()
                && normalize_path_key(Path::new(&window.image_path)) == target_key
        })
        .map(|window| window.hwnd)
        .min()
}

/// Enumerates windows and tries to switch to the requested executable.
///
/// Returns `None` when no eligible window exists; the caller should launch a
/// new process in that case.
pub(crate) fn attempt_switch(target: &Path) -> Option<SwitchResult> {
    let windows = enumerate_windows();
    let hwnd = select_candidate(target, &windows)?;

    let window = hwnd as *mut c_void;

    // SAFETY: arbitrary integer handles are validated with IsWindow first.
    if unsafe { IsWindow(window) } == 0 {
        return Some(SwitchResult::Denied(
            "the selected application window closed before it could be activated".to_owned(),
        ));
    }

    // Restore a minimized window before activation. ShowWindow is a no-op for
    // an ordinary visible window.
    // SAFETY: the handle was just validated as a live window; both calls have
    // no pointer payload beyond the handle.
    if unsafe { IsIconic(window) } != 0 {
        let _ = unsafe { ShowWindow(window, SW_RESTORE) };
    }

    // SAFETY: the handle is live. SetForegroundWindow may still be denied by
    // the foreground-activation policy.
    let requested = unsafe { SetForegroundWindow(window) };

    for attempt in 0..3 {
        if is_foreground(window) {
            return Some(SwitchResult::Verified(hwnd));
        }
        if attempt < 2 {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    if requested == 0 {
        Some(SwitchResult::Denied(
            "Windows denied the foreground-activation request for the application window"
                .to_owned(),
        ))
    } else {
        Some(SwitchResult::Unverified(
            "Windows accepted the activation request but the application window was not observed as foreground"
                .to_owned(),
        ))
    }
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

fn enumerate_windows() -> Vec<WindowDescriptor> {
    let mut windows = Vec::new();
    let state = &mut windows as *mut Vec<WindowDescriptor> as *mut c_void;

    // SAFETY: the callback receives a pointer to a live local `Vec` for the
    // duration of the call. EnumWindows is synchronous and does not retain it.
    unsafe {
        EnumWindows(enum_window, state);
    }
    windows
}

extern "system" fn enum_window(hwnd: *mut c_void, state: *mut c_void) -> i32 {
    // SAFETY: `state` points to the `Vec<WindowDescriptor>` owned by
    // `enumerate_windows` for the whole synchronous enumeration. Every window
    // query below is read-only and validated.
    unsafe {
        let windows = &mut *(state as *mut Vec<WindowDescriptor>);
        if windows.len() >= MAX_ENUMERATED_WINDOWS {
            return 0;
        }

        let mut pid = 0_u32;
        let thread = GetWindowThreadProcessId(hwnd, &mut pid);
        if thread == 0 || pid == 0 {
            return 1;
        }

        let own_process = pid == GetCurrentProcessId();
        let image_path = if own_process {
            String::new()
        } else {
            process_image_path(pid).unwrap_or_default()
        };

        windows.push(WindowDescriptor {
            hwnd: hwnd as isize,
            image_path,
            visible: IsWindowVisible(hwnd) != 0,
            minimized: IsIconic(hwnd) != 0,
            top_level: GetAncestor(hwnd, GA_ROOT) == hwnd,
            own_process,
        });
    }
    1
}

unsafe fn process_image_path(pid: u32) -> Option<String> {
    // SAFETY: OpenProcess is called with a scalar process id and requests only
    // limited query access. The handle is closed exactly once below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return None;
    }

    let mut buffer = vec![0_u16; WINDOW_PATH_UNITS];
    let mut length = buffer.len() as u32;
    // SAFETY: the handle is live, the buffer is valid for `length` UTF-16 units
    // and the length pointer references a live local.
    let queried =
        unsafe { QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length) };
    // SAFETY: the handle was returned by OpenProcess and is closed once here.
    unsafe {
        CloseHandle(process);
    }

    if queried == 0 {
        return None;
    }
    let length = length as usize;
    if length == 0 || length > buffer.len() {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..length]))
}

#[link(name = "user32")]
extern "system" {
    fn EnumWindows(
        callback: extern "system" fn(*mut c_void, *mut c_void) -> i32,
        state: *mut c_void,
    ) -> i32;
    fn GetForegroundWindow() -> *mut c_void;
    fn GetAncestor(hwnd: *mut c_void, flags: u32) -> *mut c_void;
    fn GetWindowThreadProcessId(hwnd: *mut c_void, process_id: *mut u32) -> u32;
    fn IsWindow(hwnd: *mut c_void) -> i32;
    fn IsWindowVisible(hwnd: *mut c_void) -> i32;
    fn IsIconic(hwnd: *mut c_void) -> i32;
    fn ShowWindow(hwnd: *mut c_void, command: i32) -> i32;
    fn SetForegroundWindow(hwnd: *mut c_void) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcessId() -> u32;
    fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> *mut c_void;
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn QueryFullProcessImageNameW(
        process: *mut c_void,
        flags: u32,
        name: *mut u16,
        size: *mut u32,
    ) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn window(hwnd: isize, image: &str) -> WindowDescriptor {
        WindowDescriptor {
            hwnd,
            image_path: image.to_owned(),
            visible: true,
            minimized: false,
            top_level: true,
            own_process: false,
        }
    }

    fn target() -> PathBuf {
        PathBuf::from(r"C:\Program Files\Example\example.exe")
    }

    #[test]
    fn picks_the_lowest_eligible_handle_independent_of_input_order() {
        let target = target();
        let mut windows = vec![
            window(40, r"C:\Program Files\Example\example.exe"),
            window(7, r"c:/program files/example/EXAMPLE.EXE"),
            window(12, r"C:\Program Files\Example\example.exe"),
        ];

        assert_eq!(select_candidate(&target, &windows), Some(7));
        windows.reverse();
        assert_eq!(select_candidate(&target, &windows), Some(7));
    }

    #[test]
    fn ignores_hidden_foreign_and_non_top_level_windows() {
        let target = target();
        let windows = vec![
            WindowDescriptor {
                visible: false,
                ..window(1, r"C:\Program Files\Example\example.exe")
            },
            WindowDescriptor {
                own_process: true,
                ..window(3, r"C:\Program Files\Example\example.exe")
            },
            WindowDescriptor {
                top_level: false,
                ..window(4, r"C:\Program Files\Example\example.exe")
            },
            WindowDescriptor {
                image_path: String::new(),
                ..window(5, "")
            },
        ];

        assert_eq!(select_candidate(&target, &windows), None);
    }

    #[test]
    fn minimized_windows_remain_eligible_for_restore_and_activation() {
        let target = target();
        let windows = vec![
            WindowDescriptor {
                minimized: true,
                ..window(9, r"C:\Program Files\Example\example.exe")
            },
            window(12, r"C:\Program Files\Example\example.exe"),
        ];

        assert_eq!(select_candidate(&target, &windows), Some(9));
    }

    #[test]
    fn does_not_match_a_different_executable_or_a_basename_only() {
        let target = target();
        let windows = vec![
            window(1, r"C:\Other\example.exe"),
            window(2, r"C:\Program Files\Example\helper.exe"),
        ];
        assert_eq!(select_candidate(&target, &windows), None);
    }

    #[test]
    fn an_empty_window_set_has_no_candidate() {
        assert_eq!(select_candidate(&target(), &[]), None);
    }

    #[test]
    fn unverified_and_denied_results_are_not_successes() {
        assert!(matches!(
            SwitchResult::Verified(1),
            SwitchResult::Verified(_)
        ));
        assert!(matches!(
            SwitchResult::Denied("denied".to_owned()),
            SwitchResult::Denied(_)
        ));
        assert!(matches!(
            SwitchResult::Unverified("unverified".to_owned()),
            SwitchResult::Unverified(_)
        ));
    }
}
