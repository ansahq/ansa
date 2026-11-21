//! Command descriptors and typed parameter schemas.
//!
//! Everything in this module is data. A command never owns a callback, and a
//! parameter value is parsed into a typed [`ParameterValue`] before an action
//! can be prepared.

use std::collections::{BTreeMap, BTreeSet};

use ansa_domain::CommandId;

use crate::error::{ActionError, RegistrationError};
use crate::grant::capability_names;

/// Maximum number of Unicode scalar values accepted in one parameter value.
pub const MAX_PARAMETER_VALUE_CHARS: usize = 4096;

/// Maximum number of parameters a command may declare.
pub const MAX_PARAMETERS_PER_COMMAND: usize = 64;

/// Maximum number of options a choice parameter may declare.
pub const MAX_CHOICE_OPTIONS: usize = 64;

/// Maximum number of Unicode scalar values accepted in a parameter name.
pub const MAX_PARAMETER_NAME_CHARS: usize = 64;

/// Maximum number of Unicode scalar values accepted in a command title.
pub const MAX_TITLE_CHARS: usize = 256;

/// Maximum number of keywords a command may declare.
pub const MAX_KEYWORDS: usize = 64;

/// Maximum number of Unicode scalar values accepted in one keyword.
pub const MAX_KEYWORD_CHARS: usize = 64;

/// Maximum number of Unicode scalar values accepted in a command identity or
/// untrusted provider namespace.
pub const MAX_COMMAND_ID_CHARS: usize = 512;

/// How a registered command is executed.
///
/// Kinds carry inert data only. The platform host decides how to perform the
/// operation; no variant contains a shell command string or a closure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandKind {
    /// A reviewed first-party behavior owned by the launcher.
    Builtin,
    /// Launch a desktop application at `path`.
    Application {
        /// Resolved or resolvable application path.
        path: String,
    },
    /// Open a URL with the host's preferred opener.
    Quicklink {
        /// URL to open.
        url: String,
    },
    /// Run a reviewed script identified by `script_id`.
    ///
    /// The identifier is resolved by the host. It is not a shell string.
    Script {
        /// Stable identifier of the reviewed script.
        script_id: String,
    },
    /// Invoke one action exposed by a trusted extension instance.
    Extension {
        /// Stable extension instance identity.
        extension_id: String,
        /// Action name scoped to that extension.
        action: String,
    },
}

impl CommandKind {
    /// A stable, UI-safe category label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Builtin => "Built-in",
            Self::Application { .. } => "Application",
            Self::Quicklink { .. } => "Quicklink",
            Self::Script { .. } => "Script",
            Self::Extension { .. } => "Extension",
        }
    }

    /// Whether performing this kind requires explicit confirmation.
    ///
    /// Every kind that can reach an external program or an OS opener is
    /// consequential: applications, quicklinks, scripts and extension actions.
    /// Only reviewed first-party [`Self::Builtin`] behavior remains
    /// non-consequential. The registry enforces this policy at authorization
    /// time for every registered command.
    pub fn is_consequential(&self) -> bool {
        !matches!(self, Self::Builtin)
    }

    /// The minimum capability this kind requires.
    ///
    /// Registration unions this capability with any spec-declared
    /// capabilities, so an under-declared application, quicklink, script or
    /// extension cannot execute with an empty grant snapshot. Built-ins have no
    /// kind-level capability and rely on their registered behavior.
    pub fn required_capability(&self) -> Option<&'static str> {
        match self {
            Self::Builtin => None,
            Self::Application { .. } => Some(capability_names::LAUNCH_APPLICATION),
            Self::Quicklink { .. } => Some(capability_names::OPEN_URL),
            Self::Script { .. } => Some(capability_names::RUN_SCRIPT),
            Self::Extension { .. } => Some(capability_names::INVOKE_EXTENSION),
        }
    }

    /// The stable target identity carried by this kind, if any.
    ///
    /// Built-ins have no separate target: their target is the command itself.
    /// Targets are identity strings, not display text: registration rejects
    /// control characters and all invisible format characters, including
    /// joiners and variation selectors, in every target component.
    pub fn target_identity(&self) -> Option<String> {
        match self {
            Self::Builtin => None,
            Self::Application { path } => Some(path.clone()),
            Self::Quicklink { url } => Some(url.clone()),
            Self::Script { script_id } => Some(script_id.clone()),
            Self::Extension {
                extension_id,
                action,
            } => Some(format!("{extension_id}:{action}")),
        }
    }

    /// A short user-facing sentence describing the effect on `target`.
    pub fn effect_summary(&self, title: &str, target: &str) -> String {
        match self {
            Self::Builtin => format!("Run {title}"),
            Self::Application { .. } => format!("Launch application {target}"),
            Self::Quicklink { .. } => format!("Open {target}"),
            Self::Script { .. } => format!("Run script {target}"),
            Self::Extension { .. } => format!("Invoke extension action {target}"),
        }
    }

    /// Returns the first structural defect in this kind's target data.
    pub(crate) fn invalid_identity(&self) -> Option<&'static str> {
        match self {
            Self::Builtin => None,
            Self::Application { path } => validate_target(
                path,
                "application path must not be empty",
                "application path must not contain control or invisible format characters",
            ),
            Self::Quicklink { url } => validate_target(
                url,
                "quicklink URL must not be empty",
                "quicklink URL must not contain control or invisible format characters",
            ),
            Self::Script { script_id } => validate_target(
                script_id,
                "script identifier must not be empty",
                "script identifier must not contain control or invisible format characters",
            ),
            Self::Extension {
                extension_id,
                action,
            } => validate_target(
                extension_id,
                "extension identifier must not be empty",
                "extension identifier must not contain control or invisible format characters",
            )
            .or_else(|| {
                validate_target(
                    action,
                    "extension action must not be empty",
                    "extension action must not contain control or invisible format characters",
                )
            }),
        }
    }
}

fn validate_target(
    value: &str,
    empty_reason: &'static str,
    format_reason: &'static str,
) -> Option<&'static str> {
    if value.trim().is_empty() {
        return Some(empty_reason);
    }
    if value.chars().any(char::is_control) || first_invisible_identity_character(value).is_some() {
        return Some(format_reason);
    }
    None
}

/// Finds the first invisible format character unsuitable for a stable
/// identity.
pub(crate) fn first_invisible_identity_character(value: &str) -> Option<(char, usize)> {
    value
        .chars()
        .enumerate()
        .find(|(_, character)| is_identity_format_character(*character))
        .map(|(position, character)| (character, position))
}

/// Finds the first bidi/zero-width format character unsuitable for display
/// text or host targets.
///
/// This deliberately permits emoji joiners and variation selectors, which are
/// legitimate in human-readable titles. Command identities use the stricter
/// [`first_invisible_identity_character`] instead.
pub(crate) fn first_display_format_character(value: &str) -> Option<(char, usize)> {
    value
        .chars()
        .enumerate()
        .find(|(_, character)| is_display_format_character(*character))
        .map(|(position, character)| (character, position))
}

fn is_identity_format_character(character: char) -> bool {
    is_display_format_character(character)
        || matches!(
            character,
            '\u{00AD}'
                | '\u{180E}'
                | '\u{200C}'..='\u{200D}'
                | '\u{FE00}'..='\u{FE0F}'
                | '\u{E0100}'..='\u{E01EF}'
        )
}

fn is_display_format_character(character: char) -> bool {
    matches!(
        character,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'
            | '\u{200E}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{E0000}'..='\u{E007F}'
    )
}

/// The declared type of a command parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParameterType {
    /// Free-form text.
    Text,
    /// A finite floating-point number.
    Number,
    /// A boolean accepting `true` or `false` (case-insensitive).
    Boolean,
    /// One of a fixed set of exact option strings.
    Choice {
        /// Declared options. Must be non-empty, unique and non-blank.
        options: Vec<String>,
    },
    /// A filesystem path that the host resolves. The registry does not
    /// canonicalize or expand it.
    Path,
}

impl ParameterType {
    /// A stable name used in diagnostics.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Choice { .. } => "choice",
            Self::Path => "path",
        }
    }

    /// The declared options of a choice parameter.
    pub fn choice_options(&self) -> Option<&[String]> {
        match self {
            Self::Choice { options } => Some(options),
            _ => None,
        }
    }
}

/// A typed parameter value after validation.
#[derive(Debug, Clone, PartialEq)]
pub enum ParameterValue {
    /// A UTF-8 text value.
    Text(String),
    /// A finite number.
    Number(f64),
    /// A boolean.
    Boolean(bool),
    /// One of the parameter's declared options.
    Choice(String),
    /// A non-empty path string.
    Path(String),
}

impl ParameterValue {
    /// A stable type name used in diagnostics.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Text(_) => "text",
            Self::Number(_) => "number",
            Self::Boolean(_) => "boolean",
            Self::Choice(_) => "choice",
            Self::Path(_) => "path",
        }
    }

    /// The string form of text, path and choice values.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Text(value) | Self::Path(value) | Self::Choice(value) => Some(value),
            Self::Number(_) | Self::Boolean(_) => None,
        }
    }

    /// The numeric form of a number value.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(value) => Some(*value),
            _ => None,
        }
    }

    /// The boolean form of a boolean value.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Boolean(value) => Some(*value),
            _ => None,
        }
    }

    /// Whether this value has the same variant as `expected`.
    ///
    /// Choice values must also be one of the declared options. This checks
    /// shape only; finiteness and length are checked during registration and
    /// input binding.
    pub fn matches_type(&self, expected: &ParameterType) -> bool {
        match (self, expected) {
            (Self::Text(_), ParameterType::Text)
            | (Self::Number(_), ParameterType::Number)
            | (Self::Boolean(_), ParameterType::Boolean)
            | (Self::Path(_), ParameterType::Path) => true,
            (Self::Choice(value), ParameterType::Choice { options }) => {
                options.iter().any(|option| option == value)
            }
            _ => false,
        }
    }
}

/// One declared command parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct Parameter {
    name: String,
    required: bool,
    default: Option<ParameterValue>,
    value_type: ParameterType,
}

impl Parameter {
    /// Creates a required parameter with no default.
    pub fn required(name: impl Into<String>, value_type: ParameterType) -> Self {
        Self {
            name: name.into(),
            required: true,
            default: None,
            value_type,
        }
    }

    /// Creates an optional parameter with no default.
    pub fn optional(name: impl Into<String>, value_type: ParameterType) -> Self {
        Self {
            name: name.into(),
            required: false,
            default: None,
            value_type,
        }
    }

    /// Attaches a default value used when the parameter is absent or empty.
    pub fn with_default(mut self, default: ParameterValue) -> Self {
        self.default = Some(default);
        self
    }

    /// The stable parameter name used as identity in input and execution.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether a value must resolve (from input or a default) before prepare.
    pub fn is_required(&self) -> bool {
        self.required
    }

    /// The declared default, if any.
    pub fn default_value(&self) -> Option<&ParameterValue> {
        self.default.as_ref()
    }

    /// The declared type.
    pub fn value_type(&self) -> &ParameterType {
        &self.value_type
    }

    fn validate_default(&self, id: &CommandId) -> Result<(), RegistrationError> {
        let Some(default) = &self.default else {
            return Ok(());
        };
        if !default.matches_type(&self.value_type) {
            return Err(RegistrationError::InvalidDefault {
                id: id.clone(),
                parameter: self.name.clone(),
                reason: format!(
                    "default value is {}, expected {}",
                    default.type_name(),
                    self.value_type.label()
                ),
            });
        }

        match default {
            ParameterValue::Number(value) if !value.is_finite() => {
                return Err(RegistrationError::InvalidDefault {
                    id: id.clone(),
                    parameter: self.name.clone(),
                    reason: "default number must be finite".into(),
                });
            }
            ParameterValue::Text(value) | ParameterValue::Path(value) => {
                if value.is_empty() {
                    return Err(RegistrationError::InvalidDefault {
                        id: id.clone(),
                        parameter: self.name.clone(),
                        reason: "default value must not be empty".into(),
                    });
                }
                if value.chars().count() > MAX_PARAMETER_VALUE_CHARS {
                    return Err(RegistrationError::InvalidDefault {
                        id: id.clone(),
                        parameter: self.name.clone(),
                        reason: format!(
                            "default value exceeds {MAX_PARAMETER_VALUE_CHARS} characters"
                        ),
                    });
                }
                if value.chars().any(char::is_control) {
                    return Err(RegistrationError::InvalidDefault {
                        id: id.clone(),
                        parameter: self.name.clone(),
                        reason: "default value must not contain control characters".into(),
                    });
                }
                if first_display_format_character(value).is_some() {
                    return Err(RegistrationError::InvalidDefault {
                        id: id.clone(),
                        parameter: self.name.clone(),
                        reason: "default value must not contain invisible format characters".into(),
                    });
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn parse(&self, command: &CommandId, raw: &str) -> Result<ParameterValue, ActionError> {
        if raw.chars().count() > MAX_PARAMETER_VALUE_CHARS {
            return Err(invalid_input(
                command,
                &self.name,
                format!("value exceeds {MAX_PARAMETER_VALUE_CHARS} characters"),
            ));
        }
        if raw.chars().any(char::is_control) {
            return Err(invalid_input(
                command,
                &self.name,
                "value must not contain control characters",
            ));
        }

        match &self.value_type {
            ParameterType::Text => Ok(ParameterValue::Text(raw.to_owned())),
            ParameterType::Number => {
                let trimmed = raw.trim();
                let parsed = trimmed.parse::<f64>().map_err(|_| {
                    invalid_input(command, &self.name, "expected a finite decimal number")
                })?;
                if !parsed.is_finite() {
                    return Err(invalid_input(
                        command,
                        &self.name,
                        "expected a finite decimal number",
                    ));
                }
                Ok(ParameterValue::Number(parsed))
            }
            ParameterType::Boolean => {
                let trimmed = raw.trim();
                if trimmed.eq_ignore_ascii_case("true") {
                    Ok(ParameterValue::Boolean(true))
                } else if trimmed.eq_ignore_ascii_case("false") {
                    Ok(ParameterValue::Boolean(false))
                } else {
                    Err(invalid_input(
                        command,
                        &self.name,
                        "expected 'true' or 'false'",
                    ))
                }
            }
            ParameterType::Choice { options } => {
                if options.iter().any(|option| option == raw) {
                    Ok(ParameterValue::Choice(raw.to_owned()))
                } else {
                    Err(invalid_input(
                        command,
                        &self.name,
                        format!("expected one of: {}", options.join(", ")),
                    ))
                }
            }
            ParameterType::Path => {
                if raw.trim().is_empty() {
                    return Err(invalid_input(command, &self.name, "path must not be empty"));
                }
                Ok(ParameterValue::Path(raw.to_owned()))
            }
        }
    }
}

/// An ordered set of parameter declarations.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParameterSchema {
    parameters: Vec<Parameter>,
}

impl ParameterSchema {
    /// Creates an empty schema.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a parameter declaration.
    pub fn with_parameter(mut self, parameter: Parameter) -> Self {
        self.parameters.push(parameter);
        self
    }

    /// The declarations in their declared order.
    pub fn parameters(&self) -> &[Parameter] {
        &self.parameters
    }

    /// Number of declarations.
    pub fn len(&self) -> usize {
        self.parameters.len()
    }

    /// Whether no parameters are declared.
    pub fn is_empty(&self) -> bool {
        self.parameters.is_empty()
    }

    /// Finds a declaration by stable name.
    pub fn get(&self, name: &str) -> Option<&Parameter> {
        self.parameters
            .iter()
            .find(|parameter| parameter.name == name)
    }

    pub(crate) fn validate(&self, id: &CommandId) -> Result<(), RegistrationError> {
        if self.parameters.len() > MAX_PARAMETERS_PER_COMMAND {
            return Err(RegistrationError::TooManyParameters {
                id: id.clone(),
                limit: MAX_PARAMETERS_PER_COMMAND,
            });
        }

        let mut seen = BTreeSet::new();
        for parameter in &self.parameters {
            if parameter.name.is_empty() {
                return Err(RegistrationError::InvalidParameterName {
                    id: id.clone(),
                    parameter: String::new(),
                    reason: "parameter name must not be empty".into(),
                });
            }
            if parameter.name.chars().count() > MAX_PARAMETER_NAME_CHARS {
                return Err(RegistrationError::InvalidParameterName {
                    id: id.clone(),
                    parameter: parameter.name.chars().take(32).collect(),
                    reason: format!("parameter name exceeds {MAX_PARAMETER_NAME_CHARS} characters"),
                });
            }
            if parameter.name.chars().any(char::is_control) {
                return Err(RegistrationError::InvalidParameterName {
                    id: id.clone(),
                    parameter: parameter.name.clone(),
                    reason: "parameter name must not contain control characters".into(),
                });
            }
            if !is_valid_parameter_name(&parameter.name) {
                return Err(RegistrationError::InvalidParameterName {
                    id: id.clone(),
                    parameter: parameter.name.clone(),
                    reason:
                        "parameter name must start with an ASCII letter or '_' and contain only ASCII letters, digits, '_', '-' or '.'"
                            .into(),
                });
            }
            if !seen.insert(parameter.name.as_str()) {
                return Err(RegistrationError::DuplicateParameter {
                    id: id.clone(),
                    parameter: parameter.name.clone(),
                });
            }

            if let ParameterType::Choice { options } = &parameter.value_type {
                if options.is_empty() {
                    return Err(RegistrationError::InvalidParameterType {
                        id: id.clone(),
                        parameter: parameter.name.clone(),
                        reason: "choice parameter must declare at least one option".into(),
                    });
                }
                if options.len() > MAX_CHOICE_OPTIONS {
                    return Err(RegistrationError::InvalidParameterType {
                        id: id.clone(),
                        parameter: parameter.name.clone(),
                        reason: format!(
                            "choice parameter declares more than {MAX_CHOICE_OPTIONS} options"
                        ),
                    });
                }
                let mut option_set = BTreeSet::new();
                for option in options {
                    if option.trim().is_empty() {
                        return Err(RegistrationError::InvalidParameterType {
                            id: id.clone(),
                            parameter: parameter.name.clone(),
                            reason: "choice options must not be empty".into(),
                        });
                    }
                    if option.chars().any(char::is_control) {
                        return Err(RegistrationError::InvalidParameterType {
                            id: id.clone(),
                            parameter: parameter.name.clone(),
                            reason: "choice options must not contain control characters".into(),
                        });
                    }
                    if first_display_format_character(option).is_some() {
                        return Err(RegistrationError::InvalidParameterType {
                            id: id.clone(),
                            parameter: parameter.name.clone(),
                            reason: "choice options must not contain invisible format characters"
                                .into(),
                        });
                    }
                    if option.chars().count() > MAX_PARAMETER_VALUE_CHARS {
                        return Err(RegistrationError::InvalidParameterType {
                            id: id.clone(),
                            parameter: parameter.name.clone(),
                            reason: format!(
                                "choice option exceeds {MAX_PARAMETER_VALUE_CHARS} characters"
                            ),
                        });
                    }
                    if !option_set.insert(option.as_str()) {
                        return Err(RegistrationError::InvalidParameterType {
                            id: id.clone(),
                            parameter: parameter.name.clone(),
                            reason: format!("duplicate choice option '{option}'"),
                        });
                    }
                }
            }

            parameter.validate_default(id)?;
        }

        Ok(())
    }

    /// Validates raw input and produces typed values, applying defaults.
    ///
    /// Unknown names, malformed values, missing required values and oversized
    /// values are rejected. An optional value supplied as the empty string is
    /// treated as absent, so its default applies.
    pub(crate) fn bind(
        &self,
        command: &CommandId,
        input: &ActionInput,
    ) -> Result<ParameterValues, ActionError> {
        for name in input.values.keys() {
            if self.get(name).is_none() {
                return Err(invalid_input(
                    command,
                    name,
                    "unknown parameter for this command",
                ));
            }
        }

        let mut values = BTreeMap::new();
        for parameter in &self.parameters {
            let value = match input.get(&parameter.name) {
                Some(raw) if !raw.is_empty() => parameter.parse(command, raw)?,
                _ => {
                    if let Some(default) = &parameter.default {
                        default.clone()
                    } else if parameter.required {
                        return Err(invalid_input(
                            command,
                            &parameter.name,
                            "required value is missing",
                        ));
                    } else {
                        continue;
                    }
                }
            };
            values.insert(parameter.name.clone(), value);
        }

        Ok(ParameterValues { values })
    }
}

fn is_valid_parameter_name(name: &str) -> bool {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    characters
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.'))
}

fn invalid_input(command: &CommandId, field: &str, reason: impl Into<String>) -> ActionError {
    ActionError::InvalidInput {
        command: command.clone(),
        field: field.to_owned(),
        reason: reason.into(),
    }
}

/// Raw, unvalidated action input keyed by stable parameter name.
///
/// Values are strings for the same reason a UI text field is a string: parsing
/// and validation belong to the registry, and must happen before an action is
/// prepared.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActionInput {
    values: BTreeMap<String, String>,
}

impl ActionInput {
    /// Creates empty input.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces one raw value.
    pub fn with(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.values.insert(name.into(), value.into());
        self
    }

    /// Adds or replaces one raw value.
    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<String>) -> Option<String> {
        self.values.insert(name.into(), value.into())
    }

    /// Reads a raw value by stable parameter name.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    /// Iterates raw values in stable name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }

    /// Number of supplied values.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether no values were supplied.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// Validated, typed parameter values bound to a prepared action.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParameterValues {
    values: BTreeMap<String, ParameterValue>,
}

impl ParameterValues {
    /// Reads a typed value by stable parameter name.
    pub fn get(&self, name: &str) -> Option<&ParameterValue> {
        self.values.get(name)
    }

    /// Iterates typed values in stable name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &ParameterValue)> {
        self.values
            .iter()
            .map(|(name, value)| (name.as_str(), value))
    }

    /// Number of resolved values, including defaults.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether no values resolved.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// A registered command descriptor.
///
/// The descriptor is inert data. Stable [`CommandId`] values, not titles or
/// keywords, identify commands for execution.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandSpec {
    id: CommandId,
    title: String,
    keywords: Vec<String>,
    parameters: ParameterSchema,
    required_capabilities: Vec<String>,
    kind: CommandKind,
}

impl CommandSpec {
    /// Creates a spec with no keywords, parameters or capability requirements.
    pub fn new(id: CommandId, title: impl Into<String>, kind: CommandKind) -> Self {
        Self {
            id,
            title: title.into(),
            keywords: Vec::new(),
            parameters: ParameterSchema::new(),
            required_capabilities: Vec::new(),
            kind,
        }
    }

    /// Replaces the search keywords. Blank keywords are ignored.
    pub fn with_keywords<I, S>(mut self, keywords: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.keywords = keywords
            .into_iter()
            .map(Into::into)
            .filter(|keyword| !keyword.trim().is_empty())
            .collect();
        self
    }

    /// Replaces the parameter schema.
    pub fn with_parameters(mut self, parameters: ParameterSchema) -> Self {
        self.parameters = parameters;
        self
    }

    /// Adds one required capability name.
    pub fn requiring(mut self, capability: impl Into<String>) -> Self {
        self.required_capabilities.push(capability.into());
        self
    }

    /// Adds the minimum capability implied by this spec's kind, if any.
    ///
    /// Registration applies this union so a command cannot be under-declared.
    pub(crate) fn merge_kind_capability(&mut self) {
        if let Some(capability) = self.kind.required_capability()
            && !self
                .required_capabilities
                .iter()
                .any(|declared| declared == capability)
        {
            self.required_capabilities.push(capability.to_owned());
        }
    }

    /// The stable command identity.
    pub fn id(&self) -> &CommandId {
        &self.id
    }

    /// The human-readable title. Never used as execution identity.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Search keywords.
    pub fn keywords(&self) -> &[String] {
        &self.keywords
    }

    /// The typed parameter schema.
    pub fn parameters(&self) -> &ParameterSchema {
        &self.parameters
    }

    /// Capability names the user must hold before authorization.
    ///
    /// For a registered spec this includes the minimum capability implied by
    /// the command kind plus any names declared with
    /// [`requiring`](Self::requiring). A spec that has not been registered
    /// reports only its declared names.
    pub fn required_capabilities(&self) -> &[String] {
        &self.required_capabilities
    }

    /// How the host performs this command.
    pub fn kind(&self) -> &CommandKind {
        &self.kind
    }

    /// Whether performing this command requires explicit confirmation.
    pub fn requires_confirmation(&self) -> bool {
        self.kind.is_consequential()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command_id() -> CommandId {
        CommandId::parse("demo:parameter").unwrap()
    }

    #[test]
    fn binds_defaults_and_optional_values() {
        let schema = ParameterSchema::new()
            .with_parameter(Parameter::required("query", ParameterType::Text))
            .with_parameter(
                Parameter::optional("limit", ParameterType::Number)
                    .with_default(ParameterValue::Number(10.0)),
            )
            .with_parameter(
                Parameter::optional("safe", ParameterType::Boolean)
                    .with_default(ParameterValue::Boolean(true)),
            )
            .with_parameter(
                Parameter::optional(
                    "mode",
                    ParameterType::Choice {
                        options: vec!["fast".into(), "safe".into()],
                    },
                )
                .with_default(ParameterValue::Choice("fast".into())),
            )
            .with_parameter(Parameter::optional("path", ParameterType::Path));

        let input = ActionInput::new()
            .with("query", "hello")
            .with("limit", "")
            .with("safe", "FALSE");
        let values = schema.bind(&command_id(), &input).unwrap();

        assert_eq!(
            values.get("query"),
            Some(&ParameterValue::Text("hello".into()))
        );
        assert_eq!(values.get("limit"), Some(&ParameterValue::Number(10.0)));
        assert_eq!(values.get("safe"), Some(&ParameterValue::Boolean(false)));
        assert_eq!(
            values.get("mode"),
            Some(&ParameterValue::Choice("fast".into()))
        );
        assert!(values.get("path").is_none());
        assert_eq!(values.len(), 4);
    }

    #[test]
    fn required_default_satisfies_missing_value() {
        let schema = ParameterSchema::new().with_parameter(
            Parameter::required("limit", ParameterType::Number)
                .with_default(ParameterValue::Number(3.0)),
        );
        let values = schema.bind(&command_id(), &ActionInput::new()).unwrap();
        assert_eq!(values.get("limit"), Some(&ParameterValue::Number(3.0)));
    }

    #[test]
    fn rejects_missing_unknown_and_malformed_values() {
        let schema = ParameterSchema::new()
            .with_parameter(Parameter::required("query", ParameterType::Text))
            .with_parameter(Parameter::optional("limit", ParameterType::Number))
            .with_parameter(Parameter::optional("safe", ParameterType::Boolean))
            .with_parameter(Parameter::optional(
                "mode",
                ParameterType::Choice {
                    options: vec!["fast".into()],
                },
            ))
            .with_parameter(Parameter::optional("path", ParameterType::Path));

        let cases = [
            ActionInput::new(),
            ActionInput::new().with("query", "x").with("unknown", "1"),
            ActionInput::new().with("query", "x").with("limit", "NaN"),
            ActionInput::new().with("query", "x").with("limit", "inf"),
            ActionInput::new().with("query", "x").with("limit", "1e999"),
            ActionInput::new().with("query", "x").with("limit", "many"),
            ActionInput::new().with("query", "x").with("safe", "yes"),
            ActionInput::new().with("query", "x").with("mode", "other"),
            ActionInput::new().with("query", "x").with("path", "   "),
            ActionInput::new().with("query", "x").with("path", "a\0b"),
            ActionInput::new().with("query", "a\0b"),
            ActionInput::new().with("query", "a\nb"),
            ActionInput::new().with("query", "a\tb"),
        ];

        for input in cases {
            let error = schema.bind(&command_id(), &input).unwrap_err();
            assert_eq!(error.code(), crate::ErrorCode::InvalidInput);
        }
    }

    #[test]
    fn rejects_oversized_values_without_echoing_them() {
        let schema = ParameterSchema::new()
            .with_parameter(Parameter::required("query", ParameterType::Text));
        let long = "x".repeat(MAX_PARAMETER_VALUE_CHARS + 1);
        let error = schema
            .bind(&command_id(), &ActionInput::new().with("query", long))
            .unwrap_err();
        assert_eq!(error.code(), crate::ErrorCode::InvalidInput);
        assert!(!error.to_string().contains("xxxx"));
    }

    #[test]
    fn schema_registration_rules_are_specific() {
        let id = command_id();

        let duplicate = ParameterSchema::new()
            .with_parameter(Parameter::required("x", ParameterType::Text))
            .with_parameter(Parameter::optional("x", ParameterType::Text));
        assert!(matches!(
            duplicate.validate(&id),
            Err(RegistrationError::DuplicateParameter { parameter, .. }) if parameter == "x"
        ));

        let empty =
            ParameterSchema::new().with_parameter(Parameter::required("", ParameterType::Text));
        assert!(matches!(
            empty.validate(&id),
            Err(RegistrationError::InvalidParameterName { .. })
        ));

        let bad_name = ParameterSchema::new()
            .with_parameter(Parameter::required("bad name", ParameterType::Text));
        assert!(matches!(
            bad_name.validate(&id),
            Err(RegistrationError::InvalidParameterName { parameter, .. }) if parameter == "bad name"
        ));

        let empty_choice = ParameterSchema::new().with_parameter(Parameter::required(
            "mode",
            ParameterType::Choice {
                options: Vec::new(),
            },
        ));
        assert!(matches!(
            empty_choice.validate(&id),
            Err(RegistrationError::InvalidParameterType { .. })
        ));

        let duplicate_option = ParameterSchema::new().with_parameter(Parameter::required(
            "mode",
            ParameterType::Choice {
                options: vec!["a".into(), "a".into()],
            },
        ));
        assert!(matches!(
            duplicate_option.validate(&id),
            Err(RegistrationError::InvalidParameterType { .. })
        ));

        let wrong_default = ParameterSchema::new().with_parameter(
            Parameter::optional("limit", ParameterType::Number)
                .with_default(ParameterValue::Text("ten".into())),
        );
        assert!(matches!(
            wrong_default.validate(&id),
            Err(RegistrationError::InvalidDefault { parameter, .. }) if parameter == "limit"
        ));

        let bad_choice_default = ParameterSchema::new().with_parameter(
            Parameter::optional(
                "mode",
                ParameterType::Choice {
                    options: vec!["a".into()],
                },
            )
            .with_default(ParameterValue::Choice("b".into())),
        );
        assert!(matches!(
            bad_choice_default.validate(&id),
            Err(RegistrationError::InvalidDefault { .. })
        ));

        let infinite_default = ParameterSchema::new().with_parameter(
            Parameter::optional("limit", ParameterType::Number)
                .with_default(ParameterValue::Number(f64::INFINITY)),
        );
        assert!(matches!(
            infinite_default.validate(&id),
            Err(RegistrationError::InvalidDefault { .. })
        ));
    }

    #[test]
    fn deterministic_malformed_input_fuzz_never_panics() {
        let schema = ParameterSchema::new()
            .with_parameter(Parameter::required("query", ParameterType::Text))
            .with_parameter(Parameter::optional("limit", ParameterType::Number))
            .with_parameter(Parameter::optional("safe", ParameterType::Boolean))
            .with_parameter(Parameter::optional(
                "mode",
                ParameterType::Choice {
                    options: vec!["fast".into(), "safe".into()],
                },
            ))
            .with_parameter(Parameter::optional("path", ParameterType::Path));

        let alphabet: Vec<char> = "abcXYZ019 -_.:/\\\0\t\n\u{1f600}\u{7f}".chars().collect();
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut accepted = 0;
        for iteration in 0..512 {
            if iteration % 8 == 0 {
                let valid = ActionInput::new()
                    .with("query", "hello")
                    .with("limit", "12.5")
                    .with("safe", "true")
                    .with("mode", "safe")
                    .with("path", "/tmp/example");
                let values = schema.bind(&command_id(), &valid).unwrap();
                assert_eq!(
                    values.get("query"),
                    Some(&ParameterValue::Text("hello".into()))
                );
                accepted += 1;
                continue;
            }

            let mut input = ActionInput::new();
            for name in ["query", "limit", "safe", "mode", "path"] {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let length = (state as usize) % 24;
                let mut value = String::new();
                for _ in 0..length {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    let index = (state as usize) % alphabet.len();
                    value.push(alphabet[index]);
                }
                input.insert(name, value);
            }
            match schema.bind(&command_id(), &input) {
                Ok(values) => {
                    accepted += 1;
                    assert!(values.get("query").is_some());
                }
                Err(error) => assert_eq!(error.code(), crate::ErrorCode::InvalidInput),
            }
        }
        // The fuzz corpus should exercise both accepted and rejected input.
        assert!(accepted > 0);
        assert!(accepted < 512);
    }

    #[test]
    fn schema_count_and_choice_fuzz_never_panics() {
        let option_alphabet: Vec<&str> = vec!["a", "b", "", "x\0y", "safe", "fast"];
        let mut state = 0xa5a5_5a5a_1234_4321_u64;
        for _ in 0..128 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let parameter_count = (state as usize) % (MAX_PARAMETERS_PER_COMMAND + 8);

            let mut schema = ParameterSchema::new();
            for index in 0..parameter_count {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let choice = state & 1 == 0;
                if choice {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    let option_count = (state as usize) % (MAX_CHOICE_OPTIONS + 8);
                    let options = (0..option_count)
                        .map(|option_index| {
                            state ^= state << 13;
                            state ^= state >> 7;
                            state ^= state << 17;
                            let base = option_alphabet[(state as usize) % option_alphabet.len()];
                            format!("{base}{option_index}")
                        })
                        .collect();
                    schema = schema.with_parameter(Parameter::optional(
                        format!("p{index}"),
                        ParameterType::Choice { options },
                    ));
                } else {
                    schema = schema.with_parameter(Parameter::optional(
                        format!("p{index}"),
                        ParameterType::Text,
                    ));
                }
            }

            match schema.validate(&command_id()) {
                Ok(()) => {
                    assert!(parameter_count <= MAX_PARAMETERS_PER_COMMAND);
                }
                Err(RegistrationError::TooManyParameters { .. }) => {
                    assert!(parameter_count > MAX_PARAMETERS_PER_COMMAND);
                }
                Err(_) => {}
            }
        }
    }

    #[test]
    fn action_input_preserves_names_and_values() {
        let input = ActionInput::new().with("b", "2").with("a", "1");
        assert_eq!(input.get("a"), Some("1"));
        assert_eq!(input.len(), 2);
        assert_eq!(
            input.iter().collect::<Vec<_>>(),
            vec![("a", "1"), ("b", "2")]
        );
        assert!(!input.is_empty());
        assert!(ActionInput::new().is_empty());
    }

    #[test]
    fn command_kinds_are_data_and_classify_consequence() {
        assert!(!CommandKind::Builtin.is_consequential());
        assert!(CommandKind::Application { path: "a".into() }.is_consequential());
        assert!(CommandKind::Quicklink { url: "u".into() }.is_consequential());
        assert!(
            CommandKind::Script {
                script_id: "s".into()
            }
            .is_consequential()
        );
        assert!(
            CommandKind::Extension {
                extension_id: "e".into(),
                action: "a".into(),
            }
            .is_consequential()
        );
        assert_eq!(
            CommandKind::Extension {
                extension_id: "e".into(),
                action: "a".into(),
            }
            .target_identity()
            .as_deref(),
            Some("e:a")
        );
        assert!(CommandKind::Builtin.target_identity().is_none());
    }

    #[test]
    fn kind_targets_reject_invisible_format_characters() {
        assert!(
            CommandKind::Application {
                path: "demo\u{200B}.exe".into()
            }
            .invalid_identity()
            .is_some()
        );
        assert!(
            CommandKind::Quicklink {
                url: "https://example.com/\u{202E}evil".into()
            }
            .invalid_identity()
            .is_some()
        );
        assert!(
            CommandKind::Extension {
                extension_id: "vendor\u{200B}".into(),
                action: "run".into(),
            }
            .invalid_identity()
            .is_some()
        );
        // Targets are identities, so joiners and variation selectors are
        // stricter than display-text policy.
        assert!(
            CommandKind::Application {
                path: "demo\u{200D}.exe".into()
            }
            .invalid_identity()
            .is_some()
        );
        assert!(
            CommandKind::Script {
                script_id: "script\u{FE0F}".into()
            }
            .invalid_identity()
            .is_some()
        );
    }

    #[test]
    fn kinds_imply_minimum_capabilities() {
        assert_eq!(CommandKind::Builtin.required_capability(), None);
        assert_eq!(
            CommandKind::Application { path: "a".into() }.required_capability(),
            Some(capability_names::LAUNCH_APPLICATION)
        );
        assert_eq!(
            CommandKind::Quicklink { url: "u".into() }.required_capability(),
            Some(capability_names::OPEN_URL)
        );
        assert_eq!(
            CommandKind::Script {
                script_id: "s".into()
            }
            .required_capability(),
            Some(capability_names::RUN_SCRIPT)
        );
        assert_eq!(
            CommandKind::Extension {
                extension_id: "e".into(),
                action: "a".into(),
            }
            .required_capability(),
            Some(capability_names::INVOKE_EXTENSION)
        );
    }

    #[test]
    fn schema_rejects_control_characters_and_display_bounds() {
        let id = command_id();

        let control_option = ParameterSchema::new().with_parameter(Parameter::required(
            "mode",
            ParameterType::Choice {
                options: vec!["a\0b".into()],
            },
        ));
        assert!(matches!(
            control_option.validate(&id),
            Err(RegistrationError::InvalidParameterType { .. })
        ));

        let control_default = ParameterSchema::new().with_parameter(
            Parameter::optional("text", ParameterType::Text)
                .with_default(ParameterValue::Text("a\nb".into())),
        );
        assert!(matches!(
            control_default.validate(&id),
            Err(RegistrationError::InvalidDefault { .. })
        ));

        let invisible_option = ParameterSchema::new().with_parameter(Parameter::required(
            "mode",
            ParameterType::Choice {
                options: vec!["sa\u{200B}fe".into()],
            },
        ));
        assert!(matches!(
            invisible_option.validate(&id),
            Err(RegistrationError::InvalidParameterType { .. })
        ));

        let invisible_default = ParameterSchema::new().with_parameter(
            Parameter::optional("text", ParameterType::Text)
                .with_default(ParameterValue::Text("sa\u{200B}fe".into())),
        );
        assert!(matches!(
            invisible_default.validate(&id),
            Err(RegistrationError::InvalidDefault { .. })
        ));

        let too_many =
            (0..=MAX_PARAMETERS_PER_COMMAND).fold(ParameterSchema::new(), |schema, index| {
                schema.with_parameter(Parameter::optional(
                    format!("p{index}"),
                    ParameterType::Text,
                ))
            });
        assert!(matches!(
            too_many.validate(&id),
            Err(RegistrationError::TooManyParameters { limit, .. })
                if limit == MAX_PARAMETERS_PER_COMMAND
        ));

        let too_many_options = ParameterSchema::new().with_parameter(Parameter::required(
            "mode",
            ParameterType::Choice {
                options: (0..=MAX_CHOICE_OPTIONS)
                    .map(|index| format!("option-{index}"))
                    .collect(),
            },
        ));
        assert!(matches!(
            too_many_options.validate(&id),
            Err(RegistrationError::InvalidParameterType { .. })
        ));
    }

    #[test]
    fn parameter_name_control_characters_are_rejected() {
        let schema = ParameterSchema::new()
            .with_parameter(Parameter::required("query\n", ParameterType::Text));
        assert!(matches!(
            schema.validate(&command_id()),
            Err(RegistrationError::InvalidParameterName { .. })
        ));
    }

    #[test]
    fn parameter_value_accessors_are_typed() {
        assert_eq!(ParameterValue::Text("x".into()).as_str(), Some("x"));
        assert_eq!(ParameterValue::Number(2.5).as_f64(), Some(2.5));
        assert_eq!(ParameterValue::Boolean(true).as_bool(), Some(true));
        assert_eq!(ParameterValue::Choice("a".into()).as_str(), Some("a"));
        assert_eq!(
            ParameterValue::Path("/tmp/x".into()).as_str(),
            Some("/tmp/x")
        );
        assert_eq!(ParameterValue::Boolean(true).as_str(), None);
        assert_eq!(ParameterValue::Text("x".into()).as_f64(), None);
    }
}
