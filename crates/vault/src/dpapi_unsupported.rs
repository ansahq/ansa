//! Explicit unsupported backend.
//!
//! There is deliberately no plaintext fallback and no home-grown cipher. On
//! non-Windows hosts [`crate::Vault::open`] fails before creating a directory,
//! and these helpers fail defensively if they are ever reached.

use crate::error::VaultError;

pub(crate) fn protect(_plaintext: &[u8], _entropy: &[u8]) -> Result<Vec<u8>, VaultError> {
    Err(unsupported())
}

pub(crate) fn unprotect(_ciphertext: &[u8], _entropy: &[u8]) -> Result<Vec<u8>, VaultError> {
    Err(unsupported())
}

fn unsupported() -> VaultError {
    VaultError::Unsupported {
        platform: std::env::consts::OS,
        reason: "only Windows DPAPI is implemented; no plaintext fallback exists",
    }
}
