//! Shortcut description, normalization and registration probing.
//!
//! The onboarding crate never registers a global shortcut itself. It records a
//! [`ShortcutChoice`] only after a host-supplied [`ShortcutProbe`] reports that
//! the shortcut is available, or after the user explicitly continues with the
//! documented default without registration confirmation.

use std::fmt;
use std::str::FromStr;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A modifier key in the launcher's portable shortcut vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Modifier {
    Ctrl,
    Alt,
    Shift,
    Super,
}

impl Modifier {
    /// Every modifier, in canonical display order.
    pub const ALL: [Self; 4] = [Self::Ctrl, Self::Alt, Self::Shift, Self::Super];

    /// Canonical display label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Ctrl => "Ctrl",
            Self::Alt => "Alt",
            Self::Shift => "Shift",
            Self::Super => "Super",
        }
    }

    fn canonical_index(self) -> usize {
        match self {
            Self::Ctrl => 0,
            Self::Alt => 1,
            Self::Shift => 2,
            Self::Super => 3,
        }
    }
}

impl fmt::Display for Modifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// The non-modifier key of a shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Key {
    /// The space bar.
    Space,
    /// A single ASCII letter or digit.
    Character(char),
    /// A function key from `F1` through `F24`.
    Function(u8),
}

impl Key {
    /// Canonical display label.
    pub fn label(self) -> String {
        match self {
            Self::Space => "Space".to_owned(),
            Self::Character(character) => character.to_ascii_uppercase().to_string(),
            Self::Function(number) => format!("F{number}"),
        }
    }

    fn parse(token: &str) -> Result<Self, ShortcutParseError> {
        if token.eq_ignore_ascii_case("space") {
            return Ok(Self::Space);
        }

        if let Some(number) = token
            .strip_prefix('f')
            .or_else(|| token.strip_prefix('F'))
            .and_then(|digits| digits.parse::<u8>().ok())
        {
            if (1..=24).contains(&number) {
                return Ok(Self::Function(number));
            }
            return Err(ShortcutParseError::UnsupportedKey {
                token: token.to_owned(),
            });
        }

        let mut characters = token.chars();
        if let (Some(character), None) = (characters.next(), characters.next())
            && character.is_ascii_alphanumeric()
        {
            return Ok(Self::Character(character.to_ascii_lowercase()));
        }

        Err(ShortcutParseError::UnsupportedKey {
            token: token.to_owned(),
        })
    }
}

impl fmt::Display for Key {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.label())
    }
}

/// A normalized keyboard shortcut.
///
/// Construction always sorts and de-duplicates modifiers, so two spelling
/// variants of the same chord compare equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Shortcut {
    modifiers: [bool; 4],
    key: Key,
}

impl Shortcut {
    /// The three candidate shortcuts offered during onboarding, in order.
    pub const CANDIDATES: [Self; 3] = [
        Self::from_parts(&[Modifier::Ctrl], Key::Space),
        Self::from_parts(&[Modifier::Alt], Key::Space),
        Self::from_parts(&[Modifier::Ctrl, Modifier::Shift], Key::Space),
    ];

    /// Builds a normalized shortcut from modifiers and a key.
    ///
    /// At least one modifier is required for a global launcher shortcut.
    pub fn new(
        modifiers: impl IntoIterator<Item = Modifier>,
        key: Key,
    ) -> Result<Self, ShortcutParseError> {
        let mut flags = [false; 4];
        for modifier in modifiers {
            flags[modifier.canonical_index()] = true;
        }
        if !flags.iter().any(|present| *present) {
            return Err(ShortcutParseError::MissingModifier);
        }
        Ok(Self {
            modifiers: flags,
            key,
        })
    }

    const fn from_parts(modifiers: &[Modifier], key: Key) -> Self {
        let mut flags = [false; 4];
        let mut index = 0;
        while index < modifiers.len() {
            flags[modifiers[index] as usize] = true;
            index += 1;
        }
        Self {
            modifiers: flags,
            key,
        }
    }

    /// Parses a textual shortcut such as `Ctrl+Space`.
    pub fn parse(raw: &str) -> Result<Self, ShortcutParseError> {
        <Self as FromStr>::from_str(raw)
    }

    /// The documented default, `Ctrl+Space`.
    ///
    /// The host still has to probe it before onboarding calls it registered.
    pub fn documented_default() -> Self {
        Self::CANDIDATES[0]
    }

    /// The active modifiers in canonical order.
    pub fn modifiers(&self) -> Vec<Modifier> {
        Modifier::ALL
            .into_iter()
            .filter(|modifier| self.modifiers[modifier.canonical_index()])
            .collect()
    }

    /// The non-modifier key.
    pub fn key(&self) -> Key {
        self.key
    }

    /// A stable, human-readable form such as `Ctrl+Shift+Space`.
    pub fn display(&self) -> String {
        let mut parts: Vec<String> = self
            .modifiers()
            .into_iter()
            .map(|modifier| modifier.label().to_owned())
            .collect();
        parts.push(self.key.label());
        parts.join("+")
    }
}

impl Default for Shortcut {
    fn default() -> Self {
        Self::documented_default()
    }
}

impl fmt::Display for Shortcut {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.display())
    }
}

impl FromStr for Shortcut {
    type Err = ShortcutParseError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(ShortcutParseError::Empty);
        }

        let mut modifiers = Vec::new();
        let mut key = None;

        for token in raw.split('+') {
            let token = token.trim();
            if token.is_empty() {
                return Err(ShortcutParseError::EmptyToken);
            }

            match token.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => modifiers.push(Modifier::Ctrl),
                "alt" | "opt" | "option" => modifiers.push(Modifier::Alt),
                "shift" => modifiers.push(Modifier::Shift),
                "super" | "meta" | "cmd" | "command" | "win" | "windows" => {
                    modifiers.push(Modifier::Super);
                }
                _ => {
                    if key.is_some() {
                        return Err(ShortcutParseError::MultipleKeys {
                            token: token.to_owned(),
                        });
                    }
                    key = Some(Key::parse(token)?);
                }
            }
        }

        let key = key.ok_or(ShortcutParseError::MissingKey)?;
        Self::new(modifiers, key)
    }
}

impl Serialize for Shortcut {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.display())
    }
}

impl<'de> Deserialize<'de> for Shortcut {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ShortcutVisitor;

        impl Visitor<'_> for ShortcutVisitor {
            type Value = Shortcut;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a shortcut such as Ctrl+Space")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                value.parse().map_err(de::Error::custom)
            }
        }

        deserializer.deserialize_str(ShortcutVisitor)
    }
}

/// How a host confirmed the chosen shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShortcutVerification {
    /// The host probe reported the chord as available.
    Registered,
    /// The user explicitly continued with the documented default without
    /// registration confirmation.
    UnverifiedDefault,
}

impl ShortcutVerification {
    /// True only for a probe-confirmed registration.
    pub fn is_registered(self) -> bool {
        matches!(self, Self::Registered)
    }

    /// User-facing explanation of the verification state.
    pub fn label(self) -> &'static str {
        match self {
            Self::Registered => "Registered",
            Self::UnverifiedDefault => "Using the documented default; registration not confirmed",
        }
    }
}

/// A persisted shortcut together with what was actually verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortcutChoice {
    shortcut: Shortcut,
    verification: ShortcutVerification,
}

impl ShortcutChoice {
    pub(crate) fn new(shortcut: Shortcut, verification: ShortcutVerification) -> Self {
        Self {
            shortcut,
            verification,
        }
    }

    /// The selected shortcut.
    pub fn shortcut(&self) -> Shortcut {
        self.shortcut
    }

    /// What the host confirmed, if anything.
    pub fn verification(&self) -> ShortcutVerification {
        self.verification
    }

    /// True only when the probe confirmed the shortcut is registered.
    pub fn is_registered(&self) -> bool {
        self.verification.is_registered()
    }
}

/// The result of asking the host about a shortcut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// The shortcut is registered and usable.
    Available,
    /// Another application or the desktop already owns the chord.
    Conflict { source: String },
    /// The platform has no global-shortcut facility.
    Unsupported,
    /// The operating system or policy denied registration.
    Denied { reason: String },
}

impl ProbeOutcome {
    /// True only for [`ProbeOutcome::Available`].
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    /// True when another candidate could plausibly succeed.
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::Conflict { .. } | Self::Unsupported | Self::Denied { .. }
        )
    }

    /// Plain-language explanation for a non-available outcome.
    pub fn explanation(&self) -> Option<String> {
        match self {
            Self::Available => None,
            Self::Conflict { source } => Some(format!("{source} is already using this shortcut")),
            Self::Unsupported => {
                Some("Global shortcuts are not supported on this platform".to_owned())
            }
            Self::Denied { reason } => Some(format!("The system denied the shortcut: {reason}")),
        }
    }
}

/// Host adapter that checks whether a global shortcut can be registered.
pub trait ShortcutProbe {
    /// Probes the shortcut without claiming registration on failure.
    fn probe(&self, shortcut: &Shortcut) -> ProbeOutcome;
}

/// Reasons a textual shortcut could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortcutParseError {
    Empty,
    EmptyToken,
    MissingModifier,
    MissingKey,
    MultipleKeys { token: String },
    UnsupportedKey { token: String },
}

impl fmt::Display for ShortcutParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("shortcut must not be empty"),
            Self::EmptyToken => formatter.write_str("shortcut contains an empty key token"),
            Self::MissingModifier => {
                formatter.write_str("a global shortcut requires at least one modifier")
            }
            Self::MissingKey => formatter.write_str("shortcut is missing a non-modifier key"),
            Self::MultipleKeys { token } => {
                write!(
                    formatter,
                    "shortcut has more than one key; unexpected {token:?}"
                )
            }
            Self::UnsupportedKey { token } => {
                write!(formatter, "unsupported shortcut key {token:?}")
            }
        }
    }
}

impl std::error::Error for ShortcutParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing_normalizes_modifier_order_and_aliases() {
        let canonical = Shortcut::parse("ctrl+shift+space").unwrap();
        let alias = Shortcut::parse("  SHIFT + Control + Space  ").unwrap();
        let windows = Shortcut::parse("shift+control+space").unwrap();

        assert_eq!(canonical, alias);
        assert_eq!(canonical, windows);
        assert_eq!(canonical.display(), "Ctrl+Shift+Space");
        assert_eq!(canonical.modifiers(), vec![Modifier::Ctrl, Modifier::Shift]);
        assert_eq!(canonical.key(), Key::Space);
    }

    #[test]
    fn duplicate_modifiers_are_ignored() {
        let shortcut = Shortcut::parse("Ctrl+Ctrl+Alt+Space").unwrap();
        assert_eq!(shortcut.modifiers(), vec![Modifier::Ctrl, Modifier::Alt]);
        assert_eq!(shortcut.display(), "Ctrl+Alt+Space");
    }

    #[test]
    fn shortcut_display_round_trips_through_serde() {
        let shortcut = Shortcut::parse("Ctrl+Shift+Space").unwrap();
        let json = serde_json::to_string(&shortcut).unwrap();
        assert_eq!(json, "\"Ctrl+Shift+Space\"");
        let parsed: Shortcut = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, shortcut);
    }

    #[test]
    fn invalid_shortcuts_are_rejected() {
        assert_eq!("".parse::<Shortcut>(), Err(ShortcutParseError::Empty));
        assert_eq!(
            "Ctrl+".parse::<Shortcut>(),
            Err(ShortcutParseError::EmptyToken)
        );
        assert_eq!(
            "Space".parse::<Shortcut>(),
            Err(ShortcutParseError::MissingModifier)
        );
        assert_eq!(
            "Ctrl".parse::<Shortcut>(),
            Err(ShortcutParseError::MissingKey)
        );
        assert!(matches!(
            "Ctrl+A+B".parse::<Shortcut>(),
            Err(ShortcutParseError::MultipleKeys { .. })
        ));
        assert!(matches!(
            "Ctrl+F25".parse::<Shortcut>(),
            Err(ShortcutParseError::UnsupportedKey { .. })
        ));
    }

    #[test]
    fn candidates_are_the_documented_three_and_first_is_default() {
        assert_eq!(Shortcut::CANDIDATES.len(), 3);
        assert_eq!(Shortcut::CANDIDATES[0].display(), "Ctrl+Space");
        assert_eq!(Shortcut::CANDIDATES[1].display(), "Alt+Space");
        assert_eq!(Shortcut::CANDIDATES[2].display(), "Ctrl+Shift+Space");
        assert_eq!(Shortcut::default(), Shortcut::documented_default());
    }
}
