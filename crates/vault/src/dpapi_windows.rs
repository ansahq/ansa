//! Windows DPAPI (`CryptProtectData` / `CryptUnprotectData`) FFI.
//!
//! This is the only module that calls into the operating system. All pointers
//! are derived from live Rust buffers for the duration of a single call, and
//! every output buffer returned by DPAPI is released with `LocalFree`.

use core::ffi::c_void;
use core::ptr;

use zeroize::Zeroize;

use crate::error::VaultError;
use crate::{MAX_PLAINTEXT_BYTES, MAX_SEALED_FILE_BYTES};

/// Do not display a UI prompt. All calls are silent and may fail instead.
const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x0000_0001;

#[repr(C)]
struct DataBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

#[link(name = "crypt32")]
unsafe extern "system" {
    fn CryptProtectData(
        data_in: *const DataBlob,
        data_description: *const u16,
        optional_entropy: *const DataBlob,
        reserved: *mut c_void,
        prompt: *mut c_void,
        flags: u32,
        data_out: *mut DataBlob,
    ) -> i32;

    fn CryptUnprotectData(
        data_in: *const DataBlob,
        data_description: *mut *mut u16,
        optional_entropy: *const DataBlob,
        reserved: *mut c_void,
        prompt: *mut c_void,
        flags: u32,
        data_out: *mut DataBlob,
    ) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
}

/// Seal plaintext to the current Windows user, bound to `entropy`.
pub(crate) fn protect(plaintext: &[u8], entropy: &[u8]) -> Result<Vec<u8>, VaultError> {
    let input_len = u32::try_from(plaintext.len()).map_err(|_| VaultError::LimitExceeded {
        kind: "plaintext",
        limit: MAX_PLAINTEXT_BYTES,
    })?;
    let entropy_len = u32::try_from(entropy.len()).map_err(|_| VaultError::LimitExceeded {
        kind: "entropy",
        limit: MAX_PLAINTEXT_BYTES,
    })?;

    let input = DataBlob {
        cb_data: input_len,
        pb_data: plaintext.as_ptr().cast_mut(),
    };
    let entropy_blob = DataBlob {
        cb_data: entropy_len,
        pb_data: entropy.as_ptr().cast_mut(),
    };
    let mut output = DataBlob {
        cb_data: 0,
        pb_data: ptr::null_mut(),
    };

    // SAFETY: `input`, `entropy_blob` and `output` are valid, correctly
    // aligned `DATA_BLOB` values for the duration of the call. The input and
    // entropy buffers outlive the call and are only read by DPAPI. `output` is
    // zeroed so DPAPI can replace it with an owned allocation.
    let succeeded = unsafe {
        CryptProtectData(
            &input,
            ptr::null(),
            &entropy_blob,
            ptr::null_mut(),
            ptr::null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };

    if succeeded == 0 {
        return Err(protect_error());
    }
    take_output(output, MAX_SEALED_FILE_BYTES, false)
}

/// Unseal a DPAPI-protected blob with the current Windows user, using the same
/// `entropy` that was supplied at protection time.
pub(crate) fn unprotect(ciphertext: &[u8], entropy: &[u8]) -> Result<Vec<u8>, VaultError> {
    let input_len = u32::try_from(ciphertext.len()).map_err(|_| VaultError::LimitExceeded {
        kind: "ciphertext",
        limit: MAX_SEALED_FILE_BYTES,
    })?;
    let entropy_len = u32::try_from(entropy.len()).map_err(|_| VaultError::LimitExceeded {
        kind: "entropy",
        limit: MAX_PLAINTEXT_BYTES,
    })?;

    let input = DataBlob {
        cb_data: input_len,
        pb_data: ciphertext.as_ptr().cast_mut(),
    };
    let entropy_blob = DataBlob {
        cb_data: entropy_len,
        pb_data: entropy.as_ptr().cast_mut(),
    };
    let mut output = DataBlob {
        cb_data: 0,
        pb_data: ptr::null_mut(),
    };

    // SAFETY: the same invariants as `protect` hold. The description pointer is
    // optional and explicitly null, and the prompt structure is not requested
    // because `CRYPTPROTECT_UI_FORBIDDEN` is set.
    let succeeded = unsafe {
        CryptUnprotectData(
            &input,
            ptr::null_mut(),
            &entropy_blob,
            ptr::null_mut(),
            ptr::null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };

    if succeeded == 0 {
        return Err(unprotect_error());
    }
    take_output(output, MAX_PLAINTEXT_BYTES, true)
}

fn take_output(output: DataBlob, limit: usize, sensitive: bool) -> Result<Vec<u8>, VaultError> {
    let length = output.cb_data as usize;
    if output.pb_data.is_null() {
        if length == 0 {
            return Ok(Vec::new());
        }
        return Err(VaultError::Internal {
            operation: "DPAPI returned a null output pointer",
        });
    }
    if length > limit {
        // SAFETY: DPAPI returned this pointer and it must be released exactly
        // once with `LocalFree`, even on the rejection path.
        unsafe {
            if sensitive {
                core::slice::from_raw_parts_mut(output.pb_data, length).zeroize();
            }
            LocalFree(output.pb_data.cast());
        }
        return Err(VaultError::LimitExceeded {
            kind: "DPAPI output",
            limit,
        });
    }

    // SAFETY: DPAPI guarantees `pb_data` points to `cb_data` initialized bytes
    // for the lifetime of this allocation. The slice is copied before the
    // allocation is freed.
    let bytes =
        unsafe { core::slice::from_raw_parts(output.pb_data.cast_const(), length) }.to_vec();

    // SAFETY: the allocation is owned by the caller and released exactly once.
    unsafe {
        if sensitive {
            core::slice::from_raw_parts_mut(output.pb_data, length).zeroize();
        }
        LocalFree(output.pb_data.cast());
    }
    Ok(bytes)
}

fn protect_error() -> VaultError {
    VaultError::Dpapi {
        operation: "protect",
        os_code: last_error_code(),
    }
}

/// `WIN32_ERROR` values all map to one safe error so callers cannot use the
/// vault as an oracle; no payload or Windows message is echoed.
fn unprotect_error() -> VaultError {
    VaultError::DecryptionFailed {
        os_code: last_error_code(),
    }
}

fn last_error_code() -> u32 {
    std::io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(0)
        .unsigned_abs()
}
