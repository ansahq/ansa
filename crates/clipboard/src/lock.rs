//! Cross-process single-instance lock for a clipboard store directory.
//!
//! Windows uses `CreateFileW` with a zero share mode on `<root>/.lock`. The OS
//! releases the handle when the process exits, including on a crash, and a
//! second open receives `ERROR_SHARING_VIOLATION` (or `ERROR_LOCK_VIOLATION`)
//! which is surfaced as [`ClipboardError::AlreadyLocked`]. The lock file is
//! intentionally left in place: deleting it would allow a third process to
//! create a new file while a second process still holds the old one.
//!
//! Non-Windows builds fall back to an exclusive-create lock file and remove it
//! on drop. That fallback cannot distinguish a stale file left by a crash from a
//! live lock; Windows is the supported desktop target for this crate.
//!
//! The lock is non-blocking by design. A caller that wants to wait must retry
//! `ClipboardStore::open` rather than block the capture thread.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::error::ClipboardError;

const LOCK_FILE: &str = ".lock";

/// A held store lock. Dropping it releases the lock.
pub(crate) struct StoreLock {
    path: PathBuf,
    #[cfg(windows)]
    handle: *mut std::ffi::c_void,
    #[cfg(not(windows))]
    _file: std::fs::File,
}

impl fmt::Debug for StoreLock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoreLock")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl StoreLock {
    /// Acquire the lock for `root` without waiting.
    pub(crate) fn acquire(root: &Path) -> Result<Self, ClipboardError> {
        let path = root.join(LOCK_FILE);
        #[cfg(windows)]
        {
            Self::acquire_windows(path)
        }
        #[cfg(not(windows))]
        {
            Self::acquire_fallback(path)
        }
    }

    #[cfg(windows)]
    fn acquire_windows(path: PathBuf) -> Result<Self, ClipboardError> {
        use std::os::windows::ffi::OsStrExt;

        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: the path is a NUL-terminated UTF-16 buffer that lives for the
        // call; all other arguments are plain scalars or null pointers. Share
        // mode 0 requests exclusive access, which is the lock itself.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null_mut(),
                OPEN_ALWAYS,
                FILE_ATTRIBUTE_HIDDEN,
                std::ptr::null_mut(),
            )
        };
        if handle as isize == -1 {
            // SAFETY: GetLastError reads thread-local state and takes no
            // arguments.
            let os_code = unsafe { GetLastError() };
            return if matches!(os_code, ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION) {
                Err(ClipboardError::AlreadyLocked)
            } else {
                Err(ClipboardError::Native {
                    operation: "CreateFileW(store lock)",
                    os_code,
                })
            };
        }
        Ok(Self { path, handle })
    }

    #[cfg(not(windows))]
    fn acquire_fallback(path: PathBuf) -> Result<Self, ClipboardError> {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => Ok(Self { path, _file: file }),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(ClipboardError::AlreadyLocked)
            }
            Err(error) => Err(ClipboardError::Io(error)),
        }
    }
}

#[cfg(windows)]
impl Drop for StoreLock {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateFileW and is owned by this value.
        // Closing it releases the no-share lock; the lock file remains on disk.
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

#[cfg(not(windows))]
impl Drop for StoreLock {
    fn drop(&mut self) {
        // Best-effort release for the non-Windows fallback. The open file keeps
        // the name present until this point.
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(windows)]
const GENERIC_READ: u32 = 0x8000_0000;
#[cfg(windows)]
const GENERIC_WRITE: u32 = 0x4000_0000;
#[cfg(windows)]
const OPEN_ALWAYS: u32 = 4;
#[cfg(windows)]
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
#[cfg(windows)]
const ERROR_SHARING_VIOLATION: u32 = 32;
#[cfg(windows)]
const ERROR_LOCK_VIOLATION: u32 = 33;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileW(
        file_name: *const u16,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *mut std::ffi::c_void,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: *mut std::ffi::c_void,
    ) -> *mut std::ffi::c_void;
    fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    fn GetLastError() -> u32;
}
