//! Clipboard capture, filtering, sealed history and checked insertion.
//!
//! # What this crate does
//!
//! * [`read_clipboard_snapshot`] reads bounded text, file lists, exclusion
//!   markers and best-effort source attribution on supported native desktops
//!   without writing to the clipboard.
//! * [`filter::evaluate`] rejects excluded, sensitive, oversized, truncated and
//!   gated content **before** any sealing, metadata write or preview entry. The
//!   Windows marker set includes `ExcludeClipboardContentFromMonitorProcessing`,
//!   `CanIncludeInClipboardHistory`, `CanUploadToCloudClipboard` and
//!   `Clipboard Viewer Ignore`.
//! * [`store::ClipboardStore`] seals accepted payloads with `ansa-vault`
//!   and persists only sealed envelopes plus bounded, non-sensitive
//!   metadata. Quotas are deterministic and pinned items survive eviction. A
//!   per-store manifest and non-blocking cross-process lock prevent two
//!   instances from losing captures or deleting each other's fresh payloads.
//!   Pre-namespace stores are migrated explicitly on open; if that migration is
//!   not safe, opening fails with [`ClipboardError::LegacyLayout`] rather than
//!   presenting undecryptable items.
//! * [`insertion::execute_insertion`] copies first and attempts an automatic
//!   paste only when explicitly enabled, the target still matches and the
//!   clipboard is still ours. Missing destination evidence is reported as
//!   `Unverified`, never as `Inserted`.
//!
//! # What this crate does not do
//!
//! * There is no plaintext persistence, plaintext fallback, WAL or log of
//!   clipboard content. [`store::MemorySealBackend`] is a hidden test double
//!   that performs no encryption and is never selected by the production
//!   constructor.
//! * Image capture is not implemented. It is modelled and reported as
//!   unsupported.
//! * Capture is pull-only: a host must call [`ClipboardStore::capture_current`]
//!   and forward OS lock events to the store. There is no self-copy suppression.
//! * Pause and private mode are session flags, not persisted settings. The
//!   caller must restore them and the capture policy after reopening a store.
//! * This crate does not ship a Windows automatic-paste host or a `SendInput`
//!   adapter. Copy-only is the only production insertion path today; the
//!   automatic transaction is policy code exercised by host fakes.
//! * DPAPI is user-bound, not passphrase-bound, and the macOS Keychain/Linux
//!   Secret Service key is likewise available according to OS/provider policy.
//!   [`store::ClipboardStore::lock`] is a session gate and a preview-cache wipe,
//!   not cryptographic re-authentication or protection from another process the
//!   OS/provider allows to read the key.
//!
//! # Filtering order
//!
//! Lock/pause/private-mode gate, native exclusion markers, configured
//! format/MIME/app exclusions, size and truncation bounds, then content-based
//! sensitive detection. App attribution is never the only filter because it can
//! be missing or wrong.
//!
//! # Example
//!
//! ```no_run
//! use ansa_clipboard::{CapturePolicy, ClipboardStore};
//! use ansa_vault::Vault;
//!
//! let Ok(vault) = Vault::open("vault") else {
//!     return;
//! };
//! let Ok(mut store) = ClipboardStore::open("history", CapturePolicy::default(), vault) else {
//!     return;
//! };
//! let _ = store.capture_current(0);
//! ```

#![deny(unsafe_op_in_unsafe_fn)]
#![deny(missing_debug_implementations)]

mod content;
mod error;
mod filter;
mod insertion;
#[cfg(target_os = "linux")]
mod linux;
mod lock;
#[cfg(target_os = "macos")]
mod macos;
mod model;
mod native;
mod store;
mod windows;

pub use content::ClipboardContent;
pub use error::ClipboardError;
pub use filter::{
    CaptureGate, CaptureRejection, ClipboardSnapshot, ExclusionMarkers, FilterOutcome,
    SnapshotContent, evaluate, is_sensitive_text,
};
pub use insertion::{
    ClipboardOwner, FocusOutcome, HostFailure, InsertionConfig, InsertionEvidence, InsertionHost,
    InsertionOutcome, InsertionPolicy, InsertionTarget, ObservedTarget, PasteAttempt, PasteTarget,
    TargetIssue, TargetPermission, TransactionPaste, execute_insertion,
};
pub use model::{
    ABSOLUTE_MAX_BYTES, ABSOLUTE_MAX_ITEM_BYTES, ABSOLUTE_MAX_ITEMS, CapturePolicy,
    ClipboardFormat, ClipboardHistory, ClipboardItem, DEFAULT_MAX_AGE_SECS, DEFAULT_MAX_BYTES,
    DEFAULT_MAX_ITEM_BYTES, DEFAULT_MAX_ITEMS, HistoryError, InsertedItems, ItemDraft, ItemSummary,
    MAX_POLICY_ENTRIES, MAX_POLICY_ENTRY_CHARS, MAX_SOURCE_APP_CHARS, SourceApp, SourceConfidence,
};
pub use native::{
    CaptureCapabilities, CaptureSupport, ClipboardTransport, clipboard_capture_capabilities,
    read_clipboard_snapshot,
};
#[doc(hidden)]
pub use store::MemorySealBackend;
pub use store::{
    CaptureRecord, CaptureReport, ClipboardStore, DecryptedItem, SealBackend, StoreState,
    VaultBackend,
};
pub use windows::{
    MAX_FILES, MAX_HDROP_UNITS, MAX_OPEN_ATTEMPTS, MAX_PATH_UNITS, MAX_TEXT_BYTES,
    OPEN_RETRY_DELAY_MS,
};
