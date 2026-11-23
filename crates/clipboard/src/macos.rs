//! Read-only AppKit general pasteboard adapter.

use std::sync::Mutex;

use objc2::rc::{AutoreleasePool, autoreleasepool};
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSWorkspace};
use objc2_foundation::{NSArray, NSData, NSString};

use crate::native::offer::{
    self, Dialect, Format, MAX_BYTES, MAX_OFFER_TYPE_BYTES, MAX_OFFER_TYPES,
};
use crate::native::{NativeSerial, unchanged};
use crate::{
    CapturePolicy, ClipboardError, ClipboardSnapshot, MAX_SOURCE_APP_CHARS, SnapshotContent,
    SourceApp, SourceConfidence,
};

// Serialize observations so a delayed caller cannot move the counter mapping
// backwards and give a previously observed pasteboard generation a new ID.
static SERIAL: Mutex<NativeSerial> = Mutex::new(NativeSerial::new());

pub(crate) fn read(policy: Option<&CapturePolicy>) -> Result<ClipboardSnapshot, ClipboardError> {
    let mut serial = SERIAL.lock().map_err(|_| failed("pasteboard state"))?;
    autoreleasepool(|pool| {
        let board = NSPasteboard::generalPasteboard();
        let before = board.changeCount();
        let sequence = serial.observe(before);
        let source = frontmost_app(pool);
        let result = read_offer(&board, before, sequence, source.clone(), policy, pool);
        let mut snapshot = unchanged(before, board.changeCount(), result)?;
        // Treat a foreground switch as unknown, never as verified provenance.
        if source != frontmost_app(pool) {
            snapshot.source = None;
            snapshot.confidence = SourceConfidence::Unknown;
        }
        unchanged(before, board.changeCount(), Ok(snapshot))
    })
}

fn read_offer(
    board: &NSPasteboard,
    before: isize,
    sequence: u64,
    source: Option<SourceApp>,
    policy: Option<&CapturePolicy>,
    pool: AutoreleasePool<'_>,
) -> Result<ClipboardSnapshot, ClipboardError> {
    let mut types = Vec::new();
    if let Some(board_types) = board.types() {
        add_types(&mut types, &board_types, pool)?;
    }
    // Do not assume the board-level type list describes every item. A marker
    // on any item protects the entire offer, even if some other item is read.
    let items = board.pasteboardItems();
    if let Some(items) = &items {
        if items.count() > offer::MAX_FILES {
            return Err(offer::limit("pasteboard item count"));
        }
        for index in 0..items.count() {
            let item = items.objectAtIndex(index);
            add_types(&mut types, &item.types(), pool)?;
        }
    }

    offer::snapshot(
        Dialect::Mac,
        &types,
        sequence,
        source,
        policy,
        |format, name| {
            if board.changeCount() != before {
                return Err(ClipboardError::ClipboardChanged);
            }
            let kind = NSString::from_str(name);
            match format {
                Format::Text => {
                    let data = board
                        .dataForType(&kind)
                        .ok_or_else(|| failed("read pasteboard text"))?;
                    offer::text(bounded_data(&data, MAX_BYTES)?)
                }
                Format::Files => read_files(board, items.as_deref(), &kind),
                Format::Image => unreachable!("the offer layer never requests image bytes"),
            }
        },
    )
}

fn add_types(
    types: &mut Vec<String>,
    advertised: &NSArray<NSString>,
    pool: AutoreleasePool<'_>,
) -> Result<(), ClipboardError> {
    if advertised.count() > MAX_OFFER_TYPES {
        return Err(offer::limit("pasteboard types per item"));
    }
    for index in 0..advertised.count() {
        let name = advertised.objectAtIndex(index);
        if name.len() > MAX_OFFER_TYPE_BYTES {
            return Err(offer::limit("pasteboard type bytes"));
        }
        // SAFETY: the retained type string and the active outer pool both
        // live for the borrow. Type names are only copied into bounded memory.
        offer::add_type(types, unsafe { name.to_str(pool) })?;
    }
    Ok(())
}

fn read_files(
    board: &NSPasteboard,
    items: Option<&NSArray<NSPasteboardItem>>,
    kind: &NSString,
) -> Result<SnapshotContent, ClipboardError> {
    let mut files = offer::Files::new();
    let mut found = false;
    if let Some(items) = items {
        for index in 0..items.count() {
            let item = items.objectAtIndex(index);
            let types = item.types();
            if !types.containsObject(kind) {
                continue;
            }
            let data = item
                .dataForType(kind)
                .ok_or_else(|| failed("read pasteboard file URL"))?;
            files.push(bounded_data(&data, MAX_BYTES)?)?;
            found = true;
        }
    }
    if !found {
        let data = board
            .dataForType(kind)
            .ok_or_else(|| failed("read pasteboard file URL"))?;
        files.push(bounded_data(&data, MAX_BYTES)?)?;
    }
    Ok(files.finish())
}

fn bounded_data(data: &NSData, limit: usize) -> Result<&[u8], ClipboardError> {
    if data.len() > limit {
        return Err(offer::limit("pasteboard representation bytes"));
    }
    // SAFETY: NSData returned by the pasteboard is immutable, is retained for
    // this borrow, and never crosses a thread or escapes its autorelease pool.
    Ok(unsafe { data.as_bytes_unchecked() })
}

fn frontmost_app(pool: AutoreleasePool<'_>) -> Option<SourceApp> {
    let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    let bundle = app.bundleIdentifier()?;
    if bundle.len() > MAX_SOURCE_APP_CHARS {
        return None;
    }
    // SAFETY: bundle and the active pool both outlive this short borrow.
    SourceApp::new(unsafe { bundle.to_str(pool) })
}

fn failed(operation: &'static str) -> ClipboardError {
    ClipboardError::CaptureFailed { operation }
}

#[cfg(test)]
mod tests {
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::NSPasteboardWriting;

    use super::*;
    use crate::{CaptureGate, CaptureRejection, FilterOutcome, evaluate};

    #[test]
    #[ignore = "requires a real macOS desktop pasteboard server"]
    fn native_macos_unique_pasteboard_fixtures() {
        autoreleasepool(|pool| {
            let board = NSPasteboard::pasteboardWithUniqueName();
            board.clearContents();
            let kind = NSString::from_str(offer::MAC_TEXT);
            let value = NSString::from_str("Ansa native text fixture é");
            assert!(board.setString_forType(&value, &kind));
            let serial = board.changeCount();
            assert!(matches!(
                &read_offer(&board, serial, 1, None, None, pool).unwrap().content,
                SnapshotContent::Text(text) if text == "Ansa native text fixture é"
            ));

            board.clearContents();
            let file_kind = NSString::from_str(offer::MAC_FILE);
            let first = NSPasteboardItem::new();
            let second = NSPasteboardItem::new();
            assert!(
                first.setString_forType(&NSString::from_str("file:///tmp/fixture-a"), &file_kind)
            );
            assert!(
                second
                    .setString_forType(&NSString::from_str("file:///tmp/fixture%20b"), &file_kind)
            );
            let items = NSArray::<ProtocolObject<dyn NSPasteboardWriting>>::from_retained_slice(&[
                ProtocolObject::from_retained(first),
                ProtocolObject::from_retained(second),
            ]);
            assert!(board.writeObjects(&items));
            let serial = board.changeCount();
            assert!(matches!(
                &read_offer(&board, serial, 2, None, None, pool).unwrap().content,
                SnapshotContent::FileList(paths) if paths == &["/tmp/fixture-a", "/tmp/fixture b"]
            ));

            for marker in [
                "org.nspasteboard.ConcealedType",
                "org.nspasteboard.TransientType",
                "org.nspasteboard.AutoGeneratedType",
            ] {
                board.clearContents();
                let text = NSPasteboardItem::new();
                assert!(text.setString_forType(&value, &kind));
                let metadata = NSPasteboardItem::new();
                assert!(
                    metadata.setData_forType(&NSData::with_bytes(&[]), &NSString::from_str(marker))
                );
                let items =
                    NSArray::<ProtocolObject<dyn NSPasteboardWriting>>::from_retained_slice(&[
                        ProtocolObject::from_retained(text),
                        ProtocolObject::from_retained(metadata),
                    ]);
                assert!(board.writeObjects(&items));
                let serial = board.changeCount();
                let snapshot = read_offer(&board, serial, 3, None, None, pool).unwrap();
                assert_eq!(snapshot.byte_len(), 0);
                assert_eq!(
                    evaluate(&snapshot, &CapturePolicy::default(), CaptureGate::default()),
                    FilterOutcome::Reject(CaptureRejection::MonitorProcessingExcluded)
                );
            }
            board.clearContents();
        });
    }
}
