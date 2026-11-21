//! Structured quicklink templates: literal text, typed arguments and encoding.
//!
//! Grammar (single pass, no re-expansion):
//!
//! * literal text is copied to the resolved target unchanged;
//! * `{{` produces a literal `{` and `}}` produces a literal `}`;
//! * `{name}` inserts the declared argument using its declared encoding;
//! * `{name:mode}` inserts it with an explicit per-placeholder encoding;
//! * a lone `{` or `}` is malformed.
//!
//! Declared arguments are named `A-Z a-z 0-9 _ -` (starting with a letter or
//! underscore), must be unique, and each must appear exactly once in the
//! template. Argument values are substituted once and never rescanned, so a
//! value containing `{other}` or shell metacharacters stays literal data.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::encoding::{EncodeError, encode_component, encode_literal_segment};
use crate::text::is_unsafe_format_character;

/// Maximum number of characters in a template source.
pub const MAX_TEMPLATE_CHARS: usize = 4096;
/// Maximum number of declared arguments in one template.
pub const MAX_ARGUMENTS: usize = 32;
/// Maximum number of characters in one argument name.
pub const MAX_ARGUMENT_NAME_CHARS: usize = 64;
/// Maximum number of choices for a `choice` argument.
pub const MAX_CHOICES: usize = 64;
/// Maximum number of characters in one choice value.
pub const MAX_CHOICE_CHARS: usize = 128;
/// Maximum number of characters in a fully resolved target.
pub const MAX_RESOLVED_TARGET_CHARS: usize = 8_192;
/// Maximum byte length of a fully resolved target.
///
/// This matches the host URL boundary (8,192 bytes) so a preview cannot pass a
/// target the platform adapter would reject. It also bounds local paths.
pub const MAX_RESOLVED_TARGET_BYTES: usize = 8_192;

/// How one substituted value is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    /// RFC 3986 percent-encoding as a single URL path segment.
    UrlPathSegment,
    /// RFC 3986 percent-encoding as a single URL query value (no `+`).
    UrlQueryValue,
    /// Verbatim filesystem segment; separators, controls and `..` rejected.
    LiteralPathSegment,
}

impl Encoding {
    /// Stable serialized label.
    pub fn label(self) -> &'static str {
        match self {
            Self::UrlPathSegment => "url_path_segment",
            Self::UrlQueryValue => "url_query_value",
            Self::LiteralPathSegment => "literal_path_segment",
        }
    }

    /// Parses a label or a short alias, as accepted in `{name:mode}`.
    pub fn parse_label(raw: &str) -> Option<Self> {
        match raw {
            "url_path_segment" | "path" => Some(Self::UrlPathSegment),
            "url_query_value" | "query" => Some(Self::UrlQueryValue),
            "literal_path_segment" | "literal" => Some(Self::LiteralPathSegment),
            _ => None,
        }
    }

    fn encode(self, value: &str) -> Result<String, EncodeError> {
        match self {
            Self::UrlPathSegment | Self::UrlQueryValue => Ok(encode_component(value)),
            Self::LiteralPathSegment => encode_literal_segment(value),
        }
    }
}

impl fmt::Display for Encoding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// The declared type of an argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArgKind {
    /// Free text; may contain spaces, quotes, percent signs and Unicode.
    Text,
    /// A finite decimal literal, validated and preserved verbatim.
    Number,
    /// `true` or `false`.
    Boolean,
    /// One exact value from the declared choice list.
    Choice,
}

impl ArgKind {
    /// Stable serialized label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Choice => "choice",
        }
    }
}

impl fmt::Display for ArgKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// A typed argument value.
///
/// Numbers are stored as their validated, trimmed literal text instead of a
/// floating-point value, so `1.50` round-trips exactly through JSON and no
/// locale or float-formatting drift can change a persisted target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ArgumentValue {
    Text(String),
    Number(String),
    Boolean(bool),
    Choice(String),
}

/// Why a number literal was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueError {
    EmptyNumber,
    InvalidNumber { value: String },
}

impl fmt::Display for ValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyNumber => formatter.write_str("number argument is empty"),
            Self::InvalidNumber { value } => write!(
                formatter,
                "{value:?} is not a finite decimal number; use digits with an optional sign, fraction and exponent"
            ),
        }
    }
}

impl std::error::Error for ValueError {}

impl ArgumentValue {
    /// Wraps free text.
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }

    /// Validates and wraps a decimal number literal.
    pub fn number(value: &str) -> Result<Self, ValueError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(ValueError::EmptyNumber);
        }
        match trimmed.parse::<f64>() {
            Ok(parsed) if parsed.is_finite() => Ok(Self::Number(trimmed.to_owned())),
            _ => Err(ValueError::InvalidNumber {
                value: value.to_owned(),
            }),
        }
    }

    /// Wraps a boolean.
    pub fn boolean(value: bool) -> Self {
        Self::Boolean(value)
    }

    /// Wraps a choice value.
    pub fn choice(value: impl Into<String>) -> Self {
        Self::Choice(value.into())
    }

    /// The declared type of this value.
    pub fn kind(&self) -> ArgKind {
        match self {
            Self::Text(_) => ArgKind::Text,
            Self::Number(_) => ArgKind::Number,
            Self::Boolean(_) => ArgKind::Boolean,
            Self::Choice(_) => ArgKind::Choice,
        }
    }

    fn render(&self) -> String {
        match self {
            Self::Text(value) | Self::Number(value) | Self::Choice(value) => value.clone(),
            Self::Boolean(value) => value.to_string(),
        }
    }

    fn rendered_len(&self) -> usize {
        match self {
            Self::Text(value) | Self::Number(value) | Self::Choice(value) => value.len(),
            Self::Boolean(_) => 5,
        }
    }
}

/// One declared template argument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArgSpec {
    /// Placeholder name, e.g. `query`.
    pub name: String,
    /// Declared type.
    pub kind: ArgKind,
    /// Encoding used when a placeholder does not override it.
    pub encoding: Encoding,
    /// Whether the caller must supply a value.
    pub required: bool,
    /// Default used when an optional argument is omitted.
    pub default: Option<ArgumentValue>,
    /// Allowed values for [`ArgKind::Choice`]; empty for other kinds.
    #[serde(default)]
    pub choices: Vec<String>,
}

impl ArgSpec {
    /// Creates a required argument with no default.
    pub fn new(name: impl Into<String>, kind: ArgKind, encoding: Encoding) -> Self {
        Self {
            name: name.into(),
            kind,
            encoding,
            required: true,
            default: None,
            choices: Vec::new(),
        }
    }

    /// Creates a required choice argument.
    pub fn choice<I, S>(name: impl Into<String>, encoding: Encoding, choices: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            name: name.into(),
            kind: ArgKind::Choice,
            encoding,
            required: true,
            default: None,
            choices: choices.into_iter().map(Into::into).collect(),
        }
    }

    /// Marks the argument optional and supplies its required default.
    ///
    /// Optional arguments without a default are rejected: an omitted argument
    /// must never silently resolve to an ambiguous empty string.
    pub fn optional(mut self, default: ArgumentValue) -> Self {
        self.required = false;
        self.default = Some(default);
        self
    }

    /// Replaces the default without changing requiredness.
    pub fn with_default(mut self, default: ArgumentValue) -> Self {
        self.default = Some(default);
        self
    }

    fn validate(&self) -> Result<(), TemplateError> {
        if self.name.is_empty() {
            return Err(TemplateError::EmptyPlaceholder { position: 0 });
        }
        validate_name(&self.name, 0)?;
        if self.kind != ArgKind::Choice && !self.choices.is_empty() {
            return Err(TemplateError::UnexpectedChoices {
                name: self.name.clone(),
            });
        }
        if self.kind == ArgKind::Choice {
            if self.choices.is_empty() {
                return Err(TemplateError::MissingChoices {
                    name: self.name.clone(),
                });
            }
            if self.choices.len() > MAX_CHOICES {
                return Err(TemplateError::TooManyChoices {
                    name: self.name.clone(),
                    limit: MAX_CHOICES,
                });
            }
            let mut seen = BTreeSet::new();
            for choice in &self.choices {
                if choice.trim().is_empty() {
                    return Err(TemplateError::EmptyChoice {
                        name: self.name.clone(),
                    });
                }
                if choice.chars().count() > MAX_CHOICE_CHARS {
                    return Err(TemplateError::ChoiceTooLong {
                        name: self.name.clone(),
                        limit: MAX_CHOICE_CHARS,
                    });
                }
                if choice.chars().any(char::is_control) {
                    return Err(TemplateError::ControlCharacter { position: 0 });
                }
                if !seen.insert(choice.as_str()) {
                    return Err(TemplateError::DuplicateChoice {
                        name: self.name.clone(),
                        choice: choice.clone(),
                    });
                }
            }
        }
        if self.required && self.default.is_some() {
            return Err(TemplateError::RequiredWithDefault {
                name: self.name.clone(),
            });
        }
        if !self.required && self.default.is_none() {
            return Err(TemplateError::OptionalWithoutDefault {
                name: self.name.clone(),
            });
        }
        if let Some(default) = &self.default {
            self.validate_default(default)?;
        }
        Ok(())
    }

    fn validate_default(&self, default: &ArgumentValue) -> Result<(), TemplateError> {
        if default.rendered_len() > MAX_RESOLVED_TARGET_BYTES {
            return Err(TemplateError::DefaultTooLong {
                name: self.name.clone(),
                limit: MAX_RESOLVED_TARGET_BYTES,
            });
        }
        if default.kind() != self.kind {
            return Err(TemplateError::DefaultKindMismatch {
                name: self.name.clone(),
                expected: self.kind,
                actual: default.kind(),
            });
        }
        match default {
            ArgumentValue::Number(raw) => {
                if !is_canonical_number(raw) {
                    return Err(TemplateError::InvalidDefaultNumber {
                        name: self.name.clone(),
                        value: raw.clone(),
                    });
                }
            }
            ArgumentValue::Choice(value) if !self.choices.iter().any(|item| item == value) => {
                return Err(TemplateError::DefaultNotInChoices {
                    name: self.name.clone(),
                    value: value.clone(),
                });
            }
            _ => {}
        }
        Ok(())
    }

    fn check_value(&self, value: &ArgumentValue) -> Result<(), ResolveError> {
        if value.kind() != self.kind {
            return Err(ResolveError::KindMismatch {
                name: self.name.clone(),
                expected: self.kind,
                actual: value.kind(),
            });
        }
        match value {
            ArgumentValue::Number(raw) => {
                if !is_canonical_number(raw) {
                    return Err(ResolveError::InvalidNumber {
                        name: self.name.clone(),
                        value: raw.clone(),
                    });
                }
            }
            ArgumentValue::Choice(value) if !self.choices.iter().any(|item| item == value) => {
                return Err(ResolveError::NotAChoice {
                    name: self.name.clone(),
                    value: value.clone(),
                    choices: self.choices.clone(),
                });
            }
            _ => {}
        }
        Ok(())
    }
}

/// One parsed template part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// Text copied verbatim into the target.
    Literal(String),
    /// A named placeholder and the encoding chosen for this occurrence.
    Placeholder { name: String, encoding: Encoding },
}

/// A parsed, validated template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    parts: Vec<Part>,
    arguments: Vec<ArgSpec>,
}

/// A malformed or internally inconsistent template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateError {
    TooLong {
        limit: usize,
    },
    TooManyArguments {
        limit: usize,
    },
    UnclosedPlaceholder {
        position: usize,
    },
    UnexpectedClosingBrace {
        position: usize,
    },
    NestedPlaceholder {
        position: usize,
    },
    EmptyPlaceholder {
        position: usize,
    },
    InvalidName {
        name: String,
        position: usize,
    },
    NameTooLong {
        limit: usize,
    },
    UnknownMode {
        mode: String,
        position: usize,
    },
    UnknownPlaceholder {
        name: String,
        position: usize,
    },
    DuplicatePlaceholder {
        name: String,
        position: usize,
    },
    DuplicateArgument {
        name: String,
    },
    UnusedArgument {
        name: String,
    },
    RequiredWithDefault {
        name: String,
    },
    OptionalWithoutDefault {
        name: String,
    },
    DefaultKindMismatch {
        name: String,
        expected: ArgKind,
        actual: ArgKind,
    },
    InvalidDefaultNumber {
        name: String,
        value: String,
    },
    DefaultTooLong {
        name: String,
        limit: usize,
    },
    DefaultNotInChoices {
        name: String,
        value: String,
    },
    MissingChoices {
        name: String,
    },
    UnexpectedChoices {
        name: String,
    },
    TooManyChoices {
        name: String,
        limit: usize,
    },
    EmptyChoice {
        name: String,
    },
    ChoiceTooLong {
        name: String,
        limit: usize,
    },
    DuplicateChoice {
        name: String,
        choice: String,
    },
    ControlCharacter {
        position: usize,
    },
    UnsafeFormatCharacter {
        character: char,
        position: usize,
    },
}

impl fmt::Display for TemplateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong { limit } => {
                write!(formatter, "template is longer than {limit} characters")
            }
            Self::TooManyArguments { limit } => {
                write!(formatter, "template declares more than {limit} arguments")
            }
            Self::UnclosedPlaceholder { position } => {
                write!(
                    formatter,
                    "placeholder starting at byte {position} is missing '}}'"
                )
            }
            Self::UnexpectedClosingBrace { position } => write!(
                formatter,
                "unexpected '}}' at byte {position}; escape it by doubling the brace"
            ),
            Self::NestedPlaceholder { position } => {
                write!(formatter, "'{{' inside a placeholder at byte {position}")
            }
            Self::EmptyPlaceholder { position } => {
                write!(formatter, "empty placeholder '{{}}' at byte {position}")
            }
            Self::InvalidName { name, position } => write!(
                formatter,
                "invalid placeholder name {name:?} at byte {position}; use letters, digits, '_' or '-', starting with a letter or '_'"
            ),
            Self::NameTooLong { limit } => {
                write!(formatter, "argument name is longer than {limit} characters")
            }
            Self::UnknownMode { mode, position } => write!(
                formatter,
                "unknown encoding mode {mode:?} at byte {position}; use url_path_segment, url_query_value or literal_path_segment"
            ),
            Self::UnknownPlaceholder { name, position } => write!(
                formatter,
                "placeholder {name:?} at byte {position} has no argument declaration"
            ),
            Self::DuplicatePlaceholder { name, position } => write!(
                formatter,
                "placeholder {name:?} appears more than once (again at byte {position}); declare each argument exactly once"
            ),
            Self::DuplicateArgument { name } => {
                write!(formatter, "argument {name:?} is declared more than once")
            }
            Self::UnusedArgument { name } => write!(
                formatter,
                "argument {name:?} is declared but never used in the template"
            ),
            Self::RequiredWithDefault { name } => write!(
                formatter,
                "argument {name:?} is required and also has a default; mark it optional or remove the default"
            ),
            Self::OptionalWithoutDefault { name } => write!(
                formatter,
                "argument {name:?} is optional but has no default"
            ),
            Self::DefaultKindMismatch {
                name,
                expected,
                actual,
            } => write!(
                formatter,
                "argument {name:?} is {expected} but its default is {actual}"
            ),
            Self::InvalidDefaultNumber { name, value } => write!(
                formatter,
                "argument {name:?} has invalid number default {value:?}"
            ),
            Self::DefaultTooLong { name, limit } => write!(
                formatter,
                "argument {name:?} has a default longer than {limit} bytes"
            ),
            Self::DefaultNotInChoices { name, value } => write!(
                formatter,
                "argument {name:?} has default {value:?} which is not one of its choices"
            ),
            Self::MissingChoices { name } => {
                write!(formatter, "choice argument {name:?} has no allowed choices")
            }
            Self::UnexpectedChoices { name } => write!(
                formatter,
                "non-choice argument {name:?} must not declare choices"
            ),
            Self::TooManyChoices { name, limit } => write!(
                formatter,
                "choice argument {name:?} has more than {limit} choices"
            ),
            Self::EmptyChoice { name } => {
                write!(
                    formatter,
                    "choice argument {name:?} contains an empty choice"
                )
            }
            Self::ChoiceTooLong { name, limit } => write!(
                formatter,
                "choice argument {name:?} contains a choice longer than {limit} characters"
            ),
            Self::DuplicateChoice { name, choice } => write!(
                formatter,
                "choice argument {name:?} repeats the choice {choice:?}"
            ),
            Self::ControlCharacter { position } => write!(
                formatter,
                "template contains a control character at byte {position}"
            ),
            Self::UnsafeFormatCharacter {
                character,
                position,
            } => write!(
                formatter,
                "template contains invisible format character U+{:04X} at byte {position}",
                *character as u32
            ),
        }
    }
}

impl std::error::Error for TemplateError {}

/// Why a template could not be resolved with the supplied values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    MissingRequired {
        name: String,
    },
    UnknownArgument {
        name: String,
    },
    KindMismatch {
        name: String,
        expected: ArgKind,
        actual: ArgKind,
    },
    InvalidNumber {
        name: String,
        value: String,
    },
    InvalidBoolean {
        name: String,
        value: String,
    },
    NotAChoice {
        name: String,
        value: String,
        choices: Vec<String>,
    },
    Encoding {
        name: String,
        error: EncodeError,
    },
    TargetTooLong {
        limit: usize,
    },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRequired { name } => {
                write!(formatter, "argument {name:?} is required")
            }
            Self::UnknownArgument { name } => {
                write!(formatter, "no argument named {name:?} is declared")
            }
            Self::KindMismatch {
                name,
                expected,
                actual,
            } => write!(
                formatter,
                "argument {name:?} expects {expected} but received {actual}"
            ),
            Self::InvalidNumber { name, value } => {
                write!(formatter, "argument {name:?} is not a number: {value:?}")
            }
            Self::InvalidBoolean { name, value } => write!(
                formatter,
                "argument {name:?} must be 'true' or 'false', found {value:?}"
            ),
            Self::NotAChoice {
                name,
                value,
                choices,
            } => write!(
                formatter,
                "argument {name:?} value {value:?} is not one of {choices:?}"
            ),
            Self::Encoding { name, error } => {
                write!(formatter, "argument {name:?} cannot be encoded: {error}")
            }
            Self::TargetTooLong { limit } => write!(
                formatter,
                "the resolved target is longer than {limit} bytes or characters"
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

impl Template {
    /// Parses and validates a template against its argument declarations.
    ///
    /// Every declared argument must appear exactly once, every placeholder must
    /// be declared, and defaults are checked against their declared type.
    pub fn parse(source: &str, arguments: Vec<ArgSpec>) -> Result<Self, TemplateError> {
        if source.chars().count() > MAX_TEMPLATE_CHARS {
            return Err(TemplateError::TooLong {
                limit: MAX_TEMPLATE_CHARS,
            });
        }
        if arguments.len() > MAX_ARGUMENTS {
            return Err(TemplateError::TooManyArguments {
                limit: MAX_ARGUMENTS,
            });
        }

        let mut declarations: BTreeMap<&str, &ArgSpec> = BTreeMap::new();
        let mut declared_names = Vec::with_capacity(arguments.len());
        for spec in &arguments {
            spec.validate()?;
            if declarations.insert(spec.name.as_str(), spec).is_some() {
                return Err(TemplateError::DuplicateArgument {
                    name: spec.name.clone(),
                });
            }
            declared_names.push(spec.name.as_str());
        }

        let mut parts = Vec::new();
        let mut literal = String::new();
        let mut used: BTreeSet<String> = BTreeSet::new();
        let mut characters = source.char_indices().peekable();

        while let Some((position, character)) = characters.next() {
            match character {
                '{' => {
                    if matches!(characters.peek(), Some((_, '{'))) {
                        characters.next();
                        literal.push('{');
                        continue;
                    }
                    let mut name = String::new();
                    let mut mode = String::new();
                    let mut saw_colon = false;
                    let mut closed = false;
                    for (inner_position, inner) in characters.by_ref() {
                        match inner {
                            '}' => {
                                closed = true;
                                break;
                            }
                            '{' => {
                                return Err(TemplateError::NestedPlaceholder {
                                    position: inner_position,
                                });
                            }
                            ':' if !saw_colon => saw_colon = true,
                            ':' => {
                                return Err(TemplateError::InvalidName {
                                    name: format!("{name}:{mode}:"),
                                    position,
                                });
                            }
                            other if saw_colon => mode.push(other),
                            other => name.push(other),
                        }
                    }
                    if !closed {
                        return Err(TemplateError::UnclosedPlaceholder { position });
                    }
                    if name.is_empty() {
                        return Err(TemplateError::EmptyPlaceholder { position });
                    }
                    validate_name(&name, position)?;
                    let Some(spec) = declarations.get(name.as_str()) else {
                        return Err(TemplateError::UnknownPlaceholder { name, position });
                    };
                    let encoding = if !saw_colon {
                        spec.encoding
                    } else {
                        Encoding::parse_label(&mode)
                            .ok_or(TemplateError::UnknownMode { mode, position })?
                    };
                    if !used.insert(name.clone()) {
                        return Err(TemplateError::DuplicatePlaceholder { name, position });
                    }
                    if !literal.is_empty() {
                        parts.push(Part::Literal(std::mem::take(&mut literal)));
                    }
                    parts.push(Part::Placeholder { name, encoding });
                }
                '}' => {
                    if matches!(characters.peek(), Some((_, '}'))) {
                        characters.next();
                        literal.push('}');
                        continue;
                    }
                    return Err(TemplateError::UnexpectedClosingBrace { position });
                }
                other if other == '\0' || other.is_control() => {
                    return Err(TemplateError::ControlCharacter { position });
                }
                other if is_unsafe_format_character(other) => {
                    return Err(TemplateError::UnsafeFormatCharacter {
                        character: other,
                        position,
                    });
                }
                other => literal.push(other),
            }
        }
        if !literal.is_empty() {
            parts.push(Part::Literal(literal));
        }
        for name in declared_names {
            if !used.contains(name) {
                return Err(TemplateError::UnusedArgument {
                    name: name.to_owned(),
                });
            }
        }

        Ok(Self { parts, arguments })
    }

    /// Declared arguments, in author order.
    pub fn arguments(&self) -> &[ArgSpec] {
        &self.arguments
    }

    /// Parsed parts, in template order.
    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    /// Declared argument by exact name.
    pub fn argument(&self, name: &str) -> Option<&ArgSpec> {
        self.arguments.iter().find(|spec| spec.name == name)
    }

    /// True when the template includes at least one placeholder.
    pub fn has_placeholders(&self) -> bool {
        self.parts
            .iter()
            .any(|part| matches!(part, Part::Placeholder { .. }))
    }

    /// Literal text before the first placeholder (empty when one starts the
    /// template). Used for static scheme checks at save time.
    pub fn static_prefix(&self) -> String {
        let mut prefix = String::new();
        for part in &self.parts {
            match part {
                Part::Literal(text) => prefix.push_str(text),
                Part::Placeholder { .. } => break,
            }
        }
        prefix
    }

    /// Canonical source with escaped braces and an explicit encoding on every
    /// placeholder, e.g. `https://example.test/{query:url_query_value}`.
    pub fn source(&self) -> String {
        let mut source = String::new();
        for part in &self.parts {
            match part {
                Part::Literal(text) => {
                    for character in text.chars() {
                        match character {
                            '{' => source.push_str("{{"),
                            '}' => source.push_str("}}"),
                            other => source.push(other),
                        }
                    }
                }
                Part::Placeholder { name, encoding } => {
                    source.push('{');
                    source.push_str(name);
                    source.push(':');
                    source.push_str(encoding.label());
                    source.push('}');
                }
            }
        }
        source
    }

    /// Resolves typed values into a target using single-pass substitution.
    pub fn resolve(
        &self,
        values: &BTreeMap<String, ArgumentValue>,
    ) -> Result<String, ResolveError> {
        for name in values.keys() {
            if self.argument(name).is_none() {
                return Err(ResolveError::UnknownArgument { name: name.clone() });
            }
        }

        let mut target = String::new();
        for part in &self.parts {
            match part {
                Part::Literal(text) => target.push_str(text),
                Part::Placeholder { name, encoding } => {
                    let Some(spec) = self.argument(name) else {
                        return Err(ResolveError::UnknownArgument { name: name.clone() });
                    };
                    let value = match values.get(name) {
                        Some(value) => {
                            spec.check_value(value)?;
                            value
                        }
                        None => spec
                            .default
                            .as_ref()
                            .ok_or_else(|| ResolveError::MissingRequired { name: name.clone() })?,
                    };
                    let raw_len = value.rendered_len();
                    let remaining = MAX_RESOLVED_TARGET_BYTES.saturating_sub(target.len());
                    if raw_len > remaining {
                        return Err(ResolveError::TargetTooLong {
                            limit: MAX_RESOLVED_TARGET_BYTES,
                        });
                    }
                    let encoded = encoding.encode(&value.render()).map_err(|error| {
                        ResolveError::Encoding {
                            name: name.clone(),
                            error,
                        }
                    })?;
                    target.push_str(&encoded);
                }
            }
        }
        if target.len() > MAX_RESOLVED_TARGET_BYTES
            || target.chars().count() > MAX_RESOLVED_TARGET_CHARS
        {
            return Err(ResolveError::TargetTooLong {
                limit: MAX_RESOLVED_TARGET_BYTES,
            });
        }
        Ok(target)
    }

    /// Resolves user-entered text, parsing each value according to its type.
    pub fn resolve_text(&self, values: &BTreeMap<String, String>) -> Result<String, ResolveError> {
        for name in values.keys() {
            if self.argument(name).is_none() {
                return Err(ResolveError::UnknownArgument { name: name.clone() });
            }
        }
        let mut typed: BTreeMap<String, ArgumentValue> = BTreeMap::new();
        for spec in &self.arguments {
            let Some(raw) = values.get(&spec.name) else {
                if let Some(default) = &spec.default {
                    typed.insert(spec.name.clone(), default.clone());
                    continue;
                }
                return Err(ResolveError::MissingRequired {
                    name: spec.name.clone(),
                });
            };
            if raw.len() > MAX_RESOLVED_TARGET_BYTES {
                return Err(ResolveError::TargetTooLong {
                    limit: MAX_RESOLVED_TARGET_BYTES,
                });
            }
            let value = match spec.kind {
                ArgKind::Text => ArgumentValue::Text(raw.clone()),
                ArgKind::Number => {
                    ArgumentValue::number(raw).map_err(|_| ResolveError::InvalidNumber {
                        name: spec.name.clone(),
                        value: raw.clone(),
                    })?
                }
                ArgKind::Boolean => match raw.trim().to_ascii_lowercase().as_str() {
                    "true" => ArgumentValue::Boolean(true),
                    "false" => ArgumentValue::Boolean(false),
                    _ => {
                        return Err(ResolveError::InvalidBoolean {
                            name: spec.name.clone(),
                            value: raw.clone(),
                        });
                    }
                },
                ArgKind::Choice => {
                    if !spec.choices.iter().any(|choice| choice == raw) {
                        return Err(ResolveError::NotAChoice {
                            name: spec.name.clone(),
                            value: raw.clone(),
                            choices: spec.choices.clone(),
                        });
                    }
                    ArgumentValue::Choice(raw.clone())
                }
            };
            typed.insert(spec.name.clone(), value);
        }
        self.resolve(&typed)
    }
}

impl Serialize for Template {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let source = self.source();
        TemplateWire {
            source: &source,
            arguments: &self.arguments,
        }
        .serialize(serializer)
    }
}

#[derive(Serialize)]
struct TemplateWire<'a> {
    source: &'a str,
    arguments: &'a [ArgSpec],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TemplateRepr {
    source: String,
    arguments: Vec<ArgSpec>,
}

impl<'de> Deserialize<'de> for Template {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let repr = TemplateRepr::deserialize(deserializer)?;
        Self::parse(&repr.source, repr.arguments).map_err(de::Error::custom)
    }
}

fn is_canonical_number(raw: &str) -> bool {
    raw.trim() == raw && ArgumentValue::number(raw).is_ok()
}

fn validate_name(name: &str, position: usize) -> Result<(), TemplateError> {
    if name.chars().count() > MAX_ARGUMENT_NAME_CHARS {
        return Err(TemplateError::NameTooLong {
            limit: MAX_ARGUMENT_NAME_CHARS,
        });
    }
    let mut characters = name.chars();
    match characters.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => {
            return Err(TemplateError::InvalidName {
                name: name.to_owned(),
                position,
            });
        }
    }
    for character in characters {
        if !character.is_ascii_alphanumeric() && character != '_' && character != '-' {
            return Err(TemplateError::InvalidName {
                name: name.to_owned(),
                position,
            });
        }
    }
    Ok(())
}
