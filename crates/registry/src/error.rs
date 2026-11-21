//! Stable error taxonomy for action execution and registration.

use std::error::Error;
use std::fmt;

use ansa_domain::{CommandId, CommandIdError};

use crate::execution::OperationId;

/// Stable, user-visible category for an action failure.
///
/// The string forms returned by [`ErrorCode::as_str`] are part of the public
/// contract: logs, analytics and UI mappings may rely on them. [`ErrorCode::ALL`]
/// is the complete table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ErrorCode {
    /// Input was missing, malformed, unknown or out of bounds.
    InvalidInput,
    /// A required capability grant is absent.
    DeniedPermission,
    /// The command or its resolved target no longer exists.
    MissingTarget,
    /// The host cannot perform this kind on this system.
    UnsupportedCapability,
    /// The user or host cancelled before a definite effect.
    Cancelled,
    /// State changed between preparation and execution.
    Conflict,
    /// The host could not determine whether the effect occurred.
    Uncertain,
    /// A bounded registry resource, such as prepared actions or the operation
    /// sequence, is exhausted.
    ResourceExhausted,
}

impl ErrorCode {
    /// Every stable error code, in table order.
    pub const ALL: [Self; 8] = [
        Self::InvalidInput,
        Self::DeniedPermission,
        Self::MissingTarget,
        Self::UnsupportedCapability,
        Self::Cancelled,
        Self::Conflict,
        Self::Uncertain,
        Self::ResourceExhausted,
    ];

    /// The stable snake-case code used by logs and UI mappings.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::DeniedPermission => "denied_permission",
            Self::MissingTarget => "missing_target",
            Self::UnsupportedCapability => "unsupported_capability",
            Self::Cancelled => "cancelled",
            Self::Conflict => "conflict",
            Self::Uncertain => "uncertain",
            Self::ResourceExhausted => "resource_exhausted",
        }
    }

    /// One sentence explaining the category.
    pub const fn description(self) -> &'static str {
        match self {
            Self::InvalidInput => "The supplied input is not valid for this command.",
            Self::DeniedPermission => "A required permission is not granted.",
            Self::MissingTarget => "The command or target no longer exists.",
            Self::UnsupportedCapability => "This action is not supported by the host.",
            Self::Cancelled => "The action was cancelled before a definite effect.",
            Self::Conflict => "The target changed after the action was prepared.",
            Self::Uncertain => "The host could not determine whether the action took effect.",
            Self::ResourceExhausted => "A bounded registry resource is exhausted.",
        }
    }

    /// User-facing remedy text. This text is deliberately stable so UI can
    /// present it without translating internal errors.
    pub const fn remedy(self) -> &'static str {
        match self {
            Self::InvalidInput => "Check the value for the highlighted field and try again.",
            Self::DeniedPermission => "Grant the required permission in Settings, then try again.",
            Self::MissingTarget => "The item is no longer available. Refresh and choose another.",
            Self::UnsupportedCapability => "This action is not supported on this system.",
            Self::Cancelled => "The action was cancelled. Nothing was changed.",
            Self::Conflict => "The item changed while you were deciding. Reopen it and try again.",
            Self::Uncertain => {
                "Ansa cannot confirm whether the action finished. Reconcile before retrying."
            }
            Self::ResourceExhausted => {
                "The launcher has reached a safety limit. Restart it and try again."
            }
        }
    }
}

/// A typed, stable failure from preparing, authorizing or executing an action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionError {
    /// Raw input failed schema validation. This variant is reserved for
    /// parameter binding; reference and lifecycle failures use
    /// [`Self::UnknownReference`], [`Self::Conflict`] or [`Self::Uncertain`].
    InvalidInput {
        /// Command whose input was rejected.
        command: CommandId,
        /// Offending parameter name.
        field: String,
        /// Why the value was rejected.
        reason: String,
    },
    /// A capability required by the command is not granted.
    DeniedPermission {
        /// Command that needed the capability.
        command: CommandId,
        /// Missing capability name.
        capability: String,
        /// Why the grant was considered absent.
        reason: String,
    },
    /// The command is not registered, or its resolved target disappeared.
    MissingTarget {
        /// Command being prepared or executed.
        command: CommandId,
        /// Stable target identity, or the command identity for built-ins.
        target: String,
    },
    /// An opaque action reference is not known to this registry. It is foreign,
    /// discarded, or otherwise no longer resolvable.
    UnknownReference {
        /// Command the caller attempted to use it with.
        command: CommandId,
        /// Diagnostic form of the reference.
        reference: String,
    },
    /// The host cannot perform this command kind.
    UnsupportedCapability {
        /// Command that is unsupported.
        command: CommandId,
        /// Capability the host cannot provide.
        capability: String,
    },
    /// Preparation or authorization was cancelled, typically because explicit
    /// confirmation was not supplied.
    Cancelled {
        /// Command that was cancelled.
        command: CommandId,
        /// Why cancellation happened.
        reason: String,
    },
    /// The target or operation state changed since the action was prepared.
    Conflict {
        /// Command with the conflict.
        command: CommandId,
        /// What changed.
        reason: String,
    },
    /// An earlier attempt has an unknown outcome. The caller must reconcile
    /// instead of retrying automatically.
    Uncertain {
        /// Command with the uncertain operation.
        command: CommandId,
        /// Operation to reconcile.
        operation: OperationId,
        /// Why the operation is uncertain.
        reason: String,
    },
    /// A bounded registry resource cannot allocate another identity or record.
    ResourceExhausted {
        /// Command being prepared or executed.
        command: CommandId,
        /// Resource name, such as `prepared actions` or `operation sequence`.
        resource: String,
        /// Limit that was reached.
        limit: u64,
    },
}

impl ActionError {
    /// The stable category of this error.
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidInput { .. } => ErrorCode::InvalidInput,
            Self::DeniedPermission { .. } => ErrorCode::DeniedPermission,
            Self::MissingTarget { .. } => ErrorCode::MissingTarget,
            Self::UnknownReference { .. } => ErrorCode::MissingTarget,
            Self::UnsupportedCapability { .. } => ErrorCode::UnsupportedCapability,
            Self::Cancelled { .. } => ErrorCode::Cancelled,
            Self::Conflict { .. } => ErrorCode::Conflict,
            Self::Uncertain { .. } => ErrorCode::Uncertain,
            Self::ResourceExhausted { .. } => ErrorCode::ResourceExhausted,
        }
    }

    /// The command this error belongs to.
    pub fn command_id(&self) -> &CommandId {
        match self {
            Self::InvalidInput { command, .. }
            | Self::DeniedPermission { command, .. }
            | Self::MissingTarget { command, .. }
            | Self::UnknownReference { command, .. }
            | Self::UnsupportedCapability { command, .. }
            | Self::Cancelled { command, .. }
            | Self::Conflict { command, .. }
            | Self::Uncertain { command, .. }
            | Self::ResourceExhausted { command, .. } => command,
        }
    }

    /// The stable user-facing remedy for this error's category.
    pub fn remedy(&self) -> &'static str {
        self.code().remedy()
    }
}

impl fmt::Display for ActionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput {
                command,
                field,
                reason,
            } => write!(
                f,
                "command '{command}' parameter '{field}' is invalid: {reason}"
            ),
            Self::DeniedPermission {
                command,
                capability,
                reason,
            } => write!(
                f,
                "command '{command}' requires capability '{capability}': {reason}"
            ),
            Self::MissingTarget { command, target } => {
                write!(f, "command '{command}': target '{target}' no longer exists")
            }
            Self::UnknownReference { command, reference } => write!(
                f,
                "command '{command}' references unknown action '{reference}'"
            ),
            Self::UnsupportedCapability {
                command,
                capability,
            } => write!(
                f,
                "command '{command}' is unsupported: the host cannot provide '{capability}'"
            ),
            Self::Cancelled { command, reason } => {
                write!(f, "command '{command}' was cancelled: {reason}")
            }
            Self::Conflict { command, reason } => {
                write!(
                    f,
                    "command '{command}' conflicts with current state: {reason}"
                )
            }
            Self::Uncertain {
                command,
                operation,
                reason,
            } => write!(
                f,
                "command '{command}' operation {operation} is uncertain: {reason}"
            ),
            Self::ResourceExhausted {
                command,
                resource,
                limit,
            } => write!(
                f,
                "command '{command}' exhausted {resource} (limit {limit})"
            ),
        }
    }
}

impl Error for ActionError {}

/// A rejected command registration.
///
/// Every variant names the offending command identity and, where relevant, the
/// offending field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationError {
    /// A raw identifier was not a valid namespaced [`CommandId`].
    InvalidCommandId {
        /// Raw identity exactly as supplied.
        raw: String,
        /// Parser diagnostics.
        reason: CommandIdError,
    },
    /// The identity is already registered.
    DuplicateCommand {
        /// Duplicate identity.
        id: CommandId,
    },
    /// The identity contains an invisible format character that would make it
    /// visually ambiguous.
    InvalidCommandIdCharacter {
        /// Command with the bad identity.
        id: CommandId,
        /// Offending character.
        character: char,
        /// Character position in the identity.
        position: usize,
    },
    /// The identity exceeds the registry's length bound.
    CommandIdTooLong {
        /// Command with the bad identity.
        id: CommandId,
        /// Maximum accepted characters.
        limit: usize,
    },
    /// An untrusted provider namespace exceeds the registry's length bound.
    ProviderNamespaceTooLong {
        /// Namespace as supplied by the caller.
        namespace: String,
        /// Maximum accepted characters.
        limit: usize,
    },
    /// The identity does not belong to the namespace a provider claims.
    NamespaceMismatch {
        /// Command with the mismatched identity.
        id: CommandId,
        /// Namespace the provider claimed.
        expected: String,
        /// Namespace encoded in the identity.
        actual: String,
    },
    /// An untrusted provider claimed a namespace reserved for first-party use.
    ReservedNamespace {
        /// Command attempted by the provider.
        id: CommandId,
        /// Reserved namespace.
        namespace: String,
    },
    /// An untrusted provider attempted a kind reserved for first-party use.
    ForbiddenKind {
        /// Command attempted by the provider.
        id: CommandId,
        /// Why the kind is forbidden.
        reason: String,
    },
    /// The title was empty or whitespace only.
    EmptyTitle {
        /// Command with the empty title.
        id: CommandId,
    },
    /// The title or a keyword is too long or contains control characters.
    InvalidDisplayText {
        /// Command with the bad text.
        id: CommandId,
        /// Offending field, such as `title` or `keyword 2`.
        field: String,
        /// Why the text was rejected.
        reason: String,
    },
    /// The command kind carried structurally invalid target data.
    InvalidKind {
        /// Command with the invalid kind.
        id: CommandId,
        /// What is wrong.
        reason: String,
    },
    /// A command declares more parameters than the registry accepts.
    TooManyParameters {
        /// Command with too many parameters.
        id: CommandId,
        /// Maximum accepted count.
        limit: usize,
    },
    /// Two parameters share a name.
    DuplicateParameter {
        /// Command with the duplicate.
        id: CommandId,
        /// Duplicated parameter name.
        parameter: String,
    },
    /// A parameter name is empty or malformed.
    InvalidParameterName {
        /// Command with the bad name.
        id: CommandId,
        /// Offending name, possibly truncated for diagnostics.
        parameter: String,
        /// Why the name was rejected.
        reason: String,
    },
    /// A parameter type declaration is invalid, such as an empty choice list.
    InvalidParameterType {
        /// Command with the bad type.
        id: CommandId,
        /// Parameter with the bad type.
        parameter: String,
        /// Why the type was rejected.
        reason: String,
    },
    /// A parameter default does not match its declared type or domain.
    InvalidDefault {
        /// Command with the bad default.
        id: CommandId,
        /// Parameter with the bad default.
        parameter: String,
        /// Why the default was rejected.
        reason: String,
    },
    /// A required capability name was empty.
    EmptyCapabilityName {
        /// Command with the bad requirement.
        id: CommandId,
    },
    /// A capability requirement is listed twice.
    DuplicateCapability {
        /// Command with the duplicate.
        id: CommandId,
        /// Duplicated capability name.
        capability: String,
    },
    /// A capability requirement is not present in the registry catalog.
    UnknownCapability {
        /// Command with the unknown requirement.
        id: CommandId,
        /// Unknown capability name.
        capability: String,
    },
}

impl fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCommandId { raw, reason } => {
                write!(f, "command id {raw:?} is malformed: {reason}")
            }
            Self::DuplicateCommand { id } => {
                write!(f, "command id '{id}' is already registered")
            }
            Self::InvalidCommandIdCharacter {
                id,
                character,
                position,
            } => write!(
                f,
                "command id '{id}' contains invisible format character {character:?} at position {position}"
            ),
            Self::CommandIdTooLong { id, limit } => {
                write!(f, "command id '{id}' exceeds {limit} characters")
            }
            Self::ProviderNamespaceTooLong { namespace, limit } => write!(
                f,
                "provider namespace {namespace:?} exceeds {limit} characters"
            ),
            Self::NamespaceMismatch {
                id,
                expected,
                actual,
            } => write!(
                f,
                "command '{id}' namespace '{actual}' does not match provider namespace '{expected}'"
            ),
            Self::ReservedNamespace { id, namespace } => write!(
                f,
                "command '{id}' cannot be registered by an untrusted provider: namespace '{namespace}' is reserved for first-party commands"
            ),
            Self::ForbiddenKind { id, reason } => {
                write!(f, "command '{id}' kind is forbidden: {reason}")
            }
            Self::EmptyTitle { id } => write!(f, "command '{id}' has an empty title"),
            Self::InvalidDisplayText { id, field, reason } => {
                write!(f, "command '{id}' {field} is invalid: {reason}")
            }
            Self::InvalidKind { id, reason } => {
                write!(f, "command '{id}' has an invalid kind: {reason}")
            }
            Self::TooManyParameters { id, limit } => {
                write!(f, "command '{id}' declares more than {limit} parameters")
            }
            Self::DuplicateParameter { id, parameter } => write!(
                f,
                "command '{id}' parameter '{parameter}': duplicate parameter name"
            ),
            Self::InvalidParameterName {
                id,
                parameter,
                reason,
            } => write!(f, "command '{id}' parameter '{parameter}': {reason}"),
            Self::InvalidParameterType {
                id,
                parameter,
                reason,
            } => write!(
                f,
                "command '{id}' parameter '{parameter}' type is invalid: {reason}"
            ),
            Self::InvalidDefault {
                id,
                parameter,
                reason,
            } => write!(
                f,
                "command '{id}' parameter '{parameter}' default is invalid: {reason}"
            ),
            Self::EmptyCapabilityName { id } => {
                write!(f, "command '{id}' declares an empty capability name")
            }
            Self::DuplicateCapability { id, capability } => {
                write!(
                    f,
                    "command '{id}' capability '{capability}': duplicate requirement"
                )
            }
            Self::UnknownCapability { id, capability } => {
                write!(
                    f,
                    "command '{id}' capability '{capability}': unknown capability"
                )
            }
        }
    }
}

impl Error for RegistrationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidCommandId { reason, .. } => Some(reason),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> CommandId {
        CommandId::parse("builtin:settings").unwrap()
    }

    #[test]
    fn error_maptable_is_stable_and_complete() {
        let expected = [
            (
                "invalid_input",
                "Check the value for the highlighted field and try again.",
            ),
            (
                "denied_permission",
                "Grant the required permission in Settings, then try again.",
            ),
            (
                "missing_target",
                "The item is no longer available. Refresh and choose another.",
            ),
            (
                "unsupported_capability",
                "This action is not supported on this system.",
            ),
            (
                "cancelled",
                "The action was cancelled. Nothing was changed.",
            ),
            (
                "conflict",
                "The item changed while you were deciding. Reopen it and try again.",
            ),
            (
                "uncertain",
                "Ansa cannot confirm whether the action finished. Reconcile before retrying.",
            ),
            (
                "resource_exhausted",
                "The launcher has reached a safety limit. Restart it and try again.",
            ),
        ];
        assert_eq!(ErrorCode::ALL.len(), expected.len());
        for (code, (text, remedy)) in ErrorCode::ALL.into_iter().zip(expected) {
            assert_eq!(code.as_str(), text);
            assert_eq!(code.remedy(), remedy);
            assert!(!code.description().is_empty());
        }
    }

    #[test]
    fn errors_map_to_their_code_and_remedy() {
        let cases = [
            (
                ActionError::InvalidInput {
                    command: id(),
                    field: "query".into(),
                    reason: "bad".into(),
                },
                ErrorCode::InvalidInput,
            ),
            (
                ActionError::DeniedPermission {
                    command: id(),
                    capability: "cap".into(),
                    reason: "missing".into(),
                },
                ErrorCode::DeniedPermission,
            ),
            (
                ActionError::MissingTarget {
                    command: id(),
                    target: "x".into(),
                },
                ErrorCode::MissingTarget,
            ),
            (
                ActionError::UnknownReference {
                    command: id(),
                    reference: "action".into(),
                },
                ErrorCode::MissingTarget,
            ),
            (
                ActionError::UnsupportedCapability {
                    command: id(),
                    capability: "cap".into(),
                },
                ErrorCode::UnsupportedCapability,
            ),
            (
                ActionError::Cancelled {
                    command: id(),
                    reason: "no".into(),
                },
                ErrorCode::Cancelled,
            ),
            (
                ActionError::Conflict {
                    command: id(),
                    reason: "changed".into(),
                },
                ErrorCode::Conflict,
            ),
            (
                ActionError::Uncertain {
                    command: id(),
                    operation: OperationId::new(1, 1),
                    reason: "unknown".into(),
                },
                ErrorCode::Uncertain,
            ),
            (
                ActionError::ResourceExhausted {
                    command: id(),
                    resource: "prepared actions".into(),
                    limit: 1,
                },
                ErrorCode::ResourceExhausted,
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(error.code(), expected);
            assert_eq!(error.remedy(), expected.remedy());
            assert_eq!(error.command_id(), &id());
            assert!(!error.to_string().is_empty());
        }
    }

    #[test]
    fn registration_errors_name_the_identity_and_field() {
        let error = RegistrationError::DuplicateParameter {
            id: id(),
            parameter: "query".into(),
        };
        let text = error.to_string();
        assert!(text.contains("builtin:settings"));
        assert!(text.contains("query"));

        let malformed = RegistrationError::InvalidCommandId {
            raw: "settings".into(),
            reason: CommandIdError::MissingNamespaceSeparator,
        };
        assert!(malformed.to_string().contains("settings"));
        assert!(malformed.source().is_some());
    }
}
