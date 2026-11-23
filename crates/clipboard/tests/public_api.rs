//! Public API integration checks: filtering, sealed round-trip through the
//! test backend, insertion outcomes through a fake host, and the cross-process
//! store lock.

use std::path::Path;
use std::process::Command;

use ansa_clipboard::CapturePolicy;
use ansa_clipboard::{
    ClipboardContent, ClipboardError, ClipboardOwner, ClipboardSnapshot, ClipboardStore,
    FocusOutcome, HostFailure, InsertionConfig, InsertionEvidence, InsertionHost, InsertionOutcome,
    InsertionPolicy, InsertionTarget, MemorySealBackend, ObservedTarget, PasteAttempt,
    TargetPermission, execute_insertion,
};

fn locked_store(root: &Path) -> Result<ClipboardStore, ClipboardError> {
    ClipboardStore::open_with_backend(
        root,
        CapturePolicy::default(),
        Box::new(MemorySealBackend::new()),
    )
}

#[test]
fn cross_process_second_store_is_already_locked() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = locked_store(dir.path()).expect("first store");
    let status = Command::new(std::env::current_exe().expect("current exe"))
        .args([
            "--exact",
            "child_process_lock_probe",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("ANSA_CLIP_LOCK_DIR", dir.path())
        .status()
        .expect("spawn child test process");
    assert!(status.success(), "child process reported a lock failure");
    drop(store);
}

#[test]
fn child_process_lock_probe() {
    let Some(directory) = std::env::var_os("ANSA_CLIP_LOCK_DIR") else {
        // Normal in-process test run; the parent test drives the child.
        return;
    };
    let error = locked_store(Path::new(&directory)).expect_err("child must not acquire the lock");
    assert!(
        matches!(error, ClipboardError::AlreadyLocked),
        "unexpected child result: {error}"
    );
}

#[test]
fn capture_filter_and_round_trip_through_the_public_api() {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = ansa_clipboard::MemorySealBackend::new();
    let handle = backend.clone();
    let mut store =
        ClipboardStore::open_with_backend(dir.path(), CapturePolicy::default(), Box::new(backend))
            .expect("store");

    let report = store
        .capture(ClipboardSnapshot::text(1, "public api sentinel"), 10)
        .expect("capture");
    assert!(report.is_stored());
    assert_eq!(store.items().len(), 1);
    assert_eq!(handle.len(), 1);

    let id = store.items()[0].id;
    let decrypted = store.decrypt_item(id).expect("decrypt");
    match &decrypted.content {
        ClipboardContent::Text(value) => assert_eq!(value, "public api sentinel"),
        ClipboardContent::FileList(_) => panic!("wrong content"),
    }

    let excluded = store
        .capture(
            ClipboardSnapshot::text(2, "ghp_abcdefghijklmnopqrstuvwxyz"),
            11,
        )
        .expect("capture");
    assert!(!excluded.is_stored());
    assert_eq!(store.items().len(), 1);

    store.lock();
    assert!(store.is_locked());
    assert!(store.decrypt_item(id).is_err());
}

struct FakeHost {
    observed: ObservedTarget,
    sequence: u64,
    owner: ClipboardOwner,
    evidence: InsertionEvidence,
    pasted: bool,
}

impl FakeHost {
    fn new() -> Self {
        Self {
            observed: ObservedTarget {
                exists: true,
                process_id: 1,
                process_created_at: 2,
                window_handle: 3,
                window_thread_id: 4,
                generation: 5,
                permission: TargetPermission::Allowed,
                secure_input: false,
                ime_composing: false,
                foreground: true,
            },
            sequence: 100,
            owner: ClipboardOwner::Ours,
            evidence: InsertionEvidence::Unavailable,
            pasted: false,
        }
    }
}

impl InsertionHost for FakeHost {
    fn write_clipboard(&mut self, _content: &ClipboardContent) -> Result<u64, HostFailure> {
        self.sequence += 1;
        self.owner = ClipboardOwner::Ours;
        Ok(self.sequence)
    }

    fn clipboard_sequence(&mut self) -> u64 {
        self.sequence
    }

    fn clipboard_owner(&mut self) -> ClipboardOwner {
        self.owner
    }

    fn observe_target(&mut self, _target: &InsertionTarget) -> Result<ObservedTarget, HostFailure> {
        Ok(self.observed)
    }

    fn request_focus(&mut self, _target: &InsertionTarget) -> FocusOutcome {
        FocusOutcome::Granted
    }

    fn send_paste(&mut self, _target: &InsertionTarget, expected_sequence: u64) -> PasteAttempt {
        if expected_sequence != self.sequence {
            return PasteAttempt::Failed;
        }
        self.pasted = true;
        PasteAttempt::Sent
    }

    fn observe_insertion(
        &mut self,
        _target: &InsertionTarget,
        _expected: &ClipboardContent,
    ) -> InsertionEvidence {
        self.evidence
    }
}

fn target() -> InsertionTarget {
    InsertionTarget {
        process_id: 1,
        process_created_at: 2,
        window_handle: 3,
        window_thread_id: 4,
        generation: 5,
        permission: TargetPermission::Allowed,
        foreground: true,
    }
}

#[test]
fn insertion_is_copy_only_by_default_and_unverified_without_evidence() {
    let content = ClipboardContent::text("paste me");
    let mut host = FakeHost::new();
    let copied = execute_insertion(
        &mut host,
        &target(),
        &content,
        InsertionPolicy::CopyOnly,
        InsertionConfig::default(),
    );
    assert_eq!(copied, InsertionOutcome::Copied { sequence: 101 });
    assert!(!copied.claims_inserted());
    assert!(!host.pasted);

    let config = InsertionConfig {
        automatic_enabled: true,
        allow_focus_request: true,
        allow_input_injection: true,
    };
    let mut host = FakeHost::new();
    let outcome = execute_insertion(
        &mut host,
        &target(),
        &content,
        InsertionPolicy::Automatic,
        config,
    );
    assert!(matches!(outcome, InsertionOutcome::Unverified { .. }));
    assert!(!outcome.claims_inserted());
    assert!(host.pasted);
}
