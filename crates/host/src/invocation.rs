//! Opaque, monotonically issued invocation identity.
//!
//! An [`InvocationId`] is only created by an [`InvocationIssuer`] (normally the
//! one owned by [`crate::HostController`]). It combines a process-unique nonce
//! with a monotonic counter, so an id from an earlier summon can never compare
//! equal to the id of a later summon, even after wraparound, restart or a
//! second controller in the same process.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_ISSUER: AtomicU64 = AtomicU64::new(1);

fn process_nonce() -> u64 {
    static NONCE: OnceLock<u64> = OnceLock::new();
    *NONCE.get_or_init(|| {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(0);
        let pid = u64::from(std::process::id());
        let address = (&now as *const u64 as usize) as u64;
        let mixed = now ^ pid.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ address.rotate_left(23);
        if mixed == 0 { 1 } else { mixed }
    })
}

/// Opaque identity of one launcher summon.
///
/// The fields are private and `Debug` deliberately redacts the nonce. The value
/// is `Copy` only so it can be passed to checked controller calls; it cannot be
/// constructed outside this crate.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InvocationId {
    nonce: u64,
    sequence: u64,
}

impl InvocationId {
    /// The monotonic sequence number, safe to include in logs.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
}

impl std::fmt::Debug for InvocationId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("InvocationId(..)")
    }
}

impl std::fmt::Display for InvocationId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "invocation#{}", self.sequence)
    }
}

/// The only supported source of [`InvocationId`] values.
///
/// Every call to [`InvocationIssuer::issue`] returns a value that differs from
/// every earlier value in this process. Cloning is intentionally not
/// implemented: share the controller (or a borrow of it) instead of copying an
/// issuer so the counter cannot diverge.
#[derive(Debug)]
pub struct InvocationIssuer {
    nonce: u64,
    next_sequence: AtomicU64,
}

impl InvocationIssuer {
    /// Creates a new issuer with a fresh process-unique nonce.
    pub fn new() -> Self {
        let issuer_sequence = NEXT_ISSUER.fetch_add(1, Ordering::Relaxed);
        let nonce = process_nonce() ^ issuer_sequence.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        Self {
            nonce: if nonce == 0 { 1 } else { nonce },
            next_sequence: AtomicU64::new(1),
        }
    }

    /// Issues the next unique invocation identity.
    pub fn issue(&self) -> InvocationId {
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        InvocationId {
            nonce: self.nonce,
            sequence,
        }
    }
}

impl Default for InvocationIssuer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn issued_invocations_are_unique_and_ordered() {
        let issuer = InvocationIssuer::new();
        let first = issuer.issue();
        let second = issuer.issue();
        assert_ne!(first, second);
        assert!(second.sequence() > first.sequence());
        assert_eq!(first.sequence(), 1);
        assert_eq!(first.to_string(), "invocation#1");
    }

    #[test]
    fn a_second_issuer_cannot_reproduce_the_first_ids() {
        let first = InvocationIssuer::new();
        let second = InvocationIssuer::new();
        assert_ne!(first.issue(), second.issue());
    }

    #[test]
    fn ids_are_unique_across_many_issues() {
        let issuer = InvocationIssuer::new();
        let ids: HashSet<_> = (0..10_000).map(|_| issuer.issue()).collect();
        assert_eq!(ids.len(), 10_000);
    }

    #[test]
    fn debug_output_does_not_expose_the_process_nonce() {
        let id = InvocationIssuer::new().issue();
        assert_eq!(format!("{id:?}"), "InvocationId(..)");
        assert!(!format!("{id:?}").contains(&id.sequence().to_string()));
    }
}
