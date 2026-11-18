//! Deterministic property tests for the typed view stack.
//!
//! The generator is a tiny xorshift PRNG so the suite stays dependency-free and
//! reproducible. Eight seeds × 500 operations give 4000 random
//! push/pop/escape steps, well above the 1000-sequence requirement.

use ansa_core::navigation::{EscapeAction, MAX_VIEW_DEPTH, ParentActions, ScrollAnchor, ViewId};
use ansa_core::{Engine, EscapeOutcome};
use ansa_domain::{BuiltinCommand, Command, CommandId, CommandKind};

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

fn command(id: &str, title: &str) -> Command {
    Command::new(
        CommandId::parse(id).unwrap(),
        title,
        CommandKind::Builtin(BuiltinCommand::About),
    )
}

fn engine() -> Engine {
    Engine::new(vec![
        command("app:alpha", "Alpha"),
        command("app:beta", "Beta"),
        command("app:gamma", "Gamma"),
        command("app:delta", "Delta"),
    ])
}

fn query_for(value: usize) -> &'static str {
    match value % 5 {
        0 => "",
        1 => "a",
        2 => "al",
        3 => "b",
        _ => "zz-no-match",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    query: String,
    selected: Option<CommandId>,
    scroll: ScrollAnchor,
}

fn snapshot(engine: &Engine, scroll: ScrollAnchor) -> Snapshot {
    Snapshot {
        query: engine.query().to_owned(),
        selected: engine.selected().map(|command| command.id.clone()),
        scroll,
    }
}

#[test]
fn random_push_pop_escape_sequences_stay_bounded_and_restore_once() {
    for seed in 1..=8 {
        let mut rng = Rng::new((seed as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut engine = engine();
        let mut model: Vec<Snapshot> = Vec::new();
        let mut last_generation = engine.view_generation();
        let mut restored_generations = std::collections::HashSet::new();
        let mut hidden = engine.is_hidden();

        for step in 0..500 {
            match rng.below(10) {
                0..=2 if model.len() < 8 => {
                    let scroll = ScrollAnchor::at(rng.below(4));
                    let parent = snapshot(&engine, scroll);
                    let returned = engine
                        .push_view(
                            ViewId::new(format!("probe:{}", rng.below(6))).unwrap(),
                            scroll,
                        )
                        .expect("below the depth bound");
                    assert_ne!(returned, 0);
                    assert!(returned > last_generation);
                    last_generation = engine.view_generation();
                    model.push(parent);
                    // The child view gets its own transient query.
                    engine.set_query(query_for(rng.below(5)));
                }
                3 | 4 => {
                    let restored = engine.pop_view();
                    match model.pop() {
                        Some(expected) => {
                            let restored = restored.expect("model has a frame");
                            assert_eq!(restored.query(), expected.query);
                            assert_eq!(restored.scroll(), expected.scroll);
                            if let Some(selected) = &expected.selected {
                                assert_eq!(
                                    engine.selected().map(|command| &command.id),
                                    Some(selected),
                                    "restored selection must be the stable identity"
                                );
                            }
                            assert_eq!(restored.selected(), expected.selected.as_ref());
                            assert!(restored.generation() > restored.frame_generation());
                            assert!(
                                restored_generations.insert(restored.frame_generation()),
                                "frame {} restored twice",
                                restored.frame_generation()
                            );
                        }
                        None => assert!(restored.is_none(), "root must never be popped"),
                    }
                }
                5 => {
                    let composition = rng.below(4) == 0;
                    engine.set_composition_active(composition);
                    let actions_open = engine.actions_open();
                    let depth = engine.view_depth();
                    let outcome = engine.resolve_escape();
                    if composition {
                        assert_eq!(outcome, EscapeAction::CancelComposition);
                        assert_eq!(engine.actions_open(), actions_open);
                        assert_eq!(engine.view_depth(), depth);
                        assert_eq!(engine.is_hidden(), hidden);
                    } else if actions_open {
                        assert_eq!(outcome, EscapeAction::CloseActions);
                        assert_eq!(engine.view_depth(), depth);
                    } else if depth > 0 {
                        let EscapeAction::RestoreView(restored) = outcome else {
                            panic!("depth {depth} must restore a view");
                        };
                        let expected = model.pop().expect("model depth mirrors engine");
                        assert_eq!(restored.query(), expected.query);
                        assert_eq!(restored.scroll(), expected.scroll);
                        assert_eq!(engine.query(), expected.query);
                        assert!(restored_generations.insert(restored.frame_generation()));
                    } else if !hidden {
                        assert_eq!(outcome, EscapeAction::HideRoot);
                        hidden = true;
                    } else {
                        assert_eq!(outcome, EscapeAction::None);
                    }
                    engine.set_composition_active(false);
                }
                6 => {
                    if rng.below(3) == 0 {
                        engine.begin_invocation();
                        hidden = false;
                    } else if rng.below(2) == 0 {
                        engine.move_selection(if rng.below(2) == 0 { 1 } else { -1 });
                    } else {
                        engine.set_query(query_for(rng.below(5)));
                    }
                }
                7 => {
                    if rng.below(2) == 0 {
                        engine.open_actions();
                    } else {
                        engine.close_actions();
                    }
                }
                8 => {
                    if engine.view_depth() > 0 {
                        let restored = engine.pop_view().unwrap();
                        let expected = model.pop().unwrap();
                        assert_eq!(restored.query(), expected.query);
                        assert!(restored_generations.insert(restored.frame_generation()));
                    }
                }
                _ => {
                    // Escape through the legacy wrapper must agree with the
                    // typed plan's state effects.
                    engine.set_composition_active(false);
                    engine.close_actions();
                    let depth = engine.view_depth();
                    let outcome = engine.escape();
                    if depth > 0 {
                        assert_eq!(outcome, EscapeOutcome::ViewPopped);
                        let expected = model.pop().unwrap();
                        assert_eq!(engine.query(), expected.query);
                    } else if !hidden {
                        assert_eq!(outcome, EscapeOutcome::Dismiss);
                        hidden = true;
                    }
                }
            }

            assert_eq!(engine.view_depth(), model.len(), "step {step} seed {seed}");
            assert!(engine.view_depth() <= MAX_VIEW_DEPTH);
            assert!(engine.view_generation() >= last_generation);
            last_generation = engine.view_generation();
            let _ = ParentActions::Closed;
        }
    }
}

#[test]
fn nested_views_close_in_order_with_exact_parent_state_once() {
    let mut engine = engine();
    engine.set_query("alpha");
    assert!(engine.select_index(0));
    let alpha = engine.selected().unwrap().id.clone();

    let outer_parent = Snapshot {
        query: engine.query().to_owned(),
        selected: Some(alpha.clone()),
        scroll: ScrollAnchor::at(3),
    };
    engine
        .push_view(ViewId::new("ansa:outer").unwrap(), outer_parent.scroll)
        .unwrap();
    engine.set_query("beta");
    assert!(engine.select_index(0));
    let beta = engine.selected().unwrap().id.clone();
    let inner_parent = Snapshot {
        query: engine.query().to_owned(),
        selected: Some(beta.clone()),
        scroll: ScrollAnchor::at(1),
    };
    engine
        .push_view(ViewId::new("ansa:inner").unwrap(), inner_parent.scroll)
        .unwrap();
    engine.set_query("gamma");
    assert_eq!(engine.view_depth(), 2);

    let revision_before = engine.query_token().revision();
    let inner = match engine.resolve_escape() {
        EscapeAction::RestoreView(restored) => restored,
        other => panic!("expected inner restore, got {other:?}"),
    };
    assert_eq!(inner.view().as_str(), "ansa:inner");
    assert_eq!(inner.query(), "beta");
    assert_eq!(inner.selected(), Some(&beta));
    assert_eq!(inner.scroll(), ScrollAnchor::at(1));
    assert_eq!(engine.query(), "beta");
    assert_eq!(engine.selected().map(|command| &command.id), Some(&beta));
    assert_eq!(engine.view_depth(), 1);
    assert_eq!(
        engine.query_token().revision(),
        revision_before + 1,
        "restoration bumps the revision exactly once"
    );

    let outer = match engine.resolve_escape() {
        EscapeAction::RestoreView(restored) => restored,
        other => panic!("expected outer restore, got {other:?}"),
    };
    assert_eq!(outer.view().as_str(), "ansa:outer");
    assert_eq!(outer.query(), "alpha");
    assert_eq!(outer.selected(), Some(&alpha));
    assert_eq!(outer.scroll(), ScrollAnchor::at(3));
    assert_eq!(engine.query(), "alpha");
    assert_eq!(engine.selected().map(|command| &command.id), Some(&alpha));
    assert_eq!(engine.view_depth(), 0);
    assert_eq!(engine.current_view().as_str(), "root");

    assert_eq!(engine.resolve_escape(), EscapeAction::HideRoot);
    assert!(engine.is_hidden());
    assert_eq!(engine.resolve_escape(), EscapeAction::None);
}

#[test]
fn escape_hierarchy_is_composition_then_actions_then_view_then_root() {
    let mut engine = engine();
    engine.set_query("alpha");
    engine
        .push_view(ViewId::new("ansa:about").unwrap(), ScrollAnchor::at(2))
        .unwrap();
    engine.open_actions();
    engine.set_composition_active(true);

    assert_eq!(engine.resolve_escape(), EscapeAction::CancelComposition);
    assert!(engine.actions_open());
    assert_eq!(engine.view_depth(), 1);
    assert!(!engine.is_hidden());

    engine.set_composition_active(false);
    assert_eq!(engine.resolve_escape(), EscapeAction::CloseActions);
    assert!(!engine.actions_open());
    assert_eq!(engine.view_depth(), 1);

    assert!(matches!(
        engine.resolve_escape(),
        EscapeAction::RestoreView(_)
    ));
    assert_eq!(engine.view_depth(), 0);
    assert_eq!(engine.query(), "alpha");

    assert_eq!(engine.resolve_escape(), EscapeAction::HideRoot);
    assert_eq!(engine.resolve_escape(), EscapeAction::None);
}
