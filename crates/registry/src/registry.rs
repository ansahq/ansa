//! The command registry and its prepare/authorize/execute/reconcile lifecycle.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};

use ansa_domain::CommandId;

use crate::command::{
    ActionInput, CommandKind, CommandSpec, MAX_COMMAND_ID_CHARS, MAX_KEYWORD_CHARS, MAX_KEYWORDS,
    MAX_TITLE_CHARS, ParameterValues, first_display_format_character,
    first_invisible_identity_character,
};
use crate::error::{ActionError, RegistrationError};
use crate::execution::{
    ActionRef, Authorization, Effect, EffectTarget, ExecutionOutcome, ExecutionRequest, Host,
    HostReconcile, HostReport, OperationId, PreparedAction, ReconcileOutcome, ReconcileRequest,
    ResolvedTarget, TargetProbe,
};
use crate::grant::{CapabilityCatalog, Confirmation, GrantSnapshot};

static NEXT_REGISTRY: AtomicU64 = AtomicU64::new(1);

/// Maximum number of prepared actions retained by default.
pub const DEFAULT_MAX_PREPARED_ACTIONS: usize = 1024;

/// Maximum number of terminal operation tombstones retained by default.
pub const DEFAULT_MAX_TERMINAL_OPERATIONS: usize = 4096;

/// Retention and capacity limits for a [`Registry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistryLimits {
    /// Maximum number of prepared actions that have not been discarded or
    /// pruned. `prepare` fails closed when this limit is reached.
    pub max_prepared_actions: usize,
    /// Maximum number of terminal operation tombstones (`Completed`, `Failed`
    /// or `Unknown`) retained. The oldest are pruned automatically when this
    /// limit is exceeded. Uncertain/still-pending operations are never pruned.
    pub max_terminal_operations: usize,
}

impl Default for RegistryLimits {
    fn default() -> Self {
        Self {
            max_prepared_actions: DEFAULT_MAX_PREPARED_ACTIONS,
            max_terminal_operations: DEFAULT_MAX_TERMINAL_OPERATIONS,
        }
    }
}

/// The process-wide registry instance identifier space is exhausted.
///
/// This is unreachable under any realistic workload, but construction fails
/// closed rather than reusing an identity that could resolve references from a
/// previous registry instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistryExhausted;

impl std::fmt::Display for RegistryExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("registry instance identifier space is exhausted")
    }
}

impl std::error::Error for RegistryExhausted {}

#[derive(Debug, Clone)]
struct PreparedRecord {
    action: ActionRef,
    command: CommandId,
    parameters: ParameterValues,
    target: ResolvedTarget,
    requires_confirmation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationStatus {
    Completed,
    Failed,
    Unknown,
    Uncertain,
}

#[derive(Debug, Clone)]
struct OperationRecord {
    action: ActionRef,
    command: CommandId,
    target: ResolvedTarget,
    status: OperationStatus,
}

/// The single command and action registry.
///
/// A registry owns inert [`CommandSpec`] values, the prepared actions it has
/// issued, and the operations those actions produced. It never owns a callback,
/// never runs shell strings, and never retries automatically.
///
/// Instances are independent: an [`ActionRef`] or [`OperationId`] produced by
/// one registry is rejected (or reported unknown) by another, even if the
/// sequence numbers coincide.
///
/// # Uncertainty and retention
///
/// Pending (`Uncertain`/still-pending) operations are tracked as a set per
/// command. Any pending operation blocks *new* preparations of its command, not
/// only replay of the exact [`ActionRef`]. [`Registry::reconcile`] removes only
/// the resolved operation from that set: the command stays blocked while any
/// other operation remains pending. If a host reports `Unknown` and later
/// `StillPending` for the same operation, it is re-added to the set.
///
/// When a command is blocked, fresh `prepare`/`authorize`/`execute` report the
/// oldest pending operation (by operation sequence) in
/// [`ActionError::Uncertain`] and include the pending count in the reason. Exact
/// action references still report the operation bound to that reference.
///
/// Preparing the same command twice before any execution creates two distinct
/// intents. There is no implicit idempotency key and no deduplication: callers
/// that need one should perform a single prepare per user intent. Prepared and
/// completed state is bounded; see [`Registry::discard_prepared`],
/// [`Registry::prune_completed_operations`] and [`RegistryLimits`]. In
/// particular, [`RegistryLimits::max_terminal_operations`] automatically evicts
/// the oldest `Completed`/`Failed`/`Unknown` tombstones; `Uncertain` and
/// still-pending operations are never evicted.
///
/// # Catalog size
///
/// The command catalog is deliberately a host-controlled boundary: `specs` is
/// not capacity-limited by this crate because first-party catalogs are expected
/// to be bounded by the host's discovery/registration policy. A host that
/// accepts third-party registrations must cap provider/catalog count and rebuild
/// the registry when that policy changes. Command identities and untrusted
/// provider namespaces are individually bounded by [`MAX_COMMAND_ID_CHARS`].
#[derive(Debug)]
pub struct Registry {
    instance: u64,
    specs: BTreeMap<CommandId, CommandSpec>,
    capabilities: CapabilityCatalog,
    limits: RegistryLimits,
    next_sequence: u64,
    prepared: BTreeMap<u64, PreparedRecord>,
    operations: BTreeMap<OperationId, OperationRecord>,
    action_operations: BTreeMap<ActionRef, OperationId>,
    pending_operations: BTreeMap<CommandId, BTreeSet<OperationId>>,
}

impl Registry {
    /// Creates an empty registry with the built-in capability catalog and
    /// default limits.
    ///
    /// # Panics
    ///
    /// Panics if the process-wide registry instance identifier space is
    /// exhausted. Use [`Registry::try_new`] to handle that condition explicitly.
    pub fn new() -> Self {
        Self::with_capabilities(CapabilityCatalog::builtin())
    }

    /// Creates an empty registry with an explicit capability catalog and
    /// default limits.
    ///
    /// # Panics
    ///
    /// Panics if the registry instance identifier space is exhausted.
    pub fn with_capabilities(capabilities: CapabilityCatalog) -> Self {
        Self::with_limits(capabilities, RegistryLimits::default())
    }

    /// Creates an empty registry with explicit capabilities and limits.
    ///
    /// # Panics
    ///
    /// Panics if the registry instance identifier space is exhausted.
    pub fn with_limits(capabilities: CapabilityCatalog, limits: RegistryLimits) -> Self {
        Self::try_with_limits(capabilities, limits)
            .expect("registry instance identifier space is exhausted")
    }

    /// Fallible variant of [`Registry::with_limits`].
    pub fn try_with_limits(
        capabilities: CapabilityCatalog,
        limits: RegistryLimits,
    ) -> Result<Self, RegistryExhausted> {
        let instance = NEXT_REGISTRY
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| RegistryExhausted)?;
        Ok(Self {
            instance,
            specs: BTreeMap::new(),
            capabilities,
            limits,
            next_sequence: 0,
            prepared: BTreeMap::new(),
            operations: BTreeMap::new(),
            action_operations: BTreeMap::new(),
            pending_operations: BTreeMap::new(),
        })
    }

    /// Fallible variant of [`Registry::new`].
    pub fn try_new() -> Result<Self, RegistryExhausted> {
        Self::try_with_limits(CapabilityCatalog::builtin(), RegistryLimits::default())
    }

    /// Fallible variant of [`Registry::with_capabilities`].
    pub fn try_with_capabilities(
        capabilities: CapabilityCatalog,
    ) -> Result<Self, RegistryExhausted> {
        Self::try_with_limits(capabilities, RegistryLimits::default())
    }

    /// The configured retention and capacity limits.
    pub fn limits(&self) -> RegistryLimits {
        self.limits
    }

    /// Registers a validated command descriptor atomically.
    ///
    /// This is the trusted first-party entry point. All checks run before the
    /// spec is inserted. A failed registration leaves the registry exactly as
    /// it was. On success the minimum capability implied by
    /// [`CommandSpec::kind`] is unioned into
    /// [`CommandSpec::required_capabilities`].
    ///
    /// Identities are byte-exact: case variants and canonically equivalent
    /// Unicode remain distinct. Registration rejects invisible zero-width and
    /// bidi format characters that would make an identity visually ambiguous.
    ///
    /// # Errors
    ///
    /// Rejects duplicate identities, malformed identities, empty or
    /// control-bearing titles or keywords, malformed kind targets, duplicate
    /// or invalid parameter declarations, invalid defaults, and empty,
    /// duplicate or unknown capability requirements. Every error names the
    /// offending command; field-specific errors also name the field.
    pub fn register(&mut self, mut spec: CommandSpec) -> Result<(), RegistrationError> {
        if self.specs.contains_key(spec.id()) {
            return Err(RegistrationError::DuplicateCommand {
                id: spec.id().clone(),
            });
        }
        self.validate_spec(&spec)?;
        spec.merge_kind_capability();
        self.specs.insert(spec.id().clone(), spec);
        Ok(())
    }

    /// Registers a command from an untrusted provider.
    ///
    /// Enforces the *namespace convention*:
    ///
    /// * the spec identity must live in `provider_namespace`;
    /// * the namespace must not be one of the reserved first-party namespaces
    ///   (`builtin`, `app`, `quicklink`, `script`, `extension`), compared
    ///   case-insensitively;
    /// * [`CommandKind::Builtin`] is forbidden to untrusted providers.
    ///
    /// Vendor commands of other kinds are accepted, with the normal kind
    /// capability and confirmation policy applied by [`Registry::register`].
    ///
    /// # This is not provider authentication
    ///
    /// `provider_namespace` is a caller-supplied string, not a minted provider
    /// identity. This method catches accidental or naive impersonation in code
    /// that forwards the provider's own namespace; it does **not** defend
    /// against a compromised in-process caller, which can call
    /// [`Registry::register`] or [`Registry::register_raw`] directly or pass a
    /// victim's namespace. It also does not validate target ownership: a
    /// vendor-scoped extension or script command may name another extension,
    /// script or application path. The host must authenticate providers, keep
    /// trusted registration functions out of provider reach, and validate
    /// `extension_id`/`script_id`/path ownership before granting capabilities.
    ///
    /// # Errors
    ///
    /// Returns [`RegistrationError::ProviderNamespaceTooLong`],
    /// [`RegistrationError::ReservedNamespace`],
    /// [`RegistrationError::NamespaceMismatch`] or
    /// [`RegistrationError::ForbiddenKind`], then any error from
    /// [`Registry::register`].
    pub fn register_untrusted(
        &mut self,
        provider_namespace: &str,
        spec: CommandSpec,
    ) -> Result<(), RegistrationError> {
        if provider_namespace.chars().count() > MAX_COMMAND_ID_CHARS {
            return Err(RegistrationError::ProviderNamespaceTooLong {
                namespace: provider_namespace.to_owned(),
                limit: MAX_COMMAND_ID_CHARS,
            });
        }
        if is_reserved_namespace(provider_namespace) {
            return Err(RegistrationError::ReservedNamespace {
                id: spec.id().clone(),
                namespace: provider_namespace.to_owned(),
            });
        }
        if spec.id().namespace() != provider_namespace {
            return Err(RegistrationError::NamespaceMismatch {
                id: spec.id().clone(),
                expected: provider_namespace.to_owned(),
                actual: spec.id().namespace().to_owned(),
            });
        }
        if matches!(spec.kind(), CommandKind::Builtin) {
            return Err(RegistrationError::ForbiddenKind {
                id: spec.id().clone(),
                reason: "Builtin commands are reserved for trusted first-party registration".into(),
            });
        }
        self.register(spec)
    }

    /// Registers a command from a raw identifier and kind.
    ///
    /// This is a **trusted first-party** convenience for call sites that hold a
    /// string identity: it parses a namespaced [`CommandId`] and delegates to
    /// [`Registry::register`]. It performs no namespace provenance checks and
    /// must not be exposed to providers or untrusted input; use
    /// [`Registry::register_untrusted`] for provider commands. Because it is
    /// public, in-process code can bypass untrusted provenance by design.
    pub fn register_raw(
        &mut self,
        raw_id: &str,
        title: impl Into<String>,
        kind: CommandKind,
    ) -> Result<(), RegistrationError> {
        let id =
            CommandId::parse(raw_id).map_err(|reason| RegistrationError::InvalidCommandId {
                raw: raw_id.to_owned(),
                reason,
            })?;
        self.register(CommandSpec::new(id, title, kind))
    }

    /// Looks up a registered command by stable identity.
    pub fn get(&self, id: &CommandId) -> Option<&CommandSpec> {
        self.specs.get(id)
    }

    /// Iterates registered commands in stable identity order.
    pub fn specs(&self) -> impl Iterator<Item = &CommandSpec> {
        self.specs.values()
    }

    /// Number of registered commands.
    pub fn len(&self) -> usize {
        self.specs.len()
    }

    /// Whether no commands are registered.
    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// Validates raw input and resolves the concrete target.
    ///
    /// Preparation performs no host effect. It produces an opaque
    /// [`ActionRef`] and an [`Effect`] summary that a UI can show before
    /// authorization. Required confirmation is reported through
    /// [`PreparedAction::requires_confirmation`].
    ///
    /// An unresolved uncertain operation for this command blocks preparation
    /// until [`Registry::reconcile`] resolves it. If several operations are
    /// pending, the oldest is reported with the pending count. Preparing the same command
    /// twice before any execution is allowed and creates two distinct intents;
    /// there is no implicit idempotency key.
    ///
    /// # Errors
    ///
    /// Returns [`ActionError::InvalidInput`] for malformed input,
    /// [`ActionError::MissingTarget`] when the command or target does not
    /// exist, [`ActionError::UnsupportedCapability`] when the host cannot
    /// perform the kind, [`ActionError::Uncertain`] while the command has an
    /// unresolved operation, and [`ActionError::ResourceExhausted`] when the
    /// prepared-action limit or the sequence space is exhausted.
    pub fn prepare<H: Host + ?Sized>(
        &mut self,
        command: &CommandId,
        input: &ActionInput,
        host: &H,
    ) -> Result<PreparedAction, ActionError> {
        if let Some(error) = self.pending_block_error(command, "preparing") {
            return Err(error);
        }
        let Some(spec) = self.specs.get(command) else {
            return Err(ActionError::MissingTarget {
                command: command.clone(),
                target: command.to_string(),
            });
        };

        let parameters = spec.parameters().bind(command, input)?;
        if self.prepared.len() >= self.limits.max_prepared_actions {
            return Err(ActionError::ResourceExhausted {
                command: command.clone(),
                resource: "prepared actions".into(),
                limit: self.limits.max_prepared_actions as u64,
            });
        }
        let probe = host.probe_target(spec);
        let target = match probe {
            TargetProbe::Present { identity, revision } => {
                if identity.trim().is_empty() {
                    return Err(ActionError::Conflict {
                        command: command.clone(),
                        reason: "host returned an empty target identity".into(),
                    });
                }
                ResolvedTarget::new(identity, revision)
            }
            TargetProbe::Missing => {
                return Err(ActionError::MissingTarget {
                    command: command.clone(),
                    target: spec
                        .kind()
                        .target_identity()
                        .unwrap_or_else(|| command.to_string()),
                });
            }
            TargetProbe::Unsupported { capability } => {
                return Err(ActionError::UnsupportedCapability {
                    command: command.clone(),
                    capability: normalized_capability(&capability),
                });
            }
        };

        let effect = Effect::new(
            EffectTarget::new(target.identity().to_owned(), spec.kind().label()),
            spec.kind().effect_summary(spec.title(), target.identity()),
            spec.requires_confirmation(),
        );
        let requires_confirmation = spec.requires_confirmation();
        let target_revision = target.revision();

        let sequence = self.allocate_sequence(command)?;
        let action = ActionRef::new(self.instance, sequence);
        self.prepared.insert(
            sequence,
            PreparedRecord {
                action,
                command: command.clone(),
                parameters: parameters.clone(),
                target,
                requires_confirmation,
            },
        );

        Ok(PreparedAction::new(
            action,
            command.clone(),
            effect,
            parameters,
            target_revision,
            requires_confirmation,
        ))
    }

    /// Checks the grant snapshot and explicit confirmation for a prepared
    /// action.
    ///
    /// Authorization also fails before grant checks when the command has an
    /// unresolved uncertain operation(s), or when this exact action already
    /// produced an operation.
    ///
    /// The authorization is bound to [`GrantSnapshot::revision`]. Execution
    /// rejects a snapshot with a different revision, which catches accidentally
    /// stale grants. The registry is not a permission store: the snapshot must
    /// come from trusted host state.
    ///
    /// # Errors
    ///
    /// Returns [`ActionError::UnknownReference`] for a reference this registry
    /// does not know, [`ActionError::Conflict`] for a cross-command reference or
    /// an action that already executed, [`ActionError::Uncertain`] while the
    /// command or action has an unresolved outcome, [`ActionError::Cancelled`]
    /// when a consequential action was not explicitly confirmed, and
    /// [`ActionError::DeniedPermission`] when a required capability is absent.
    pub fn authorize(
        &self,
        prepared: &PreparedAction,
        grants: &GrantSnapshot,
        confirmation: Confirmation,
    ) -> Result<Authorization, ActionError> {
        let action = prepared.action_ref();
        if action.registry() != self.instance {
            return Err(unknown_reference(prepared.command_id(), action));
        }
        // Attribution first: if this exact action produced an operation, report
        // that operation rather than a newer command-level uncertainty.
        if let Some(operation) = self.action_operations.get(&action).copied() {
            if let Some(operation_record) = self.operations.get(&operation)
                && operation_record.command != *prepared.command_id()
            {
                return Err(ActionError::Conflict {
                    command: prepared.command_id().clone(),
                    reason: format!(
                        "action reference was prepared for '{}', not '{}'",
                        operation_record.command,
                        prepared.command_id()
                    ),
                });
            }
            return Err(self.replay_error(prepared.command_id(), operation));
        }

        let Some(record) = self.prepared.get(&action.sequence()) else {
            return Err(unknown_reference(prepared.command_id(), action));
        };
        if record.action != action {
            return Err(ActionError::Conflict {
                command: prepared.command_id().clone(),
                reason: "action reference no longer matches the prepared action".into(),
            });
        }
        if record.command != *prepared.command_id() {
            return Err(ActionError::Conflict {
                command: prepared.command_id().clone(),
                reason: format!(
                    "action reference was prepared for '{}', not '{}'",
                    record.command,
                    prepared.command_id()
                ),
            });
        }

        if let Some(error) = self.pending_block_error(&record.command, "authorizing") {
            return Err(error);
        }

        if record.requires_confirmation && confirmation != Confirmation::Confirmed {
            return Err(ActionError::Cancelled {
                command: record.command.clone(),
                reason: "explicit confirmation is required for this action".into(),
            });
        }

        let Some(spec) = self.specs.get(&record.command) else {
            return Err(ActionError::MissingTarget {
                command: record.command.clone(),
                target: record.command.to_string(),
            });
        };
        for capability in spec.required_capabilities() {
            if !grants.contains(capability) {
                return Err(ActionError::DeniedPermission {
                    command: record.command.clone(),
                    capability: capability.clone(),
                    reason: "capability is not granted".into(),
                });
            }
        }

        Ok(Authorization::new(
            record.action,
            record.command.clone(),
            confirmation == Confirmation::Confirmed,
            grants.revision(),
        ))
    }

    /// Re-checks every guard and performs at most one host operation.
    ///
    /// The guards are deliberately redundant with authorization:
    ///
    /// 1. the reference must belong to this registry and this exact command;
    /// 2. the command must not have any unresolved uncertain operation;
    /// 3. the exact action must not already have produced an operation;
    /// 4. a consequential action must still be confirmed;
    /// 5. the grant snapshot revision must match authorization;
    /// 6. every required capability must still be granted;
    /// 7. the target must still exist at the prepared identity and revision.
    ///
    /// Replay, uncertainty and binding checks run before host probing or grant
    /// checks, so a replay cannot be misreported as a target or permission
    /// failure. The host is asked to perform exactly once and is never retried
    /// here.
    ///
    /// # Errors
    ///
    /// Returns [`ActionError::UnknownReference`] for a reference this registry
    /// does not know, [`ActionError::Conflict`] for cross-command, stale,
    /// replayed or grant-snapshot changes, [`ActionError::Uncertain`] while an
    /// operation is unresolved, [`ActionError::Cancelled`] for missing
    /// confirmation, [`ActionError::DeniedPermission`] for absent grants and
    /// [`ActionError::ResourceExhausted`] if the operation sequence space is
    /// exhausted before any host effect.
    pub fn execute<H: Host + ?Sized>(
        &mut self,
        command: &CommandId,
        authorization: &Authorization,
        grants: &GrantSnapshot,
        host: &mut H,
    ) -> Result<ExecutionOutcome, ActionError> {
        let action = authorization.action_ref();
        if action.registry() != self.instance {
            return Err(unknown_reference(command, action));
        }
        if authorization.command_id() != command {
            return Err(ActionError::Conflict {
                command: command.clone(),
                reason: format!(
                    "authorization was prepared for '{}', not '{}'",
                    authorization.command_id(),
                    command
                ),
            });
        }
        // Attribution first: if this exact action produced an operation, report
        // that operation rather than a newer command-level uncertainty.
        if let Some(operation) = self.action_operations.get(&action).copied() {
            if let Some(operation_record) = self.operations.get(&operation)
                && operation_record.command != *command
            {
                return Err(ActionError::Conflict {
                    command: command.clone(),
                    reason: format!(
                        "action reference was prepared for '{}', not '{}'",
                        operation_record.command, command
                    ),
                });
            }
            return Err(self.replay_error(command, operation));
        }

        let Some(record) = self.prepared.get(&action.sequence()).cloned() else {
            return Err(unknown_reference(command, action));
        };
        if record.action != action {
            return Err(ActionError::Conflict {
                command: command.clone(),
                reason: "action reference no longer matches the prepared action".into(),
            });
        }
        if record.command != *command {
            return Err(ActionError::Conflict {
                command: command.clone(),
                reason: format!(
                    "action reference was prepared for '{}', not '{}'",
                    record.command, command
                ),
            });
        }

        if let Some(error) = self.pending_block_error(command, "executing") {
            return Err(error);
        }

        if record.requires_confirmation && !authorization.confirmed() {
            return Err(ActionError::Cancelled {
                command: command.clone(),
                reason: "explicit confirmation is required for this action".into(),
            });
        }
        if authorization.grant_revision() != grants.revision() {
            return Err(ActionError::Conflict {
                command: command.clone(),
                reason: format!(
                    "grant snapshot changed since authorization (revision {} -> {})",
                    authorization.grant_revision(),
                    grants.revision()
                ),
            });
        }

        let Some(spec) = self.specs.get(command) else {
            return Err(ActionError::MissingTarget {
                command: command.clone(),
                target: command.to_string(),
            });
        };
        let kind = spec.kind().clone();
        let required_capabilities = spec.required_capabilities().to_vec();

        for capability in &required_capabilities {
            if !grants.contains(capability) {
                return Err(ActionError::DeniedPermission {
                    command: command.clone(),
                    capability: capability.clone(),
                    reason: "capability is no longer granted".into(),
                });
            }
        }

        let target = match host.probe_target(spec) {
            TargetProbe::Present { identity, revision } => {
                if identity.trim().is_empty() {
                    return Err(ActionError::Conflict {
                        command: command.clone(),
                        reason: "host returned an empty target identity".into(),
                    });
                }
                if identity != record.target.identity() || revision != record.target.revision() {
                    return Err(ActionError::Conflict {
                        command: command.clone(),
                        reason: format!(
                            "target changed after prepare ('{}', revision {}) -> ('{}', revision {})",
                            record.target.identity(),
                            record.target.revision(),
                            identity,
                            revision
                        ),
                    });
                }
                record.target.clone()
            }
            TargetProbe::Missing => {
                return Err(ActionError::MissingTarget {
                    command: command.clone(),
                    target: record.target.identity().to_owned(),
                });
            }
            TargetProbe::Unsupported { capability } => {
                return Err(ActionError::UnsupportedCapability {
                    command: command.clone(),
                    capability: normalized_capability(&capability),
                });
            }
        };

        // Allocate the operation identity before the host can perform anything:
        // if the sequence space is exhausted, fail closed with no effect.
        let operation = OperationId::new(self.instance, self.allocate_sequence(command)?);
        let request = ExecutionRequest::new(
            command.clone(),
            kind,
            record.parameters.clone(),
            target.clone(),
        );
        let report = host.perform(request);

        match report {
            HostReport::Completed => {
                self.record_operation(
                    operation,
                    command.clone(),
                    target,
                    OperationStatus::Completed,
                    action,
                );
                Ok(ExecutionOutcome::completed(operation))
            }
            HostReport::Uncertain { reason } => {
                self.record_operation(
                    operation,
                    command.clone(),
                    target,
                    OperationStatus::Uncertain,
                    action,
                );
                self.add_pending_operation(command, operation);
                Ok(ExecutionOutcome::uncertain(operation, reason))
            }
            HostReport::Denied { capability, reason } => Err(ActionError::DeniedPermission {
                command: command.clone(),
                capability: {
                    let capability = normalized_capability(&capability);
                    if capability == "host" {
                        required_capabilities
                            .first()
                            .cloned()
                            .unwrap_or_else(|| "host".into())
                    } else {
                        capability
                    }
                },
                reason,
            }),
            HostReport::Unsupported { capability } => Err(ActionError::UnsupportedCapability {
                command: command.clone(),
                capability: normalized_capability(&capability),
            }),
            HostReport::Missing { target } => Err(ActionError::MissingTarget {
                command: command.clone(),
                target: if target.trim().is_empty() {
                    record.target.identity().to_owned()
                } else {
                    target
                },
            }),
            HostReport::Conflict { reason } => Err(ActionError::Conflict {
                command: command.clone(),
                reason,
            }),
            HostReport::Cancelled { reason } => Err(ActionError::Cancelled {
                command: command.clone(),
                reason,
            }),
        }
    }

    /// Resolves a previous operation by identity.
    ///
    /// A completed or failed operation is reported without another host call.
    /// An uncertain operation is re-probed through the host. A definite
    /// (`Completed`/`Failed`) or [`ReconcileOutcome::Unknown`] result removes
    /// only that operation from the command's pending set; the command stays
    /// blocked while any other operation remains pending.
    /// [`ReconcileOutcome::StillPending`] adds/re-adds the operation and keeps
    /// the block. An unknown result leaves the exact action blocked for replay
    /// but permits a fresh preparation only once no other operation is pending.
    pub fn reconcile<H: Host + ?Sized>(
        &mut self,
        operation: OperationId,
        host: &mut H,
    ) -> ReconcileOutcome {
        if operation.registry() != self.instance {
            return ReconcileOutcome::Unknown;
        }
        let Some(record) = self.operations.get(&operation) else {
            return ReconcileOutcome::Unknown;
        };
        let command = record.command.clone();
        match record.status {
            OperationStatus::Completed => {
                self.remove_pending_operation(&command, operation);
                return ReconcileOutcome::Completed;
            }
            OperationStatus::Failed => {
                self.release_prepared_for_operation(operation);
                self.remove_pending_operation(&command, operation);
                return ReconcileOutcome::Failed;
            }
            OperationStatus::Uncertain | OperationStatus::Unknown => {}
        }

        let request = ReconcileRequest::new(operation, command.clone(), record.target.clone());
        match host.reconcile(request) {
            HostReconcile::Completed => {
                if let Some(record) = self.operations.get_mut(&operation) {
                    record.status = OperationStatus::Completed;
                }
                self.remove_pending_operation(&command, operation);
                self.enforce_terminal_limit();
                ReconcileOutcome::Completed
            }
            HostReconcile::Failed => {
                if let Some(record) = self.operations.get_mut(&operation) {
                    record.status = OperationStatus::Failed;
                }
                self.release_prepared_for_operation(operation);
                self.remove_pending_operation(&command, operation);
                self.enforce_terminal_limit();
                ReconcileOutcome::Failed
            }
            HostReconcile::StillPending => {
                // Unknown is removed from the pending set so a host that later
                // discovers a pending record can re-add it. Sets preserve every
                // pending operation for the command.
                if let Some(record) = self.operations.get_mut(&operation) {
                    record.status = OperationStatus::Uncertain;
                }
                self.add_pending_operation(&command, operation);
                ReconcileOutcome::StillPending
            }
            HostReconcile::Unknown => {
                if let Some(record) = self.operations.get_mut(&operation) {
                    record.status = OperationStatus::Unknown;
                }
                self.release_prepared_for_operation(operation);
                self.remove_pending_operation(&command, operation);
                self.enforce_terminal_limit();
                ReconcileOutcome::Unknown
            }
        }
    }

    /// Discards an unexecuted prepared action.
    ///
    /// Returns `true` when a record was removed. References that already
    /// produced an operation cannot be discarded; they may be pruned after
    /// completion with [`Registry::prune_completed_operations`]. Discarding
    /// frees prepared-action capacity but does not alter command uncertainty.
    pub fn discard_prepared(&mut self, action: ActionRef) -> bool {
        if action.registry() != self.instance || self.action_operations.contains_key(&action) {
            return false;
        }
        self.prepared.remove(&action.sequence()).is_some()
    }

    /// Prunes completed operation history, keeping the newest `keep`
    /// operations.
    ///
    /// For every pruned operation the registry also removes its action
    /// tombstone and prepared record, so replaying the old [`ActionRef`] fails
    /// closed with [`ActionError::UnknownReference`] instead of performing the
    /// effect again. Failed and Unknown tombstones are bounded automatically by
    /// [`RegistryLimits::max_terminal_operations`]; uncertain/still-pending
    /// operations are never pruned by either path.
    ///
    /// Returns the number of completed operations removed.
    pub fn prune_completed_operations(&mut self, keep: usize) -> usize {
        let completed: Vec<OperationId> = self
            .operations
            .iter()
            .filter(|(_, record)| record.status == OperationStatus::Completed)
            .map(|(operation, _)| *operation)
            .collect();
        let remove_count = completed.len().saturating_sub(keep);
        let mut removed = 0;
        for operation in completed.into_iter().take(remove_count) {
            if self.remove_operation(operation) {
                removed += 1;
            }
        }
        removed
    }

    fn validate_spec(&self, spec: &CommandSpec) -> Result<(), RegistrationError> {
        let id = spec.id();
        if id.as_str().chars().count() > MAX_COMMAND_ID_CHARS {
            return Err(RegistrationError::CommandIdTooLong {
                id: id.clone(),
                limit: MAX_COMMAND_ID_CHARS,
            });
        }
        if let Some((character, position)) = first_invisible_identity_character(id.as_str()) {
            return Err(RegistrationError::InvalidCommandIdCharacter {
                id: id.clone(),
                character,
                position,
            });
        }
        if spec.title().trim().is_empty() {
            return Err(RegistrationError::EmptyTitle { id: id.clone() });
        }
        validate_display_text(id, "title", spec.title(), MAX_TITLE_CHARS)?;

        if spec.keywords().len() > MAX_KEYWORDS {
            return Err(RegistrationError::InvalidDisplayText {
                id: id.clone(),
                field: "keywords".into(),
                reason: format!("command declares more than {MAX_KEYWORDS} keywords"),
            });
        }
        for (index, keyword) in spec.keywords().iter().enumerate() {
            if keyword.trim().is_empty() {
                return Err(RegistrationError::InvalidDisplayText {
                    id: id.clone(),
                    field: format!("keyword {index}"),
                    reason: "keyword must not be empty".into(),
                });
            }
            validate_display_text(id, &format!("keyword {index}"), keyword, MAX_KEYWORD_CHARS)?;
        }

        if let Some(reason) = spec.kind().invalid_identity() {
            return Err(RegistrationError::InvalidKind {
                id: id.clone(),
                reason: reason.into(),
            });
        }
        spec.parameters().validate(id)?;

        if let Some(capability) = spec.kind().required_capability()
            && !self.capabilities.contains(capability)
        {
            return Err(RegistrationError::UnknownCapability {
                id: id.clone(),
                capability: capability.to_owned(),
            });
        }

        let mut seen = BTreeSet::new();
        for capability in spec.required_capabilities() {
            if capability.trim().is_empty() {
                return Err(RegistrationError::EmptyCapabilityName { id: id.clone() });
            }
            if !seen.insert(capability.as_str()) {
                return Err(RegistrationError::DuplicateCapability {
                    id: id.clone(),
                    capability: capability.clone(),
                });
            }
            if !self.capabilities.contains(capability) {
                return Err(RegistrationError::UnknownCapability {
                    id: id.clone(),
                    capability: capability.clone(),
                });
            }
        }
        Ok(())
    }

    fn allocate_sequence(&mut self, command: &CommandId) -> Result<u64, ActionError> {
        let sequence = self.next_sequence;
        self.next_sequence =
            sequence
                .checked_add(1)
                .ok_or_else(|| ActionError::ResourceExhausted {
                    command: command.clone(),
                    resource: "action sequence".into(),
                    limit: u64::MAX,
                })?;
        Ok(sequence)
    }

    fn add_pending_operation(&mut self, command: &CommandId, operation: OperationId) {
        self.pending_operations
            .entry(command.clone())
            .or_default()
            .insert(operation);
    }

    fn remove_pending_operation(&mut self, command: &CommandId, operation: OperationId) {
        if let Some(operations) = self.pending_operations.get_mut(command) {
            operations.remove(&operation);
            if operations.is_empty() {
                self.pending_operations.remove(command);
            }
        }
    }

    /// Builds the deterministic command-level block error.
    ///
    /// The oldest pending operation is reported, with the total pending count,
    /// so a fresh caller has a stable operation to reconcile.
    fn pending_block_error(&self, command: &CommandId, action: &str) -> Option<ActionError> {
        let operations = self.pending_operations.get(command)?;
        let oldest = *operations.iter().next()?;
        Some(uncertain_command_error(
            command,
            oldest,
            format!(
                "the command has {} unresolved uncertain operation(s); reconcile before {action} again",
                operations.len()
            ),
        ))
    }

    /// Number of pending uncertain operations for `command`.
    ///
    /// Any value greater than zero blocks fresh preparation, authorization and
    /// execution for that command.
    pub fn pending_operation_count(&self, command: &CommandId) -> usize {
        self.pending_operations
            .get(command)
            .map_or(0, BTreeSet::len)
    }

    /// Releases the prepared record for a terminal/unresolved operation.
    ///
    /// The operation tombstone remains so replaying the exact reference still
    /// fails closed, but prepared-action capacity is reclaimed.
    fn release_prepared_for_operation(&mut self, operation: OperationId) {
        if let Some(record) = self.operations.get(&operation) {
            self.prepared.remove(&record.action.sequence());
        }
    }

    fn remove_operation(&mut self, operation: OperationId) -> bool {
        if let Some(record) = self.operations.remove(&operation) {
            self.action_operations.remove(&record.action);
            self.prepared.remove(&record.action.sequence());
            self.remove_pending_operation(&record.command, operation);
            true
        } else {
            false
        }
    }

    /// Bounds terminal tombstones by removing the oldest `Completed`, `Failed`
    /// and `Unknown` operations.
    ///
    /// Uncertain/still-pending operations are never removed. Removing a
    /// tombstone also removes its action mapping and prepared record, so an old
    /// reference fails closed as unknown rather than executing again.
    fn enforce_terminal_limit(&mut self) {
        let limit = self.limits.max_terminal_operations;
        let terminal: Vec<OperationId> = self
            .operations
            .iter()
            .filter(|(_, record)| {
                matches!(
                    record.status,
                    OperationStatus::Completed | OperationStatus::Failed | OperationStatus::Unknown
                )
            })
            .map(|(operation, _)| *operation)
            .collect();
        if terminal.len() <= limit {
            return;
        }
        let remove_count = terminal.len() - limit;
        for operation in terminal.into_iter().take(remove_count) {
            self.remove_operation(operation);
        }
    }

    fn replay_error(&self, command: &CommandId, operation: OperationId) -> ActionError {
        match self.operations.get(&operation).map(|record| record.status) {
            Some(OperationStatus::Completed) => ActionError::Conflict {
                command: command.clone(),
                reason: "this action has already executed".into(),
            },
            Some(OperationStatus::Failed) => ActionError::Conflict {
                command: command.clone(),
                reason: "this action failed; prepare a new action".into(),
            },
            Some(OperationStatus::Uncertain) | Some(OperationStatus::Unknown) => {
                uncertain_command_error(
                    command,
                    operation,
                    "the previous attempt has an unknown or uncertain outcome; reconcile before retrying",
                )
            }
            None => ActionError::Conflict {
                command: command.clone(),
                reason: "operation history is no longer available".into(),
            },
        }
    }

    fn record_operation(
        &mut self,
        operation: OperationId,
        command: CommandId,
        target: ResolvedTarget,
        status: OperationStatus,
        action: ActionRef,
    ) {
        self.operations.insert(
            operation,
            OperationRecord {
                action,
                command,
                target,
                status,
            },
        );
        self.action_operations.insert(action, operation);
        self.enforce_terminal_limit();
    }

    #[cfg(test)]
    fn prepared_count(&self) -> usize {
        self.prepared.len()
    }

    #[cfg(test)]
    fn operation_count(&self) -> usize {
        self.operations.len()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

fn normalized_capability(capability: &str) -> String {
    let trimmed = capability.trim();
    if trimmed.is_empty() {
        "host".into()
    } else {
        trimmed.to_owned()
    }
}

fn unknown_reference(command: &CommandId, action: ActionRef) -> ActionError {
    ActionError::UnknownReference {
        command: command.clone(),
        reference: format!("{action:?}"),
    }
}

fn uncertain_command_error(
    command: &CommandId,
    operation: OperationId,
    reason: impl Into<String>,
) -> ActionError {
    ActionError::Uncertain {
        command: command.clone(),
        operation,
        reason: reason.into(),
    }
}

fn is_reserved_namespace(namespace: &str) -> bool {
    ["builtin", "app", "quicklink", "script", "extension"]
        .iter()
        .any(|reserved| namespace.eq_ignore_ascii_case(reserved))
}

fn validate_display_text(
    id: &CommandId,
    field: &str,
    value: &str,
    max_chars: usize,
) -> Result<(), RegistrationError> {
    if value.chars().any(char::is_control) {
        return Err(RegistrationError::InvalidDisplayText {
            id: id.clone(),
            field: field.to_owned(),
            reason: "text must not contain control characters".into(),
        });
    }
    if let Some((character, position)) = first_display_format_character(value) {
        return Err(RegistrationError::InvalidDisplayText {
            id: id.clone(),
            field: field.to_owned(),
            reason: format!(
                "text must not contain invisible format character {character:?} at position {position}"
            ),
        });
    }
    if value.chars().count() > max_chars {
        return Err(RegistrationError::InvalidDisplayText {
            id: id.clone(),
            field: field.to_owned(),
            reason: format!("text exceeds {max_chars} characters"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{CommandKind, Parameter, ParameterSchema, ParameterType, ParameterValue};
    use crate::error::ErrorCode;
    use crate::grant::capability_names;

    fn id(raw: &str) -> CommandId {
        CommandId::parse(raw).unwrap()
    }

    fn demo_spec() -> CommandSpec {
        CommandSpec::new(
            id("app:demo"),
            "Demo",
            CommandKind::Application {
                path: "C:\\Demo\\demo.exe".into(),
            },
        )
        .with_keywords(["demo", "application"])
        .with_parameters(
            ParameterSchema::new()
                .with_parameter(Parameter::required("query", ParameterType::Text))
                .with_parameter(
                    Parameter::optional("limit", ParameterType::Number)
                        .with_default(ParameterValue::Number(5.0)),
                ),
        )
        .requiring(capability_names::LAUNCH_APPLICATION)
    }

    fn script_spec() -> CommandSpec {
        CommandSpec::new(
            id("script:reviewed"),
            "Reviewed Script",
            CommandKind::Script {
                script_id: "reviewed".into(),
            },
        )
        .requiring(capability_names::RUN_SCRIPT)
    }

    fn quicklink_spec() -> CommandSpec {
        CommandSpec::new(
            id("quicklink:docs"),
            "Docs",
            CommandKind::Quicklink {
                url: "https://example.com/docs".into(),
            },
        )
        .requiring(capability_names::OPEN_URL)
    }

    #[derive(Debug)]
    struct FakeHost {
        present: bool,
        identity: String,
        revision: u64,
        unsupported: Option<String>,
        report: HostReport,
        reconcile: HostReconcile,
        performed: Vec<ExecutionRequest>,
        reconciliations: usize,
        probes: std::cell::Cell<usize>,
    }

    impl FakeHost {
        fn new() -> Self {
            Self {
                present: true,
                identity: "resolved-target".into(),
                revision: 1,
                unsupported: None,
                report: HostReport::Completed,
                reconcile: HostReconcile::Completed,
                performed: Vec::new(),
                reconciliations: 0,
                probes: std::cell::Cell::new(0),
            }
        }

        fn probe_count(&self) -> usize {
            self.probes.get()
        }
    }

    impl Host for FakeHost {
        fn probe_target(&self, _spec: &CommandSpec) -> TargetProbe {
            self.probes.set(self.probes.get() + 1);
            if let Some(capability) = &self.unsupported {
                TargetProbe::Unsupported {
                    capability: capability.clone(),
                }
            } else if self.present {
                TargetProbe::Present {
                    identity: self.identity.clone(),
                    revision: self.revision,
                }
            } else {
                TargetProbe::Missing
            }
        }

        fn perform(&mut self, request: ExecutionRequest) -> HostReport {
            self.performed.push(request);
            self.report.clone()
        }

        fn reconcile(&mut self, _request: ReconcileRequest) -> HostReconcile {
            self.reconciliations += 1;
            self.reconcile.clone()
        }
    }

    fn grants_for(capability: &str) -> GrantSnapshot {
        GrantSnapshot::from_capabilities([capability])
    }

    fn prepare_and_authorize(
        registry: &mut Registry,
        host: &FakeHost,
        capability: &str,
    ) -> (PreparedAction, Authorization, GrantSnapshot) {
        let grants = grants_for(capability);
        let prepared = registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "find"),
                host,
            )
            .unwrap();
        let authorization = registry
            .authorize(&prepared, &grants, Confirmation::Confirmed)
            .unwrap();
        (prepared, authorization, grants)
    }

    #[test]
    fn duplicate_registration_fails_atomically() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();

        let error = registry
            .register(CommandSpec::new(
                id("app:demo"),
                "Replacement",
                CommandKind::Application {
                    path: "other.exe".into(),
                },
            ))
            .unwrap_err();
        assert!(matches!(
            error,
            RegistrationError::DuplicateCommand { ref id } if id.as_str() == "app:demo"
        ));
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.get(&id("app:demo")).unwrap().title(), "Demo");
    }

    #[test]
    fn malformed_raw_ids_fail_with_useful_errors() {
        let mut registry = Registry::new();
        for raw in ["", "demo", ":x", "app:", "app:bad id", "app:a:b"] {
            let error = registry
                .register_raw(
                    raw,
                    "Untrusted",
                    CommandKind::Quicklink {
                        url: "https://example.com".into(),
                    },
                )
                .unwrap_err();
            assert!(
                matches!(error, RegistrationError::InvalidCommandId { .. }),
                "unexpected error for {raw:?}: {error}"
            );
            assert!(error.to_string().contains(raw) || raw.is_empty());
            assert!(registry.is_empty());
        }

        registry
            .register_raw(
                "quicklink:valid",
                "Valid",
                CommandKind::Quicklink {
                    url: "https://example.com".into(),
                },
            )
            .unwrap();
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn registration_rejects_schema_and_capability_defects() {
        let cases: Vec<CommandSpec> = vec![
            CommandSpec::new(id("app:empty-title"), "   ", CommandKind::Builtin),
            CommandSpec::new(
                id("app:empty-path"),
                "Empty path",
                CommandKind::Application { path: " ".into() },
            ),
            CommandSpec::new(
                id("app:duplicate-parameter"),
                "Duplicate parameter",
                CommandKind::Builtin,
            )
            .with_parameters(
                ParameterSchema::new()
                    .with_parameter(Parameter::required("value", ParameterType::Text))
                    .with_parameter(Parameter::optional("value", ParameterType::Text)),
            ),
            CommandSpec::new(
                id("app:empty-parameter"),
                "Empty parameter",
                CommandKind::Builtin,
            )
            .with_parameters(
                ParameterSchema::new().with_parameter(Parameter::required("", ParameterType::Text)),
            ),
            CommandSpec::new(id("app:bad-default"), "Bad default", CommandKind::Builtin)
                .with_parameters(
                    ParameterSchema::new().with_parameter(
                        Parameter::optional(
                            "mode",
                            ParameterType::Choice {
                                options: vec!["a".into()],
                            },
                        )
                        .with_default(ParameterValue::Choice("b".into())),
                    ),
                ),
            CommandSpec::new(
                id("app:empty-capability"),
                "Empty capability",
                CommandKind::Builtin,
            )
            .requiring(""),
            CommandSpec::new(
                id("app:duplicate-capability"),
                "Duplicate capability",
                CommandKind::Builtin,
            )
            .requiring(capability_names::OPEN_URL)
            .requiring(capability_names::OPEN_URL),
            CommandSpec::new(
                id("app:unknown-capability"),
                "Unknown capability",
                CommandKind::Builtin,
            )
            .requiring("vendor.unknown"),
        ];

        for spec in cases {
            let expected_id = spec.id().clone();
            let mut registry = Registry::new();
            let error = registry.register(spec).unwrap_err();
            assert!(
                error.to_string().contains(expected_id.as_str()),
                "error must name {expected_id}: {error}"
            );
            assert_eq!(registry.len(), 0, "failed registration must be atomic");
        }
    }

    #[test]
    fn custom_capability_catalog_allows_registration() {
        let mut registry =
            Registry::with_capabilities(CapabilityCatalog::from_names(["vendor.permission"]));
        registry
            .register(
                CommandSpec::new(id("extension:vendor"), "Vendor", CommandKind::Builtin)
                    .requiring("vendor.permission"),
            )
            .unwrap();
        assert_eq!(registry.len(), 1);

        let error = registry
            .register(CommandSpec::new(
                id("vendor:quicklink"),
                "Missing kind capability",
                CommandKind::Quicklink {
                    url: "https://example.com".into(),
                },
            ))
            .unwrap_err();
        assert!(matches!(
            error,
            RegistrationError::UnknownCapability { ref capability, .. }
                if capability == capability_names::OPEN_URL
        ));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn prepare_applies_defaults_and_reports_the_effect_before_execution() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let host = FakeHost::new();

        let prepared = registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "find"),
                &host,
            )
            .unwrap();

        assert_eq!(prepared.command_id(), &id("app:demo"));
        assert_eq!(prepared.effect().target().identity(), "resolved-target");
        assert_eq!(prepared.effect().target().kind(), "Application");
        assert_eq!(
            prepared.effect().summary(),
            "Launch application resolved-target"
        );
        assert!(prepared.effect().is_consequential());
        assert!(prepared.requires_confirmation());
        assert_eq!(prepared.target_revision(), 1);
        assert_eq!(
            prepared.parameters().get("query"),
            Some(&ParameterValue::Text("find".into()))
        );
        assert_eq!(
            prepared.parameters().get("limit"),
            Some(&ParameterValue::Number(5.0))
        );
        assert!(host.performed.is_empty());
    }

    #[test]
    fn consequential_action_exposes_target_and_effect_before_execution() {
        let mut registry = Registry::new();
        registry.register(script_spec()).unwrap();
        let host = FakeHost::new();

        let prepared = registry
            .prepare(&id("script:reviewed"), &ActionInput::new(), &host)
            .unwrap();
        assert!(prepared.requires_confirmation());
        assert!(prepared.effect().is_consequential());
        assert_eq!(prepared.effect().target().identity(), "resolved-target");
        assert_eq!(prepared.effect().target().kind(), "Script");
        assert!(prepared.effect().summary().contains("Run script"));

        let grants = grants_for(capability_names::RUN_SCRIPT);
        let error = registry
            .authorize(&prepared, &grants, Confirmation::Unconfirmed)
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::Cancelled);
        assert!(host.performed.is_empty());
    }

    #[test]
    fn under_declared_kinds_receive_minimum_capabilities() {
        let mut registry = Registry::new();
        registry
            .register(CommandSpec::new(
                id("quicklink:no-cap"),
                "No capability declared",
                CommandKind::Quicklink {
                    url: "https://example.com".into(),
                },
            ))
            .unwrap();
        let spec = registry.get(&id("quicklink:no-cap")).unwrap();
        assert_eq!(
            spec.required_capabilities(),
            &[capability_names::OPEN_URL.to_owned()]
        );
        assert!(spec.requires_confirmation());

        let host = FakeHost::new();
        let prepared = registry
            .prepare(&id("quicklink:no-cap"), &ActionInput::new(), &host)
            .unwrap();
        let error = registry
            .authorize(&prepared, &GrantSnapshot::none(), Confirmation::Confirmed)
            .unwrap_err();
        assert!(matches!(
            error,
            ActionError::DeniedPermission { ref capability, .. }
                if capability == capability_names::OPEN_URL
        ));

        registry
            .register(CommandSpec::new(
                id("script:no-cap"),
                "No capability declared",
                CommandKind::Script {
                    script_id: "reviewed".into(),
                },
            ))
            .unwrap();
        assert_eq!(
            registry
                .get(&id("script:no-cap"))
                .unwrap()
                .required_capabilities(),
            &[capability_names::RUN_SCRIPT.to_owned()]
        );

        registry
            .register(CommandSpec::new(
                id("app:no-cap"),
                "No capability declared",
                CommandKind::Application {
                    path: "demo.exe".into(),
                },
            ))
            .unwrap();
        assert_eq!(
            registry
                .get(&id("app:no-cap"))
                .unwrap()
                .required_capabilities(),
            &[capability_names::LAUNCH_APPLICATION.to_owned()]
        );

        registry
            .register(CommandSpec::new(
                id("extension:no-cap"),
                "No capability declared",
                CommandKind::Extension {
                    extension_id: "vendor".into(),
                    action: "run".into(),
                },
            ))
            .unwrap();
        assert_eq!(
            registry
                .get(&id("extension:no-cap"))
                .unwrap()
                .required_capabilities(),
            &[capability_names::INVOKE_EXTENSION.to_owned()]
        );
    }

    #[test]
    fn declaration_unions_with_kind_capability_without_duplication() {
        let mut registry = Registry::new();
        registry
            .register(
                CommandSpec::new(
                    id("app:explicit"),
                    "Explicit",
                    CommandKind::Application {
                        path: "demo.exe".into(),
                    },
                )
                .requiring(capability_names::LAUNCH_APPLICATION)
                .requiring(capability_names::OPEN_URL),
            )
            .unwrap();
        assert_eq!(
            registry
                .get(&id("app:explicit"))
                .unwrap()
                .required_capabilities(),
            &[
                capability_names::LAUNCH_APPLICATION.to_owned(),
                capability_names::OPEN_URL.to_owned(),
            ]
        );
    }

    #[test]
    fn authorization_checks_grants_and_names_the_missing_capability() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let host = FakeHost::new();
        let prepared = registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "find"),
                &host,
            )
            .unwrap();

        let error = registry
            .authorize(&prepared, &GrantSnapshot::none(), Confirmation::Confirmed)
            .unwrap_err();
        assert!(matches!(
            error,
            ActionError::DeniedPermission {
                ref capability, ..
            } if capability == capability_names::LAUNCH_APPLICATION
        ));

        let authorization = registry
            .authorize(
                &prepared,
                &grants_for(capability_names::LAUNCH_APPLICATION),
                Confirmation::Confirmed,
            )
            .unwrap();
        assert!(authorization.confirmed());
        assert_eq!(authorization.command_id(), &id("app:demo"));
    }

    #[test]
    fn execute_returns_a_typed_outcome_and_one_host_request() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        let (_, authorization, grants) =
            prepare_and_authorize(&mut registry, &host, capability_names::LAUNCH_APPLICATION);

        let outcome = registry
            .execute(&id("app:demo"), &authorization, &grants, &mut host)
            .unwrap();
        assert!(outcome.is_completed());
        assert!(!outcome.is_uncertain());
        assert_eq!(host.performed.len(), 1);

        let request = &host.performed[0];
        assert_eq!(request.command_id(), &id("app:demo"));
        assert!(matches!(
            request.kind(),
            CommandKind::Application { path } if path == "C:\\Demo\\demo.exe"
        ));
        assert_eq!(
            request.parameters().get("query"),
            Some(&ParameterValue::Text("find".into()))
        );
        assert_eq!(
            request.parameters().get("limit"),
            Some(&ParameterValue::Number(5.0))
        );
        assert_eq!(request.target().identity(), "resolved-target");
        assert_eq!(request.target().revision(), 1);
        assert!(outcome.operation_id().to_string().starts_with("operation-"));

        let error = registry
            .execute(&id("app:demo"), &authorization, &grants, &mut host)
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::Conflict);
        assert_eq!(host.performed.len(), 1);
    }

    #[test]
    fn cross_command_reference_is_rejected_without_performing() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        registry.register(quicklink_spec()).unwrap();
        let mut host = FakeHost::new();
        let (_, authorization, grants) =
            prepare_and_authorize(&mut registry, &host, capability_names::LAUNCH_APPLICATION);

        let error = registry
            .execute(&id("quicklink:docs"), &authorization, &grants, &mut host)
            .unwrap_err();
        assert!(matches!(error, ActionError::Conflict { .. }));
        assert_eq!(error.code(), ErrorCode::Conflict);
        assert!(error.to_string().contains("app:demo"));
        assert!(error.to_string().contains("quicklink:docs"));
        assert!(host.performed.is_empty());
    }

    #[test]
    fn stale_target_revision_is_rejected() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        let (_, authorization, grants) =
            prepare_and_authorize(&mut registry, &host, capability_names::LAUNCH_APPLICATION);

        host.revision = 2;
        let error = registry
            .execute(&id("app:demo"), &authorization, &grants, &mut host)
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::Conflict);
        assert!(error.to_string().contains("revision 1"));
        assert!(host.performed.is_empty());
    }

    #[test]
    fn target_identity_change_is_rejected() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        let (_, authorization, grants) =
            prepare_and_authorize(&mut registry, &host, capability_names::LAUNCH_APPLICATION);

        host.identity = "different-target".into();
        let error = registry
            .execute(&id("app:demo"), &authorization, &grants, &mut host)
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::Conflict);
        assert!(host.performed.is_empty());
    }

    #[test]
    fn missing_target_between_prepare_and_execute_is_rejected() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        let (_, authorization, grants) =
            prepare_and_authorize(&mut registry, &host, capability_names::LAUNCH_APPLICATION);

        host.present = false;
        let error = registry
            .execute(&id("app:demo"), &authorization, &grants, &mut host)
            .unwrap_err();
        assert!(matches!(
            error,
            ActionError::MissingTarget { ref target, .. } if target == "resolved-target"
        ));
        assert!(host.performed.is_empty());
    }

    #[test]
    fn revoked_permission_between_authorize_and_execute_is_rejected() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        let (_, authorization, _) =
            prepare_and_authorize(&mut registry, &host, capability_names::LAUNCH_APPLICATION);

        let revoked = GrantSnapshot::none();
        let error = registry
            .execute(&id("app:demo"), &authorization, &revoked, &mut host)
            .unwrap_err();
        assert!(matches!(
            error,
            ActionError::DeniedPermission {
                ref capability, ..
            } if capability == capability_names::LAUNCH_APPLICATION
        ));
        assert!(host.performed.is_empty());
    }

    #[test]
    fn uncertain_outcome_is_not_retried_and_reconcile_resolves_it() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        host.report = HostReport::Uncertain {
            reason: "connection dropped after send".into(),
        };
        let (prepared, authorization, grants) =
            prepare_and_authorize(&mut registry, &host, capability_names::LAUNCH_APPLICATION);

        let outcome = registry
            .execute(&id("app:demo"), &authorization, &grants, &mut host)
            .unwrap();
        assert!(outcome.is_uncertain());
        assert!(!outcome.is_completed());
        let operation = outcome.operation_id();
        assert_eq!(host.performed.len(), 1);

        let reauthorize = registry
            .authorize(&prepared, &grants, Confirmation::Confirmed)
            .unwrap_err();
        assert!(matches!(
            reauthorize,
            ActionError::Uncertain {
                operation: reauthorized,
                ..
            } if reauthorized == operation
        ));

        let second = registry
            .execute(&id("app:demo"), &authorization, &grants, &mut host)
            .unwrap_err();
        assert!(matches!(
            second,
            ActionError::Uncertain {
                operation: second_operation,
                ..
            } if second_operation == operation
        ));
        assert_eq!(host.performed.len(), 1, "uncertain effects must not retry");

        host.reconcile = HostReconcile::StillPending;
        assert_eq!(
            registry.reconcile(operation, &mut host),
            ReconcileOutcome::StillPending
        );
        host.reconcile = HostReconcile::Unknown;
        assert_eq!(
            registry.reconcile(operation, &mut host),
            ReconcileOutcome::Unknown
        );
        host.reconcile = HostReconcile::Completed;
        assert_eq!(
            registry.reconcile(operation, &mut host),
            ReconcileOutcome::Completed
        );
        assert_eq!(host.reconciliations, 3);

        let third = registry
            .execute(&id("app:demo"), &authorization, &grants, &mut host)
            .unwrap_err();
        assert_eq!(third.code(), ErrorCode::Conflict);
        assert_eq!(host.performed.len(), 1, "completed effects never replay");
    }

    #[test]
    fn uncertain_operation_blocks_fresh_preparation_until_reconcile() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        host.report = HostReport::Uncertain {
            reason: "send result lost".into(),
        };
        let (_, authorization, grants) =
            prepare_and_authorize(&mut registry, &host, capability_names::LAUNCH_APPLICATION);

        let outcome = registry
            .execute(&id("app:demo"), &authorization, &grants, &mut host)
            .unwrap();
        assert!(outcome.is_uncertain());
        let operation = outcome.operation_id();
        assert_eq!(host.performed.len(), 1);

        let fresh = registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "again"),
                &host,
            )
            .unwrap_err();
        assert!(matches!(
            fresh,
            ActionError::Uncertain {
                operation: blocked,
                ..
            } if blocked == operation
        ));
        assert_eq!(host.performed.len(), 1);

        host.reconcile = HostReconcile::StillPending;
        assert_eq!(
            registry.reconcile(operation, &mut host),
            ReconcileOutcome::StillPending
        );
        assert!(matches!(
            registry
                .prepare(
                    &id("app:demo"),
                    &ActionInput::new().with("query", "again"),
                    &host,
                )
                .unwrap_err(),
            ActionError::Uncertain { .. }
        ));

        host.reconcile = HostReconcile::Completed;
        assert_eq!(
            registry.reconcile(operation, &mut host),
            ReconcileOutcome::Completed
        );
        host.report = HostReport::Completed;
        let fresh = registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "again"),
                &host,
            )
            .unwrap();
        let fresh_authorization = registry
            .authorize(&fresh, &grants, Confirmation::Confirmed)
            .unwrap();
        let second = registry
            .execute(&id("app:demo"), &fresh_authorization, &grants, &mut host)
            .unwrap();
        assert!(second.is_completed());
        assert_eq!(host.performed.len(), 2);
    }

    #[test]
    fn failed_reconcile_clears_uncertain_block() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        host.report = HostReport::Uncertain {
            reason: "unknown".into(),
        };
        let (_, authorization, grants) =
            prepare_and_authorize(&mut registry, &host, capability_names::LAUNCH_APPLICATION);
        let operation = registry
            .execute(&id("app:demo"), &authorization, &grants, &mut host)
            .unwrap()
            .operation_id();

        host.reconcile = HostReconcile::Failed;
        assert_eq!(
            registry.reconcile(operation, &mut host),
            ReconcileOutcome::Failed
        );
        registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "retry"),
                &host,
            )
            .unwrap();
    }

    #[test]
    fn older_still_pending_keeps_command_blocked_after_newer_completes() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        let grants = grants_for(capability_names::LAUNCH_APPLICATION);
        host.report = HostReport::Uncertain {
            reason: "first uncertain".into(),
        };

        let prepared_a = registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "first"),
                &host,
            )
            .unwrap();
        let authorization_a = registry
            .authorize(&prepared_a, &grants, Confirmation::Confirmed)
            .unwrap();
        let operation_a = registry
            .execute(&id("app:demo"), &authorization_a, &grants, &mut host)
            .unwrap()
            .operation_id();

        host.reconcile = HostReconcile::Unknown;
        assert_eq!(
            registry.reconcile(operation_a, &mut host),
            ReconcileOutcome::Unknown
        );
        assert_eq!(registry.pending_operation_count(&id("app:demo")), 0);

        let prepared_b = registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "second"),
                &host,
            )
            .unwrap();
        let authorization_b = registry
            .authorize(&prepared_b, &grants, Confirmation::Confirmed)
            .unwrap();
        let operation_b = registry
            .execute(&id("app:demo"), &authorization_b, &grants, &mut host)
            .unwrap()
            .operation_id();

        host.reconcile = HostReconcile::StillPending;
        assert_eq!(
            registry.reconcile(operation_a, &mut host),
            ReconcileOutcome::StillPending
        );
        assert_eq!(registry.pending_operation_count(&id("app:demo")), 2);

        host.reconcile = HostReconcile::Completed;
        assert_eq!(
            registry.reconcile(operation_b, &mut host),
            ReconcileOutcome::Completed
        );
        assert_eq!(registry.pending_operation_count(&id("app:demo")), 1);

        let blocked = registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "must-block"),
                &host,
            )
            .unwrap_err();
        assert!(matches!(
            blocked,
            ActionError::Uncertain {
                operation: reported,
                ref reason,
                ..
            } if reported == operation_a && reason.contains("1 unresolved")
        ));

        host.reconcile = HostReconcile::Completed;
        assert_eq!(
            registry.reconcile(operation_a, &mut host),
            ReconcileOutcome::Completed
        );
        assert_eq!(registry.pending_operation_count(&id("app:demo")), 0);
        registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "allowed"),
                &host,
            )
            .unwrap();
        assert_eq!(host.performed.len(), 2);
    }

    #[test]
    fn two_pending_operations_keep_the_block_until_both_resolve() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        let grants = grants_for(capability_names::LAUNCH_APPLICATION);
        host.report = HostReport::Uncertain {
            reason: "uncertain".into(),
        };

        let prepared_a = registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "a"),
                &host,
            )
            .unwrap();
        let authorization_a = registry
            .authorize(&prepared_a, &grants, Confirmation::Confirmed)
            .unwrap();
        let operation_a = registry
            .execute(&id("app:demo"), &authorization_a, &grants, &mut host)
            .unwrap()
            .operation_id();

        host.reconcile = HostReconcile::Unknown;
        registry.reconcile(operation_a, &mut host);

        let prepared_b = registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "b"),
                &host,
            )
            .unwrap();
        let authorization_b = registry
            .authorize(&prepared_b, &grants, Confirmation::Confirmed)
            .unwrap();
        let operation_b = registry
            .execute(&id("app:demo"), &authorization_b, &grants, &mut host)
            .unwrap()
            .operation_id();

        host.reconcile = HostReconcile::StillPending;
        assert_eq!(
            registry.reconcile(operation_a, &mut host),
            ReconcileOutcome::StillPending
        );
        assert_eq!(registry.pending_operation_count(&id("app:demo")), 2);

        host.reconcile = HostReconcile::Completed;
        assert_eq!(
            registry.reconcile(operation_a, &mut host),
            ReconcileOutcome::Completed
        );
        assert_eq!(registry.pending_operation_count(&id("app:demo")), 1);
        assert!(matches!(
            registry
                .prepare(
                    &id("app:demo"),
                    &ActionInput::new().with("query", "blocked"),
                    &host,
                )
                .unwrap_err(),
            ActionError::Uncertain {
                operation: reported,
                ..
            } if reported == operation_b
        ));

        host.reconcile = HostReconcile::Failed;
        assert_eq!(
            registry.reconcile(operation_b, &mut host),
            ReconcileOutcome::Failed
        );
        assert_eq!(registry.pending_operation_count(&id("app:demo")), 0);
        registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "allowed"),
                &host,
            )
            .unwrap();
    }

    #[test]
    fn repeated_unknown_pending_cycles_keep_the_set_consistent() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        let grants = grants_for(capability_names::LAUNCH_APPLICATION);
        host.report = HostReport::Uncertain {
            reason: "uncertain".into(),
        };

        let prepared = registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "cycles"),
                &host,
            )
            .unwrap();
        let authorization = registry
            .authorize(&prepared, &grants, Confirmation::Confirmed)
            .unwrap();
        let operation = registry
            .execute(&id("app:demo"), &authorization, &grants, &mut host)
            .unwrap()
            .operation_id();

        for cycle in 0..6 {
            host.reconcile = HostReconcile::Unknown;
            assert_eq!(
                registry.reconcile(operation, &mut host),
                ReconcileOutcome::Unknown
            );
            assert_eq!(
                registry.pending_operation_count(&id("app:demo")),
                0,
                "cycle {cycle}"
            );

            host.reconcile = HostReconcile::StillPending;
            assert_eq!(
                registry.reconcile(operation, &mut host),
                ReconcileOutcome::StillPending
            );
            assert_eq!(
                registry.pending_operation_count(&id("app:demo")),
                1,
                "cycle {cycle}"
            );
            assert_eq!(registry.operation_count(), 1, "cycle {cycle}");
        }

        host.reconcile = HostReconcile::Completed;
        assert_eq!(
            registry.reconcile(operation, &mut host),
            ReconcileOutcome::Completed
        );
        assert_eq!(registry.pending_operation_count(&id("app:demo")), 0);
        registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "after"),
                &host,
            )
            .unwrap();
    }

    #[test]
    fn still_pending_after_unknown_reestablishes_command_block() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        host.report = HostReport::Uncertain {
            reason: "host lost the result".into(),
        };
        let (_, authorization, grants) =
            prepare_and_authorize(&mut registry, &host, capability_names::LAUNCH_APPLICATION);
        let operation = registry
            .execute(&id("app:demo"), &authorization, &grants, &mut host)
            .unwrap()
            .operation_id();

        host.reconcile = HostReconcile::Unknown;
        assert_eq!(
            registry.reconcile(operation, &mut host),
            ReconcileOutcome::Unknown
        );
        let fresh = registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "unknown-cleared"),
                &host,
            )
            .unwrap();
        assert!(registry.discard_prepared(fresh.action_ref()));

        host.reconcile = HostReconcile::StillPending;
        assert_eq!(
            registry.reconcile(operation, &mut host),
            ReconcileOutcome::StillPending
        );
        let blocked = registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "must-block"),
                &host,
            )
            .unwrap_err();
        assert!(matches!(
            blocked,
            ActionError::Uncertain {
                operation: blocked_operation,
                ..
            } if blocked_operation == operation
        ));
        assert_eq!(host.performed.len(), 1);

        host.reconcile = HostReconcile::Failed;
        assert_eq!(
            registry.reconcile(operation, &mut host),
            ReconcileOutcome::Failed
        );
        registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "failed-cleared"),
                &host,
            )
            .unwrap();
    }

    #[test]
    fn multiple_uncertain_actions_report_the_exact_operation() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        host.report = HostReport::Uncertain {
            reason: "first unknown".into(),
        };
        let grants = grants_for(capability_names::LAUNCH_APPLICATION);
        let prepared_a = registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "first"),
                &host,
            )
            .unwrap();
        let authorization_a = registry
            .authorize(&prepared_a, &grants, Confirmation::Confirmed)
            .unwrap();
        let operation_a = registry
            .execute(&id("app:demo"), &authorization_a, &grants, &mut host)
            .unwrap()
            .operation_id();

        host.reconcile = HostReconcile::Unknown;
        assert_eq!(
            registry.reconcile(operation_a, &mut host),
            ReconcileOutcome::Unknown
        );

        let prepared_b = registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "second"),
                &host,
            )
            .unwrap();
        let authorization_b = registry
            .authorize(&prepared_b, &grants, Confirmation::Confirmed)
            .unwrap();
        let operation_b = registry
            .execute(&id("app:demo"), &authorization_b, &grants, &mut host)
            .unwrap()
            .operation_id();
        assert_ne!(operation_a, operation_b);

        let replay = registry
            .execute(&id("app:demo"), &authorization_a, &grants, &mut host)
            .unwrap_err();
        assert!(matches!(
            replay,
            ActionError::Uncertain {
                operation: reported,
                ..
            } if reported == operation_a
        ));
        let reauthorize = registry
            .authorize(&prepared_a, &grants, Confirmation::Confirmed)
            .unwrap_err();
        assert!(matches!(
            reauthorize,
            ActionError::Uncertain {
                operation: reported,
                ..
            } if reported == operation_a
        ));

        let fresh = registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "third"),
                &host,
            )
            .unwrap_err();
        assert!(matches!(
            fresh,
            ActionError::Uncertain {
                operation: reported,
                ..
            } if reported == operation_b
        ));
        assert_eq!(host.performed.len(), 2);
    }

    #[test]
    fn failed_and_unknown_resolutions_release_prepared_capacity() {
        let mut registry = Registry::with_limits(
            CapabilityCatalog::builtin(),
            RegistryLimits {
                max_prepared_actions: 1,
                ..Default::default()
            },
        );
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        host.report = HostReport::Uncertain {
            reason: "host lost the result".into(),
        };
        let grants = grants_for(capability_names::LAUNCH_APPLICATION);

        for cycle in 0..6 {
            let prepared = registry
                .prepare(
                    &id("app:demo"),
                    &ActionInput::new().with("query", format!("cycle-{cycle}")),
                    &host,
                )
                .unwrap();
            let authorization = registry
                .authorize(&prepared, &grants, Confirmation::Confirmed)
                .unwrap();
            let operation = registry
                .execute(&id("app:demo"), &authorization, &grants, &mut host)
                .unwrap()
                .operation_id();

            if cycle % 2 == 0 {
                host.reconcile = HostReconcile::Failed;
                assert_eq!(
                    registry.reconcile(operation, &mut host),
                    ReconcileOutcome::Failed
                );
            } else {
                host.reconcile = HostReconcile::Unknown;
                assert_eq!(
                    registry.reconcile(operation, &mut host),
                    ReconcileOutcome::Unknown
                );
            }
            assert_eq!(
                registry.prepared_count(),
                0,
                "terminal resolution must release prepared capacity"
            );
        }

        registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "still-available"),
                &host,
            )
            .unwrap();
    }

    #[test]
    fn terminal_tombstone_retention_is_bounded_and_fails_closed() {
        let mut registry = Registry::with_limits(
            CapabilityCatalog::builtin(),
            RegistryLimits {
                max_prepared_actions: 8,
                max_terminal_operations: 3,
            },
        );
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        host.report = HostReport::Uncertain {
            reason: "host lost the result".into(),
        };
        let grants = grants_for(capability_names::LAUNCH_APPLICATION);
        let mut authorizations = Vec::new();
        let mut operations = Vec::new();

        for cycle in 0..10 {
            let prepared = registry
                .prepare(
                    &id("app:demo"),
                    &ActionInput::new().with("query", format!("cycle-{cycle}")),
                    &host,
                )
                .unwrap();
            let authorization = registry
                .authorize(&prepared, &grants, Confirmation::Confirmed)
                .unwrap();
            let operation = registry
                .execute(&id("app:demo"), &authorization, &grants, &mut host)
                .unwrap()
                .operation_id();
            authorizations.push(authorization);
            operations.push(operation);

            if cycle % 2 == 0 {
                host.reconcile = HostReconcile::Failed;
                assert_eq!(
                    registry.reconcile(operation, &mut host),
                    ReconcileOutcome::Failed
                );
            } else {
                host.reconcile = HostReconcile::Unknown;
                assert_eq!(
                    registry.reconcile(operation, &mut host),
                    ReconcileOutcome::Unknown
                );
            }
            assert!(registry.operation_count() <= 3);
        }

        let performs = host.performed.len();
        let pruned = registry
            .execute(&id("app:demo"), &authorizations[0], &grants, &mut host)
            .unwrap_err();
        assert!(matches!(pruned, ActionError::UnknownReference { .. }));
        assert_eq!(host.performed.len(), performs);

        // The newest tombstone is retained, so exact replay is still blocked.
        let recent = registry
            .execute(&id("app:demo"), &authorizations[9], &grants, &mut host)
            .unwrap_err();
        assert!(matches!(
            recent,
            ActionError::Uncertain {
                operation: reported,
                ..
            } if reported == operations[9]
        ));
        assert_eq!(host.performed.len(), performs);
        assert!(registry.operation_count() <= 3);
    }

    #[test]
    fn automatic_terminal_pruning_never_removes_pending_operations() {
        let mut registry = Registry::with_limits(
            CapabilityCatalog::builtin(),
            RegistryLimits {
                max_prepared_actions: 8,
                max_terminal_operations: 1,
            },
        );
        registry.register(demo_spec()).unwrap();
        registry.register(quicklink_spec()).unwrap();
        let mut host = FakeHost::new();

        host.report = HostReport::Uncertain {
            reason: "pending".into(),
        };
        let pending = registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "pending"),
                &host,
            )
            .unwrap();
        let pending_authorization = registry
            .authorize(
                &pending,
                &grants_for(capability_names::LAUNCH_APPLICATION),
                Confirmation::Confirmed,
            )
            .unwrap();
        let pending_operation = registry
            .execute(
                &id("app:demo"),
                &pending_authorization,
                &grants_for(capability_names::LAUNCH_APPLICATION),
                &mut host,
            )
            .unwrap()
            .operation_id();
        host.reconcile = HostReconcile::StillPending;
        assert_eq!(
            registry.reconcile(pending_operation, &mut host),
            ReconcileOutcome::StillPending
        );

        host.report = HostReport::Completed;
        for cycle in 0..5 {
            let prepared = registry
                .prepare(&id("quicklink:docs"), &ActionInput::new(), &host)
                .unwrap();
            let authorization = registry
                .authorize(
                    &prepared,
                    &grants_for(capability_names::OPEN_URL),
                    Confirmation::Confirmed,
                )
                .unwrap();
            registry
                .execute(
                    &id("quicklink:docs"),
                    &authorization,
                    &grants_for(capability_names::OPEN_URL),
                    &mut host,
                )
                .unwrap();
            assert!(registry.operation_count() <= 2, "cycle {cycle}");
        }

        let reconciliations = host.reconciliations;
        host.reconcile = HostReconcile::StillPending;
        assert_eq!(
            registry.reconcile(pending_operation, &mut host),
            ReconcileOutcome::StillPending
        );
        assert_eq!(host.reconciliations, reconciliations + 1);
    }

    #[test]
    fn identity_and_provider_namespace_lengths_are_bounded() {
        let mut registry = Registry::new();
        let long_id = CommandId::from_parts("vendor", "x".repeat(MAX_COMMAND_ID_CHARS)).unwrap();
        let error = registry
            .register(CommandSpec::new(long_id, "Long id", CommandKind::Builtin))
            .unwrap_err();
        assert!(matches!(
            error,
            RegistrationError::CommandIdTooLong { limit, .. } if limit == MAX_COMMAND_ID_CHARS
        ));
        assert!(registry.is_empty());

        let namespace = "v".repeat(MAX_COMMAND_ID_CHARS + 1);
        let id_in_long_namespace = CommandId::from_parts(&namespace, "tool").unwrap();
        let error = registry
            .register_untrusted(
                &namespace,
                CommandSpec::new(
                    id_in_long_namespace,
                    "Long namespace",
                    CommandKind::Extension {
                        extension_id: "vendor".into(),
                        action: "run".into(),
                    },
                ),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            RegistrationError::ProviderNamespaceTooLong { limit, .. }
                if limit == MAX_COMMAND_ID_CHARS
        ));
    }

    #[test]
    fn untrusted_namespace_is_a_convention_not_authentication() {
        let mut registry = Registry::new();

        let error = registry
            .register_untrusted(
                "attacker",
                CommandSpec::new(
                    id("victim:tool"),
                    "Claims another namespace",
                    CommandKind::Extension {
                        extension_id: "victim".into(),
                        action: "run".into(),
                    },
                ),
            )
            .unwrap_err();
        assert!(matches!(error, RegistrationError::NamespaceMismatch { .. }));

        // In-process code can bypass the convention through the trusted API;
        // this is documented as the host trust boundary, not prevented here.
        registry
            .register(CommandSpec::new(
                id("victim:tool"),
                "Trusted registration",
                CommandKind::Extension {
                    extension_id: "victim".into(),
                    action: "run".into(),
                },
            ))
            .unwrap();

        // Target ownership is not validated against the provider namespace;
        // the host must enforce it.
        registry
            .register_untrusted(
                "vendor",
                CommandSpec::new(
                    id("vendor:proxy"),
                    "Proxy target",
                    CommandKind::Extension {
                        extension_id: "victim".into(),
                        action: "run".into(),
                    },
                ),
            )
            .unwrap();
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn registration_rejects_invisible_display_and_target_text() {
        let cases = [
            (
                CommandSpec::new(
                    id("app:invisible-title"),
                    "Title\u{200B}spoof",
                    CommandKind::Builtin,
                ),
                false,
            ),
            (
                CommandSpec::new(id("app:invisible-keyword"), "Title", CommandKind::Builtin)
                    .with_keywords(["key\u{200B}word"]),
                false,
            ),
            (
                CommandSpec::new(
                    id("app:invisible-target"),
                    "Title",
                    CommandKind::Application {
                        path: "demo\u{200B}.exe".into(),
                    },
                ),
                true,
            ),
        ];

        for (spec, target_error) in cases {
            let mut registry = Registry::new();
            let error = registry.register(spec).unwrap_err();
            if target_error {
                assert!(matches!(error, RegistrationError::InvalidKind { .. }));
            } else {
                assert!(matches!(
                    error,
                    RegistrationError::InvalidDisplayText { .. }
                ));
            }
            assert!(registry.is_empty());
        }
    }

    #[test]
    fn completed_operation_reconciles_without_a_host_call() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        let (_, authorization, grants) =
            prepare_and_authorize(&mut registry, &host, capability_names::LAUNCH_APPLICATION);

        let outcome = registry
            .execute(&id("app:demo"), &authorization, &grants, &mut host)
            .unwrap();
        assert_eq!(
            registry.reconcile(outcome.operation_id(), &mut host),
            ReconcileOutcome::Completed
        );
        assert_eq!(host.reconciliations, 0);
    }

    #[test]
    fn reconcile_reports_unknown_for_foreign_operations() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        let (_, authorization, grants) =
            prepare_and_authorize(&mut registry, &host, capability_names::LAUNCH_APPLICATION);
        let outcome = registry
            .execute(&id("app:demo"), &authorization, &grants, &mut host)
            .unwrap();

        let mut other = Registry::new();
        assert_eq!(
            other.reconcile(outcome.operation_id(), &mut host),
            ReconcileOutcome::Unknown
        );
    }

    #[test]
    fn references_from_another_registry_are_rejected() {
        let mut first = Registry::new();
        first.register(demo_spec()).unwrap();
        let mut second = Registry::new();
        second.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        let grants = grants_for(capability_names::LAUNCH_APPLICATION);

        let prepared = second
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "find"),
                &host,
            )
            .unwrap();
        let error = first
            .authorize(&prepared, &grants, Confirmation::Confirmed)
            .unwrap_err();
        assert!(matches!(error, ActionError::UnknownReference { .. }));
        assert_eq!(error.code(), ErrorCode::MissingTarget);

        let authorization = second
            .authorize(&prepared, &grants, Confirmation::Confirmed)
            .unwrap();
        let error = first
            .execute(&id("app:demo"), &authorization, &grants, &mut host)
            .unwrap_err();
        assert!(matches!(error, ActionError::UnknownReference { .. }));
        assert_eq!(error.code(), ErrorCode::MissingTarget);
        assert!(host.performed.is_empty());
    }

    #[test]
    fn unsupported_host_probe_is_surfaced_at_prepare() {
        let mut registry = Registry::new();
        registry.register(script_spec()).unwrap();
        let mut host = FakeHost::new();
        host.unsupported = Some("vendor.runner".into());

        let error = registry
            .prepare(&id("script:reviewed"), &ActionInput::new(), &host)
            .unwrap_err();
        assert!(matches!(
            error,
            ActionError::UnsupportedCapability { ref capability, .. }
                if capability == "vendor.runner"
        ));
        assert_eq!(registry.prepared_count(), 0);
    }

    #[test]
    fn prepare_rejects_unknown_commands_and_invalid_input() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let host = FakeHost::new();

        let error = registry
            .prepare(&id("app:missing"), &ActionInput::new(), &host)
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::MissingTarget);

        let error = registry
            .prepare(&id("app:demo"), &ActionInput::new(), &host)
            .unwrap_err();
        assert!(matches!(
            error,
            ActionError::InvalidInput { ref field, .. } if field == "query"
        ));
        assert_eq!(registry.prepared_count(), 0);
    }

    #[test]
    fn host_denial_after_probe_maps_to_denied_permission() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        host.report = HostReport::Denied {
            capability: String::new(),
            reason: "sandbox refused launch".into(),
        };
        let (_, authorization, grants) =
            prepare_and_authorize(&mut registry, &host, capability_names::LAUNCH_APPLICATION);

        let error = registry
            .execute(&id("app:demo"), &authorization, &grants, &mut host)
            .unwrap_err();
        assert!(matches!(
            error,
            ActionError::DeniedPermission {
                ref capability,
                ref reason,
                ..
            } if capability == capability_names::LAUNCH_APPLICATION
                && reason == "sandbox refused launch"
        ));
    }

    #[test]
    fn untrusted_registration_enforces_provenance() {
        let mut registry = Registry::new();

        let error = registry
            .register_untrusted(
                "builtin",
                CommandSpec::new(id("builtin:settings"), "Fake", CommandKind::Builtin),
            )
            .unwrap_err();
        assert!(matches!(error, RegistrationError::ReservedNamespace { .. }));
        assert!(registry.is_empty());

        let error = registry
            .register_untrusted(
                "Builtin",
                CommandSpec::new(id("Builtin:settings"), "Fake", CommandKind::Builtin),
            )
            .unwrap_err();
        assert!(matches!(error, RegistrationError::ReservedNamespace { .. }));

        let error = registry
            .register_untrusted(
                "vendor",
                CommandSpec::new(id("builtin:settings"), "Fake", CommandKind::Builtin),
            )
            .unwrap_err();
        assert!(matches!(error, RegistrationError::NamespaceMismatch { .. }));

        let error = registry
            .register_untrusted(
                "app",
                CommandSpec::new(
                    id("app:demo"),
                    "Fake app",
                    CommandKind::Application {
                        path: "demo.exe".into(),
                    },
                ),
            )
            .unwrap_err();
        assert!(matches!(error, RegistrationError::ReservedNamespace { .. }));

        let error = registry
            .register_untrusted(
                "vendor",
                CommandSpec::new(id("vendor:builtin"), "Fake", CommandKind::Builtin),
            )
            .unwrap_err();
        assert!(matches!(error, RegistrationError::ForbiddenKind { .. }));

        registry
            .register_untrusted(
                "vendor",
                CommandSpec::new(
                    id("vendor:extension"),
                    "Vendor extension",
                    CommandKind::Extension {
                        extension_id: "vendor".into(),
                        action: "run".into(),
                    },
                ),
            )
            .unwrap();
        assert_eq!(
            registry
                .get(&id("vendor:extension"))
                .unwrap()
                .required_capabilities(),
            &[capability_names::INVOKE_EXTENSION.to_owned()]
        );

        registry
            .register_untrusted(
                "vendor",
                CommandSpec::new(
                    id("vendor:script"),
                    "Vendor script",
                    CommandKind::Script {
                        script_id: "vendor-script".into(),
                    },
                ),
            )
            .unwrap();
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn registration_rejects_invisible_ids_but_keeps_case_variants_distinct() {
        let mut registry = Registry::new();
        let error = registry
            .register(CommandSpec::new(
                CommandId::parse("app:set\u{200B}tings").unwrap(),
                "Invisible",
                CommandKind::Builtin,
            ))
            .unwrap_err();
        assert!(matches!(
            error,
            RegistrationError::InvalidCommandIdCharacter { character, .. }
                if character == '\u{200B}'
        ));
        assert!(registry.is_empty());

        registry
            .register(CommandSpec::new(
                id("app:Demo"),
                "Upper",
                CommandKind::Builtin,
            ))
            .unwrap();
        registry
            .register(CommandSpec::new(
                id("app:demo"),
                "Lower",
                CommandKind::Builtin,
            ))
            .unwrap();
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn registration_rejects_control_and_oversized_display_text() {
        let cases = [
            CommandSpec::new(id("app:control-title"), "T\0X", CommandKind::Builtin),
            CommandSpec::new(id("app:newline-title"), "T\nX", CommandKind::Builtin),
            CommandSpec::new(id("app:control-keyword"), "Title", CommandKind::Builtin)
                .with_keywords(["good", "bad\0keyword"]),
            CommandSpec::new(
                id("app:long-title"),
                "x".repeat(MAX_TITLE_CHARS + 1),
                CommandKind::Builtin,
            ),
            CommandSpec::new(id("app:long-keyword"), "Title", CommandKind::Builtin)
                .with_keywords(["x".repeat(MAX_KEYWORD_CHARS + 1)]),
        ];

        for spec in cases {
            let expected = spec.id().clone();
            let mut registry = Registry::new();
            let error = registry.register(spec).unwrap_err();
            assert!(
                matches!(error, RegistrationError::InvalidDisplayText { .. }),
                "unexpected error for {expected}: {error}"
            );
            assert!(registry.is_empty());
        }

        let mut registry = Registry::new();
        let too_many_keywords = (0..=MAX_KEYWORDS)
            .map(|index| format!("keyword-{index}"))
            .collect::<Vec<_>>();
        let error = registry
            .register(
                CommandSpec::new(id("app:too-many-keywords"), "Title", CommandKind::Builtin)
                    .with_keywords(too_many_keywords),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            RegistrationError::InvalidDisplayText { ref field, .. } if field == "keywords"
        ));
    }

    #[test]
    fn execute_rejects_a_different_grant_snapshot_revision() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        let grants = GrantSnapshot::from_capabilities([capability_names::LAUNCH_APPLICATION])
            .with_revision(7);
        let prepared = registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "find"),
                &host,
            )
            .unwrap();
        let authorization = registry
            .authorize(&prepared, &grants, Confirmation::Confirmed)
            .unwrap();
        assert_eq!(authorization.grant_revision(), 7);

        let newer = GrantSnapshot::from_capabilities([capability_names::LAUNCH_APPLICATION])
            .with_revision(8);
        let error = registry
            .execute(&id("app:demo"), &authorization, &newer, &mut host)
            .unwrap_err();
        assert!(matches!(
            error,
            ActionError::Conflict { ref reason, .. } if reason.contains("grant snapshot")
        ));
        assert!(host.performed.is_empty());
    }

    #[test]
    fn replay_guards_run_before_grants_and_probes() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        let (prepared, authorization, grants) =
            prepare_and_authorize(&mut registry, &host, capability_names::LAUNCH_APPLICATION);
        registry
            .execute(&id("app:demo"), &authorization, &grants, &mut host)
            .unwrap();
        let probes = host.probe_count();

        let authorize_error = registry
            .authorize(&prepared, &GrantSnapshot::none(), Confirmation::Confirmed)
            .unwrap_err();
        assert_eq!(authorize_error.code(), ErrorCode::Conflict);

        host.present = false;
        host.revision = 99;
        let execute_error = registry
            .execute(
                &id("app:demo"),
                &authorization,
                &GrantSnapshot::none(),
                &mut host,
            )
            .unwrap_err();
        assert_eq!(execute_error.code(), ErrorCode::Conflict);
        assert_eq!(host.probe_count(), probes, "replay must not probe");
        assert_eq!(host.performed.len(), 1);
    }

    #[test]
    fn discard_prepared_frees_capacity_and_fails_closed() {
        let mut registry = Registry::with_limits(
            CapabilityCatalog::builtin(),
            RegistryLimits {
                max_prepared_actions: 1,
                ..Default::default()
            },
        );
        registry.register(demo_spec()).unwrap();
        let host = FakeHost::new();
        let prepared = registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "one"),
                &host,
            )
            .unwrap();
        let action = prepared.action_ref();

        let full = registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "two"),
                &host,
            )
            .unwrap_err();
        assert!(matches!(
            full,
            ActionError::ResourceExhausted { ref resource, limit, .. }
                if resource == "prepared actions" && limit == 1
        ));
        assert_eq!(full.code(), ErrorCode::ResourceExhausted);

        assert!(registry.discard_prepared(action));
        assert!(!registry.discard_prepared(action));
        assert!(matches!(
            registry
                .authorize(
                    &prepared,
                    &grants_for(capability_names::LAUNCH_APPLICATION),
                    Confirmation::Confirmed,
                )
                .unwrap_err(),
            ActionError::UnknownReference { .. }
        ));

        registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "two"),
                &host,
            )
            .unwrap();
    }

    #[test]
    fn executed_actions_cannot_be_discarded() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        let (prepared, authorization, grants) =
            prepare_and_authorize(&mut registry, &host, capability_names::LAUNCH_APPLICATION);
        registry
            .execute(&id("app:demo"), &authorization, &grants, &mut host)
            .unwrap();
        assert!(!registry.discard_prepared(prepared.action_ref()));
    }

    #[test]
    fn prune_completed_operations_is_bounded_and_fails_closed() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        let mut authorizations = Vec::new();
        let mut operations = Vec::new();

        for query in ["one", "two", "three"] {
            let prepared = registry
                .prepare(
                    &id("app:demo"),
                    &ActionInput::new().with("query", query),
                    &host,
                )
                .unwrap();
            let authorization = registry
                .authorize(
                    &prepared,
                    &grants_for(capability_names::LAUNCH_APPLICATION),
                    Confirmation::Confirmed,
                )
                .unwrap();
            let outcome = registry
                .execute(
                    &id("app:demo"),
                    &authorization,
                    &grants_for(capability_names::LAUNCH_APPLICATION),
                    &mut host,
                )
                .unwrap();
            authorizations.push(authorization);
            operations.push(outcome.operation_id());
        }
        assert_eq!(host.performed.len(), 3);
        assert_eq!(registry.prune_completed_operations(1), 2);

        assert_eq!(
            registry.reconcile(operations[0], &mut host),
            ReconcileOutcome::Unknown
        );
        assert_eq!(
            registry.reconcile(operations[2], &mut host),
            ReconcileOutcome::Completed
        );
        let error = registry
            .execute(
                &id("app:demo"),
                &authorizations[0],
                &grants_for(capability_names::LAUNCH_APPLICATION),
                &mut host,
            )
            .unwrap_err();
        assert!(matches!(error, ActionError::UnknownReference { .. }));
        assert_eq!(host.performed.len(), 3);
        assert_eq!(registry.prune_completed_operations(1), 0);
    }

    #[test]
    fn pruning_never_removes_uncertain_operations() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        registry.register(quicklink_spec()).unwrap();
        let mut host = FakeHost::new();

        let quicklink = registry
            .prepare(&id("quicklink:docs"), &ActionInput::new(), &host)
            .unwrap();
        let quicklink_authorization = registry
            .authorize(
                &quicklink,
                &grants_for(capability_names::OPEN_URL),
                Confirmation::Confirmed,
            )
            .unwrap();
        registry
            .execute(
                &id("quicklink:docs"),
                &quicklink_authorization,
                &grants_for(capability_names::OPEN_URL),
                &mut host,
            )
            .unwrap();

        host.report = HostReport::Uncertain {
            reason: "unknown".into(),
        };
        let demo = registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "find"),
                &host,
            )
            .unwrap();
        let demo_authorization = registry
            .authorize(
                &demo,
                &grants_for(capability_names::LAUNCH_APPLICATION),
                Confirmation::Confirmed,
            )
            .unwrap();
        let uncertain = registry
            .execute(
                &id("app:demo"),
                &demo_authorization,
                &grants_for(capability_names::LAUNCH_APPLICATION),
                &mut host,
            )
            .unwrap()
            .operation_id();

        assert_eq!(registry.prune_completed_operations(0), 1);
        host.reconcile = HostReconcile::Completed;
        assert_eq!(
            registry.reconcile(uncertain, &mut host),
            ReconcileOutcome::Completed
        );
        registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "after"),
                &host,
            )
            .unwrap();
    }

    #[test]
    fn sequence_exhaustion_fails_closed_before_host_effect() {
        let mut registry = Registry::new();
        registry.register(demo_spec()).unwrap();
        let mut host = FakeHost::new();
        let (_, authorization, grants) =
            prepare_and_authorize(&mut registry, &host, capability_names::LAUNCH_APPLICATION);

        registry.next_sequence = u64::MAX;
        let execute_error = registry
            .execute(&id("app:demo"), &authorization, &grants, &mut host)
            .unwrap_err();
        assert!(matches!(
            execute_error,
            ActionError::ResourceExhausted { ref resource, .. }
                if resource == "action sequence"
        ));
        assert!(host.performed.is_empty());

        let prepare_error = registry
            .prepare(
                &id("app:demo"),
                &ActionInput::new().with("query", "again"),
                &host,
            )
            .unwrap_err();
        assert_eq!(prepare_error.code(), ErrorCode::ResourceExhausted);
        assert!(host.performed.is_empty());
    }

    #[test]
    fn host_reports_map_to_stable_codes() {
        let cases = [
            (
                HostReport::Unsupported {
                    capability: "vendor.runner".into(),
                },
                ErrorCode::UnsupportedCapability,
            ),
            (
                HostReport::Missing {
                    target: "gone".into(),
                },
                ErrorCode::MissingTarget,
            ),
            (
                HostReport::Conflict {
                    reason: "changed".into(),
                },
                ErrorCode::Conflict,
            ),
            (
                HostReport::Cancelled {
                    reason: "user".into(),
                },
                ErrorCode::Cancelled,
            ),
        ];

        for (report, expected) in cases {
            let mut registry = Registry::new();
            registry.register(demo_spec()).unwrap();
            let mut host = FakeHost::new();
            host.report = report;
            let (_, authorization, grants) =
                prepare_and_authorize(&mut registry, &host, capability_names::LAUNCH_APPLICATION);
            let error = registry
                .execute(&id("app:demo"), &authorization, &grants, &mut host)
                .unwrap_err();
            assert_eq!(error.code(), expected);
        }
    }

    #[test]
    fn registration_id_fuzz_is_atomic() {
        let alphabet: Vec<char> = "abZ09:._- \u{200B}\u{202E}\0\t".chars().collect();
        let mut state = 0x0f1e_2d3c_4b5a_6978_u64;
        for _ in 0..256 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let length = (state as usize) % 12;
            let mut raw = String::new();
            for _ in 0..length {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                raw.push(alphabet[(state as usize) % alphabet.len()]);
            }

            let mut registry = Registry::new();
            match registry.register_raw(
                &raw,
                "Fuzz",
                CommandKind::Quicklink {
                    url: "https://example.com".into(),
                },
            ) {
                Ok(()) => assert_eq!(registry.len(), 1),
                Err(_) => assert!(registry.is_empty()),
            }
        }
    }

    #[test]
    fn registration_fuzz_is_atomic_and_names_the_id() {
        let alphabet: Vec<char> = "ab Z0-_./\\\0\t\n\u{1f600}".chars().collect();
        let mut state = 0x1234_5678_9abc_def0_u64;
        for _ in 0..256 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let title_len = (state as usize) % 8;
            let mut title = String::new();
            for _ in 0..title_len {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                title.push(alphabet[(state as usize) % alphabet.len()]);
            }

            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let path_len = (state as usize) % 8;
            let mut path = String::new();
            for _ in 0..path_len {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                path.push(alphabet[(state as usize) % alphabet.len()]);
            }

            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let name_len = (state as usize) % 8;
            let mut name = String::new();
            for _ in 0..name_len {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                name.push(alphabet[(state as usize) % alphabet.len()]);
            }

            let spec = CommandSpec::new(id("app:fuzz"), title, CommandKind::Application { path })
                .with_parameters(
                    ParameterSchema::new()
                        .with_parameter(Parameter::optional(name, ParameterType::Text)),
                );

            let mut registry = Registry::new();
            match registry.register(spec) {
                Ok(()) => assert_eq!(registry.len(), 1),
                Err(error) => {
                    assert!(error.to_string().contains("app:fuzz"));
                    assert!(registry.is_empty());
                }
            }
        }
    }
}
