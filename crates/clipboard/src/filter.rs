//! Capture filtering.
//!
//! Filtering runs before any sealing, metadata write or preview cache entry.
//! The order is deliberately conservative:
//!
//! 1. session gate (locked, paused, private mode);
//! 2. Windows exclusion markers;
//! 3. configured format/MIME/app exclusions;
//! 4. size and truncation bounds;
//! 5. content-based sensitive detection.
//!
//! Content detection always runs for accepted text and file-list items. App
//! attribution can be missing or wrong, so it is never the only secret filter:
//! a source that is not identified is still scanned.

use std::fmt;

use crate::content::{ClipboardContent, wipe_string};
use crate::model::{CapturePolicy, ClipboardFormat, ItemSummary, SourceApp, SourceConfidence};

/// Windows exclusion markers read alongside a clipboard snapshot.
///
/// Missing (`None`) means the marker format was absent. A present marker with
/// malformed data is reported as `Some(false)` by the adapter, which fails
/// closed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExclusionMarkers {
    /// `CanIncludeInClipboardHistory`: `Some(false)` means the producer asked
    /// that the content not enter local clipboard history.
    pub can_include_in_history: Option<bool>,
    /// `CanUploadToCloudClipboard`: `Some(false)` means the producer asked
    /// that the content not be uploaded to cloud clipboard services. Local
    /// capture honours this too unless
    /// [`CapturePolicy::capture_cloud_excluded`] is explicitly enabled.
    pub can_upload_to_cloud: Option<bool>,
    /// `Clipboard Viewer Ignore`: presence asks viewers not to observe the
    /// content. This build treats presence as a full capture exclusion.
    pub viewer_ignore: bool,
    /// `ExcludeClipboardContentFromMonitorProcessing`: presence asks every
    /// clipboard monitor to ignore all formats currently on the clipboard.
    /// This build treats presence as a full capture exclusion.
    pub exclude_from_monitor_processing: bool,
}

impl ExclusionMarkers {
    /// Construct markers for tests and adapters.
    #[must_use]
    pub fn new(
        can_include_in_history: Option<bool>,
        can_upload_to_cloud: Option<bool>,
        viewer_ignore: bool,
        exclude_from_monitor_processing: bool,
    ) -> Self {
        Self {
            can_include_in_history,
            can_upload_to_cloud,
            viewer_ignore,
            exclude_from_monitor_processing,
        }
    }

    /// Whether the producer explicitly opted out of local history.
    #[must_use]
    pub fn is_history_opt_out(self) -> bool {
        self.viewer_ignore
            || self.exclude_from_monitor_processing
            || self.can_include_in_history == Some(false)
    }

    /// Whether the producer explicitly opted out of cloud upload.
    #[must_use]
    pub fn is_cloud_opt_out(self) -> bool {
        self.can_upload_to_cloud == Some(false)
    }
}

/// Raw clipboard content as observed by a platform adapter.
///
/// This type can hold plaintext temporarily. Its `Debug` implementation prints
/// only shape information.
#[derive(PartialEq, Eq)]
pub enum SnapshotContent {
    /// Unicode text.
    Text(String),
    /// File paths from a file-list format.
    FileList(Vec<String>),
    /// An image that this build cannot capture, with a non-sensitive reason.
    Image {
        /// Why image capture is unavailable.
        reason: String,
    },
}

impl fmt::Debug for SnapshotContent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(value) => formatter
                .debug_struct("SnapshotContent::Text")
                .field("characters", &value.chars().count())
                .finish_non_exhaustive(),
            Self::FileList(paths) => formatter
                .debug_struct("SnapshotContent::FileList")
                .field("files", &paths.len())
                .finish_non_exhaustive(),
            Self::Image { reason } => formatter
                .debug_struct("SnapshotContent::Image")
                .field("reason", reason)
                .finish(),
        }
    }
}

impl SnapshotContent {
    /// Payload size in bytes, including a small file-list framing allowance.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        match self {
            Self::Text(value) => value.len(),
            Self::FileList(paths) => paths.iter().fold(0usize, |total, path| {
                total.saturating_add(path.len()).saturating_add(4)
            }),
            Self::Image { .. } => 0,
        }
    }

    /// Number of Unicode scalar values in text content.
    #[must_use]
    pub fn character_count(&self) -> Option<usize> {
        match self {
            Self::Text(value) => Some(value.chars().count()),
            Self::FileList(_) | Self::Image { .. } => None,
        }
    }

    /// Number of paths in file-list content.
    #[must_use]
    pub fn file_count(&self) -> Option<usize> {
        match self {
            Self::FileList(paths) => Some(paths.len()),
            Self::Text(_) | Self::Image { .. } => None,
        }
    }

    /// The modelled format.
    #[must_use]
    pub fn format(&self) -> ClipboardFormat {
        match self {
            Self::Text(_) => ClipboardFormat::Text,
            Self::FileList(_) => ClipboardFormat::FileList,
            Self::Image { .. } => ClipboardFormat::Image,
        }
    }

    /// Convert accepted snapshot content into decrypted content.
    ///
    /// This copies the bounded payload. The snapshot's own buffer is wiped by
    /// its `Drop` implementation, and the returned content wipes separately.
    #[must_use]
    pub fn into_content(&self) -> Option<ClipboardContent> {
        match self {
            Self::Text(value) => Some(ClipboardContent::Text(value.clone())),
            Self::FileList(paths) => Some(ClipboardContent::FileList(paths.clone())),
            Self::Image { .. } => None,
        }
    }

    /// Shape summary safe to persist.
    #[must_use]
    pub fn summary(&self) -> ItemSummary {
        match self {
            Self::Text(value) => ItemSummary::Text {
                characters: value.chars().count(),
            },
            Self::FileList(paths) => ItemSummary::FileList { files: paths.len() },
            Self::Image { .. } => ItemSummary::Image,
        }
    }

    /// Whether there is no capturable payload.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Text(value) => value.is_empty(),
            Self::FileList(paths) => paths.is_empty() || paths.iter().all(String::is_empty),
            Self::Image { .. } => false,
        }
    }
}

impl Drop for SnapshotContent {
    fn drop(&mut self) {
        match self {
            Self::Text(value) => wipe_string(value),
            Self::FileList(paths) => {
                for path in paths {
                    wipe_string(path);
                }
            }
            Self::Image { .. } => {}
        }
    }
}

/// One clipboard observation from a platform adapter.
#[derive(PartialEq, Eq)]
pub struct ClipboardSnapshot {
    /// Windows clipboard sequence number, or `0` when unavailable.
    pub sequence: u64,
    /// Observed content.
    pub content: SnapshotContent,
    /// Producer exclusion markers.
    pub markers: ExclusionMarkers,
    /// Best-effort source application, if one was observed.
    pub source: Option<SourceApp>,
    /// Confidence in `source`.
    pub confidence: SourceConfidence,
    /// MIME type reported by the adapter, if any.
    pub mime_type: Option<String>,
    /// Whether the adapter had to stop at its size bound. Truncated content is
    /// rejected rather than scanned incompletely.
    pub truncated: bool,
}

impl fmt::Debug for ClipboardSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClipboardSnapshot")
            .field("sequence", &self.sequence)
            .field("content", &self.content)
            .field("markers", &self.markers)
            .field("source", &self.source)
            .field("confidence", &self.confidence)
            .field("mime_type", &self.mime_type)
            .field("truncated", &self.truncated)
            .finish()
    }
}

impl ClipboardSnapshot {
    /// Construct a text snapshot with unknown attribution and no markers.
    #[must_use]
    pub fn text(sequence: u64, value: impl Into<String>) -> Self {
        Self {
            sequence,
            content: SnapshotContent::Text(value.into()),
            markers: ExclusionMarkers::default(),
            source: None,
            confidence: SourceConfidence::Unknown,
            mime_type: None,
            truncated: false,
        }
    }

    /// Construct a file-list snapshot with unknown attribution and no markers.
    #[must_use]
    pub fn file_list(sequence: u64, paths: Vec<String>) -> Self {
        Self {
            sequence,
            content: SnapshotContent::FileList(paths),
            markers: ExclusionMarkers::default(),
            source: None,
            confidence: SourceConfidence::Unknown,
            mime_type: None,
            truncated: false,
        }
    }

    /// Construct an unsupported-image snapshot.
    #[must_use]
    pub fn image(sequence: u64, reason: impl Into<String>) -> Self {
        Self {
            sequence,
            content: SnapshotContent::Image {
                reason: reason.into(),
            },
            markers: ExclusionMarkers::default(),
            source: None,
            confidence: SourceConfidence::Unknown,
            mime_type: None,
            truncated: false,
        }
    }

    /// Attach exclusion markers.
    #[must_use]
    pub fn with_markers(mut self, markers: ExclusionMarkers) -> Self {
        self.markers = markers;
        self
    }

    /// Attach a source application and its confidence.
    #[must_use]
    pub fn with_source(mut self, source: SourceApp, confidence: SourceConfidence) -> Self {
        self.source = Some(source);
        self.confidence = confidence;
        self
    }

    /// Attach a MIME type.
    #[must_use]
    pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = Some(mime_type.into());
        self
    }

    /// Mark the observation as truncated by the adapter bound.
    #[must_use]
    pub fn truncated(mut self) -> Self {
        self.truncated = true;
        self
    }

    /// Payload size in bytes.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.content.byte_len()
    }

    /// The modelled format.
    #[must_use]
    pub fn format(&self) -> ClipboardFormat {
        self.content.format()
    }
}

/// Session gate applied before any other filtering.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaptureGate {
    /// The store is locked (vault lock or explicit lock).
    pub locked: bool,
    /// The user paused capture.
    pub paused: bool,
    /// The user enabled private mode.
    pub private_mode: bool,
}

/// Why a snapshot was not captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureRejection {
    /// Capture is stopped because the store is locked.
    Locked,
    /// Capture is paused.
    Paused,
    /// Private mode is enabled.
    PrivateMode,
    /// No text or file paths were present.
    Empty,
    /// The item exceeds `CapturePolicy::max_item_bytes`.
    TooLarge {
        /// Observed bytes.
        bytes: usize,
        /// Configured limit.
        limit: usize,
    },
    /// `CanIncludeInClipboardHistory = 0`.
    HistoryMarker,
    /// `CanUploadToCloudClipboard = 0` with local capture of cloud-excluded
    /// content disabled.
    CloudMarker,
    /// `Clipboard Viewer Ignore` was present.
    ViewerIgnore,
    /// `ExcludeClipboardContentFromMonitorProcessing` was present.
    MonitorProcessingExcluded,
    /// The format is excluded by policy.
    FormatExcluded(ClipboardFormat),
    /// The MIME type is excluded by policy.
    MimeExcluded,
    /// The source application is excluded by policy.
    AppExcluded,
    /// Content matched a configured or built-in sensitive-content rule.
    SensitiveContent,
    /// Image capture is not implemented in this build.
    ImageUnsupported,
    /// The adapter truncated the observation at its bound.
    Truncated,
    /// Pinned items alone keep the quota exhausted.
    QuotaExhausted,
}

impl CaptureRejection {
    /// Stable label for logs and tests. Contains no clipboard content.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Locked => "locked",
            Self::Paused => "paused",
            Self::PrivateMode => "private-mode",
            Self::Empty => "empty",
            Self::TooLarge { .. } => "too-large",
            Self::HistoryMarker => "history-marker",
            Self::CloudMarker => "cloud-marker",
            Self::ViewerIgnore => "viewer-ignore",
            Self::MonitorProcessingExcluded => "monitor-processing-excluded",
            Self::FormatExcluded(_) => "format-excluded",
            Self::MimeExcluded => "mime-excluded",
            Self::AppExcluded => "app-excluded",
            Self::SensitiveContent => "sensitive-content",
            Self::ImageUnsupported => "image-unsupported",
            Self::Truncated => "truncated",
            Self::QuotaExhausted => "quota-exhausted",
        }
    }
}

impl fmt::Display for CaptureRejection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Locked => formatter.write_str("clipboard capture is locked"),
            Self::Paused => formatter.write_str("clipboard capture is paused"),
            Self::PrivateMode => formatter.write_str("private mode is enabled"),
            Self::Empty => formatter.write_str("clipboard had no text or file paths"),
            Self::TooLarge { bytes, limit } => {
                write!(formatter, "clipboard item is {bytes} bytes (limit {limit})")
            }
            Self::HistoryMarker => {
                formatter.write_str("the producer excluded the item from clipboard history")
            }
            Self::CloudMarker => {
                formatter.write_str("the producer excluded the item from cloud clipboard")
            }
            Self::ViewerIgnore => formatter.write_str("the producer marked the item viewer-ignore"),
            Self::MonitorProcessingExcluded => {
                formatter.write_str("the producer excluded the item from all clipboard monitors")
            }
            Self::FormatExcluded(format) => {
                write!(formatter, "the {format} format is excluded by policy")
            }
            Self::MimeExcluded => formatter.write_str("the MIME type is excluded by policy"),
            Self::AppExcluded => {
                formatter.write_str("the source application is excluded by policy")
            }
            Self::SensitiveContent => {
                formatter.write_str("content matched a sensitive-content rule")
            }
            Self::ImageUnsupported => {
                formatter.write_str("image capture is not implemented in this build")
            }
            Self::Truncated => {
                formatter.write_str("content exceeded the adapter bound and was not captured")
            }
            Self::QuotaExhausted => {
                formatter.write_str("pinned items keep the clipboard quota exhausted")
            }
        }
    }
}

/// Result of filtering one snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOutcome {
    /// The snapshot may be sealed and stored.
    Accept,
    /// The snapshot must not be sealed, stored, logged with content or
    /// previewed.
    Reject(CaptureRejection),
}

impl FilterOutcome {
    /// Whether the snapshot was accepted.
    #[must_use]
    pub fn is_accept(self) -> bool {
        matches!(self, Self::Accept)
    }
}

/// Apply the session gate and every capture filter.
#[must_use]
pub fn evaluate(
    snapshot: &ClipboardSnapshot,
    policy: &CapturePolicy,
    gate: CaptureGate,
) -> FilterOutcome {
    if gate.locked {
        return FilterOutcome::Reject(CaptureRejection::Locked);
    }
    if gate.paused {
        return FilterOutcome::Reject(CaptureRejection::Paused);
    }
    if gate.private_mode {
        return FilterOutcome::Reject(CaptureRejection::PrivateMode);
    }

    if snapshot.markers.viewer_ignore {
        return FilterOutcome::Reject(CaptureRejection::ViewerIgnore);
    }
    if snapshot.markers.exclude_from_monitor_processing {
        return FilterOutcome::Reject(CaptureRejection::MonitorProcessingExcluded);
    }
    if snapshot.markers.can_include_in_history == Some(false) {
        return FilterOutcome::Reject(CaptureRejection::HistoryMarker);
    }
    if snapshot.markers.is_cloud_opt_out() && !policy.capture_cloud_excluded {
        return FilterOutcome::Reject(CaptureRejection::CloudMarker);
    }

    let format = snapshot.format();
    if policy.excludes_format(format) {
        return FilterOutcome::Reject(CaptureRejection::FormatExcluded(format));
    }
    if format == ClipboardFormat::Image {
        return FilterOutcome::Reject(CaptureRejection::ImageUnsupported);
    }
    if snapshot.truncated {
        return FilterOutcome::Reject(CaptureRejection::Truncated);
    }
    if snapshot.content.is_empty() {
        return FilterOutcome::Reject(CaptureRejection::Empty);
    }

    let bytes = snapshot.byte_len();
    if bytes > policy.max_item_bytes {
        return FilterOutcome::Reject(CaptureRejection::TooLarge {
            bytes,
            limit: policy.max_item_bytes,
        });
    }

    if let Some(mime_type) = snapshot.mime_type.as_deref()
        && policy.excludes_mime_type(mime_type)
    {
        return FilterOutcome::Reject(CaptureRejection::MimeExcluded);
    }

    // App attribution is checked only when it exists. It is never required and
    // never the only filter: content is scanned below regardless of source.
    if let Some(source) = snapshot.source.as_ref()
        && policy.excludes_app(source)
    {
        return FilterOutcome::Reject(CaptureRejection::AppExcluded);
    }

    let sensitive = match &snapshot.content {
        SnapshotContent::Text(value) => is_sensitive_text(value, policy),
        SnapshotContent::FileList(paths) => paths
            .iter()
            .any(|path| is_sensitive_text(path, policy) || looks_like_credential_path(path)),
        SnapshotContent::Image { .. } => false,
    };
    if sensitive {
        return FilterOutcome::Reject(CaptureRejection::SensitiveContent);
    }

    FilterOutcome::Accept
}

/// Built-in and configured sensitive-content detection.
///
/// This is a conservative heuristic, not a proof. It rejects private-key
/// blocks, common API token prefixes, credential assignments (including JSON
/// `"key": "value"` and quoted `KEY="value"` forms), payment-card numbers that
/// pass Luhn, US Social Security numbers and configured literal markers.
/// False positives are preferred to silent capture; Luhn is a checksum, not
/// card-issuer validation, so a 13–19 digit order/account number has a small
/// chance of being rejected.
///
/// Configured markers and text are compared after Unicode-aware lowercasing,
/// so non-ASCII markers such as Cyrillic uppercase text match consistently.
/// This is lowercase matching rather than full Unicode case folding.
#[must_use]
pub fn is_sensitive_text(value: &str, policy: &CapturePolicy) -> bool {
    let mut lower = value.to_lowercase();
    let sensitive = is_sensitive_lower(value, &lower, policy);
    crate::content::wipe_string(&mut lower);
    sensitive
}

fn is_sensitive_lower(original: &str, lower: &str, policy: &CapturePolicy) -> bool {
    if policy
        .sensitive_markers
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return true;
    }
    if lower.contains("private key-----") && lower.contains("-----begin") {
        return true;
    }
    if contains_token_prefix(original) || contains_jwt(original) {
        return true;
    }
    if contains_sensitive_assignment(lower) {
        return true;
    }
    if contains_payment_card(original) || contains_ssn(original.as_bytes()) {
        return true;
    }
    false
}

fn contains_token_prefix(value: &str) -> bool {
    const PREFIXES: [&str; 10] = [
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "github_pat_",
        "sk-",
        "xoxb-",
        "xoxp-",
        "AKIA",
        "ASIA",
    ];
    for prefix in PREFIXES {
        let mut from = 0;
        while let Some(relative) = value[from..].find(prefix) {
            let start = from + relative + prefix.len();
            let token_len = value[start..]
                .chars()
                .take_while(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
                })
                .count();
            if token_len >= 12 {
                return true;
            }
            from = start;
        }
    }
    false
}

fn contains_jwt(value: &str) -> bool {
    value
        .split(|character: char| {
            character.is_whitespace() || matches!(character, '"' | '\'' | ',' | ';' | '(' | ')')
        })
        .any(|token| {
            token.starts_with("eyJ")
                && token.split('.').count() == 3
                && token.split('.').all(|part| part.len() >= 10)
        })
}

fn contains_sensitive_assignment(lower: &str) -> bool {
    const KEYS: [&str; 9] = [
        "password",
        "passwd",
        "secret",
        "api_key",
        "apikey",
        "access_token",
        "auth_token",
        "client_secret",
        "private_key",
    ];
    for key in KEYS {
        let mut from = 0;
        while let Some(relative) = lower[from..].find(key) {
            let after_key = from + relative + key.len();
            let rest = lower[after_key..].trim_start();
            // JSON object form: `"password": "value"` leaves a quote between
            // the key and the separator.
            let rest = rest
                .strip_prefix('"')
                .or_else(|| rest.strip_prefix('\''))
                .unwrap_or(rest)
                .trim_start();
            let Some(rest) = rest.strip_prefix('=').or_else(|| rest.strip_prefix(':')) else {
                from = after_key;
                continue;
            };
            let rest = rest.trim_start();
            let rest = rest
                .strip_prefix('"')
                .or_else(|| rest.strip_prefix('\''))
                .or_else(|| rest.strip_prefix('`'))
                .unwrap_or(rest);
            let mut candidate: String = rest
                .chars()
                .take_while(|character| {
                    !character.is_whitespace()
                        && !matches!(
                            character,
                            '"' | '\'' | '`' | ',' | ';' | ')' | ']' | '}' | '<'
                        )
                })
                .collect();
            let matched = {
                let candidate = candidate.trim_matches(|character: char| {
                    matches!(character, '"' | '\'' | '`' | '<' | '>')
                });
                candidate.len() >= 6
                    && candidate
                        .chars()
                        .any(|character| character.is_ascii_alphanumeric())
                    && !is_placeholder(candidate)
            };
            crate::content::wipe_string(&mut candidate);
            if matched {
                return true;
            }
            from = after_key;
        }
    }
    false
}

fn is_placeholder(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "changeme" | "example" | "placeholder" | "redacted" | "null" | "none" | "dummy"
    )
}

fn contains_payment_card(value: &str) -> bool {
    let mut digits = Vec::new();
    for character in value.chars().chain(std::iter::once('\n')) {
        if let Some(digit) = character
            .to_digit(10)
            .filter(|_| character.is_ascii_digit())
        {
            digits.push(digit);
        } else if matches!(character, ' ' | '-') && !digits.is_empty() {
            // Common card separators do not end the candidate run.
        } else {
            if (13..=19).contains(&digits.len()) && luhn_valid(&digits) {
                return true;
            }
            digits.clear();
        }
    }
    false
}

fn luhn_valid(digits: &[u32]) -> bool {
    let mut sum = 0u32;
    let mut double = false;
    for digit in digits.iter().rev() {
        let mut value = *digit;
        if double {
            value *= 2;
            if value > 9 {
                value -= 9;
            }
        }
        sum = sum.saturating_add(value);
        double = !double;
    }
    sum.is_multiple_of(10)
}

fn contains_ssn(bytes: &[u8]) -> bool {
    bytes.windows(11).any(|window| {
        window[..3].iter().all(u8::is_ascii_digit)
            && window[3] == b'-'
            && window[4..6].iter().all(u8::is_ascii_digit)
            && window[6] == b'-'
            && window[7..11].iter().all(u8::is_ascii_digit)
            && &window[..3] != b"000"
            && &window[..3] != b"666"
    })
}

fn looks_like_credential_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("id_rsa")
        || lower.contains("id_ed25519")
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.contains("\\.ssh\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accepted(snapshot: &ClipboardSnapshot, policy: &CapturePolicy) -> bool {
        evaluate(snapshot, policy, CaptureGate::default()).is_accept()
    }

    fn rejection(snapshot: &ClipboardSnapshot, policy: &CapturePolicy) -> CaptureRejection {
        match evaluate(snapshot, policy, CaptureGate::default()) {
            FilterOutcome::Reject(reason) => reason,
            FilterOutcome::Accept => panic!("expected rejection"),
        }
    }

    #[test]
    fn session_gate_stops_capture_before_reading_content() {
        let snapshot = ClipboardSnapshot::text(1, "hello");
        let policy = CapturePolicy::default();
        for (gate, expected) in [
            (
                CaptureGate {
                    locked: true,
                    ..CaptureGate::default()
                },
                CaptureRejection::Locked,
            ),
            (
                CaptureGate {
                    paused: true,
                    ..CaptureGate::default()
                },
                CaptureRejection::Paused,
            ),
            (
                CaptureGate {
                    private_mode: true,
                    ..CaptureGate::default()
                },
                CaptureRejection::PrivateMode,
            ),
        ] {
            assert_eq!(
                evaluate(&snapshot, &policy, gate),
                FilterOutcome::Reject(expected)
            );
        }
    }

    #[test]
    fn windows_exclusion_markers_are_honoured() {
        let policy = CapturePolicy::default();
        let marker = |history: Option<bool>, cloud: Option<bool>, viewer: bool| {
            ClipboardSnapshot::text(1, "hello")
                .with_markers(ExclusionMarkers::new(history, cloud, viewer, false))
        };
        assert_eq!(
            rejection(&marker(Some(false), None, false), &policy),
            CaptureRejection::HistoryMarker
        );
        assert_eq!(
            rejection(&marker(None, Some(false), false), &policy),
            CaptureRejection::CloudMarker
        );
        assert_eq!(
            rejection(&marker(None, None, true), &policy),
            CaptureRejection::ViewerIgnore
        );
        assert!(accepted(&marker(Some(true), Some(true), false), &policy));
    }

    #[test]
    fn cloud_exclusion_can_be_explicitly_captured() {
        let policy = CapturePolicy {
            capture_cloud_excluded: true,
            ..CapturePolicy::default()
        };
        let snapshot = ClipboardSnapshot::text(1, "hello").with_markers(ExclusionMarkers::new(
            None,
            Some(false),
            false,
            false,
        ));
        assert!(accepted(&snapshot, &policy));
    }

    #[test]
    fn monitor_processing_exclusion_blocks_capture_entirely() {
        let snapshot = ClipboardSnapshot::text(1, "hello")
            .with_markers(ExclusionMarkers::new(None, None, false, true));
        assert_eq!(
            rejection(&snapshot, &CapturePolicy::default()),
            CaptureRejection::MonitorProcessingExcluded
        );
        assert!(snapshot.markers.is_history_opt_out());
    }

    #[test]
    fn configured_app_mime_and_format_exclusions_apply() {
        let policy = CapturePolicy {
            excluded_apps: vec!["keepass".into()],
            excluded_mime_types: vec!["text/html".into()],
            excluded_formats: vec![ClipboardFormat::FileList],
            ..CapturePolicy::default()
        }
        .normalized();

        let app = ClipboardSnapshot::text(1, "hello").with_source(
            SourceApp::new("KeePass").expect("name"),
            SourceConfidence::Probable,
        );
        assert_eq!(rejection(&app, &policy), CaptureRejection::AppExcluded);

        let mime = ClipboardSnapshot::text(2, "hello").with_mime_type("text/HTML");
        assert_eq!(rejection(&mime, &policy), CaptureRejection::MimeExcluded);
        assert!(policy.excludes_mime_type("text/html"));
        assert!(!policy.excludes_mime_type("image/png"));

        let wildcard = CapturePolicy {
            excluded_mime_types: vec!["text/*".into()],
            ..CapturePolicy::default()
        }
        .normalized();
        assert!(wildcard.excludes_mime_type("text/plain"));
        assert!(!wildcard.excludes_mime_type("image/png"));

        let files = ClipboardSnapshot::file_list(3, vec!["C:\\a.txt".into()]);
        assert_eq!(
            rejection(&files, &policy),
            CaptureRejection::FormatExcluded(ClipboardFormat::FileList)
        );
    }

    #[test]
    fn missing_attribution_does_not_bypass_content_filtering() {
        let policy = CapturePolicy {
            excluded_apps: vec!["keepass".into()],
            ..CapturePolicy::default()
        }
        .normalized();
        let secret = ClipboardSnapshot::text(1, "password=hunter2hunter2");
        assert_eq!(
            rejection(&secret, &policy),
            CaptureRejection::SensitiveContent
        );
    }

    #[test]
    fn size_empty_and_truncation_rules_are_applied() {
        let policy = CapturePolicy {
            max_item_bytes: 8,
            ..CapturePolicy::default()
        }
        .normalized();
        assert_eq!(
            rejection(&ClipboardSnapshot::text(1, ""), &policy),
            CaptureRejection::Empty
        );
        assert_eq!(
            rejection(&ClipboardSnapshot::text(2, "nine bytes"), &policy),
            CaptureRejection::TooLarge {
                bytes: 10,
                limit: 8
            }
        );
        assert_eq!(
            rejection(&ClipboardSnapshot::text(3, "hi").truncated(), &policy),
            CaptureRejection::Truncated
        );
        assert_eq!(
            rejection(&ClipboardSnapshot::image(4, "unsupported"), &policy),
            CaptureRejection::ImageUnsupported
        );
    }

    #[test]
    fn sensitive_fixtures_are_rejected_before_persistence() {
        let policy = CapturePolicy::default();
        let fixtures = [
            "-----BEGIN RSA PRIVATE KEY-----\nMIIE...",
            "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
            "password=correct-horse-battery-staple",
            "authorization: bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.signature123",
            "4111 1111 1111 1111",
            "078-05-1120",
        ];
        for (index, fixture) in fixtures.iter().enumerate() {
            let snapshot = ClipboardSnapshot::text(index as u64 + 1, *fixture);
            assert_eq!(
                rejection(&snapshot, &policy),
                CaptureRejection::SensitiveContent,
                "fixture {index} was not rejected"
            );
        }
    }

    #[test]
    fn configured_sensitive_markers_are_case_insensitive_for_unicode() {
        let policy = CapturePolicy {
            sensitive_markers: vec!["ПАРОЛЬ".into()],
            ..CapturePolicy::default()
        }
        .normalized();
        for (index, value) in ["ПАРОЛЬ", "ПарОль", "пароль"].iter().enumerate() {
            assert!(
                is_sensitive_text(value, &policy),
                "unicode marker did not match {value}"
            );
            assert_eq!(
                rejection(&ClipboardSnapshot::text(index as u64 + 1, *value), &policy),
                CaptureRejection::SensitiveContent
            );
        }
        assert!(!is_sensitive_text("ordinary text", &policy));
    }

    #[test]
    fn json_and_quoted_credential_forms_are_detected() {
        let policy = CapturePolicy::default();
        for (index, value) in [
            "{\"password\": \"hunter2-hunter2\"}",
            "{\"api_key\": \"abcd1234efgh5678\"}",
            "PASSWORD=\"hunter2-hunter2\"",
            "'client_secret': 'abcd1234efgh5678'",
            "password: \"correct horse\"",
        ]
        .iter()
        .enumerate()
        {
            assert!(
                is_sensitive_text(value, &policy),
                "credential form was not detected: {value}"
            );
            assert_eq!(
                rejection(&ClipboardSnapshot::text(index as u64 + 1, *value), &policy),
                CaptureRejection::SensitiveContent
            );
        }
    }

    #[test]
    fn configured_sensitive_markers_are_case_insensitive() {
        let policy = CapturePolicy {
            sensitive_markers: vec!["ACME-INTERNAL".into()],
            ..CapturePolicy::default()
        }
        .normalized();
        assert_eq!(
            rejection(&ClipboardSnapshot::text(1, "x acme-internal y"), &policy),
            CaptureRejection::SensitiveContent
        );
    }

    #[test]
    fn ordinary_text_is_accepted() {
        let policy = CapturePolicy::default();
        for (index, value) in [
            "hello world",
            "version 1.2.3",
            "The meeting is at 10:30",
            "SELECT * FROM users",
            "C:\\Users\\person\\notes.txt",
        ]
        .iter()
        .enumerate()
        {
            assert!(
                accepted(&ClipboardSnapshot::text(index as u64 + 1, *value), &policy),
                "{value} was rejected"
            );
        }
    }

    #[test]
    fn file_list_paths_are_scanned() {
        let policy = CapturePolicy::default();
        assert_eq!(
            rejection(
                &ClipboardSnapshot::file_list(1, vec!["C:\\Users\\me\\.ssh\\id_rsa".into()]),
                &policy
            ),
            CaptureRejection::SensitiveContent
        );
        assert!(accepted(
            &ClipboardSnapshot::file_list(2, vec!["C:\\Users\\me\\notes.txt".into()]),
            &policy
        ));
    }

    #[test]
    fn snapshot_content_conversion_copies_the_bounded_payload() {
        let snapshot = ClipboardSnapshot::text(1, "converted");
        let content = snapshot.content.into_content().expect("content");
        assert_eq!(content.preview(20), "converted");
        assert_eq!(content.byte_len(), "converted".len());
    }

    #[test]
    fn rejection_messages_never_echo_content() {
        let snapshot = ClipboardSnapshot::text(1, "password=supersecretvalue");
        let rejection = rejection(&snapshot, &CapturePolicy::default());
        assert!(!rejection.to_string().contains("supersecretvalue"));
        assert!(!format!("{snapshot:?}").contains("supersecretvalue"));
    }

    #[test]
    fn luhn_detection_is_not_applied_to_short_numbers() {
        assert!(!contains_payment_card("1234"));
        assert!(contains_payment_card("4111111111111111"));
        assert!(!contains_payment_card("4111111111111112"));
    }
}
