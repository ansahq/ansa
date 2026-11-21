//! Versioned JSON import and export with per-item diagnostics.
//!
//! Exports are deterministic: quicklinks are ordered by stable identity, tags
//! are canonicalised, and the document carries [`SCHEMA_VERSION`] (1). Import
//! accepts version 1 documents and revalidates every item under the current
//! policy, so tightened validation is enforced without changing the previous
//! shape. Malformed items are reported with their index and do not discard the
//! rest of the set.
//!
//! Merge mode never overwrites an existing quicklink; conflicting identities are
//! reported. Replace mode is all-or-nothing and requires
//! [`ImportMode::ReplaceConfirmed`].

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::Serialize;
use serde_json::Value;

use crate::id::QuicklinkId;
use crate::library::{Library, MAX_QUICKLINKS, alias_key};
use crate::model::Quicklink;
use crate::policy::{OpenPolicy, resolve_target};
use crate::template::{ArgKind, ArgumentValue, Template};

/// Current export schema version.
///
/// The document shape is unchanged by the tightened validation, so version 1 is
/// retained for consumer compatibility (`ansa-alpha` currently pins it). Import
/// always revalidates every item with the current policy. A future shape change
/// can raise this and keep [`MIN_SUPPORTED_SCHEMA_VERSION`] at 1.
pub const SCHEMA_VERSION: u32 = 1;
/// Oldest import schema version accepted.
pub const MIN_SUPPORTED_SCHEMA_VERSION: u32 = 1;
/// Maximum accepted import document size in bytes.
pub const MAX_IMPORT_BYTES: usize = 4 * 1024 * 1024;

/// How an import interacts with existing records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportMode {
    /// Add new quicklinks and never overwrite existing ones.
    Merge,
    /// Replace the whole library atomically, but only if every item is valid.
    ///
    /// This is a caller contract: the host must obtain explicit user
    /// confirmation (for example from a dry-run report) before selecting it.
    /// The crate has no UI and cannot verify that confirmation.
    ReplaceConfirmed,
}

/// Import behaviour for one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportOptions {
    /// Merge or all-or-nothing replacement.
    pub mode: ImportMode,
    /// Compute and report the outcome without mutating the library.
    pub dry_run: bool,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            mode: ImportMode::Merge,
            dry_run: false,
        }
    }
}

impl ImportOptions {
    /// Merge mode, no dry run.
    pub fn merge() -> Self {
        Self::default()
    }

    /// All-or-nothing replacement mode, no dry run.
    pub fn replace_confirmed() -> Self {
        Self {
            mode: ImportMode::ReplaceConfirmed,
            dry_run: false,
        }
    }

    /// Returns these options with dry-run enabled.
    pub fn with_dry_run(mut self) -> Self {
        self.dry_run = true;
        self
    }
}

/// One per-item import failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportItemError {
    /// Zero-based index in the document's `quicklinks` array, when known.
    pub index: Option<usize>,
    /// Identity hint from the raw item, when readable.
    pub id: Option<String>,
    /// Actionable explanation.
    pub message: String,
}

impl fmt::Display for ImportItemError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.index, &self.id) {
            (Some(index), Some(id)) => write!(formatter, "item {index} ({id}): {}", self.message),
            (Some(index), None) => write!(formatter, "item {index}: {}", self.message),
            (None, Some(id)) => write!(formatter, "document ({id}): {}", self.message),
            (None, None) => write!(formatter, "document: {}", self.message),
        }
    }
}

/// Outcome of an import, including a dry run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    /// Number of items in the document.
    pub total: usize,
    /// Items applied (or, for a dry run, that would be applied).
    pub applied: usize,
    /// Items not applied, including every item when replacement is refused.
    pub skipped: usize,
    /// Per-item and document-level failures.
    pub errors: Vec<ImportItemError>,
    /// Whether this was a dry run.
    pub dry_run: bool,
    /// Whether the library was actually replaced.
    pub replaced: bool,
    /// Whether the library would be replaced by a non-dry-run import.
    pub would_replace: bool,
}

impl ImportReport {
    fn empty(dry_run: bool) -> Self {
        Self {
            total: 0,
            applied: 0,
            skipped: 0,
            errors: Vec::new(),
            dry_run,
            replaced: false,
            would_replace: false,
        }
    }

    fn fail(&mut self, index: Option<usize>, id: Option<String>, message: impl Into<String>) {
        self.errors.push(ImportItemError {
            index,
            id,
            message: message.into(),
        });
    }

    /// True when at least one item or the document failed.
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// True when every item was applied and no error was reported.
    ///
    /// For a dry run this means every item *would* be applied; nothing was
    /// stored.
    pub fn is_complete(&self) -> bool {
        self.errors.is_empty() && self.applied == self.total
    }

    fn finish(mut self) -> Self {
        self.skipped = self.total.saturating_sub(self.applied);
        self
    }
}

impl fmt::Display for ImportReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let qualifier = if self.dry_run {
            "would apply"
        } else {
            "applied"
        };
        write!(
            formatter,
            "{qualifier} {} of {} quicklinks with {} error(s)",
            self.applied,
            self.total,
            self.errors.len()
        )
    }
}

/// Why an export could not be serialized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportError {
    Serialize(String),
}

impl fmt::Display for ExportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize(message) => {
                write!(formatter, "could not serialize quicklinks: {message}")
            }
        }
    }
}

impl std::error::Error for ExportError {}

#[derive(Serialize)]
struct ExportDocument<'a> {
    schema_version: u32,
    quicklinks: Vec<&'a Quicklink>,
}

impl Library {
    /// Serializes the library as deterministic, versioned JSON.
    pub fn export_json(&self) -> Result<String, ExportError> {
        let document = ExportDocument {
            schema_version: SCHEMA_VERSION,
            quicklinks: self.entries.values().collect(),
        };
        serde_json::to_string_pretty(&document)
            .map_err(|error| ExportError::Serialize(error.to_string()))
    }

    /// Imports a versioned JSON document and returns a per-item report.
    ///
    /// This uses [`OpenPolicy::with_local_paths`] so ordinary persisted local
    /// links can be restored, but network/device paths and executables remain
    /// disabled. Hosts with a different policy should call
    /// [`Library::import_json_with_policy`].
    ///
    /// Every item is resolved with its defaults (or a documented sample for a
    /// required argument) and checked through the same target policy as
    /// [`Library::preview`] before it is accepted. Unsafe defaults, blocked
    /// schemes and opener mismatches become per-item errors.
    ///
    /// Merge mode applies every valid, non-conflicting item and leaves stored
    /// records untouched. Replace mode changes nothing unless every item is
    /// valid, in which case the whole library is replaced atomically.
    pub fn import_json(&mut self, json: &str, options: ImportOptions) -> ImportReport {
        self.import_json_with_policy(json, options, OpenPolicy::with_local_paths())
    }

    /// Imports with an explicit target policy.
    ///
    /// The policy is applied to every item before any item is added, using the
    /// item's defaults and the placeholder sample described in
    /// [`Library::import_json`]. Replace mode remains all-or-nothing.
    pub fn import_json_with_policy(
        &mut self,
        json: &str,
        options: ImportOptions,
        policy: OpenPolicy,
    ) -> ImportReport {
        let mut report = ImportReport::empty(options.dry_run);
        if json.len() > MAX_IMPORT_BYTES {
            report.fail(
                None,
                None,
                format!("document is larger than {MAX_IMPORT_BYTES} bytes"),
            );
            return report;
        }
        let document: Value = match serde_json::from_str(json) {
            Ok(value) => value,
            Err(error) => {
                report.fail(None, None, format!("invalid JSON: {error}"));
                return report;
            }
        };
        let Some(object) = document.as_object() else {
            report.fail(None, None, "document must be a JSON object");
            return report;
        };
        match object.get("schema_version").and_then(Value::as_u64) {
            Some(version)
                if (u64::from(MIN_SUPPORTED_SCHEMA_VERSION)..=u64::from(SCHEMA_VERSION))
                    .contains(&version) => {}
            Some(version) => {
                report.fail(
                    None,
                    None,
                    format!(
                        "unsupported schema_version {version}; this build reads versions {MIN_SUPPORTED_SCHEMA_VERSION}..={SCHEMA_VERSION}"
                    ),
                );
                return report;
            }
            None => {
                report.fail(
                    None,
                    None,
                    format!(
                        "missing integer schema_version; expected {MIN_SUPPORTED_SCHEMA_VERSION}..={SCHEMA_VERSION}"
                    ),
                );
                return report;
            }
        }
        let Some(items) = object.get("quicklinks").and_then(Value::as_array) else {
            report.fail(None, None, "document must contain a 'quicklinks' array");
            return report;
        };
        report.total = items.len();
        if items.len() > crate::library::MAX_QUICKLINKS {
            report.fail(
                None,
                None,
                format!(
                    "document contains more than {} quicklinks",
                    crate::library::MAX_QUICKLINKS
                ),
            );
            return report.finish();
        }

        let mut parsed: Vec<(usize, Quicklink)> = Vec::new();
        let mut ids: BTreeSet<QuicklinkId> = BTreeSet::new();
        let mut aliases: BTreeMap<String, QuicklinkId> = BTreeMap::new();
        for (index, item) in items.iter().enumerate() {
            let id_hint = item.get("id").and_then(Value::as_str).map(str::to_owned);
            let link = match serde_json::from_value::<Quicklink>(item.clone()) {
                Ok(link) => link.normalized(),
                Err(error) => {
                    report.fail(Some(index), id_hint, error.to_string());
                    continue;
                }
            };
            if let Err(error) = link.validate() {
                report.fail(Some(index), Some(link.id().to_string()), error.to_string());
                continue;
            }
            if let Err(message) = validate_import_target(&link, policy) {
                report.fail(Some(index), Some(link.id().to_string()), message);
                continue;
            }
            if ids.contains(link.id()) {
                report.fail(
                    Some(index),
                    Some(link.id().to_string()),
                    "duplicate identity in this document",
                );
                continue;
            }
            if let Some(alias) = link.alias() {
                let key = alias_key(alias);
                if let Some(existing) = aliases.get(&key) {
                    report.fail(
                        Some(index),
                        Some(link.id().to_string()),
                        format!("alias {alias:?} is also used by {existing} in this document"),
                    );
                    continue;
                }
                aliases.insert(key, link.id().clone());
            }
            ids.insert(link.id().clone());
            parsed.push((index, link));
        }

        match options.mode {
            ImportMode::Merge => self.import_merge(parsed, options, &mut report),
            ImportMode::ReplaceConfirmed => self.import_replace(parsed, options, &mut report),
        }
        report.finish()
    }

    fn import_merge(
        &mut self,
        parsed: Vec<(usize, Quicklink)>,
        options: ImportOptions,
        report: &mut ImportReport,
    ) {
        let new_count = parsed
            .iter()
            .filter(|(_, link)| !self.entries.contains_key(link.id()))
            .count();
        let projected = self.entries.len().saturating_add(new_count);
        if projected > MAX_QUICKLINKS {
            report.fail(
                None,
                None,
                format!(
                    "merge needs room for {new_count} new quicklinks (library has {}, maximum {MAX_QUICKLINKS}); no quicklinks were imported",
                    self.entries.len()
                ),
            );
            return;
        }

        let mut aliases: BTreeMap<String, QuicklinkId> = self
            .entries
            .values()
            .filter_map(|link| {
                link.alias()
                    .map(|alias| (alias_key(alias), link.id().clone()))
            })
            .collect();
        let mut changed = false;
        for (index, link) in parsed {
            if let Some(existing) = self.entries.get(link.id()) {
                if existing == &link {
                    report.applied += 1;
                    continue;
                }
                report.fail(
                    Some(index),
                    Some(link.id().to_string()),
                    "conflicts with the stored quicklink; merge never overwrites, use an explicit replace",
                );
                continue;
            }
            if let Some(alias) = link.alias()
                && let Some(owner) = aliases.get(&alias_key(alias))
            {
                report.fail(
                    Some(index),
                    Some(link.id().to_string()),
                    format!("alias {alias:?} is already used by {owner}"),
                );
                continue;
            }
            if let Some(alias) = link.alias() {
                aliases.insert(alias_key(alias), link.id().clone());
            }
            if !options.dry_run {
                self.entries.insert(link.id().clone(), link);
                changed = true;
            }
            report.applied += 1;
        }
        if changed {
            self.revision = self.revision.saturating_add(1);
        }
    }

    fn import_replace(
        &mut self,
        parsed: Vec<(usize, Quicklink)>,
        options: ImportOptions,
        report: &mut ImportReport,
    ) {
        if report.has_errors() {
            return;
        }
        report.would_replace = true;
        if !options.dry_run {
            self.entries = parsed
                .into_iter()
                .map(|(_, link)| (link.id().clone(), link))
                .collect();
            self.revision = self.revision.saturating_add(1);
            report.replaced = true;
        }
        report.applied = report.total;
    }
}

/// Builds the argument set used to check an imported template before storage.
///
/// Optional arguments use their declared default. Required arguments use a
/// deterministic, safe sample: `"sample"` for text, `1` for numbers, `true` for
/// booleans and the first choice. A template whose target is only a placeholder
/// therefore resolves to a relative sample and is rejected, which is correct:
/// component encoding cannot introduce an absolute URL scheme or path
/// separator. Actual user values are still checked by preview before opening.
fn import_sample_values(template: &Template) -> BTreeMap<String, ArgumentValue> {
    template
        .arguments()
        .iter()
        .map(|spec| {
            let value = spec.default.clone().unwrap_or_else(|| match spec.kind {
                ArgKind::Text => ArgumentValue::text("sample"),
                ArgKind::Number => ArgumentValue::Number("1".to_owned()),
                ArgKind::Boolean => ArgumentValue::Boolean(true),
                ArgKind::Choice => {
                    ArgumentValue::Choice(spec.choices.first().cloned().unwrap_or_default())
                }
            });
            (spec.name.clone(), value)
        })
        .collect()
}

/// Resolves an import sample and applies the same target policy as preview.
fn validate_import_target(link: &Quicklink, policy: OpenPolicy) -> Result<(), String> {
    let values = import_sample_values(link.template());
    let target = link
        .template()
        .resolve(&values)
        .map_err(|error| format!("cannot resolve the import sample: {error}"))?;
    resolve_target(&target, link.opener(), policy).map_err(|error| {
        format!("resolved sample target {target:?} is not allowed by the import policy: {error}")
    })?;
    Ok(())
}
