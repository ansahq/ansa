//! Typed recovery reports.
//!
//! Recovery is deliberately explicit: when an irreversible migration writes a
//! pre-migration snapshot, or a restore aborts after touching the live files,
//! the store records what happened, which file is involved and where the
//! preserved original lives. Callers can surface this instead of relying on a
//! free-form error string.

use std::path::{Path, PathBuf};

/// What produced a [`RecoveryInfo`] entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryKind {
    /// A verified `VACUUM INTO` snapshot was written before an irreversible
    /// migration. The snapshot can be restored with
    /// [`crate::Store::restore_from`].
    PreMigrationSnapshot,
    /// A restore attempt failed. The original database was kept (or a
    /// pre-restore copy was preserved when rollback could not finish).
    RestoreAborted,
}

/// A recovery action that already happened, or a preserved recovery copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryInfo {
    /// What prompted the report.
    pub kind: RecoveryKind,
    /// Snapshot or backup file the report refers to.
    pub path: PathBuf,
    /// Original database (or preserved pre-restore copy) to fall back to,
    /// when one is known to exist.
    pub recovery_copy: Option<PathBuf>,
    /// Human-readable reason suitable for a log or a recovery prompt.
    pub reason: String,
}

impl RecoveryInfo {
    pub(crate) fn pre_migration_snapshot(path: PathBuf, version: u32) -> Self {
        Self {
            kind: RecoveryKind::PreMigrationSnapshot,
            path,
            recovery_copy: None,
            reason: format!(
                "verified snapshot written before irreversible migration v{version}; \
                 restore it with Store::restore_from if the migration is not wanted"
            ),
        }
    }

    pub(crate) fn restore_aborted(
        source: &Path,
        recovery_copy: Option<PathBuf>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            kind: RecoveryKind::RestoreAborted,
            path: source.to_path_buf(),
            recovery_copy,
            reason: reason.into(),
        }
    }
}
