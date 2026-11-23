//! Manual native desktop probes. Use only synthetic clipboard fixtures.

#![cfg(any(target_os = "macos", target_os = "linux"))]

use ansa_clipboard::{
    CapturePolicy, CaptureRejection, CaptureReport, CaptureSupport, ClipboardContent,
    ClipboardStore, ClipboardTransport, SourceConfidence, clipboard_capture_capabilities,
    read_clipboard_snapshot,
};
use ansa_vault::Vault;

#[test]
#[ignore = "requires a real desktop and an explicitly prepared synthetic clipboard fixture"]
fn native_desktop_capture_probe() {
    let mode = std::env::var("ANSA_CLIPBOARD_PROBE").expect("set ANSA_CLIPBOARD_PROBE");
    let expected_transport = match std::env::var("ANSA_CLIPBOARD_TRANSPORT").as_deref() {
        Ok("macos") => ClipboardTransport::MacOs,
        Ok("x11") => ClipboardTransport::X11,
        Ok("wayland") => ClipboardTransport::Wayland,
        _ => panic!("set ANSA_CLIPBOARD_TRANSPORT to macos, x11 or wayland"),
    };
    let capabilities = clipboard_capture_capabilities();
    assert_eq!(capabilities.transport, expected_transport);
    assert!(matches!(
        capabilities.image,
        CaptureSupport::Unsupported { .. }
    ));
    if mode == "unavailable" {
        assert!(matches!(
            capabilities.text,
            CaptureSupport::Unsupported { .. }
        ));
        assert!(matches!(
            capabilities.file_list,
            CaptureSupport::Unsupported { .. }
        ));
        assert!(matches!(
            read_clipboard_snapshot(),
            Err(ansa_clipboard::ClipboardError::CaptureUnavailable { .. })
        ));
        return;
    }
    assert!(!matches!(
        capabilities.text,
        CaptureSupport::Unsupported { .. }
    ));
    assert!(!matches!(
        capabilities.file_list,
        CaptureSupport::Unsupported { .. }
    ));

    let root = tempfile::tempdir().expect("temp profile");
    let vault = Vault::open(root.path().join("vault")).expect("OS vault; no plaintext fallback");
    let history = root.path().join("history");
    let mut store = ClipboardStore::open(&history, CapturePolicy::default(), vault).unwrap();
    let observed = read_clipboard_snapshot().expect("snapshot");
    assert_ne!(observed.confidence, SourceConfidence::Confirmed);
    if expected_transport == ClipboardTransport::Wayland {
        assert_eq!(observed.confidence, SourceConfidence::Unknown);
    }
    let report = store.capture_current(1).expect("native capture");
    match mode.as_str() {
        "marked" | "image" | "sensitive" => {
            let reason = match mode.as_str() {
                "marked" => CaptureRejection::MonitorProcessingExcluded,
                "sensitive" => CaptureRejection::SensitiveContent,
                _ => CaptureRejection::ImageUnsupported,
            };
            assert_eq!(report, CaptureReport::Skipped(reason));
            assert!(store.items().is_empty());
            assert_eq!(store.cached_preview_count(), 0);
            assert!(!history.join("history.json").exists());
        }
        "text" | "files" => {
            let CaptureReport::Stored(record) = report else {
                panic!("expected the prepared fixture to be persisted")
            };
            let data = store
                .decrypt_item(record.item.id)
                .expect("vault round-trip");
            match (&data.content, mode.as_str()) {
                (ClipboardContent::Text(text), "text") => {
                    assert!(text == "Ansa native text fixture é")
                }
                (ClipboardContent::FileList(paths), "files") => {
                    assert!(paths == &["/tmp/ansa-native-file-fixture.txt"])
                }
                _ => panic!("unexpected fixture format"),
            }
            drop(data);
            drop(store.preview(record.item.id, 64).expect("cached preview"));
            store.lock();
            assert_eq!(store.cached_preview_count(), 0);
            assert!(store.decrypt_item(record.item.id).is_err());
            store.unlock().expect("explicit native vault unlock");
            assert_eq!(store.capture_current(2).unwrap(), CaptureReport::Duplicate);
            assert_eq!(store.items().len(), 1);
        }
        _ => panic!("unknown synthetic fixture mode"),
    }
}
