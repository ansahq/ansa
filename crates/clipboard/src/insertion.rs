//! Safe insertion transactions.
//!
//! The launcher copies the selected value to the clipboard first and only then
//! considers an automatic paste. Every automatic step is checked against the
//! target captured at summon time:
//!
//! 1. the target process, creation time, window and invocation generation must
//!    still match;
//! 2. the captured and observed permission state must both be `Allowed`;
//! 3. secure-input and IME-composition boundaries block injection;
//! 4. focus must be observed (or a focus request must be verified);
//! 5. the clipboard sequence and owner must still be ours immediately before
//!    input is sent, so a concurrent user copy is never overwritten;
//! 6. insertion is reported as `Inserted` only when the host observes
//!    destination evidence. Missing evidence yields `Unverified`.
//!
//! The transaction never restores an older clipboard value. If the user copies
//! something while a paste is in progress, that newer value stays untouched.
//!
//! ## Honest limit
//!
//! This crate does not ship a Windows `InsertionHost` or a `SendInput` adapter.
//! Only [`InsertionPolicy::CopyOnly`] can therefore be wired in production
//! today; the automatic path is exercised against host fakes and remains
//! opt-in, explicit and unverified by default.

use std::fmt;

use crate::content::ClipboardContent;

/// Whether insertion may be attempted at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InsertionPolicy {
    /// Only place the value on the clipboard. This is the default and the safe
    /// fallback.
    #[default]
    CopyOnly,
    /// Try automatic insertion after the copy, subject to
    /// [`InsertionConfig`] and all runtime checks.
    Automatic,
}

/// Explicit switches required before any automatic insertion.
///
/// Every field defaults to `false`, so automatic insertion is disabled unless a
/// caller deliberately turns it on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InsertionConfig {
    /// Master switch. Automatic insertion is never attempted when false.
    pub automatic_enabled: bool,
    /// Allow the host to request foreground focus for the target.
    pub allow_focus_request: bool,
    /// Allow the host to send paste input. Without this, the outcome can only be
    /// `Copied` or `CopyFallback`.
    pub allow_input_injection: bool,
}

/// Captured permission state for an insertion target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetPermission {
    /// Automation/accessibility permission was observed as allowed.
    Allowed,
    /// Permission was observed as denied.
    Denied,
    /// Permission could not be determined. Automatic insertion is refused.
    Unknown,
}

/// A target captured when the launcher was summoned.
///
/// `generation` is the caller's invocation counter. It must change whenever the
/// user navigates to a different target, so a stale transaction cannot paste
/// into a new window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsertionTarget {
    /// Owning process id.
    pub process_id: u32,
    /// Process creation time, protecting against pid reuse.
    pub process_created_at: u64,
    /// Top-level window handle, as an integer for platform neutrality.
    pub window_handle: isize,
    /// Creating thread id of the window.
    pub window_thread_id: u32,
    /// Invocation generation.
    pub generation: u64,
    /// Permission state observed at summon time.
    pub permission: TargetPermission,
    /// Whether the target was foreground at summon time.
    pub foreground: bool,
}

/// A fresh observation of the target made by the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservedTarget {
    /// Whether a window with the recorded handle still exists.
    pub exists: bool,
    /// Current owning process id.
    pub process_id: u32,
    /// Current process creation time.
    pub process_created_at: u64,
    /// Current window handle.
    pub window_handle: isize,
    /// Current creating thread id.
    pub window_thread_id: u32,
    /// Current invocation generation.
    pub generation: u64,
    /// Current permission state.
    pub permission: TargetPermission,
    /// Whether a secure-input boundary is active.
    pub secure_input: bool,
    /// Whether an IME composition is in progress.
    pub ime_composing: bool,
    /// Whether the target is currently foreground.
    pub foreground: bool,
}

/// Why a target is no longer safe for insertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetIssue {
    /// The window no longer exists.
    NotPresent,
    /// The window now belongs to a different process.
    ProcessChanged,
    /// The process creation time changed; the pid may have been reused.
    ProcessRecreated,
    /// The window handle or creating thread changed.
    WindowChanged,
    /// The user switched targets or the invocation is stale.
    GenerationChanged,
    /// Permission is denied or could not be verified.
    PermissionDenied,
    /// A secure-input boundary is active.
    SecureInput,
    /// An IME composition is active.
    ImeComposing,
    /// The target is not foreground.
    NotForeground,
}

impl TargetIssue {
    /// Human-readable reason suitable for a status line.
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            Self::NotPresent => "the target window no longer exists",
            Self::ProcessChanged => "the target window now belongs to a different process",
            Self::ProcessRecreated => "the target process may have been replaced",
            Self::WindowChanged => "the target window identity changed",
            Self::GenerationChanged => "the user switched targets before insertion",
            Self::PermissionDenied => "target permission is denied or could not be verified",
            Self::SecureInput => "a secure-input boundary is active",
            Self::ImeComposing => "an IME composition is active",
            Self::NotForeground => "the target is not in the foreground",
        }
    }
}

impl InsertionTarget {
    /// Verify a fresh observation against this captured target.
    pub fn verify(&self, observed: &ObservedTarget) -> Result<(), TargetIssue> {
        if !observed.exists {
            return Err(TargetIssue::NotPresent);
        }
        if observed.process_id != self.process_id {
            return Err(TargetIssue::ProcessChanged);
        }
        if observed.process_created_at != self.process_created_at {
            return Err(TargetIssue::ProcessRecreated);
        }
        if observed.window_handle != self.window_handle
            || observed.window_thread_id != self.window_thread_id
        {
            return Err(TargetIssue::WindowChanged);
        }
        if observed.generation != self.generation {
            return Err(TargetIssue::GenerationChanged);
        }
        if self.permission != TargetPermission::Allowed
            || observed.permission != TargetPermission::Allowed
        {
            return Err(TargetIssue::PermissionDenied);
        }
        if observed.secure_input {
            return Err(TargetIssue::SecureInput);
        }
        if observed.ime_composing {
            return Err(TargetIssue::ImeComposing);
        }
        if !observed.foreground {
            return Err(TargetIssue::NotForeground);
        }
        Ok(())
    }
}

/// Who currently owns the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardOwner {
    /// The launcher wrote the value and still owns it.
    Ours,
    /// Another application owns the clipboard.
    Other,
    /// Ownership could not be observed.
    Unknown,
}

/// Result of requesting foreground focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusOutcome {
    /// The host requested focus and observed the target as foreground.
    Granted,
    /// The host refused or Windows denied foreground activation.
    Denied,
    /// The request was made but could not be verified.
    Unknown,
}

/// Result of sending paste input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteAttempt {
    /// Input was sent to the observed target.
    Sent,
    /// The host has no input-injection implementation.
    Unsupported,
    /// The host attempted input but it failed.
    Failed,
}

/// Destination evidence after a paste attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertionEvidence {
    /// The destination confirmed that the content arrived.
    Confirmed,
    /// The destination explicitly did not receive the content.
    NotConfirmed,
    /// No evidence is available. Never treated as success.
    Unavailable,
}

/// A host failure that is safe to display and does not contain content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostFailure {
    /// The clipboard could not be opened or written.
    ClipboardWrite,
    /// The clipboard stayed busy.
    ClipboardBusy,
    /// The target could not be observed.
    TargetQuery,
    /// The operation is not implemented by this host.
    Unsupported,
}

impl fmt::Display for HostFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClipboardWrite => formatter.write_str("the clipboard could not be written"),
            Self::ClipboardBusy => formatter.write_str("the clipboard stayed busy"),
            Self::TargetQuery => formatter.write_str("the target could not be observed"),
            Self::Unsupported => formatter.write_str("the host does not support this operation"),
        }
    }
}

impl std::error::Error for HostFailure {}

/// Platform operations used by an insertion transaction.
///
/// Implementations must not log clipboard content. `write_clipboard` returns
/// the sequence number observed after the write.
pub trait InsertionHost {
    /// Place `content` on the clipboard, returning the new sequence number.
    fn write_clipboard(&mut self, content: &ClipboardContent) -> Result<u64, HostFailure>;
    /// Current clipboard sequence number.
    fn clipboard_sequence(&mut self) -> u64;
    /// Current clipboard owner.
    fn clipboard_owner(&mut self) -> ClipboardOwner;
    /// Observe the target afresh.
    fn observe_target(&mut self, target: &InsertionTarget) -> Result<ObservedTarget, HostFailure>;
    /// Request foreground focus.
    fn request_focus(&mut self, target: &InsertionTarget) -> FocusOutcome;
    /// Send paste input to the target.
    fn send_paste(&mut self, target: &InsertionTarget) -> PasteAttempt;
    /// Look for destination evidence that the content arrived.
    fn observe_insertion(
        &mut self,
        target: &InsertionTarget,
        expected: &ClipboardContent,
    ) -> InsertionEvidence;
}

/// Truthful result of an insertion transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertionOutcome {
    /// The value is on the clipboard. No paste was attempted.
    Copied {
        /// Clipboard sequence after the write.
        sequence: u64,
    },
    /// Insertion was attempted and destination evidence confirmed it.
    Inserted {
        /// Clipboard sequence after the copy.
        sequence: u64,
    },
    /// Automatic insertion was not attempted or failed; the copy is the
    /// fallback. `reason` never contains clipboard content.
    CopyFallback {
        /// Non-sensitive explanation.
        reason: String,
        /// Clipboard sequence after the copy.
        sequence: u64,
    },
    /// A policy or permission boundary prevented insertion. `copied` reports
    /// whether the value was placed on the clipboard first.
    Denied {
        /// Non-sensitive explanation.
        reason: String,
        /// Whether the copy succeeded.
        copied: bool,
    },
    /// Input was sent but the destination could not confirm insertion. This is
    /// deliberately not `Inserted`.
    Unverified {
        /// Non-sensitive explanation.
        reason: String,
        /// Clipboard sequence after the copy.
        sequence: u64,
    },
}

impl InsertionOutcome {
    /// Stable label for logs and UI.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Copied { .. } => "copied",
            Self::Inserted { .. } => "inserted",
            Self::CopyFallback { .. } => "copy-fallback",
            Self::Denied { .. } => "denied",
            Self::Unverified { .. } => "unverified",
        }
    }

    /// Whether the value is known to be on the clipboard.
    #[must_use]
    pub fn copied(&self) -> bool {
        match self {
            Self::Copied { .. }
            | Self::Inserted { .. }
            | Self::CopyFallback { .. }
            | Self::Unverified { .. } => true,
            Self::Denied { copied, .. } => *copied,
        }
    }

    /// Whether destination evidence allowed an `Inserted` claim.
    #[must_use]
    pub fn claims_inserted(&self) -> bool {
        matches!(self, Self::Inserted { .. })
    }
}

/// Execute a checked insertion transaction.
///
/// The copy always happens first. Automatic insertion then runs only when
/// `policy` is [`InsertionPolicy::Automatic`] and `config` explicitly enables
/// the required steps. Any failed check produces a truthful copy fallback,
/// denial or unverified outcome; none of them claims insertion.
pub fn execute_insertion(
    host: &mut dyn InsertionHost,
    target: &InsertionTarget,
    content: &ClipboardContent,
    policy: InsertionPolicy,
    config: InsertionConfig,
) -> InsertionOutcome {
    let sequence = match host.write_clipboard(content) {
        Ok(sequence) => sequence,
        Err(error) => {
            return InsertionOutcome::Denied {
                reason: format!("copy failed: {error}"),
                copied: false,
            };
        }
    };
    if host.clipboard_owner() == ClipboardOwner::Other {
        return InsertionOutcome::Denied {
            reason: "another application took clipboard ownership during the copy".to_owned(),
            copied: false,
        };
    }

    if policy == InsertionPolicy::CopyOnly {
        return InsertionOutcome::Copied { sequence };
    }
    if !config.automatic_enabled {
        return fallback(sequence, "automatic insertion is disabled");
    }
    if !config.allow_input_injection {
        return fallback(sequence, "input injection is disabled");
    }
    if target.permission != TargetPermission::Allowed {
        return InsertionOutcome::Denied {
            reason: TargetIssue::PermissionDenied.reason().to_owned(),
            copied: true,
        };
    }

    let observed = match host.observe_target(target) {
        Ok(observed) => observed,
        Err(error) => return fallback(sequence, &format!("target verification failed: {error}")),
    };
    match target.verify(&observed) {
        Ok(()) => {}
        Err(TargetIssue::NotForeground) => {
            if !config.allow_focus_request {
                return fallback(sequence, "focus requests are disabled");
            }
            match host.request_focus(target) {
                FocusOutcome::Granted => {
                    let observed = match host.observe_target(target) {
                        Ok(observed) => observed,
                        Err(error) => {
                            return fallback(
                                sequence,
                                &format!("post-focus target verification failed: {error}"),
                            );
                        }
                    };
                    if let Err(issue) = target.verify(&observed) {
                        return denied_or_fallback(issue, sequence);
                    }
                }
                FocusOutcome::Denied => {
                    return InsertionOutcome::Denied {
                        reason: "the target did not accept focus".to_owned(),
                        copied: true,
                    };
                }
                FocusOutcome::Unknown => {
                    return fallback(sequence, "foreground focus could not be verified");
                }
            }
        }
        Err(issue) => return denied_or_fallback(issue, sequence),
    }

    // A user copy between the fallback write and the paste must win. The
    // launcher never restores its older value.
    if host.clipboard_sequence() != sequence {
        return fallback(sequence, "the clipboard changed before insertion");
    }
    if host.clipboard_owner() == ClipboardOwner::Other {
        return fallback(sequence, "clipboard ownership changed before insertion");
    }

    // Re-verify immediately before input; focus may have changed while the
    // clipboard was checked.
    let observed = match host.observe_target(target) {
        Ok(observed) => observed,
        Err(error) => return fallback(sequence, &format!("final target check failed: {error}")),
    };
    if let Err(issue) = target.verify(&observed) {
        return denied_or_fallback(issue, sequence);
    }

    match host.send_paste(target) {
        PasteAttempt::Sent => {}
        PasteAttempt::Unsupported => {
            return fallback(sequence, "input injection is not supported by this host");
        }
        PasteAttempt::Failed => return fallback(sequence, "paste input could not be sent"),
    }

    match host.observe_insertion(target, content) {
        InsertionEvidence::Confirmed => InsertionOutcome::Inserted { sequence },
        InsertionEvidence::NotConfirmed => {
            fallback(sequence, "the destination did not receive the content")
        }
        InsertionEvidence::Unavailable => InsertionOutcome::Unverified {
            reason: "insertion could not be verified at the destination".to_owned(),
            sequence,
        },
    }
}

fn fallback(sequence: u64, reason: &str) -> InsertionOutcome {
    InsertionOutcome::CopyFallback {
        reason: reason.to_owned(),
        sequence,
    }
}

fn denied_or_fallback(issue: TargetIssue, sequence: u64) -> InsertionOutcome {
    match issue {
        TargetIssue::PermissionDenied | TargetIssue::SecureInput | TargetIssue::ImeComposing => {
            InsertionOutcome::Denied {
                reason: issue.reason().to_owned(),
                copied: true,
            }
        }
        _ => fallback(sequence, issue.reason()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct FakeHost {
        observed: ObservedTarget,
        sequence: u64,
        owner: ClipboardOwner,
        write_fails: bool,
        observe_fails: bool,
        focus: FocusOutcome,
        paste: PasteAttempt,
        evidence: InsertionEvidence,
        bump_sequence_on_focus: bool,
        bump_generation_on_focus: bool,
        writes: usize,
        focus_requests: usize,
        paste_attempts: usize,
    }

    impl FakeHost {
        fn new() -> Self {
            Self {
                observed: observed(),
                sequence: 10,
                owner: ClipboardOwner::Ours,
                write_fails: false,
                observe_fails: false,
                focus: FocusOutcome::Granted,
                paste: PasteAttempt::Sent,
                evidence: InsertionEvidence::Confirmed,
                bump_sequence_on_focus: false,
                bump_generation_on_focus: false,
                writes: 0,
                focus_requests: 0,
                paste_attempts: 0,
            }
        }
    }

    impl InsertionHost for FakeHost {
        fn write_clipboard(&mut self, _content: &ClipboardContent) -> Result<u64, HostFailure> {
            self.writes += 1;
            if self.write_fails {
                return Err(HostFailure::ClipboardWrite);
            }
            self.sequence += 1;
            Ok(self.sequence)
        }

        fn clipboard_sequence(&mut self) -> u64 {
            self.sequence
        }

        fn clipboard_owner(&mut self) -> ClipboardOwner {
            self.owner
        }

        fn observe_target(
            &mut self,
            _target: &InsertionTarget,
        ) -> Result<ObservedTarget, HostFailure> {
            if self.observe_fails {
                return Err(HostFailure::TargetQuery);
            }
            Ok(self.observed)
        }

        fn request_focus(&mut self, _target: &InsertionTarget) -> FocusOutcome {
            self.focus_requests += 1;
            if self.bump_sequence_on_focus {
                self.sequence += 1;
            }
            if self.bump_generation_on_focus {
                self.observed.generation += 1;
            }
            if self.focus == FocusOutcome::Granted {
                self.observed.foreground = true;
            }
            self.focus
        }

        fn send_paste(&mut self, _target: &InsertionTarget) -> PasteAttempt {
            self.paste_attempts += 1;
            self.paste
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
            process_id: 100,
            process_created_at: 55,
            window_handle: 0x1234,
            window_thread_id: 7,
            generation: 3,
            permission: TargetPermission::Allowed,
            foreground: false,
        }
    }

    fn observed() -> ObservedTarget {
        ObservedTarget {
            exists: true,
            process_id: 100,
            process_created_at: 55,
            window_handle: 0x1234,
            window_thread_id: 7,
            generation: 3,
            permission: TargetPermission::Allowed,
            secure_input: false,
            ime_composing: false,
            foreground: true,
        }
    }

    fn enabled() -> InsertionConfig {
        InsertionConfig {
            automatic_enabled: true,
            allow_focus_request: true,
            allow_input_injection: true,
        }
    }

    fn content() -> ClipboardContent {
        ClipboardContent::text("insert me")
    }

    fn run(
        host: &mut FakeHost,
        policy: InsertionPolicy,
        config: InsertionConfig,
    ) -> InsertionOutcome {
        execute_insertion(host, &target(), &content(), policy, config)
    }

    #[test]
    fn copy_only_writes_and_never_attempts_paste() {
        let mut host = FakeHost::new();
        let outcome = run(&mut host, InsertionPolicy::CopyOnly, enabled());
        assert_eq!(outcome, InsertionOutcome::Copied { sequence: 11 });
        assert_eq!(host.writes, 1);
        assert_eq!(host.paste_attempts, 0);
        assert!(outcome.copied());
        assert!(!outcome.claims_inserted());
    }

    #[test]
    fn automatic_is_refused_unless_explicitly_enabled() {
        let mut host = FakeHost::new();
        let outcome = run(
            &mut host,
            InsertionPolicy::Automatic,
            InsertionConfig::default(),
        );
        assert!(matches!(outcome, InsertionOutcome::CopyFallback { .. }));
        assert_eq!(host.paste_attempts, 0);

        let mut host = FakeHost::new();
        let config = InsertionConfig {
            automatic_enabled: true,
            ..InsertionConfig::default()
        };
        let outcome = run(&mut host, InsertionPolicy::Automatic, config);
        assert!(matches!(outcome, InsertionOutcome::CopyFallback { .. }));
        assert_eq!(host.paste_attempts, 0);
    }

    #[test]
    fn a_confirmed_automatic_insertion_is_reported_as_inserted() {
        let mut host = FakeHost::new();
        let outcome = run(&mut host, InsertionPolicy::Automatic, enabled());
        assert_eq!(outcome, InsertionOutcome::Inserted { sequence: 11 });
        assert_eq!(host.paste_attempts, 1);
        assert!(outcome.claims_inserted());
    }

    #[test]
    fn missing_destination_evidence_is_unverified_not_inserted() {
        let mut host = FakeHost::new();
        host.evidence = InsertionEvidence::Unavailable;
        let outcome = run(&mut host, InsertionPolicy::Automatic, enabled());
        assert!(matches!(outcome, InsertionOutcome::Unverified { .. }));
        assert!(!outcome.claims_inserted());

        let mut host = FakeHost::new();
        host.evidence = InsertionEvidence::NotConfirmed;
        let outcome = run(&mut host, InsertionPolicy::Automatic, enabled());
        assert!(matches!(outcome, InsertionOutcome::CopyFallback { .. }));
        assert!(!outcome.claims_inserted());
    }

    #[test]
    fn an_exited_target_is_a_copy_fallback_and_is_never_replaced() {
        let mut host = FakeHost::new();
        host.observed.exists = false;
        let outcome = run(&mut host, InsertionPolicy::Automatic, enabled());
        assert!(matches!(outcome, InsertionOutcome::CopyFallback { .. }));
        assert_eq!(host.paste_attempts, 0);
        assert_eq!(host.writes, 1);
    }

    #[test]
    fn a_target_switch_or_stale_generation_is_a_copy_fallback() {
        let mut host = FakeHost::new();
        host.observed.generation = 4;
        let outcome = run(&mut host, InsertionPolicy::Automatic, enabled());
        assert!(matches!(outcome, InsertionOutcome::CopyFallback { .. }));
        assert_eq!(host.paste_attempts, 0);

        let mut host = FakeHost::new();
        host.observed.process_id = 101;
        let outcome = run(&mut host, InsertionPolicy::Automatic, enabled());
        assert!(matches!(outcome, InsertionOutcome::CopyFallback { .. }));
        assert_eq!(host.paste_attempts, 0);
    }

    #[test]
    fn a_target_switch_during_focus_is_detected_before_paste() {
        let mut host = FakeHost::new();
        host.observed.foreground = false;
        host.bump_generation_on_focus = true;
        let outcome = run(&mut host, InsertionPolicy::Automatic, enabled());
        assert!(matches!(outcome, InsertionOutcome::CopyFallback { .. }));
        assert_eq!(host.focus_requests, 1);
        assert_eq!(host.paste_attempts, 0);
    }

    #[test]
    fn a_concurrent_user_copy_is_not_overwritten_and_blocks_paste() {
        let mut host = FakeHost::new();
        host.observed.foreground = false;
        host.bump_sequence_on_focus = true;
        let outcome = run(&mut host, InsertionPolicy::Automatic, enabled());
        assert!(matches!(outcome, InsertionOutcome::CopyFallback { .. }));
        assert_eq!(host.writes, 1, "the launcher must not restore its value");
        assert_eq!(host.paste_attempts, 0);
    }

    #[test]
    fn denied_permission_secure_input_and_ime_block_injection() {
        for configure in [
            |host: &mut FakeHost| host.observed.permission = TargetPermission::Denied,
            |host: &mut FakeHost| host.observed.permission = TargetPermission::Unknown,
            |host: &mut FakeHost| host.observed.secure_input = true,
            |host: &mut FakeHost| host.observed.ime_composing = true,
        ] {
            let mut host = FakeHost::new();
            configure(&mut host);
            let outcome = run(&mut host, InsertionPolicy::Automatic, enabled());
            assert!(
                matches!(outcome, InsertionOutcome::Denied { copied: true, .. }),
                "unexpected outcome: {outcome:?}"
            );
            assert_eq!(host.paste_attempts, 0);
        }
    }

    #[test]
    fn captured_permission_denial_blocks_before_observation() {
        let mut host = FakeHost::new();
        let mut denied_target = target();
        denied_target.permission = TargetPermission::Denied;
        let outcome = execute_insertion(
            &mut host,
            &denied_target,
            &content(),
            InsertionPolicy::Automatic,
            enabled(),
        );
        assert!(matches!(
            outcome,
            InsertionOutcome::Denied { copied: true, .. }
        ));
        assert_eq!(host.focus_requests, 0);
    }

    #[test]
    fn denied_focus_is_denied_with_the_copy_preserved() {
        let mut host = FakeHost::new();
        host.observed.foreground = false;
        host.focus = FocusOutcome::Denied;
        let outcome = run(&mut host, InsertionPolicy::Automatic, enabled());
        assert!(matches!(
            outcome,
            InsertionOutcome::Denied { copied: true, .. }
        ));
        assert_eq!(host.paste_attempts, 0);
    }

    #[test]
    fn unverified_focus_does_not_attempt_paste() {
        let mut host = FakeHost::new();
        host.observed.foreground = false;
        host.focus = FocusOutcome::Unknown;
        let outcome = run(&mut host, InsertionPolicy::Automatic, enabled());
        assert!(matches!(outcome, InsertionOutcome::CopyFallback { .. }));
        assert_eq!(host.paste_attempts, 0);
    }

    #[test]
    fn focus_requests_can_be_disabled() {
        let mut host = FakeHost::new();
        host.observed.foreground = false;
        let config = InsertionConfig {
            allow_focus_request: false,
            ..enabled()
        };
        let outcome = run(&mut host, InsertionPolicy::Automatic, config);
        assert!(matches!(outcome, InsertionOutcome::CopyFallback { .. }));
        assert_eq!(host.focus_requests, 0);
    }

    #[test]
    fn clipboard_write_failure_and_foreign_ownership_are_denied() {
        let mut host = FakeHost::new();
        host.write_fails = true;
        let outcome = run(&mut host, InsertionPolicy::CopyOnly, enabled());
        assert!(matches!(
            outcome,
            InsertionOutcome::Denied { copied: false, .. }
        ));
        assert!(!outcome.copied());

        let mut host = FakeHost::new();
        host.owner = ClipboardOwner::Other;
        let outcome = run(&mut host, InsertionPolicy::CopyOnly, enabled());
        assert!(matches!(
            outcome,
            InsertionOutcome::Denied { copied: false, .. }
        ));
    }

    #[test]
    fn host_query_and_input_failures_fall_back_to_copy() {
        let mut host = FakeHost::new();
        host.observe_fails = true;
        let outcome = run(&mut host, InsertionPolicy::Automatic, enabled());
        assert!(matches!(outcome, InsertionOutcome::CopyFallback { .. }));
        assert_eq!(host.paste_attempts, 0);

        let mut host = FakeHost::new();
        host.paste = PasteAttempt::Unsupported;
        let outcome = run(&mut host, InsertionPolicy::Automatic, enabled());
        assert!(matches!(outcome, InsertionOutcome::CopyFallback { .. }));

        let mut host = FakeHost::new();
        host.paste = PasteAttempt::Failed;
        let outcome = run(&mut host, InsertionPolicy::Automatic, enabled());
        assert!(matches!(outcome, InsertionOutcome::CopyFallback { .. }));
    }

    #[test]
    fn every_target_issue_has_a_reason() {
        for issue in [
            TargetIssue::NotPresent,
            TargetIssue::ProcessChanged,
            TargetIssue::ProcessRecreated,
            TargetIssue::WindowChanged,
            TargetIssue::GenerationChanged,
            TargetIssue::PermissionDenied,
            TargetIssue::SecureInput,
            TargetIssue::ImeComposing,
            TargetIssue::NotForeground,
        ] {
            assert!(!issue.reason().is_empty());
        }
    }

    #[test]
    fn outcome_labels_are_stable_and_truthful() {
        let cases = [
            (InsertionOutcome::Copied { sequence: 1 }, "copied"),
            (InsertionOutcome::Inserted { sequence: 1 }, "inserted"),
            (
                InsertionOutcome::CopyFallback {
                    reason: "x".into(),
                    sequence: 1,
                },
                "copy-fallback",
            ),
            (
                InsertionOutcome::Denied {
                    reason: "x".into(),
                    copied: false,
                },
                "denied",
            ),
            (
                InsertionOutcome::Unverified {
                    reason: "x".into(),
                    sequence: 1,
                },
                "unverified",
            ),
        ];
        for (outcome, label) in cases {
            assert_eq!(outcome.label(), label);
        }
    }
}
