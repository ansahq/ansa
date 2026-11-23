//! Exercise the production vault and new fallback paths without native clipboard
//! access or desktop secrets. No test double encrypts these payloads.

use std::fs;
use std::path::Path;

use ansa_clipboard::{
    CapturePolicy, CaptureRejection, CaptureReport, ClipboardContent, ClipboardSnapshot,
    ClipboardStore,
};
use ansa_vault::{MAX_MEMORY_SECRET_BYTES, MAX_PLAINTEXT_BYTES, Vault};

const TEXT: &str = "Ansa synthetic clipboard sentinel · café";
const FILE: &str = "/tmp/ansa-synthetic-files-sentinel.txt";
const PASSPHRASE: &[u8] = b"ansa disposable test profile passphrase";

fn assert_no_plaintext(root: &Path) {
    for entry in fs::read_dir(root).expect("list test profile") {
        let entry = entry.expect("entry");
        if entry.file_type().unwrap().is_dir() {
            assert_no_plaintext(&entry.path());
        } else if entry.file_name() != ".lock" {
            // A no-share, empty file is held open by the Windows store lock.
            let bytes = fs::read(entry.path()).expect("read test file");
            for fixture in [TEXT, FILE, std::str::from_utf8(PASSPHRASE).unwrap()] {
                assert!(
                    !bytes
                        .windows(fixture.len())
                        .any(|window| window == fixture.as_bytes())
                );
            }
        }
    }
}

/// Exact encoded size used by the memory-only named-secret map.
fn encoded_secret_size(alias: &str, plaintext_len: usize) -> usize {
    let context = format!("secret:{}", alias.to_ascii_lowercase());
    2 * alias.len() + context.len() + 2 * (plaintext_len + 40)
}

#[test]
fn explicit_memory_only_history_writes_nothing_and_drops_items_at_lock() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("never-created");
    let mut store = ClipboardStore::open(
        &root,
        CapturePolicy::default(),
        Vault::memory_only().unwrap(),
    )
    .unwrap();
    assert!(!store.is_persistent());
    assert!(
        store
            .capture(ClipboardSnapshot::text(1, TEXT), 1)
            .unwrap()
            .is_stored()
    );
    let id = store.items()[0].id;
    drop(store.preview(id, 80).unwrap());
    assert_eq!(store.cached_preview_count(), 1);
    store.lock();
    assert!(store.items().is_empty());
    assert_eq!(store.cached_preview_count(), 0);
    assert!(store.decrypt_item(id).is_err());
    store.unlock().unwrap();
    assert_eq!(
        store.capture(ClipboardSnapshot::text(1, TEXT), 2).unwrap(),
        CaptureReport::Duplicate
    );
    assert!(
        store
            .capture(ClipboardSnapshot::text(2, TEXT), 3)
            .unwrap()
            .is_stored()
    );
    assert!(!root.exists());
}

#[test]
fn failed_volatile_capture_preserves_the_previous_row_and_payload() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("never-created");
    let vault = Vault::memory_only().expect("memory-only vault");

    // Leave 4 KiB of encoded vault space: enough for the small row below but
    // far too little for a 128 KiB envelope after the small row's payload is
    // retained. These unrelated secrets share the vault with the clipboard
    // history, which the store's estimator cannot see.
    let mut used = 0;
    let large = vec![b'f'; MAX_PLAINTEXT_BYTES];
    for index in 0..7 {
        let alias = format!("unrelated{index}");
        vault.put_secret(&alias, &large).expect("filler secret");
        used += encoded_secret_size(&alias, large.len());
    }
    let last_alias = "unrelated7";
    let remaining = MAX_MEMORY_SECRET_BYTES - 4096 - used;
    let fixed = 2 * last_alias.len() + format!("secret:{last_alias}").len();
    let last_len = (remaining - fixed) / 2 - 40;
    assert!(last_len <= MAX_PLAINTEXT_BYTES);
    vault
        .put_secret(last_alias, &vec![b'f'; last_len])
        .expect("final filler secret");

    let policy = CapturePolicy {
        max_items: 1,
        ..CapturePolicy::default()
    };
    let mut store = ClipboardStore::open(&root, policy, vault).expect("memory-only store opens");
    assert!(!store.is_persistent());

    let kept_text = "small kept row";
    let report = store
        .capture(ClipboardSnapshot::text(1, kept_text), 1)
        .expect("small capture");
    let CaptureReport::Stored(kept) = report else {
        panic!("small row was not stored: {report}");
    };

    // max_items=1 evicts the small row from the candidate, but the vault cannot
    // seal the replacement even after that eviction. The transaction must
    // reject the capture without deleting the retained row's ciphertext.
    let large_row = "x".repeat(128 * 1024);
    assert_eq!(
        store
            .capture(ClipboardSnapshot::text(2, &large_row), 2)
            .expect("quota rejection is reported as a skip"),
        CaptureReport::Skipped(CaptureRejection::QuotaExhausted)
    );

    assert_eq!(store.items().len(), 1);
    assert_eq!(store.items()[0].id, kept.item.id);
    let decrypted = store
        .decrypt_item(kept.item.id)
        .expect("the previous row's payload must survive");
    match &decrypted.content {
        ClipboardContent::Text(value) => assert_eq!(value, kept_text),
        ClipboardContent::FileList(_) => panic!("wrong content"),
    }
    assert_eq!(store.cached_preview_count(), 0);
    assert!(!root.exists(), "memory-only capture must not touch disk");
}

#[test]
fn passphrase_history_round_trip_lock_and_key_loss_never_write_plaintext() {
    let dir = tempfile::tempdir().expect("tempdir");
    let vault_root = dir.path().join("vault");
    let history_root = dir.path().join("history");
    let mut store = ClipboardStore::open(
        &history_root,
        CapturePolicy::default(),
        Vault::open_passphrase(&vault_root, PASSPHRASE).unwrap(),
    )
    .unwrap();
    assert!(store.is_persistent());
    assert!(
        store
            .capture(ClipboardSnapshot::text(1, TEXT), 1)
            .unwrap()
            .is_stored()
    );
    let text_id = store.items()[0].id;
    assert!(
        store
            .capture(ClipboardSnapshot::file_list(2, vec![FILE.into()]), 2)
            .unwrap()
            .is_stored()
    );
    let file_id = store.items()[0].id;
    assert!(
        matches!(&store.decrypt_item(text_id).unwrap().content, ClipboardContent::Text(value) if value == TEXT)
    );
    assert!(
        matches!(&store.decrypt_item(file_id).unwrap().content, ClipboardContent::FileList(paths) if paths == &[FILE])
    );
    drop(store.preview(text_id, 80).unwrap());
    assert_no_plaintext(dir.path());

    store.lock();
    assert_eq!(store.cached_preview_count(), 0);
    assert!(store.decrypt_item(file_id).is_err());
    assert!(store.unlock().is_err());
    store.unlock_with_passphrase(PASSPHRASE).unwrap();
    assert_eq!(
        store.capture(ClipboardSnapshot::text(1, TEXT), 3).unwrap(),
        CaptureReport::Duplicate
    );
    assert_eq!(store.items().len(), 2);
    drop(store.preview(text_id, 80).unwrap());

    // Simulate the persistent wrapping material becoming unavailable, with
    // ciphertext and clipboard metadata still present.
    store.lock();
    fs::remove_file(vault_root.join("vault.json")).unwrap();
    let metadata = fs::read(history_root.join("history.json")).unwrap();
    assert!(store.unlock_with_passphrase(PASSPHRASE).is_err());
    assert!(store.is_locked());
    assert_eq!(store.cached_preview_count(), 0);
    assert_eq!(
        store.capture(ClipboardSnapshot::text(3, TEXT), 4).unwrap(),
        CaptureReport::Skipped(CaptureRejection::Locked)
    );
    assert_eq!(
        fs::read(history_root.join("history.json")).unwrap(),
        metadata
    );
    assert_no_plaintext(dir.path());
}
