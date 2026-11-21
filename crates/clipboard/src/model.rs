//! Clipboard item metadata, capture policy and deterministic history.
//!
//! Nothing in this module holds clipboard payload bytes. [`ClipboardItem`] is a
//! bounded, non-sensitive metadata record; the encrypted payload lives in the
//! sealed backend owned by [`crate::store::ClipboardStore`]. Keeping payloads
//! out of the model makes accidental logging and metadata persistence safe by
//! construction.
//!
//! Quotas are deterministic: expired and over-quota entries are evicted oldest
//! first, ordered by `(captured_at, id)`. Pinned entries are never evicted by
//! quota enforcement or age pruning, but an explicit [`ClipboardHistory::remove`]
//! or [`ClipboardHistory::clear`] still removes them.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Hard ceiling on stored metadata entries, regardless of policy.
pub const ABSOLUTE_MAX_ITEMS: usize = 4096;

/// Hard ceiling on the configured history byte quota (1 GiB is deliberately
/// not allowed; history is a convenience cache, not an archive).
pub const ABSOLUTE_MAX_BYTES: usize = 64 * 1024 * 1024;

/// Hard ceiling on one captured item. The vault accepts 1 MiB of plaintext, so
/// this leaves room for the binary envelope and metadata.
pub const ABSOLUTE_MAX_ITEM_BYTES: usize = 900 * 1024;

/// Default policy: 200 items, 8 MiB total, 128 KiB per item, 7 days.
pub const DEFAULT_MAX_ITEMS: usize = 200;
/// Default total byte quota.
pub const DEFAULT_MAX_BYTES: usize = 8 * 1024 * 1024;
/// Default per-item byte limit.
pub const DEFAULT_MAX_ITEM_BYTES: usize = 128 * 1024;
/// Default maximum age in seconds (7 days).
pub const DEFAULT_MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60;

/// Maximum number of configured app/MIME/marker exclusions kept.
pub const MAX_POLICY_ENTRIES: usize = 64;
/// Maximum length of one configured exclusion or sensitive marker.
pub const MAX_POLICY_ENTRY_CHARS: usize = 128;
/// Maximum length of a sanitized source application name.
pub const MAX_SOURCE_APP_CHARS: usize = 64;

const MIN_MAX_AGE_SECS: u64 = 1;
const MAX_MAX_AGE_SECS: u64 = 365 * 24 * 60 * 60;

/// The clipboard formats this build models.
///
/// `Image` exists so the adapter and policy can report it explicitly; image
/// capture is not implemented and `Image` snapshots are rejected with
/// [`crate::filter::CaptureRejection::ImageUnsupported`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardFormat {
    /// `CF_UNICODETEXT` on Windows.
    Text,
    /// `CF_HDROP` on Windows.
    FileList,
    /// `CF_DIB`/`CF_DIBV5`; modelled but unsupported for capture.
    Image,
}

impl ClipboardFormat {
    /// Stable lowercase label for diagnostics and UI.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::FileList => "file-list",
            Self::Image => "image",
        }
    }
}

impl fmt::Display for ClipboardFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// How much is known about which application produced a clipboard item.
///
/// Windows can report the clipboard owner window while the clipboard is open,
/// but that is evidence of ownership, not proof that a particular application
/// copied the bytes. The adapter therefore reports at most `Probable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceConfidence {
    /// The caller supplied a verified source identity.
    Confirmed,
    /// A source was observed, but the attribution is best-effort (for example
    /// the Windows clipboard owner process).
    Probable,
    /// No attribution is available. Never guess.
    Unknown,
}

impl SourceConfidence {
    /// Stable lowercase label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Probable => "probable",
            Self::Unknown => "unknown",
        }
    }
}

/// A bounded, sanitized application identifier.
///
/// Only a process image stem such as `Code` or `explorer` is representable.
/// Full paths, command lines and window titles are rejected because they can
/// contain user content.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SourceApp {
    name: String,
}

impl SourceApp {
    /// Sanitize and construct an application identity.
    ///
    /// Accepts 1..=64 bytes of ASCII alphanumeric, `.`, `_` or `-`. Anything
    /// else, including path separators, whitespace and non-ASCII text, returns
    /// `None` rather than being coerced.
    #[must_use]
    pub fn new(candidate: &str) -> Option<Self> {
        let name = candidate.trim();
        let length = name.len();
        if !(1..=MAX_SOURCE_APP_CHARS).contains(&length) {
            return None;
        }
        if !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return None;
        }
        Some(Self {
            name: name.to_owned(),
        })
    }

    /// The sanitized process image stem.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Debug for SourceApp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SourceApp")
            .field(&self.name)
            .finish()
    }
}

impl TryFrom<String> for SourceApp {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
            .ok_or_else(|| "source application name is not a bounded identifier".into())
    }
}

impl From<SourceApp> for String {
    fn from(value: SourceApp) -> Self {
        value.name
    }
}

/// Non-sensitive shape information about one item, safe to persist and display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemSummary {
    /// A text item with a character count (not its content).
    Text {
        /// Number of Unicode scalar values.
        characters: usize,
    },
    /// A file list with a file count (not the paths).
    FileList {
        /// Number of paths.
        files: usize,
    },
    /// An image placeholder; never persisted by this build.
    Image,
}

impl ItemSummary {
    /// Human-readable summary that contains no payload.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Text { characters } => format!("{characters} characters"),
            Self::FileList { files } => format!("{files} files"),
            Self::Image => "image (unsupported)".to_owned(),
        }
    }
}

/// Persisted, non-sensitive metadata for one clipboard item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipboardItem {
    /// Stable, monotonically assigned identifier. Sealed payload aliases are
    /// derived from this value.
    pub id: u64,
    /// Captured format.
    pub format: ClipboardFormat,
    /// Payload byte length as observed before encryption.
    pub bytes: usize,
    /// Caller-supplied capture time in seconds. Used only for ordering and age.
    pub captured_at: u64,
    /// Windows clipboard sequence number, or `0` when unavailable.
    pub sequence: u64,
    /// Sanitized source application, if attribution was available.
    pub source: Option<SourceApp>,
    /// Confidence in `source`; `Unknown` whenever `source` is `None`.
    pub confidence: SourceConfidence,
    /// Whether the user pinned this item against automatic eviction.
    pub pinned: bool,
    /// Non-sensitive shape summary.
    pub summary: ItemSummary,
}

impl ClipboardItem {
    /// A short, content-free description for UI and diagnostics.
    #[must_use]
    pub fn description(&self) -> String {
        format!(
            "{} {} from {}",
            self.format.label(),
            self.summary.label(),
            self.source
                .as_ref()
                .map_or("an unknown application", SourceApp::name)
        )
    }

    fn is_consistent(&self) -> bool {
        if self.id == 0 || self.bytes == 0 || self.bytes > ABSOLUTE_MAX_ITEM_BYTES {
            return false;
        }
        if self.source.is_none() && self.confidence != SourceConfidence::Unknown {
            return false;
        }
        matches!(
            (self.format, self.summary),
            (ClipboardFormat::Text, ItemSummary::Text { .. })
                | (ClipboardFormat::FileList, ItemSummary::FileList { .. })
                | (ClipboardFormat::Image, ItemSummary::Image)
        )
    }
}

/// User-configured capture policy.
///
/// The policy is supplied by the caller on every store open; it is not persisted
/// in `history.json`. The host must load it from settings so configured
/// exclusions are not silently replaced by defaults after a restart.
///
/// Values are normalized by [`CapturePolicy::normalized`]; constructing the
/// struct directly is allowed, but every consumer that matters calls
/// `normalized` first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturePolicy {
    /// Maximum number of stored items, including pinned items.
    pub max_items: usize,
    /// Maximum total payload bytes of stored items.
    pub max_bytes: usize,
    /// Maximum payload bytes of one item.
    pub max_item_bytes: usize,
    /// Maximum age in seconds. `0` disables age expiry.
    pub max_age_secs: u64,
    /// Case-insensitive process names to exclude.
    pub excluded_apps: Vec<String>,
    /// Case-insensitive MIME type strings to exclude.
    pub excluded_mime_types: Vec<String>,
    /// Formats that must never be captured.
    pub excluded_formats: Vec<ClipboardFormat>,
    /// Case-insensitive literal markers configured by the user as sensitive.
    pub sensitive_markers: Vec<String>,
    /// Capture content that sets `CanUploadToCloudClipboard = 0`. Defaults to
    /// `false`, so a cloud-upload opt-out is also a local-history opt-out.
    pub capture_cloud_excluded: bool,
}

impl Default for CapturePolicy {
    fn default() -> Self {
        Self {
            max_items: DEFAULT_MAX_ITEMS,
            max_bytes: DEFAULT_MAX_BYTES,
            max_item_bytes: DEFAULT_MAX_ITEM_BYTES,
            max_age_secs: DEFAULT_MAX_AGE_SECS,
            excluded_apps: Vec::new(),
            excluded_mime_types: Vec::new(),
            excluded_formats: Vec::new(),
            sensitive_markers: Vec::new(),
            capture_cloud_excluded: false,
        }
    }
}

impl CapturePolicy {
    /// Construct the default policy.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Clamp every numeric field and normalize exclusion lists.
    ///
    /// `max_item_bytes` is capped by `max_bytes`, entry counts are bounded and
    /// duplicate/empty entries are removed. Empty sensitive markers are dropped
    /// because an empty marker would reject all content.
    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.max_items = self.max_items.clamp(1, ABSOLUTE_MAX_ITEMS);
        self.max_bytes = self.max_bytes.clamp(1024, ABSOLUTE_MAX_BYTES);
        self.max_item_bytes = self
            .max_item_bytes
            .clamp(1, ABSOLUTE_MAX_ITEM_BYTES)
            .min(self.max_bytes);
        if self.max_age_secs != 0 {
            self.max_age_secs = self.max_age_secs.clamp(MIN_MAX_AGE_SECS, MAX_MAX_AGE_SECS);
        }
        normalize_list(&mut self.excluded_apps);
        normalize_list(&mut self.excluded_mime_types);
        normalize_list(&mut self.sensitive_markers);
        self.excluded_formats.sort_by_key(|format| format.label());
        self.excluded_formats.dedup();
        self
    }

    /// Whether `app` is excluded by name.
    #[must_use]
    pub fn excludes_app(&self, app: &SourceApp) -> bool {
        self.excluded_apps
            .iter()
            .any(|excluded| excluded.eq_ignore_ascii_case(app.name()))
    }

    /// Whether `mime_type` is excluded.
    #[must_use]
    pub fn excludes_mime_type(&self, mime_type: &str) -> bool {
        self.excluded_mime_types
            .iter()
            .any(|excluded| mime_type_matches(excluded, mime_type))
    }

    /// Whether `format` is excluded.
    #[must_use]
    pub fn excludes_format(&self, format: ClipboardFormat) -> bool {
        self.excluded_formats.contains(&format)
    }
}

fn normalize_list(values: &mut Vec<String>) {
    let mut normalized: Vec<String> = values
        .drain(..)
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty() && value.chars().count() <= MAX_POLICY_ENTRY_CHARS)
        .collect();
    normalized.sort();
    normalized.dedup();
    normalized.truncate(MAX_POLICY_ENTRIES);
    *values = normalized;
}

fn mime_type_matches(excluded: &str, mime_type: &str) -> bool {
    if excluded == "*" {
        return true;
    }
    if let Some(prefix) = excluded.strip_suffix("/*") {
        let Some(head) = mime_type.get(..prefix.len()) else {
            return false;
        };
        return head.eq_ignore_ascii_case(prefix)
            && mime_type.as_bytes().get(prefix.len()) == Some(&b'/');
    }
    excluded.eq_ignore_ascii_case(mime_type)
}

/// The metadata-only capture log.
///
/// Items are kept newest-first. The structure is deliberately cheap to clone so
/// the store can build a candidate state, persist it, then commit it atomically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardHistory {
    policy: CapturePolicy,
    items: Vec<ClipboardItem>,
    total_bytes: usize,
    next_id: u64,
    paused: bool,
    private_mode: bool,
}

impl ClipboardHistory {
    /// Create an empty history with a normalized policy.
    #[must_use]
    pub fn new(policy: CapturePolicy) -> Self {
        Self {
            policy: policy.normalized(),
            items: Vec::new(),
            total_bytes: 0,
            next_id: 1,
            paused: false,
            private_mode: false,
        }
    }

    /// Rebuild a history from persisted metadata.
    ///
    /// Rejects duplicate ids, zero ids, inconsistent summaries, out-of-range
    /// sizes and a `next_id` that could collide.
    pub fn from_stored(
        policy: CapturePolicy,
        next_id: u64,
        items: Vec<ClipboardItem>,
    ) -> Result<Self, HistoryError> {
        if items.len() > ABSOLUTE_MAX_ITEMS {
            return Err(HistoryError::Corrupt("too many stored items"));
        }
        let mut seen = Vec::with_capacity(items.len());
        let mut total_bytes = 0usize;
        let mut max_id = 0u64;
        for item in &items {
            if !item.is_consistent() {
                return Err(HistoryError::Corrupt("inconsistent item metadata"));
            }
            if seen.contains(&item.id) {
                return Err(HistoryError::Corrupt("duplicate item identifier"));
            }
            seen.push(item.id);
            max_id = max_id.max(item.id);
            total_bytes = total_bytes
                .checked_add(item.bytes)
                .ok_or(HistoryError::Corrupt("item byte total overflowed"))?;
        }
        if next_id <= max_id {
            return Err(HistoryError::Corrupt(
                "next identifier would collide with a stored item",
            ));
        }
        if next_id == u64::MAX {
            return Err(HistoryError::Corrupt("item identifier space is exhausted"));
        }
        let mut items = items;
        sort_items(&mut items);
        Ok(Self {
            policy: policy.normalized(),
            items,
            total_bytes,
            next_id,
            paused: false,
            private_mode: false,
        })
    }

    /// Current policy.
    #[must_use]
    pub fn policy(&self) -> &CapturePolicy {
        &self.policy
    }

    /// Replace the policy and evict entries that no longer fit.
    ///
    /// Returns evicted entries oldest-first. Pinned entries are retained even if
    /// their presence keeps a quota exceeded.
    pub fn set_policy(&mut self, policy: CapturePolicy) -> Vec<ClipboardItem> {
        self.policy = policy.normalized();
        self.enforce_capacity()
    }

    /// Stored items, newest-first.
    #[must_use]
    pub fn items(&self) -> &[ClipboardItem] {
        &self.items
    }

    /// Number of stored items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether no items are stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Total payload bytes represented by the metadata.
    #[must_use]
    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    /// Identifier that will be assigned to the next item.
    #[must_use]
    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    /// Recover the identifier counter from a persisted manifest when the
    /// metadata file is missing.
    ///
    /// `u64::MAX` is accepted here and causes the next insert to fail with
    /// [`HistoryError::IdExhausted`] rather than reuse an identifier.
    pub(crate) fn reset_next_id(&mut self, next_id: u64) {
        self.next_id = next_id.max(1);
    }

    /// Whether capture is paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Pause capture. Stored items are unaffected.
    ///
    /// This is session state: it is deliberately not written to `history.json`
    /// and resets when the process restarts. The host must persist the user's
    /// pause choice in settings and re-apply it after
    /// [`crate::store::ClipboardStore::open`].
    pub fn pause(&mut self) {
        self.paused = true;
    }

    /// Resume capture.
    pub fn resume(&mut self) {
        self.paused = false;
    }

    /// Whether private mode is enabled.
    #[must_use]
    pub fn is_private_mode(&self) -> bool {
        self.private_mode
    }

    /// Enable or disable private mode.
    ///
    /// Session-only, like [`ClipboardHistory::pause`]. Persist the choice in
    /// settings and re-apply it after opening a store.
    pub fn set_private_mode(&mut self, enabled: bool) {
        self.private_mode = enabled;
    }

    /// Look up one item.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<&ClipboardItem> {
        self.items.iter().find(|item| item.id == id)
    }

    /// Insert a draft item, assigning an identifier and enforcing quotas.
    ///
    /// Work happens on a local candidate and is committed only on success, so a
    /// rejection leaves the history unchanged. `evicted` contains age-expired
    /// and over-quota entries oldest-first.
    pub fn insert(&mut self, draft: ItemDraft, now: u64) -> Result<InsertedItems, HistoryError> {
        if self.next_id == u64::MAX {
            return Err(HistoryError::IdExhausted);
        }
        if draft.bytes == 0 || draft.bytes > self.policy.max_item_bytes {
            return Err(HistoryError::InvalidItem(
                "item is empty or exceeds the per-item limit",
            ));
        }
        if draft.sequence != 0
            && self
                .items
                .iter()
                .any(|item| item.sequence == draft.sequence)
        {
            return Err(HistoryError::DuplicateSequence);
        }

        let mut items = self.items.clone();
        let mut total_bytes = self.total_bytes;
        let mut evicted = Vec::new();

        // Age expiry first, then quota. Both are oldest-first.
        if self.policy.max_age_secs != 0 {
            let mut index = 0;
            while index < items.len() {
                let age = now.saturating_sub(items[index].captured_at);
                if !items[index].pinned && age > self.policy.max_age_secs {
                    let item = items.remove(index);
                    total_bytes = total_bytes.saturating_sub(item.bytes);
                    evicted.push(item);
                } else {
                    index += 1;
                }
            }
        }

        let id = self.next_id;
        let item = ClipboardItem {
            id,
            format: draft.format,
            bytes: draft.bytes,
            captured_at: draft.captured_at,
            sequence: draft.sequence,
            source: draft.source,
            confidence: draft.confidence,
            pinned: false,
            summary: draft.summary,
        };
        if !item.is_consistent() {
            return Err(HistoryError::InvalidItem("item metadata is inconsistent"));
        }
        total_bytes = total_bytes
            .checked_add(item.bytes)
            .ok_or(HistoryError::InvalidItem("item byte total overflowed"))?;
        items.push(item.clone());
        sort_items(&mut items);

        let mut rejected_new_item = false;
        while items.len() > self.policy.max_items || total_bytes > self.policy.max_bytes {
            let Some(index) = oldest_unpinned_index(&items) else {
                rejected_new_item = true;
                break;
            };
            let removed = items.remove(index);
            total_bytes = total_bytes.saturating_sub(removed.bytes);
            evicted.push(removed);
        }

        if rejected_new_item {
            // The candidate never becomes visible. Pinned items already stored
            // keep the quota exceeded, so the new item is rejected.
            return Err(HistoryError::QuotaExhausted);
        }
        if !items.iter().any(|candidate| candidate.id == id) {
            return Err(HistoryError::QuotaExhausted);
        }

        self.items = items;
        self.total_bytes = total_bytes;
        self.next_id += 1;
        Ok(InsertedItems { item, evicted })
    }

    /// Pin or unpin an item. Returns `false` when the id is unknown.
    pub fn set_pinned(&mut self, id: u64, pinned: bool) -> bool {
        let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        item.pinned = pinned;
        true
    }

    /// Remove one item, pinned or not.
    pub fn remove(&mut self, id: u64) -> Option<ClipboardItem> {
        let index = self.items.iter().position(|item| item.id == id)?;
        let item = self.items.remove(index);
        self.total_bytes = self.total_bytes.saturating_sub(item.bytes);
        Some(item)
    }

    /// Remove every item, pinned or not.
    pub fn clear(&mut self) -> Vec<ClipboardItem> {
        self.total_bytes = 0;
        std::mem::take(&mut self.items)
    }

    /// Remove only unpinned items.
    pub fn clear_unpinned(&mut self) -> Vec<ClipboardItem> {
        let mut removed = Vec::new();
        let mut index = 0;
        while index < self.items.len() {
            if self.items[index].pinned {
                index += 1;
            } else {
                let item = self.items.remove(index);
                self.total_bytes = self.total_bytes.saturating_sub(item.bytes);
                removed.push(item);
            }
        }
        removed
    }

    /// Prune age-expired unpinned items and return them oldest-first.
    pub fn prune_expired(&mut self, now: u64) -> Vec<ClipboardItem> {
        if self.policy.max_age_secs == 0 {
            return Vec::new();
        }
        let mut removed = Vec::new();
        let mut index = 0;
        while index < self.items.len() {
            let age = now.saturating_sub(self.items[index].captured_at);
            if !self.items[index].pinned && age > self.policy.max_age_secs {
                let item = self.items.remove(index);
                self.total_bytes = self.total_bytes.saturating_sub(item.bytes);
                removed.push(item);
            } else {
                index += 1;
            }
        }
        removed
    }

    /// Evict over-quota and age-expired items. Returns evicted items
    /// oldest-first. Pinned items are retained.
    pub fn enforce_quotas(&mut self, now: u64) -> Vec<ClipboardItem> {
        let mut evicted = self.prune_expired(now);
        evicted.extend(self.enforce_capacity());
        evicted
    }

    /// Evict only count/byte overages. Age is not considered, so a policy update
    /// never silently expires content.
    fn enforce_capacity(&mut self) -> Vec<ClipboardItem> {
        let mut evicted = Vec::new();
        while self.items.len() > self.policy.max_items || self.total_bytes > self.policy.max_bytes {
            let Some(index) = oldest_unpinned_index(&self.items) else {
                break;
            };
            let removed = self.items.remove(index);
            self.total_bytes = self.total_bytes.saturating_sub(removed.bytes);
            evicted.push(removed);
        }
        evicted
    }
}

fn sort_items(items: &mut [ClipboardItem]) {
    items.sort_by(|left, right| {
        right
            .captured_at
            .cmp(&left.captured_at)
            .then_with(|| right.id.cmp(&left.id))
    });
}

fn oldest_unpinned_index(items: &[ClipboardItem]) -> Option<usize> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| !item.pinned)
        .min_by_key(|(_, item)| (item.captured_at, item.id))
        .map(|(index, _)| index)
}

/// A draft item supplied by the store after filtering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemDraft {
    /// Captured format.
    pub format: ClipboardFormat,
    /// Payload byte length.
    pub bytes: usize,
    /// Capture time in seconds.
    pub captured_at: u64,
    /// Clipboard sequence number or `0`.
    pub sequence: u64,
    /// Sanitized source application.
    pub source: Option<SourceApp>,
    /// Attribution confidence.
    pub confidence: SourceConfidence,
    /// Non-sensitive summary.
    pub summary: ItemSummary,
}

/// Result of [`ClipboardHistory::insert`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsertedItems {
    /// Newly stored item.
    pub item: ClipboardItem,
    /// Entries evicted to make room, oldest-first.
    pub evicted: Vec<ClipboardItem>,
}

/// Why a history mutation was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryError {
    /// The same clipboard sequence number is already stored.
    DuplicateSequence,
    /// Pinned items alone keep the quota exceeded.
    QuotaExhausted,
    /// The identifier space is exhausted; no further item can be stored.
    IdExhausted,
    /// The draft is empty or otherwise invalid.
    InvalidItem(&'static str),
    /// Persisted metadata failed validation.
    Corrupt(&'static str),
}

impl fmt::Display for HistoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateSequence => formatter.write_str("clipboard sequence already captured"),
            Self::QuotaExhausted => {
                formatter.write_str("clipboard quota is exhausted by pinned items")
            }
            Self::IdExhausted => formatter.write_str("clipboard item identifiers are exhausted"),
            Self::InvalidItem(reason) => write!(formatter, "invalid clipboard item: {reason}"),
            Self::Corrupt(reason) => write!(formatter, "corrupt clipboard metadata: {reason}"),
        }
    }
}

impl std::error::Error for HistoryError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(id: u64, bytes: usize, captured_at: u64) -> ItemDraft {
        ItemDraft {
            format: ClipboardFormat::Text,
            bytes,
            captured_at,
            sequence: id,
            source: None,
            confidence: SourceConfidence::Unknown,
            summary: ItemSummary::Text {
                characters: bytes / 2,
            },
        }
    }

    fn policy(max_items: usize, max_bytes: usize, max_item_bytes: usize) -> CapturePolicy {
        CapturePolicy {
            max_items,
            max_bytes,
            max_item_bytes,
            ..CapturePolicy::default()
        }
        .normalized()
    }

    #[test]
    fn quota_evicts_oldest_first_and_is_deterministic() {
        let mut history = ClipboardHistory::new(policy(3, 100, 20));
        for id in 1..=4 {
            history.insert(draft(id, 10, id), id).expect("insert");
        }
        assert_eq!(
            history
                .items()
                .iter()
                .map(|item| item.id)
                .collect::<Vec<_>>(),
            vec![4, 3, 2]
        );
        assert_eq!(history.total_bytes(), 30);

        let inserted = history.insert(draft(5, 10, 5), 5).expect("insert");
        assert_eq!(inserted.evicted.len(), 1);
        assert_eq!(inserted.evicted[0].id, 2);
        assert_eq!(
            history
                .items()
                .iter()
                .map(|item| item.id)
                .collect::<Vec<_>>(),
            vec![5, 4, 3]
        );
    }

    #[test]
    fn pinned_items_survive_quota_and_age_but_remain_removable() {
        let mut history = ClipboardHistory::new(policy(2, 100, 20));
        history.insert(draft(1, 10, 100), 100).expect("insert");
        history.insert(draft(2, 10, 101), 101).expect("insert");
        assert!(history.set_pinned(1, true));

        let inserted = history.insert(draft(3, 10, 102), 102).expect("insert");
        assert_eq!(inserted.evicted.len(), 1);
        assert_eq!(inserted.evicted[0].id, 2);
        assert!(history.get(1).expect("pinned item").pinned);

        let expired = history.enforce_quotas(100_000);
        assert!(expired.is_empty());
        assert!(history.get(1).is_some());

        let removed = history.remove(1).expect("pinned item is removable");
        assert_eq!(removed.id, 1);
        assert!(history.get(1).is_none());
    }

    #[test]
    fn pinned_items_alone_exhaust_the_quota_without_deleting_them() {
        let mut history = ClipboardHistory::new(policy(1, 100, 20));
        history.insert(draft(1, 10, 1), 1).expect("insert");
        history.set_pinned(1, true);
        let before = history.clone();

        let error = history.insert(draft(2, 10, 2), 2).expect_err("quota");
        assert_eq!(error, HistoryError::QuotaExhausted);
        assert_eq!(history, before);
    }

    #[test]
    fn age_pruning_keeps_pinned_items() {
        let mut history = ClipboardHistory::new(CapturePolicy {
            max_age_secs: 10,
            ..CapturePolicy::default()
        });
        history.insert(draft(1, 10, 0), 0).expect("insert");
        history.insert(draft(2, 10, 5), 5).expect("insert");
        history.set_pinned(1, true);

        let evicted = history.prune_expired(100);
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0].id, 2);
        assert!(history.get(1).is_some());
    }

    #[test]
    fn duplicate_sequences_and_oversized_items_are_rejected_without_mutation() {
        let mut history = ClipboardHistory::new(policy(10, 1000, 20));
        history.insert(draft(1, 10, 1), 1).expect("insert");
        let before = history.clone();

        let mut duplicate = draft(2, 10, 2);
        duplicate.sequence = 1;
        assert_eq!(
            history.insert(duplicate, 2),
            Err(HistoryError::DuplicateSequence)
        );
        assert_eq!(
            history.insert(draft(3, 21, 3), 3),
            Err(HistoryError::InvalidItem(
                "item is empty or exceeds the per-item limit"
            ))
        );
        assert_eq!(history, before);
    }

    #[test]
    fn duplicate_sequence_from_another_item_is_rejected() {
        let mut history = ClipboardHistory::new(policy(10, 1000, 20));
        history.insert(draft(1, 10, 1), 1).expect("insert");
        let mut second = draft(2, 10, 2);
        second.sequence = 1;
        assert_eq!(
            history.insert(second, 2),
            Err(HistoryError::DuplicateSequence)
        );
    }

    #[test]
    fn policy_change_evicts_unpinned_items_but_keeps_pinned() {
        let mut history = ClipboardHistory::new(policy(10, 1000, 20));
        history.insert(draft(1, 10, 1), 1).expect("insert");
        history.insert(draft(2, 10, 2), 2).expect("insert");
        history.set_pinned(1, true);

        let evicted = history.set_policy(policy(1, 1000, 20));
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0].id, 2);
        assert!(history.get(1).is_some());
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn clear_removes_pinned_items_and_clear_unpinned_keeps_them() {
        let mut history = ClipboardHistory::new(policy(10, 1000, 20));
        history.insert(draft(1, 10, 1), 1).expect("insert");
        history.insert(draft(2, 10, 2), 2).expect("insert");
        history.set_pinned(1, true);

        let removed = history.clear_unpinned();
        assert_eq!(removed.len(), 1);
        assert_eq!(history.len(), 1);

        let removed = history.clear();
        assert_eq!(removed.len(), 1);
        assert!(history.is_empty());
        assert_eq!(history.total_bytes(), 0);
    }

    #[test]
    fn identifier_saturation_is_rejected_instead_of_reusing_ids() {
        assert!(
            ClipboardHistory::from_stored(CapturePolicy::default(), u64::MAX, Vec::new()).is_err()
        );
        let mut history = ClipboardHistory::new(policy(10, 1000, 20));
        history.reset_next_id(u64::MAX);
        assert_eq!(
            history.insert(draft(1, 10, 1), 1),
            Err(HistoryError::IdExhausted)
        );
        assert!(history.is_empty());
    }

    #[test]
    fn pause_and_private_mode_are_independent_session_flags() {
        let mut history = ClipboardHistory::new(CapturePolicy::default());
        assert!(!history.is_paused());
        assert!(!history.is_private_mode());
        history.pause();
        assert!(history.is_paused());
        history.set_private_mode(true);
        assert!(history.is_private_mode());
        history.resume();
        assert!(!history.is_paused());
        assert!(history.is_private_mode());
    }

    #[test]
    fn policy_normalization_bounds_and_deduplicates_entries() {
        let policy = CapturePolicy {
            max_items: 0,
            max_bytes: 1,
            max_item_bytes: usize::MAX,
            max_age_secs: 1,
            excluded_apps: vec!["  B ".into(), "a".into(), "".into(), "b".into()],
            sensitive_markers: vec!["".into()],
            ..CapturePolicy::default()
        }
        .normalized();
        assert_eq!(policy.max_items, 1);
        assert_eq!(policy.max_bytes, 1024);
        assert_eq!(policy.max_item_bytes, 1024);
        assert_eq!(policy.excluded_apps, vec!["a", "b"]);
        assert!(policy.sensitive_markers.is_empty());
    }

    #[test]
    fn stored_metadata_is_validated() {
        let item = ClipboardItem {
            id: 1,
            format: ClipboardFormat::Text,
            bytes: 10,
            captured_at: 1,
            sequence: 1,
            source: None,
            confidence: SourceConfidence::Unknown,
            pinned: false,
            summary: ItemSummary::Text { characters: 5 },
        };
        assert!(
            ClipboardHistory::from_stored(CapturePolicy::default(), 2, vec![item.clone()]).is_ok()
        );
        assert!(
            ClipboardHistory::from_stored(CapturePolicy::default(), 1, vec![item.clone()]).is_err()
        );
        let mut duplicate = item.clone();
        duplicate.id = 2;
        assert!(
            ClipboardHistory::from_stored(
                CapturePolicy::default(),
                3,
                vec![item.clone(), item.clone()]
            )
            .is_err()
        );
        assert!(
            ClipboardHistory::from_stored(CapturePolicy::default(), 3, vec![duplicate, item])
                .is_ok()
        );
    }

    #[test]
    fn source_app_rejects_paths_and_titles() {
        assert_eq!(SourceApp::new("Code").expect("valid").name(), "Code");
        assert!(SourceApp::new("C:\\Windows\\notepad.exe").is_none());
        assert!(SourceApp::new("a window title").is_none());
        assert!(SourceApp::new("").is_none());
        assert!(SourceApp::new(&"a".repeat(65)).is_none());
    }
}
