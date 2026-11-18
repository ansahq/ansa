//! Public-contract checks for the shared input widget.
//!
//! These tests deliberately stay outside the GPUI window stack: they pin the
//! typed composition/action hierarchy, the size limits and the user-visible
//! rejection reason that hosts depend on.

use ansa_ui_components::{
    EditIntent, EditIntents, EditOutcome, EditRejected, EditRejection, MAX_HISTORY_BYTES,
    MAX_HISTORY_ENTRIES, MAX_TEXT_SCALARS,
};

const INTENTS: [EditIntent; 11] = [
    EditIntent::Undo,
    EditIntent::Redo,
    EditIntent::Copy,
    EditIntent::Cut,
    EditIntent::Paste,
    EditIntent::MoveCaret,
    EditIntent::ExtendSelection,
    EditIntent::Backspace,
    EditIntent::Delete,
    EditIntent::Enter,
    EditIntent::Escape,
];

#[test]
fn composition_defers_every_action_intent() {
    let composing = EditIntents::new(true);
    assert!(composing.is_composing());

    for intent in INTENTS {
        assert!(
            !composing.allows(intent),
            "{intent:?} must not fire during composition"
        );
        assert_ne!(
            composing.outcome(intent),
            EditOutcome::Execute,
            "{intent:?} must not execute during composition"
        );
    }

    assert_eq!(
        composing.outcome(EditIntent::Escape),
        EditOutcome::CancelComposition
    );
    assert_eq!(
        composing.outcome(EditIntent::Enter),
        EditOutcome::DeferToComposition
    );
}

#[test]
fn idle_input_executes_every_intent() {
    let idle = EditIntents::new(false);
    assert!(!idle.is_composing());

    for intent in INTENTS {
        assert!(idle.allows(intent), "{intent:?} must run when idle");
        assert_eq!(idle.outcome(intent), EditOutcome::Execute);
    }
}

#[test]
fn rejection_reason_is_user_visible() {
    let event = EditRejected {
        reason: EditRejection::TextTooLong {
            limit: MAX_TEXT_SCALARS,
            would_be: MAX_TEXT_SCALARS + 1,
        },
    };

    assert_eq!(
        event.reason,
        EditRejection::TextTooLong {
            limit: MAX_TEXT_SCALARS,
            would_be: MAX_TEXT_SCALARS + 1,
        }
    );
    assert_eq!(event.message(), "Search is limited to 4,096 characters");
    assert!(!event.message().is_empty());
}

fn positive(value: usize) -> bool {
    value > 0
}

fn at_least(value: usize, minimum: usize) -> bool {
    value >= minimum
}

#[test]
fn documented_limits_are_positive_and_consistent() {
    assert!(positive(MAX_TEXT_SCALARS));
    assert!(positive(MAX_HISTORY_ENTRIES));
    assert!(at_least(MAX_HISTORY_BYTES, MAX_TEXT_SCALARS * 4));
}
