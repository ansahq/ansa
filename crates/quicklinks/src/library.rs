//! In-memory quicklink library and the preview/commit gate.
//!
//! The library is deliberately renderer- and storage-agnostic. It enforces
//! identity stability, alias uniqueness, bounded size and the preview rule that
//! a quicklink may only be saved through a still-current [`Preview`]. Durable
//! persistence and tombstones live in the storage crate.

use std::collections::BTreeMap;
use std::fmt;

use crate::id::QuicklinkId;
use crate::model::{ModelError, Preview, PreviewError, Quicklink};
use crate::policy::{OpenPolicy, OpenerProbe};

/// Maximum number of quicklinks held in one library.
pub const MAX_QUICKLINKS: usize = 10_000;

/// A deterministic, bounded collection of quicklinks.
#[derive(Debug, Clone, Default)]
pub struct Library {
    pub(crate) entries: BTreeMap<QuicklinkId, Quicklink>,
    pub(crate) revision: u64,
}

/// Why a direct library operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryError {
    NotFound { id: QuicklinkId },
}

impl fmt::Display for LibraryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { id } => write!(formatter, "quicklink {id} does not exist"),
        }
    }
}

impl std::error::Error for LibraryError {}

/// Why a preview could not be committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitError {
    /// The library changed after the preview was built (stale preview).
    Conflict {
        preview: u64,
        current: u64,
    },
    /// An update referenced a quicklink that no longer exists.
    NotFound {
        id: QuicklinkId,
    },
    /// An insert referenced an identity that already exists.
    DuplicateId {
        id: QuicklinkId,
    },
    /// Another quicklink already owns the alias.
    AliasTaken {
        alias: String,
        existing: QuicklinkId,
    },
    LimitReached {
        limit: usize,
    },
    FingerprintMismatch,
    Model(ModelError),
}

impl fmt::Display for CommitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict { preview, current } => write!(
                formatter,
                "preview conflict: this preview is stale (built at revision {preview}, library is at {current}); preview again before saving"
            ),
            Self::NotFound { id } => write!(formatter, "quicklink {id} does not exist"),
            Self::DuplicateId { id } => {
                write!(
                    formatter,
                    "quicklink {id} already exists; update it instead"
                )
            }
            Self::AliasTaken { alias, existing } => write!(
                formatter,
                "alias {alias:?} is already used by {existing}; choose another alias"
            ),
            Self::LimitReached { limit } => {
                write!(
                    formatter,
                    "the library is full; remove a quicklink before adding more than {limit}"
                )
            }
            Self::FingerprintMismatch => formatter.write_str(
                "this preview's fingerprint no longer matches its data; preview again before saving",
            ),
            Self::Model(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for CommitError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoreMode {
    Upsert,
    Insert,
    Update,
}

impl Library {
    /// Creates an empty library at revision zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Current revision; incremented by every successful mutation.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Number of stored quicklinks.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when the library has no quicklinks.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Looks up one quicklink by identity.
    pub fn get(&self, id: &QuicklinkId) -> Option<&Quicklink> {
        self.entries.get(id)
    }

    /// All quicklinks in stable identity order.
    ///
    /// The result is bounded by [`MAX_QUICKLINKS`]. Hosts that render a page at
    /// a time should use [`Library::list_page`].
    pub fn list(&self) -> Vec<&Quicklink> {
        self.entries.values().collect()
    }

    /// A bounded page of quicklinks in stable identity order.
    pub fn list_page(&self, offset: usize, limit: usize) -> Vec<&Quicklink> {
        self.entries.values().skip(offset).take(limit).collect()
    }

    /// Builds a preview without changing the library.
    ///
    /// This resolves the template with `values`, enforces scheme/path policy,
    /// probes the selected opener and fingerprints the exact proposal. The
    /// returned [`Preview`] is the only way to save that proposal.
    pub fn preview(
        &self,
        candidate: &Quicklink,
        values: &BTreeMap<String, String>,
        policy: OpenPolicy,
        probe: &dyn OpenerProbe,
    ) -> Result<Preview, PreviewError> {
        let candidate = candidate.normalized();
        if let Some(alias) = candidate.alias()
            && let Some(existing) = self.alias_owner(alias, Some(candidate.id()))
        {
            return Err(PreviewError::AliasTaken {
                alias: alias.to_owned(),
                existing: existing.id().clone(),
            });
        }
        if !self.entries.contains_key(candidate.id()) && self.entries.len() >= MAX_QUICKLINKS {
            return Err(PreviewError::LimitReached {
                limit: MAX_QUICKLINKS,
            });
        }
        Preview::build(candidate, values, policy, probe, self.revision)
    }

    /// Saves a preview as an insert-or-update.
    pub fn commit(&mut self, preview: &Preview) -> Result<Quicklink, CommitError> {
        self.store(preview, StoreMode::Upsert)
    }

    /// Saves a preview, requiring that the identity is new.
    pub fn insert(&mut self, preview: &Preview) -> Result<Quicklink, CommitError> {
        self.store(preview, StoreMode::Insert)
    }

    /// Saves a preview, requiring that the identity already exists.
    pub fn update(&mut self, preview: &Preview) -> Result<Quicklink, CommitError> {
        self.store(preview, StoreMode::Update)
    }

    /// Removes a quicklink and bumps the revision.
    pub fn remove(&mut self, id: &QuicklinkId) -> Result<Quicklink, LibraryError> {
        let removed = self
            .entries
            .remove(id)
            .ok_or_else(|| LibraryError::NotFound { id: id.clone() })?;
        self.revision = self.revision.saturating_add(1);
        Ok(removed)
    }

    fn store(&mut self, preview: &Preview, mode: StoreMode) -> Result<Quicklink, CommitError> {
        if preview.revision() != self.revision {
            return Err(CommitError::Conflict {
                preview: preview.revision(),
                current: self.revision,
            });
        }
        if !preview.verify_fingerprint() {
            return Err(CommitError::FingerprintMismatch);
        }
        let candidate = preview.candidate().clone();
        candidate.validate().map_err(CommitError::Model)?;
        let exists = self.entries.contains_key(candidate.id());
        match mode {
            StoreMode::Insert if exists => {
                return Err(CommitError::DuplicateId {
                    id: candidate.id().clone(),
                });
            }
            StoreMode::Update if !exists => {
                return Err(CommitError::NotFound {
                    id: candidate.id().clone(),
                });
            }
            StoreMode::Insert | StoreMode::Update | StoreMode::Upsert => {}
        }
        if let Some(alias) = candidate.alias()
            && let Some(existing) = self.alias_owner(alias, Some(candidate.id()))
        {
            return Err(CommitError::AliasTaken {
                alias: alias.to_owned(),
                existing: existing.id().clone(),
            });
        }
        if !exists && self.entries.len() >= MAX_QUICKLINKS {
            return Err(CommitError::LimitReached {
                limit: MAX_QUICKLINKS,
            });
        }
        self.entries
            .insert(candidate.id().clone(), candidate.clone());
        self.revision = self.revision.saturating_add(1);
        Ok(candidate)
    }

    /// Finds the quicklink that owns `alias`, ignoring `exclude`.
    pub(crate) fn alias_owner(
        &self,
        alias: &str,
        exclude: Option<&QuicklinkId>,
    ) -> Option<&Quicklink> {
        let key = alias_key(alias);
        self.entries.values().find(|link| {
            link.alias().is_some_and(|value| alias_key(value) == key) && Some(link.id()) != exclude
        })
    }
}

/// Case-insensitive alias comparison key.
pub(crate) fn alias_key(alias: &str) -> String {
    alias.trim().to_lowercase()
}
