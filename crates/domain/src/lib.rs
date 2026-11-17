//! Renderer-independent domain types for the Ansa launcher.
//!
//! This crate owns stable identities and the command/capability vocabulary.
//! It deliberately performs no platform discovery, storage, or rendering.

use std::fmt;
use std::str::FromStr;

/// A stable, namespaced command identity.
///
/// Identities are opaque to the renderer and must never be derived from
/// display text. The canonical textual form is `namespace:local`, for example
/// `app:com.example.Editor`, `quicklink:8f14e45f`, or `builtin:settings`.
///
/// Display names may change freely; this identity may not.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommandId(String);

/// Reasons a [`CommandId`] could not be constructed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandIdError {
    /// The raw identity was empty.
    Empty,
    /// The raw identity did not contain a namespace separator (`:`).
    MissingNamespaceSeparator,
    /// The namespace portion was empty.
    EmptyNamespace,
    /// The local portion was empty.
    EmptyLocal,
    /// The identity contained a control character or whitespace.
    InvalidCharacter { character: char, position: usize },
}

impl fmt::Display for CommandIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("command id must not be empty"),
            Self::MissingNamespaceSeparator => {
                f.write_str("command id must contain a namespace separator ':'")
            }
            Self::EmptyNamespace => f.write_str("command id namespace must not be empty"),
            Self::EmptyLocal => f.write_str("command id local part must not be empty"),
            Self::InvalidCharacter {
                character,
                position,
            } => write!(
                f,
                "command id contains invalid character {character:?} at position {position}"
            ),
        }
    }
}

impl std::error::Error for CommandIdError {}

impl CommandId {
    /// Parses a canonical `namespace:local` identity.
    ///
    /// This is the primary fallible constructor:
    /// `CommandId::new("app:com.example.Editor")?`.
    pub fn new(raw: impl Into<String>) -> Result<Self, CommandIdError> {
        Self::parse(raw)
    }

    /// Builds an identity from namespace and local parts.
    ///
    /// Both parts must be non-empty, and neither may contain `:` or
    /// whitespace/control characters.
    pub fn from_parts(
        namespace: impl AsRef<str>,
        local: impl AsRef<str>,
    ) -> Result<Self, CommandIdError> {
        let namespace = namespace.as_ref();
        let local = local.as_ref();
        if namespace.is_empty() {
            return Err(CommandIdError::EmptyNamespace);
        }
        if local.is_empty() {
            return Err(CommandIdError::EmptyLocal);
        }
        validate_segment(namespace, 0)?;
        validate_segment(local, namespace.chars().count() + 1)?;
        Ok(Self(format!("{namespace}:{local}")))
    }

    /// Parses a canonical `namespace:local` identity.
    pub fn parse(raw: impl Into<String>) -> Result<Self, CommandIdError> {
        let raw = raw.into();
        if raw.is_empty() {
            return Err(CommandIdError::Empty);
        }
        let Some((namespace, local)) = raw.split_once(':') else {
            return Err(CommandIdError::MissingNamespaceSeparator);
        };
        if namespace.is_empty() {
            return Err(CommandIdError::EmptyNamespace);
        }
        if local.is_empty() {
            return Err(CommandIdError::EmptyLocal);
        }
        validate_segment(namespace, 0)?;
        validate_segment(local, namespace.chars().count() + 1)?;
        Ok(Self(raw))
    }

    /// The namespace portion, e.g. `app`.
    pub fn namespace(&self) -> &str {
        self.0
            .split_once(':')
            .expect("CommandId invariant: namespace separator")
            .0
    }

    /// The local, namespace-scoped portion, e.g. `com.example.Editor`.
    pub fn local(&self) -> &str {
        self.0
            .split_once(':')
            .expect("CommandId invariant: namespace separator")
            .1
    }

    /// The canonical `namespace:local` representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The stable identity of a built-in command.
    pub fn builtin(command: BuiltinCommand) -> Self {
        Self(format!("builtin:{}", command.slug()))
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for CommandId {
    type Err = CommandIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<String> for CommandId {
    type Error = CommandIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl TryFrom<&str> for CommandId {
    type Error = CommandIdError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

fn validate_segment(segment: &str, offset: usize) -> Result<(), CommandIdError> {
    for (index, character) in segment.chars().enumerate() {
        if character == ':' || character.is_whitespace() || character.is_control() {
            return Err(CommandIdError::InvalidCharacter {
                character,
                position: offset + index,
            });
        }
    }
    Ok(())
}

/// A built-in, first-party command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BuiltinCommand {
    Settings,
    RefreshApplications,
    About,
}

impl BuiltinCommand {
    /// Every built-in command currently exposed by the core.
    pub const ALL: [Self; 3] = [Self::Settings, Self::RefreshApplications, Self::About];

    /// Stable local slug used in the built-in namespace.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Settings => "settings",
            Self::RefreshApplications => "refresh-applications",
            Self::About => "about",
        }
    }

    /// Human-readable default title.
    pub fn title(self) -> &'static str {
        match self {
            Self::Settings => "Settings",
            Self::RefreshApplications => "Refresh Applications",
            Self::About => "About Ansa",
        }
    }

    /// A stable command descriptor for this built-in.
    pub fn command(self) -> Command {
        Command::new(
            CommandId::builtin(self),
            self.title(),
            CommandKind::Builtin(self),
        )
    }
}

/// How a command is executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandKind {
    /// Launch or switch to a packaged/desktop application.
    Application { path: String },
    /// Open a URL through the host's preferred opener.
    Quicklink { url: String },
    /// Run a reviewed first-party behavior.
    Builtin(BuiltinCommand),
}

impl CommandKind {
    /// Stable, UI-safe category label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Application { .. } => "Application",
            Self::Quicklink { .. } => "Quicklink",
            Self::Builtin(_) => "Built-in",
        }
    }

    /// The concrete target, when this kind has one.
    pub fn target(&self) -> Option<&str> {
        match self {
            Self::Application { path } => Some(path),
            Self::Quicklink { url } => Some(url),
            Self::Builtin(_) => None,
        }
    }
}

/// A searchable command descriptor.
///
/// This is data, not an executable callback. The host resolves the identity
/// and action at invocation time and re-checks target existence and grants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub id: CommandId,
    pub title: String,
    pub subtitle: Option<String>,
    pub keywords: Vec<String>,
    pub alias: Option<String>,
    pub kind: CommandKind,
}

impl Command {
    /// Creates a command with no optional metadata.
    pub fn new(id: CommandId, title: impl Into<String>, kind: CommandKind) -> Self {
        Self {
            id,
            title: title.into(),
            subtitle: None,
            keywords: Vec::new(),
            alias: None,
            kind,
        }
    }

    /// Sets or clears the secondary display line.
    pub fn with_subtitle(mut self, subtitle: impl Into<String>) -> Self {
        let subtitle = subtitle.into();
        self.subtitle = if subtitle.trim().is_empty() {
            None
        } else {
            Some(subtitle)
        };
        self
    }

    /// Replaces the search keywords.
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

    /// Sets or clears the single exact-match alias.
    pub fn with_alias(mut self, alias: impl Into<String>) -> Self {
        let alias = alias.into();
        self.alias = if alias.trim().is_empty() {
            None
        } else {
            Some(alias)
        };
        self
    }
}

/// Truthful availability of a platform or command capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityStatus {
    Available,
    PermissionRequired,
    TemporarilyUnavailable,
    Unsupported,
    /// Usable, but with a disclosed limitation and an explicit fallback.
    Degraded {
        reason: String,
        fallback: String,
    },
}

impl CapabilityStatus {
    /// True only when the full capability is currently available.
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    /// True when the capability can be used at all right now.
    pub fn is_usable(&self) -> bool {
        matches!(self, Self::Available | Self::Degraded { .. })
    }

    /// Whether the user must grant access before use.
    pub fn requires_permission(&self) -> bool {
        matches!(self, Self::PermissionRequired)
    }

    /// User-facing explanation for non-available states.
    pub fn explanation(&self) -> Option<&str> {
        match self {
            Self::Available => None,
            Self::PermissionRequired => Some("Permission required"),
            Self::TemporarilyUnavailable => Some("Temporarily unavailable"),
            Self::Unsupported => Some("Unsupported on this platform"),
            Self::Degraded { reason, .. } => Some(reason),
        }
    }
}

/// A named capability and its current status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    pub name: String,
    pub status: CapabilityStatus,
}

impl Capability {
    pub fn new(name: impl Into<String>, status: CapabilityStatus) -> Self {
        Self {
            name: name.into(),
            status,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_id_round_trips_and_exposes_parts() {
        let id = CommandId::new("app:com.example.Editor").unwrap();
        assert_eq!(id.as_str(), "app:com.example.Editor");
        assert_eq!(id.namespace(), "app");
        assert_eq!(id.local(), "com.example.Editor");
        assert_eq!(id.to_string(), "app:com.example.Editor");

        let parts = CommandId::from_parts("app", "com.example.Editor").unwrap();
        assert_eq!(parts, id);
        assert_eq!(
            CommandId::parse("quicklink:abc").unwrap().namespace(),
            "quicklink"
        );
        assert_eq!(
            CommandId::try_from("builtin:about").unwrap().local(),
            "about"
        );
    }

    #[test]
    fn command_id_rejects_malformed_values() {
        assert_eq!(CommandId::parse(""), Err(CommandIdError::Empty));
        assert_eq!(
            CommandId::parse("settings"),
            Err(CommandIdError::MissingNamespaceSeparator)
        );
        assert_eq!(
            CommandId::parse(":settings"),
            Err(CommandIdError::EmptyNamespace)
        );
        assert_eq!(
            CommandId::parse("builtin:"),
            Err(CommandIdError::EmptyLocal)
        );
        assert_eq!(
            CommandId::parse("builtin:set tings"),
            Err(CommandIdError::InvalidCharacter {
                character: ' ',
                position: 11
            })
        );
    }

    #[test]
    fn command_id_rejects_extra_colons_and_whitespace_segments() {
        assert!(CommandId::from_parts("app", "a:b").is_err());
        assert!(CommandId::from_parts(" app", "x").is_err());
        assert!(CommandId::from_parts("app", "x\n").is_err());
        assert!(CommandId::from_parts("", "x").is_err());
        assert!(CommandId::from_parts("app", "").is_err());
    }

    #[test]
    fn builtin_ids_are_stable() {
        assert_eq!(
            CommandId::builtin(BuiltinCommand::Settings).as_str(),
            "builtin:settings"
        );
        assert_eq!(
            CommandId::builtin(BuiltinCommand::RefreshApplications).as_str(),
            "builtin:refresh-applications"
        );
        assert_eq!(BuiltinCommand::About.title(), "About Ansa");
        assert_eq!(BuiltinCommand::ALL.len(), 3);
    }

    #[test]
    fn command_builders_normalize_empty_optional_text() {
        let command = Command::new(
            CommandId::parse("app:demo").unwrap(),
            "Demo",
            CommandKind::Application {
                path: "demo.exe".into(),
            },
        )
        .with_subtitle("  ")
        .with_keywords(["alpha", " ", "beta"])
        .with_alias("");

        assert_eq!(command.subtitle, None);
        assert_eq!(command.keywords, vec!["alpha", "beta"]);
        assert_eq!(command.alias, None);
        assert_eq!(command.kind.label(), "Application");
        assert_eq!(command.kind.target(), Some("demo.exe"));
    }

    #[test]
    fn capability_status_is_truthful() {
        assert!(CapabilityStatus::Available.is_available());
        assert!(CapabilityStatus::Available.is_usable());
        assert!(!CapabilityStatus::PermissionRequired.is_available());
        assert!(CapabilityStatus::PermissionRequired.requires_permission());
        let degraded = CapabilityStatus::Degraded {
            reason: "Clipboard history requires a supported compositor".into(),
            fallback: "Copy only".into(),
        };
        assert!(!degraded.is_available());
        assert!(degraded.is_usable());
        assert_eq!(
            degraded.explanation(),
            Some("Clipboard history requires a supported compositor")
        );
        let capability = Capability::new("Clipboard", degraded);
        assert_eq!(capability.name, "Clipboard");
    }
}
