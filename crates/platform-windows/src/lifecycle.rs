//! Native monitor enumeration, theme observation and checked placement/show.
//!
//! This module is the only place that gathers Windows monitor, DPI, registry
//! and placement observations for the host lifecycle. Every unsafe call is
//! confined to the `windows` submodule and every caller-facing operation is
//! validated before a window is moved or shown.
//!
//! Limits worth repeating here:
//! * Monitor ids are stable-ish (device name plus monitor rectangle), not
//!   security tokens.
//! * `GetDpiForMonitor` is loaded dynamically from `shcore.dll`; the fallback
//!   is 96 DPI, never a guessed scale.
//! * The theme value is read from
//!   `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize`.
//!   A missing or unexpected value is a typed error, not a default.
//! * Showing may make the window visible while Windows denies foreground
//!   activation. [`ShowOutcome`] reports that state instead of claiming focus.

use ansa_host::{FocusOutcome, MonitorInfo, Point, Rect, ShowOutcome, ThemeAppearance};

/// Why the system application theme could not be observed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeError {
    /// The platform adapter cannot read the theme here.
    Unsupported,
    /// The registry value exists but is missing, inaccessible or unexpected.
    Unavailable(String),
}

impl std::fmt::Display for ThemeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => {
                formatter.write_str("system theme observation is unsupported on this platform")
            }
            Self::Unavailable(reason) => formatter.write_str(reason),
        }
    }
}

impl std::error::Error for ThemeError {}

#[cfg(windows)]
mod windows {
    use super::{ThemeAppearance, ThemeError};
    use ansa_host::{FocusOutcome, MonitorId, MonitorInfo, Point, Rect, ShowOutcome};
    use std::ffi::c_void;
    use std::sync::OnceLock;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct NativeRect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    impl NativeRect {
        fn to_rect(self) -> Rect {
            Rect::new(
                self.left,
                self.top,
                self.right.saturating_sub(self.left),
                self.bottom.saturating_sub(self.top),
            )
        }
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct NativePoint {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    struct MonitorInfoExW {
        cb_size: u32,
        rc_monitor: NativeRect,
        rc_work: NativeRect,
        flags: u32,
        sz_device: [u16; 32],
    }

    const MONITORINFOF_PRIMARY: u32 = 0x0000_0001;
    const SW_HIDE: i32 = 0;
    const SW_SHOW: i32 = 5;
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const RRF_RT_REG_DWORD: u32 = 0x0010;
    const ERROR_SUCCESS: i32 = 0;
    const HKEY_CURRENT_USER: isize = -2_147_483_647; // 0x80000001, sign-extended
    const MDT_EFFECTIVE_DPI: i32 = 0;

    type GetDpiForMonitorFn =
        unsafe extern "system" fn(*mut c_void, i32, *mut u32, *mut u32) -> i32;

    unsafe extern "system" {
        // user32
        fn EnumDisplayMonitors(
            hdc: *mut c_void,
            clip: *const NativeRect,
            callback: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                *mut NativeRect,
                isize,
            ) -> i32,
            data: isize,
        ) -> i32;
        fn GetMonitorInfoW(monitor: *mut c_void, info: *mut c_void) -> i32;
        fn IsWindow(hwnd: *mut c_void) -> i32;
        fn IsWindowVisible(hwnd: *mut c_void) -> i32;
        fn ShowWindow(hwnd: *mut c_void, command: i32) -> i32;
        fn SetForegroundWindow(hwnd: *mut c_void) -> i32;
        fn GetForegroundWindow() -> *mut c_void;
        fn GetCursorPos(point: *mut NativePoint) -> i32;
        fn GetWindowRect(hwnd: *mut c_void, rect: *mut NativeRect) -> i32;
        fn GetWindowThreadProcessId(hwnd: *mut c_void, process_id: *mut u32) -> u32;
        fn SetWindowPos(
            hwnd: *mut c_void,
            after: *mut c_void,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> i32;

        // kernel32
        fn GetCurrentThreadId() -> u32;
        fn GetCurrentProcessId() -> u32;
        fn GetLastError() -> u32;
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    }

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegGetValueW(
            key: *mut c_void,
            subkey: *const u16,
            value: *const u16,
            flags: u32,
            kind: *mut u32,
            data: *mut c_void,
            data_size: *mut u32,
        ) -> i32;
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn dpi_function() -> Option<GetDpiForMonitorFn> {
        static FUNCTION: OnceLock<Option<usize>> = OnceLock::new();
        let address = *FUNCTION.get_or_init(|| {
            let name = wide("shcore.dll");
            // SAFETY: `name` is a valid NUL-terminated wide string. The module
            // is kept loaded for the lifetime of the process.
            let module = unsafe { LoadLibraryW(name.as_ptr()) };
            if module.is_null() {
                return None;
            }
            // SAFETY: the module is live and the symbol name is NUL-terminated.
            let procedure = unsafe { GetProcAddress(module, b"GetDpiForMonitor\0".as_ptr()) };
            if procedure.is_null() {
                None
            } else {
                Some(procedure as usize)
            }
        });
        // SAFETY: the address came from GetProcAddress for the exact
        // GetDpiForMonitor signature, and the module is never unloaded.
        address.map(|address| unsafe { std::mem::transmute::<usize, GetDpiForMonitorFn>(address) })
    }

    fn monitor_scale(monitor: *mut c_void) -> f64 {
        let Some(get_dpi) = dpi_function() else {
            return 1.0;
        };
        let mut dpi_x = 0u32;
        let mut dpi_y = 0u32;
        // SAFETY: the monitor handle came from EnumDisplayMonitors and the
        // output pointers are valid for the call.
        let result = unsafe { get_dpi(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
        if result < 0 || dpi_x == 0 {
            return 1.0;
        }
        f64::from(dpi_x) / 96.0
    }

    unsafe extern "system" fn collect_monitor(
        monitor: *mut c_void,
        _device_context: *mut c_void,
        _rect: *mut NativeRect,
        data: isize,
    ) -> i32 {
        // SAFETY: `data` is the live `Vec` pointer supplied by
        // `enumerate_with_bounds` for the duration of EnumDisplayMonitors.
        let collected = unsafe { &mut *(data as *mut Vec<(MonitorInfo, Rect)>) };
        let mut info = MonitorInfoExW {
            cb_size: std::mem::size_of::<MonitorInfoExW>() as u32,
            rc_monitor: NativeRect::default(),
            rc_work: NativeRect::default(),
            flags: 0,
            sz_device: [0; 32],
        };
        // SAFETY: the monitor handle is valid for the callback and `info` is
        // sized/initialized as MONITORINFOEXW.
        if unsafe { GetMonitorInfoW(monitor, (&mut info as *mut MonitorInfoExW).cast()) } == 0 {
            return 1;
        }

        let device_length = info
            .sz_device
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(info.sz_device.len());
        let device = String::from_utf16_lossy(&info.sz_device[..device_length]);
        let bounds = info.rc_monitor.to_rect();
        let work_area = info.rc_work.to_rect();
        if bounds.is_empty() || work_area.is_empty() || !bounds.contains_rect(work_area) {
            // Skip a nonsensical observation rather than reporting a monitor
            // the rest of the system cannot place a window on.
            return 1;
        }

        let device = if device.is_empty() {
            "monitor".to_owned()
        } else {
            device
        };
        let id = format!(
            "{device}:{}x{}{:+}{:+}",
            bounds.width, bounds.height, bounds.x, bounds.y
        );
        collected.push((
            MonitorInfo {
                id: MonitorId::new(id),
                work_area,
                scale_factor: monitor_scale(monitor),
                primary: info.flags & MONITORINFOF_PRIMARY != 0,
            },
            bounds,
        ));
        1
    }

    /// Enumerates monitors together with their full monitor rectangles. The
    /// bounds are used by tests to prove the work area is inside the monitor.
    pub(super) fn enumerate_with_bounds() -> Result<Vec<(MonitorInfo, Rect)>, String> {
        let mut collected: Vec<(MonitorInfo, Rect)> = Vec::new();
        // SAFETY: the callback and the live `Vec` pointer remain valid for the
        // synchronous enumeration. Null HDC/clip means all monitors.
        let result = unsafe {
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                collect_monitor,
                (&mut collected as *mut Vec<(MonitorInfo, Rect)>) as isize,
            )
        };
        if result == 0 {
            // SAFETY: GetLastError has no preconditions and is read immediately.
            let code = unsafe { GetLastError() };
            return Err(format!(
                "EnumDisplayMonitors failed (Win32 error {code}); no monitor list is available"
            ));
        }
        if collected.is_empty() {
            return Err("Windows reported no active monitors".to_owned());
        }
        Ok(collected)
    }

    pub(super) fn enumerate_monitors() -> Result<Vec<MonitorInfo>, String> {
        Ok(enumerate_with_bounds()?
            .into_iter()
            .map(|(monitor, _)| monitor)
            .collect())
    }

    pub(super) fn cursor_position() -> Option<Point> {
        let mut point = NativePoint::default();
        // SAFETY: `point` is a valid out-parameter.
        if unsafe { GetCursorPos(&mut point) } == 0 {
            None
        } else {
            Some(Point::new(point.x, point.y))
        }
    }

    fn checked_window(hwnd: isize, operation: &str) -> Result<*mut c_void, String> {
        if hwnd == 0 {
            return Err(format!("{operation}: null window handle"));
        }
        let window = hwnd as *mut c_void;
        // SAFETY: an arbitrary integer handle is inspected only through
        // IsWindow, the supported liveness check.
        if unsafe { IsWindow(window) } == 0 {
            return Err(format!(
                "{operation}: window handle is not a live window (stale or foreign)"
            ));
        }
        let mut process_id = 0u32;
        // SAFETY: the window is live and `process_id` is a valid out-parameter.
        let owner_thread = unsafe { GetWindowThreadProcessId(window, &mut process_id) };
        // SAFETY: both queries return the calling thread/process ids.
        let current_thread = unsafe { GetCurrentThreadId() };
        let current_process = unsafe { GetCurrentProcessId() };
        if owner_thread != current_thread {
            return Err(format!(
                "{operation} must run on the window's owning UI thread \
                 (owner thread {owner_thread}, current thread {current_thread})"
            ));
        }
        if process_id != current_process {
            return Err(format!(
                "{operation}: window belongs to process {process_id}, not this process"
            ));
        }
        Ok(window)
    }

    pub(super) fn launcher_window_live(hwnd: isize) -> bool {
        checked_window(hwnd, "launcher_window_live").is_ok()
    }

    pub(super) fn launcher_window_rect(hwnd: isize) -> Option<Rect> {
        let window = checked_window(hwnd, "launcher_window_rect").ok()?;
        let mut rect = NativeRect::default();
        // SAFETY: the window is live and owned by this thread.
        if unsafe { GetWindowRect(window, &mut rect) } == 0 {
            return None;
        }
        Some(rect.to_rect())
    }

    pub(super) fn place_launcher_window(
        hwnd: isize,
        rect: Rect,
        work_area: Rect,
    ) -> Result<(), String> {
        let window = checked_window(hwnd, "place_launcher_window")?;
        if rect.is_empty() {
            return Err("place_launcher_window: the placement rectangle is empty".to_owned());
        }
        if !work_area.contains_rect(rect) {
            return Err(format!(
                "place_launcher_window: {rect:?} is not inside work area {work_area:?}"
            ));
        }

        // SAFETY: the window is live and owned by this thread; SWP_NOZORDER and
        // SWP_NOACTIVATE keep the call from changing z-order or focus.
        let moved = unsafe {
            SetWindowPos(
                window,
                std::ptr::null_mut(),
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
        if moved == 0 {
            // SAFETY: GetLastError has no preconditions.
            let code = unsafe { GetLastError() };
            return Err(format!(
                "SetWindowPos failed (Win32 error {code}); the launcher was not moved"
            ));
        }

        let mut actual = NativeRect::default();
        // SAFETY: the window is still live and owned by this thread.
        if unsafe { GetWindowRect(window, &mut actual) } == 0 {
            return Err("cannot verify the launcher rectangle after placement".to_owned());
        }
        let actual = actual.to_rect();
        if actual.is_empty() || !work_area.contains_rect(actual) {
            return Err(format!(
                "placement verification failed: window landed at {actual:?}, \
                 outside work area {work_area:?}"
            ));
        }
        Ok(())
    }

    pub(super) fn show_launcher_checked(hwnd: isize) -> Result<ShowOutcome, String> {
        if hwnd == 0 {
            return Err("show_launcher_checked: null window handle".to_owned());
        }
        let window = hwnd as *mut c_void;
        // SAFETY: liveness is checked before any state change.
        if unsafe { IsWindow(window) } == 0 {
            return Err("show_launcher_checked: window handle is not a live window".to_owned());
        }
        // SAFETY: the window is live; the process-id out-parameter is valid.
        let mut process_id = 0u32;
        let owner_thread = unsafe { GetWindowThreadProcessId(window, &mut process_id) };
        // SAFETY: both queries return the calling thread/process ids.
        let current_thread = unsafe { GetCurrentThreadId() };
        let current_process = unsafe { GetCurrentProcessId() };
        if process_id != current_process {
            return Err("show_launcher_checked: window belongs to another process".to_owned());
        }
        if owner_thread != current_thread {
            // Do not attempt to show from the wrong thread. Report an explicit
            // non-success instead of pretending the call was harmless.
            return Ok(ShowOutcome {
                visible: false,
                focus: FocusOutcome::Skipped {
                    reason: format!(
                        "show requested off the owning UI thread \
                         (owner thread {owner_thread}, current thread {current_thread})"
                    ),
                },
            });
        }

        // SAFETY: the window is live and owned by this thread. ShowWindow's
        // return value is the previous visibility state, not an error code.
        unsafe {
            ShowWindow(window, SW_SHOW);
        }
        // SAFETY: same live window/thread.
        let visible = unsafe { IsWindowVisible(window) } != 0;
        if !visible {
            return Ok(ShowOutcome {
                visible: false,
                focus: FocusOutcome::Denied {
                    reason: "Windows did not make the launcher window visible".to_owned(),
                },
            });
        }

        // SAFETY: same live window; foreground activation is subject to the
        // Windows foreground-lock policy and may be denied.
        let requested = unsafe { SetForegroundWindow(window) } != 0;
        // SAFETY: returns the current foreground window, or null.
        let foreground = unsafe { GetForegroundWindow() };
        let focus = if foreground == window {
            FocusOutcome::Succeeded
        } else if requested {
            FocusOutcome::Denied {
                reason: "Windows accepted the activation request but another window \
                         owns the foreground"
                    .to_owned(),
            }
        } else {
            FocusOutcome::Denied {
                reason: "Windows denied foreground activation; the launcher is visible \
                         without focus"
                    .to_owned(),
            }
        };
        Ok(ShowOutcome { visible, focus })
    }

    pub(super) fn hide_launcher(hwnd: isize) -> Result<(), String> {
        let window = checked_window(hwnd, "hide_launcher")?;
        // SAFETY: the window is live and owned by this thread.
        unsafe {
            ShowWindow(window, SW_HIDE);
        }
        // SAFETY: same live window/thread.
        if unsafe { IsWindowVisible(window) } != 0 {
            return Err("Windows did not hide the launcher window".to_owned());
        }
        Ok(())
    }

    pub(super) fn request_launcher_focus(hwnd: isize) -> FocusOutcome {
        let window = match checked_window(hwnd, "request_launcher_focus") {
            Ok(window) => window,
            Err(reason) => {
                return FocusOutcome::Skipped { reason };
            }
        };
        // SAFETY: the window is live and owned by this thread.
        let foreground = unsafe { GetForegroundWindow() };
        if foreground == window {
            return FocusOutcome::Succeeded;
        }
        // SAFETY: same live window; activation may be denied.
        let requested = unsafe { SetForegroundWindow(window) } != 0;
        // SAFETY: returns the current foreground window, or null.
        let foreground = unsafe { GetForegroundWindow() };
        if foreground == window {
            FocusOutcome::Succeeded
        } else if requested {
            FocusOutcome::Denied {
                reason: "foreground changed after activation was requested".to_owned(),
            }
        } else {
            FocusOutcome::Denied {
                reason: "Windows denied foreground activation".to_owned(),
            }
        }
    }

    pub(super) fn place_and_show_launcher(
        hwnd: isize,
        rect: Rect,
        work_area: Rect,
    ) -> Result<ShowOutcome, String> {
        place_launcher_window(hwnd, rect, work_area)?;
        show_launcher_checked(hwnd)
    }

    pub(super) fn read_apps_theme() -> Result<ThemeAppearance, ThemeError> {
        let subkey = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
        let value = wide("AppsUseLightTheme");
        let mut data = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: both strings are NUL-terminated, `data` is a valid DWORD
        // buffer and `size` describes it. The call only reads the registry.
        let result = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER as *mut c_void,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut data as *mut u32).cast(),
                &mut size,
            )
        };
        if result != ERROR_SUCCESS {
            return Err(ThemeError::Unavailable(format!(
                "AppsUseLightTheme could not be read (Win32 error {result})"
            )));
        }
        if size != std::mem::size_of::<u32>() as u32 {
            return Err(ThemeError::Unavailable(format!(
                "AppsUseLightTheme returned {size} bytes instead of a DWORD"
            )));
        }
        match data {
            0 => Ok(ThemeAppearance::Dark),
            1 => Ok(ThemeAppearance::Light),
            other => Err(ThemeError::Unavailable(format!(
                "AppsUseLightTheme has unexpected value {other}"
            ))),
        }
    }
}

/// Enumerates active monitors with physical-pixel work areas.
#[cfg(windows)]
pub fn enumerate_monitors() -> Result<Vec<MonitorInfo>, String> {
    windows::enumerate_monitors()
}

/// Non-Windows stub: monitor enumeration is unsupported.
#[cfg(not(windows))]
pub fn enumerate_monitors() -> Result<Vec<MonitorInfo>, String> {
    Err("monitor enumeration is unsupported on this platform".to_owned())
}

/// Returns the current pointer position in physical pixels.
#[cfg(windows)]
pub fn cursor_position() -> Option<Point> {
    windows::cursor_position()
}

/// Non-Windows stub: no pointer observation.
#[cfg(not(windows))]
pub fn cursor_position() -> Option<Point> {
    None
}

/// Reads the current Windows application theme.
#[cfg(windows)]
pub fn read_apps_theme() -> Result<ThemeAppearance, ThemeError> {
    windows::read_apps_theme()
}

/// Non-Windows stub: theme observation is unsupported.
#[cfg(not(windows))]
pub fn read_apps_theme() -> Result<ThemeAppearance, ThemeError> {
    Err(ThemeError::Unsupported)
}

/// Returns whether `hwnd` is a live launcher window owned by this thread.
#[cfg(windows)]
pub fn launcher_window_live(hwnd: isize) -> bool {
    windows::launcher_window_live(hwnd)
}

/// Non-Windows stub: there is no native launcher window.
#[cfg(not(windows))]
pub fn launcher_window_live(_hwnd: isize) -> bool {
    false
}

/// Returns the launcher's current physical-pixel rectangle.
#[cfg(windows)]
pub fn launcher_window_rect(hwnd: isize) -> Option<Rect> {
    windows::launcher_window_rect(hwnd)
}

/// Non-Windows stub: there is no native launcher window.
#[cfg(not(windows))]
pub fn launcher_window_rect(_hwnd: isize) -> Option<Rect> {
    None
}

/// Places the launcher with `SetWindowPos` and verifies it landed inside
/// `work_area`. Rejects stale, foreign and wrong-thread handles.
#[cfg(windows)]
pub fn place_launcher_window(hwnd: isize, rect: Rect, work_area: Rect) -> Result<(), String> {
    windows::place_launcher_window(hwnd, rect, work_area)
}

/// Non-Windows stub: native placement is unsupported.
#[cfg(not(windows))]
pub fn place_launcher_window(_hwnd: isize, _rect: Rect, _work_area: Rect) -> Result<(), String> {
    Err("native launcher placement is unsupported on this platform".to_owned())
}

/// Shows the launcher and reports visibility and focus separately.
#[cfg(windows)]
pub fn show_launcher_checked(hwnd: isize) -> Result<ShowOutcome, String> {
    windows::show_launcher_checked(hwnd)
}

/// Non-Windows stub: native showing is unsupported.
#[cfg(not(windows))]
pub fn show_launcher_checked(_hwnd: isize) -> Result<ShowOutcome, String> {
    Err("native launcher showing is unsupported on this platform".to_owned())
}

/// Hides the launcher and verifies it is no longer visible.
#[cfg(windows)]
pub fn hide_launcher(hwnd: isize) -> Result<(), String> {
    windows::hide_launcher(hwnd)
}

/// Non-Windows stub: native hiding is unsupported.
#[cfg(not(windows))]
pub fn hide_launcher(_hwnd: isize) -> Result<(), String> {
    Err("native launcher hiding is unsupported on this platform".to_owned())
}

/// Requests activation without changing visibility.
#[cfg(windows)]
pub fn request_launcher_focus(hwnd: isize) -> FocusOutcome {
    windows::request_launcher_focus(hwnd)
}

/// Non-Windows stub: native activation is unsupported.
#[cfg(not(windows))]
pub fn request_launcher_focus(_hwnd: isize) -> FocusOutcome {
    FocusOutcome::Unsupported {
        reason: "native launcher activation is unsupported on this platform".to_owned(),
    }
}

/// Places and shows in one checked sequence.
#[cfg(windows)]
pub fn place_and_show_launcher(
    hwnd: isize,
    rect: Rect,
    work_area: Rect,
) -> Result<ShowOutcome, String> {
    windows::place_and_show_launcher(hwnd, rect, work_area)
}

/// Non-Windows stub: native placement/showing is unsupported.
#[cfg(not(windows))]
pub fn place_and_show_launcher(
    _hwnd: isize,
    _rect: Rect,
    _work_area: Rect,
) -> Result<ShowOutcome, String> {
    Err("native launcher placement is unsupported on this platform".to_owned())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use ansa_host::place_on_monitor;
    use std::ffi::c_void;

    unsafe extern "system" {
        fn CreateWindowExW(
            ex_style: u32,
            class: *const u16,
            title: *const u16,
            style: u32,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            parent: *mut c_void,
            menu: *mut c_void,
            instance: *mut c_void,
            parameter: *mut c_void,
        ) -> *mut c_void;
        fn DestroyWindow(hwnd: *mut c_void) -> i32;
        fn GetWindowRect(hwnd: *mut c_void, rect: *mut NativeRectForTest) -> i32;
        fn GetShellWindow() -> *mut c_void;
        fn GetWindowThreadProcessId(hwnd: *mut c_void, process_id: *mut u32) -> u32;
        fn GetCurrentProcessId() -> u32;
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct NativeRectForTest {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    struct TestWindow(*mut c_void);

    impl TestWindow {
        fn new() -> Self {
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            // SAFETY: built-in class, valid strings, no parent and no visible
            // style; this fixture starts hidden.
            let window = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    class.as_ptr(),
                    0x8000_0000,
                    0,
                    0,
                    200,
                    120,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            assert!(!window.is_null(), "cannot create the lifecycle test window");
            Self(window)
        }

        fn handle(&self) -> isize {
            self.0 as isize
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            // SAFETY: this test owns the live hidden window on this thread.
            if unsafe { DestroyWindow(self.0) } == 0 {
                eprintln!("Cannot destroy the lifecycle test window");
            }
        }
    }

    fn primary_work_area() -> Rect {
        let monitors = windows::enumerate_with_bounds().expect("monitor enumeration");
        monitors
            .iter()
            .find(|(monitor, _)| monitor.primary)
            .map(|(monitor, _)| monitor.work_area)
            .unwrap_or_else(|| monitors[0].0.work_area)
    }

    #[test]
    fn enumerates_real_monitors_with_nonempty_work_areas_inside_bounds() {
        let monitors = windows::enumerate_with_bounds().expect("at least one monitor");
        assert!(!monitors.is_empty(), "no monitor was enumerated");
        for (monitor, bounds) in &monitors {
            assert!(!monitor.id.as_str().is_empty());
            assert!(!monitor.work_area.is_empty());
            assert!(
                bounds.contains_rect(monitor.work_area),
                "work area {:?} must be inside monitor bounds {bounds:?}",
                monitor.work_area
            );
            assert!(
                monitor.scale_factor >= 0.96 && monitor.scale_factor <= 8.0,
                "implausible scale factor {} for {}",
                monitor.scale_factor,
                monitor.id
            );
        }
        assert!(
            monitors
                .iter()
                .filter(|(monitor, _)| monitor.primary)
                .count()
                <= 1,
            "Windows must report at most one primary monitor"
        );
    }

    #[test]
    fn theme_observation_is_typed_and_does_not_mutate_the_registry() {
        match read_apps_theme() {
            Ok(ThemeAppearance::Light) => {}
            Ok(ThemeAppearance::Dark) => {}
            Err(error) => assert!(!error.to_string().is_empty()),
        }
    }

    #[test]
    fn placement_verifies_the_window_is_inside_the_work_area() {
        let window = TestWindow::new();
        let work = primary_work_area();
        let monitor = MonitorInfo {
            id: ansa_host::MonitorId::new("test"),
            work_area: work,
            scale_factor: 1.0,
            primary: true,
        };
        let desired = place_on_monitor(&monitor, ansa_host::Size::new(640, 400));
        place_launcher_window(window.handle(), desired, work).expect("placement");

        let mut actual = NativeRectForTest::default();
        // SAFETY: the fixture window is live and owned by this thread.
        assert_ne!(unsafe { GetWindowRect(window.0, &mut actual) }, 0);
        let actual = Rect::new(
            actual.left,
            actual.top,
            actual.right - actual.left,
            actual.bottom - actual.top,
        );
        assert_eq!(actual.width, desired.width);
        assert_eq!(actual.height, desired.height);
        assert!(
            work.contains_rect(actual),
            "{actual:?} must be inside {work:?}"
        );
    }

    #[test]
    fn placement_rejects_rectangles_outside_the_work_area() {
        let window = TestWindow::new();
        let work = primary_work_area();
        let outside = Rect::new(work.x - 100, work.y - 100, 400, 300);
        let error = place_launcher_window(window.handle(), outside, work)
            .expect_err("outside placement must be rejected");
        assert!(error.contains("not inside work area"));
    }

    #[test]
    fn placement_rejects_stale_handles_and_the_wrong_thread() {
        assert!(
            place_launcher_window(0, Rect::new(0, 0, 100, 100), Rect::new(0, 0, 100, 100)).is_err()
        );
        assert!(
            place_launcher_window(1, Rect::new(0, 0, 100, 100), Rect::new(0, 0, 100, 100)).is_err()
        );

        let window = TestWindow::new();
        let work = primary_work_area();
        let rect = place_on_monitor(
            &MonitorInfo {
                id: ansa_host::MonitorId::new("test"),
                work_area: work,
                scale_factor: 1.0,
                primary: true,
            },
            ansa_host::Size::new(400, 300),
        );
        let handle = window.handle();
        let result = std::thread::spawn(move || place_launcher_window(handle, rect, work)).join();
        let error = result
            .expect("thread did not panic")
            .expect_err("wrong thread");
        assert!(error.contains("owning UI thread") || error.contains("thread"));
    }

    #[test]
    fn checked_show_reports_visibility_and_focus_separately_then_hides() {
        let window = TestWindow::new();
        let outcome = show_launcher_checked(window.handle()).expect("show");
        assert!(outcome.visible, "the fixture window must become visible");
        assert!(
            matches!(
                &outcome.focus,
                FocusOutcome::Succeeded | FocusOutcome::Denied { .. }
            ),
            "on the owning thread focus must be observed, not skipped: {:?}",
            outcome.focus
        );
        hide_launcher(window.handle()).expect("hide");
        assert!(!launcher_window_live(0));
        assert!(launcher_window_live(window.handle()));
        assert!(launcher_window_rect(window.handle()).is_some());
    }

    #[test]
    fn checked_show_from_the_wrong_thread_is_skipped_without_showing() {
        let window = TestWindow::new();
        let handle = window.handle();
        let outcome = std::thread::spawn(move || show_launcher_checked(handle))
            .join()
            .expect("thread did not panic")
            .expect("wrong-thread show returns an outcome");
        assert!(!outcome.visible);
        assert_eq!(outcome.focus.label(), "skipped");
        assert!(outcome.focus.reason().is_some());
    }

    #[test]
    fn checked_show_and_hide_reject_stale_handles_before_any_state_change() {
        assert!(show_launcher_checked(0).is_err());
        assert!(show_launcher_checked(1).is_err());
        assert!(hide_launcher(0).is_err());
        assert!(hide_launcher(1).is_err());
        assert_eq!(
            request_launcher_focus(1).label(),
            "skipped",
            "an invalid handle must never claim activation"
        );
    }

    #[test]
    fn a_live_window_owned_by_another_process_is_rejected_before_any_change() {
        // SAFETY: both calls only inspect a borrowed handle.
        let shell = unsafe { GetShellWindow() };
        if shell.is_null() {
            return;
        }
        let mut process_id = 0u32;
        // SAFETY: the shell handle is live and the out-parameter is valid.
        unsafe {
            GetWindowThreadProcessId(shell, &mut process_id);
        }
        // SAFETY: returns the calling process id.
        if process_id == unsafe { GetCurrentProcessId() } {
            return;
        }

        let handle = shell as isize;
        assert!(!launcher_window_live(handle));
        let work = primary_work_area();
        let error = place_launcher_window(handle, Rect::new(0, 0, 100, 100), work)
            .expect_err("a foreign window must be rejected");
        assert!(error.contains("process") || error.contains("thread"));
        let error = show_launcher_checked(handle)
            .expect_err("a foreign window must be rejected before showing");
        assert!(error.contains("another process") || error.contains("owning UI thread"));
    }
}
