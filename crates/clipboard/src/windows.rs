//! Checked Windows clipboard reader.
//!
//! This module reads only:
//!
//! * the clipboard sequence number (`GetClipboardSequenceNumber`);
//! * `CF_UNICODETEXT`;
//! * `CF_HDROP` through `DragQueryFileW`;
//! * the producer exclusion markers `CanIncludeInClipboardHistory`,
//!   `CanUploadToCloudClipboard`, `Clipboard Viewer Ignore` and
//!   `ExcludeClipboardContentFromMonitorProcessing`;
//! * the best-effort clipboard owner process image name for attribution.
//!
//! It never writes, clears or restores the clipboard, never injects input and
//! never calls `EmptyClipboard`. `OpenClipboard` is retried a bounded number of
//! times and a scoped guard calls `CloseClipboard` on every path, including
//! errors and panics.
//!
//! Images are reported as [`crate::filter::SnapshotContent::Image`] with a
//! reason; image capture is not implemented.
//!
//! ## Bounds
//!
//! Text is limited to [`MAX_TEXT_BYTES`], file lists to [`MAX_FILES`] paths and
//! [`MAX_HDROP_UNITS`] aggregate UTF-16 units, with [`MAX_PATH_UNITS`] per path.
//! If a bound is reached the snapshot is marked `truncated` and the filter
//! rejects it, because a partial secret scan is not acceptable.
//!
//! ## Attribution
//!
//! The owner window's process is a hint, not proof of who copied the content.
//! The adapter reports [`SourceConfidence::Probable`] when it can read the
//! process image stem and never invents a name. When attribution fails the
//! field is `None` and confidence is `Unknown`.
//!
//! ## Marker policy
//!
//! `CanIncludeInClipboardHistory` and `CanUploadToCloudClipboard` accept only
//! the documented DWORD value `1` as permission; `0`, any other value and an
//! unreadable DWORD are treated as an opt-out. `Clipboard Viewer Ignore` and
//! `ExcludeClipboardContentFromMonitorProcessing` are presence markers and
//! exclude the whole capture.
//!
//! ## Manual probe
//!
//! Automated tests never read the real clipboard. For a manual adapter probe,
//! copy content from a password manager that sets
//! `ExcludeClipboardContentFromMonitorProcessing`, then run:
//!
//! ```text
//! cargo test -p ansa-clipboard --offline -- --ignored real_clipboard_read_does_not_modify_it --nocapture
//! ```
//!
//! The test prints the safe `Debug` snapshot, which shows marker booleans
//! without clipboard text. Run it only on a test workstation, never on a
//! desktop whose clipboard contains sensitive content.

use crate::error::ClipboardError;
use crate::filter::ClipboardSnapshot;

/// Maximum UTF-16 text bytes read from one snapshot (512 KiB).
pub const MAX_TEXT_BYTES: usize = 512 * 1024;

/// Maximum number of file paths read from one file list.
pub const MAX_FILES: u32 = 1024;

/// Maximum aggregate UTF-16 units across all paths in one file list (1 MiB).
pub const MAX_HDROP_UNITS: u32 = 512 * 1024;

/// Maximum UTF-16 units read for one file path.
pub const MAX_PATH_UNITS: u32 = 32_768;

/// Maximum number of `OpenClipboard` attempts.
pub const MAX_OPEN_ATTEMPTS: u32 = 12;

/// Delay between `OpenClipboard` attempts.
pub const OPEN_RETRY_DELAY_MS: u64 = 10;

/// Read a clipboard snapshot.
///
/// On non-Windows targets this returns [`ClipboardError::UnsupportedPlatform`].
/// Callers must still run the snapshot through
/// [`crate::filter::evaluate`] before persisting anything.
#[cfg(windows)]
pub fn read_clipboard_snapshot() -> Result<ClipboardSnapshot, ClipboardError> {
    imp::read()
}

/// Non-Windows stub: no clipboard adapter is implemented.
#[cfg(not(windows))]
pub fn read_clipboard_snapshot() -> Result<ClipboardSnapshot, ClipboardError> {
    Err(ClipboardError::UnsupportedPlatform {
        platform: std::env::consts::OS,
    })
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BooleanMarkerDecision {
    /// The marker format is absent.
    Absent,
    /// The marker is present with the documented allowed value (`1`).
    Allowed,
    /// The marker is present with any other value, or its DWORD is unreadable.
    Excluded,
}

/// Interpret a boolean clipboard marker.
///
/// Only the documented DWORD value `1` allows capture. `0`, any nonstandard
/// value and an unreadable DWORD all fail closed, so a malformed producer
/// opt-out is not silently treated as permission.
#[cfg(any(windows, test))]
fn boolean_marker_decision(present: bool, value: Option<u32>) -> BooleanMarkerDecision {
    if !present {
        return BooleanMarkerDecision::Absent;
    }
    if value == Some(1) {
        BooleanMarkerDecision::Allowed
    } else {
        BooleanMarkerDecision::Excluded
    }
}

/// Whether one more path fits in the aggregate file-list memory budget.
#[cfg(any(windows, test))]
fn hdrop_budget_allows(consumed_units: usize, requested_units: u32) -> bool {
    consumed_units.saturating_add(requested_units as usize) <= MAX_HDROP_UNITS as usize
}

#[cfg(any(windows, test))]
fn utf16_to_string(units: &[u16]) -> Result<String, ClipboardError> {
    String::from_utf16(units).map_err(|_| ClipboardError::MalformedPayload {
        reason: "clipboard text was not valid UTF-16",
    })
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::path::Path;
    use std::time::Duration;

    use super::{
        BooleanMarkerDecision, MAX_FILES, MAX_OPEN_ATTEMPTS, MAX_PATH_UNITS, MAX_TEXT_BYTES,
        OPEN_RETRY_DELAY_MS, boolean_marker_decision, hdrop_budget_allows, utf16_to_string,
    };
    use crate::content::wipe_utf16;
    use crate::error::ClipboardError;
    use crate::filter::{ClipboardSnapshot, ExclusionMarkers, SnapshotContent};
    use crate::model::{SourceApp, SourceConfidence};

    const CF_DIB: u32 = 8;
    const CF_UNICODETEXT: u32 = 13;
    const CF_HDROP: u32 = 15;
    const CF_DIBV5: u32 = 17;
    const MAX_TEXT_UNITS: usize = MAX_TEXT_BYTES / 2;
    const MAX_ATTRIBUTION_PATH_UNITS: u32 = 32_768;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const DRAG_QUERY_FILE_COUNT: u32 = u32::MAX;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn OpenClipboard(owner: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn GetClipboardSequenceNumber() -> u32;
        fn IsClipboardFormatAvailable(format: u32) -> i32;
        fn RegisterClipboardFormatW(name: *const u16) -> u32;
        fn GetClipboardData(format: u32) -> *mut c_void;
        fn GetClipboardOwner() -> *mut c_void;
        fn GetWindowThreadProcessId(window: *mut c_void, process_id: *mut u32) -> u32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalLock(handle: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(handle: *mut c_void) -> i32;
        fn GlobalSize(handle: *mut c_void) -> usize;
        fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> *mut c_void;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn QueryFullProcessImageNameW(
            process: *mut c_void,
            flags: u32,
            name: *mut u16,
            size: *mut u32,
        ) -> i32;
        fn GetLastError() -> u32;
    }

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn DragQueryFileW(hdrop: *mut c_void, file: u32, name: *mut u16, cch: u32) -> u32;
    }

    /// Scoped `CloseClipboard` guard.
    struct ClipboardGuard;

    impl ClipboardGuard {
        fn open() -> Result<Self, ClipboardError> {
            for attempt in 0..MAX_OPEN_ATTEMPTS {
                // SAFETY: a null owner asks for a read-only open of the shared
                // clipboard. No pointer is dereferenced.
                if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
                    return Ok(Self);
                }
                if attempt + 1 < MAX_OPEN_ATTEMPTS {
                    std::thread::sleep(Duration::from_millis(OPEN_RETRY_DELAY_MS));
                }
            }
            Err(ClipboardError::ClipboardBusy {
                attempts: MAX_OPEN_ATTEMPTS,
            })
        }
    }

    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            // SAFETY: this guard exists only after a successful OpenClipboard,
            // and CloseClipboard is called exactly once per successful open.
            // The return value is ignored because there is no recovery path at
            // drop time.
            let _ = unsafe { CloseClipboard() };
        }
    }

    pub(super) fn read() -> Result<ClipboardSnapshot, ClipboardError> {
        let guard = ClipboardGuard::open()?;

        // SAFETY: GetClipboardSequenceNumber takes no arguments and only reads
        // a process-wide counter.
        let sequence = u64::from(unsafe { GetClipboardSequenceNumber() });

        let markers = ExclusionMarkers {
            can_include_in_history: boolean_marker("CanIncludeInClipboardHistory"),
            can_upload_to_cloud: boolean_marker("CanUploadToCloudClipboard"),
            viewer_ignore: marker_is_present("Clipboard Viewer Ignore"),
            exclude_from_monitor_processing: marker_is_present(
                "ExcludeClipboardContentFromMonitorProcessing",
            ),
        };
        let attribution = attribution();
        let (content, mime_type, truncated) = if let Some((paths, truncated)) = read_file_list()? {
            (
                SnapshotContent::FileList(paths),
                Some("application/x-windows-file-list"),
                truncated,
            )
        } else if let Some((text, truncated)) = read_text()? {
            (SnapshotContent::Text(text), Some("text/plain"), truncated)
        } else if image_is_available() {
            (
                SnapshotContent::Image {
                    reason: "image capture is not implemented in this build".to_owned(),
                },
                Some("image/bmp"),
                false,
            )
        } else {
            return Err(ClipboardError::NoSupportedFormat);
        };

        // Close the clipboard before constructing the result so no handle
        // escapes this function.
        drop(guard);

        let mut snapshot = ClipboardSnapshot {
            sequence,
            content,
            markers,
            source: None,
            confidence: SourceConfidence::Unknown,
            mime_type: mime_type.map(str::to_owned),
            truncated,
        };
        if let Some((source, confidence)) = attribution {
            snapshot = snapshot.with_source(source, confidence);
        }
        Ok(snapshot)
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn marker_is_present(name: &str) -> bool {
        let wide = wide(name);
        // SAFETY: the UTF-16 buffer is NUL-terminated and lives for the call.
        let format = unsafe { RegisterClipboardFormatW(wide.as_ptr()) };
        if format == 0 {
            return false;
        }
        // SAFETY: a registered format id is a plain scalar argument.
        (unsafe { IsClipboardFormatAvailable(format) }) != 0
    }

    fn boolean_marker(name: &str) -> Option<bool> {
        let wide = wide(name);
        // SAFETY: the UTF-16 buffer is NUL-terminated and lives for the call.
        let format = unsafe { RegisterClipboardFormatW(wide.as_ptr()) };
        if format == 0 {
            return None;
        }
        // SAFETY: a registered format id is a plain scalar argument.
        let present = (unsafe { IsClipboardFormatAvailable(format) }) != 0;
        let value = if present { read_dword(format) } else { None };
        match boolean_marker_decision(present, value) {
            BooleanMarkerDecision::Absent => None,
            BooleanMarkerDecision::Allowed => Some(true),
            BooleanMarkerDecision::Excluded => Some(false),
        }
    }

    fn read_dword(format: u32) -> Option<u32> {
        // SAFETY: `format` is a clipboard format id and the clipboard is open.
        // The returned handle is owned by the clipboard and is not freed here.
        let handle = unsafe { GetClipboardData(format) };
        if handle.is_null() {
            return None;
        }
        // SAFETY: the handle is a valid clipboard memory handle.
        let size = unsafe { GlobalSize(handle) };
        if size < 4 {
            return None;
        }
        // SAFETY: GlobalLock returns a readable pointer for a valid movable
        // clipboard handle, or null. The lock is released below.
        let pointer = unsafe { GlobalLock(handle) };
        if pointer.is_null() {
            return None;
        }
        // SAFETY: the pointer is readable for at least `size` bytes and
        // `read_unaligned` makes no alignment assumption.
        let value = unsafe { std::ptr::read_unaligned(pointer.cast::<u32>()) };
        // SAFETY: this pairs with the successful GlobalLock above. The return
        // value is ignored because GlobalUnlock reports zero both when the lock
        // count reaches zero and on failure; the lock was still released.
        let _ = unsafe { GlobalUnlock(handle) };
        Some(value)
    }

    fn read_text() -> Result<Option<(String, bool)>, ClipboardError> {
        // SAFETY: a clipboard format id is a plain scalar argument.
        if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) } == 0 {
            return Ok(None);
        }
        // SAFETY: the clipboard is open and the format is available.
        let handle = unsafe { GetClipboardData(CF_UNICODETEXT) };
        if handle.is_null() {
            return Err(native("GetClipboardData(CF_UNICODETEXT)"));
        }
        // SAFETY: the handle is a valid clipboard memory handle.
        let size = unsafe { GlobalSize(handle) };
        // SAFETY: GlobalLock returns a readable pointer, or null.
        let pointer = unsafe { GlobalLock(handle) };
        if pointer.is_null() {
            return Err(native("GlobalLock(text)"));
        }

        let capacity = (size / 2).min(MAX_TEXT_UNITS);
        let mut units = Vec::with_capacity(capacity.min(4096));
        let mut terminated = false;
        for index in 0..capacity {
            // SAFETY: the lock covers `size` bytes and index < size / 2.
            let unit = unsafe { std::ptr::read_unaligned(pointer.cast::<u16>().add(index)) };
            if unit == 0 {
                terminated = true;
                break;
            }
            units.push(unit);
        }
        // SAFETY: paired with the successful GlobalLock above; the return value
        // is ignored for the reason documented in `read_dword`.
        let _ = unsafe { GlobalUnlock(handle) };

        let converted = utf16_to_string(&units);
        wipe_utf16(&mut units);
        let text = converted?;
        Ok(Some((text, !terminated)))
    }

    fn read_file_list() -> Result<Option<(Vec<String>, bool)>, ClipboardError> {
        // SAFETY: a clipboard format id is a plain scalar argument.
        if unsafe { IsClipboardFormatAvailable(CF_HDROP) } == 0 {
            return Ok(None);
        }
        // SAFETY: the clipboard is open and CF_HDROP is available.
        let handle = unsafe { GetClipboardData(CF_HDROP) };
        if handle.is_null() {
            return Err(native("GetClipboardData(CF_HDROP)"));
        }

        // SAFETY: `handle` is an HDROP; the count query uses the documented
        // sentinel index and a null buffer.
        let count =
            unsafe { DragQueryFileW(handle, DRAG_QUERY_FILE_COUNT, std::ptr::null_mut(), 0) };
        if count == 0 {
            return Ok(Some((Vec::new(), false)));
        }
        let take = count.min(MAX_FILES);
        let mut truncated = count > take;
        let mut paths = Vec::with_capacity(take as usize);
        let mut consumed_units = 0usize;

        for index in 0..take {
            // SAFETY: `index` is below the reported count; a null buffer with
            // length 0 asks only for the character count.
            let length = unsafe { DragQueryFileW(handle, index, std::ptr::null_mut(), 0) };
            if length == 0 {
                continue;
            }
            if length > MAX_PATH_UNITS || !hdrop_budget_allows(consumed_units, length) {
                truncated = true;
                if length > MAX_PATH_UNITS {
                    continue;
                }
                break;
            }
            consumed_units = consumed_units.saturating_add(length as usize);
            let mut buffer = vec![0u16; length as usize + 1];
            // SAFETY: the buffer is writable for `length + 1` UTF-16 units and
            // the clipboard is still open.
            let written = unsafe {
                DragQueryFileW(handle, index, buffer.as_mut_ptr(), length.saturating_add(1))
            };
            if written == 0 {
                continue;
            }
            buffer.truncate(written as usize);
            let converted = utf16_to_string(&buffer);
            wipe_utf16(&mut buffer);
            paths.push(converted?);
        }

        Ok(Some((paths, truncated)))
    }

    fn image_is_available() -> bool {
        // SAFETY: clipboard format ids are plain scalar arguments.
        unsafe {
            IsClipboardFormatAvailable(CF_DIB) != 0 || IsClipboardFormatAvailable(CF_DIBV5) != 0
        }
    }

    fn attribution() -> Option<(SourceApp, SourceConfidence)> {
        // SAFETY: GetClipboardOwner takes no arguments; the clipboard is open.
        let owner = unsafe { GetClipboardOwner() };
        if owner.is_null() {
            return None;
        }
        let mut process_id = 0u32;
        // SAFETY: `owner` is a live window handle observed above and the
        // out-parameter points to a live local.
        unsafe { GetWindowThreadProcessId(owner, &mut process_id) };
        if process_id == 0 {
            return None;
        }

        // SAFETY: OpenProcess is called with query-only access and a plain pid.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
        if process.is_null() {
            return None;
        }

        let mut buffer = vec![0u16; MAX_ATTRIBUTION_PATH_UNITS as usize];
        let mut size = MAX_ATTRIBUTION_PATH_UNITS;
        // SAFETY: the buffer is writable for `size` UTF-16 units and `size`
        // points to a live local initialized to that capacity.
        let queried =
            unsafe { QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut size) } != 0;
        // SAFETY: the handle came from OpenProcess and is owned here.
        let _ = unsafe { CloseHandle(process) };
        if !queried || size == 0 {
            return None;
        }
        buffer.truncate(size as usize);

        let path = String::from_utf16_lossy(&buffer);
        let stem = Path::new(&path).file_stem()?.to_str()?;
        let app = SourceApp::new(stem)?;
        Some((app, SourceConfidence::Probable))
    }

    fn native(operation: &'static str) -> ClipboardError {
        // SAFETY: GetLastError takes no arguments and only reads thread-local
        // state.
        let os_code = unsafe { GetLastError() };
        ClipboardError::Native { operation, os_code }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_decoding_is_strict() {
        assert_eq!(
            utf16_to_string(&[b'h' as u16, b'i' as u16]).expect("valid"),
            "hi"
        );
        assert!(utf16_to_string(&[0xD800]).is_err());
    }

    #[test]
    fn bounds_are_finite_and_consistent() {
        const {
            assert!(MAX_TEXT_BYTES >= 64 * 1024);
            assert!(MAX_FILES >= 1);
            assert!(MAX_PATH_UNITS >= 260);
            assert!(MAX_OPEN_ATTEMPTS >= 1);
            assert!(OPEN_RETRY_DELAY_MS >= 1);
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_adapter_is_explicitly_unsupported() {
        assert!(matches!(
            read_clipboard_snapshot(),
            Err(ClipboardError::UnsupportedPlatform { .. })
        ));
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "reads the real clipboard; run manually on a test machine"]
    fn real_clipboard_read_does_not_modify_it() {
        // This is intentionally ignored: an automated test must not inspect a
        // developer's clipboard. The adapter only calls OpenClipboard for
        // reading and never EmptyClipboard/SetClipboardData. Run with
        // --ignored on a test workstation; Debug output contains marker
        // booleans and shape metadata, never clipboard text.
        match read_clipboard_snapshot() {
            Ok(snapshot) => println!("{snapshot:?}"),
            Err(error) => println!("clipboard read returned: {error}"),
        }
    }

    #[test]
    fn boolean_markers_fail_closed_except_for_the_documented_value() {
        assert_eq!(
            boolean_marker_decision(false, None),
            BooleanMarkerDecision::Absent
        );
        assert_eq!(
            boolean_marker_decision(true, Some(1)),
            BooleanMarkerDecision::Allowed
        );
        assert_eq!(
            boolean_marker_decision(true, Some(0)),
            BooleanMarkerDecision::Excluded
        );
        assert_eq!(
            boolean_marker_decision(true, Some(2)),
            BooleanMarkerDecision::Excluded
        );
        assert_eq!(
            boolean_marker_decision(true, Some(0x30)),
            BooleanMarkerDecision::Excluded
        );
        assert_eq!(
            boolean_marker_decision(true, None),
            BooleanMarkerDecision::Excluded
        );
    }

    #[test]
    fn hdrop_aggregate_budget_is_bounded() {
        assert!(hdrop_budget_allows(0, MAX_HDROP_UNITS));
        assert!(!hdrop_budget_allows(1, MAX_HDROP_UNITS));
        assert!(!hdrop_budget_allows(MAX_HDROP_UNITS as usize, 1));
        assert!(hdrop_budget_allows(100, 200));
    }
}
