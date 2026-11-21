//! Credential references.
//!
//! Plaintext secrets are **never** accepted or stored by this crate. Only a
//! validated reference to an operating-system credential store or an approved
//! vault is persisted in `secret_refs`. The actual secret must be resolved by
//! the platform integration at the point of use.
//!
//! Approval schemes are deliberately explicit:
//!
//! * `secret://` — application-managed vault indirection
//! * `keychain://` — macOS Keychain
//! * `windows-credential://` — Windows Credential Manager
//! * `libsecret://` — freedesktop Secret Service
//! * `vault://` — a separately approved external vault
//!
//! A reference containing raw key material (for example a bare token) is
//! rejected by [`SecretRef::new`].

use std::fmt;

use crate::error::StorageError;

const APPROVED_SCHEMES: [&str; 5] = [
    "secret://",
    "keychain://",
    "windows-credential://",
    "libsecret://",
    "vault://",
];

/// A validated, non-secret reference to a credential.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretRef {
    alias: String,
    provider: String,
    reference: String,
}

impl SecretRef {
    /// Validate and construct a credential reference.
    ///
    /// `reference` must use one of the approved schemes; anything else — in
    /// particular a raw token — is rejected. The rejected value is not echoed
    /// in the error.
    pub fn new(
        alias: impl Into<String>,
        provider: impl Into<String>,
        reference: impl Into<String>,
    ) -> Result<Self, StorageError> {
        let alias = alias.into();
        let provider = provider.into();
        let reference = reference.into();

        if !is_identifier(&alias, 64) {
            return Err(StorageError::InvalidSecretRef {
                reason: "alias must be 1..=64 letters, digits, '.', '_' or '-'",
            });
        }
        if !is_identifier(&provider, 32) {
            return Err(StorageError::InvalidSecretRef {
                reason: "provider must be 1..=32 letters, digits, '.', '_' or '-'",
            });
        }
        validate_reference(&reference)?;

        Ok(Self {
            alias,
            provider,
            reference,
        })
    }

    /// Stable identifier used by settings and commands.
    #[must_use]
    pub fn alias(&self) -> &str {
        &self.alias
    }

    /// OS provider or vault name.
    #[must_use]
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// The opaque reference handed to the OS secret store. This is not a
    /// secret, but it is still redacted from `Debug` output.
    #[must_use]
    pub fn reference(&self) -> &str {
        &self.reference
    }
}

impl fmt::Debug for SecretRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretRef")
            .field("alias", &self.alias)
            .field("provider", &self.provider)
            .field("reference", &"<redacted>")
            .finish()
    }
}

fn is_identifier(value: &str, max_len: usize) -> bool {
    let length = value.len();
    if !(1..=max_len).contains(&length) {
        return false;
    }
    value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

fn validate_reference(value: &str) -> Result<(), StorageError> {
    if value.is_empty() || value.len() > 512 {
        return Err(StorageError::InvalidSecretRef {
            reason: "reference must be 1..=512 bytes",
        });
    }
    if value
        .chars()
        .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(StorageError::InvalidSecretRef {
            reason: "reference must not contain control characters or whitespace",
        });
    }
    let Some(scheme) = APPROVED_SCHEMES
        .iter()
        .find(|scheme| value.starts_with(**scheme))
    else {
        return Err(StorageError::InvalidSecretRef {
            reason: "reference must use an approved secret-store scheme",
        });
    };
    if value.len() == scheme.len() {
        return Err(StorageError::InvalidSecretRef {
            reason: "reference must not be only a scheme",
        });
    }
    Ok(())
}
