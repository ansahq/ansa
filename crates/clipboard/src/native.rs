//! Platform dispatch and portable offer validation.

#[cfg(windows)]
use crate::windows;
use crate::{ClipboardError, ClipboardSnapshot};

/// Desktop clipboard transport selected by the capture engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardTransport {
    Windows,
    MacOs,
    X11,
    Wayland,
    Unsupported,
}

/// Support for one format, independent of the current clipboard contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureSupport {
    Available,
    Degraded { reason: &'static str },
    Unsupported { reason: &'static str },
}

/// Platform adapter capabilities. These do not attest to vault availability,
/// operating-system access grants or successful transfer of a particular offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureCapabilities {
    pub transport: ClipboardTransport,
    pub text: CaptureSupport,
    pub file_list: CaptureSupport,
    pub image: CaptureSupport,
}

pub(crate) const IMAGE_UNSUPPORTED: &str = "image pixels are not read by this build";

/// Opaque, metadata-only generations. The high bit separates these from the
/// legacy Windows counter. Random process domains prevent native serial reuse
/// after a desktop/connection/process restart from hiding a different payload.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub(crate) fn next_generation() -> u64 {
    use std::hash::{BuildHasher, Hasher, RandomState};
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: OnceLock<AtomicU64> = OnceLock::new();
    let next = NEXT.get_or_init(|| AtomicU64::new(RandomState::new().build_hasher().finish()));
    (next.fetch_add(1, Ordering::Relaxed) & (u64::MAX >> 1)) | (1 << 63)
}

#[cfg(any(target_os = "macos", test))]
#[derive(Default)]
pub(crate) struct NativeSerial(Option<(isize, u64)>);

#[cfg(any(target_os = "macos", test))]
impl NativeSerial {
    #[cfg(target_os = "macos")]
    pub(crate) const fn new() -> Self {
        Self(None)
    }

    pub(crate) fn observe(&mut self, serial: isize) -> u64 {
        if let Some((previous, generation)) = self.0
            && serial == previous
        {
            return generation;
        }
        let generation = next_generation();
        self.0 = Some((serial, generation));
        generation
    }
}

/// A snapshot is usable only if its native selection generation did not move
/// while metadata and payload were obtained. Dropping `read` on a race wipes
/// the snapshot's owned payload instead of returning a partial observation.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub(crate) fn unchanged<T: Eq>(
    before: T,
    after: T,
    read: Result<ClipboardSnapshot, ClipboardError>,
) -> Result<ClipboardSnapshot, ClipboardError> {
    if before != after {
        return Err(ClipboardError::ClipboardChanged);
    }
    read
}

impl CaptureCapabilities {
    pub(crate) fn new(
        transport: ClipboardTransport,
        text: CaptureSupport,
        file_list: CaptureSupport,
    ) -> Self {
        Self {
            transport,
            text,
            file_list,
            image: CaptureSupport::Unsupported {
                reason: IMAGE_UNSUPPORTED,
            },
        }
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    pub(crate) fn unavailable(transport: ClipboardTransport, reason: &'static str) -> Self {
        let unsupported = CaptureSupport::Unsupported { reason };
        Self::new(transport, unsupported, unsupported)
    }
}

/// Report capabilities for the current platform/session, without reading any
/// clipboard payload. On Linux this probes the session's control protocol.
#[must_use]
pub fn clipboard_capture_capabilities() -> CaptureCapabilities {
    #[cfg(windows)]
    {
        CaptureCapabilities::new(
            ClipboardTransport::Windows,
            CaptureSupport::Available,
            CaptureSupport::Available,
        )
    }
    #[cfg(target_os = "macos")]
    {
        CaptureCapabilities::new(
            ClipboardTransport::MacOs,
            CaptureSupport::Available,
            CaptureSupport::Degraded {
                reason: "modern local file URLs only; legacy lists and file promises are not read",
            },
        )
    }
    #[cfg(target_os = "linux")]
    {
        crate::linux::capabilities()
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        CaptureCapabilities::unavailable(ClipboardTransport::Unsupported, "no native adapter")
    }
}

/// Read a bounded snapshot from the native clipboard. A protected offer may
/// return a payload-free snapshot carrying only its opt-out marker. Feed the
/// result to the normal capture filter; do not preview it directly.
///
/// Prefer [`crate::ClipboardStore::capture_current`] when capturing history, so
/// the session gate and configured offer exclusions can also prevent reads.
pub fn read_clipboard_snapshot() -> Result<ClipboardSnapshot, ClipboardError> {
    read(None)
}

pub(crate) fn read(
    policy: Option<&crate::CapturePolicy>,
) -> Result<ClipboardSnapshot, ClipboardError> {
    #[cfg(windows)]
    {
        let _ = policy;
        windows::read_clipboard_snapshot()
    }
    #[cfg(target_os = "macos")]
    {
        crate::macos::read(policy)
    }
    #[cfg(target_os = "linux")]
    {
        crate::linux::read(policy)
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = policy;
        Err(ClipboardError::UnsupportedPlatform {
            platform: std::env::consts::OS,
        })
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub(crate) mod offer {
    use crate::content::wipe_bytes;
    use crate::filter::offer_rejection;
    use crate::{
        CapturePolicy, ClipboardError, ClipboardSnapshot, ExclusionMarkers, SnapshotContent,
        SourceApp, SourceConfidence,
    };

    pub(super) use super::IMAGE_UNSUPPORTED;
    pub(crate) const MAX_OFFER_TYPES: usize = 256;
    pub(crate) const MAX_OFFER_TYPE_BYTES: usize = 256;
    pub(crate) const MAX_BYTES: usize = crate::windows::MAX_TEXT_BYTES;
    pub(crate) const MAX_FILES: usize = crate::windows::MAX_FILES as usize;
    pub(crate) const MAX_PATH_BYTES: usize = crate::windows::MAX_PATH_UNITS as usize;

    #[cfg(any(target_os = "linux", test))]
    pub(crate) const URI_LIST: &str = "text/uri-list";
    #[cfg(any(target_os = "macos", test))]
    pub(crate) const MAC_FILE: &str = "public.file-url";
    #[cfg(any(target_os = "macos", test))]
    pub(crate) const MAC_TEXT: &str = "public.utf8-plain-text";

    #[derive(Clone, Copy)]
    pub(crate) enum Dialect {
        #[cfg(any(target_os = "macos", test))]
        Mac,
        #[cfg(any(target_os = "linux", test))]
        Linux,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Format {
        Text,
        Files,
        Image,
    }

    fn marker(dialect: Dialect, name: &str) -> bool {
        let names: &[&str] = match dialect {
            #[cfg(any(target_os = "macos", test))]
            Dialect::Mac => &[
                "org.nspasteboard.ConcealedType",
                "org.nspasteboard.TransientType",
                "org.nspasteboard.AutoGeneratedType",
                "de.petermaurer.TransientPasteboardType",
                "com.agilebits.onepassword",
            ],
            #[cfg(any(target_os = "linux", test))]
            Dialect::Linux => &["x-kde-passwordManagerHint"],
        };
        names.iter().any(|known| name.eq_ignore_ascii_case(known))
    }

    pub(crate) fn add_type(types: &mut Vec<String>, name: &str) -> Result<(), ClipboardError> {
        if name.is_empty()
            || name.len() > MAX_OFFER_TYPE_BYTES
            || name.bytes().any(|b| b.is_ascii_control())
        {
            return Err(malformed("invalid native clipboard type"));
        }
        if !types.iter().any(|seen| seen == name) {
            if types.len() == MAX_OFFER_TYPES {
                return Err(limit("too many clipboard types"));
            }
            types.push(name.to_owned());
        }
        Ok(())
    }

    fn representation(dialect: Dialect, types: &[String]) -> Option<(Format, &str, &'static str)> {
        let find = |name: &str| types.iter().find(|value| value.eq_ignore_ascii_case(name));
        let mut images = types.iter();
        match dialect {
            #[cfg(any(target_os = "macos", test))]
            Dialect::Mac => {
                if let Some(name) = find(MAC_FILE) {
                    return Some((Format::Files, name, "text/uri-list"));
                }
                if let Some(name) = find(MAC_TEXT) {
                    return Some((Format::Text, name, "text/plain"));
                }
                images
                    .find(|name| mac_mime(name).is_some_and(|mime| mime.starts_with("image/")))
                    .map(|name| {
                        (
                            Format::Image,
                            name.as_str(),
                            mac_mime(name).unwrap_or("image/*"),
                        )
                    })
            }
            #[cfg(any(target_os = "linux", test))]
            Dialect::Linux => {
                if let Some(name) = find(URI_LIST) {
                    return Some((Format::Files, name, URI_LIST));
                }
                // Only UTF-8; legacy STRING (Latin-1) and COMPOUND_TEXT are not
                // guessed or passed through lossy decoding.
                if let Some(name) = types.iter().find(|name| utf8_mime(name)) {
                    return Some((Format::Text, name, "text/plain"));
                }
                for name in ["UTF8_STRING", "text/plain"] {
                    if let Some(name) = find(name) {
                        return Some((Format::Text, name, "text/plain"));
                    }
                }
                images
                    .find(|name| {
                        name.to_ascii_lowercase().starts_with("image/")
                            || ["PIXMAP", "BITMAP", "application/x-qt-image"]
                                .iter()
                                .any(|alias| name.eq_ignore_ascii_case(alias))
                    })
                    .map(|name| (Format::Image, name.as_str(), "image/*"))
            }
        }
    }

    #[cfg(any(target_os = "linux", test))]
    fn utf8_mime(name: &str) -> bool {
        let Some((mime, params)) = name.split_once(';') else {
            return false;
        };
        let Some((key, value)) = params.split_once('=') else {
            return false;
        };
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .unwrap_or(value);
        mime.trim().eq_ignore_ascii_case("text/plain")
            && key.trim().eq_ignore_ascii_case("charset")
            && value.eq_ignore_ascii_case("utf-8")
    }

    #[cfg(any(target_os = "macos", test))]
    fn mac_mime(name: &str) -> Option<&'static str> {
        // Map common advertised UTIs even when we do not read their data.
        Some(match name.to_ascii_lowercase().as_str() {
            "public.file-url" => "text/uri-list",
            "public.utf8-plain-text"
            | "public.utf16-plain-text"
            | "public.plain-text"
            | "public.utf16-external-plain-text" => "text/plain",
            "public.html" => "text/html",
            "public.rtf" => "text/rtf",
            "public.png" => "image/png",
            "public.jpeg" => "image/jpeg",
            "public.tiff" => "image/tiff",
            "public.heic" => "image/heic",
            "com.compuserve.gif" => "image/gif",
            "com.microsoft.bmp" => "image/bmp",
            "public.image" => "image/*",
            _ => return None,
        })
    }

    /// Construct a payload-free offer first; only call `receive` for a
    /// supported, permitted format. Never fallback after a failed read.
    pub(crate) fn snapshot(
        dialect: Dialect,
        types: &[String],
        sequence: u64,
        source: Option<SourceApp>,
        policy: Option<&CapturePolicy>,
        receive: impl FnOnce(Format, &str) -> Result<SnapshotContent, ClipboardError>,
    ) -> Result<ClipboardSnapshot, ClipboardError> {
        if types.iter().any(|name| marker(dialect, name)) {
            return Ok(
                ClipboardSnapshot::text(sequence, "").with_markers(ExclusionMarkers {
                    exclude_from_monitor_processing: true,
                    ..ExclusionMarkers::default()
                }),
            );
        }
        let (format, native_type, mime) =
            representation(dialect, types).ok_or(ClipboardError::NoSupportedFormat)?;
        let mut snapshot = match format {
            Format::Text => ClipboardSnapshot::text(sequence, ""),
            Format::Files => ClipboardSnapshot::file_list(sequence, Vec::new()),
            Format::Image => ClipboardSnapshot::image(sequence, IMAGE_UNSUPPORTED),
        }
        .with_mime_type(mime);
        if let Some(app) = source {
            snapshot = snapshot.with_source(app, SourceConfidence::Probable);
        }
        snapshot.offered_types.extend_from_slice(types);
        #[cfg(any(target_os = "macos", test))]
        if matches!(dialect, Dialect::Mac) {
            snapshot.offered_types.extend(
                types
                    .iter()
                    .filter_map(|name| mac_mime(name).map(String::from)),
            );
        }
        if format != Format::Image
            && !policy.is_some_and(|policy| offer_rejection(&snapshot, policy).is_some())
        {
            snapshot.content = receive(format, native_type)?;
        }
        Ok(snapshot)
    }

    pub(crate) fn text(bytes: &[u8]) -> Result<SnapshotContent, ClipboardError> {
        if bytes.len() > MAX_BYTES {
            return Err(limit("text bytes"));
        }
        if bytes.contains(&0) {
            return Err(malformed("NUL in UTF-8 text"));
        }
        let value = std::str::from_utf8(bytes).map_err(|_| malformed("invalid UTF-8"))?;
        Ok(SnapshotContent::Text(value.to_owned()))
    }

    #[cfg(any(target_os = "linux", test))]
    pub(crate) fn uri_list(bytes: &[u8]) -> Result<SnapshotContent, ClipboardError> {
        if bytes.len() > MAX_BYTES {
            return Err(limit("URI-list bytes"));
        }
        if bytes.contains(&0) {
            return Err(malformed("NUL in URI list"));
        }
        let lines = std::str::from_utf8(bytes).map_err(|_| malformed("invalid URI-list UTF-8"))?;
        let mut files = Files::new();
        for line in lines.split('\n') {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if !line.is_empty() && !line.starts_with('#') {
                files.push(line.as_bytes())?;
            }
        }
        Ok(files.finish())
    }

    pub(crate) struct Files {
        content: SnapshotContent,
        bytes: usize,
    }

    impl Files {
        pub(crate) fn new() -> Self {
            Self {
                content: SnapshotContent::FileList(Vec::new()),
                bytes: 0,
            }
        }

        pub(crate) fn push(&mut self, uri: &[u8]) -> Result<(), ClipboardError> {
            let SnapshotContent::FileList(paths) = &mut self.content else {
                unreachable!()
            };
            // Budget the wire bytes too, not just the decoded path strings.
            self.bytes = self.bytes.saturating_add(uri.len()).saturating_add(4);
            if paths.len() == MAX_FILES || self.bytes > MAX_BYTES {
                return Err(limit("file list count or bytes"));
            }
            paths.push(file_path(uri)?);
            Ok(())
        }

        pub(crate) fn finish(self) -> SnapshotContent {
            self.content
        }
    }

    fn file_path(uri: &[u8]) -> Result<String, ClipboardError> {
        let invalid = || malformed("expected a local file URI");
        let uri = std::str::from_utf8(uri).map_err(|_| invalid())?;
        let (scheme, rest) = uri.split_once(':').ok_or_else(invalid)?;
        if !scheme.eq_ignore_ascii_case("file") {
            return Err(invalid());
        }
        let mut path = rest;
        if let Some(after_slashes) = rest.strip_prefix("//") {
            let slash = after_slashes.find('/').ok_or_else(invalid)?;
            let authority = &after_slashes[..slash];
            if !authority.is_empty() && !authority.eq_ignore_ascii_case("localhost") {
                return Err(invalid());
            }
            path = &after_slashes[slash..];
        }
        if !path.starts_with('/') {
            return Err(invalid());
        }
        if path.len() > MAX_PATH_BYTES * 3 {
            return Err(limit("file URI bytes"));
        }
        let mut decoded = Scratch(Vec::with_capacity(path.len().min(MAX_PATH_BYTES)));
        let mut input = path.bytes();
        while let Some(byte) = input.next() {
            let byte = match byte {
                b'%' => {
                    let hi = input.next().and_then(hex).ok_or_else(invalid)?;
                    let lo = input.next().and_then(hex).ok_or_else(invalid)?;
                    (hi << 4) | lo
                }
                b'?' | b'#' | b'\\' | b' ' => return Err(invalid()),
                value => value,
            };
            if byte.is_ascii_control() {
                return Err(invalid());
            }
            if decoded.0.len() == MAX_PATH_BYTES {
                return Err(limit("decoded file path bytes"));
            }
            decoded.0.push(byte);
        }
        if decoded.0.starts_with(b"//") {
            return Err(invalid());
        }
        let path = std::str::from_utf8(&decoded.0).map_err(|_| invalid())?;
        Ok(path.to_owned())
    }

    fn hex(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    pub(crate) struct Scratch(pub(crate) Vec<u8>);

    impl Drop for Scratch {
        fn drop(&mut self) {
            wipe_bytes(&mut self.0);
        }
    }

    pub(crate) fn malformed(reason: &'static str) -> ClipboardError {
        ClipboardError::MalformedPayload { reason }
    }

    pub(crate) fn limit(reason: &'static str) -> ClipboardError {
        ClipboardError::CaptureLimit { reason }
    }
}

#[cfg(test)]
mod tests {
    use super::offer::{self, Dialect, Format, MAC_FILE, MAC_TEXT, MAX_BYTES, MAX_FILES, URI_LIST};
    use super::*;
    use crate::{
        CaptureGate, CapturePolicy, CaptureRejection, CaptureReport, ClipboardFormat,
        ClipboardStore, FilterOutcome, MemorySealBackend, SnapshotContent, SourceApp,
        SourceConfidence, evaluate,
    };

    fn names(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn store(dir: &std::path::Path, backend: MemorySealBackend) -> ClipboardStore {
        ClipboardStore::open_with_backend(dir, CapturePolicy::default(), Box::new(backend))
            .expect("fixture store")
    }

    #[test]
    fn protected_offers_never_call_the_provider_or_write_history() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = MemorySealBackend::new();
        let mut store = store(dir.path(), backend.clone());
        let fixtures = [
            (Dialect::Mac, MAC_TEXT, "org.nspasteboard.ConcealedType"),
            (Dialect::Mac, MAC_FILE, "org.nspasteboard.TransientType"),
            (Dialect::Mac, MAC_TEXT, "org.nspasteboard.AutoGeneratedType"),
            (Dialect::Linux, "text/plain", "x-kde-passwordManagerHint"),
            (Dialect::Linux, URI_LIST, "X-KDE-PASSWORDMANAGERHINT"),
        ];
        for (dialect, representation, marker) in fixtures {
            let snapshot = offer::snapshot(
                dialect,
                &names(&[representation, marker]),
                1,
                None,
                None,
                |_, _| panic!("privacy marker must block bytes before provider access"),
            )
            .expect("marker-only observation");
            assert_eq!(snapshot.byte_len(), 0);
            assert_eq!(snapshot.confidence, SourceConfidence::Unknown);
            assert_eq!(
                store.capture(snapshot, 10).expect("skipped"),
                CaptureReport::Skipped(CaptureRejection::MonitorProcessingExcluded)
            );
        }
        assert!(backend.is_empty());
        assert!(store.items().is_empty());
        assert_eq!(store.cached_preview_count(), 0);
        assert!(!dir.path().join("history.json").exists());
    }

    #[test]
    fn all_offered_types_and_best_effort_source_inform_filtering() {
        let fixtures = [
            (
                Dialect::Mac,
                names(&[MAC_TEXT, "public.html"]),
                CapturePolicy {
                    excluded_mime_types: names(&["text/html"]),
                    ..CapturePolicy::default()
                },
                CaptureRejection::MimeExcluded,
            ),
            (
                Dialect::Linux,
                names(&["UTF8_STRING", "text/html;charset=UTF-8"]),
                CapturePolicy {
                    excluded_mime_types: names(&["text/html"]),
                    ..CapturePolicy::default()
                },
                CaptureRejection::MimeExcluded,
            ),
            (
                Dialect::Linux,
                names(&[URI_LIST, "text/plain"]),
                CapturePolicy {
                    excluded_formats: vec![ClipboardFormat::FileList],
                    ..CapturePolicy::default()
                },
                CaptureRejection::FormatExcluded(ClipboardFormat::FileList),
            ),
            (
                Dialect::Mac,
                names(&[MAC_TEXT]),
                CapturePolicy {
                    excluded_apps: names(&["COM.EXAMPLE.APP"]),
                    ..CapturePolicy::default()
                },
                CaptureRejection::AppExcluded,
            ),
        ];
        for (dialect, types, policy, reason) in fixtures {
            let policy = policy.normalized();
            let snapshot = offer::snapshot(
                dialect,
                &types,
                1,
                SourceApp::new("com.example.app"),
                Some(&policy),
                |_, _| panic!("known excluded offer requested its payload"),
            )
            .expect("metadata-only snapshot");
            assert_eq!(snapshot.confidence, SourceConfidence::Probable);
            assert_eq!(
                evaluate(&snapshot, &policy, CaptureGate::default()),
                FilterOutcome::Reject(reason)
            );
        }
    }

    #[test]
    fn changed_generation_discards_bytes_but_unchanged_generation_is_usable() {
        use std::cell::Cell;

        let generation = Cell::new(91);
        let before = generation.get();
        let snapshot = offer::snapshot(
            Dialect::Mac,
            &names(&[MAC_TEXT]),
            before,
            None,
            None,
            |_, _| {
                generation.set(92);
                offer::text(b"old payload must not leave the adapter")
            },
        );
        assert!(matches!(
            unchanged(before, generation.get(), snapshot),
            Err(ClipboardError::ClipboardChanged)
        ));
        let received = ClipboardSnapshot::text(92, "new clipboard data");
        assert!(unchanged(92, 92, Ok(received)).is_ok());
    }

    #[test]
    fn a_payload_read_that_races_a_new_generation_is_discarded() {
        use std::cell::Cell;

        let dir = tempfile::tempdir().expect("tempdir");
        let backend = MemorySealBackend::new();
        let store = store(dir.path(), backend.clone());
        let generation = Cell::new(41);
        let before = generation.get();
        let provides = Cell::new(0);
        let snapshot = offer::snapshot(
            Dialect::Linux,
            &names(&[URI_LIST]),
            before,
            None,
            None,
            |format, _| {
                assert_eq!(format, Format::Files);
                provides.set(provides.get() + 1);
                // The user copies again while the provider is fetching bytes.
                generation.set(42);
                offer::uri_list(b"file:///tmp/ansa-raced-payload-sentinel")
            },
        );
        assert_eq!(provides.get(), 1);
        assert!(matches!(
            unchanged(before, generation.get(), snapshot),
            Err(ClipboardError::ClipboardChanged)
        ));
        assert!(backend.is_empty());
        assert!(store.items().is_empty());
        assert_eq!(store.cached_preview_count(), 0);
        assert!(!dir.path().join("history.json").exists());
    }

    #[test]
    fn marker_metadata_still_blocks_bytes_when_policy_also_excludes_the_offer() {
        let policy = CapturePolicy {
            excluded_mime_types: names(&["text/plain"]),
            excluded_apps: names(&["com.example.app"]),
            ..CapturePolicy::default()
        }
        .normalized();
        let snapshot = offer::snapshot(
            Dialect::Mac,
            &names(&[MAC_TEXT, "org.nspasteboard.ConcealedType"]),
            1,
            SourceApp::new("com.example.app"),
            Some(&policy),
            |_, _| panic!("marker or exclusion must block provider access"),
        )
        .expect("marker-only snapshot");
        assert_eq!(snapshot.byte_len(), 0);
        assert_eq!(
            evaluate(&snapshot, &policy, CaptureGate::default()),
            FilterOutcome::Reject(CaptureRejection::MonitorProcessingExcluded)
        );
    }

    #[test]
    fn native_serial_reuse_after_reset_does_not_hide_a_distinct_capture() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = MemorySealBackend::new();
        let mut first_session = NativeSerial::default();
        let first = first_session.observe(12);
        assert_eq!(first, first_session.observe(12));
        assert!(first > u64::from(u32::MAX));
        let mut store_instance = store(dir.path(), backend.clone());
        assert!(
            store_instance
                .capture(ClipboardSnapshot::text(first, "first session"), 1)
                .unwrap()
                .is_stored()
        );
        let first_id = store_instance.items()[0].id;
        drop(
            store_instance
                .preview(first_id, 40)
                .expect("create cached preview"),
        );
        store_instance.lock();
        assert_eq!(store_instance.cached_preview_count(), 0);
        store_instance.unlock().expect("unlock");
        assert_eq!(
            store_instance
                .capture(ClipboardSnapshot::text(first, "first session"), 2)
                .unwrap(),
            CaptureReport::Duplicate
        );
        drop(store_instance);
        let mut reopened = store(dir.path(), backend);
        let later = NativeSerial::default().observe(12);
        assert_ne!(first, later);
        assert!(
            reopened
                .capture(ClipboardSnapshot::text(later, "second session"), 3)
                .unwrap()
                .is_stored()
        );
        assert_ne!(first_session.observe(1), first);
        assert_ne!(first_session.observe(12), first);
    }

    #[test]
    fn mime_matrix_is_strict_and_does_not_read_images_or_fallback_on_errors() {
        let policy = CapturePolicy::default();
        let snapshot = offer::snapshot(
            Dialect::Linux,
            &names(&[
                "image/png",
                "text/plain",
                "UTF8_STRING",
                "text/plain; charset=\"UTF-8\"",
            ]),
            2,
            None,
            None,
            |format, name| {
                assert_eq!(
                    (format, name),
                    (Format::Text, "text/plain; charset=\"UTF-8\"")
                );
                offer::text("Café".as_bytes())
            },
        )
        .expect("choose UTF-8");
        assert_eq!(snapshot.confidence, SourceConfidence::Unknown);
        assert!(matches!(&snapshot.content, SnapshotContent::Text(value) if value == "Café"));
        for (dialect, names) in [
            (Dialect::Mac, names(&["public.tiff"])),
            (Dialect::Linux, names(&["PIXMAP"])),
        ] {
            let image = offer::snapshot(dialect, &names, 1, None, None, |_, _| {
                panic!("image request")
            })
            .expect("image unsupported snapshot");
            assert_eq!(
                evaluate(&image, &policy, CaptureGate::default()),
                FilterOutcome::Reject(CaptureRejection::ImageUnsupported)
            );
        }
        assert!(matches!(
            offer::snapshot(
                Dialect::Linux,
                &names(&["STRING", "text/plain;charset=latin1"]),
                1,
                None,
                None,
                |_, _| panic!("not UTF-8")
            ),
            Err(ClipboardError::NoSupportedFormat)
        ));
        assert!(matches!(
            offer::snapshot(
                Dialect::Linux,
                &names(&[URI_LIST, "text/plain"]),
                1,
                None,
                None,
                |format, _| {
                    assert_eq!(format, Format::Files);
                    offer::uri_list(b"file://remote-host/forbidden")
                }
            ),
            Err(ClipboardError::MalformedPayload { .. })
        ));
    }

    #[test]
    fn file_uri_decoding_is_all_or_nothing_and_preserves_the_sensitive_scan() {
        let contents = offer::uri_list(
            b"# comment\r\nfile:///tmp/ordinary%20name.txt\r\nfile://localhost/tmp/caf%C3%A9\n",
        )
        .unwrap();
        let SnapshotContent::FileList(paths) = &contents else {
            panic!("wrong format")
        };
        assert_eq!(paths, &["/tmp/ordinary name.txt", "/tmp/café"]);

        for invalid in [
            "file://remote/tmp/name",
            "https://example.test/name",
            "file:relative",
            "file:///tmp/bad%0Aname",
            "file:///tmp/bad%00name",
            "file:///tmp/bad%FF",
            "file:///tmp/bad%GG",
            "file:///tmp/bad#fragment",
            "file:///tmp/bad?query",
            "file:////srv/share",
            "file:///tmp/raw spaces",
            "file:///tmp/bad%",
        ] {
            // An otherwise legitimate first line must not escape when a
            // second line is unrepresentable or rejected.
            let list = format!("file:///tmp/ordinary\n{invalid}");
            assert!(matches!(
                offer::uri_list(list.as_bytes()),
                Err(ClipboardError::MalformedPayload { .. })
            ));
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let backend = MemorySealBackend::new();
        let mut store = store(dir.path(), backend.clone());
        let snapshot = offer::snapshot(
            Dialect::Linux,
            &names(&[URI_LIST]),
            1,
            None,
            None,
            |_, _| offer::uri_list(b"file:///tmp/%2essh/unusual-name-sentinel"),
        )
        .expect("valid syntax");
        assert_eq!(
            store.capture(snapshot, 1).unwrap(),
            CaptureReport::Skipped(CaptureRejection::SensitiveContent)
        );
        assert!(backend.is_empty());
        assert!(store.items().is_empty());
        assert_eq!(store.cached_preview_count(), 0);
        assert!(!dir.path().join("history.json").exists());
    }

    #[test]
    fn adapter_bounds_and_utf8_errors_fail_closed_without_truncation() {
        assert!(offer::text(&vec![b'a'; MAX_BYTES]).is_ok());
        assert!(matches!(
            offer::text(&vec![b'a'; MAX_BYTES + 1]),
            Err(ClipboardError::CaptureLimit { .. })
        ));
        for invalid in [b"valid\0tail".as_slice(), b"\xFF", b"\xE2\x82"] {
            assert!(matches!(
                offer::text(invalid),
                Err(ClipboardError::MalformedPayload { .. })
            ));
        }
        let uris = "file:///tmp/path\n".repeat(MAX_FILES + 1);
        assert!(matches!(
            offer::uri_list(uris.as_bytes()),
            Err(ClipboardError::CaptureLimit { .. })
        ));
        let path = format!("file:///{}", "x".repeat(offer::MAX_PATH_BYTES));
        assert!(matches!(
            offer::uri_list(path.as_bytes()),
            Err(ClipboardError::CaptureLimit { .. })
        ));
        let mut types = Vec::new();
        for index in 0..offer::MAX_OFFER_TYPES {
            offer::add_type(&mut types, &format!("application/x-type-{index}")).unwrap();
        }
        assert!(matches!(
            offer::add_type(&mut types, "new-type"),
            Err(ClipboardError::CaptureLimit { .. })
        ));
        assert!(offer::add_type(&mut types, "bad\nname").is_err());
    }
}
