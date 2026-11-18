//! Invocation-scoped action references.
//!
//! An [`ActionRef`] is an opaque capability: only [`ActionRegistry::issue`] can
//! create one, and it captures the engine invocation, query revision, current
//! view frame, target command (or exact calculator expression), typed action
//! identity and bounded parameters at issue time.
//!
//! [`ActionRegistry::resolve`] revalidates all of those against the current
//! [`Engine`] and [`ViewStack`]. A delayed callback from an earlier view or an
//! earlier invocation is rejected with a typed [`ActionError`] instead of
//! running in the current context. The UI must resolve a reference before it
//! executes anything; it must not dispatch by action identity directly.
//!
//! Resolution precedence is deliberate and documented on [`ActionRegistry::resolve`].

use std::fmt;

use ansa_domain::CommandId;

use crate::action_panel::ActionId;
use crate::calculator::MAX_EXPRESSION_CHARS;
use crate::navigation::{ViewId, ViewStack};
use crate::query::{QueryRoute, route_query};
use crate::{Engine, QueryToken};

/// Maximum number of Unicode scalar values accepted in an action identity.
pub const MAX_ACTION_KEY_CHARS: usize = 64;

/// Maximum number of parameters carried by one action reference.
pub const MAX_ACTION_PARAMS: usize = 4;

/// Maximum number of Unicode scalar values accepted in one parameter.
pub const MAX_ACTION_PARAM_CHARS: usize = 256;

/// Why an action reference could not be issued or resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionError {
    /// The identity, parameters or engine target could not form a valid
    /// reference. Nothing was executed.
    Malformed,
    /// The action identity is not one this build knows.
    UnknownAction,
    /// The reference belongs to an earlier launcher invocation.
    StaleInvocation,
    /// The query revision or the target command/expression changed.
    StaleTarget,
    /// The reference was issued in a different view than the current one.
    CrossView,
    /// The view identity matches but its frame instance is gone (or the
    /// reference otherwise belongs to an expired frame).
    ExpiredView,
}

impl ActionError {
    /// Stable, user-facing status line text.
    pub fn status_message(self) -> &'static str {
        match self {
            Self::Malformed => "Malformed action — reopen actions",
            Self::UnknownAction => "Unknown action — reopen actions",
            Self::StaleInvocation => "Action invocation expired — reopen actions",
            Self::StaleTarget => "Action target changed — reopen actions",
            Self::CrossView => "Action belongs to another view — reopen actions",
            Self::ExpiredView => "Action view expired — reopen actions",
        }
    }
}

impl fmt::Display for ActionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.status_message())
    }
}

impl std::error::Error for ActionError {}

/// A validated, stable action identity.
///
/// UI code builds these from typed [`ActionId`] values. Strings exist only for
/// validation and fuzzing; they can never carry display text because the
/// accepted alphabet is ASCII label characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ActionKey(String);

impl ActionKey {
    /// Validates a stable action label.
    pub fn parse(raw: impl Into<String>) -> Result<Self, ActionError> {
        let raw = raw.into();
        if raw.is_empty() || raw.chars().count() > MAX_ACTION_KEY_CHARS {
            return Err(ActionError::Malformed);
        }
        if raw.chars().any(|character| !is_label_character(character)) {
            return Err(ActionError::Malformed);
        }
        Ok(Self(raw))
    }

    /// The panel "primary" action.
    pub fn primary() -> Self {
        Self("primary".to_owned())
    }

    /// The panel "copy target" action.
    pub fn copy_target() -> Self {
        Self("copy-target".to_owned())
    }

    /// The panel "toggle favorite" action.
    pub fn toggle_favorite() -> Self {
        Self("toggle-favorite".to_owned())
    }

    /// The stable label.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this build can execute this identity.
    pub fn is_known(&self) -> bool {
        self.panel_action().is_some()
    }

    /// The typed panel action, when this key names one.
    pub fn panel_action(&self) -> Option<ActionId> {
        match self.0.as_str() {
            "primary" => Some(ActionId::Primary),
            "copy-target" => Some(ActionId::CopyTarget),
            "toggle-favorite" => Some(ActionId::ToggleFavorite),
            _ => None,
        }
    }
}

impl From<ActionId> for ActionKey {
    fn from(id: ActionId) -> Self {
        match id {
            ActionId::Primary => Self::primary(),
            ActionId::CopyTarget => Self::copy_target(),
            ActionId::ToggleFavorite => Self::toggle_favorite(),
        }
    }
}

fn is_label_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
}

/// The command or exact expression an action applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionTarget {
    /// A stable command identity.
    Command(CommandId),
    /// An exact calculator expression, byte-for-byte as entered.
    Calculator(String),
}

impl ActionTarget {
    /// The command identity, for command targets.
    pub fn command(&self) -> Option<&CommandId> {
        match self {
            Self::Command(id) => Some(id),
            Self::Calculator(_) => None,
        }
    }

    /// The exact calculator expression, for calculator targets.
    pub fn expression(&self) -> Option<&str> {
        match self {
            Self::Command(_) => None,
            Self::Calculator(expression) => Some(expression),
        }
    }
}

/// An issued, immutable capability to run one action against one target.
///
/// Fields are private: the only constructor is [`ActionRegistry::issue`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionRef {
    invocation: u64,
    revision: u64,
    view: ViewId,
    view_generation: u64,
    target: ActionTarget,
    action: ActionKey,
    params: Vec<String>,
}

impl ActionRef {
    /// Launcher invocation captured at issue time.
    pub fn invocation(&self) -> u64 {
        self.invocation
    }

    /// Engine query revision captured at issue time.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// View in which the reference was issued.
    pub fn view(&self) -> &ViewId {
        &self.view
    }

    /// View-stack generation captured at issue time.
    pub fn view_generation(&self) -> u64 {
        self.view_generation
    }

    /// Command or expression captured at issue time.
    pub fn target(&self) -> &ActionTarget {
        &self.target
    }

    /// Stable action identity.
    pub fn action(&self) -> &ActionKey {
        &self.action
    }

    /// Bounded parameters.
    pub fn params(&self) -> &[String] {
        &self.params
    }

    /// The typed panel action this reference names, when any.
    pub fn panel_action(&self) -> Option<ActionId> {
        self.action.panel_action()
    }

    /// The query token captured at issue time.
    pub fn token(&self) -> QueryToken {
        QueryToken {
            invocation: self.invocation,
            revision: self.revision,
        }
    }
}

/// The validated action a reference resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAction {
    target: ActionTarget,
    action: ActionKey,
    params: Vec<String>,
}

impl ResolvedAction {
    /// Command or expression this action applies to.
    pub fn target(&self) -> &ActionTarget {
        &self.target
    }

    /// Stable action identity.
    pub fn action(&self) -> &ActionKey {
        &self.action
    }

    /// Bounded parameters captured at issue time.
    pub fn params(&self) -> &[String] {
        &self.params
    }

    /// The typed panel action, when this action names one.
    pub fn panel_action(&self) -> Option<ActionId> {
        self.action.panel_action()
    }
}

/// Issues and validates invocation-scoped action references.
#[derive(Debug, Clone, Copy, Default)]
pub struct ActionRegistry;

impl ActionRegistry {
    /// Issues a reference bound to the engine's current invocation, revision
    /// and view frame.
    ///
    /// The target is derived from the engine: the selected command for a search
    /// query, or the exact expression for a calculator query. Unknown action
    /// identities and malformed or overlong parameters are rejected without
    /// panicking.
    pub fn issue(
        engine: &Engine,
        views: &ViewStack,
        action: ActionKey,
        params: &[&str],
    ) -> Result<ActionRef, ActionError> {
        if !action.is_known() {
            return Err(ActionError::UnknownAction);
        }
        if params.len() > MAX_ACTION_PARAMS {
            return Err(ActionError::Malformed);
        }
        let mut bounded = Vec::with_capacity(params.len());
        for param in params {
            if param.chars().count() > MAX_ACTION_PARAM_CHARS || param.chars().any(char::is_control)
            {
                return Err(ActionError::Malformed);
            }
            bounded.push((*param).to_owned());
        }
        let token = engine.query_token();
        let target = target_from_engine(engine)?;
        Ok(ActionRef {
            invocation: token.invocation(),
            revision: token.revision(),
            view: views.current(),
            view_generation: views.generation(),
            target,
            action,
            params: bounded,
        })
    }

    /// Issues a reference for a typed panel action.
    pub fn issue_panel(
        engine: &Engine,
        views: &ViewStack,
        action: ActionId,
    ) -> Result<ActionRef, ActionError> {
        Self::issue(engine, views, ActionKey::from(action), &[])
    }

    /// Revalidates `reference` against the current engine and view stack.
    ///
    /// Precedence is fixed so a caller always gets the most specific typed
    /// error:
    ///
    /// 1. [`ActionError::UnknownAction`] for an identity this build does not know.
    /// 2. [`ActionError::StaleInvocation`] when the launcher invocation moved.
    /// 3. [`ActionError::CrossView`] when the current view identity differs.
    /// 4. [`ActionError::ExpiredView`] when the view identity matches but the
    ///    frame generation moved (the old frame instance is gone).
    /// 5. [`ActionError::StaleTarget`] when the query revision moved or the
    ///    target command/expression no longer matches.
    ///
    /// Only an [`Ok`] result may execute. A stale reference never falls back to
    /// "the command with the same identity" in a different view.
    pub fn resolve(
        reference: &ActionRef,
        engine: &Engine,
        views: &ViewStack,
    ) -> Result<ResolvedAction, ActionError> {
        if !reference.action.is_known() {
            return Err(ActionError::UnknownAction);
        }
        let token = engine.query_token();
        if reference.invocation != token.invocation() {
            return Err(ActionError::StaleInvocation);
        }
        let current_view = views.current();
        if reference.view != current_view {
            return Err(ActionError::CrossView);
        }
        if reference.view_generation != views.generation() {
            return Err(ActionError::ExpiredView);
        }
        if reference.revision != token.revision() {
            return Err(ActionError::StaleTarget);
        }
        let target_matches = match &reference.target {
            ActionTarget::Command(id) => engine.selected().map(|command| &command.id) == Some(id),
            ActionTarget::Calculator(expression) => engine.query() == expression,
        };
        if !target_matches {
            return Err(ActionError::StaleTarget);
        }
        Ok(ResolvedAction {
            target: reference.target.clone(),
            action: reference.action.clone(),
            params: reference.params.clone(),
        })
    }
}

fn target_from_engine(engine: &Engine) -> Result<ActionTarget, ActionError> {
    match route_query(engine.query()) {
        QueryRoute::Calculator(_) => {
            let expression = engine.query();
            if expression.chars().count() > MAX_EXPRESSION_CHARS + 1 {
                return Err(ActionError::Malformed);
            }
            Ok(ActionTarget::Calculator(expression.to_owned()))
        }
        QueryRoute::Search => engine
            .selected()
            .map(|command| ActionTarget::Command(command.id.clone()))
            .ok_or(ActionError::Malformed),
    }
}
