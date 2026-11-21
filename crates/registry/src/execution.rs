//! Opaque action references, effects, outcomes and the host contract.

use std::fmt;

use ansa_domain::CommandId;

use crate::command::{CommandKind, CommandSpec, ParameterValues};

/// An opaque handle to one prepared action.
///
/// An `ActionRef` is a capability, not authentication. It is created only by
/// [`crate::Registry::prepare`], is scoped to that registry instance, and is
/// re-checked against the stored command, parameters, target revision and
/// grants on every execution. Display text is never part of this identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActionRef {
    registry: u64,
    sequence: u64,
}

impl ActionRef {
    pub(crate) fn new(registry: u64, sequence: u64) -> Self {
        Self { registry, sequence }
    }

    pub(crate) fn registry(self) -> u64 {
        self.registry
    }

    pub(crate) fn sequence(self) -> u64 {
        self.sequence
    }
}

/// Stable identity of one host operation returned by execute.
///
/// Operation identities are scoped to the issuing registry instance. They are
/// safe to persist in memory for reconciliation but are not authentication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OperationId {
    registry: u64,
    sequence: u64,
}

impl OperationId {
    pub(crate) fn new(registry: u64, sequence: u64) -> Self {
        Self { registry, sequence }
    }

    pub(crate) fn registry(self) -> u64 {
        self.registry
    }
}

impl fmt::Display for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "operation-{:x}-{:x}", self.registry, self.sequence)
    }
}

/// The host-resolved target and its revision at the time it was observed.
///
/// Revisions are host-defined monotonic counters. The registry compares the
/// revision observed at prepare with a fresh probe immediately before execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    identity: String,
    revision: u64,
}

impl ResolvedTarget {
    pub(crate) fn new(identity: impl Into<String>, revision: u64) -> Self {
        Self {
            identity: identity.into(),
            revision,
        }
    }

    /// Stable host target identity, such as an absolute path, URL or script id.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// Host-defined revision observed with this identity.
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// A user-visible target description attached to an effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectTarget {
    identity: String,
    kind: &'static str,
}

impl EffectTarget {
    pub(crate) fn new(identity: impl Into<String>, kind: &'static str) -> Self {
        Self {
            identity: identity.into(),
            kind,
        }
    }

    /// Stable host target identity.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// Command kind label, for display only.
    pub fn kind(&self) -> &str {
        self.kind
    }
}

/// A pre-execution summary of what an action would do and to which target.
///
/// The summary is produced by [`crate::Registry::prepare`] before any
/// authorization or host effect, so a UI can display it to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effect {
    target: EffectTarget,
    summary: String,
    consequential: bool,
}

impl Effect {
    pub(crate) fn new(target: EffectTarget, summary: String, consequential: bool) -> Self {
        Self {
            target,
            summary,
            consequential,
        }
    }

    /// The resolved target.
    pub fn target(&self) -> &EffectTarget {
        &self.target
    }

    /// A short user-facing description of the effect.
    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// Whether the effect requires explicit confirmation.
    pub fn is_consequential(&self) -> bool {
        self.consequential
    }
}

/// A prepared action, including its opaque reference and effect summary.
///
/// Each preparation is a distinct intent. Two preparations of the same command
/// before any execution are not deduplicated; an unresolved uncertain
/// operation, however, blocks new preparations for its command entirely.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedAction {
    action: ActionRef,
    command: CommandId,
    effect: Effect,
    parameters: ParameterValues,
    target_revision: u64,
    requires_confirmation: bool,
}

impl PreparedAction {
    pub(crate) fn new(
        action: ActionRef,
        command: CommandId,
        effect: Effect,
        parameters: ParameterValues,
        target_revision: u64,
        requires_confirmation: bool,
    ) -> Self {
        Self {
            action,
            command,
            effect,
            parameters,
            target_revision,
            requires_confirmation,
        }
    }

    /// The opaque reference to pass to authorization and execution.
    pub fn action_ref(&self) -> ActionRef {
        self.action
    }

    /// The command this action was prepared for.
    pub fn command_id(&self) -> &CommandId {
        &self.command
    }

    /// The effect summary, including the resolved target.
    pub fn effect(&self) -> &Effect {
        &self.effect
    }

    /// The typed parameter values, with defaults applied.
    pub fn parameters(&self) -> &ParameterValues {
        &self.parameters
    }

    /// The target revision observed at preparation time.
    pub fn target_revision(&self) -> u64 {
        self.target_revision
    }

    /// Whether execution requires explicit confirmation.
    pub fn requires_confirmation(&self) -> bool {
        self.requires_confirmation
    }
}

/// Proof that a prepared action passed authorization at one point in time.
///
/// This token is not a bearer credential and is not authentication. Execution
/// re-checks the binding, target revision, current grants and the grant
/// snapshot revision before performing any effect. A successful `authorize`
/// only records that these checks passed for the supplied snapshot; it does not
/// mean the registry owns or can refresh permission state.
#[derive(Debug)]
pub struct Authorization {
    action: ActionRef,
    command: CommandId,
    confirmed: bool,
    grant_revision: u64,
}

impl Authorization {
    pub(crate) fn new(
        action: ActionRef,
        command: CommandId,
        confirmed: bool,
        grant_revision: u64,
    ) -> Self {
        Self {
            action,
            command,
            confirmed,
            grant_revision,
        }
    }

    /// The action this authorization covers.
    pub fn action_ref(&self) -> ActionRef {
        self.action
    }

    /// The command this authorization covers.
    pub fn command_id(&self) -> &CommandId {
        &self.command
    }

    /// Whether explicit confirmation was supplied.
    pub fn confirmed(&self) -> bool {
        self.confirmed
    }

    /// The grant-snapshot revision this authorization was bound to.
    pub fn grant_revision(&self) -> u64 {
        self.grant_revision
    }
}

/// The typed status of an executed operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionStatus {
    /// The host reported a definite completed effect.
    Completed,
    /// The host could not determine whether the effect occurred.
    Uncertain {
        /// Host explanation. The caller must reconcile instead of retrying.
        reason: String,
    },
}

/// The result of a guarded execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionOutcome {
    operation: OperationId,
    status: ExecutionStatus,
}

impl ExecutionOutcome {
    pub(crate) fn completed(operation: OperationId) -> Self {
        Self {
            operation,
            status: ExecutionStatus::Completed,
        }
    }

    pub(crate) fn uncertain(operation: OperationId, reason: impl Into<String>) -> Self {
        Self {
            operation,
            status: ExecutionStatus::Uncertain {
                reason: reason.into(),
            },
        }
    }

    /// The operation identity to use with reconciliation.
    pub fn operation_id(&self) -> OperationId {
        self.operation
    }

    /// The typed status.
    pub fn status(&self) -> &ExecutionStatus {
        &self.status
    }

    /// Whether the host reported a definite completed effect.
    pub fn is_completed(&self) -> bool {
        matches!(self.status, ExecutionStatus::Completed)
    }

    /// Whether the host could not determine the outcome.
    pub fn is_uncertain(&self) -> bool {
        matches!(self.status, ExecutionStatus::Uncertain { .. })
    }
}

/// The result of reconciling an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileOutcome {
    /// The host confirmed the operation completed.
    Completed,
    /// The host reports the operation failed before a definite effect.
    Failed,
    /// The host still cannot determine the operation's fate.
    StillPending,
    /// The registry does not know this operation, or the host no longer does.
    Unknown,
}

/// A host's answer to target probing.
///
/// This is data returned by the host; the registry never treats an absent or
/// malformed probe as success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetProbe {
    /// The target exists at `identity` and `revision`.
    Present {
        /// Stable target identity, which must not be blank.
        identity: String,
        /// Host-defined monotonic revision.
        revision: u64,
    },
    /// The target no longer exists.
    Missing,
    /// The host cannot support the capability this command needs.
    Unsupported {
        /// Capability the host cannot provide.
        capability: String,
    },
}

/// A host's report after performing an operation.
///
/// The host must not retry internally. When it cannot determine whether an
/// effect occurred it must return [`HostReport::Uncertain`] so the registry can
/// surface the operation for reconciliation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostReport {
    /// The effect definitely completed.
    Completed,
    /// The effect may have happened; the outcome is unknown.
    Uncertain {
        /// Why the outcome is unknown.
        reason: String,
    },
    /// A permission was refused at execution time.
    Denied {
        /// Capability that was refused. Empty means "the command's first
        /// required capability or the host itself".
        capability: String,
        /// Explanation.
        reason: String,
    },
    /// The host cannot perform this kind.
    Unsupported {
        /// Capability the host cannot provide.
        capability: String,
    },
    /// The target disappeared during execution.
    Missing {
        /// Target identity, or empty to use the prepared target.
        target: String,
    },
    /// The target changed or another operation conflicted.
    Conflict {
        /// What changed.
        reason: String,
    },
    /// The operation was cancelled before a definite effect.
    Cancelled {
        /// Why it was cancelled.
        reason: String,
    },
}

/// A host's answer when reconciling a previous operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostReconcile {
    /// The effect is confirmed complete.
    Completed,
    /// The operation failed before a definite effect.
    Failed,
    /// The host still cannot determine the outcome.
    StillPending,
    /// The host has no record of the operation.
    Unknown,
}

/// A typed execution request handed to [`Host::perform`].
///
/// The request contains only the bound command identity, its inert kind, typed
/// parameter values and the resolved target. It never contains a shell string,
/// a display name used as identity, or a caller-provided callback.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionRequest {
    command: CommandId,
    kind: CommandKind,
    parameters: ParameterValues,
    target: ResolvedTarget,
}

impl ExecutionRequest {
    pub(crate) fn new(
        command: CommandId,
        kind: CommandKind,
        parameters: ParameterValues,
        target: ResolvedTarget,
    ) -> Self {
        Self {
            command,
            kind,
            parameters,
            target,
        }
    }

    /// The bound command identity.
    pub fn command_id(&self) -> &CommandId {
        &self.command
    }

    /// The registered command kind, as data.
    pub fn kind(&self) -> &CommandKind {
        &self.kind
    }

    /// Validated, typed parameter values.
    pub fn parameters(&self) -> &ParameterValues {
        &self.parameters
    }

    /// The re-probed target that still matches the prepared revision.
    pub fn target(&self) -> &ResolvedTarget {
        &self.target
    }
}

/// A typed request handed to [`Host::reconcile`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileRequest {
    operation: OperationId,
    command: CommandId,
    target: ResolvedTarget,
}

impl ReconcileRequest {
    pub(crate) fn new(operation: OperationId, command: CommandId, target: ResolvedTarget) -> Self {
        Self {
            operation,
            command,
            target,
        }
    }

    /// The operation to reconcile.
    pub fn operation_id(&self) -> OperationId {
        self.operation
    }

    /// The command that produced the operation.
    pub fn command_id(&self) -> &CommandId {
        &self.command
    }

    /// The target observed at preparation time.
    pub fn target(&self) -> &ResolvedTarget {
        &self.target
    }
}

/// Host-side target probing and effect performance.
///
/// Implementations may use platform APIs. The contract is deliberately narrow:
///
/// * [`Host::probe_target`] resolves the registered kind to a stable identity
///   and revision, or reports missing/unsupported.
/// * [`Host::perform`] performs exactly one operation and reports its outcome.
///   It must not retry, and must report uncertain effects as
///   [`HostReport::Uncertain`].
/// * [`Host::reconcile`] checks observable effects for a previous operation.
///   It reports [`HostReconcile::Completed`] or [`HostReconcile::Failed`] only
///   when the fate is definite; otherwise it reports still-pending or unknown.
///
/// The host never receives an [`ActionRef`], an [`Authorization`] or display
/// text as authority; it receives only inert, typed data.
pub trait Host {
    /// Probes the current target for `spec`.
    fn probe_target(&self, spec: &CommandSpec) -> TargetProbe;

    /// Performs exactly one operation described by `request`.
    fn perform(&mut self, request: ExecutionRequest) -> HostReport;

    /// Reconciles a previously issued operation.
    fn reconcile(&mut self, request: ReconcileRequest) -> HostReconcile;
}
