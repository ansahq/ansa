//! A small `ShellExecuteW` binding, isolated from the UI and domain crates.
//!
//! Callers pass an already validated file path or URL. No command line is
//! constructed and no string is interpolated into `cmd.exe` or PowerShell.

use std::ffi::{c_void, OsStr};
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

const SW_SHOWNORMAL: i32 = 1;

#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        hwnd: *mut c_void,
        operation: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show_command: i32,
    ) -> isize;
}

pub(crate) fn open_application(path: &str) -> Result<(), String> {
    let target = prepare_application_target(path)?;
    shell_open(target.as_os_str())
}

pub(crate) fn open_url(url: &str) -> Result<(), String> {
    shell_open(OsStr::new(url))
}

fn prepare_application_target(path: &str) -> Result<PathBuf, String> {
    if path.is_empty() {
        return Err("application path must not be empty".to_owned());
    }
    if path.contains('\0') {
        return Err("application path must not contain NUL".to_owned());
    }

    let supplied = Path::new(path);
    let extension = supplied
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("lnk" | "exe") => {}
        _ => {
            return Err("only existing .lnk and .exe application entries can be opened".to_owned())
        }
    }

    let absolute = if supplied.is_absolute() {
        supplied.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("cannot resolve the current directory: {error}"))?
            .join(supplied)
    };

    let metadata = fs::metadata(&absolute)
        .map_err(|error| format!("application entry is not accessible: {error}"))?;
    if !metadata.is_file() {
        return Err("application entry is not a regular file".to_owned());
    }

    Ok(absolute)
}

fn shell_open(target: &OsStr) -> Result<(), String> {
    let operation = wide_string("open");
    let file = wide_os_string(target)?;

    // SAFETY: both wide strings are NUL-terminated and alive for the duration
    // of the call. Optional parameters are null. ShellExecuteW does not retain
    // the pointers after returning.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };

    if result > 32 {
        Ok(())
    } else {
        Err(shell_error(result))
    }
}

fn wide_string(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_os_string(value: &OsStr) -> Result<Vec<u16>, String> {
    let mut units: Vec<u16> = value.encode_wide().collect();
    if units.contains(&0) {
        return Err("launch target must not contain NUL".to_owned());
    }
    units.push(0);
    Ok(units)
}

fn shell_error(code: isize) -> String {
    let description = match code {
        0 => "the operating system is out of memory or resources",
        2 => "the file was not found",
        3 => "the path was not found",
        5 => "access was denied",
        26 => "a sharing violation occurred",
        27 => "the file association is incomplete or invalid",
        28 => "the DDE transaction timed out",
        29 => "the DDE transaction failed",
        30 => "the DDE transaction is busy",
        31 => "there is no registered handler for this file type",
        32 => "a required DLL was not found",
        _ => "the shell reported an unknown failure",
    };
    format!("Windows ShellExecuteW failed (code {code}): {description}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_application_extensions_without_launching() {
        for path in [
            r"C:\Windows\notepad.txt",
            r"C:\Windows\notepad.bat",
            r"C:\Windows\System32\cmd.exe",
        ] {
            // cmd.exe is syntactically a .exe, so only the first two must be
            // rejected by extension. This test intentionally avoids calling
            // ShellExecuteW for cmd.exe.
            if path.to_ascii_lowercase().ends_with(".exe") {
                continue;
            }
            assert!(prepare_application_target(path).is_err(), "{path}");
        }
    }

    #[test]
    fn rejects_empty_and_nul_paths() {
        assert!(prepare_application_target("").is_err());
        assert!(prepare_application_target("C:\\bad\0.lnk").is_err());
    }

    #[test]
    fn rejects_missing_application_file() {
        let missing = std::env::temp_dir()
            .join("ansa-platform-windows-missing-application.lnk")
            .to_string_lossy()
            .into_owned();
        let _ = fs::remove_file(&missing);
        assert!(prepare_application_target(&missing).is_err());
    }

    #[test]
    fn rejects_directory_that_looks_like_an_application() {
        let directory = std::env::temp_dir().join("ansa-platform-windows-directory.lnk");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("create directory fixture");
        let path = directory.to_string_lossy().into_owned();
        let result = prepare_application_target(&path);
        let _ = fs::remove_dir_all(&directory);
        assert!(result.is_err());
    }

    #[test]
    fn wide_strings_are_nul_terminated() {
        assert_eq!(
            wide_string("open"),
            vec![b'o' as u16, b'p' as u16, b'e' as u16, b'n' as u16, 0]
        );
        assert!(wide_os_string(OsStr::new("C:\\App.lnk"))
            .unwrap()
            .ends_with(&[0]));
    }
}
