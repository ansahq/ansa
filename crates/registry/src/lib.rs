//! Typed command registry and action lifecycle for Ansa.
//!
//! # Authority model
//!
//! The registry is the single execution path for built-in, application,
//! quicklink, script and extension commands. It deliberately separates *data*
//! from *authority*:
//!
//! * A [`CommandSpec`] is inert data: a stable
//!   [`CommandId`](ansa_domain::CommandId), a title,
//!   keywords, a typed [`ParameterSchema`], required capability names and a
//!   [`CommandKind`]. No command stores a callback or a shell string.
//! * Registration enforces policy rather than trusting self-declaration. Every
//!   application, quicklink, script and extension kind has a minimum capability
//!   which is unioned into the spec's requirements, and every non-builtin kind
//!   is consequential and requires confirmation. [`Registry::register_untrusted`]
//!   additionally enforces provider namespace ownership and forbids
//!   [`CommandKind::Builtin`]; [`Registry::register`] is the trusted first-party
//!   API.
//! * [`Registry::prepare`] validates raw input and asks the [`Host`] to resolve
//!   the concrete target. It returns an [`Effect`] summary and an opaque
//!   [`ActionRef`] bound to the command, the typed parameters and the target
//!   revision observed at preparation time.
//! * [`Registry::authorize`] checks a [`GrantSnapshot`] and explicit
//!   confirmation, then binds the authorization to the snapshot's revision. It
//!   returns an [`Authorization`].
//! * [`Registry::execute`] re-checks the action binding, grant-snapshot
//!   revision, target revision, target existence, confirmation and current
//!   grants immediately before asking the host to perform the operation. It
//!   never retries.
//! * [`Registry::reconcile`] resolves an operation previously left
//!   [`ExecutionStatus::Uncertain`] (or confirms a completed/failed one). An
//!   unresolved uncertain operation blocks fresh preparations of its command,
//!   not merely replay of the exact reference.
//!
//! Opaque references are capabilities, not authentication. Possession of an
//! [`ActionRef`] or an [`Authorization`] does not prove that the user consented
//! or that a permission is still granted: the registry re-checks every guard at
//! execution time. Both types are scoped to the [`Registry`] instance that
//! created them and cannot be constructed or forged by callers. Display text
//! is never used as identity.
//!
//! A [`GrantSnapshot`] must come from trusted host permission state. The
//! registry is not a permission store and cannot authenticate a caller-supplied
//! snapshot; revision binding only catches an accidentally stale snapshot.
//! Snapshots constructed at revision zero (`none`/`from_capabilities`) provide
//! no staleness detection at all. Likewise, preparing the same command twice
//! before any execution creates two distinct intents: there is no implicit
//! idempotency key.
//!
//! [`Registry::register_untrusted`] is a namespace convention, not provider
//! authentication. Its `provider_namespace` argument is caller-supplied, a
//! compromised in-process caller can use the trusted [`Registry::register`] API
//! directly, and target ownership (`extension_id`, `script_id`, paths) is not
//! validated against the provider. The host must authenticate providers, keep
//! trusted registration out of provider reach, validate target ownership, and
//! bound the catalog it chooses to accept.
//!
//! # Example
//!
//! ```
//! use ansa_domain::CommandId;
//! use ansa_registry::{
//!     ActionInput, CommandKind, CommandSpec, Confirmation, GrantSnapshot, Host, HostReport,
//!     HostReconcile, ReconcileRequest, Registry, TargetProbe, capability_names,
//!     ExecutionRequest,
//! };
//!
//! struct ExampleHost;
//!
//! impl Host for ExampleHost {
//!     fn probe_target(&self, _spec: &CommandSpec) -> TargetProbe {
//!         TargetProbe::Present {
//!             identity: "https://example.com".into(),
//!             revision: 7,
//!         }
//!     }
//!
//!     fn perform(&mut self, _request: ExecutionRequest) -> HostReport {
//!         HostReport::Completed
//!     }
//!
//!     fn reconcile(&mut self, _request: ReconcileRequest) -> HostReconcile {
//!         HostReconcile::Completed
//!     }
//! }
//!
//! let mut registry = Registry::new();
//! registry
//!     .register(
//!         CommandSpec::new(
//!             CommandId::parse("quicklink:example").unwrap(),
//!             "Example",
//!             CommandKind::Quicklink {
//!                 url: "https://example.com".into(),
//!             },
//!         )
//!         .requiring(capability_names::OPEN_URL),
//!     )
//!     .unwrap();
//!
//! let id = CommandId::parse("quicklink:example").unwrap();
//! let mut grants = GrantSnapshot::none();
//! grants.grant(capability_names::OPEN_URL);
//!
//! let mut host = ExampleHost;
//! let prepared = registry
//!     .prepare(&id, &ActionInput::new(), &host)
//!     .unwrap();
//! let authorization = registry
//!     .authorize(&prepared, &grants, Confirmation::Confirmed)
//!     .unwrap();
//! let outcome = registry
//!     .execute(&id, &authorization, &grants, &mut host)
//!     .unwrap();
//! assert!(outcome.is_completed());
//! ```

#![forbid(unsafe_code)]
#![deny(missing_debug_implementations)]
#![deny(missing_docs)]

mod command;
mod error;
mod execution;
mod grant;
mod registry;

pub use command::{
    ActionInput, CommandKind, CommandSpec, MAX_CHOICE_OPTIONS, MAX_COMMAND_ID_CHARS,
    MAX_KEYWORD_CHARS, MAX_KEYWORDS, MAX_PARAMETER_NAME_CHARS, MAX_PARAMETER_VALUE_CHARS,
    MAX_PARAMETERS_PER_COMMAND, MAX_TITLE_CHARS, Parameter, ParameterSchema, ParameterType,
    ParameterValue, ParameterValues,
};
pub use error::{ActionError, ErrorCode, RegistrationError};
pub use execution::{
    ActionRef, Authorization, Effect, EffectTarget, ExecutionOutcome, ExecutionRequest,
    ExecutionStatus, Host, HostReconcile, HostReport, OperationId, PreparedAction,
    ReconcileOutcome, ReconcileRequest, ResolvedTarget, TargetProbe,
};
pub use grant::{
    CapabilityCatalog, Confirmation, GrantRevisionExhausted, GrantSnapshot, capability_names,
};
pub use registry::{
    DEFAULT_MAX_PREPARED_ACTIONS, DEFAULT_MAX_TERMINAL_OPERATIONS, Registry, RegistryExhausted,
    RegistryLimits,
};
