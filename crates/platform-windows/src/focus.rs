//! Checked, best-effort capture and restoration of the user's foreground window.
//!
//! This module implements the launcher dismissal sequence:
//!
//! 1. Before the launcher is shown, [`capture_foreground_target`] records the
//!    foreground top-level window together with its process id, creating
//!    thread, process creation time and an open, query-only process handle.
//! 2. On dismissal, [`dismiss_launcher`] hides the launcher, and considers focus
//!    return only while the launcher still owns the foreground and the captured
//!    target remains live, visible and top-level. A different foreground window
//!    observed after hiding is left alone.
//!
//! Focus restoration is a checked request, not a guarantee. Windows can deny
//! foreground activation, and the returned [`FocusRestoreOutcome`] reports
//! exactly what was observed. No input is injected, no thread input is
//! attached, no elevation is requested, and no secure-desktop path is
//! attempted.
//!
//! ## Residual identity limits
//!
//! Holding a process handle prevents that process id from being recycled while
//! the [`FocusTarget`] is alive, and the stored creation time is verified
//! again before restoration. Window handles are not generation-tagged, though:
//! if the original window is destroyed and a new window with the same handle
//! value is created by the same process and thread, the verification cannot
//! distinguish them. Do not treat a `FocusTarget` as a security token or as
//! proof that a particular window object still exists.

#[cfg(windows)]
mod native;

/// An opaque, thread-safe record of the foreground window captured before the
/// launcher was shown.
///
/// The handle is intentionally not exposed. Dropping all clones releases the
/// process handle, which is what keeps the process id from being recycled while
/// the target is held. Cloning shares the same process handle.
///
/// The type is `Send` and `Sync` so it can be captured on the hotkey worker and
/// consumed on the UI thread.
#[cfg(windows)]
#[derive(Clone, Debug)]
pub struct FocusTarget {
    pub(crate) data: native::TargetData,
}

/// Non-Windows placeholder matching the Windows `FocusTarget` API.
#[cfg(not(windows))]
#[derive(Clone, Debug)]
pub struct FocusTarget;

/// The observed result of a checked launcher dismissal.
#[must_use]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FocusRestoreOutcome {
    /// The launcher was hidden and the captured window was observed as the
    /// foreground window afterward.
    Restored,
    /// The launcher was hidden, but focus restoration was intentionally not
    /// attempted. This is the safe outcome when the user switched away, no
    /// target was captured, or the captured target no longer passed
    /// verification.
    Skipped {
        /// Human-readable reason suitable for diagnostics or a status line.
        reason: String,
    },
    /// The launcher was hidden and restoration was attempted, but Windows
    /// denied activation or the target changed before activation completed.
    Denied {
        /// Human-readable reason suitable for diagnostics or a status line.
        reason: String,
    },
}

impl FocusRestoreOutcome {
    /// Returns `true` only for [`Self::Restored`].
    pub fn is_restored(&self) -> bool {
        matches!(self, Self::Restored)
    }

    /// Returns a stable label for logs and UI.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Restored => "restored",
            Self::Skipped { .. } => "skipped",
            Self::Denied { .. } => "denied",
        }
    }
}

/// Captures the current foreground window as a possible restoration target.
///
/// Call this on the hotkey worker immediately before showing the launcher, or
/// on the UI thread before the launcher window is created at startup. The call
/// is not thread-affine: `GetForegroundWindow` and the process-identity queries
/// are safe from any thread.
///
/// `excluded` should be the launcher's HWND when one already exists (for
/// example on a hide/show toggle), so the launcher cannot capture itself.
///
/// Returns `None` when there is no eligible target, including when:
/// * no window is foreground, or the foreground window is not a live top-level
///   window;
/// * the window belongs to this process, or is the excluded launcher window;
/// * the window is hidden or minimized;
/// * the owning process cannot be opened for limited query access, so process
///   identity cannot be protected against PID reuse; or
/// * the owning process has already exited.
///
/// The returned value holds a process handle and records the process creation
/// time. See the module documentation for the residual HWND-reuse limitation.
///
/// If the launcher is already foreground, this returns `None` because its own
/// process is rejected. Keep the target captured when the launcher was shown;
/// use this call while the launcher is visible only when the user has switched
/// to another application and a new target is actually needed.
#[cfg(windows)]
pub fn capture_foreground_target(excluded: Option<isize>) -> Option<FocusTarget> {
    native::capture(excluded)
}

/// Non-Windows stub: no native foreground capture is implemented, so this
/// always returns `None`.
#[cfg(not(windows))]
pub fn capture_foreground_target(_excluded: Option<isize>) -> Option<FocusTarget> {
    None
}

/// Returns whether `launcher_hwnd` is the current foreground window.
///
/// This is a UI-thread helper used to decide between dismiss and summon:
/// * when the launcher is visible and this returns `true`, the hotkey should
///   dismiss it (hide and restore);
/// * when the launcher is visible and this returns `false`, the user has
///   switched away, so the hotkey should capture a new target and activate the
///   launcher instead of hiding it.
///
/// The check is deliberately conservative and fails closed. It returns `false`
/// for a null handle, a stale handle, a window whose creating thread or process
/// is not the calling thread/process, or a foreground window other than
/// `launcher_hwnd`. It never changes visibility, focus or z-order.
///
/// Call this on the UI thread that owns the launcher. The check is safe to
/// call from other threads, but a foreign-thread window is rejected rather
/// than treated as the launcher. On non-Windows builds this always returns
/// `false`.
#[cfg(windows)]
pub fn launcher_is_foreground(launcher_hwnd: isize) -> bool {
    native::launcher_is_foreground(launcher_hwnd)
}

/// Non-Windows stub: there is no native launcher HWND to inspect.
#[cfg(not(windows))]
pub fn launcher_is_foreground(_launcher_hwnd: isize) -> bool {
    false
}

/// Hides the launcher and, only when it is still safe, restores the captured
/// foreground target.
///
/// This must run on the UI thread that owns `launcher_hwnd`, because hiding the
/// window is thread-affine and is checked before any state change.
///
/// Sequence:
/// 1. Validate that `launcher_hwnd` is a live, top-level window owned by the
///    calling thread and process.
/// 2. Check whether the launcher still owns the foreground. If it does not, the
///    launcher is hidden and the user's current focus is left untouched
///    (`Skipped`).
/// 3. Otherwise, if a target was supplied, verify it before hiding: it must
///    still be a live, visible, non-minimized top-level window with matching
///    PID, creating thread, live process and process creation time. A failed
///    check hides the launcher without restoring (`Skipped`).
/// 4. Re-check foreground ownership, hide, and re-verify the target. If an
///    unrelated window is now foreground, leave it alone. Otherwise request
///    activation where needed and observe the actual foreground window. The result is
///    `Restored` only if the target is observed as foreground; otherwise it is
///    `Denied`.
///
/// Returns `Err` for caller misuse (null, stale, foreign, non-top-level or
/// wrong-thread launcher handles) and for a failed hide. Those errors occur
/// before or instead of a claimed focus change.
///
/// `target` may be `None` when the caller deliberately wants only to hide the
/// launcher, matching the behavior of [`crate::set_launcher_visible`].
#[cfg(windows)]
pub fn dismiss_launcher(
    launcher_hwnd: isize,
    target: Option<&FocusTarget>,
) -> Result<FocusRestoreOutcome, String> {
    native::dismiss(launcher_hwnd, target)
}

/// Non-Windows stub: native launcher dismissal is unsupported.
#[cfg(not(windows))]
pub fn dismiss_launcher(
    _launcher_hwnd: isize,
    _target: Option<&FocusTarget>,
) -> Result<FocusRestoreOutcome, String> {
    Err(crate::unsupported("dismissing the launcher"))
}

/// Pure policy shared by the native implementation and platform-neutral tests.
///
/// Keeping the decisions here means the unsafe Win32 layer only gathers
/// observations and performs the final, checked calls.
#[cfg(any(windows, test))]
mod policy {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum PostHideAction {
        AlreadyRestored,
        RequestActivation,
        LeaveUnchanged,
    }

    pub(crate) fn post_hide_action(
        target_foreground: bool,
        launcher_foreground: bool,
        has_foreground: bool,
    ) -> PostHideAction {
        if target_foreground {
            PostHideAction::AlreadyRestored
        } else if launcher_foreground || !has_foreground {
            PostHideAction::RequestActivation
        } else {
            PostHideAction::LeaveUnchanged
        }
    }
    /// A snapshot of the candidate foreground window used by the capture policy.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub(crate) struct CaptureSnapshot {
        pub(crate) is_window: bool,
        pub(crate) excluded: bool,
        pub(crate) own_process: bool,
        pub(crate) visible: bool,
        pub(crate) minimized: bool,
        pub(crate) top_level: bool,
        pub(crate) process_opened: bool,
        pub(crate) process_alive: bool,
    }

    /// Why a foreground candidate was not captured.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum CaptureRejection {
        NotWindow,
        Excluded,
        OwnProcess,
        NotVisible,
        Minimized,
        NotTopLevel,
        ProcessUnavailable,
        ProcessExited,
    }

    /// Returns `Ok(())` only for a live, visible, non-minimized, top-level
    /// foreign window whose process identity could be pinned.
    pub(crate) fn capture_target_snapshot(
        snapshot: CaptureSnapshot,
    ) -> Result<(), CaptureRejection> {
        if !snapshot.is_window {
            return Err(CaptureRejection::NotWindow);
        }
        if snapshot.excluded {
            return Err(CaptureRejection::Excluded);
        }
        if snapshot.own_process {
            return Err(CaptureRejection::OwnProcess);
        }
        if !snapshot.visible {
            return Err(CaptureRejection::NotVisible);
        }
        if snapshot.minimized {
            return Err(CaptureRejection::Minimized);
        }
        if !snapshot.top_level {
            return Err(CaptureRejection::NotTopLevel);
        }
        if !snapshot.process_opened {
            return Err(CaptureRejection::ProcessUnavailable);
        }
        if !snapshot.process_alive {
            return Err(CaptureRejection::ProcessExited);
        }
        Ok(())
    }

    /// A snapshot of a previously captured target at restoration time.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub(crate) struct TargetSnapshot {
        pub(crate) is_window: bool,
        pub(crate) same_as_launcher: bool,
        pub(crate) own_process: bool,
        pub(crate) same_pid: bool,
        pub(crate) same_thread: bool,
        pub(crate) alive: bool,
        pub(crate) creation_time_matches: bool,
        pub(crate) visible: bool,
        pub(crate) minimized: bool,
        pub(crate) top_level: bool,
    }

    /// Why a captured target is no longer safe to restore.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum TargetIssue {
        NotWindow,
        LauncherWindow,
        OwnProcess,
        ProcessMismatch,
        ThreadMismatch,
        ProcessExited,
        ProcessRecreated,
        NotVisible,
        Minimized,
        NotTopLevel,
    }

    impl TargetIssue {
        /// Returns a human-readable reason for diagnostics.
        pub(crate) fn reason(self) -> &'static str {
            match self {
                Self::NotWindow => "the captured window handle is no longer a live window",
                Self::LauncherWindow => "the captured window is the launcher's own window",
                Self::OwnProcess => "the captured window belongs to this process",
                Self::ProcessMismatch => "the window now belongs to a different process",
                Self::ThreadMismatch => "the window now belongs to a different creating thread",
                Self::ProcessExited => "the captured process has exited",
                Self::ProcessRecreated => {
                    "the process creation time no longer matches; the process id may have been reused"
                }
                Self::NotVisible => "the window is not visible",
                Self::Minimized => "the window is minimized",
                Self::NotTopLevel => "the window is not a top-level window",
            }
        }
    }

    /// Verifies that the captured window is still the same live target.
    pub(crate) fn verify_target_snapshot(snapshot: TargetSnapshot) -> Result<(), TargetIssue> {
        if !snapshot.is_window {
            return Err(TargetIssue::NotWindow);
        }
        if snapshot.same_as_launcher {
            return Err(TargetIssue::LauncherWindow);
        }
        if snapshot.own_process {
            return Err(TargetIssue::OwnProcess);
        }
        if !snapshot.same_pid {
            return Err(TargetIssue::ProcessMismatch);
        }
        if !snapshot.same_thread {
            return Err(TargetIssue::ThreadMismatch);
        }
        if !snapshot.alive {
            return Err(TargetIssue::ProcessExited);
        }
        if !snapshot.creation_time_matches {
            return Err(TargetIssue::ProcessRecreated);
        }
        if !snapshot.visible {
            return Err(TargetIssue::NotVisible);
        }
        if snapshot.minimized {
            return Err(TargetIssue::Minimized);
        }
        if !snapshot.top_level {
            return Err(TargetIssue::NotTopLevel);
        }
        Ok(())
    }

    /// Inputs that decide whether dismissal may request focus restoration.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub(crate) struct DismissPolicyInput {
        pub(crate) launcher_owns_foreground: bool,
        pub(crate) target_supplied: bool,
        pub(crate) target_valid: bool,
    }

    /// What dismissal is allowed to do after the launcher is hidden.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum DismissAction {
        /// Hide only; never activate another window.
        HideOnly,
        /// Hide, then request activation of the verified target.
        HideAndRestore,
    }

    /// Focus is restored only when the launcher itself owned the foreground
    /// and a supplied target passed verification. Any other state must not
    /// steal focus from the user's current window.
    pub(crate) fn dismiss_action(input: DismissPolicyInput) -> DismissAction {
        if input.launcher_owns_foreground && input.target_supplied && input.target_valid {
            DismissAction::HideAndRestore
        } else {
            DismissAction::HideOnly
        }
    }
}

#[cfg(test)]
mod tests {
    use super::policy::{
        capture_target_snapshot, dismiss_action, verify_target_snapshot, CaptureRejection,
        CaptureSnapshot, DismissAction, DismissPolicyInput, TargetIssue, TargetSnapshot,
    };
    use super::{FocusRestoreOutcome, FocusTarget};

    #[test]
    fn a_new_foreground_app_after_hiding_is_left_alone() {
        use super::policy::{post_hide_action, PostHideAction};
        assert_eq!(
            post_hide_action(false, false, true),
            PostHideAction::LeaveUnchanged
        );
        assert_eq!(
            post_hide_action(true, false, true),
            PostHideAction::AlreadyRestored
        );
        assert_eq!(
            post_hide_action(false, false, false),
            PostHideAction::RequestActivation
        );
        assert_eq!(
            post_hide_action(false, true, true),
            PostHideAction::RequestActivation
        );
    }

    fn valid_capture() -> CaptureSnapshot {
        CaptureSnapshot {
            is_window: true,
            excluded: false,
            own_process: false,
            visible: true,
            minimized: false,
            top_level: true,
            process_opened: true,
            process_alive: true,
        }
    }

    fn valid_target() -> TargetSnapshot {
        TargetSnapshot {
            is_window: true,
            same_as_launcher: false,
            own_process: false,
            same_pid: true,
            same_thread: true,
            alive: true,
            creation_time_matches: true,
            visible: true,
            minimized: false,
            top_level: true,
        }
    }

    #[test]
    fn capture_policy_accepts_a_live_visible_top_level_foreign_window() {
        assert_eq!(capture_target_snapshot(valid_capture()), Ok(()));
    }

    #[test]
    fn capture_policy_rejects_each_unsafe_candidate() {
        let cases = [
            (
                CaptureSnapshot {
                    is_window: false,
                    ..valid_capture()
                },
                CaptureRejection::NotWindow,
            ),
            (
                CaptureSnapshot {
                    excluded: true,
                    ..valid_capture()
                },
                CaptureRejection::Excluded,
            ),
            (
                CaptureSnapshot {
                    own_process: true,
                    ..valid_capture()
                },
                CaptureRejection::OwnProcess,
            ),
            (
                CaptureSnapshot {
                    visible: false,
                    ..valid_capture()
                },
                CaptureRejection::NotVisible,
            ),
            (
                CaptureSnapshot {
                    minimized: true,
                    ..valid_capture()
                },
                CaptureRejection::Minimized,
            ),
            (
                CaptureSnapshot {
                    top_level: false,
                    ..valid_capture()
                },
                CaptureRejection::NotTopLevel,
            ),
            (
                CaptureSnapshot {
                    process_opened: false,
                    ..valid_capture()
                },
                CaptureRejection::ProcessUnavailable,
            ),
            (
                CaptureSnapshot {
                    process_alive: false,
                    ..valid_capture()
                },
                CaptureRejection::ProcessExited,
            ),
        ];

        for (snapshot, expected) in cases {
            assert_eq!(capture_target_snapshot(snapshot), Err(expected));
        }
    }

    #[test]
    fn target_policy_accepts_the_same_live_visible_top_level_window() {
        assert_eq!(verify_target_snapshot(valid_target()), Ok(()));
    }

    #[test]
    fn target_policy_rejects_recycled_or_unsafe_windows() {
        let cases = [
            (
                TargetSnapshot {
                    is_window: false,
                    ..valid_target()
                },
                TargetIssue::NotWindow,
            ),
            (
                TargetSnapshot {
                    same_as_launcher: true,
                    ..valid_target()
                },
                TargetIssue::LauncherWindow,
            ),
            (
                TargetSnapshot {
                    own_process: true,
                    ..valid_target()
                },
                TargetIssue::OwnProcess,
            ),
            (
                TargetSnapshot {
                    same_pid: false,
                    ..valid_target()
                },
                TargetIssue::ProcessMismatch,
            ),
            (
                TargetSnapshot {
                    same_thread: false,
                    ..valid_target()
                },
                TargetIssue::ThreadMismatch,
            ),
            (
                TargetSnapshot {
                    alive: false,
                    ..valid_target()
                },
                TargetIssue::ProcessExited,
            ),
            (
                TargetSnapshot {
                    creation_time_matches: false,
                    ..valid_target()
                },
                TargetIssue::ProcessRecreated,
            ),
            (
                TargetSnapshot {
                    visible: false,
                    ..valid_target()
                },
                TargetIssue::NotVisible,
            ),
            (
                TargetSnapshot {
                    minimized: true,
                    ..valid_target()
                },
                TargetIssue::Minimized,
            ),
            (
                TargetSnapshot {
                    top_level: false,
                    ..valid_target()
                },
                TargetIssue::NotTopLevel,
            ),
        ];

        for (snapshot, expected) in cases {
            assert_eq!(verify_target_snapshot(snapshot), Err(expected));
        }
    }

    #[test]
    fn dismiss_restores_only_while_the_launcher_owns_the_foreground() {
        let restore = DismissPolicyInput {
            launcher_owns_foreground: true,
            target_supplied: true,
            target_valid: true,
        };
        assert_eq!(dismiss_action(restore), DismissAction::HideAndRestore);

        for input in [
            DismissPolicyInput {
                launcher_owns_foreground: false,
                ..restore
            },
            DismissPolicyInput {
                target_supplied: false,
                ..restore
            },
            DismissPolicyInput {
                target_valid: false,
                ..restore
            },
        ] {
            assert_eq!(dismiss_action(input), DismissAction::HideOnly);
        }
    }

    #[test]
    fn outcomes_do_not_claim_restoration_without_observation() {
        assert!(FocusRestoreOutcome::Restored.is_restored());

        let skipped = FocusRestoreOutcome::Skipped {
            reason: "the user switched away".to_owned(),
        };
        assert!(!skipped.is_restored());
        assert_eq!(skipped.label(), "skipped");

        let denied = FocusRestoreOutcome::Denied {
            reason: "foreground activation was denied".to_owned(),
        };
        assert!(!denied.is_restored());
        assert_eq!(denied.label(), "denied");
    }

    #[test]
    fn every_target_issue_has_a_nonempty_reason() {
        for issue in [
            TargetIssue::NotWindow,
            TargetIssue::LauncherWindow,
            TargetIssue::OwnProcess,
            TargetIssue::ProcessMismatch,
            TargetIssue::ThreadMismatch,
            TargetIssue::ProcessExited,
            TargetIssue::ProcessRecreated,
            TargetIssue::NotVisible,
            TargetIssue::Minimized,
            TargetIssue::NotTopLevel,
        ] {
            assert!(!issue.reason().is_empty());
        }
    }

    #[test]
    fn focus_target_can_cross_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FocusTarget>();
    }
}
