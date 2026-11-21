//! Reveal-in-file-manager and structured copy-identity actions.
//!
//! Reveal uses the shell item APIs directly:
//!
//! 1. the caller supplies an absolute path that exists (and is not a reparse
//!    point);
//! 2. `SHParseDisplayName` converts that single path to an absolute PIDL;
//! 3. `SHOpenFolderAndSelectItems` asks the registered file manager to open the
//!    containing folder and select the item.
//!
//! No `explorer.exe` command line is constructed and no metadata is
//! interpolated into a shell string. If the shell rejects the request, an
//! explicit error is returned; this module never reports a reveal it did not
//! observe as accepted.
//!
//! Structured copy identity is a pure formatter on [`crate::AppEntry`]
//! (`identity_summary` / [`crate::copy_identity`]), so it is also usable when
//! the shell is unavailable (for example in a CLI or a remotely driven test).

use std::ffi::c_void;
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

const COINIT_APARTMENTTHREADED: u32 = 0x2;
const RPC_E_CHANGED_MODE: u32 = 0x8001_0106;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

/// Reveals an existing path in the registered file manager.
///
/// The path must be absolute, exist and contain no NUL. Relative paths are
/// rejected rather than resolved through the shell. The call returns `Ok(())`
/// only when `SHOpenFolderAndSelectItems` reports success; a denied or failed
/// shell request returns a descriptive error.
pub fn reveal_in_file_manager(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("reveal path must not be empty".to_owned());
    }
    if path.contains('\0') {
        return Err("reveal path must not contain NUL".to_owned());
    }
    let supplied = Path::new(path);
    if !supplied.is_absolute() {
        return Err(format!(
            "reveal path '{}' is not absolute; it is never resolved through a shell command",
            supplied.display()
        ));
    }
    let metadata = fs::symlink_metadata(supplied).map_err(|error| {
        format!(
            "reveal path '{}' is not accessible: {error}",
            supplied.display()
        )
    })?;
    if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
        return Err(format!(
            "reveal path '{}' is a reparse point; refusing to follow it",
            supplied.display()
        ));
    }
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(format!(
            "reveal path '{}' is neither a file nor a directory",
            supplied.display()
        ));
    }

    let mut wide: Vec<u16> = supplied.as_os_str().encode_wide().collect();
    wide.push(0);

    let com = ComApartment::initialize()?;
    let mut pidl: *mut c_void = std::ptr::null_mut();
    let mut attributes = 0_u32;
    // SAFETY: `wide` is NUL-terminated and alive for the call; `pidl` and
    // `attributes` are valid output pointers. SHParseDisplayName allocates the
    // returned PIDL with CoTaskMemAlloc, freed below.
    let parsed = unsafe {
        SHParseDisplayName(
            wide.as_ptr(),
            std::ptr::null_mut(),
            &mut pidl,
            attributes,
            &mut attributes as *mut u32,
        )
    };
    if parsed < 0 || pidl.is_null() {
        return Err(format!(
            "the shell could not parse the reveal path (HRESULT 0x{parsed:08X})"
        ));
    }
    let pidl = OwnedPidl(pidl);

    // SAFETY: `pidl.0` is an absolute PIDL produced by SHParseDisplayName. With
    // a null folder and cidl 0, the shell selects that absolute item. The PIDL
    // stays alive for the duration of the call.
    let opened = unsafe { SHOpenFolderAndSelectItems(pidl.0, 0, std::ptr::null(), 0) };
    drop(pidl);
    drop(com);

    if opened < 0 {
        return Err(format!(
            "the file manager rejected the reveal request (HRESULT 0x{opened:08X})"
        ));
    }
    Ok(())
}

fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    (metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
}

/// Initializes COM for the shell call and balances `CoUninitialize`.
///
/// `RPC_E_CHANGED_MODE` means COM is already initialized in another apartment;
/// the shell APIs are still callable, and this guard must not uninitialize a
/// COM apartment it did not create.
#[derive(Debug)]
struct ComApartment {
    owned: bool,
}

impl ComApartment {
    fn initialize() -> Result<Self, String> {
        // SAFETY: a null reserved pointer and an explicit apartment model are
        // the documented arguments. The call has no pointer outputs.
        let result = unsafe { CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED) };
        if result >= 0 {
            return Ok(Self { owned: true });
        }
        if result as u32 == RPC_E_CHANGED_MODE {
            return Ok(Self { owned: false });
        }
        Err(format!(
            "COM could not be initialized for the shell reveal (HRESULT 0x{result:08X})"
        ))
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: the apartment was initialized once by this guard.
            unsafe {
                CoUninitialize();
            }
        }
    }
}

#[derive(Debug)]
struct OwnedPidl(*mut c_void);

impl Drop for OwnedPidl {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: SHParseDisplayName allocates the PIDL with CoTaskMemAlloc
            // and the handle is freed exactly once.
            unsafe {
                CoTaskMemFree(self.0);
            }
        }
    }
}

#[link(name = "shell32")]
extern "system" {
    fn SHParseDisplayName(
        name: *const u16,
        bind_context: *mut c_void,
        pidl: *mut *mut c_void,
        attributes_in: u32,
        attributes_out: *mut u32,
    ) -> i32;
    fn SHOpenFolderAndSelectItems(
        folder: *const c_void,
        item_count: u32,
        items: *const *const c_void,
        flags: u32,
    ) -> i32;
}

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, apartment_model: u32) -> i32;
    fn CoUninitialize();
    fn CoTaskMemFree(pointer: *mut c_void);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AppEntry;

    fn entry() -> AppEntry {
        AppEntry {
            id: "app:00112233445566778899aabbccddeeff".to_owned(),
            title: "Example".to_owned(),
            path: r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Example.lnk".to_owned(),
            target: Some(r"C:\Program Files\Example\example.exe".to_owned()),
            arguments: Some("--profile test".to_owned()),
            working_directory: Some(r"C:\Program Files\Example".to_owned()),
            icon_source: Some(r"C:\Program Files\Example\example.exe,0".to_owned()),
            packaged: false,
            broken: None,
        }
    }

    #[test]
    fn reveal_rejects_relative_missing_and_nul_paths_before_shell_calls() {
        assert!(reveal_in_file_manager("").is_err());
        assert!(reveal_in_file_manager(r"relative\App.lnk").is_err());
        assert!(reveal_in_file_manager(r"C:\does\not\exist\App.lnk").is_err());
        assert!(reveal_in_file_manager("C:\\bad\0.lnk").is_err());
    }

    #[test]
    fn copy_identity_is_deterministic_and_explicit_about_empty_fields() {
        let identity = entry().identity_summary();
        assert_eq!(
            identity,
            concat!(
                "id: app:00112233445566778899aabbccddeeff\n",
                "title: Example\n",
                "path: C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs\\Example.lnk\n",
                "target: C:\\Program Files\\Example\\example.exe\n",
                "arguments: --profile test\n",
                "working_directory: C:\\Program Files\\Example\n",
                "icon_source: C:\\Program Files\\Example\\example.exe,0\n",
                "packaged: false\n",
                "broken: \n",
            )
        );
        assert_eq!(identity, crate::copy_identity(&entry()));

        let mut bare = AppEntry::new("app:bare", "Bare", r"C:\Bare.exe");
        bare.packaged = true;
        bare.broken = Some("moved; refresh the catalog".to_owned());
        let identity = crate::copy_identity(&bare);
        assert!(identity.contains("target: \n"));
        assert!(identity.contains("arguments: \n"));
        assert!(identity.contains("packaged: true\n"));
        assert!(identity.contains("broken: moved; refresh the catalog\n"));
    }
}
