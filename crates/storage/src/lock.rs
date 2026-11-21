//! Single-instance guard for a durable data directory.
//!
//! Opening a [`crate::Store`] takes an exclusive advisory lock keyed to the
//! *identity* of the durable database file, not to its path string. On Windows
//! the identity is the volume serial number plus file index returned by
//! `GetFileInformationByHandle` (through `winapi-util`); on Unix it is the
//! device plus inode. Hard links, symlinks and junctions to the same file
//! therefore resolve to the same lock file in a fixed per-user lock directory,
//! and a second [`crate::Store`] fails with
//! [`crate::StorageError::AlreadyLocked`] before SQLite opens a second writer
//! or WAL.
//!
//! The OS owns the lock, so a crashed process releases it when its handles are
//! closed. The lock is advisory: a raw SQLite client that does not use this
//! crate is not stopped by it.
//!
//! Windows and Unix both block reads of an exclusively locked file, so the
//! holder PID is written to a separate `<key>.lock.owner` note that a refused
//! opener can read for diagnostics. The note is best-effort (there is a short
//! window after lock acquisition and after release) and is stale after a
//! crash; the next successful open replaces it.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::StorageError;
use crate::paths;

/// Suffix appended to the identity key for the lock itself.
const LOCK_SUFFIX: &str = ".lock";
/// Suffix for the unlocked diagnostic file naming the lock holder.
const OWNER_SUFFIX: &str = ".lock.owner";

/// Lock and owner-note paths for one durable-file identity.
#[derive(Debug, Clone)]
pub(crate) struct LockPaths {
    pub(crate) lock: PathBuf,
    pub(crate) owner: PathBuf,
}

/// An exclusive, advisory OS file lock.
pub(crate) struct InstanceLock {
    file: File,
    path: PathBuf,
    owner_path: PathBuf,
    nonce: String,
}

impl InstanceLock {
    /// Acquire the identity lock without blocking.
    ///
    /// The durable file is opened (and created if absent) only to read its
    /// identity; SQLite is not opened here, so a refused second store never
    /// creates a second WAL. `File::try_lock` returns immediately when another
    /// handle owns the lock.
    pub(crate) fn acquire(main_path: &Path) -> Result<Self, StorageError> {
        Self::acquire_inner(main_path, true)
    }

    /// Acquire the identity lock for a file that must already exist.
    ///
    /// Used for staged/rollback copies so a missing file fails instead of
    /// being silently created empty.
    pub(crate) fn acquire_existing(path: &Path) -> Result<Self, StorageError> {
        Self::acquire_inner(path, false)
    }

    fn acquire_inner(main_path: &Path, create: bool) -> Result<Self, StorageError> {
        if let Some(parent) = main_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let database = OpenOptions::new()
            .create(create)
            .read(true)
            .write(true)
            .truncate(false)
            .open(main_path)?;
        let key = identity_key(main_path, &database);
        drop(database);

        let paths = lock_paths(&key);
        if let Some(directory) = paths.lock.parent() {
            std::fs::create_dir_all(directory)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&paths.lock)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(StorageError::AlreadyLocked {
                    holder_pid: holder_pid(&paths.owner),
                    path: paths.lock,
                });
            }
            Err(TryLockError::Error(error)) => return Err(StorageError::Io(error)),
        }

        // Publish the note only after we own the lock, so losing an attempt can
        // never clobber a live holder's note. The note is staged and renamed so
        // a reader never sees a half-written PID.
        let nonce = note_nonce();
        let staged_note = paths.owner.with_file_name(format!(
            "{}.{nonce}.tmp",
            paths
                .owner
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("owner")
        ));
        write_owner_note(&staged_note, &nonce);
        publish_owner_note(&staged_note, &paths.owner);

        Ok(Self {
            file,
            path: paths.lock,
            owner_path: paths.owner,
            nonce,
        })
    }

    /// Path of the lock file.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // Remove our note while we still hold the lock, so a successor cannot
        // have written its own note yet. The nonce check is defence in depth.
        remove_owner_note(&self.owner_path, &self.nonce);
        // Closing the handle would release the lock as well; unlock first so
        // the release is explicit and immediate.
        let _ = self.file.unlock();
    }
}

/// Lock paths for an existing durable database, when its identity is readable.
///
/// Used by the backup/restore guards, which run while the store (and therefore
/// the file) exists.
pub(crate) fn paths_for_existing(main_path: &Path) -> Option<LockPaths> {
    let file = File::open(main_path).ok()?;
    Some(lock_paths(&identity_key(main_path, &file)))
}

/// True when both paths refer to the same file identity.
pub(crate) fn same_identity(left: &Path, right: &Path) -> bool {
    let (Ok(left_file), Ok(right_file)) = (File::open(left), File::open(right)) else {
        return false;
    };
    identity_key(left, &left_file) == identity_key(right, &right_file)
}

fn lock_paths(key: &str) -> LockPaths {
    let directory = lock_dir();
    LockPaths {
        lock: directory.join(format!("{key}{LOCK_SUFFIX}")),
        owner: directory.join(format!("{key}{OWNER_SUFFIX}")),
    }
}

/// A fixed, per-user lock directory outside the data directory.
///
/// Keeping it outside the data directory is what makes hard-link aliases
/// (which live in different directories) agree on one lock. Windows uses the
/// per-user local application data directory; Unix prefers the per-user
/// runtime directory, then a per-home name under the temp directory.
#[cfg(windows)]
fn lock_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("ansa-storage")
        .join("locks")
}

#[cfg(unix)]
fn lock_dir() -> PathBuf {
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime).join("ansa-storage-locks");
    }
    let user = std::env::var_os("HOME")
        .map(|home| fnv1a(home.to_string_lossy().as_bytes()))
        .unwrap_or(0);
    std::env::temp_dir().join(format!("ansa-storage-locks-{user:016x}"))
}

#[cfg(not(any(windows, unix)))]
fn lock_dir() -> PathBuf {
    std::env::temp_dir().join("ansa-storage-locks")
}

/// Stable identity key for the open file.
///
/// On Windows this asks `GetFileInformationByHandle` for `dwVolumeSerialNumber`
/// and the 64-bit file index; on Unix it uses `dev`/`ino`. If the platform
/// cannot provide an identity, fall back to a hash of the normalized path and
/// document that hard links are not detectable in that case.
#[cfg(windows)]
fn identity_key(_path: &Path, file: &File) -> String {
    match winapi_util::file::information(file) {
        Ok(info) => format!(
            "w-{:08x}-{:016x}",
            info.volume_serial_number(),
            info.file_index()
        ),
        Err(_) => path_key(_path),
    }
}

#[cfg(unix)]
fn identity_key(_path: &Path, file: &File) -> String {
    use std::os::unix::fs::MetadataExt;
    match file.metadata() {
        Ok(metadata) => format!("u-{:016x}-{:016x}", metadata.dev(), metadata.ino()),
        Err(_) => path_key(_path),
    }
}

#[cfg(not(any(windows, unix)))]
fn identity_key(path: &Path, _file: &File) -> String {
    path_key(path)
}

fn path_key(path: &Path) -> String {
    let normalized = paths::normalized(path);
    format!("p-{:016x}", fnv1a(normalized.to_string_lossy().as_bytes()))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn note_nonce() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("{}-{nanos}", std::process::id())
}

fn write_owner_note(path: &Path, nonce: &str) {
    if let Ok(mut note) = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
    {
        let _ = writeln!(note, "pid={}", std::process::id());
        let _ = writeln!(note, "nonce={nonce}");
    }
}

fn publish_owner_note(staged: &Path, destination: &Path) {
    // `fs::rename` does not replace an existing file on Windows. Removing the
    // destination first leaves a short window with no PID note, but never
    // publishes the wrong PID (we only reach here while holding the lock).
    if std::fs::rename(staged, destination).is_err() {
        let _ = std::fs::remove_file(destination);
        let _ = std::fs::rename(staged, destination);
    }
    let _ = std::fs::remove_file(staged);
}

fn remove_owner_note(path: &Path, nonce: &str) {
    let Ok(note) = std::fs::read_to_string(path) else {
        return;
    };
    if note
        .lines()
        .any(|line| line.trim().strip_prefix("nonce=") == Some(nonce))
    {
        let _ = std::fs::remove_file(path);
    }
}

fn holder_pid(path: &Path) -> Option<u32> {
    let note = std::fs::read_to_string(path).ok()?;
    note.lines()
        .find_map(|line| line.trim().strip_prefix("pid=")?.trim().parse().ok())
}
