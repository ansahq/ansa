//! Cross-process single-instance lock for a clipboard store directory.
//!
//! Windows uses `CreateFileW` with a zero share mode on `<root>/.lock`. The OS
//! releases the handle when the process exits, including on a crash, and a
//! second open receives `ERROR_SHARING_VIOLATION` (or `ERROR_LOCK_VIOLATION`)
//! which is surfaced as [`ClipboardError::AlreadyLocked`]. The lock file is
//! intentionally left in place: deleting it would allow a third process to
//! create a new file while a second process still holds the old one.
//!
//! Linux and macOS use an advisory `flock` on `<root>/.lock`. The kernel
//! releases the lock when the process exits or crashes, so a stale file does
//! not block a later open. The file is intentionally left in place: unlinking a
//! live lock file would let two processes lock different inodes and both
//! proceed. Linux uses `rustix`; macOS uses a minimal `extern "C"` declaration
//! so the clipboard crate does not gain a new dependency.
//!
//! Other Unix builds fall back to an exclusive-create lock file and remove it
//! on drop. That fallback cannot distinguish a stale file left by a crash from
//! a live lock, so those targets may require deleting a stale `.lock` manually.
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
        #[cfg(target_os = "linux")]
        {
            Self::acquire_linux_flock(path)
        }
        #[cfg(target_os = "macos")]
        {
            Self::acquire_macos_flock(path)
        }
        #[cfg(all(not(windows), not(target_os = "linux"), not(target_os = "macos")))]
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

    #[cfg(target_os = "linux")]
    fn acquire_linux_flock(path: PathBuf) -> Result<Self, ClipboardError> {
        use rustix::io::Errno;

        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(Self { path, _file: file }),
            Err(Errno::WOULDBLOCK) => Err(ClipboardError::AlreadyLocked),
            Err(error) => Err(ClipboardError::Io(error.into())),
        }
    }

    #[cfg(target_os = "macos")]
    fn acquire_macos_flock(path: PathBuf) -> Result<Self, ClipboardError> {
        use std::os::fd::AsRawFd;

        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        // SAFETY: `file` owns a valid descriptor for this call. The operation
        // is a bitmask of platform constants, and `flock` has no pointer
        // arguments or caller-owned memory.
        let result = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
        if result == 0 {
            return Ok(Self { path, _file: file });
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            Err(ClipboardError::AlreadyLocked)
        } else {
            Err(ClipboardError::Io(error))
        }
    }

    #[cfg(all(not(windows), not(target_os = "linux"), not(target_os = "macos")))]
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

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl Drop for StoreLock {
    fn drop(&mut self) {
        // Closing the file releases the kernel `flock`. Keep the lock file in
        // place so another process cannot lock a different inode while a
        // concurrent open is still using the old one.
    }
}

#[cfg(all(not(windows), not(target_os = "linux"), not(target_os = "macos")))]
impl Drop for StoreLock {
    fn drop(&mut self) {
        // Best-effort release for the fallback. The open file keeps the name
        // present until this point.
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn linux_flock_is_exclusive_and_recoverable_after_release() {
        let directory = tempfile::tempdir().expect("tempdir");
        let first = StoreLock::acquire(directory.path()).expect("first lock");
        assert!(matches!(
            StoreLock::acquire(directory.path()),
            Err(ClipboardError::AlreadyLocked)
        ));
        drop(first);

        // A stale lock *file* must not look like a held lock after a crash or
        // normal release.
        let second = StoreLock::acquire(directory.path()).expect("reacquire");
        drop(second);
        assert!(directory.path().join(LOCK_FILE).exists());
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::*;

    #[test]
    #[ignore = "requires a real macOS host and file-locking support"]
    fn macos_flock_is_exclusive_and_recoverable_after_release() {
        let directory = tempfile::tempdir().expect("tempdir");
        let first = StoreLock::acquire(directory.path()).expect("first lock");
        assert!(matches!(
            StoreLock::acquire(directory.path()),
            Err(ClipboardError::AlreadyLocked)
        ));
        drop(first);

        let second = StoreLock::acquire(directory.path()).expect("reacquire");
        drop(second);
        assert!(directory.path().join(LOCK_FILE).exists());
    }
}

#[cfg(target_os = "macos")]
const LOCK_EX: std::ffi::c_int = 2;
#[cfg(target_os = "macos")]
const LOCK_NB: std::ffi::c_int = 4;

#[cfg(target_os = "macos")]
#[link(name = "System")]
unsafe extern "C" {
    fn flock(fd: std::ffi::c_int, operation: std::ffi::c_int) -> std::ffi::c_int;
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
