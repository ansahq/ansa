//! Sealed clipboard history storage.
//!
//! Layout under the store root:
//!
//! ```text
//! <store>/.lock                   cross-process single-instance lock
//! <store>/manifest.json           clipboard-owned alias manifest
//! <store>/history.json            bounded, non-sensitive metadata
//! <vault>/secrets/ansa-clip-<store>-<id>.sealed   DPAPI envelope
//! ```
//!
//! The metadata file never contains clipboard text or paths. Payloads are
//! encoded into a small binary envelope, sealed through [`SealBackend`] and
//! written only as ciphertext. [`ClipboardStore::capture`] filters before it
//! seals anything, so rejected content leaves no file, metadata row or preview
//! cache entry.
//!
//! ## Namespace and cleanup
//!
//! Aliases carry a random per-store identifier. `manifest.json` records the
//! aliases this store wrote, before the sealed payload exists. Automatic
//! cleanup only deletes aliases listed in that manifest, unreferenced by
//! non-empty metadata, and carrying the current store identifier. A missing or
//! empty history never triggers destructive cleanup; only an explicit
//! [`ClipboardStore::clear_all`] sweeps every manifest entry.
//!
//! ## Legacy migration
//!
//! A store written before alias namespacing has `history.json` without
//! `store_id` and sealed payloads under `ansa-clip-<id>`, with no manifest.
//! Opening it performs an explicit migration: every legacy payload is decrypted
//! in a preflight pass, new namespaced aliases are written, upgraded metadata is
//! committed, and only then are the old aliases deleted. Any decryption failure
//! aborts before writes with [`ClipboardError::LegacyLayout`] and leaves the old
//! store intact. A metadata/manifest state that cannot be identified safely is
//! also refused rather than presented as usable.
//!
//! ## Single instance
//!
//! Opening a store takes a non-blocking cross-process lock on `<store>/.lock`.
//! A second open returns [`ClipboardError::AlreadyLocked`]. Windows holds the
//! lock through a no-share file handle, so a crash releases it; the lock file
//! remains on disk by design. Two concurrent stores cannot interleave captures
//! or cleanup.
//!
//! ## Locking
//!
//! [`ClipboardStore::lock`] stops capture and decryption and wipes the bounded
//! preview cache. It also locks the underlying vault. An external vault lock is
//! observed on the next operation and treated the same way. Unlocking rebuilds
//! the metadata index from disk; it never replays captures, so entries are not
//! duplicated.
//!
//! Pause and private mode are **session state**: they are not written to disk
//! and reset when the store is reopened. The host must persist the user's
//! choices in settings and re-apply them. The capture policy is likewise
//! supplied by the caller on each open.
//!
//! ## Honest limits
//!
//! DPAPI is user-bound, not passphrase-bound. The lock is a session gate and
//! cannot stop another process running as the same Windows user from opening
//! the sealed files. Wiping in-memory previews is best-effort: the allocator
//! and the operating system may retain old pages. Payload deletion is
//! authoritative in metadata first; a failed ciphertext delete leaves a
//! manifest tombstone for a later non-empty cleanup or explicit clear.

use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::hash::{BuildHasher, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use ansa_vault::Vault;
use serde::{Deserialize, Serialize};

use crate::content::{ClipboardContent, wipe_bytes, wipe_string};
use crate::error::ClipboardError;
use crate::filter::{CaptureGate, CaptureRejection, ClipboardSnapshot, FilterOutcome, evaluate};
use crate::lock::StoreLock;
use crate::model::{
    ABSOLUTE_MAX_ITEMS, CapturePolicy, ClipboardHistory, ClipboardItem, HistoryError, ItemDraft,
    SourceConfidence,
};

const METADATA_FILE: &str = "history.json";
const MANIFEST_FILE: &str = "manifest.json";
const METADATA_VERSION: u32 = 1;
const MANIFEST_VERSION: u32 = 1;
const MAX_METADATA_BYTES: usize = 4 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_MANIFEST_ALIASES: usize = ABSOLUTE_MAX_ITEMS + 512;
const STORE_ID_CHARS: usize = 32;
const TEMP_PREFIX: &str = ".tmp-history-";
const ALIAS_PREFIX: &str = "ansa-clip-";
const ENVELOPE_MAGIC: &[u8; 4] = b"ACB1";
const ENVELOPE_VERSION: u8 = 1;
const FORMAT_TEXT: u8 = 1;
const FORMAT_FILE_LIST: u8 = 2;
const MAX_ENVELOPE_BYTES: usize = 1000 * 1024;
const MAX_FILE_ENTRIES: usize = 65_536;
const MAX_PREVIEW_ENTRIES: usize = 32;
const MAX_PREVIEW_BYTES: usize = 16 * 1024;
const MAX_PREVIEW_CHARS: usize = 512;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Storage for sealed payloads.
///
/// [`VaultBackend`] is the production implementation. [`MemorySealBackend`] is
/// an explicit test double and must not be used for real data.
pub trait SealBackend {
    /// Seal `plaintext` under `alias`.
    fn seal(&self, alias: &str, plaintext: &[u8]) -> Result<(), ClipboardError>;
    /// Open the sealed payload stored under `alias`.
    fn open(&self, alias: &str) -> Result<Vec<u8>, ClipboardError>;
    /// Delete the payload under `alias`; returns whether one existed.
    fn delete(&self, alias: &str) -> Result<bool, ClipboardError>;
    /// Lock the backend for this session.
    fn lock(&self);
    /// Unlock the backend for this session.
    fn unlock(&self);
    /// Whether sealed operations currently fail.
    fn is_locked(&self) -> bool;
}

/// Production backend that delegates to [`ansa_vault::Vault`].
#[derive(Debug)]
pub struct VaultBackend {
    vault: Vault,
}

impl VaultBackend {
    /// Wrap an opened vault.
    #[must_use]
    pub fn new(vault: Vault) -> Self {
        Self { vault }
    }
}

impl SealBackend for VaultBackend {
    fn seal(&self, alias: &str, plaintext: &[u8]) -> Result<(), ClipboardError> {
        self.vault.put_secret(alias, plaintext)?;
        Ok(())
    }

    fn open(&self, alias: &str) -> Result<Vec<u8>, ClipboardError> {
        Ok(self.vault.get_secret(alias)?)
    }

    fn delete(&self, alias: &str) -> Result<bool, ClipboardError> {
        Ok(self.vault.delete_secret(alias)?)
    }

    fn lock(&self) {
        self.vault.lock();
    }

    fn unlock(&self) {
        self.vault.unlock();
    }

    fn is_locked(&self) -> bool {
        self.vault.is_locked()
    }
}

#[derive(Default)]
struct MemoryState {
    secrets: std::collections::BTreeMap<String, Vec<u8>>,
    locked: bool,
}

/// In-memory test double.
///
/// This type stores plaintext in memory and performs **no encryption**. It is
/// hidden from the public documentation and intended only for tests. It is
/// never selected by [`ClipboardStore::open`]. Do not use it for real data.
#[doc(hidden)]
#[derive(Clone, Default)]
pub struct MemorySealBackend {
    state: Arc<Mutex<MemoryState>>,
}

impl fmt::Debug for MemorySealBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MemorySealBackend")
            .field("entries", &self.len())
            .field("locked", &self.is_locked())
            .finish()
    }
}

impl MemorySealBackend {
    /// Create an empty, unlocked test backend.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of stored aliases.
    #[must_use]
    pub fn len(&self) -> usize {
        self.state.lock().map_or(0, |state| state.secrets.len())
    }

    /// Whether no payload is stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether `alias` has a stored payload.
    #[must_use]
    pub fn contains(&self, alias: &str) -> bool {
        self.state
            .lock()
            .is_ok_and(|state| state.secrets.contains_key(alias))
    }

    /// Sorted aliases currently stored.
    #[must_use]
    pub fn aliases(&self) -> Vec<String> {
        self.state.lock().map_or_else(
            |_| Vec::new(),
            |state| state.secrets.keys().cloned().collect(),
        )
    }

    fn with_state<T>(
        &self,
        operation: &'static str,
        action: impl FnOnce(&mut MemoryState) -> T,
    ) -> Result<T, ClipboardError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ClipboardError::Backend { operation })?;
        Ok(action(&mut state))
    }
}

impl SealBackend for MemorySealBackend {
    fn seal(&self, alias: &str, plaintext: &[u8]) -> Result<(), ClipboardError> {
        self.with_state("seal", |state| {
            if state.locked {
                return Err(ClipboardError::Locked);
            }
            state.secrets.insert(alias.to_owned(), plaintext.to_vec());
            Ok(())
        })?
    }

    fn open(&self, alias: &str) -> Result<Vec<u8>, ClipboardError> {
        self.with_state("open", |state| {
            if state.locked {
                return Err(ClipboardError::Locked);
            }
            state
                .secrets
                .get(alias)
                .cloned()
                .ok_or(ClipboardError::ItemNotFound)
        })?
    }

    fn delete(&self, alias: &str) -> Result<bool, ClipboardError> {
        self.with_state("delete", |state| {
            if state.locked {
                return Err(ClipboardError::Locked);
            }
            Ok(state.secrets.remove(alias).is_some())
        })?
    }

    fn lock(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.locked = true;
        }
    }

    fn unlock(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.locked = false;
        }
    }

    fn is_locked(&self) -> bool {
        self.state.lock().is_ok_and(|state| state.locked)
    }
}

/// Non-sensitive store state for status UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreState {
    /// Whether capture and decryption are locked.
    pub locked: bool,
    /// Whether capture is paused.
    pub paused: bool,
    /// Whether private mode is enabled.
    pub private_mode: bool,
    /// Number of stored items.
    pub items: usize,
    /// Total represented payload bytes.
    pub bytes: usize,
}

/// Result of a successful capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureRecord {
    /// Stored metadata.
    pub item: ClipboardItem,
    /// Identifiers evicted from metadata and storage.
    pub evicted: Vec<u64>,
}

/// Outcome of one capture attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureReport {
    /// The item was sealed and stored.
    Stored(CaptureRecord),
    /// Capture was deliberately skipped; the rejection contains no content.
    Skipped(CaptureRejection),
    /// The same clipboard sequence was already stored.
    Duplicate,
}

impl CaptureReport {
    /// Whether an item was stored.
    #[must_use]
    pub fn is_stored(&self) -> bool {
        matches!(self, Self::Stored(_))
    }
}

impl fmt::Display for CaptureReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stored(record) => write!(formatter, "stored clipboard item {}", record.item.id),
            Self::Skipped(reason) => write!(formatter, "skipped clipboard capture: {reason}"),
            Self::Duplicate => formatter.write_str("clipboard sequence was already captured"),
        }
    }
}

/// Explicitly decrypted content returned by [`ClipboardStore::decrypt_item`].
///
/// `Debug` prints metadata only. Dropping the value overwrites the content
/// buffers on a best-effort basis.
pub struct DecryptedItem {
    /// Non-sensitive metadata.
    pub item: ClipboardItem,
    /// Decrypted content.
    pub content: ClipboardContent,
}

impl fmt::Debug for DecryptedItem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DecryptedItem")
            .field("item", &self.item)
            .field("content", &self.content)
            .finish()
    }
}

/// Bounded preview cache cleared on lock.
#[derive(Default)]
struct PreviewCache {
    entries: Vec<PreviewEntry>,
    bytes: usize,
}

struct PreviewEntry {
    id: u64,
    text: String,
}

impl PreviewCache {
    fn insert(&mut self, id: u64, text: String) {
        self.remove(id);
        while !self.entries.is_empty()
            && (self.entries.len() >= MAX_PREVIEW_ENTRIES
                || self.bytes.saturating_add(text.len()) > MAX_PREVIEW_BYTES)
        {
            self.pop_oldest();
        }
        self.bytes = self.bytes.saturating_add(text.len());
        self.entries.push(PreviewEntry { id, text });
    }

    fn remove(&mut self, id: u64) {
        if let Some(index) = self.entries.iter().position(|entry| entry.id == id) {
            let mut entry = self.entries.remove(index);
            self.bytes = self.bytes.saturating_sub(entry.text.len());
            wipe_string(&mut entry.text);
        }
    }

    fn pop_oldest(&mut self) {
        if !self.entries.is_empty() {
            let mut entry = self.entries.remove(0);
            self.bytes = self.bytes.saturating_sub(entry.text.len());
            wipe_string(&mut entry.text);
        }
    }

    fn clear(&mut self) {
        for entry in &mut self.entries {
            wipe_string(&mut entry.text);
        }
        self.entries.clear();
        self.bytes = 0;
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

impl Drop for PreviewCache {
    fn drop(&mut self) {
        self.clear();
    }
}

/// Bounded ownership manifest for clipboard-sealed aliases.
///
/// The manifest is the only source of truth for automatic orphan deletion.
/// An alias is deleted automatically only when it is listed here, carries this
/// store's identifier and is unreferenced by non-empty metadata. A missing or
/// empty history suppresses automatic deletion entirely.
#[derive(Debug, Clone)]
struct Manifest {
    store_id: String,
    aliases: BTreeSet<String>,
    present: bool,
}

impl Manifest {
    fn empty_with_new_id() -> Self {
        Self {
            store_id: generate_store_id(),
            aliases: BTreeSet::new(),
            present: false,
        }
    }

    fn max_item_id(&self) -> Option<u64> {
        self.aliases
            .iter()
            .filter_map(|alias| alias_item_id(&self.store_id, alias))
            .max()
    }

    fn insert(&mut self, alias: String, referenced: &BTreeSet<String>) {
        if self.aliases.len() >= MAX_MANIFEST_ALIASES {
            // Preserve live references and drop old tombstones rather than
            // grow without bound. Dropped tombstones stop being candidates for
            // automatic cleanup; they are never deleted unsafely.
            self.aliases.retain(|candidate| {
                referenced.contains(candidate) || !alias_belongs_to(&self.store_id, candidate)
            });
        }
        self.aliases.insert(alias);
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    version: u32,
    store_id: String,
    aliases: Vec<String>,
}

fn generate_store_id() -> String {
    // Two independently seeded `RandomState` hashers provide 128 bits of
    // OS-seeded process randomness. This identifier namespaces clipboard
    // aliases; it is not a secret or an authentication token.
    let mut identifier = String::with_capacity(STORE_ID_CHARS);
    for _ in 0..2 {
        let state = std::collections::hash_map::RandomState::new();
        let mut hasher = state.build_hasher();
        hasher.write_u128(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos()),
        );
        hasher.write_u32(std::process::id());
        identifier.push_str(&format!("{:016x}", hasher.finish()));
    }
    identifier
}

fn is_valid_store_id(store_id: &str) -> bool {
    store_id.len() == STORE_ID_CHARS
        && store_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn alias_prefix(store_id: &str) -> String {
    format!("{ALIAS_PREFIX}{store_id}-")
}

fn legacy_alias_for(id: u64) -> String {
    format!("{ALIAS_PREFIX}{id}")
}

fn alias_belongs_to(store_id: &str, alias: &str) -> bool {
    is_valid_store_id(store_id) && alias_item_id(store_id, alias).is_some()
}

fn alias_item_id(store_id: &str, alias: &str) -> Option<u64> {
    let prefix = alias_prefix(store_id);
    let suffix = alias.strip_prefix(&prefix)?;
    if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let id = suffix.parse::<u64>().ok()?;
    (id > 0).then_some(id)
}

fn load_manifest(root: &Path) -> Result<Manifest, ClipboardError> {
    let path = root.join(MANIFEST_FILE);
    if !path.exists() {
        return Ok(Manifest::empty_with_new_id());
    }
    let bytes = read_bounded(&path, MAX_MANIFEST_BYTES)?;
    let file: ManifestFile =
        serde_json::from_slice(&bytes).map_err(|_| ClipboardError::CorruptManifest {
            reason: "clipboard manifest is not valid JSON",
        })?;
    if file.version != MANIFEST_VERSION {
        return Err(ClipboardError::CorruptManifest {
            reason: "clipboard manifest version is not supported",
        });
    }
    if !is_valid_store_id(&file.store_id) {
        return Err(ClipboardError::CorruptManifest {
            reason: "clipboard manifest store identifier is invalid",
        });
    }
    if file.aliases.len() > MAX_MANIFEST_ALIASES {
        return Err(ClipboardError::CorruptManifest {
            reason: "clipboard manifest has too many aliases",
        });
    }
    let aliases = file
        .aliases
        .into_iter()
        .filter(|alias| alias_belongs_to(&file.store_id, alias))
        .collect();
    Ok(Manifest {
        store_id: file.store_id,
        aliases,
        present: true,
    })
}

/// A filtered, sealed clipboard history.
pub struct ClipboardStore {
    root: PathBuf,
    backend: Box<dyn SealBackend>,
    history: ClipboardHistory,
    previews: PreviewCache,
    locked: bool,
    store_id: String,
    manifest: Manifest,
    metadata_present: bool,
    _lock: StoreLock,
}

impl fmt::Debug for ClipboardStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClipboardStore")
            .field("root", &self.root)
            .field("locked", &self.locked)
            .field("paused", &self.history.is_paused())
            .field("private_mode", &self.history.is_private_mode())
            .field("items", &self.history.len())
            .field("bytes", &self.history.total_bytes())
            .finish_non_exhaustive()
    }
}

impl ClipboardStore {
    /// Open a store backed by an already-open DPAPI vault.
    ///
    /// The caller must pass [`Vault::open`]'s result; this crate never creates
    /// a plaintext fallback.
    pub fn open(
        root: impl AsRef<Path>,
        policy: CapturePolicy,
        vault: Vault,
    ) -> Result<Self, ClipboardError> {
        Self::open_with_backend(root, policy, Box::new(VaultBackend::new(vault)))
    }

    /// Open a store with an explicit sealing backend.
    ///
    /// This is the test/embedding constructor. Production code should use
    /// [`ClipboardStore::open`] so payloads are DPAPI-sealed.
    #[doc(hidden)]
    pub fn open_with_backend(
        root: impl AsRef<Path>,
        policy: CapturePolicy,
        backend: Box<dyn SealBackend>,
    ) -> Result<Self, ClipboardError> {
        let root = root.as_ref();
        if root.exists() && !root.is_dir() {
            return Err(ClipboardError::CorruptHistory {
                reason: "store path exists but is not a directory",
            });
        }
        fs::create_dir_all(root)?;
        let root = fs::canonicalize(root)?;
        let lock = StoreLock::acquire(&root)?;

        let mut manifest = load_manifest(&root)?;
        let loaded = load_history(&root, policy.clone())?;
        let legacy = loaded.metadata_present && loaded.store_id.is_none();
        if legacy && manifest.present {
            return Err(ClipboardError::LegacyLayout {
                reason: "legacy metadata is accompanied by a namespaced manifest",
            });
        }
        let store_id = resolve_store_id(&mut manifest, loaded.store_id.as_deref())?;
        let mut history = loaded.history;
        if !loaded.metadata_present {
            let next_id = manifest
                .max_item_id()
                .map_or(1, |identifier| identifier.saturating_add(1));
            history.reset_next_id(next_id);
        }
        let mut store = Self {
            root,
            backend,
            history,
            previews: PreviewCache::default(),
            locked: false,
            store_id,
            manifest,
            metadata_present: loaded.metadata_present,
            _lock: lock,
        };
        if legacy {
            store.migrate_legacy()?;
        }

        let evicted = store.history.set_policy(policy);
        if !evicted.is_empty() {
            store.persist_history(&store.history)?;
            for item in &evicted {
                let alias = store.alias_for(item.id);
                let legacy_alias = legacy_alias_for(item.id);
                store.manifest.aliases.insert(alias.clone());
                store.manifest.present = true;
                if store.backend.delete(&alias).is_ok() {
                    store.manifest.aliases.remove(&alias);
                }
                let _ = store.backend.delete(&legacy_alias);
            }
            let _ = store.persist_manifest();
        }

        // A metadata file written by a migration but lacking its manifest (for
        // example a crash after the metadata commit) can rebuild ownership from
        // the referenced aliases. This only records aliases; it never deletes.
        if store.metadata_present && !store.manifest.present && !store.history.is_empty() {
            for item in store.history.items() {
                let alias = store.alias_for(item.id);
                if alias_belongs_to(&store.store_id, &alias) {
                    store.manifest.aliases.insert(alias);
                }
            }
            store.manifest.present = true;
            let _ = store.persist_manifest();
        }

        store.cleanup_orphans()?;
        Ok(store)
    }

    /// Store root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Current non-sensitive state.
    #[must_use]
    pub fn state(&self) -> StoreState {
        StoreState {
            locked: self.is_locked(),
            paused: self.history.is_paused(),
            private_mode: self.history.is_private_mode(),
            items: self.history.len(),
            bytes: self.history.total_bytes(),
        }
    }

    /// Stored metadata, newest-first.
    #[must_use]
    pub fn items(&self) -> &[ClipboardItem] {
        self.history.items()
    }

    /// Metadata for one item.
    #[must_use]
    pub fn item(&self, id: u64) -> Option<&ClipboardItem> {
        self.history.get(id)
    }

    /// Number of entries in the bounded preview cache.
    #[must_use]
    pub fn cached_preview_count(&self) -> usize {
        self.previews.len()
    }

    /// Whether capture and decryption are locked.
    #[must_use]
    pub fn is_locked(&self) -> bool {
        self.locked || self.backend.is_locked()
    }

    /// Observe an external vault/OS lock immediately.
    ///
    /// Call this from an OS lock-event handler so the preview cache is wiped
    /// without waiting for the next store operation. Returns whether the store
    /// is now locked.
    pub fn sync_lock_state(&mut self) -> bool {
        self.sync_lock();
        self.locked
    }

    /// Capture a snapshot.
    ///
    /// Filtering, sealing and metadata commitment happen in that order. On any
    /// rejection or error no metadata entry, sealed payload or preview entry is
    /// created.
    pub fn capture(
        &mut self,
        snapshot: ClipboardSnapshot,
        now: u64,
    ) -> Result<CaptureReport, ClipboardError> {
        self.sync_lock();

        let gate = CaptureGate {
            locked: self.locked,
            paused: self.history.is_paused(),
            private_mode: self.history.is_private_mode(),
        };
        if let FilterOutcome::Reject(rejection) = evaluate(&snapshot, self.history.policy(), gate) {
            return Ok(CaptureReport::Skipped(rejection));
        }

        let format = snapshot.format();
        let bytes = snapshot.byte_len();
        let summary = snapshot.content.summary();
        let sequence = snapshot.sequence;
        let source = snapshot.source.clone();
        let confidence = if source.is_some() {
            snapshot.confidence
        } else {
            SourceConfidence::Unknown
        };
        let Some(content) = snapshot.content.into_content() else {
            return Ok(CaptureReport::Skipped(CaptureRejection::ImageUnsupported));
        };

        let mut plaintext = encode_payload(&content);
        // `content` is dropped here and wipes its buffers.
        drop(content);

        if plaintext.len() > MAX_ENVELOPE_BYTES {
            let bytes = plaintext.len();
            wipe_bytes(&mut plaintext);
            return Err(ClipboardError::EnvelopeTooLarge {
                bytes,
                limit: MAX_ENVELOPE_BYTES,
            });
        }

        let mut candidate = self.history.clone();
        let draft = ItemDraft {
            format,
            bytes,
            captured_at: now,
            sequence,
            source,
            confidence,
            summary,
        };
        let inserted = match candidate.insert(draft, now) {
            Ok(inserted) => inserted,
            Err(HistoryError::DuplicateSequence) => {
                wipe_bytes(&mut plaintext);
                return Ok(CaptureReport::Duplicate);
            }
            Err(HistoryError::QuotaExhausted) => {
                wipe_bytes(&mut plaintext);
                return Ok(CaptureReport::Skipped(CaptureRejection::QuotaExhausted));
            }
            Err(error) => {
                wipe_bytes(&mut plaintext);
                return Err(error.into());
            }
        };

        let alias = self.alias_for(inserted.item.id);
        let mut manifest = self.manifest.clone();
        let referenced: BTreeSet<String> = candidate
            .items()
            .iter()
            .map(|item| self.alias_for(item.id))
            .collect();
        manifest.insert(alias.clone(), &referenced);
        manifest.present = true;

        // Record ownership before the sealed payload can exist, so a crash
        // cannot leave a clipboard seal that no manifest claims.
        if let Err(error) = self.persist_manifest_value(&manifest) {
            wipe_bytes(&mut plaintext);
            return Err(error);
        }

        let sealed = self.backend.seal(&alias, &plaintext);
        wipe_bytes(&mut plaintext);
        if let Err(error) = sealed {
            manifest.aliases.remove(&alias);
            let _ = self.persist_manifest_value(&manifest);
            return Err(error);
        }

        if let Err(error) = self.persist_history(&candidate) {
            // Remove the just-written payload so a failed metadata commit does
            // not leave a hidden entry behind. Keep the manifest tombstone if
            // deletion fails; cleanup will retry it later.
            if self.backend.delete(&alias).is_ok() {
                manifest.aliases.remove(&alias);
                let _ = self.persist_manifest_value(&manifest);
            }
            return Err(error);
        }

        self.history = candidate;
        self.manifest = manifest;
        self.metadata_present = true;

        // Metadata is committed first; deleting evicted ciphertext is
        // best-effort. A failed delete stays in the manifest as a tombstone.
        let mut manifest_changed = false;
        for evicted in &inserted.evicted {
            let alias = self.alias_for(evicted.id);
            self.manifest.aliases.insert(alias.clone());
            self.manifest.present = true;
            manifest_changed = true;
            if self.backend.delete(&alias).is_ok() {
                self.manifest.aliases.remove(&alias);
            }
            let _ = self.backend.delete(&legacy_alias_for(evicted.id));
            self.previews.remove(evicted.id);
        }
        if manifest_changed {
            let _ = self.persist_manifest();
        }

        Ok(CaptureReport::Stored(CaptureRecord {
            item: inserted.item,
            evicted: inserted.evicted.into_iter().map(|item| item.id).collect(),
        }))
    }

    /// Explicitly decrypt one item for preview or search.
    pub fn decrypt_item(&mut self, id: u64) -> Result<DecryptedItem, ClipboardError> {
        self.validate_unlocked()?;
        let item = self
            .history
            .get(id)
            .cloned()
            .ok_or(ClipboardError::ItemNotFound)?;
        let mut plaintext = self.backend.open(&self.alias_for(id))?;
        let decoded = decode_payload(&plaintext);
        wipe_bytes(&mut plaintext);
        let content = decoded?;
        if content.format() != item.format {
            return Err(ClipboardError::CorruptHistory {
                reason: "sealed payload format does not match metadata",
            });
        }
        Ok(DecryptedItem { item, content })
    }

    /// Return a bounded preview and cache it until lock.
    pub fn preview(&mut self, id: u64, max_chars: usize) -> Result<String, ClipboardError> {
        let decrypted = self.decrypt_item(id)?;
        let max_chars = max_chars.clamp(1, MAX_PREVIEW_CHARS);
        let preview = decrypted.content.preview(max_chars);
        self.previews.insert(id, preview.clone());
        Ok(preview)
    }

    /// Lock capture and decryption and wipe the preview cache.
    pub fn lock(&mut self) {
        self.locked = true;
        self.backend.lock();
        self.previews.clear();
    }

    /// Unlock and rebuild the metadata index from disk.
    ///
    /// Paused and private-mode session flags are preserved. Captures are never
    /// replayed, so entries are not duplicated.
    pub fn unlock(&mut self) -> Result<(), ClipboardError> {
        self.backend.unlock();
        self.locked = false;
        self.previews.clear();
        if let Err(error) = self.reload_from_disk() {
            self.locked = true;
            self.backend.lock();
            return Err(error);
        }
        Ok(())
    }

    /// Rebuild the metadata index from disk without changing lock state.
    pub fn reload_index(&mut self) -> Result<(), ClipboardError> {
        self.validate_unlocked()?;
        self.previews.clear();
        self.reload_from_disk()
    }

    /// Pause capture. Stored items are untouched.
    ///
    /// Session-only; persist the user's choice in settings and re-apply after
    /// reopening the store.
    pub fn pause(&mut self) {
        self.history.pause();
    }

    /// Resume capture.
    pub fn resume(&mut self) {
        self.history.resume();
    }

    /// Enable or disable private mode.
    ///
    /// Session-only; persist the user's choice in settings and re-apply after
    /// reopening the store.
    pub fn set_private_mode(&mut self, enabled: bool) {
        self.history.set_private_mode(enabled);
    }

    /// Replace the capture policy and delete payloads for evicted entries.
    pub fn update_policy(&mut self, policy: CapturePolicy) -> Result<Vec<u64>, ClipboardError> {
        self.validate_unlocked()?;
        let mut candidate = self.history.clone();
        let evicted = candidate.set_policy(policy);
        self.persist_history(&candidate)?;
        self.history = candidate;
        let mut manifest_changed = false;
        for item in &evicted {
            let alias = self.alias_for(item.id);
            self.manifest.aliases.insert(alias.clone());
            self.manifest.present = true;
            manifest_changed = true;
            if self.backend.delete(&alias).is_ok() {
                self.manifest.aliases.remove(&alias);
            }
            let _ = self.backend.delete(&legacy_alias_for(item.id));
            self.previews.remove(item.id);
        }
        if manifest_changed {
            let _ = self.persist_manifest();
        }
        Ok(evicted.into_iter().map(|item| item.id).collect())
    }

    /// Pin or unpin an item.
    pub fn set_pinned(&mut self, id: u64, pinned: bool) -> Result<bool, ClipboardError> {
        self.validate_unlocked()?;
        let mut candidate = self.history.clone();
        if !candidate.set_pinned(id, pinned) {
            return Ok(false);
        }
        self.persist_history(&candidate)?;
        self.history = candidate;
        Ok(true)
    }

    /// Delete one item and its sealed payload, including the legacy
    /// `ansa-clip-<id>` alias if a migration left one behind.
    ///
    /// The metadata commit is authoritative. If ciphertext deletion fails, the
    /// alias remains in the ownership manifest as a tombstone and is retried by
    /// a later non-empty-history cleanup or an explicit [`Self::clear_all`].
    pub fn delete_item(&mut self, id: u64) -> Result<bool, ClipboardError> {
        self.validate_unlocked()?;
        let mut candidate = self.history.clone();
        if candidate.remove(id).is_none() {
            return Ok(false);
        }
        self.persist_history(&candidate)?;
        self.history = candidate;
        self.previews.remove(id);
        let alias = self.alias_for(id);
        self.manifest.aliases.insert(alias.clone());
        self.manifest.present = true;
        if self.backend.delete(&alias).is_ok() {
            self.manifest.aliases.remove(&alias);
        }
        let _ = self.backend.delete(&legacy_alias_for(id));
        let _ = self.persist_manifest();
        Ok(true)
    }

    /// Delete every item and sealed payload, including pinned items and legacy
    /// `ansa-clip-<id>` aliases.
    ///
    /// This is an explicit destructive operation. It also sweeps every alias in
    /// the ownership manifest, including tombstones left by earlier failed
    /// deletes. A failed payload delete leaves its tombstone in the manifest.
    pub fn clear_all(&mut self) -> Result<usize, ClipboardError> {
        self.validate_unlocked()?;
        let mut candidate = self.history.clone();
        let removed = candidate.clear();
        let mut alias_set: BTreeSet<String> = self.manifest.aliases.clone();
        let mut legacy_aliases = Vec::with_capacity(removed.len());
        for item in &removed {
            alias_set.insert(self.alias_for(item.id));
            legacy_aliases.push(legacy_alias_for(item.id));
        }
        for alias in &alias_set {
            self.manifest.aliases.insert(alias.clone());
        }
        self.manifest.present = true;
        self.persist_history(&candidate)?;
        self.history = candidate;
        self.metadata_present = true;
        self.previews.clear();
        for alias in alias_set {
            if self.backend.delete(&alias).is_ok() {
                self.manifest.aliases.remove(&alias);
            }
        }
        for alias in legacy_aliases {
            let _ = self.backend.delete(&alias);
        }
        let _ = self.persist_manifest();
        Ok(removed.len())
    }

    fn validate_unlocked(&mut self) -> Result<(), ClipboardError> {
        self.sync_lock();
        if self.locked {
            Err(ClipboardError::Locked)
        } else {
            Ok(())
        }
    }

    fn sync_lock(&mut self) {
        if self.backend.is_locked() {
            self.locked = true;
            self.previews.clear();
        }
    }

    fn reload_from_disk(&mut self) -> Result<(), ClipboardError> {
        let paused = self.history.is_paused();
        let private_mode = self.history.is_private_mode();
        let policy = self.history.policy().clone();
        let loaded = load_history(&self.root, policy.clone())?;
        if let Some(stored_id) = loaded.store_id.as_deref()
            && stored_id != self.store_id
        {
            return Err(ClipboardError::CorruptHistory {
                reason: "metadata store identifier does not match the manifest",
            });
        }
        let mut history = loaded.history;
        if !loaded.metadata_present {
            let next_id = self
                .manifest
                .max_item_id()
                .map_or(1, |identifier| identifier.saturating_add(1));
            history.reset_next_id(next_id);
        }
        let evicted = history.set_policy(policy);
        history.set_private_mode(private_mode);
        if paused {
            history.pause();
        } else {
            history.resume();
        }
        self.metadata_present = loaded.metadata_present;
        if !evicted.is_empty() {
            self.persist_history(&history)?;
            for item in &evicted {
                let alias = self.alias_for(item.id);
                self.manifest.aliases.insert(alias.clone());
                self.manifest.present = true;
                if self.backend.delete(&alias).is_ok() {
                    self.manifest.aliases.remove(&alias);
                }
                let _ = self.backend.delete(&legacy_alias_for(item.id));
                self.previews.remove(item.id);
            }
            let _ = self.persist_manifest();
        }
        self.history = history;
        self.cleanup_orphans()?;
        Ok(())
    }

    fn persist_history(&self, history: &ClipboardHistory) -> Result<(), ClipboardError> {
        let metadata = HistoryMetadata {
            version: METADATA_VERSION,
            store_id: Some(self.store_id.clone()),
            next_id: history.next_id(),
            items: history.items().to_vec(),
        };
        let bytes = serde_json::to_vec(&metadata).map_err(|_| ClipboardError::CorruptHistory {
            reason: "clipboard metadata could not be encoded",
        })?;
        if bytes.len() > MAX_METADATA_BYTES {
            return Err(ClipboardError::MetadataTooLarge {
                limit: MAX_METADATA_BYTES,
            });
        }
        atomic_write(&self.root, &self.metadata_path(), &bytes)
    }

    fn persist_manifest(&self) -> Result<(), ClipboardError> {
        self.persist_manifest_value(&self.manifest)
    }

    fn persist_manifest_value(&self, manifest: &Manifest) -> Result<(), ClipboardError> {
        if !manifest.present && manifest.aliases.is_empty() {
            return Ok(());
        }
        let file = ManifestFile {
            version: MANIFEST_VERSION,
            store_id: manifest.store_id.clone(),
            aliases: manifest.aliases.iter().cloned().collect(),
        };
        let bytes = serde_json::to_vec(&file).map_err(|_| ClipboardError::CorruptManifest {
            reason: "clipboard manifest could not be encoded",
        })?;
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(ClipboardError::MetadataTooLarge {
                limit: MAX_MANIFEST_BYTES,
            });
        }
        atomic_write(&self.root, &self.manifest_path(), &bytes)
    }

    fn metadata_path(&self) -> PathBuf {
        self.root.join(METADATA_FILE)
    }

    fn manifest_path(&self) -> PathBuf {
        self.root.join(MANIFEST_FILE)
    }

    fn alias_for(&self, id: u64) -> String {
        format!("{}{}", alias_prefix(&self.store_id), id)
    }

    /// Migrate a pre-namespace store to the current alias layout.
    ///
    /// All old payloads are decrypted in a preflight pass before any write. New
    /// aliases are then written, metadata is committed, and only afterwards are
    /// the old aliases deleted. A decryption failure leaves every old file
    /// untouched; a write failure rolls back the new aliases. If the metadata
    /// commit succeeds but manifest or old-alias cleanup fails, the store is
    /// still usable and the next open rebuilds the manifest / explicit
    /// delete-clear paths clean the leftovers.
    fn migrate_legacy(&mut self) -> Result<(), ClipboardError> {
        let items = self.history.items().to_vec();

        for item in &items {
            match self.backend.open(&legacy_alias_for(item.id)) {
                Ok(mut plaintext) => {
                    let decoded = decode_payload(&plaintext);
                    let usable = decoded
                        .as_ref()
                        .is_ok_and(|content| content.format() == item.format);
                    drop(decoded);
                    wipe_bytes(&mut plaintext);
                    if !usable {
                        return Err(ClipboardError::LegacyLayout {
                            reason: "a legacy clipboard item is not a usable payload",
                        });
                    }
                }
                Err(_) => {
                    return Err(ClipboardError::LegacyLayout {
                        reason: "a legacy clipboard item could not be decrypted",
                    });
                }
            }
        }

        let mut written: Vec<String> = Vec::with_capacity(items.len());
        for item in &items {
            let mut plaintext = match self.backend.open(&legacy_alias_for(item.id)) {
                Ok(plaintext) => plaintext,
                Err(_) => {
                    for alias in &written {
                        let _ = self.backend.delete(alias);
                    }
                    return Err(ClipboardError::LegacyLayout {
                        reason: "a legacy clipboard item could not be decrypted",
                    });
                }
            };
            let alias = self.alias_for(item.id);
            let sealed = self.backend.seal(&alias, &plaintext);
            wipe_bytes(&mut plaintext);
            if let Err(error) = sealed {
                for alias in &written {
                    let _ = self.backend.delete(alias);
                }
                return Err(error);
            }
            written.push(alias);
        }

        if let Err(error) = self.persist_history(&self.history) {
            for alias in &written {
                let _ = self.backend.delete(alias);
            }
            return Err(error);
        }
        self.metadata_present = true;

        for alias in &written {
            self.manifest.aliases.insert(alias.clone());
        }
        self.manifest.present = true;
        let _ = self.persist_manifest();

        for item in &items {
            let _ = self.backend.delete(&legacy_alias_for(item.id));
        }
        Ok(())
    }

    /// Remove only aliases that this store's manifest proves it owns, that the
    /// metadata no longer references, and only while the metadata exists and is
    /// non-empty. A missing or empty history is never treated as "delete all".
    fn cleanup_orphans(&mut self) -> Result<(), ClipboardError> {
        if !self.metadata_present || self.history.is_empty() || !self.manifest.present {
            return Ok(());
        }
        let referenced: BTreeSet<String> = self
            .history
            .items()
            .iter()
            .map(|item| self.alias_for(item.id))
            .collect();
        let candidates: Vec<String> = self
            .manifest
            .aliases
            .iter()
            .filter(|alias| !referenced.contains(*alias))
            .cloned()
            .collect();
        let mut changed = false;
        for alias in candidates {
            if !alias_belongs_to(&self.store_id, &alias) {
                self.manifest.aliases.remove(&alias);
                changed = true;
                continue;
            }
            if self.backend.delete(&alias).is_ok() {
                self.manifest.aliases.remove(&alias);
                changed = true;
            }
        }
        if changed {
            let _ = self.persist_manifest();
        }
        Ok(())
    }
}

impl Drop for ClipboardStore {
    fn drop(&mut self) {
        self.previews.clear();
        self.locked = true;
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryMetadata {
    version: u32,
    /// Store namespace that owns the sealed aliases. Missing in metadata
    /// written before store identification existed; the manifest supplies it in
    /// that case.
    #[serde(default)]
    store_id: Option<String>,
    next_id: u64,
    items: Vec<ClipboardItem>,
}

struct LoadedHistory {
    history: ClipboardHistory,
    metadata_present: bool,
    store_id: Option<String>,
}

fn resolve_store_id(
    manifest: &mut Manifest,
    metadata_store_id: Option<&str>,
) -> Result<String, ClipboardError> {
    if let Some(stored_id) = metadata_store_id {
        if !is_valid_store_id(stored_id) {
            return Err(ClipboardError::CorruptHistory {
                reason: "metadata store identifier is invalid",
            });
        }
        if manifest.present && manifest.store_id != stored_id {
            return Err(ClipboardError::CorruptHistory {
                reason: "metadata store identifier does not match the manifest",
            });
        }
        if !manifest.present {
            manifest.store_id = stored_id.to_owned();
        }
    }
    Ok(manifest.store_id.clone())
}

fn load_history(root: &Path, policy: CapturePolicy) -> Result<LoadedHistory, ClipboardError> {
    let path = root.join(METADATA_FILE);
    if !path.exists() {
        return Ok(LoadedHistory {
            history: ClipboardHistory::new(policy),
            metadata_present: false,
            store_id: None,
        });
    }
    let bytes = read_bounded(&path, MAX_METADATA_BYTES)?;
    let metadata: HistoryMetadata =
        serde_json::from_slice(&bytes).map_err(|_| ClipboardError::CorruptHistory {
            reason: "clipboard metadata is not valid JSON",
        })?;
    if metadata.version != METADATA_VERSION {
        return Err(ClipboardError::CorruptHistory {
            reason: "clipboard metadata version is not supported",
        });
    }
    let history = ClipboardHistory::from_stored(policy, metadata.next_id, metadata.items)?;
    Ok(LoadedHistory {
        history,
        metadata_present: true,
        store_id: metadata.store_id,
    })
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, ClipboardError> {
    let file = File::open(path)?;
    let length = file.metadata()?.len();
    if length > limit as u64 {
        return Err(ClipboardError::MetadataTooLarge { limit });
    }
    let mut bytes = Vec::with_capacity(length as usize);
    let mut limited = file.take(limit as u64 + 1);
    limited.read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(ClipboardError::MetadataTooLarge { limit });
    }
    Ok(bytes)
}

fn atomic_write(directory: &Path, target: &Path, bytes: &[u8]) -> Result<(), ClipboardError> {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = directory.join(format!("{TEMP_PREFIX}{}-{counter}", std::process::id()));

    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    Ok(result?)
}

fn encode_payload(content: &ClipboardContent) -> Vec<u8> {
    let mut output = Vec::with_capacity(content.byte_len() + 16);
    output.extend_from_slice(ENVELOPE_MAGIC);
    output.push(ENVELOPE_VERSION);
    match content {
        ClipboardContent::Text(value) => {
            output.push(FORMAT_TEXT);
            output.extend_from_slice(value.as_bytes());
        }
        ClipboardContent::FileList(paths) => {
            output.push(FORMAT_FILE_LIST);
            output.extend_from_slice(&(paths.len() as u32).to_le_bytes());
            for path in paths {
                output.extend_from_slice(&(path.len() as u32).to_le_bytes());
                output.extend_from_slice(path.as_bytes());
            }
        }
    }
    output
}

fn decode_payload(bytes: &[u8]) -> Result<ClipboardContent, ClipboardError> {
    if bytes.len() < 6 || &bytes[..4] != ENVELOPE_MAGIC {
        return Err(ClipboardError::MalformedPayload {
            reason: "envelope header is missing or invalid",
        });
    }
    if bytes[4] != ENVELOPE_VERSION {
        return Err(ClipboardError::MalformedPayload {
            reason: "envelope version is not supported",
        });
    }
    match bytes[5] {
        FORMAT_TEXT => {
            let value =
                std::str::from_utf8(&bytes[6..]).map_err(|_| ClipboardError::MalformedPayload {
                    reason: "text is not valid UTF-8",
                })?;
            Ok(ClipboardContent::Text(value.to_owned()))
        }
        FORMAT_FILE_LIST => {
            let mut offset = 6usize;
            let count = read_u32(bytes, &mut offset)? as usize;
            if count > MAX_FILE_ENTRIES {
                return Err(ClipboardError::MalformedPayload {
                    reason: "file list is too long",
                });
            }
            let mut paths = Vec::with_capacity(count);
            for _ in 0..count {
                let length = read_u32(bytes, &mut offset)? as usize;
                let end = offset
                    .checked_add(length)
                    .ok_or(ClipboardError::MalformedPayload {
                        reason: "file path length overflowed",
                    })?;
                let slice = bytes
                    .get(offset..end)
                    .ok_or(ClipboardError::MalformedPayload {
                        reason: "file path is truncated",
                    })?;
                let path =
                    std::str::from_utf8(slice).map_err(|_| ClipboardError::MalformedPayload {
                        reason: "file path is not valid UTF-8",
                    })?;
                paths.push(path.to_owned());
                offset = end;
            }
            if offset != bytes.len() {
                return Err(ClipboardError::MalformedPayload {
                    reason: "file list has trailing bytes",
                });
            }
            Ok(ClipboardContent::FileList(paths))
        }
        _ => Err(ClipboardError::MalformedPayload {
            reason: "envelope format tag is unknown",
        }),
    }
}

fn read_u32(bytes: &[u8], offset: &mut usize) -> Result<u32, ClipboardError> {
    let end = offset
        .checked_add(4)
        .ok_or(ClipboardError::MalformedPayload {
            reason: "file list length overflowed",
        })?;
    let slice = bytes
        .get(*offset..end)
        .ok_or(ClipboardError::MalformedPayload {
            reason: "file list header is truncated",
        })?;
    let mut raw = [0u8; 4];
    raw.copy_from_slice(slice);
    *offset = end;
    Ok(u32::from_le_bytes(raw))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::filter::{ExclusionMarkers, SnapshotContent};
    use crate::model::{ClipboardFormat, ItemSummary, SourceApp, SourceConfidence};

    fn legacy_item(id: u64, text: &str) -> ClipboardItem {
        ClipboardItem {
            id,
            format: ClipboardFormat::Text,
            bytes: text.len(),
            captured_at: id,
            sequence: id,
            source: None,
            confidence: SourceConfidence::Unknown,
            pinned: false,
            summary: ItemSummary::Text {
                characters: text.chars().count(),
            },
        }
    }

    fn write_legacy_store(
        dir: &Path,
        backend: &MemorySealBackend,
        items: &[(u64, &str)],
    ) -> Vec<u8> {
        let metadata = HistoryMetadata {
            version: METADATA_VERSION,
            store_id: None,
            next_id: items.iter().map(|(id, _)| *id).max().map_or(1, |id| id + 1),
            items: items
                .iter()
                .map(|(id, text)| legacy_item(*id, text))
                .collect(),
        };
        let bytes = serde_json::to_vec(&metadata).expect("encode legacy metadata");
        fs::write(dir.join("history.json"), &bytes).expect("write legacy history");
        for (id, text) in items {
            backend
                .seal(
                    &legacy_alias_for(*id),
                    &encode_payload(&ClipboardContent::text(*text)),
                )
                .expect("seal legacy payload");
        }
        bytes
    }

    fn open_store(dir: &Path, policy: CapturePolicy) -> (ClipboardStore, MemorySealBackend) {
        let backend = MemorySealBackend::new();
        let handle = backend.clone();
        let store =
            ClipboardStore::open_with_backend(dir, policy, Box::new(backend)).expect("store opens");
        (store, handle)
    }

    fn text(sequence: u64, value: &str) -> ClipboardSnapshot {
        ClipboardSnapshot::text(sequence, value)
    }

    fn assert_no_sentinel(path: &Path, sentinel: &str) {
        let needle = sentinel.as_bytes();
        for entry in fs::read_dir(path).expect("read dir") {
            let entry = entry.expect("entry");
            let entry_path = entry.path();
            if entry_path.file_name().and_then(|name| name.to_str()) == Some(".lock") {
                // The cross-process lock file is held with no sharing; reading it
                // from the same process would fail and it contains no data.
                continue;
            }
            if entry.file_type().expect("file type").is_dir() {
                assert_no_sentinel(&entry_path, sentinel);
            } else {
                let bytes = fs::read(&entry_path).expect("read file");
                assert!(
                    !bytes.windows(needle.len()).any(|window| window == needle),
                    "plaintext sentinel reached {}",
                    entry_path.display()
                );
            }
        }
    }

    #[test]
    fn text_round_trip_is_sealed_and_metadata_has_no_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut store, backend) = open_store(dir.path(), CapturePolicy::default());
        let sentinel = "unique clipboard sentinel 9f3ac2";
        let report = store.capture(text(1, sentinel), 100).expect("capture");
        let CaptureReport::Stored(record) = report else {
            panic!("expected stored");
        };
        assert_eq!(record.item.format, ClipboardFormat::Text);
        assert!(!record.item.pinned);

        assert!(backend.contains(&store.alias_for(record.item.id)));
        let metadata = fs::read_to_string(dir.path().join("history.json")).expect("metadata");
        assert!(!metadata.contains(sentinel));
        assert_no_sentinel(dir.path(), sentinel);

        let decrypted = store.decrypt_item(record.item.id).expect("decrypt");
        match &decrypted.content {
            ClipboardContent::Text(value) => assert_eq!(value, sentinel),
            ClipboardContent::FileList(_) => panic!("wrong content"),
        }
    }

    #[test]
    fn file_list_round_trip_preserves_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut store, _) = open_store(dir.path(), CapturePolicy::default());
        let paths = vec![
            "C:\\Users\\person\\one.txt".to_owned(),
            "C:\\Users\\person\\two.txt".to_owned(),
        ];
        let report = store
            .capture(ClipboardSnapshot::file_list(1, paths.clone()), 1)
            .expect("capture");
        let CaptureReport::Stored(record) = report else {
            panic!("expected stored");
        };
        assert!(matches!(
            record.item.summary,
            crate::model::ItemSummary::FileList { files: 2 }
        ));
        let decrypted = store.decrypt_item(record.item.id).expect("decrypt");
        match &decrypted.content {
            ClipboardContent::FileList(actual) => assert_eq!(actual, &paths),
            ClipboardContent::Text(_) => panic!("wrong content"),
        }
    }

    #[test]
    fn sensitive_fixtures_never_reach_storage_or_preview() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut store, backend) = open_store(dir.path(), CapturePolicy::default());
        let fixtures = [
            "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
            "-----BEGIN PRIVATE KEY-----",
            "password=correct-horse-battery-staple",
            "4111 1111 1111 1111",
        ];
        for (index, fixture) in fixtures.iter().enumerate() {
            let report = store
                .capture(text(index as u64 + 1, fixture), index as u64 + 1)
                .expect("capture");
            assert_eq!(
                report,
                CaptureReport::Skipped(CaptureRejection::SensitiveContent)
            );
            assert!(!format!("{report:?}").contains(fixture));
            assert_no_sentinel(dir.path(), fixture);
        }
        assert!(store.items().is_empty());
        assert!(backend.is_empty());
        assert_eq!(store.cached_preview_count(), 0);
        assert!(matches!(
            store.preview(1, 20).expect_err("no item"),
            ClipboardError::ItemNotFound
        ));
    }

    #[test]
    fn exclusion_markers_and_app_exclusions_prevent_sealing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let policy = CapturePolicy {
            excluded_apps: vec!["keepass".into()],
            ..CapturePolicy::default()
        };
        let (mut store, backend) = open_store(dir.path(), policy);
        let marked =
            text(1, "hello").with_markers(ExclusionMarkers::new(Some(false), None, false, false));
        assert_eq!(
            store.capture(marked, 1).expect("capture"),
            CaptureReport::Skipped(CaptureRejection::HistoryMarker)
        );

        let app = text(2, "hello").with_source(
            SourceApp::new("KeePass").expect("source"),
            SourceConfidence::Probable,
        );
        assert_eq!(
            store.capture(app, 2).expect("capture"),
            CaptureReport::Skipped(CaptureRejection::AppExcluded)
        );
        assert!(backend.is_empty());
        assert!(store.items().is_empty());
    }

    #[test]
    fn quota_eviction_deletes_the_oldest_sealed_payload() {
        let dir = tempfile::tempdir().expect("tempdir");
        let policy = CapturePolicy {
            max_items: 2,
            ..CapturePolicy::default()
        };
        let (mut store, backend) = open_store(dir.path(), policy);
        for sequence in 1..=3 {
            store
                .capture(text(sequence, "payload"), sequence)
                .expect("capture");
        }
        let ids = store.items().iter().map(|item| item.id).collect::<Vec<_>>();
        assert_eq!(ids, vec![3, 2]);
        assert_eq!(backend.len(), 2);
        assert!(!backend.contains(&store.alias_for(1)));
        assert!(backend.contains(&store.alias_for(2)));
        assert!(matches!(
            store.decrypt_item(1).expect_err("evicted"),
            ClipboardError::ItemNotFound
        ));
    }

    #[test]
    fn pinned_items_survive_eviction_and_can_be_deleted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let policy = CapturePolicy {
            max_items: 2,
            ..CapturePolicy::default()
        };
        let (mut store, backend) = open_store(dir.path(), policy);
        let first = store.capture(text(1, "one"), 1).expect("capture");
        let CaptureReport::Stored(first) = first else {
            panic!("stored");
        };
        assert!(store.set_pinned(first.item.id, true).expect("pin"));
        store.capture(text(2, "two"), 2).expect("capture");
        store.capture(text(3, "three"), 3).expect("capture");

        assert!(store.item(first.item.id).is_some());
        assert_eq!(store.items().len(), 2);
        assert_eq!(backend.len(), 2);

        assert!(store.delete_item(first.item.id).expect("delete"));
        assert!(backend.contains(&store.alias_for(2)) || backend.contains(&store.alias_for(3)));
        assert!(!backend.contains(&store.alias_for(first.item.id)));
    }

    #[test]
    fn pinned_items_exhaust_the_quota_without_eviction() {
        let dir = tempfile::tempdir().expect("tempdir");
        let policy = CapturePolicy {
            max_items: 1,
            ..CapturePolicy::default()
        };
        let (mut store, _) = open_store(dir.path(), policy);
        let first = store.capture(text(1, "one"), 1).expect("capture");
        let CaptureReport::Stored(first) = first else {
            panic!("stored");
        };
        store.set_pinned(first.item.id, true).expect("pin");
        assert_eq!(
            store.capture(text(2, "two"), 2).expect("capture"),
            CaptureReport::Skipped(CaptureRejection::QuotaExhausted)
        );
        assert_eq!(store.items().len(), 1);
        assert_eq!(store.items()[0].id, first.item.id);
    }

    #[test]
    fn duplicate_sequence_is_not_stored_twice() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut store, _) = open_store(dir.path(), CapturePolicy::default());
        store.capture(text(7, "one"), 1).expect("capture");
        assert_eq!(
            store.capture(text(7, "one"), 2).expect("capture"),
            CaptureReport::Duplicate
        );
        assert_eq!(store.items().len(), 1);
    }

    #[test]
    fn pause_and_private_mode_stop_capture_immediately() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut store, backend) = open_store(dir.path(), CapturePolicy::default());
        store.pause();
        assert!(store.state().paused);
        assert_eq!(
            store.capture(text(1, "hello"), 1).expect("capture"),
            CaptureReport::Skipped(CaptureRejection::Paused)
        );
        store.resume();
        store.set_private_mode(true);
        assert!(store.state().private_mode);
        assert_eq!(
            store.capture(text(2, "hello"), 2).expect("capture"),
            CaptureReport::Skipped(CaptureRejection::PrivateMode)
        );
        assert!(backend.is_empty());
        assert!(store.items().is_empty());
    }

    #[test]
    fn lock_blocks_capture_and_wipes_previews_then_unlock_rebuilds_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut store, _) = open_store(dir.path(), CapturePolicy::default());
        let report = store.capture(text(1, "preview me"), 1).expect("capture");
        let CaptureReport::Stored(record) = report else {
            panic!("stored");
        };
        assert_eq!(
            store.preview(record.item.id, 20).expect("preview"),
            "preview me"
        );
        assert_eq!(store.cached_preview_count(), 1);

        store.lock();
        assert!(store.state().locked);
        assert_eq!(store.cached_preview_count(), 0);
        assert_eq!(
            store.capture(text(2, "blocked"), 2).expect("capture"),
            CaptureReport::Skipped(CaptureRejection::Locked)
        );
        assert!(matches!(
            store.preview(record.item.id, 20).expect_err("locked"),
            ClipboardError::Locked
        ));

        store.unlock().expect("unlock");
        assert_eq!(store.items().len(), 1);
        assert_eq!(
            store.preview(record.item.id, 20).expect("preview"),
            "preview me"
        );
        store.capture(text(3, "after"), 3).expect("capture");
        assert_eq!(store.items().len(), 2);
    }

    #[test]
    fn an_external_backend_lock_is_observed_and_clears_previews() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut store, backend) = open_store(dir.path(), CapturePolicy::default());
        let report = store.capture(text(1, "preview"), 1).expect("capture");
        let CaptureReport::Stored(record) = report else {
            panic!("stored");
        };
        store.preview(record.item.id, 20).expect("preview");
        assert_eq!(store.cached_preview_count(), 1);

        backend.lock();
        assert_eq!(
            store.capture(text(2, "blocked"), 2).expect("capture"),
            CaptureReport::Skipped(CaptureRejection::Locked)
        );
        assert_eq!(store.cached_preview_count(), 0);
        assert!(matches!(
            store.decrypt_item(record.item.id).expect_err("locked"),
            ClipboardError::Locked
        ));
    }

    #[test]
    fn per_item_delete_and_clear_all_remove_payloads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut store, backend) = open_store(dir.path(), CapturePolicy::default());
        let first = store.capture(text(1, "one"), 1).expect("capture");
        let CaptureReport::Stored(first) = first else {
            panic!("stored");
        };
        let second = store.capture(text(2, "two"), 2).expect("capture");
        let CaptureReport::Stored(second) = second else {
            panic!("stored");
        };

        assert!(store.delete_item(first.item.id).expect("delete"));
        assert!(!backend.contains(&store.alias_for(first.item.id)));
        assert!(backend.contains(&store.alias_for(second.item.id)));
        assert_eq!(store.items().len(), 1);

        assert_eq!(store.clear_all().expect("clear"), 1);
        assert!(store.items().is_empty());
        assert!(backend.is_empty());
    }

    #[test]
    fn metadata_survives_reopen_and_enforces_a_shrunk_policy() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let (mut store, _) = open_store(dir.path(), CapturePolicy::default());
            for sequence in 1..=3 {
                store
                    .capture(text(sequence, "payload"), sequence)
                    .expect("capture");
            }
        }
        let policy = CapturePolicy {
            max_items: 1,
            ..CapturePolicy::default()
        };
        let (store, _) = open_store(dir.path(), policy);
        assert_eq!(store.items().len(), 1);
        assert_no_sentinel(dir.path(), "payload");
    }

    #[test]
    fn age_quota_is_enforced_on_capture() {
        let dir = tempfile::tempdir().expect("tempdir");
        let policy = CapturePolicy {
            max_age_secs: 10,
            ..CapturePolicy::default()
        };
        let (mut store, backend) = open_store(dir.path(), policy);
        store.capture(text(1, "old"), 0).expect("capture");
        store.capture(text(2, "new"), 100).expect("capture");
        assert_eq!(store.items().len(), 1);
        assert!(!backend.contains(&store.alias_for(1)));
        assert!(backend.contains(&store.alias_for(2)));
    }

    #[test]
    fn policy_update_evicts_and_deletes_old_payloads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut store, backend) = open_store(dir.path(), CapturePolicy::default());
        store.capture(text(1, "one"), 1).expect("capture");
        store.capture(text(2, "two"), 2).expect("capture");
        let evicted = store
            .update_policy(CapturePolicy {
                max_items: 1,
                ..CapturePolicy::default()
            })
            .expect("policy");
        assert_eq!(evicted, vec![1]);
        assert_eq!(store.items().len(), 1);
        assert!(!backend.contains(&store.alias_for(1)));
    }

    #[test]
    fn corrupt_metadata_is_rejected_without_echoing_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("history.json"), b"{ not json }").expect("write");
        let backend = MemorySealBackend::new();
        let error = ClipboardStore::open_with_backend(
            dir.path(),
            CapturePolicy::default(),
            Box::new(backend),
        )
        .expect_err("corrupt");
        assert!(matches!(error, ClipboardError::CorruptHistory { .. }));
        assert!(!error.to_string().contains("not json"));
    }

    #[test]
    fn malformed_envelopes_are_rejected() {
        assert!(decode_payload(b"nope").is_err());
        assert!(decode_payload(b"ACB1\x01\x09").is_err());
        let mut text = encode_payload(&ClipboardContent::text("hello"));
        text.push(b'x');
        // Text envelopes consume the rest, so trailing bytes are content, not
        // an error; the file-list path proves strict framing.
        assert!(decode_payload(&text).is_ok());
        let mut files = encode_payload(&ClipboardContent::file_list(vec!["a".into()]));
        files.push(b'x');
        assert!(decode_payload(&files).is_err());
    }

    #[test]
    fn preview_cache_is_bounded_and_removed_with_items() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut store, _) = open_store(dir.path(), CapturePolicy::default());
        let report = store.capture(text(1, "preview"), 1).expect("capture");
        let CaptureReport::Stored(record) = report else {
            panic!("stored");
        };
        store.preview(record.item.id, 20).expect("preview");
        assert_eq!(store.cached_preview_count(), 1);
        store.delete_item(record.item.id).expect("delete");
        assert_eq!(store.cached_preview_count(), 0);
    }

    #[test]
    fn image_snapshot_is_reported_unsupported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut store, backend) = open_store(dir.path(), CapturePolicy::default());
        assert_eq!(
            store
                .capture(
                    ClipboardSnapshot::image(1, "image capture is not implemented"),
                    1
                )
                .expect("capture"),
            CaptureReport::Skipped(CaptureRejection::ImageUnsupported)
        );
        assert!(backend.is_empty());
    }

    #[test]
    fn snapshot_debug_does_not_echo_text() {
        let snapshot = ClipboardSnapshot {
            sequence: 1,
            content: SnapshotContent::Text("secret sentinel".into()),
            markers: ExclusionMarkers::default(),
            source: None,
            confidence: SourceConfidence::Unknown,
            mime_type: Some("text/plain".into()),
            truncated: false,
        };
        assert!(!format!("{snapshot:?}").contains("secret sentinel"));
    }

    #[test]
    fn eviction_removes_the_cached_preview() {
        let dir = tempfile::tempdir().expect("tempdir");
        let policy = CapturePolicy {
            max_items: 1,
            ..CapturePolicy::default()
        };
        let (mut store, _) = open_store(dir.path(), policy);
        let first = store.capture(text(1, "first preview"), 1).expect("capture");
        let CaptureReport::Stored(first) = first else {
            panic!("stored");
        };
        store.preview(first.item.id, 30).expect("preview");
        assert_eq!(store.cached_preview_count(), 1);

        store
            .capture(text(2, "second preview"), 2)
            .expect("capture");
        assert_eq!(store.cached_preview_count(), 0);
    }

    #[test]
    fn manifest_cleanup_removes_only_recorded_unreferenced_aliases() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut store, backend) = open_store(dir.path(), CapturePolicy::default());
        store.capture(text(1, "live"), 1).expect("capture");
        let live_alias = store.alias_for(1);
        let orphan_alias = store.alias_for(999);
        store.manifest.aliases.insert(orphan_alias.clone());
        store.manifest.present = true;
        store.persist_manifest().expect("manifest");
        backend
            .seal(&orphan_alias, b"orphaned envelope")
            .expect("seed orphan");
        backend
            .seal("ansa-clip-vault-note", b"keep me")
            .expect("seed prefixed unrelated");

        store.reload_index().expect("reload");
        assert!(!backend.contains(&orphan_alias));
        assert!(backend.contains(&live_alias));
        assert!(backend.contains("ansa-clip-vault-note"));
    }

    #[test]
    fn missing_history_never_wipes_manifest_seals_and_advances_ids() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = MemorySealBackend::new();
        let mut store = ClipboardStore::open_with_backend(
            dir.path(),
            CapturePolicy::default(),
            Box::new(backend.clone()),
        )
        .expect("store");
        let report = store
            .capture(text(1, "survives reset"), 1)
            .expect("capture");
        let CaptureReport::Stored(record) = report else {
            panic!("stored");
        };
        let old_alias = store.alias_for(record.item.id);
        drop(store);
        fs::remove_file(dir.path().join("history.json")).expect("remove history");

        let mut reopened = ClipboardStore::open_with_backend(
            dir.path(),
            CapturePolicy::default(),
            Box::new(backend.clone()),
        )
        .expect("reopen");
        assert!(reopened.items().is_empty());
        assert!(
            backend.contains(&old_alias),
            "missing history deleted a seal"
        );

        let report = reopened.capture(text(2, "new item"), 2).expect("capture");
        let CaptureReport::Stored(record) = report else {
            panic!("stored");
        };
        assert_eq!(record.item.id, 2);
        assert!(backend.contains(&old_alias));
        assert!(backend.contains(&reopened.alias_for(record.item.id)));
    }

    #[test]
    fn empty_history_never_wipes_manifest_seals() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = MemorySealBackend::new();
        let mut store = ClipboardStore::open_with_backend(
            dir.path(),
            CapturePolicy::default(),
            Box::new(backend.clone()),
        )
        .expect("store");
        let report = store.capture(text(1, "kept"), 1).expect("capture");
        let CaptureReport::Stored(record) = report else {
            panic!("stored");
        };
        let alias = store.alias_for(record.item.id);
        let store_id = store.store_id.clone();
        drop(store);

        let empty = HistoryMetadata {
            version: METADATA_VERSION,
            store_id: Some(store_id),
            next_id: 2,
            items: Vec::new(),
        };
        fs::write(
            dir.path().join("history.json"),
            serde_json::to_vec(&empty).expect("encode"),
        )
        .expect("write empty history");

        let reopened = ClipboardStore::open_with_backend(
            dir.path(),
            CapturePolicy::default(),
            Box::new(backend.clone()),
        )
        .expect("reopen");
        assert!(reopened.items().is_empty());
        assert!(
            backend.contains(&alias),
            "empty history deleted a manifest seal"
        );
    }

    #[test]
    fn legacy_store_is_migrated_and_legacy_aliases_are_cleaned() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = MemorySealBackend::new();
        let legacy_bytes = write_legacy_store(
            dir.path(),
            &backend,
            &[(1, "legacy one"), (2, "legacy two")],
        );

        let mut store = ClipboardStore::open_with_backend(
            dir.path(),
            CapturePolicy::default(),
            Box::new(backend.clone()),
        )
        .expect("legacy migration");
        assert_eq!(store.items().len(), 2);

        let new_one = store.alias_for(1);
        let new_two = store.alias_for(2);
        assert!(backend.contains(&new_one));
        assert!(backend.contains(&new_two));
        assert!(!backend.contains(&legacy_alias_for(1)));
        assert!(!backend.contains(&legacy_alias_for(2)));

        let decrypted = store.decrypt_item(1).expect("decrypt migrated item");
        match &decrypted.content {
            ClipboardContent::Text(value) => assert_eq!(value, "legacy one"),
            ClipboardContent::FileList(_) => panic!("wrong content"),
        }
        assert_ne!(
            fs::read(dir.path().join("history.json")).expect("history"),
            legacy_bytes
        );

        let upgraded: HistoryMetadata =
            serde_json::from_slice(&fs::read(dir.path().join("history.json")).expect("history"))
                .expect("decode upgraded history");
        assert!(upgraded.store_id.as_deref().is_some_and(is_valid_store_id));

        let manifest: ManifestFile =
            serde_json::from_slice(&fs::read(dir.path().join("manifest.json")).expect("manifest"))
                .expect("decode manifest");
        assert_eq!(manifest.store_id, upgraded.store_id.expect("store id"));
        assert!(manifest.aliases.contains(&new_one));

        backend
            .seal(&legacy_alias_for(1), b"leftover legacy one")
            .expect("seed leftover");
        assert!(store.delete_item(1).expect("delete"));
        assert!(!backend.contains(&new_one));
        assert!(!backend.contains(&legacy_alias_for(1)));

        backend
            .seal(&legacy_alias_for(2), b"leftover legacy two")
            .expect("seed leftover");
        assert_eq!(store.clear_all().expect("clear"), 1);
        assert!(!backend.contains(&new_two));
        assert!(!backend.contains(&legacy_alias_for(2)));
    }

    #[test]
    fn corrupt_legacy_store_refuses_without_modifying_the_old_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = MemorySealBackend::new();
        let metadata = HistoryMetadata {
            version: METADATA_VERSION,
            store_id: None,
            next_id: 3,
            items: vec![
                legacy_item(1, "readable legacy"),
                legacy_item(2, "unreadable legacy"),
            ],
        };
        let bytes = serde_json::to_vec(&metadata).expect("encode");
        fs::write(dir.path().join("history.json"), &bytes).expect("history");
        backend
            .seal(
                &legacy_alias_for(1),
                &encode_payload(&ClipboardContent::text("readable legacy")),
            )
            .expect("seal readable legacy");

        let error = ClipboardStore::open_with_backend(
            dir.path(),
            CapturePolicy::default(),
            Box::new(backend.clone()),
        )
        .expect_err("corrupt legacy store must not open");
        assert!(matches!(error, ClipboardError::LegacyLayout { .. }));
        assert_eq!(
            fs::read(dir.path().join("history.json")).expect("history"),
            bytes
        );
        assert!(!dir.path().join("manifest.json").exists());
        assert_eq!(backend.aliases(), vec![legacy_alias_for(1)]);
        assert!(!backend.contains(&legacy_alias_for(2)));

        backend
            .seal(
                &legacy_alias_for(2),
                &encode_payload(&ClipboardContent::text("unreadable legacy")),
            )
            .expect("repair legacy payload");
        let mut store = ClipboardStore::open_with_backend(
            dir.path(),
            CapturePolicy::default(),
            Box::new(backend),
        )
        .expect("migration succeeds after repair");
        let decrypted = store.decrypt_item(1).expect("decrypt readable");
        match &decrypted.content {
            ClipboardContent::Text(value) => assert_eq!(value, "readable legacy"),
            ClipboardContent::FileList(_) => panic!("wrong content"),
        }
        let decrypted = store.decrypt_item(2).expect("decrypt repaired");
        match &decrypted.content {
            ClipboardContent::Text(value) => assert_eq!(value, "unreadable legacy"),
            ClipboardContent::FileList(_) => panic!("wrong content"),
        }
    }

    #[test]
    fn legacy_store_with_an_unexpected_manifest_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = MemorySealBackend::new();
        write_legacy_store(dir.path(), &backend, &[(1, "legacy")]);
        let manifest = ManifestFile {
            version: MANIFEST_VERSION,
            store_id: generate_store_id(),
            aliases: Vec::new(),
        };
        fs::write(
            dir.path().join("manifest.json"),
            serde_json::to_vec(&manifest).expect("encode"),
        )
        .expect("manifest");

        let error = ClipboardStore::open_with_backend(
            dir.path(),
            CapturePolicy::default(),
            Box::new(backend),
        )
        .expect_err("ambiguous legacy state must be refused");
        assert!(matches!(error, ClipboardError::LegacyLayout { .. }));
        assert!(dir.path().join("history.json").exists());
    }

    #[cfg(windows)]
    #[test]
    fn vault_legacy_store_migrates_with_real_dpapi() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault_path = dir.path().join("vault");
        let store_path = dir.path().join("store");
        fs::create_dir_all(&store_path).expect("store dir");

        let vault = Vault::open(&vault_path).expect("vault");
        vault
            .put_secret(
                &legacy_alias_for(1),
                &encode_payload(&ClipboardContent::text("legacy dpapi text")),
            )
            .expect("legacy secret");
        let metadata = HistoryMetadata {
            version: METADATA_VERSION,
            store_id: None,
            next_id: 2,
            items: vec![legacy_item(1, "legacy dpapi text")],
        };
        fs::write(
            store_path.join("history.json"),
            serde_json::to_vec(&metadata).expect("encode"),
        )
        .expect("history");

        let mut store = ClipboardStore::open(&store_path, CapturePolicy::default(), vault)
            .expect("legacy migration");
        let new_alias = store.alias_for(1);
        let decrypted = store.decrypt_item(1).expect("decrypt migrated secret");
        match &decrypted.content {
            ClipboardContent::Text(value) => assert_eq!(value, "legacy dpapi text"),
            ClipboardContent::FileList(_) => panic!("wrong content"),
        }

        let verifier = Vault::open(&vault_path).expect("second vault handle");
        let aliases = verifier.list_secrets().expect("list");
        assert!(aliases.contains(&new_alias));
        assert!(!aliases.contains(&legacy_alias_for(1)));
    }

    #[test]
    fn store_identity_survives_reopen_and_decrypts_existing_items() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = MemorySealBackend::new();
        let mut store = ClipboardStore::open_with_backend(
            dir.path(),
            CapturePolicy::default(),
            Box::new(backend.clone()),
        )
        .expect("store");
        let report = store.capture(text(1, "identity"), 1).expect("capture");
        let CaptureReport::Stored(record) = report else {
            panic!("stored");
        };
        drop(store);

        let mut reopened = ClipboardStore::open_with_backend(
            dir.path(),
            CapturePolicy::default(),
            Box::new(backend),
        )
        .expect("reopen");
        let decrypted = reopened.decrypt_item(record.item.id).expect("decrypt");
        match &decrypted.content {
            ClipboardContent::Text(value) => assert_eq!(value, "identity"),
            ClipboardContent::FileList(_) => panic!("wrong content"),
        }
    }

    #[test]
    fn a_second_open_is_already_locked_and_drop_releases_the_lock() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = MemorySealBackend::new();
        let store = ClipboardStore::open_with_backend(
            dir.path(),
            CapturePolicy::default(),
            Box::new(backend.clone()),
        )
        .expect("first store");
        let error = ClipboardStore::open_with_backend(
            dir.path(),
            CapturePolicy::default(),
            Box::new(backend.clone()),
        )
        .expect_err("second open must fail");
        assert!(matches!(error, ClipboardError::AlreadyLocked));
        drop(store);
        ClipboardStore::open_with_backend(dir.path(), CapturePolicy::default(), Box::new(backend))
            .expect("lock released on drop");
    }

    #[test]
    fn errors_and_reports_never_echo_sensitive_fixtures() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (mut store, _) = open_store(dir.path(), CapturePolicy::default());
        let sentinel = "password=unique-message-sentinel";
        let report = store.capture(text(1, sentinel), 1).expect("capture");
        let rendered = format!("{report} {report:?}");
        assert!(!rendered.contains("unique-message-sentinel"));
        assert_eq!(
            report,
            CaptureReport::Skipped(CaptureRejection::SensitiveContent)
        );

        let error = store.decrypt_item(404).expect_err("missing");
        let rendered = format!("{error} {error:?}");
        assert!(!rendered.to_ascii_lowercase().contains("sentinel"));
    }

    #[cfg(windows)]
    #[test]
    fn vault_backend_round_trip_never_writes_plaintext() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = Vault::open(dir.path().join("vault")).expect("vault");
        let mut store =
            ClipboardStore::open(dir.path().join("store"), CapturePolicy::default(), vault)
                .expect("store");
        let sentinel = "vault sentinel 5c1d9e";
        let report = store.capture(text(1, sentinel), 1).expect("capture");
        let CaptureReport::Stored(record) = report else {
            panic!("stored");
        };
        assert_no_sentinel(dir.path(), sentinel);
        let decrypted = store.decrypt_item(record.item.id).expect("decrypt");
        match &decrypted.content {
            ClipboardContent::Text(value) => assert_eq!(value, sentinel),
            ClipboardContent::FileList(_) => panic!("wrong content"),
        }

        let secret = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        assert_eq!(
            store.capture(text(2, secret), 2).expect("capture"),
            CaptureReport::Skipped(CaptureRejection::SensitiveContent)
        );
        assert_eq!(store.items().len(), 1);

        let marked = text(3, "marked fixture").with_markers(ExclusionMarkers::new(
            Some(false),
            None,
            false,
            false,
        ));
        assert_eq!(
            store.capture(marked, 3).expect("capture"),
            CaptureReport::Skipped(CaptureRejection::HistoryMarker)
        );
        assert_eq!(store.items().len(), 1);

        let verifier = Vault::open(dir.path().join("vault")).expect("second vault handle");
        assert_eq!(verifier.list_secrets().expect("list").len(), 1);
        assert_no_sentinel(dir.path(), secret);
        assert_no_sentinel(dir.path(), "marked fixture");
    }
}
