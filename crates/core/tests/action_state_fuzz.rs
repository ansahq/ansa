//! Typed action-reference tests: delayed callbacks, stale contexts, malformed
//! input, and deterministic fuzzing without new dependencies.

use ansa_core::Engine;
use ansa_core::action_panel::ActionId;
use ansa_core::action_state::{
    ActionError, ActionKey, ActionRef, ActionRegistry, ActionTarget, MAX_ACTION_KEY_CHARS,
    MAX_ACTION_PARAM_CHARS, MAX_ACTION_PARAMS,
};
use ansa_core::navigation::{ScrollAnchor, ViewId};
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

    fn char_from(&mut self, alphabet: &[char]) -> char {
        alphabet[self.below(alphabet.len())]
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
    ])
}

fn issue_primary(engine: &Engine) -> Result<ActionRef, ActionError> {
    ActionRegistry::issue(
        engine,
        engine.views(),
        ActionKey::from(ActionId::Primary),
        &[],
    )
}

fn probe() -> ViewId {
    ViewId::new("ansa:probe").unwrap()
}

#[test]
fn delayed_callback_from_another_view_never_executes() {
    let mut engine = engine();
    assert_eq!(engine.selected().unwrap().id.as_str(), "app:alpha");
    let delayed = issue_primary(&engine).unwrap();
    assert_eq!(delayed.panel_action(), Some(ActionId::Primary));

    engine.push_view(probe(), ScrollAnchor::at(1)).unwrap();
    assert_eq!(
        ActionRegistry::resolve(&delayed, &engine, engine.views()),
        Err(ActionError::CrossView),
        "an old view's callback must not run in the child view"
    );

    let restored = engine.pop_view().unwrap();
    assert_eq!(restored.view().as_str(), "ansa:probe");
    assert_eq!(
        ActionRegistry::resolve(&delayed, &engine, engine.views()),
        Err(ActionError::ExpiredView),
        "the restored parent is a new frame instance"
    );

    let current = issue_primary(&engine).unwrap();
    let resolved = ActionRegistry::resolve(&current, &engine, engine.views()).unwrap();
    assert_eq!(
        resolved.target().command().map(CommandId::as_str),
        Some("app:alpha")
    );
    assert_eq!(resolved.panel_action(), Some(ActionId::Primary));
}

#[test]
fn stale_invocation_is_rejected() {
    let mut engine = engine();
    let reference = issue_primary(&engine).unwrap();
    engine.begin_invocation();
    assert_eq!(
        ActionRegistry::resolve(&reference, &engine, engine.views()),
        Err(ActionError::StaleInvocation)
    );
    assert!(issue_primary(&engine).is_ok());
}

#[test]
fn stale_revision_with_same_text_and_selection_is_rejected() {
    let mut engine = engine();
    engine.set_query("alpha");
    engine.select_index(0);
    let reference = issue_primary(&engine).unwrap();

    engine.set_query("alpha");
    assert_eq!(engine.selected().unwrap().id.as_str(), "app:alpha");
    assert_eq!(
        ActionRegistry::resolve(&reference, &engine, engine.views()),
        Err(ActionError::StaleTarget)
    );
}

#[test]
fn changed_selection_is_a_stale_target() {
    let mut engine = engine();
    let reference = issue_primary(&engine).unwrap();
    engine.move_selection(1);
    assert_eq!(
        ActionRegistry::resolve(&reference, &engine, engine.views()),
        Err(ActionError::StaleTarget)
    );
}

#[test]
fn cross_view_same_command_is_rejected() {
    let mut engine = engine();
    engine.set_query("alpha");
    let reference = issue_primary(&engine).unwrap();
    // Same invocation, query, revision and selected command; only the view
    // frame differs.
    engine.push_view(probe(), ScrollAnchor::TOP).unwrap();
    assert_eq!(engine.selected().unwrap().id.as_str(), "app:alpha");
    assert_eq!(
        ActionRegistry::resolve(&reference, &engine, engine.views()),
        Err(ActionError::CrossView)
    );
}

#[test]
fn same_view_identity_with_new_generation_is_expired() {
    let mut engine = engine();
    let reference = issue_primary(&engine).unwrap();
    engine.push_view(probe(), ScrollAnchor::TOP).unwrap();
    engine.pop_view().unwrap();
    assert_eq!(engine.current_view().as_str(), "root");
    assert_eq!(
        ActionRegistry::resolve(&reference, &engine, engine.views()),
        Err(ActionError::ExpiredView)
    );
}

#[test]
fn calculator_reference_is_bound_to_the_exact_expression() {
    let mut engine = Engine::default();
    engine.set_query("1 + 2");
    let reference = issue_primary(&engine).unwrap();
    let resolved = ActionRegistry::resolve(&reference, &engine, engine.views()).unwrap();
    assert_eq!(resolved.target().expression(), Some("1 + 2"));

    engine.set_query("1 + 3");
    assert_eq!(
        ActionRegistry::resolve(&reference, &engine, engine.views()),
        Err(ActionError::StaleTarget)
    );

    engine.set_query("1 + 2");
    assert_eq!(
        ActionRegistry::resolve(&reference, &engine, engine.views()),
        Err(ActionError::StaleTarget),
        "the same text in a later revision is a new snapshot"
    );
}

#[test]
fn unknown_and_malformed_actions_are_typed_errors() {
    let engine = engine();

    assert_eq!(ActionKey::parse(""), Err(ActionError::Malformed));
    assert_eq!(ActionKey::parse("has space"), Err(ActionError::Malformed));
    assert_eq!(ActionKey::parse("emoji😀"), Err(ActionError::Malformed));
    assert_eq!(
        ActionKey::parse("x".repeat(MAX_ACTION_KEY_CHARS + 1)),
        Err(ActionError::Malformed)
    );

    let unknown = ActionKey::parse("no-such-action").unwrap();
    assert_eq!(
        ActionRegistry::issue(&engine, engine.views(), unknown, &[]),
        Err(ActionError::UnknownAction)
    );

    let primary = ActionKey::primary();
    let too_many = ["a"; MAX_ACTION_PARAMS + 1];
    assert_eq!(
        ActionRegistry::issue(&engine, engine.views(), primary.clone(), &too_many),
        Err(ActionError::Malformed)
    );
    let overlong = "x".repeat(MAX_ACTION_PARAM_CHARS + 1);
    assert_eq!(
        ActionRegistry::issue(&engine, engine.views(), primary.clone(), &[&overlong]),
        Err(ActionError::Malformed)
    );
    assert_eq!(
        ActionRegistry::issue(&engine, engine.views(), primary.clone(), &["bad\nparam"]),
        Err(ActionError::Malformed)
    );

    // No selection and no calculator expression: nothing to bind to.
    let mut empty = Engine::default();
    assert_eq!(
        ActionRegistry::issue(&empty, empty.views(), primary, &[]),
        Err(ActionError::Malformed)
    );

    // Overlong calculator expression.
    empty.set_query(&"1".repeat(MAX_ACTION_PARAM_CHARS + 40));
    assert_eq!(issue_primary(&empty), Err(ActionError::Malformed));

    // Issuing is pure: the engine never changed.
    assert_eq!(engine.query(), "");
    assert_eq!(engine.selected().unwrap().id.as_str(), "app:alpha");
}

#[test]
fn fuzz_thousands_of_action_identities_never_panic() {
    let mut rng = Rng::new(0xC0FF_EE12_3456_789A);
    let engine = engine();
    let alphabet: Vec<char> = "abzAZ09-_.: /\\\n\t😀é"
        .chars()
        .chain(std::iter::once('\u{7}'))
        .collect();
    let mut malformed = 0usize;
    let mut known = 0usize;

    for _ in 0..4000 {
        let length = rng.below(300);
        let raw: String = (0..length).map(|_| rng.char_from(&alphabet)).collect();
        match ActionKey::parse(raw.clone()) {
            Err(ActionError::Malformed) => malformed += 1,
            Err(other) => panic!("parse produced an unexpected error: {other:?}"),
            Ok(key) => {
                known += 1;
                match ActionRegistry::issue(&engine, engine.views(), key, &[]) {
                    Ok(reference) => {
                        let _ = ActionRegistry::resolve(&reference, &engine, engine.views());
                    }
                    Err(ActionError::UnknownAction | ActionError::Malformed) => {}
                    Err(other) => panic!("issue produced an unexpected error: {other:?}"),
                }
            }
        }
    }
    assert!(malformed > 1000, "the generator must mostly be malformed");
    assert!(known > 0, "some generated labels are syntactically valid");

    // Malformed parameters are also typed, never panics.
    let primary = ActionKey::primary();
    for _ in 0..1000 {
        let param: String = (0..rng.below(400))
            .map(|_| rng.char_from(&alphabet))
            .collect();
        match ActionRegistry::issue(&engine, engine.views(), primary.clone(), &[&param]) {
            Ok(reference) => {
                assert_eq!(reference.params().len(), 1);
                assert!(ActionRegistry::resolve(&reference, &engine, engine.views()).is_ok());
            }
            Err(ActionError::Malformed) => {}
            Err(other) => panic!("param issue produced an unexpected error: {other:?}"),
        }
    }
}

fn expected(reference: &ActionRef, engine: &Engine) -> Result<(), ActionError> {
    let token = engine.query_token();
    if reference.invocation() != token.invocation() {
        return Err(ActionError::StaleInvocation);
    }
    if reference.view() != &engine.current_view() {
        return Err(ActionError::CrossView);
    }
    if reference.view_generation() != engine.view_generation() {
        return Err(ActionError::ExpiredView);
    }
    if reference.revision() != token.revision() {
        return Err(ActionError::StaleTarget);
    }
    let matches = match reference.target() {
        ActionTarget::Command(id) => engine.selected().map(|c| &c.id) == Some(id),
        ActionTarget::Calculator(expression) => engine.query() == expression,
    };
    if matches {
        Ok(())
    } else {
        Err(ActionError::StaleTarget)
    }
}

#[test]
fn fuzz_random_source_and_generation_combinations_are_typed() {
    let queries = ["", "alpha", "beta", "1 + 2", "zz-no-match", "=2*3"];
    let mut rng = Rng::new(0x1234_5678_9ABC_DEF0);
    let mut compared = 0usize;

    for _ in 0..2000 {
        let mut engine = engine();
        engine.set_query(queries[rng.below(queries.len())]);
        let _ = engine.select_index(rng.below(3));

        // Issue somewhere inside a possibly nested view stack.
        for _ in 0..rng.below(3) {
            engine
                .push_view(
                    ViewId::new(format!("ansa:frame{}", rng.below(4))).unwrap(),
                    ScrollAnchor::at(rng.below(5)),
                )
                .unwrap();
        }
        let reference = match issue_primary(&engine) {
            Ok(reference) => reference,
            Err(ActionError::Malformed) => continue,
            Err(other) => panic!("unexpected issue error: {other:?}"),
        };

        for _ in 0..rng.below(5) {
            match rng.below(8) {
                0 => {
                    engine.begin_invocation();
                }
                1 => {
                    engine.set_query(queries[rng.below(queries.len())]);
                }
                2 => engine.move_selection(if rng.below(2) == 0 { 1 } else { -1 }),
                3 => {
                    let _ = engine.push_view(
                        ViewId::new(format!("ansa:frame{}", rng.below(4))).unwrap(),
                        ScrollAnchor::at(rng.below(5)),
                    );
                }
                4 => {
                    engine.pop_view();
                }
                5 => engine.replace_catalog(vec![
                    command("app:alpha", "Alpha"),
                    command("app:beta", "Beta"),
                ]),
                6 => engine.open_actions(),
                _ => engine.close_actions(),
            }
        }

        let snapshot = (engine.query().to_owned(), engine.query_token());
        let result = ActionRegistry::resolve(&reference, &engine, engine.views());
        assert_eq!(
            result.as_ref().map(|_| ()).map_err(|error| *error),
            expected(&reference, &engine)
        );
        assert_eq!(
            (engine.query().to_owned(), engine.query_token()),
            snapshot,
            "resolve must be read-only"
        );
        compared += 1;
    }

    assert!(compared > 1500, "too few resolvable fixtures: {compared}");
}
