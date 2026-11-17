//! Native show/hide for the launcher's own top-level window.
//!
//! GPUI 0.2.2's Windows `Platform::hide()` is a no-op, so the UI layer must
//! explicitly hide/show the HWND it owns. This module is that small, checked
//! native path. It does not enumerate or manipulate arbitrary windows, and it
//! does not restore a previous target; use [`crate::dismiss_launcher`] for the
//! focus-checked dismissal path.
//!
//! Thread affinity: the caller must be on the thread that owns the window (the
//! UI/main thread), not the hotkey worker. This is checked before any state
//! change. `ShowWindow`/`SetForegroundWindow` are never called from the hotkey
//! thread.

use std::ffi::c_void;

#[repr(C)]
#[derive(Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct BlurBehind {
    flags: u32,
    enable: i32,
    region: *mut c_void,
    transition: i32,
}

#[repr(C)]
struct Margins {
    left: i32,
    right: i32,
    top: i32,
    bottom: i32,
}

#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    color: u32,
    animation: u32,
}

#[repr(C)]
struct CompositionAttribute {
    attribute: u32,
    data: *mut c_void,
    size: usize,
}

const LAUNCHER_SUBCLASS: usize = 0x414e5341;

#[link(name = "comctl32")]
extern "system" {
    fn SetWindowSubclass(
        hwnd: *mut c_void,
        callback: unsafe extern "system" fn(*mut c_void, u32, usize, isize, usize, usize) -> isize,
        id: usize,
        reference: usize,
    ) -> i32;
    fn RemoveWindowSubclass(
        hwnd: *mut c_void,
        callback: unsafe extern "system" fn(*mut c_void, u32, usize, isize, usize, usize) -> isize,
        id: usize,
    ) -> i32;
    fn DefSubclassProc(hwnd: *mut c_void, message: u32, wparam: usize, lparam: isize) -> isize;
}

unsafe extern "system" fn launcher_frame_proc(
    hwnd: *mut c_void,
    message: u32,
    wparam: usize,
    lparam: isize,
    id: usize,
    _reference: usize,
) -> isize {
    // Old GPUI reserves resize insets even for a non-resizable popup. Accept
    // the entire proposed client rectangle, without modifying cached upstream.
    if message == 0x0083 {
        return 0;
    } // WM_NCCALCSIZE
    if message == 0x0082 {
        // WM_NCDESTROY
        // SAFETY: remove this exact subclass from its owning window/thread.
        if unsafe { RemoveWindowSubclass(hwnd, launcher_frame_proc, id) } == 0 {
            eprintln!("Cannot remove launcher frame subclass during window destruction");
        }
    }
    // SAFETY: all other messages retain the framework's existing behavior.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

const SW_HIDE: i32 = 0;
const SW_SHOW: i32 = 5;

#[link(name = "user32")]
extern "system" {
    fn IsWindow(hwnd: *mut c_void) -> i32;
    fn ShowWindow(hwnd: *mut c_void, command: i32) -> i32;
    fn SetForegroundWindow(hwnd: *mut c_void) -> i32;
    fn GetForegroundWindow() -> *mut c_void;
    fn GetWindowThreadProcessId(hwnd: *mut c_void, process_id: *mut u32) -> u32;
    fn GetWindowLongW(hwnd: *mut c_void, index: i32) -> i32;
    fn SetWindowLongW(hwnd: *mut c_void, index: i32, value: i32) -> i32;
    fn GetWindowRect(hwnd: *mut c_void, rect: *mut Rect) -> i32;
    fn SetWindowRgn(hwnd: *mut c_void, region: *mut c_void, redraw: i32) -> i32;
    fn SetWindowPos(
        hwnd: *mut c_void,
        after: *mut c_void,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateRectRgn(left: i32, top: i32, right: i32, bottom: i32) -> *mut c_void;
    fn CreateRoundRectRgn(
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
        width: i32,
        height: i32,
    ) -> *mut c_void;
    fn DeleteObject(object: *mut c_void) -> i32;
}

#[link(name = "dwmapi")]
extern "system" {
    fn DwmEnableBlurBehindWindow(hwnd: *mut c_void, blur: *const BlurBehind) -> i32;
    fn DwmExtendFrameIntoClientArea(hwnd: *mut c_void, margins: *const Margins) -> i32;
}

fn owned_window(hwnd: isize) -> Result<*mut c_void, String> {
    let window = hwnd as *mut c_void;
    // SAFETY: inspect only until both liveness and thread ownership are known.
    if hwnd == 0 || unsafe { IsWindow(window) } == 0 {
        return Err("cannot style launcher: invalid window handle".into());
    }
    // SAFETY: live HWND; process-id output is optional.
    if unsafe { GetWindowThreadProcessId(window, std::ptr::null_mut()) != GetCurrentThreadId() } {
        return Err("cannot style launcher outside its owning UI thread".into());
    }
    Ok(window)
}

/// Enable per-pixel DirectComposition alpha without a native accent backdrop.
/// GPUI's Transparent appearance installs ACCENT_ENABLE_TRANSPARENTGRADIENT,
/// which can put a black rectangle behind the otherwise translucent scene.
pub fn enable_launcher_alpha(hwnd: isize) -> Result<(), String> {
    let window = owned_window(hwnd)?;
    // SAFETY: all calls operate on this UI thread's validated window. The
    // optional composition export is resolved before conversion to a function.
    unsafe {
        if GetWindowLongW(window, -20) as u32 & 0x00200000 == 0 {
            return Err(
                "Glass needs GPUI DirectComposition; it is disabled in this process".into(),
            );
        }
        let module_name: Vec<u16> = "user32.dll\0".encode_utf16().collect();
        let module = GetModuleHandleW(module_name.as_ptr());
        if module.is_null() {
            return Err("Cannot locate the Windows composition module".into());
        }
        let address = GetProcAddress(module, b"SetWindowCompositionAttribute\0".as_ptr());
        if address.is_null() {
            return Err("Windows composition attributes are unavailable".into());
        }
        let set_attribute: unsafe extern "system" fn(
            *mut c_void,
            *mut CompositionAttribute,
        ) -> i32 = std::mem::transmute(address);
        let mut accent = AccentPolicy {
            state: 0,
            flags: 0,
            color: 0,
            animation: 0,
        };
        let mut attribute = CompositionAttribute {
            attribute: 19,
            data: (&mut accent as *mut AccentPolicy).cast(),
            size: std::mem::size_of::<AccentPolicy>(),
        };
        if set_attribute(window, &mut attribute) == 0 {
            return Err("Windows could not remove the opaque accent backdrop".into());
        }
        // An off-client blur region enables client alpha, not acrylic blur or a
        // backdrop outside the window. The DWM call does not take ownership.
        let region = CreateRectRgn(0, 0, -1, -1);
        if region.is_null() {
            return Err("Cannot allocate the compositor region".into());
        }
        let result = DwmEnableBlurBehindWindow(
            window,
            &BlurBehind {
                flags: 3,
                enable: 1,
                region,
                transition: 0,
            },
        );
        if DeleteObject(region) == 0 {
            return Err("Cannot release the compositor region".into());
        }
        if result < 0 {
            return Err(format!(
                "Windows could not enable client alpha: {result:#x}"
            ));
        }
        // The compositor, not a padded GPUI surface, owns any available shadow.
        let result = DwmExtendFrameIntoClientArea(
            window,
            &Margins {
                left: 1,
                right: 1,
                top: 1,
                bottom: 1,
            },
        );
        if result < 0 {
            return Err(format!(
                "Windows could not configure the native shadow: {result:#x}"
            ));
        }
    }
    Ok(())
}

/// Clip every child and the native backdrop to the same rounded window shape.
/// Radius is in physical pixels; callers reapply after a size or DPI change.
pub fn round_launcher_window(hwnd: isize, radius: i32) -> Result<(), String> {
    let window = owned_window(hwnd)?;
    if radius <= 0 || radius > 256 {
        return Err("Invalid launcher corner radius".into());
    }
    // SAFETY: validated own-thread window and checked dimensions; Windows owns
    // the region only after a successful SetWindowRgn call.
    unsafe {
        let mut bounds = Rect::default();
        if GetWindowRect(window, &mut bounds) == 0 {
            return Err("Cannot read launcher bounds".into());
        }
        let width = bounds.right - bounds.left;
        let height = bounds.bottom - bounds.top;
        if width <= 0 || height <= 0 {
            return Err("Launcher has no drawable bounds".into());
        }
        let diameter = (radius * 2).min(width).min(height);
        let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, diameter, diameter);
        if region.is_null() {
            return Err("Cannot allocate rounded launcher region".into());
        }
        if SetWindowRgn(window, region, 1) == 0 {
            if DeleteObject(region) == 0 {
                eprintln!("Cannot release the rejected launcher region");
            }
            return Err("Windows could not apply rounded launcher corners".into());
        }
    }
    Ok(())
}

/// Remove native chrome from a live window owned by the calling UI thread.
/// Windows supplies a default caption when a framework creates a zero-style
/// popup. Set WS_POPUP explicitly and refresh the non-client frame in place.
pub fn make_launcher_frameless(hwnd: isize) -> Result<(), String> {
    let window = owned_window(hwnd)?;
    const CHROME: u32 = 0x00cf0000;
    // SAFETY: live window on its owning thread; GWL_STYLE is a 32-bit value.
    unsafe {
        if SetWindowSubclass(window, launcher_frame_proc, LAUNCHER_SUBCLASS, 0) == 0 {
            return Err("Windows could not install the launcher's client-frame adapter".into());
        }
        let style = GetWindowLongW(window, -16) as u32;
        SetWindowLongW(window, -16, ((style & !CHROME) | 0x80000000) as i32);
        if GetWindowLongW(window, -16) as u32 & CHROME != 0 {
            return Err("Windows did not remove the launcher frame".into());
        }
        // FRAMECHANGED | NOMOVE | NOSIZE | NOZORDER | NOACTIVATE
        if SetWindowPos(window, std::ptr::null_mut(), 0, 0, 0, 0, 0x37) == 0 {
            return Err("Windows could not refresh the launcher frame".into());
        }
    }
    Ok(())
}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentThreadId() -> u32;
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

/// Shows or hides the launcher's own window.
///
/// `hwnd` is the raw window handle obtained through `raw-window-handle` and
/// must be called on that window's owning UI thread.
///
/// * Hiding uses `ShowWindow(SW_HIDE)`. This helper never restores a previous
///   target; [`crate::dismiss_launcher`] performs the checked foreground
///   capture, hide and restore sequence.
/// * Showing uses `ShowWindow(SW_SHOW)` followed by `SetForegroundWindow` and
///   verifies the resulting foreground window. If Windows denies activation,
///   this returns an error that explicitly says the window is visible but
///   focus was not granted. An `Err` from the show path therefore does not mean
///   the window remained hidden.
///
/// A stale or foreign HWND is rejected before any visibility change, as is a
/// call from a thread other than the window owner.
pub fn set_launcher_visible(hwnd: isize, visible: bool) -> Result<(), String> {
    if hwnd == 0 {
        return Err("cannot change launcher visibility: null window handle".to_owned());
    }

    let window = hwnd as *mut c_void;

    // SAFETY: an arbitrary integer handle is only inspected with IsWindow,
    // which is the supported way to validate an HWND.
    if unsafe { IsWindow(window) } == 0 {
        return Err(
            "cannot change launcher visibility: window handle is not a live window".to_owned(),
        );
    }

    // SAFETY: IsWindow succeeded, so querying the owning thread is safe. The
    // process-id out-parameter is optional and intentionally null.
    let owner_thread = unsafe { GetWindowThreadProcessId(window, std::ptr::null_mut()) };
    // SAFETY: returns the calling thread's id.
    let current_thread = unsafe { GetCurrentThreadId() };
    if owner_thread != current_thread {
        return Err(format!(
            "set_launcher_visible must run on the window's owning UI thread \
             (owner thread {owner_thread}, current thread {current_thread})"
        ));
    }

    if !visible {
        // SAFETY: the handle is live and owned by this thread. ShowWindow's
        // return value is the previous visibility state, not an error code.
        unsafe {
            ShowWindow(window, SW_HIDE);
        }
        return Ok(());
    }

    // SAFETY: the handle is live and owned by this thread.
    unsafe {
        ShowWindow(window, SW_SHOW);
    }
    // SAFETY: same live handle; SetForegroundWindow is subject to Windows
    // foreground-lock policy and may be denied.
    let activation_requested = unsafe { SetForegroundWindow(window) } != 0;
    // SAFETY: returns the current foreground window, or null.
    let foreground = unsafe { GetForegroundWindow() };

    if !activation_requested || foreground != window {
        return Err(
            "launcher window is visible, but Windows denied foreground activation; \
             focus was not restored"
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
        fn GetClientRect(hwnd: *mut c_void, bounds: *mut Rect) -> i32;
        fn GetWindowRgn(hwnd: *mut c_void, region: *mut c_void) -> i32;
    }
    #[link(name = "gdi32")]
    extern "system" {
        fn PtInRegion(region: *mut c_void, x: i32, y: i32) -> i32;
    }

    struct TestWindow(*mut c_void);
    impl TestWindow {
        fn new() -> Self {
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            // SAFETY: built-in class, valid string, no parent and no visibility
            // style; this fixture never appears on the person's desktop.
            let window = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    class.as_ptr(),
                    0x80000000,
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
            assert!(!window.is_null());
            Self(window)
        }
    }
    impl Drop for TestWindow {
        fn drop(&mut self) {
            // SAFETY: this test owns the live hidden window on this thread.
            if unsafe { DestroyWindow(self.0) } == 0 {
                eprintln!("Cannot destroy appearance test window");
            }
        }
    }

    #[test]
    fn native_shape_has_no_square_corners_or_layout_gutter() {
        let window = TestWindow::new();
        assert!(make_launcher_frameless(window.0 as isize).is_ok());
        assert!(round_launcher_window(window.0 as isize, 12).is_ok());
        let mut bounds = Rect::default();
        let mut client = Rect::default();
        // SAFETY: read only the owned fixture and a newly allocated GDI region.
        unsafe {
            assert_ne!(GetWindowRect(window.0, &mut bounds), 0);
            assert_ne!(GetClientRect(window.0, &mut client), 0);
            assert_eq!(bounds.right - bounds.left, client.right);
            assert_eq!(bounds.bottom - bounds.top, client.bottom);
            let region = CreateRectRgn(0, 0, 0, 0);
            assert!(!region.is_null());
            let kind = GetWindowRgn(window.0, region);
            let samples = [
                (0, 0),
                (199, 0),
                (0, 119),
                (199, 119),
                (100, 0),
                (0, 60),
                (199, 60),
                (100, 119),
            ]
            .map(|(x, y)| PtInRegion(region, x, y) != 0);
            assert_ne!(DeleteObject(region), 0);
            assert_eq!(kind, 3);
            assert_eq!(
                samples,
                [false, false, false, false, true, true, true, true]
            );
        }
        assert!(round_launcher_window(window.0 as isize, 0).is_err());
        assert!(round_launcher_window(window.0 as isize, 257).is_err());
    }

    #[test]
    fn alpha_reports_disabled_composition_and_rejects_wrong_thread() {
        let window = TestWindow::new();
        let error = enable_launcher_alpha(window.0 as isize);
        assert!(error.is_err_and(|error| error.contains("DirectComposition")));
        let handle = window.0 as isize;
        let outcome = std::thread::spawn(move || {
            (
                make_launcher_frameless(handle).is_err(),
                round_launcher_window(handle, 12).is_err(),
                enable_launcher_alpha(handle).is_err(),
            )
        })
        .join();
        assert!(matches!(outcome, Ok((true, true, true))));
    }

    #[test]
    fn rejects_null_window_handle() {
        assert!(make_launcher_frameless(0).is_err());
        assert!(make_launcher_frameless(1).is_err());
        assert!(enable_launcher_alpha(0).is_err());
        assert!(enable_launcher_alpha(1).is_err());
        assert!(round_launcher_window(0, 12).is_err());
        assert!(round_launcher_window(1, 12).is_err());
        assert!(set_launcher_visible(0, true).is_err());
        assert!(set_launcher_visible(0, false).is_err());
    }

    #[test]
    fn rejects_stale_or_foreign_handle() {
        assert!(set_launcher_visible(1, false).is_err());
    }
}
