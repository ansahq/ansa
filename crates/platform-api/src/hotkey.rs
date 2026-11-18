//! Platform-neutral global-hotkey contract.
//!
//! This module is deliberately dependency-free and has no OS coupling. It
//! defines:
//!
//! * [`KeyChord`] — a normalized modifier set plus one non-modifier key, with a
//!   stable persisted grammar and per-platform native names.
//! * [`RegistrationOutcome`] — an explicit, typed result for every registration
//!   attempt; there is no "silently succeed" path.
//! * [`HotkeyConfig`] / [`HotkeyStore`] — a small, versioned text model with
//!   validation, a backup copy and atomic replacement.
//! * [`HotkeyManager`] — the register/unregister/rebind/suspend/resume/shutdown
//!   state machine over a [`HotkeyBackend`] (a fake backend is used in tests).
//! * [`ShortcutRecorder`] — UI-independent capture, verification and commit
//!   logic. A binding cannot be committed until it has been verified.
//!
//! The Windows adapter lives in `ansa-platform-windows`; on non-Windows hosts
//! no native adapter is claimed. The Windows adapter has its own copy of the
//! chord grammar because the two crates cannot depend on each other (the API
//! crate already depends on the adapter on Windows). A parity test in this
//! module compares the two parsers and native renderings.
//!
//! # Persisted form
//!
//! A chord is rendered as lowercase modifier names in the fixed order
//! `ctrl`, `alt`, `shift`, `meta`, followed by one key token, joined by `+`.
//! Examples: `ctrl+space`, `ctrl+alt+shift+win` is *rendered* natively as
//! `Ctrl+Alt+Shift+Win` but persisted as `ctrl+alt+shift+meta+a`.
//!
//! Supported key tokens:
//!
//! * letters `a`–`z`;
//! * digits `0`–`9`;
//! * function keys `f1`–`f24`;
//! * `space`, `enter` (`return`), `tab`, `escape` (`esc`), `backspace`,
//!   `delete` (`del`), `insert` (`ins`), `home`, `end`, `pageup` (`pgup`),
//!   `pagedown` (`pgdn`), `up`, `down`, `left`, `right`;
//! * punctuation `-`, `=`, `[`, `]`, `\`, `;`, `'`, `,`, `.`, `/`, `` ` ``
//!   (names such as `minus`, `equals`, `leftbracket`, `semicolon`, `quote`,
//!   `comma`, `period`, `slash` and `backtick` are accepted on input).
//!
//! `+` is the separator and is therefore not a key token. Empty chords,
//! modifier-only chords, duplicate modifiers and multiple keys are rejected
//! with [`ChordParseError`].

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::{CapabilityStatus, Platform};

/// The default summon chord used when no valid persisted binding exists.
///
/// This is a development default, not a completed onboarding choice. The UI
/// must present it as a default and offer rebinding.
pub const DEFAULT_SUMMON_CHORD: &str = "ctrl+space";

/// Version written to and accepted from the hotkey configuration file.
pub const HOTKEY_CONFIG_VERSION: u32 = 1;

/// Header token of the hotkey configuration file.
pub const HOTKEY_CONFIG_HEADER: &str = "ansa-hotkeys";

/// The action name used for the root launcher summon binding.
pub const SUMMON_ACTION: &str = "summon";

// ---------------------------------------------------------------------------
// Modifiers
// ---------------------------------------------------------------------------

/// One keyboard modifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Modifier {
    /// Control on Windows/Linux, Control on macOS.
    Control,
    /// Alt on Windows/Linux, Option on macOS.
    Alt,
    /// Shift.
    Shift,
    /// Windows key, Command on macOS, Super on Linux.
    Meta,
}

impl Modifier {
    /// All modifiers in persisted order.
    pub const ALL: [Self; 4] = [Self::Control, Self::Alt, Self::Shift, Self::Meta];

    /// Returns the stable lowercase persisted name.
    pub fn persisted_name(self) -> &'static str {
        match self {
            Self::Control => "ctrl",
            Self::Alt => "alt",
            Self::Shift => "shift",
            Self::Meta => "meta",
        }
    }

    /// Returns the native name for the platform.
    pub fn native_name(self, platform: Platform) -> &'static str {
        match (self, platform) {
            (Self::Control, Platform::MacOs) => "Control",
            (Self::Control, _) => "Ctrl",
            (Self::Alt, Platform::MacOs) => "Option",
            (Self::Alt, _) => "Alt",
            (Self::Shift, _) => "Shift",
            (Self::Meta, Platform::MacOs) => "Command",
            (Self::Meta, Platform::Linux) => "Super",
            (Self::Meta, Platform::Windows) => "Win",
            (Self::Meta, Platform::Other) => "Meta",
        }
    }

    fn parse_lower(token: &str) -> Option<Self> {
        match token {
            "ctrl" | "control" => Some(Self::Control),
            "alt" | "option" | "opt" => Some(Self::Alt),
            "shift" => Some(Self::Shift),
            "meta" | "super" | "win" | "windows" | "cmd" | "command" => Some(Self::Meta),
            _ => None,
        }
    }
}

/// A normalized set of modifiers.
///
/// The set cannot contain duplicates. Iteration follows the persisted order
/// Control, Alt, Shift, Meta.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers(u8);

impl Modifiers {
    const CONTROL: u8 = 1 << 0;
    const ALT: u8 = 1 << 1;
    const SHIFT: u8 = 1 << 2;
    const META: u8 = 1 << 3;

    /// An empty modifier set.
    pub const fn empty() -> Self {
        Self(0)
    }

    /// A set containing only Control.
    pub const fn control() -> Self {
        Self(Self::CONTROL)
    }

    /// Returns `true` when no modifier is present.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Returns `true` when `modifier` is present.
    pub const fn contains(self, modifier: Modifier) -> bool {
        self.0 & Self::bit(modifier) != 0
    }

    /// Adds `modifier`, returning the new set.
    pub const fn with(self, modifier: Modifier) -> Self {
        Self(self.0 | Self::bit(modifier))
    }

    /// Adds `modifier` in place.
    pub fn insert(&mut self, modifier: Modifier) {
        *self = self.with(modifier);
    }

    /// Removes `modifier` in place.
    pub fn remove(&mut self, modifier: Modifier) {
        *self = Self(self.0 & !Self::bit(modifier));
    }

    /// Returns the number of modifiers in the set.
    pub const fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    /// Iterates modifiers in persisted order.
    pub fn iter(self) -> impl Iterator<Item = Modifier> + 'static {
        Modifier::ALL
            .into_iter()
            .filter(move |modifier| self.contains(*modifier))
    }

    /// Renders the modifiers with native names, joined by `+`.
    ///
    /// An empty set renders as an empty string.
    pub fn render(self, platform: Platform) -> String {
        self.iter()
            .map(|modifier| modifier.native_name(platform))
            .collect::<Vec<_>>()
            .join("+")
    }

    /// Renders the modifiers in stable persisted form.
    pub fn persisted(self) -> String {
        self.iter()
            .map(|modifier| modifier.persisted_name())
            .collect::<Vec<_>>()
            .join("+")
    }

    const fn bit(modifier: Modifier) -> u8 {
        match modifier {
            Modifier::Control => Self::CONTROL,
            Modifier::Alt => Self::ALT,
            Modifier::Shift => Self::SHIFT,
            Modifier::Meta => Self::META,
        }
    }
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum KeyKind {
    Letter(char),
    Digit(u8),
    Function(u8),
    Space,
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    Punctuation(char),
}

/// A normalized non-modifier key.
///
/// Construct values with [`Key::letter`], [`Key::digit`], [`Key::function`],
/// [`Key::punctuation`] or the named constants such as [`Key::SPACE`]. The
/// private representation prevents invalid lowercase letters or out-of-range
/// function keys from entering a chord.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key(KeyKind);

impl Key {
    /// Space.
    pub const SPACE: Self = Self(KeyKind::Space);
    /// Enter/Return.
    pub const ENTER: Self = Self(KeyKind::Enter);
    /// Tab.
    pub const TAB: Self = Self(KeyKind::Tab);
    /// Escape.
    pub const ESCAPE: Self = Self(KeyKind::Escape);
    /// Backspace.
    pub const BACKSPACE: Self = Self(KeyKind::Backspace);
    /// Delete.
    pub const DELETE: Self = Self(KeyKind::Delete);
    /// Insert.
    pub const INSERT: Self = Self(KeyKind::Insert);
    /// Home.
    pub const HOME: Self = Self(KeyKind::Home);
    /// End.
    pub const END: Self = Self(KeyKind::End);
    /// Page Up.
    pub const PAGE_UP: Self = Self(KeyKind::PageUp);
    /// Page Down.
    pub const PAGE_DOWN: Self = Self(KeyKind::PageDown);
    /// Arrow Up.
    pub const UP: Self = Self(KeyKind::Up);
    /// Arrow Down.
    pub const DOWN: Self = Self(KeyKind::Down);
    /// Arrow Left.
    pub const LEFT: Self = Self(KeyKind::Left);
    /// Arrow Right.
    pub const RIGHT: Self = Self(KeyKind::Right);

    /// Creates a letter key from an ASCII letter, normalized to uppercase.
    pub fn letter(character: char) -> Option<Self> {
        if character.is_ascii_alphabetic() {
            Some(Self(KeyKind::Letter(character.to_ascii_uppercase())))
        } else {
            None
        }
    }

    /// Creates a digit key from an ASCII digit.
    pub fn digit(character: char) -> Option<Self> {
        character
            .to_digit(10)
            .filter(|_| character.is_ascii_digit())
            .map(|digit| Self(KeyKind::Digit(digit as u8)))
    }

    /// Creates a function key for `1..=24`.
    pub fn function(number: u8) -> Option<Self> {
        (1..=24)
            .contains(&number)
            .then_some(Self(KeyKind::Function(number)))
    }

    /// Creates a punctuation key from the supported set.
    pub fn punctuation(character: char) -> Option<Self> {
        PUNCTUATION
            .contains(&character)
            .then_some(Self(KeyKind::Punctuation(character)))
    }

    /// Parses one lowercase key token.
    fn parse_lower(token: &str) -> Option<Self> {
        let named = match token {
            "space" => Some(Self::SPACE),
            "enter" | "return" => Some(Self::ENTER),
            "tab" => Some(Self::TAB),
            "escape" | "esc" => Some(Self::ESCAPE),
            "backspace" => Some(Self::BACKSPACE),
            "delete" | "del" => Some(Self::DELETE),
            "insert" | "ins" => Some(Self::INSERT),
            "home" => Some(Self::HOME),
            "end" => Some(Self::END),
            "pageup" | "pgup" => Some(Self::PAGE_UP),
            "pagedown" | "pgdn" => Some(Self::PAGE_DOWN),
            "up" | "arrowup" => Some(Self::UP),
            "down" | "arrowdown" => Some(Self::DOWN),
            "left" | "arrowleft" => Some(Self::LEFT),
            "right" | "arrowright" => Some(Self::RIGHT),
            _ => None,
        };
        if named.is_some() {
            return named;
        }

        if let Some(name) = token.strip_prefix('f') {
            if let Ok(number) = name.parse::<u8>() {
                return Self::function(number);
            }
        }

        match token {
            "minus" => return Some(Self(KeyKind::Punctuation('-'))),
            "equals" | "equal" => return Some(Self(KeyKind::Punctuation('='))),
            "leftbracket" => return Some(Self(KeyKind::Punctuation('['))),
            "rightbracket" => return Some(Self(KeyKind::Punctuation(']'))),
            "backslash" => return Some(Self(KeyKind::Punctuation('\\'))),
            "semicolon" => return Some(Self(KeyKind::Punctuation(';'))),
            "quote" => return Some(Self(KeyKind::Punctuation('\''))),
            "comma" => return Some(Self(KeyKind::Punctuation(','))),
            "period" => return Some(Self(KeyKind::Punctuation('.'))),
            "slash" => return Some(Self(KeyKind::Punctuation('/'))),
            "backtick" => return Some(Self(KeyKind::Punctuation('`'))),
            _ => {}
        }

        let mut characters = token.chars();
        if let (Some(character), None) = (characters.next(), characters.next()) {
            if let Some(key) = Self::letter(character) {
                return Some(key);
            }
            if let Some(key) = Self::digit(character) {
                return Some(key);
            }
            if let Some(key) = Self::punctuation(character) {
                return Some(key);
            }
        }

        None
    }

    /// Stable lowercase persisted token.
    pub fn persisted_name(self) -> String {
        match self.0 {
            KeyKind::Letter(character) => character.to_ascii_lowercase().to_string(),
            KeyKind::Digit(digit) => digit.to_string(),
            KeyKind::Function(number) => format!("f{number}"),
            KeyKind::Space => "space".to_owned(),
            KeyKind::Enter => "enter".to_owned(),
            KeyKind::Tab => "tab".to_owned(),
            KeyKind::Escape => "escape".to_owned(),
            KeyKind::Backspace => "backspace".to_owned(),
            KeyKind::Delete => "delete".to_owned(),
            KeyKind::Insert => "insert".to_owned(),
            KeyKind::Home => "home".to_owned(),
            KeyKind::End => "end".to_owned(),
            KeyKind::PageUp => "pageup".to_owned(),
            KeyKind::PageDown => "pagedown".to_owned(),
            KeyKind::Up => "up".to_owned(),
            KeyKind::Down => "down".to_owned(),
            KeyKind::Left => "left".to_owned(),
            KeyKind::Right => "right".to_owned(),
            KeyKind::Punctuation(character) => character.to_string(),
        }
    }

    /// Native display name for the platform.
    pub fn native_name(self, _platform: Platform) -> String {
        match self.0 {
            KeyKind::Letter(character) => character.to_ascii_uppercase().to_string(),
            KeyKind::Digit(digit) => digit.to_string(),
            KeyKind::Function(number) => format!("F{number}"),
            KeyKind::Space => "Space".to_owned(),
            KeyKind::Enter => "Enter".to_owned(),
            KeyKind::Tab => "Tab".to_owned(),
            KeyKind::Escape => "Escape".to_owned(),
            KeyKind::Backspace => "Backspace".to_owned(),
            KeyKind::Delete => "Delete".to_owned(),
            KeyKind::Insert => "Insert".to_owned(),
            KeyKind::Home => "Home".to_owned(),
            KeyKind::End => "End".to_owned(),
            KeyKind::PageUp => "PageUp".to_owned(),
            KeyKind::PageDown => "PageDown".to_owned(),
            KeyKind::Up => "Up".to_owned(),
            KeyKind::Down => "Down".to_owned(),
            KeyKind::Left => "Left".to_owned(),
            KeyKind::Right => "Right".to_owned(),
            KeyKind::Punctuation(character) => punctuation_name(character).to_owned(),
        }
    }
}

const PUNCTUATION: [char; 11] = ['-', '=', '[', ']', '\\', ';', '\'', ',', '.', '/', '`'];

fn punctuation_name(character: char) -> &'static str {
    match character {
        '-' => "Minus",
        '=' => "Equals",
        '[' => "LeftBracket",
        ']' => "RightBracket",
        '\\' => "Backslash",
        ';' => "Semicolon",
        '\'' => "Quote",
        ',' => "Comma",
        '.' => "Period",
        '/' => "Slash",
        '`' => "Backtick",
        _ => "Punctuation",
    }
}

// ---------------------------------------------------------------------------
// Chords
// ---------------------------------------------------------------------------

/// A normalized global-hotkey chord.
///
/// A chord always has exactly one non-modifier key. Use [`KeyChord::parse`] for
/// the persisted grammar or [`KeyChord::new`] with already-validated parts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyChord {
    modifiers: Modifiers,
    key: Key,
}

/// Alias used by callers that prefer the shorter name.
pub type Hotkey = KeyChord;

impl KeyChord {
    /// Creates a chord from a modifier set and one key.
    pub fn new(modifiers: Modifiers, key: Key) -> Self {
        Self { modifiers, key }
    }

    /// Parses the stable persisted grammar.
    ///
    /// Parsing is case-insensitive and tolerates spaces around `+`, but the
    /// canonical output is lowercase with no spaces.
    pub fn parse(input: &str) -> Result<Self, ChordParseError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(ChordParseError::Empty);
        }

        let mut modifiers = Modifiers::empty();
        let mut key = None;

        for raw in trimmed.split('+') {
            let token = raw.trim();
            if token.is_empty() {
                return Err(ChordParseError::EmptyToken);
            }
            let lower = token.to_ascii_lowercase();
            if let Some(modifier) = Modifier::parse_lower(&lower) {
                if modifiers.contains(modifier) {
                    return Err(ChordParseError::DuplicateModifier(modifier));
                }
                modifiers.insert(modifier);
                continue;
            }
            if let Some(parsed) = Key::parse_lower(&lower) {
                if key.is_some() {
                    return Err(ChordParseError::MultipleKeys);
                }
                key = Some(parsed);
                continue;
            }
            return Err(ChordParseError::UnsupportedToken(token.to_owned()));
        }

        let key = key.ok_or(ChordParseError::ModifierOnly)?;
        Ok(Self { modifiers, key })
    }

    /// Returns the modifier set.
    pub fn modifiers(self) -> Modifiers {
        self.modifiers
    }

    /// Returns the non-modifier key.
    pub fn key(self) -> Key {
        self.key
    }

    /// Stable lowercase persisted form.
    pub fn persisted(self) -> String {
        let modifiers = self.modifiers.persisted();
        if modifiers.is_empty() {
            self.key.persisted_name()
        } else {
            format!("{modifiers}+{}", self.key.persisted_name())
        }
    }

    /// Renders native modifier and key names for `platform`.
    pub fn render(self, platform: Platform) -> String {
        let modifiers = self.modifiers.render(platform);
        if modifiers.is_empty() {
            self.key.native_name(platform)
        } else {
            format!("{modifiers}+{}", self.key.native_name(platform))
        }
    }

    /// Returns the documented default summon chord.
    pub fn default_summon() -> Self {
        Self {
            modifiers: Modifiers::control(),
            key: Key::SPACE,
        }
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.persisted())
    }
}

impl std::str::FromStr for KeyChord {
    type Err = ChordParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

/// A typed chord validation error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChordParseError {
    /// The input was empty.
    Empty,
    /// A `+` separated token was empty.
    EmptyToken,
    /// Modifiers without a non-modifier key.
    ModifierOnly,
    /// The same modifier appeared more than once.
    DuplicateModifier(Modifier),
    /// More than one non-modifier key was supplied.
    MultipleKeys,
    /// The token is neither a known modifier nor a supported key.
    UnsupportedToken(String),
}

impl fmt::Display for ChordParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("a hotkey chord must not be empty"),
            Self::EmptyToken => formatter.write_str(
                "a hotkey chord contains an empty '+' token; '+' is a separator and is not a key",
            ),
            Self::ModifierOnly => {
                formatter.write_str("a hotkey chord must include a non-modifier key")
            }
            Self::DuplicateModifier(modifier) => {
                write!(
                    formatter,
                    "modifier '{}' appears more than once",
                    modifier.persisted_name()
                )
            }
            Self::MultipleKeys => {
                formatter.write_str("a hotkey chord must contain exactly one non-modifier key")
            }
            Self::UnsupportedToken(token) => {
                write!(formatter, "unsupported key or modifier '{token}'")
            }
        }
    }
}

impl std::error::Error for ChordParseError {}

// ---------------------------------------------------------------------------
// Registration outcomes
// ---------------------------------------------------------------------------

/// The typed result of one registration or probe attempt.
///
/// `Registered` and `Degraded` both mean the chord is active; the other
/// variants are explicit failures. There is intentionally no `Failed` variant
/// that loses the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistrationOutcome {
    /// The chord is registered and active.
    Registered {
        /// The active chord.
        chord: KeyChord,
    },
    /// Another application, the IME or this process already owns the chord.
    Conflict {
        /// The attempted chord.
        chord: KeyChord,
        /// Platform detail, safe to show to the user.
        detail: String,
    },
    /// The host denied the registration (for example an access-denied error).
    Denied {
        /// The attempted chord.
        chord: KeyChord,
        /// Platform detail.
        detail: String,
    },
    /// No native adapter exists, or the adapter cannot express this chord.
    Unsupported {
        /// The attempted chord.
        chord: KeyChord,
        /// What the user can do instead.
        fallback: String,
    },
    /// The chord is active but with a documented limitation.
    Degraded {
        /// The active chord.
        chord: KeyChord,
        /// Why the registration is degraded.
        reason: String,
        /// What the caller or user can do instead.
        alternative: String,
    },
}

/// Alias for callers that refer to the status rather than the operation.
pub type HotkeyStatus = RegistrationOutcome;

impl RegistrationOutcome {
    /// Returns the attempted or active chord.
    pub fn chord(&self) -> KeyChord {
        match self {
            Self::Registered { chord }
            | Self::Conflict { chord, .. }
            | Self::Denied { chord, .. }
            | Self::Unsupported { chord, .. }
            | Self::Degraded { chord, .. } => *chord,
        }
    }

    /// Returns `true` only for outcomes where the chord is active.
    pub fn is_registered(&self) -> bool {
        matches!(self, Self::Registered { .. } | Self::Degraded { .. })
    }

    /// Maps the outcome to the host capability vocabulary.
    pub fn capability_status(&self) -> CapabilityStatus {
        match self {
            Self::Registered { .. } => CapabilityStatus::Available,
            Self::Conflict { chord, detail } => CapabilityStatus::Degraded {
                reason: format!("the default {chord} summon chord is unavailable: {detail}"),
                alternative:
                    "Rebind the summon shortcut, use a desktop shortcut, or run the desktop app directly."
                        .to_owned(),
            },
            Self::Denied { .. } => CapabilityStatus::PermissionRequired,
            Self::Unsupported { .. } => CapabilityStatus::Unsupported,
            Self::Degraded {
                reason,
                alternative,
                ..
            } => CapabilityStatus::Degraded {
                reason: reason.clone(),
                alternative: alternative.clone(),
            },
        }
    }
}

impl fmt::Display for RegistrationOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registered { chord } => write!(formatter, "registered {chord}"),
            Self::Conflict { chord, detail } => {
                write!(formatter, "conflict for {chord}: {detail}")
            }
            Self::Denied { chord, detail } => {
                write!(formatter, "registration denied for {chord}: {detail}")
            }
            Self::Unsupported { chord, fallback } => {
                write!(formatter, "{chord} is unsupported: {fallback}")
            }
            Self::Degraded {
                chord,
                reason,
                alternative,
            } => write!(
                formatter,
                "{chord} registered with a limitation: {reason}; alternative: {alternative}"
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Configuration model and persistence
// ---------------------------------------------------------------------------

/// One action-to-chord binding in the configuration file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotkeyBinding {
    /// Stable action name. `summon` is the root launcher binding; future
    /// per-command bindings use `command:<id>`.
    pub action: String,
    /// The chord bound to the action.
    pub chord: KeyChord,
}

impl HotkeyBinding {
    /// Creates a binding.
    pub fn new(action: impl Into<String>, chord: KeyChord) -> Self {
        Self {
            action: action.into(),
            chord,
        }
    }
}

/// Versioned hotkey configuration.
///
/// The only required binding is [`SUMMON_ACTION`]. Unknown actions are
/// preserved (so a newer file is not silently destroyed) but must use the
/// documented action-name alphabet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotkeyConfig {
    version: u32,
    bindings: Vec<HotkeyBinding>,
}

impl HotkeyConfig {
    /// Creates the documented default: version 1 with `summon ctrl+space`.
    pub fn default_summon() -> Self {
        Self {
            version: HOTKEY_CONFIG_VERSION,
            bindings: vec![HotkeyBinding::new(
                SUMMON_ACTION,
                KeyChord::default_summon(),
            )],
        }
    }

    /// Creates a configuration with one summon binding.
    pub fn with_summon(chord: KeyChord) -> Self {
        Self {
            version: HOTKEY_CONFIG_VERSION,
            bindings: vec![HotkeyBinding::new(SUMMON_ACTION, chord)],
        }
    }

    /// Returns the file format version.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Returns all bindings in file order.
    pub fn bindings(&self) -> &[HotkeyBinding] {
        &self.bindings
    }

    /// Returns the chord for an action.
    pub fn chord_for(&self, action: &str) -> Option<KeyChord> {
        self.bindings
            .iter()
            .find(|binding| binding.action == action)
            .map(|binding| binding.chord)
    }

    /// Returns the root summon chord.
    pub fn summon_chord(&self) -> Option<KeyChord> {
        self.chord_for(SUMMON_ACTION)
    }

    /// Replaces or inserts the root summon binding.
    pub fn set_summon(&mut self, chord: KeyChord) {
        if let Some(binding) = self
            .bindings
            .iter_mut()
            .find(|binding| binding.action == SUMMON_ACTION)
        {
            binding.chord = chord;
        } else {
            self.bindings.push(HotkeyBinding::new(SUMMON_ACTION, chord));
        }
    }

    /// Validates the model before rendering or saving.
    pub fn validate(&self) -> Result<(), HotkeyConfigError> {
        if self.version != HOTKEY_CONFIG_VERSION {
            return Err(HotkeyConfigError::UnsupportedVersion {
                version: self.version,
            });
        }
        if self.bindings.is_empty() {
            return Err(HotkeyConfigError::Empty);
        }

        let mut seen: Vec<&str> = Vec::with_capacity(self.bindings.len());
        for binding in &self.bindings {
            if !valid_action(&binding.action) {
                return Err(HotkeyConfigError::InvalidAction {
                    action: binding.action.clone(),
                });
            }
            if seen.iter().any(|action| *action == binding.action) {
                return Err(HotkeyConfigError::DuplicateBinding {
                    action: binding.action.clone(),
                });
            }
            seen.push(&binding.action);
        }

        if self.summon_chord().is_none() {
            return Err(HotkeyConfigError::MissingSummon);
        }
        Ok(())
    }

    /// Parses the stable text format.
    ///
    /// Blank lines and lines whose first non-space character is `#` are
    /// ignored. Errors are typed and include a 1-based line number where one is
    /// available. This function never panics on malformed input.
    pub fn parse(text: &str) -> Result<Self, HotkeyConfigError> {
        let mut lines = text.lines().enumerate().filter(|(_, line)| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        });

        let Some((_, header)) = lines.next() else {
            return Err(HotkeyConfigError::Empty);
        };
        let header_tokens: Vec<&str> = header.split_whitespace().collect();
        if header_tokens.len() != 2 || header_tokens[0] != HOTKEY_CONFIG_HEADER {
            return Err(HotkeyConfigError::MissingHeader);
        }
        let version =
            header_tokens[1]
                .parse::<u32>()
                .map_err(|_| HotkeyConfigError::InvalidHeader {
                    detail: format!("'{}' is not a version number", header_tokens[1]),
                })?;
        if version != HOTKEY_CONFIG_VERSION {
            return Err(HotkeyConfigError::UnsupportedVersion { version });
        }

        let mut bindings = Vec::new();
        for (index, line) in lines {
            let line_number = index + 1;
            let tokens: Vec<&str> = line.split_whitespace().collect();
            if tokens.len() != 2 {
                return Err(HotkeyConfigError::InvalidLine {
                    line: line_number,
                    detail: "expected '<action> <chord>'".to_owned(),
                });
            }
            let action = tokens[0];
            if !valid_action(action) {
                return Err(HotkeyConfigError::InvalidAction {
                    action: action.to_owned(),
                });
            }
            let chord =
                KeyChord::parse(tokens[1]).map_err(|error| HotkeyConfigError::InvalidChord {
                    line: line_number,
                    chord: tokens[1].to_owned(),
                    source: error,
                })?;
            if bindings
                .iter()
                .any(|binding: &HotkeyBinding| binding.action == action)
            {
                return Err(HotkeyConfigError::DuplicateBinding {
                    action: action.to_owned(),
                });
            }
            bindings.push(HotkeyBinding::new(action, chord));
        }

        let config = Self { version, bindings };
        config.validate()?;
        Ok(config)
    }

    /// Renders the stable text format.
    pub fn render(&self) -> String {
        let mut output = String::new();
        output.push_str(HOTKEY_CONFIG_HEADER);
        output.push(' ');
        output.push_str(&self.version.to_string());
        output.push('\n');
        for binding in &self.bindings {
            output.push_str(&binding.action);
            output.push(' ');
            output.push_str(&binding.chord.persisted());
            output.push('\n');
        }
        output
    }
}

fn valid_action(action: &str) -> bool {
    !action.is_empty()
        && action.len() <= 64
        && action
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'.' | b'_' | b'-'))
}

/// A typed configuration parse/validation error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyConfigError {
    /// No header and no bindings.
    Empty,
    /// The first meaningful line is not `ansa-hotkeys <version>`.
    MissingHeader,
    /// The header version token is malformed.
    InvalidHeader {
        /// Detail for diagnostics.
        detail: String,
    },
    /// A version other than [`HOTKEY_CONFIG_VERSION`].
    UnsupportedVersion {
        /// The unsupported version.
        version: u32,
    },
    /// A binding line has the wrong shape.
    InvalidLine {
        /// 1-based line number.
        line: usize,
        /// Detail for diagnostics.
        detail: String,
    },
    /// The action name contains unsupported characters.
    InvalidAction {
        /// The rejected action.
        action: String,
    },
    /// Two lines bind the same action.
    DuplicateBinding {
        /// The duplicated action.
        action: String,
    },
    /// A chord could not be parsed.
    InvalidChord {
        /// 1-based line number.
        line: usize,
        /// The raw chord token.
        chord: String,
        /// The underlying reason.
        source: ChordParseError,
    },
    /// No `summon` binding was present.
    MissingSummon,
}

impl fmt::Display for HotkeyConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("the hotkey configuration is empty"),
            Self::MissingHeader => formatter.write_str(
                "the hotkey configuration must start with 'ansa-hotkeys <version>'",
            ),
            Self::InvalidHeader { detail } => {
                write!(formatter, "invalid hotkey configuration header: {detail}")
            }
            Self::UnsupportedVersion { version } => write!(
                formatter,
                "hotkey configuration version {version} is not supported (expected {HOTKEY_CONFIG_VERSION})"
            ),
            Self::InvalidLine { line, detail } => {
                write!(formatter, "line {line}: {detail}")
            }
            Self::InvalidAction { action } => {
                write!(formatter, "invalid hotkey action name '{action}'")
            }
            Self::DuplicateBinding { action } => {
                write!(formatter, "action '{action}' is bound more than once")
            }
            Self::InvalidChord {
                line,
                chord,
                source,
            } => write!(formatter, "line {line}: invalid chord '{chord}': {source}"),
            Self::MissingSummon => {
                formatter.write_str("the hotkey configuration has no 'summon' binding")
            }
        }
    }
}

impl std::error::Error for HotkeyConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidChord { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// The result of a recoverable configuration load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotkeyConfigLoad {
    /// The usable configuration (the last good backup or the documented
    /// default when no valid file exists).
    pub config: HotkeyConfig,
    /// Why a fallback was used. `None` means the primary file was valid.
    pub recovery: Option<HotkeyRecovery>,
}

/// Why a configuration load fell back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotkeyRecovery {
    /// Human-readable reason, safe for logs and diagnostics.
    pub reason: String,
    /// `true` when the backup file supplied the binding.
    pub used_backup: bool,
}

/// A typed filesystem/persistence error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotkeyStoreError {
    /// Human-readable detail.
    pub detail: String,
}

impl HotkeyStoreError {
    fn io(operation: &str, path: &Path, error: io::Error) -> Self {
        Self {
            detail: format!("{operation} '{}' failed: {error}", path.display()),
        }
    }
}

impl fmt::Display for HotkeyStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for HotkeyStoreError {}

/// A file-backed hotkey configuration store with an injectable path.
///
/// The store deliberately does not choose the user's configuration directory;
/// the host passes a path. Saves are atomic (temp file in the same directory,
/// `sync_all`, backup copy, rename) and keep a `<name>.bak` copy of the most
/// recently verified binding so a corrupt main file can recover it. A failed
/// load never rewrites the main file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotkeyStore {
    path: PathBuf,
}

impl HotkeyStore {
    /// Creates a store rooted at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Returns the configured path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the backup path (`<name>.bak`).
    pub fn backup_path(&self) -> PathBuf {
        let name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "hotkeys.conf".to_owned());
        self.path.with_file_name(format!("{name}.bak"))
    }

    /// Loads the configuration, falling back to the backup and then to the
    /// documented default.
    ///
    /// The returned recovery record explains any fallback. This method does not
    /// write to the main file, so a stale or invalid file is left untouched
    /// until the caller explicitly saves a verified configuration.
    pub fn load(&self) -> HotkeyConfigLoad {
        let mut failure = match fs::read_to_string(&self.path) {
            Ok(text) => match HotkeyConfig::parse(&text) {
                Ok(config) => {
                    return HotkeyConfigLoad {
                        config,
                        recovery: None,
                    }
                }
                Err(error) => format!("{} is invalid: {error}", self.path.display()),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                format!("{} does not exist", self.path.display())
            }
            Err(error) => format!("{} could not be read: {error}", self.path.display()),
        };

        let backup = self.backup_path();
        match fs::read_to_string(&backup) {
            Ok(text) => match HotkeyConfig::parse(&text) {
                Ok(config) => {
                    return HotkeyConfigLoad {
                        config,
                        recovery: Some(HotkeyRecovery {
                            reason: format!(
                                "{failure}; recovered the last working binding from {}",
                                backup.display()
                            ),
                            used_backup: true,
                        }),
                    }
                }
                Err(error) => {
                    failure.push_str(&format!("; backup is invalid too: {error}"));
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                failure.push_str("; no backup exists");
            }
            Err(error) => {
                failure.push_str(&format!("; backup could not be read: {error}"));
            }
        }

        HotkeyConfigLoad {
            config: HotkeyConfig::default_summon(),
            recovery: Some(HotkeyRecovery {
                reason: format!("{failure}; using the documented default {DEFAULT_SUMMON_CHORD}"),
                used_backup: false,
            }),
        }
    }

    /// Validates and atomically saves `config`.
    ///
    /// The verified new content is copied to the backup path before the new
    /// file replaces the main file. A failure before the rename leaves the main
    /// file untouched.
    pub fn save(&self, config: &HotkeyConfig) -> Result<(), HotkeyStoreError> {
        config.validate().map_err(|error| HotkeyStoreError {
            detail: format!("refusing to save an invalid hotkey configuration: {error}"),
        })?;

        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .map_err(|error| HotkeyStoreError::io("creating directory", parent, error))?;
            }
        }

        let text = config.render();
        let temporary = self.temporary_path();
        {
            let mut file = File::create(&temporary).map_err(|error| {
                HotkeyStoreError::io("creating temporary file", &temporary, error)
            })?;
            file.write_all(text.as_bytes()).map_err(|error| {
                HotkeyStoreError::io("writing temporary file", &temporary, error)
            })?;
            file.sync_all().map_err(|error| {
                HotkeyStoreError::io("syncing temporary file", &temporary, error)
            })?;
        }

        // Keep the verified new content in the backup as well. If the main file
        // is later corrupted externally, the backup recovers the most recent
        // verified binding rather than an older one. The backup is written
        // before the rename, so a failed save leaves the old main file intact.
        let backup = self.backup_path();
        if let Err(error) = fs::copy(&temporary, &backup) {
            let _ = fs::remove_file(&temporary);
            return Err(HotkeyStoreError::io("writing backup", &backup, error));
        }

        if let Err(error) = fs::rename(&temporary, &self.path) {
            let _ = fs::remove_file(&temporary);
            return Err(HotkeyStoreError::io(
                "replacing configuration",
                &self.path,
                error,
            ));
        }
        Ok(())
    }

    fn temporary_path(&self) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let name = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "hotkeys.conf".to_owned());
        self.path.with_file_name(format!(
            ".{name}.tmp-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }
}

// ---------------------------------------------------------------------------
// Backend and manager
// ---------------------------------------------------------------------------

/// A wake-up callback invoked when the registered chord is pressed.
///
/// The callback must only wake the UI (for example by sending on a channel); it
/// runs on the native worker thread and must not block.
pub type HotkeyWake = Arc<dyn Fn() + Send + Sync>;

/// The native adapter contract used by [`HotkeyManager`].
///
/// Implementations must make `register` idempotent at the OS level: a second
/// registration of the same chord while the first is active must return a
/// conflict, never a silent duplicate. `probe` must not disturb an active
/// registration; the manager skips probing its own active chord.
pub trait HotkeyBackend: fmt::Debug {
    /// Attempts to register `chord` and returns a typed outcome.
    fn register(&mut self, chord: &KeyChord, wake: HotkeyWake) -> RegistrationOutcome;

    /// Unregisters the active chord.
    fn unregister(&mut self, chord: &KeyChord) -> Result<(), String>;

    /// Checks whether `chord` could be registered without saving anything.
    fn probe(&mut self, chord: &KeyChord) -> RegistrationOutcome;
}

/// The result of checking a candidate chord before saving it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConflictCheck {
    /// The chord can be registered.
    Available,
    /// Another application, the IME or this process already owns the chord.
    Conflict {
        /// Platform detail.
        detail: String,
    },
    /// The host denied the probe.
    Denied {
        /// Platform detail.
        detail: String,
    },
    /// No adapter can register this chord.
    Unsupported {
        /// What to do instead.
        fallback: String,
    },
}

impl ConflictCheck {
    /// Returns `true` only for [`ConflictCheck::Available`].
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }
}

/// A typed manager error. Every failure has a reason and, where applicable, the
/// chord involved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyError {
    /// A conflicting registration.
    Conflict {
        /// The candidate chord.
        chord: KeyChord,
        /// Platform detail.
        detail: String,
    },
    /// A denied registration.
    Denied {
        /// The candidate chord.
        chord: KeyChord,
        /// Platform detail.
        detail: String,
    },
    /// No adapter can register the candidate.
    Unsupported {
        /// The candidate chord.
        chord: KeyChord,
        /// What to do instead.
        fallback: String,
    },
    /// The backend reported an unexpected registration failure.
    RegistrationFailed {
        /// The candidate chord.
        chord: KeyChord,
        /// Platform detail.
        detail: String,
    },
    /// A backend unregister/register operation failed.
    Backend {
        /// The operation name.
        operation: &'static str,
        /// Platform detail.
        detail: String,
    },
    /// No binding is currently active.
    NotActive,
    /// The manager already owns a binding.
    AlreadyActive {
        /// The active chord.
        chord: KeyChord,
    },
    /// The manager is already suspended.
    AlreadySuspended,
    /// The manager is suspended and must be resumed before activating/rearming.
    Suspended {
        /// The binding that would be registered.
        chord: KeyChord,
    },
    /// The manager was shut down and cannot be reused.
    Shutdown,
    /// The configuration has no summon binding.
    NoSummonBinding,
    /// The configuration itself is invalid.
    InvalidConfig {
        /// Detail.
        detail: String,
    },
    /// Saving the verified binding failed.
    Persistence {
        /// Detail, including whether runtime rollback succeeded.
        detail: String,
    },
}

impl fmt::Display for HotkeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict { chord, detail } => write!(formatter, "conflict for {chord}: {detail}"),
            Self::Denied { chord, detail } => {
                write!(formatter, "registration denied for {chord}: {detail}")
            }
            Self::Unsupported { chord, fallback } => {
                write!(formatter, "{chord} is unsupported: {fallback}")
            }
            Self::RegistrationFailed { chord, detail } => {
                write!(formatter, "could not register {chord}: {detail}")
            }
            Self::Backend { operation, detail } => {
                write!(formatter, "hotkey backend {operation} failed: {detail}")
            }
            Self::NotActive => formatter.write_str("no hotkey binding is active"),
            Self::AlreadyActive { chord } => {
                write!(formatter, "{chord} is already the active binding")
            }
            Self::AlreadySuspended => {
                formatter.write_str("the hotkey manager is already suspended")
            }
            Self::Suspended { chord } => write!(
                formatter,
                "the hotkey manager is suspended; resume {chord} instead of activating or rearming"
            ),
            Self::Shutdown => formatter.write_str("the hotkey manager has been shut down"),
            Self::NoSummonBinding => {
                formatter.write_str("the hotkey configuration has no summon binding")
            }
            Self::InvalidConfig { detail } => {
                write!(formatter, "invalid hotkey configuration: {detail}")
            }
            Self::Persistence { detail } => {
                write!(formatter, "hotkey persistence failed: {detail}")
            }
        }
    }
}

impl std::error::Error for HotkeyError {}

/// Outcome of [`HotkeyManager::rebind`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RebindOutcome {
    /// The new binding is active (and persisted when a store was supplied).
    Applied {
        /// The binding active before the change.
        previous: Option<KeyChord>,
        /// The new binding.
        chord: KeyChord,
        /// Whether the change was written to disk.
        persisted: bool,
    },
    /// The new binding was rejected; the previous binding is still active.
    Rejected {
        /// The rejected candidate.
        attempted: KeyChord,
        /// The still-active previous binding.
        previous: Option<KeyChord>,
        /// The typed platform outcome.
        outcome: RegistrationOutcome,
    },
    /// The candidate already equals the active binding.
    Unchanged {
        /// The active binding.
        chord: KeyChord,
        /// Whether the unchanged configuration was re-saved.
        persisted: bool,
    },
}

/// Outcome of [`HotkeyManager::suspend`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SuspendOutcome {
    /// The binding was released.
    Suspended {
        /// The released chord.
        chord: KeyChord,
    },
    /// The manager was already suspended.
    AlreadySuspended,
}

/// Outcome of [`HotkeyManager::resume`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeOutcome {
    /// The binding was registered exactly once.
    Resumed {
        /// The re-registered chord.
        chord: KeyChord,
    },
    /// The manager was not suspended.
    AlreadyActive {
        /// The active chord.
        chord: KeyChord,
    },
}

/// Outcome of [`HotkeyManager::rearm`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RearmOutcome {
    /// The binding was unregistered and registered once.
    Rearmed {
        /// The re-registered chord.
        chord: KeyChord,
    },
}

/// Outcome of [`HotkeyManager::shutdown`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShutdownOutcome {
    /// The active binding was released.
    Stopped {
        /// The released chord, or `None` when nothing was active.
        released: Option<KeyChord>,
    },
    /// Shutdown had already run.
    AlreadyStopped,
}

/// The register/unregister/rebind/suspend/resume/shutdown state machine.
///
/// The manager never silently swallows a failure. A rejected rebind leaves the
/// previous working binding active and recoverable; a bind is only persisted
/// after the backend confirms registration, and a persistence failure rolls the
/// runtime registration back to the previous binding.
pub struct HotkeyManager<B: HotkeyBackend> {
    backend: B,
    config: HotkeyConfig,
    wake: HotkeyWake,
    active: Option<KeyChord>,
    last_working: Option<KeyChord>,
    suspended: bool,
    shutdown: bool,
}

impl<B: HotkeyBackend> fmt::Debug for HotkeyManager<B> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HotkeyManager")
            .field("backend", &self.backend)
            .field("config", &self.config)
            .field("active", &self.active)
            .field("last_working", &self.last_working)
            .field("suspended", &self.suspended)
            .field("shutdown", &self.shutdown)
            .finish_non_exhaustive()
    }
}

impl<B: HotkeyBackend> HotkeyManager<B> {
    /// Creates a manager with an already-loaded configuration and a no-op wake
    /// callback.
    pub fn new(backend: B, config: HotkeyConfig) -> Result<Self, HotkeyError> {
        Self::with_wake(backend, config, Arc::new(|| {}))
    }

    /// Creates a manager with a wake callback.
    pub fn with_wake(
        backend: B,
        config: HotkeyConfig,
        wake: HotkeyWake,
    ) -> Result<Self, HotkeyError> {
        config
            .validate()
            .map_err(|error| HotkeyError::InvalidConfig {
                detail: error.to_string(),
            })?;
        if config.summon_chord().is_none() {
            return Err(HotkeyError::NoSummonBinding);
        }
        Ok(Self {
            backend,
            config,
            wake,
            active: None,
            last_working: None,
            suspended: false,
            shutdown: false,
        })
    }

    /// Returns the current configuration.
    pub fn config(&self) -> &HotkeyConfig {
        &self.config
    }

    /// Returns the active chord, if any.
    pub fn active_chord(&self) -> Option<KeyChord> {
        self.active
    }

    /// Returns the last successfully registered chord.
    ///
    /// This is the previous working binding kept recoverable across rejected
    /// rebinds and conflicts.
    pub fn last_working_chord(&self) -> Option<KeyChord> {
        self.last_working
    }

    /// Returns whether the manager is suspended.
    pub fn is_suspended(&self) -> bool {
        self.suspended
    }

    /// Returns whether the manager has been shut down.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown
    }

    /// Returns a shared handle to the backend (for diagnostics).
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Registers the configured summon chord.
    pub fn activate(&mut self) -> Result<RegistrationOutcome, HotkeyError> {
        self.ensure_running()?;
        if let Some(chord) = self.active {
            return Err(HotkeyError::AlreadyActive { chord });
        }
        if self.suspended {
            let chord = self
                .last_working
                .or_else(|| self.config.summon_chord())
                .ok_or(HotkeyError::NoSummonBinding)?;
            return Err(HotkeyError::Suspended { chord });
        }
        let chord = self
            .config
            .summon_chord()
            .ok_or(HotkeyError::NoSummonBinding)?;
        let outcome = self.backend.register(&chord, Arc::clone(&self.wake));
        if outcome.is_registered() {
            self.active = Some(chord);
            self.last_working = Some(chord);
            Ok(outcome)
        } else {
            Err(outcome_error(outcome))
        }
    }

    /// Checks a candidate without registering or saving it.
    pub fn check_conflict(&mut self, chord: &KeyChord) -> ConflictCheck {
        if self.active == Some(*chord) {
            return ConflictCheck::Available;
        }
        match self.backend.probe(chord) {
            RegistrationOutcome::Registered { .. } | RegistrationOutcome::Degraded { .. } => {
                ConflictCheck::Available
            }
            RegistrationOutcome::Conflict { detail, .. } => ConflictCheck::Conflict { detail },
            RegistrationOutcome::Denied { detail, .. } => ConflictCheck::Denied { detail },
            RegistrationOutcome::Unsupported { fallback, .. } => {
                ConflictCheck::Unsupported { fallback }
            }
        }
    }

    /// Rebinds to `chord` without persistence.
    ///
    /// The candidate is probed before the active binding is released. If the
    /// new registration fails, the previous binding is re-registered and
    /// returned in [`RebindOutcome::Rejected`].
    pub fn rebind(&mut self, chord: KeyChord) -> Result<RebindOutcome, HotkeyError> {
        self.rebind_inner(chord, None)
    }

    /// Rebinds and atomically persists the verified binding.
    ///
    /// The configuration is saved only after the new chord is registered. If
    /// the save fails, the runtime binding is rolled back to the previous one.
    pub fn rebind_and_save(
        &mut self,
        chord: KeyChord,
        store: &HotkeyStore,
    ) -> Result<RebindOutcome, HotkeyError> {
        self.rebind_inner(chord, Some(store))
    }

    fn rebind_inner(
        &mut self,
        chord: KeyChord,
        store: Option<&HotkeyStore>,
    ) -> Result<RebindOutcome, HotkeyError> {
        self.ensure_running()?;

        if self.active == Some(chord) {
            let persisted = match store {
                Some(store) => {
                    store
                        .save(&self.config)
                        .map_err(|error| HotkeyError::Persistence {
                            detail: error.to_string(),
                        })?;
                    true
                }
                None => false,
            };
            return Ok(RebindOutcome::Unchanged { chord, persisted });
        }

        let check = self.check_conflict(&chord);
        match check {
            ConflictCheck::Available => {}
            ConflictCheck::Conflict { detail } => {
                return Ok(RebindOutcome::Rejected {
                    attempted: chord,
                    previous: self.active.or(self.last_working),
                    outcome: RegistrationOutcome::Conflict { chord, detail },
                })
            }
            ConflictCheck::Denied { detail } => {
                return Ok(RebindOutcome::Rejected {
                    attempted: chord,
                    previous: self.active.or(self.last_working),
                    outcome: RegistrationOutcome::Denied { chord, detail },
                })
            }
            ConflictCheck::Unsupported { fallback } => {
                return Ok(RebindOutcome::Rejected {
                    attempted: chord,
                    previous: self.active.or(self.last_working),
                    outcome: RegistrationOutcome::Unsupported { chord, fallback },
                })
            }
        }

        let previous = self.active.or(self.last_working);
        let previous_config = self.config.clone();

        // A suspended manager has no active registration; persist the verified
        // candidate and let `resume` register it exactly once. Rebinding must
        // not silently re-acquire a shortcut the caller explicitly released.
        if self.suspended {
            self.config.set_summon(chord);
            if let Some(store) = store {
                if let Err(error) = store.save(&self.config) {
                    self.config = previous_config;
                    return Err(HotkeyError::Persistence {
                        detail: error.to_string(),
                    });
                }
            }
            self.last_working = Some(chord);
            return Ok(RebindOutcome::Applied {
                previous,
                chord,
                persisted: store.is_some(),
            });
        }

        if let Some(active) = self.active {
            self.backend
                .unregister(&active)
                .map_err(|detail| HotkeyError::Backend {
                    operation: "unregister",
                    detail,
                })?;
            self.active = None;
        }

        let outcome = self.backend.register(&chord, Arc::clone(&self.wake));
        if !outcome.is_registered() {
            self.active = None;
            if let Some(previous) = previous {
                let restored = self.backend.register(&previous, Arc::clone(&self.wake));
                if restored.is_registered() {
                    self.active = Some(previous);
                    self.last_working = Some(previous);
                } else {
                    self.suspended = true;
                    return Err(HotkeyError::RegistrationFailed {
                        chord: previous,
                        detail: format!(
                            "the rejected candidate {chord} could not be restored to {previous}: {restored}; call resume to retry the last working binding"
                        ),
                    });
                }
            }
            self.config = previous_config;
            return Ok(RebindOutcome::Rejected {
                attempted: chord,
                previous,
                outcome,
            });
        }

        self.active = Some(chord);
        self.last_working = Some(chord);
        self.config.set_summon(chord);

        if let Some(store) = store {
            if let Err(error) = store.save(&self.config) {
                let rollback = self.rollback_after_persistence_failure(previous, &previous_config);
                let detail = match rollback {
                    Ok(()) => format!("{error}; runtime binding rolled back to {previous:?}"),
                    Err(rollback_error) => {
                        format!("{error}; runtime rollback also failed: {rollback_error}")
                    }
                };
                return Err(HotkeyError::Persistence { detail });
            }
        }

        Ok(RebindOutcome::Applied {
            previous,
            chord,
            persisted: store.is_some(),
        })
    }

    fn rollback_after_persistence_failure(
        &mut self,
        previous: Option<KeyChord>,
        previous_config: &HotkeyConfig,
    ) -> Result<(), HotkeyError> {
        if let Some(active) = self.active {
            if let Err(detail) = self.backend.unregister(&active) {
                self.config = previous_config.clone();
                return Err(HotkeyError::Backend {
                    operation: "rollback unregister",
                    detail,
                });
            }
            self.active = None;
        }
        self.config = previous_config.clone();
        if let Some(previous) = previous {
            let restored = self.backend.register(&previous, Arc::clone(&self.wake));
            if restored.is_registered() {
                self.active = Some(previous);
                self.last_working = Some(previous);
                Ok(())
            } else {
                Err(HotkeyError::RegistrationFailed {
                    chord: previous,
                    detail: restored.to_string(),
                })
            }
        } else {
            self.last_working = None;
            Ok(())
        }
    }

    /// Releases the active binding but keeps the last working chord recoverable.
    pub fn suspend(&mut self) -> Result<SuspendOutcome, HotkeyError> {
        self.ensure_running()?;
        if self.suspended {
            return Ok(SuspendOutcome::AlreadySuspended);
        }
        let Some(chord) = self.active else {
            return Err(HotkeyError::NotActive);
        };
        self.backend
            .unregister(&chord)
            .map_err(|detail| HotkeyError::Backend {
                operation: "unregister",
                detail,
            })?;
        self.active = None;
        self.suspended = true;
        self.last_working = Some(chord);
        Ok(SuspendOutcome::Suspended { chord })
    }

    /// Re-registers the last working binding exactly once.
    pub fn resume(&mut self) -> Result<ResumeOutcome, HotkeyError> {
        self.ensure_running()?;
        if !self.suspended {
            let chord = self.active.ok_or(HotkeyError::NotActive)?;
            return Ok(ResumeOutcome::AlreadyActive { chord });
        }
        let chord = self
            .last_working
            .or_else(|| self.config.summon_chord())
            .ok_or(HotkeyError::NoSummonBinding)?;
        let outcome = self.backend.register(&chord, Arc::clone(&self.wake));
        if !outcome.is_registered() {
            return Err(outcome_error(outcome));
        }
        self.active = Some(chord);
        self.last_working = Some(chord);
        self.suspended = false;
        Ok(ResumeOutcome::Resumed { chord })
    }

    /// Unregisters and re-registers the active binding once.
    ///
    /// This is the explicit recovery hook for session unlock or wake events.
    /// The Windows adapter does not create a message-only power/session window,
    /// so automatic sleep/lock notifications are not wired; callers must invoke
    /// this after receiving `WM_POWERBROADCAST` or `WM_WTSSESSION_CHANGE` in
    /// their own window procedure, or after their host lifecycle event.
    pub fn rearm(&mut self) -> Result<RearmOutcome, HotkeyError> {
        self.ensure_running()?;
        if self.suspended {
            let chord = self
                .last_working
                .or_else(|| self.config.summon_chord())
                .ok_or(HotkeyError::NoSummonBinding)?;
            return Err(HotkeyError::Suspended { chord });
        }
        let chord = self.active.ok_or(HotkeyError::NotActive)?;
        self.backend
            .unregister(&chord)
            .map_err(|detail| HotkeyError::Backend {
                operation: "rearm unregister",
                detail,
            })?;
        self.active = None;
        let outcome = self.backend.register(&chord, Arc::clone(&self.wake));
        if !outcome.is_registered() {
            // Re-arm released the old registration. Expose the suspended state
            // so `resume` can retry the last working binding.
            self.suspended = true;
            return Err(outcome_error(outcome));
        }
        self.active = Some(chord);
        self.last_working = Some(chord);
        Ok(RearmOutcome::Rearmed { chord })
    }

    /// Releases any active binding and permanently stops the manager.
    ///
    /// Repeated calls return [`ShutdownOutcome::AlreadyStopped`] rather than a
    /// silent success or an error.
    pub fn shutdown(&mut self) -> Result<ShutdownOutcome, HotkeyError> {
        if self.shutdown {
            return Ok(ShutdownOutcome::AlreadyStopped);
        }
        let released = if let Some(chord) = self.active {
            self.backend
                .unregister(&chord)
                .map_err(|detail| HotkeyError::Backend {
                    operation: "shutdown unregister",
                    detail,
                })?;
            self.active = None;
            Some(chord)
        } else {
            None
        };
        self.suspended = false;
        self.shutdown = true;
        Ok(ShutdownOutcome::Stopped { released })
    }

    fn ensure_running(&self) -> Result<(), HotkeyError> {
        if self.shutdown {
            Err(HotkeyError::Shutdown)
        } else {
            Ok(())
        }
    }
}

fn outcome_error(outcome: RegistrationOutcome) -> HotkeyError {
    match outcome {
        RegistrationOutcome::Registered { chord } => HotkeyError::RegistrationFailed {
            chord,
            detail: "the backend reported success but did not activate the chord".to_owned(),
        },
        RegistrationOutcome::Conflict { chord, detail } => HotkeyError::Conflict { chord, detail },
        RegistrationOutcome::Denied { chord, detail } => HotkeyError::Denied { chord, detail },
        RegistrationOutcome::Unsupported { chord, fallback } => {
            HotkeyError::Unsupported { chord, fallback }
        }
        RegistrationOutcome::Degraded { chord, reason, .. } => HotkeyError::RegistrationFailed {
            chord,
            detail: reason,
        },
    }
}

// ---------------------------------------------------------------------------
// Shortcut recorder
// ---------------------------------------------------------------------------

/// A key observed by the recorder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecorderKey {
    /// A modifier key down or up.
    Modifier(Modifier),
    /// A non-modifier key down or up.
    Key(Key),
}

/// A recorder input event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecorderEvent {
    /// Key down.
    Down(RecorderKey),
    /// Key up.
    Up(RecorderKey),
}

/// Recorder state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecorderPhase {
    /// Nothing captured.
    Idle,
    /// A non-modifier key is held; the candidate is visible but not ready.
    Capturing,
    /// The key was released; the candidate can be verified.
    Candidate,
    /// The candidate passed a conflict/availability check.
    Verified,
}

/// The result of feeding one event to [`ShortcutRecorder`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordOutcome {
    /// Modifier state changed; no chord was produced.
    ModifierChanged {
        /// The currently held modifier set.
        modifiers: Modifiers,
    },
    /// A chord is being held down.
    Capturing {
        /// The candidate chord.
        chord: KeyChord,
    },
    /// The chord is ready for verification.
    Candidate {
        /// The candidate chord.
        chord: KeyChord,
    },
    /// The event did not change the candidate.
    Ignored {
        /// Why the event was ignored.
        reason: IgnoreReason,
    },
}

/// Why a recorder event was ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IgnoreReason {
    /// A modifier-only press/release never produces a candidate.
    ModifierOnly,
    /// A key-up did not match the held non-modifier key.
    UnrelatedKeyUp,
    /// The recorder has no key state after a reset.
    NoKeyHeld,
}

/// A typed recorder failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecorderError {
    /// No candidate chord has been captured.
    NoCandidate,
    /// The non-modifier key is still held; release it before verifying.
    KeyStillHeld,
    /// The candidate has not been verified.
    NotVerified,
    /// Verification found a conflict.
    Conflict {
        /// The candidate chord.
        chord: KeyChord,
        /// Platform detail.
        detail: String,
    },
    /// Verification was denied.
    Denied {
        /// The candidate chord.
        chord: KeyChord,
        /// Platform detail.
        detail: String,
    },
    /// The candidate is unsupported.
    Unsupported {
        /// The candidate chord.
        chord: KeyChord,
        /// What to do instead.
        fallback: String,
    },
}

impl fmt::Display for RecorderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCandidate => formatter.write_str("no shortcut has been captured"),
            Self::KeyStillHeld => {
                formatter.write_str("release the non-modifier key before verifying the shortcut")
            }
            Self::NotVerified => {
                formatter.write_str("the shortcut must be verified before it can be saved")
            }
            Self::Conflict { chord, detail } => {
                write!(formatter, "conflict for {chord}: {detail}")
            }
            Self::Denied { chord, detail } => {
                write!(formatter, "registration denied for {chord}: {detail}")
            }
            Self::Unsupported { chord, fallback } => {
                write!(formatter, "{chord} is unsupported: {fallback}")
            }
        }
    }
}

impl std::error::Error for RecorderError {}

/// The result of cancelling a capture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecorderCancel {
    /// The candidate that was abandoned, if any.
    pub candidate: Option<KeyChord>,
    /// The verified chord that was abandoned, if any.
    pub verified: Option<KeyChord>,
}

/// UI-independent shortcut capture and verification logic.
///
/// Feed native key down/up events with [`ShortcutRecorder::record`]. Modifier
/// presses never produce a chord. A candidate must be verified with
/// [`ShortcutRecorder::verify`] before [`ShortcutRecorder::commit`] returns it;
/// persistence code should use the committed value (or the manager's
/// verified-rebind path) rather than the raw candidate.
#[derive(Debug)]
pub struct ShortcutRecorder {
    platform: Platform,
    phase: RecorderPhase,
    held: Modifiers,
    held_key: Option<Key>,
    candidate: Option<KeyChord>,
    verified: Option<KeyChord>,
}

impl ShortcutRecorder {
    /// Creates a recorder that renders native names for `platform`.
    pub fn new(platform: Platform) -> Self {
        Self {
            platform,
            phase: RecorderPhase::Idle,
            held: Modifiers::empty(),
            held_key: None,
            candidate: None,
            verified: None,
        }
    }

    /// Returns the current phase.
    pub fn phase(&self) -> RecorderPhase {
        self.phase
    }

    /// Returns the candidate chord, including one that is still held.
    pub fn candidate(&self) -> Option<KeyChord> {
        self.candidate
    }

    /// Returns the verified chord.
    pub fn verified(&self) -> Option<KeyChord> {
        self.verified
    }

    /// Returns the currently held modifiers.
    pub fn held_modifiers(&self) -> Modifiers {
        self.held
    }

    /// Renders the held modifier names natively, or `"none"`.
    pub fn modifier_display(&self) -> String {
        if self.held.is_empty() {
            "none".to_owned()
        } else {
            self.held.render(self.platform)
        }
    }

    /// Renders the candidate with native modifier/key names.
    pub fn candidate_display(&self) -> Option<String> {
        self.candidate.map(|chord| chord.render(self.platform))
    }

    /// Captures one key event.
    pub fn record(&mut self, event: RecorderEvent) -> RecordOutcome {
        match event {
            RecorderEvent::Down(RecorderKey::Modifier(modifier)) => {
                self.begin_new_capture_if_verified();
                self.held.insert(modifier);
                self.phase = if self.held_key.is_some() {
                    RecorderPhase::Capturing
                } else {
                    RecorderPhase::Idle
                };
                RecordOutcome::ModifierChanged {
                    modifiers: self.held,
                }
            }
            RecorderEvent::Up(RecorderKey::Modifier(modifier)) => {
                self.held.remove(modifier);
                RecordOutcome::Ignored {
                    reason: IgnoreReason::ModifierOnly,
                }
            }
            RecorderEvent::Down(RecorderKey::Key(key)) => {
                self.begin_new_capture_if_verified();
                let chord = KeyChord::new(self.held, key);
                self.held_key = Some(key);
                self.candidate = Some(chord);
                self.verified = None;
                self.phase = RecorderPhase::Capturing;
                RecordOutcome::Capturing { chord }
            }
            RecorderEvent::Up(RecorderKey::Key(key)) => {
                if self.held_key == Some(key) {
                    self.held_key = None;
                    if let Some(chord) = self.candidate {
                        self.phase = RecorderPhase::Candidate;
                        RecordOutcome::Candidate { chord }
                    } else {
                        RecordOutcome::Ignored {
                            reason: IgnoreReason::NoKeyHeld,
                        }
                    }
                } else {
                    RecordOutcome::Ignored {
                        reason: IgnoreReason::UnrelatedKeyUp,
                    }
                }
            }
        }
    }

    fn begin_new_capture_if_verified(&mut self) {
        if self.phase == RecorderPhase::Verified {
            self.phase = RecorderPhase::Idle;
            self.candidate = None;
            self.verified = None;
            self.held_key = None;
        }
    }

    /// Verifies the candidate against a conflict/availability check.
    ///
    /// A failed check leaves the recorder in the candidate state so the user can
    /// retry or cancel. It never marks the chord verified.
    pub fn verify(&mut self, check: ConflictCheck) -> Result<KeyChord, RecorderError> {
        let chord = self.candidate.ok_or(RecorderError::NoCandidate)?;
        match self.phase {
            RecorderPhase::Capturing => return Err(RecorderError::KeyStillHeld),
            RecorderPhase::Idle => return Err(RecorderError::NoCandidate),
            RecorderPhase::Candidate | RecorderPhase::Verified => {}
        }
        match check {
            ConflictCheck::Available => {
                self.phase = RecorderPhase::Verified;
                self.verified = Some(chord);
                Ok(chord)
            }
            ConflictCheck::Conflict { detail } => Err(RecorderError::Conflict { chord, detail }),
            ConflictCheck::Denied { detail } => Err(RecorderError::Denied { chord, detail }),
            ConflictCheck::Unsupported { fallback } => {
                Err(RecorderError::Unsupported { chord, fallback })
            }
        }
    }

    /// Returns the verified chord for saving.
    ///
    /// This is the only method that releases a binding for persistence, so an
    /// unverified candidate cannot be saved by accident.
    pub fn commit(&mut self) -> Result<KeyChord, RecorderError> {
        if self.phase != RecorderPhase::Verified {
            return Err(RecorderError::NotVerified);
        }
        let chord = self.verified.ok_or(RecorderError::NotVerified)?;
        self.reset();
        Ok(chord)
    }

    /// Cancels the capture and returns what was abandoned.
    pub fn cancel(&mut self) -> RecorderCancel {
        let cancelled = RecorderCancel {
            candidate: self.candidate,
            verified: self.verified,
        };
        self.reset();
        cancelled
    }

    fn reset(&mut self) {
        self.phase = RecorderPhase::Idle;
        self.held = Modifiers::empty();
        self.held_key = None;
        self.candidate = None;
        self.verified = None;
    }
}

// ---------------------------------------------------------------------------
// Windows adapter glue
// ---------------------------------------------------------------------------

/// A [`HotkeyBackend`] backed by the real Windows `RegisterHotKey` worker.
///
/// This lives in the API crate so the manager and recorder can be exercised
/// against the real adapter without making `ansa-platform-windows` depend on
/// the API crate (which would be circular). The neutral chord is converted
/// through the shared persisted grammar; a parity test keeps the two parsers
/// aligned.
#[cfg(windows)]
#[derive(Debug, Default)]
pub struct WindowsHotkeyBackend {
    registration: Option<ansa_platform_windows::HotkeyRegistration>,
}

#[cfg(windows)]
impl WindowsHotkeyBackend {
    /// Creates an idle backend.
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(windows)]
impl HotkeyBackend for WindowsHotkeyBackend {
    fn register(&mut self, chord: &KeyChord, wake: HotkeyWake) -> RegistrationOutcome {
        if self.registration.is_some() {
            return RegistrationOutcome::Conflict {
                chord: *chord,
                detail: "this backend already owns an active registration".to_owned(),
            };
        }
        let native = match ansa_platform_windows::KeyChord::parse(&chord.persisted()) {
            Ok(native) => native,
            Err(error) => {
                return RegistrationOutcome::Unsupported {
                    chord: *chord,
                    fallback: format!("the Windows adapter cannot express this chord: {error}"),
                }
            }
        };
        let callback = {
            let wake = Arc::clone(&wake);
            move || wake()
        };
        match ansa_platform_windows::HotkeyRegistration::register_chord(native, callback) {
            Ok(registration) => {
                self.registration = Some(registration);
                RegistrationOutcome::Registered { chord: *chord }
            }
            Err(error) => match error {
                ansa_platform_windows::HotkeyRegistrationError::Conflict { detail, .. } => {
                    RegistrationOutcome::Conflict {
                        chord: *chord,
                        detail,
                    }
                }
                ansa_platform_windows::HotkeyRegistrationError::Denied { detail, .. } => {
                    RegistrationOutcome::Denied {
                        chord: *chord,
                        detail,
                    }
                }
                ansa_platform_windows::HotkeyRegistrationError::InvalidChord { detail, .. } => {
                    RegistrationOutcome::Unsupported {
                        chord: *chord,
                        fallback: format!("the Windows adapter rejected the chord: {detail}"),
                    }
                }
                ansa_platform_windows::HotkeyRegistrationError::Worker { detail, .. }
                | ansa_platform_windows::HotkeyRegistrationError::NotRegistered {
                    detail, ..
                }
                | ansa_platform_windows::HotkeyRegistrationError::NotSuspended { detail, .. }
                | ansa_platform_windows::HotkeyRegistrationError::Timeout { detail, .. } => {
                    RegistrationOutcome::Unsupported {
                        chord: *chord,
                        fallback: format!("the Windows hotkey worker failed: {detail}"),
                    }
                }
            },
        }
    }

    fn unregister(&mut self, chord: &KeyChord) -> Result<(), String> {
        match self.registration.take() {
            Some(registration) => {
                if registration.chord().persisted() == chord.persisted() {
                    drop(registration);
                    Ok(())
                } else {
                    let actual = registration.chord();
                    self.registration = Some(registration);
                    Err(format!(
                        "refusing to release {actual} while asked to release {chord}"
                    ))
                }
            }
            None => Err(format!("{chord} is not registered")),
        }
    }

    fn probe(&mut self, chord: &KeyChord) -> RegistrationOutcome {
        let native = match ansa_platform_windows::KeyChord::parse(&chord.persisted()) {
            Ok(native) => native,
            Err(error) => {
                return RegistrationOutcome::Unsupported {
                    chord: *chord,
                    fallback: format!("the Windows adapter cannot express this chord: {error}"),
                }
            }
        };
        match ansa_platform_windows::probe_chord(&native) {
            ansa_platform_windows::ProbeOutcome::Available => {
                RegistrationOutcome::Registered { chord: *chord }
            }
            ansa_platform_windows::ProbeOutcome::Conflict { detail } => {
                RegistrationOutcome::Conflict {
                    chord: *chord,
                    detail,
                }
            }
            ansa_platform_windows::ProbeOutcome::Denied { detail } => RegistrationOutcome::Denied {
                chord: *chord,
                detail,
            },
            ansa_platform_windows::ProbeOutcome::Unsupported { detail } => {
                RegistrationOutcome::Unsupported {
                    chord: *chord,
                    fallback: detail,
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Clone, Debug, Default)]
    struct FakeBackend {
        active: Option<KeyChord>,
        registered_once: usize,
        conflicts: Vec<KeyChord>,
        register_only_conflicts: Vec<KeyChord>,
        deny: bool,
        register_failures: usize,
        unregister_failures: usize,
        duplicate_registration_detected: bool,
    }

    impl FakeBackend {
        fn conflict_on(chord: &str) -> Self {
            let mut backend = Self::default();
            backend.conflicts.push(KeyChord::parse(chord).unwrap());
            backend
        }
    }

    impl HotkeyBackend for FakeBackend {
        fn register(&mut self, chord: &KeyChord, _wake: HotkeyWake) -> RegistrationOutcome {
            if self.active.is_some() {
                self.duplicate_registration_detected = true;
                return RegistrationOutcome::Conflict {
                    chord: *chord,
                    detail: "fake backend already has an active registration".to_owned(),
                };
            }
            if self.deny {
                return RegistrationOutcome::Denied {
                    chord: *chord,
                    detail: "fake access denied".to_owned(),
                };
            }
            if self.register_failures > 0 {
                self.register_failures -= 1;
                return RegistrationOutcome::Conflict {
                    chord: *chord,
                    detail: "fake one-shot register failure".to_owned(),
                };
            }
            if self.conflicts.contains(chord) || self.register_only_conflicts.contains(chord) {
                return RegistrationOutcome::Conflict {
                    chord: *chord,
                    detail: "fake conflict".to_owned(),
                };
            }
            self.active = Some(*chord);
            self.registered_once += 1;
            RegistrationOutcome::Registered { chord: *chord }
        }

        fn unregister(&mut self, chord: &KeyChord) -> Result<(), String> {
            if self.unregister_failures > 0 {
                self.unregister_failures -= 1;
                return Err("fake unregister failure".to_owned());
            }
            if self.active == Some(*chord) {
                self.active = None;
                Ok(())
            } else {
                Err(format!("{chord} is not active"))
            }
        }

        fn probe(&mut self, chord: &KeyChord) -> RegistrationOutcome {
            if self.deny {
                return RegistrationOutcome::Denied {
                    chord: *chord,
                    detail: "fake access denied".to_owned(),
                };
            }
            if self.conflicts.contains(chord) {
                return RegistrationOutcome::Conflict {
                    chord: *chord,
                    detail: "fake conflict".to_owned(),
                };
            }
            RegistrationOutcome::Registered { chord: *chord }
        }
    }

    static TEMP_COUNTER: Mutex<u64> = Mutex::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let mut counter = TEMP_COUNTER.lock().unwrap();
            *counter += 1;
            let path = std::env::temp_dir().join(format!(
                "ansa-hotkey-api-{}-{}",
                std::process::id(),
                *counter
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn store(&self, name: &str) -> HotkeyStore {
            HotkeyStore::new(self.path.join(name))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn chord(input: &str) -> KeyChord {
        KeyChord::parse(input).unwrap()
    }

    // -- chord grammar -----------------------------------------------------

    #[test]
    fn parses_and_renders_native_modifier_names() {
        let parsed = chord("Ctrl + Option+Shift+Command+A");
        assert_eq!(
            parsed.persisted(),
            "ctrl+alt+shift+meta+a".to_owned(),
            "aliases must normalize to the stable persisted form"
        );
        assert_eq!(parsed.render(Platform::Windows), "Ctrl+Alt+Shift+Win+A");
        assert_eq!(
            parsed.render(Platform::MacOs),
            "Control+Option+Shift+Command+A"
        );
        assert_eq!(parsed.render(Platform::Linux), "Ctrl+Alt+Shift+Super+A");
        assert_eq!(KeyChord::default_summon().persisted(), "ctrl+space");
        assert_eq!(
            KeyChord::default_summon().render(Platform::Windows),
            "Ctrl+Space"
        );
    }

    #[test]
    fn parses_supported_key_categories() {
        for (input, persisted) in [
            ("ctrl+a", "ctrl+a"),
            ("ctrl+0", "ctrl+0"),
            ("ctrl+f24", "ctrl+f24"),
            ("ctrl+space", "ctrl+space"),
            ("shift+return", "shift+enter"),
            ("alt+esc", "alt+escape"),
            ("meta+pgdn", "meta+pagedown"),
            ("ctrl+arrowup", "ctrl+up"),
            ("ctrl+-", "ctrl+-"),
            ("ctrl+minus", "ctrl+-"),
            ("ctrl+slash", "ctrl+/"),
            ("ctrl+`", "ctrl+`"),
        ] {
            assert_eq!(
                KeyChord::parse(input).unwrap().persisted(),
                persisted,
                "input {input}"
            );
        }
    }

    #[test]
    fn rejects_empty_modifier_only_duplicate_and_unsupported_chords() {
        assert_eq!(KeyChord::parse(""), Err(ChordParseError::Empty));
        assert_eq!(KeyChord::parse("   "), Err(ChordParseError::Empty));
        assert_eq!(KeyChord::parse("ctrl"), Err(ChordParseError::ModifierOnly));
        assert_eq!(
            KeyChord::parse("ctrl+ctrl+a"),
            Err(ChordParseError::DuplicateModifier(Modifier::Control))
        );
        assert_eq!(
            KeyChord::parse("ctrl+control+a"),
            Err(ChordParseError::DuplicateModifier(Modifier::Control))
        );
        assert_eq!(
            KeyChord::parse("win+meta+a"),
            Err(ChordParseError::DuplicateModifier(Modifier::Meta))
        );
        assert_eq!(
            KeyChord::parse("ctrl+a+b"),
            Err(ChordParseError::MultipleKeys)
        );
        assert_eq!(
            KeyChord::parse("ctrl+hyper"),
            Err(ChordParseError::UnsupportedToken("hyper".to_owned()))
        );
        assert_eq!(KeyChord::parse("ctrl++a"), Err(ChordParseError::EmptyToken));
        assert_eq!(
            KeyChord::parse("ctrl+f25"),
            Err(ChordParseError::UnsupportedToken("f25".to_owned()))
        );
    }

    // -- configuration -----------------------------------------------------

    #[test]
    fn config_round_trips_and_rejects_unknown_keys_without_panicking() {
        let config = HotkeyConfig::with_summon(chord("ctrl+alt+shift+meta+a"));
        let text = config.render();
        assert!(text.starts_with("ansa-hotkeys 1\n"));
        assert_eq!(HotkeyConfig::parse(&text).unwrap(), config);

        let unknown = "ansa-hotkeys 1\nsummon ctrl+hyper\n";
        let error = HotkeyConfig::parse(unknown).unwrap_err();
        assert!(matches!(
            error,
            HotkeyConfigError::InvalidChord {
                line: 2,
                source: ChordParseError::UnsupportedToken(_),
                ..
            }
        ));
        assert!(error.to_string().contains("line 2"));

        let stale = "ansa-hotkeys 9\nsummon ctrl+space\n";
        assert_eq!(
            HotkeyConfig::parse(stale),
            Err(HotkeyConfigError::UnsupportedVersion { version: 9 })
        );
        let missing = "ansa-hotkeys 1\ncommand:calc ctrl+k\n";
        assert_eq!(
            HotkeyConfig::parse(missing),
            Err(HotkeyConfigError::MissingSummon)
        );
    }

    #[test]
    fn store_save_is_atomic_and_load_recovers_the_last_working_binding() {
        let dir = TempDir::new();
        let store = dir.store("hotkeys.conf");
        store.save(&HotkeyConfig::default_summon()).unwrap();

        let first = store.load();
        assert!(first.recovery.is_none());
        assert_eq!(
            first.config.summon_chord(),
            Some(KeyChord::default_summon())
        );

        let rebound = HotkeyConfig::with_summon(chord("ctrl+alt+k"));
        store.save(&rebound).unwrap();
        assert_eq!(store.load().config, rebound);

        // Corrupt the main file; the backup must still hold the last working
        // binding and the load must not rewrite anything.
        fs::write(store.path(), "ansa-hotkeys 1\nsummon ctrl+hyper\n").unwrap();
        let recovered = store.load();
        assert_eq!(recovered.config, rebound);
        let recovery = recovered.recovery.expect("recovery record");
        assert!(recovery.used_backup);
        assert!(recovery.reason.contains("invalid"));
        assert_eq!(
            fs::read_to_string(store.path()).unwrap(),
            "ansa-hotkeys 1\nsummon ctrl+hyper\n",
            "a failed load must leave the file untouched"
        );

        // Without a backup the documented default is returned.
        let bare = dir.store("missing.conf");
        let fallback = bare.load();
        assert_eq!(fallback.config, HotkeyConfig::default_summon());
        assert!(fallback.recovery.is_some());
        assert!(!bare.path().exists());
    }

    #[test]
    fn store_refuses_invalid_configs() {
        let dir = TempDir::new();
        let store = dir.store("hotkeys.conf");
        let mut invalid = HotkeyConfig::default_summon();
        invalid.bindings.clear();
        let error = store.save(&invalid).unwrap_err();
        assert!(error.to_string().contains("refusing to save"));
        assert!(!store.path().exists());
    }

    // -- manager -----------------------------------------------------------

    #[test]
    fn manager_registers_rebinds_and_persists_only_verified_bindings() {
        let dir = TempDir::new();
        let store = dir.store("hotkeys.conf");
        let backend = FakeBackend::default();
        let mut manager = HotkeyManager::new(backend, HotkeyConfig::default_summon()).unwrap();

        let active = manager.activate().unwrap();
        assert!(active.is_registered());
        assert_eq!(manager.active_chord(), Some(KeyChord::default_summon()));

        let candidate = chord("ctrl+alt+k");
        let applied = manager.rebind_and_save(candidate, &store).unwrap();
        assert!(matches!(
            applied,
            RebindOutcome::Applied {
                previous: Some(_),
                chord,
                persisted: true
            } if chord == candidate
        ));
        assert_eq!(store.load().config.summon_chord(), Some(candidate));
        assert_eq!(manager.active_chord(), Some(candidate));

        let shutdown = manager.shutdown().unwrap();
        assert_eq!(
            shutdown,
            ShutdownOutcome::Stopped {
                released: Some(candidate)
            }
        );
        assert_eq!(manager.shutdown().unwrap(), ShutdownOutcome::AlreadyStopped);
        assert_eq!(manager.activate().unwrap_err(), HotkeyError::Shutdown);
    }

    #[test]
    fn rejected_rebind_keeps_the_previous_working_binding() {
        let backend = FakeBackend::conflict_on("ctrl+alt+k");
        let mut manager = HotkeyManager::new(backend, HotkeyConfig::default_summon()).unwrap();
        manager.activate().unwrap();
        let previous = manager.active_chord().unwrap();

        let rejected = manager.rebind(chord("ctrl+alt+k")).unwrap();
        let RebindOutcome::Rejected {
            attempted,
            previous: reported_previous,
            outcome,
        } = rejected
        else {
            panic!("expected a rejected rebind");
        };
        assert_eq!(attempted, chord("ctrl+alt+k"));
        assert_eq!(reported_previous, Some(previous));
        assert!(matches!(outcome, RegistrationOutcome::Conflict { .. }));
        assert_eq!(manager.active_chord(), Some(previous));
        assert_eq!(manager.last_working_chord(), Some(previous));
        assert_eq!(
            manager.config().summon_chord(),
            Some(previous),
            "a rejected rebind must not change the desired binding"
        );
        assert!(!manager.backend().duplicate_registration_detected);
    }

    #[test]
    fn register_failure_after_release_restores_the_previous_binding() {
        let candidate = chord("ctrl+alt+k");
        let backend = FakeBackend {
            register_only_conflicts: vec![candidate],
            ..FakeBackend::default()
        };
        let mut manager = HotkeyManager::new(backend, HotkeyConfig::default_summon()).unwrap();
        manager.activate().unwrap();
        let previous = manager.active_chord().unwrap();

        let rejected = manager.rebind(candidate).unwrap();
        assert!(matches!(
            rejected,
            RebindOutcome::Rejected {
                attempted,
                previous: Some(restored),
                outcome: RegistrationOutcome::Conflict { .. }
            } if attempted == candidate && restored == previous
        ));
        assert_eq!(manager.active_chord(), Some(previous));
        assert_eq!(manager.last_working_chord(), Some(previous));
        assert_eq!(manager.backend().active, Some(previous));
        assert_eq!(manager.config().summon_chord(), Some(previous));
        assert!(!manager.backend().duplicate_registration_detected);
    }

    #[test]
    fn denied_rebind_keeps_previous_and_reports_denial() {
        let backend = FakeBackend {
            deny: true,
            ..FakeBackend::default()
        };
        let mut manager = HotkeyManager::new(backend, HotkeyConfig::default_summon()).unwrap();
        assert!(matches!(
            manager.activate(),
            Err(HotkeyError::Denied { .. })
        ));
        assert_eq!(manager.active_chord(), None);
    }

    #[test]
    fn persistence_failure_rolls_back_the_runtime_binding() {
        let dir = TempDir::new();
        // A directory at the config path makes the atomic rename fail.
        let blocked = dir.path.join("blocked.conf");
        fs::create_dir_all(&blocked).unwrap();
        let store = HotkeyStore::new(blocked);
        let mut manager =
            HotkeyManager::new(FakeBackend::default(), HotkeyConfig::default_summon()).unwrap();
        manager.activate().unwrap();
        let previous = manager.active_chord().unwrap();

        let error = manager
            .rebind_and_save(chord("ctrl+alt+k"), &store)
            .unwrap_err();
        assert!(matches!(error, HotkeyError::Persistence { .. }));
        assert_eq!(manager.active_chord(), Some(previous));
        assert_eq!(manager.config().summon_chord(), Some(previous));
        assert_eq!(manager.backend().active, Some(previous));
    }

    #[test]
    fn rebind_while_suspended_waits_for_resume_and_registers_once() {
        let mut manager =
            HotkeyManager::new(FakeBackend::default(), HotkeyConfig::default_summon()).unwrap();
        manager.activate().unwrap();
        let registrations_before = manager.backend().registered_once;
        let previous = manager.active_chord().unwrap();
        manager.suspend().unwrap();

        let candidate = chord("ctrl+alt+k");
        let applied = manager.rebind(candidate).unwrap();
        assert_eq!(
            applied,
            RebindOutcome::Applied {
                previous: Some(previous),
                chord: candidate,
                persisted: false
            }
        );
        assert!(manager.is_suspended());
        assert_eq!(manager.active_chord(), None);
        assert_eq!(
            manager.backend().registered_once,
            registrations_before,
            "a suspended rebind must not register the shortcut"
        );
        assert_eq!(manager.config().summon_chord(), Some(candidate));
        assert_eq!(
            manager.activate().unwrap_err(),
            HotkeyError::Suspended { chord: candidate }
        );
        assert_eq!(
            manager.rearm().unwrap_err(),
            HotkeyError::Suspended { chord: candidate }
        );

        assert_eq!(
            manager.resume().unwrap(),
            ResumeOutcome::Resumed { chord: candidate }
        );
        assert_eq!(manager.backend().registered_once, registrations_before + 1);
        assert_eq!(manager.backend().active, Some(candidate));
        assert!(!manager.is_suspended());
    }

    #[test]
    fn suspend_and_resume_do_not_duplicate_registrations_across_1000_cycles() {
        let mut manager =
            HotkeyManager::new(FakeBackend::default(), HotkeyConfig::default_summon()).unwrap();
        manager.activate().unwrap();

        for cycle in 0..1_000 {
            let suspended = manager.suspend().unwrap();
            assert!(
                matches!(suspended, SuspendOutcome::Suspended { .. }),
                "cycle {cycle}"
            );
            assert!(manager.active_chord().is_none());
            let resumed = manager.resume().unwrap();
            assert!(
                matches!(resumed, ResumeOutcome::Resumed { .. }),
                "cycle {cycle}"
            );
            assert_eq!(
                manager.backend().active,
                Some(KeyChord::default_summon()),
                "cycle {cycle}"
            );
        }

        assert_eq!(
            manager.suspend().unwrap(),
            SuspendOutcome::Suspended {
                chord: KeyChord::default_summon()
            }
        );
        assert_eq!(manager.suspend().unwrap(), SuspendOutcome::AlreadySuspended);
        assert_eq!(
            manager.resume().unwrap(),
            ResumeOutcome::Resumed {
                chord: KeyChord::default_summon()
            }
        );
        assert_eq!(
            manager.resume().unwrap(),
            ResumeOutcome::AlreadyActive {
                chord: KeyChord::default_summon()
            }
        );
        assert!(!manager.backend().duplicate_registration_detected);
    }

    #[test]
    fn rearm_releases_and_registers_exactly_once() {
        let mut manager =
            HotkeyManager::new(FakeBackend::default(), HotkeyConfig::default_summon()).unwrap();
        manager.activate().unwrap();
        let before = manager.backend().registered_once;
        assert_eq!(
            manager.rearm().unwrap(),
            RearmOutcome::Rearmed {
                chord: KeyChord::default_summon()
            }
        );
        assert_eq!(manager.backend().registered_once, before + 1);
        assert_eq!(manager.backend().active, Some(KeyChord::default_summon()));
    }

    #[test]
    fn failed_rearm_and_failed_rollback_are_resumable() {
        let mut manager =
            HotkeyManager::new(FakeBackend::default(), HotkeyConfig::default_summon()).unwrap();
        manager.activate().unwrap();
        let previous = manager.active_chord().unwrap();

        manager.backend.register_failures = 1;
        let error = manager.rearm().unwrap_err();
        assert!(matches!(error, HotkeyError::Conflict { .. }));
        assert_eq!(manager.active_chord(), None);
        assert!(manager.is_suspended());
        assert_eq!(manager.last_working_chord(), Some(previous));
        assert_eq!(
            manager.resume().unwrap(),
            ResumeOutcome::Resumed { chord: previous }
        );
        assert_eq!(manager.active_chord(), Some(previous));

        // Fail both the candidate registration and the restore attempt. The
        // manager must not claim the previous binding is active; resume retries
        // the last working chord.
        manager.backend.register_failures = 2;
        let error = manager.rebind(chord("ctrl+alt+k")).unwrap_err();
        assert!(matches!(error, HotkeyError::RegistrationFailed { .. }));
        assert_eq!(manager.active_chord(), None);
        assert!(manager.is_suspended());
        assert_eq!(manager.config().summon_chord(), Some(previous));
        assert_eq!(
            manager.resume().unwrap(),
            ResumeOutcome::Resumed { chord: previous }
        );
        assert_eq!(manager.active_chord(), Some(previous));
        assert!(!manager.is_suspended());
    }

    #[test]
    fn manager_rejects_reuse_after_shutdown_and_reports_state_errors() {
        let mut manager =
            HotkeyManager::new(FakeBackend::default(), HotkeyConfig::default_summon()).unwrap();
        assert_eq!(manager.suspend().unwrap_err(), HotkeyError::NotActive);
        assert_eq!(manager.rearm().unwrap_err(), HotkeyError::NotActive);
        manager.activate().unwrap();
        assert_eq!(
            manager.activate().unwrap_err(),
            HotkeyError::AlreadyActive {
                chord: KeyChord::default_summon()
            }
        );
        manager.shutdown().unwrap();
        assert_eq!(manager.resume().unwrap_err(), HotkeyError::Shutdown);
        assert_eq!(manager.rearm().unwrap_err(), HotkeyError::Shutdown);
    }

    // -- recorder ----------------------------------------------------------

    fn down_modifier(modifier: Modifier) -> RecorderEvent {
        RecorderEvent::Down(RecorderKey::Modifier(modifier))
    }

    fn up_modifier(modifier: Modifier) -> RecorderEvent {
        RecorderEvent::Up(RecorderKey::Modifier(modifier))
    }

    fn down_key(key: Key) -> RecorderEvent {
        RecorderEvent::Down(RecorderKey::Key(key))
    }

    fn up_key(key: Key) -> RecorderEvent {
        RecorderEvent::Up(RecorderKey::Key(key))
    }

    #[test]
    fn recorder_ignores_modifier_only_presses_and_displays_native_names() {
        let mut recorder = ShortcutRecorder::new(Platform::MacOs);
        let outcome = recorder.record(down_modifier(Modifier::Control));
        assert!(matches!(outcome, RecordOutcome::ModifierChanged { .. }));
        assert_eq!(recorder.modifier_display(), "Control");
        recorder.record(down_modifier(Modifier::Alt));
        assert_eq!(recorder.modifier_display(), "Control+Option");
        assert_eq!(recorder.candidate(), None);
        assert!(matches!(
            recorder.record(up_modifier(Modifier::Alt)),
            RecordOutcome::Ignored {
                reason: IgnoreReason::ModifierOnly
            }
        ));
        assert_eq!(recorder.phase(), RecorderPhase::Idle);
        assert_eq!(recorder.commit(), Err(RecorderError::NotVerified));
    }

    #[test]
    fn recorder_captures_a_released_chord_and_requires_verification_before_commit() {
        let mut recorder = ShortcutRecorder::new(Platform::Windows);
        recorder.record(down_modifier(Modifier::Control));
        assert!(matches!(
            recorder.record(down_key(Key::SPACE)),
            RecordOutcome::Capturing { .. }
        ));
        assert!(matches!(
            recorder.record(up_key(Key::SPACE)),
            RecordOutcome::Candidate { .. }
        ));
        assert_eq!(recorder.candidate_display().as_deref(), Some("Ctrl+Space"));
        assert_eq!(recorder.commit(), Err(RecorderError::NotVerified));

        let verified = recorder.verify(ConflictCheck::Available).unwrap();
        assert_eq!(verified, KeyChord::default_summon());
        assert_eq!(recorder.phase(), RecorderPhase::Verified);
        assert_eq!(recorder.commit().unwrap(), verified);
        assert_eq!(recorder.phase(), RecorderPhase::Idle);
    }

    #[test]
    fn recorder_conflict_before_save_keeps_the_candidate_unverified() {
        let mut recorder = ShortcutRecorder::new(Platform::Windows);
        recorder.record(down_modifier(Modifier::Control));
        recorder.record(down_key(Key::SPACE));
        recorder.record(up_key(Key::SPACE));
        let error = recorder
            .verify(ConflictCheck::Conflict {
                detail: "owned by the IME".to_owned(),
            })
            .unwrap_err();
        assert!(matches!(error, RecorderError::Conflict { .. }));
        assert_eq!(recorder.phase(), RecorderPhase::Candidate);
        assert_eq!(recorder.commit(), Err(RecorderError::NotVerified));
        assert_eq!(recorder.verified(), None);

        // A later successful verification is allowed to proceed.
        recorder.verify(ConflictCheck::Available).unwrap();
        assert!(recorder.commit().is_ok());
    }

    #[test]
    fn recorder_cancel_returns_the_abandoned_candidate_and_resets() {
        let mut recorder = ShortcutRecorder::new(Platform::Linux);
        recorder.record(down_modifier(Modifier::Meta));
        recorder.record(down_key(Key::letter('k').unwrap()));
        recorder.record(up_key(Key::letter('k').unwrap()));
        let cancel = recorder.cancel();
        assert_eq!(cancel.candidate, Some(chord("meta+k")));
        assert_eq!(cancel.verified, None);
        assert_eq!(recorder.phase(), RecorderPhase::Idle);
        assert_eq!(recorder.candidate(), None);
        assert_eq!(recorder.commit(), Err(RecorderError::NotVerified));
    }

    #[test]
    fn recorder_refuses_unsupported_verification_and_held_keys() {
        let mut recorder = ShortcutRecorder::new(Platform::Windows);
        recorder.record(down_modifier(Modifier::Control));
        let f24 = Key::function(24).unwrap();
        recorder.record(down_key(f24));
        assert!(matches!(
            recorder.verify(ConflictCheck::Available),
            Err(RecorderError::KeyStillHeld)
        ));
        recorder.record(up_key(f24));
        let unsupported = recorder
            .verify(ConflictCheck::Unsupported {
                fallback: "no adapter".to_owned(),
            })
            .unwrap_err();
        assert!(matches!(unsupported, RecorderError::Unsupported { .. }));
        assert_eq!(recorder.phase(), RecorderPhase::Candidate);
        assert_eq!(
            KeyChord::parse("ctrl+hyper").unwrap_err(),
            ChordParseError::UnsupportedToken("hyper".to_owned())
        );
    }

    // -- Windows adapter parity and integration ---------------------------

    #[cfg(windows)]
    #[test]
    fn windows_backend_registers_and_releases_a_free_chord() {
        let candidate = chord("ctrl+alt+shift+f24");
        let mut manager = HotkeyManager::new(
            WindowsHotkeyBackend::new(),
            HotkeyConfig::with_summon(candidate),
        )
        .unwrap();
        match manager.activate() {
            Ok(outcome) => {
                assert!(outcome.is_registered());
                assert_eq!(manager.active_chord(), Some(candidate));
                manager.shutdown().unwrap();
                assert_eq!(manager.active_chord(), None);
            }
            Err(
                HotkeyError::Conflict { detail, .. }
                | HotkeyError::Denied { detail, .. }
                | HotkeyError::Unsupported {
                    fallback: detail, ..
                },
            ) => {
                eprintln!("skipping Windows backend integration test: {detail}");
            }
            Err(error) => panic!("unexpected Windows backend failure: {error}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_adapter_parses_every_neutral_chord_token_identically() {
        let samples = [
            "ctrl+space",
            "ctrl+alt+shift+meta+a",
            "ctrl+0",
            "ctrl+f1",
            "ctrl+f24",
            "shift+enter",
            "alt+tab",
            "meta+escape",
            "ctrl+backspace",
            "ctrl+delete",
            "ctrl+insert",
            "ctrl+home",
            "ctrl+end",
            "ctrl+pageup",
            "ctrl+pagedown",
            "ctrl+up",
            "ctrl+down",
            "ctrl+left",
            "ctrl+right",
            "ctrl+-",
            "ctrl+=",
            "ctrl+[",
            "ctrl+]",
            "ctrl+\\",
            "ctrl+;",
            "ctrl+'",
            "ctrl+,",
            "ctrl+.",
            "ctrl+/",
            "ctrl+`",
        ];
        for sample in samples {
            let neutral = KeyChord::parse(sample).unwrap();
            let native = ansa_platform_windows::KeyChord::parse(&neutral.persisted())
                .unwrap_or_else(|error| panic!("native parser rejected {sample}: {error}"));
            assert_eq!(
                native.persisted(),
                neutral.persisted(),
                "persisted mismatch for {sample}"
            );
            assert_eq!(
                native.render(),
                neutral.render(Platform::Windows),
                "native rendering mismatch for {sample}"
            );
        }
    }
}
