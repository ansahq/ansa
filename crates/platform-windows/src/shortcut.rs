//! Deterministic shortcut normalization and onboarding recorder.
//!
//! The settings store already validates hotkey strings such as `Ctrl+Space`
//! and `shift+meta+z`. This module implements the same grammar as a typed
//! value so the platform adapter can use one representation for parsing,
//! display, persistence and `RegisterHotKey`.
//!
//! Parsing is deliberately strict and deterministic:
//! * at most 64 bytes, no control characters;
//! * one to four modifiers followed by exactly one key;
//! * duplicate modifiers are rejected rather than collapsed;
//! * at least one modifier is required (a bare key would steal typing);
//! * aliases are normalized to a single canonical spelling.
//!
//! The recorder is a small state machine suitable for onboarding. It reports
//! modifier presses, rejects a bare key, and only completes a
//! [`ShortcutChord`] once a non-modifier key arrives with at least one
//! modifier held. It never injects input and has no Windows dependency.

use std::fmt;

/// Maximum length accepted for a textual shortcut, matching the settings store.
pub const MAX_SHORTCUT_TEXT_BYTES: usize = 64;

const VK_BACK: u32 = 0x08;
const VK_TAB: u32 = 0x09;
const VK_RETURN: u32 = 0x0D;
const VK_SHIFT: u32 = 0x10;
const VK_CONTROL: u32 = 0x11;
const VK_MENU: u32 = 0x12;
const VK_ESCAPE: u32 = 0x1B;
const VK_SPACE: u32 = 0x20;
const VK_PRIOR: u32 = 0x21;
const VK_NEXT: u32 = 0x22;
const VK_END: u32 = 0x23;
const VK_HOME: u32 = 0x24;
const VK_LEFT: u32 = 0x25;
const VK_UP: u32 = 0x26;
const VK_RIGHT: u32 = 0x27;
const VK_DOWN: u32 = 0x28;
const VK_INSERT: u32 = 0x2D;
const VK_DELETE: u32 = 0x2E;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;
const VK_F1: u32 = 0x70;
const VK_F24: u32 = 0x87;

const MOD_ALT: u32 = 0x0001;
const MOD_CONTROL: u32 = 0x0002;
const MOD_SHIFT: u32 = 0x0004;
const MOD_WIN: u32 = 0x0008;
const MOD_NOREPEAT: u32 = 0x4000;

/// Why a shortcut string or recorder event was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutError {
    message: String,
}

impl ShortcutError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Returns the human-readable reason.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ShortcutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ShortcutError {}

/// One non-modifier key accepted in a global shortcut.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShortcutKey {
    /// An ASCII letter or digit, stored in canonical uppercase form.
    Character(char),
    /// One of `F1` through `F24`.
    Function(u8),
    /// Space.
    Space,
    /// Enter/Return.
    Enter,
    /// Escape.
    Escape,
    /// Tab.
    Tab,
    /// Backspace.
    Backspace,
    /// Delete.
    Delete,
    /// Insert.
    Insert,
    /// Home.
    Home,
    /// End.
    End,
    /// Page Up.
    PageUp,
    /// Page Down.
    PageDown,
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
}

impl ShortcutKey {
    /// Parses a key name using the settings-store grammar.
    pub fn parse(value: &str) -> Option<Self> {
        if value.chars().count() == 1 {
            let character = value.chars().next()?;
            if character.is_ascii_alphanumeric() {
                return Some(Self::Character(character.to_ascii_uppercase()));
            }
            return None;
        }

        let lower = value.to_ascii_lowercase();
        if let Some(digits) = lower.strip_prefix('f') {
            if let Ok(number) = digits.parse::<u8>() {
                return (1..=24).contains(&number).then_some(Self::Function(number));
            }
        }

        Some(match lower.as_str() {
            "space" => Self::Space,
            "enter" | "return" => Self::Enter,
            "esc" | "escape" => Self::Escape,
            "tab" => Self::Tab,
            "backspace" => Self::Backspace,
            "delete" => Self::Delete,
            "insert" => Self::Insert,
            "home" => Self::Home,
            "end" => Self::End,
            "pageup" => Self::PageUp,
            "pagedown" => Self::PageDown,
            "up" => Self::Up,
            "down" => Self::Down,
            "left" => Self::Left,
            "right" => Self::Right,
            _ => return None,
        })
    }

    /// Returns the Win32 virtual-key code.
    pub fn virtual_key(self) -> u32 {
        match self {
            Self::Character(character) => u32::from(character.to_ascii_uppercase() as u8),
            Self::Function(number) => VK_F1 + u32::from(number.saturating_sub(1)),
            Self::Space => VK_SPACE,
            Self::Enter => VK_RETURN,
            Self::Escape => VK_ESCAPE,
            Self::Tab => VK_TAB,
            Self::Backspace => VK_BACK,
            Self::Delete => VK_DELETE,
            Self::Insert => VK_INSERT,
            Self::Home => VK_HOME,
            Self::End => VK_END,
            Self::PageUp => VK_PRIOR,
            Self::PageDown => VK_NEXT,
            Self::Up => VK_UP,
            Self::Down => VK_DOWN,
            Self::Left => VK_LEFT,
            Self::Right => VK_RIGHT,
        }
    }

    /// Maps a Win32 virtual-key code back to a key.
    pub fn from_virtual_key(virtual_key: u32) -> Option<Self> {
        match virtual_key {
            VK_SPACE => Some(Self::Space),
            VK_RETURN => Some(Self::Enter),
            VK_ESCAPE => Some(Self::Escape),
            VK_TAB => Some(Self::Tab),
            VK_BACK => Some(Self::Backspace),
            VK_DELETE => Some(Self::Delete),
            VK_INSERT => Some(Self::Insert),
            VK_HOME => Some(Self::Home),
            VK_END => Some(Self::End),
            VK_PRIOR => Some(Self::PageUp),
            VK_NEXT => Some(Self::PageDown),
            VK_UP => Some(Self::Up),
            VK_DOWN => Some(Self::Down),
            VK_LEFT => Some(Self::Left),
            VK_RIGHT => Some(Self::Right),
            VK_F1..=VK_F24 => Some(Self::Function((virtual_key - VK_F1 + 1) as u8)),
            value if (0x30..=0x39).contains(&value) || (0x41..=0x5A).contains(&value) => {
                char::from_u32(value).map(Self::Character)
            }
            _ => None,
        }
    }

    /// Returns the canonical display spelling.
    pub fn canonical_name(self) -> String {
        match self {
            Self::Character(character) => character.to_string(),
            Self::Function(number) => format!("F{number}"),
            Self::Space => "Space".to_owned(),
            Self::Enter => "Enter".to_owned(),
            Self::Escape => "Escape".to_owned(),
            Self::Tab => "Tab".to_owned(),
            Self::Backspace => "Backspace".to_owned(),
            Self::Delete => "Delete".to_owned(),
            Self::Insert => "Insert".to_owned(),
            Self::Home => "Home".to_owned(),
            Self::End => "End".to_owned(),
            Self::PageUp => "PageUp".to_owned(),
            Self::PageDown => "PageDown".to_owned(),
            Self::Up => "Up".to_owned(),
            Self::Down => "Down".to_owned(),
            Self::Left => "Left".to_owned(),
            Self::Right => "Right".to_owned(),
        }
    }
}

/// A validated global shortcut: one or more modifiers plus one key.
///
/// The value cannot be constructed in an invalid state: [`Self::new`] and
/// [`Self::parse`] reject modifier-only chords and bare keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShortcutChord {
    ctrl: bool,
    alt: bool,
    shift: bool,
    win: bool,
    key: ShortcutKey,
}

impl ShortcutChord {
    /// Creates a chord, requiring at least one modifier.
    pub fn new(
        ctrl: bool,
        alt: bool,
        shift: bool,
        win: bool,
        key: ShortcutKey,
    ) -> Result<Self, ShortcutError> {
        if !(ctrl || alt || shift || win) {
            return Err(ShortcutError::new(
                "a global shortcut needs at least one modifier",
            ));
        }
        Ok(Self {
            ctrl,
            alt,
            shift,
            win,
            key,
        })
    }

    /// Returns the default summon shortcut, `Ctrl+Space`.
    pub fn default_summon() -> Self {
        Self {
            ctrl: true,
            alt: false,
            shift: false,
            win: false,
            key: ShortcutKey::Space,
        }
    }

    /// Parses a shortcut string with the settings-store grammar.
    pub fn parse(text: &str) -> Result<Self, ShortcutError> {
        if text.is_empty() || text.len() > MAX_SHORTCUT_TEXT_BYTES {
            return Err(ShortcutError::new(format!(
                "a shortcut must be 1..={MAX_SHORTCUT_TEXT_BYTES} bytes"
            )));
        }
        if text.chars().any(char::is_control) {
            return Err(ShortcutError::new(
                "a shortcut must not contain control characters",
            ));
        }

        let parts: Vec<&str> = text.split('+').collect();
        if parts.len() < 2 || parts.len() > 5 {
            return Err(ShortcutError::new(
                "a shortcut must have one to four modifiers plus a key",
            ));
        }
        if parts.iter().any(|part| part.is_empty()) {
            return Err(ShortcutError::new(
                "a shortcut must not contain empty components",
            ));
        }

        let (key_text, modifier_texts) = parts
            .split_last()
            .ok_or_else(|| ShortcutError::new("a shortcut must not be empty"))?;
        let key = ShortcutKey::parse(key_text)
            .ok_or_else(|| ShortcutError::new("the shortcut key is not recognised"))?;

        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut win = false;
        let mut seen = 0_u8;
        for modifier in modifier_texts {
            let slot = match modifier.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => &mut ctrl,
                "alt" | "option" => &mut alt,
                "shift" => &mut shift,
                "super" | "meta" | "cmd" | "command" | "win" | "windows" => &mut win,
                _ => {
                    return Err(ShortcutError::new(
                        "every component before the key must be a modifier",
                    ))
                }
            };
            if *slot {
                return Err(ShortcutError::new("a shortcut must not repeat a modifier"));
            }
            *slot = true;
            seen += 1;
        }
        if seen == 0 {
            return Err(ShortcutError::new(
                "a global shortcut needs at least one modifier",
            ));
        }

        Self::new(ctrl, alt, shift, win, key)
    }

    /// Whether Control is part of the chord.
    pub fn ctrl(&self) -> bool {
        self.ctrl
    }

    /// Whether Alt is part of the chord.
    pub fn alt(&self) -> bool {
        self.alt
    }

    /// Whether Shift is part of the chord.
    pub fn shift(&self) -> bool {
        self.shift
    }

    /// Whether the Windows/Super key is part of the chord.
    pub fn win(&self) -> bool {
        self.win
    }

    /// Returns the non-modifier key.
    pub fn key(&self) -> ShortcutKey {
        self.key
    }

    /// Returns the canonical modifiers in `RegisterHotKey` form.
    pub fn win32_modifiers(&self) -> u32 {
        let mut modifiers = MOD_NOREPEAT;
        if self.alt {
            modifiers |= MOD_ALT;
        }
        if self.ctrl {
            modifiers |= MOD_CONTROL;
        }
        if self.shift {
            modifiers |= MOD_SHIFT;
        }
        if self.win {
            modifiers |= MOD_WIN;
        }
        modifiers
    }

    /// Returns the Win32 virtual-key code for the non-modifier key.
    pub fn win32_virtual_key(&self) -> u32 {
        self.key.virtual_key()
    }

    /// Returns the canonical, deterministic display form.
    pub fn canonical(&self) -> String {
        let mut parts = Vec::with_capacity(5);
        if self.ctrl {
            parts.push("Ctrl".to_owned());
        }
        if self.alt {
            parts.push("Alt".to_owned());
        }
        if self.shift {
            parts.push("Shift".to_owned());
        }
        if self.win {
            parts.push("Super".to_owned());
        }
        parts.push(self.key.canonical_name());
        parts.join("+")
    }

    /// Returns the canonical form split into modifier labels.
    pub fn modifier_labels(&self) -> Vec<&'static str> {
        let mut labels = Vec::with_capacity(4);
        if self.ctrl {
            labels.push("Ctrl");
        }
        if self.alt {
            labels.push("Alt");
        }
        if self.shift {
            labels.push("Shift");
        }
        if self.win {
            labels.push("Super");
        }
        labels
    }

    /// Reports whether the chord uses the Windows/Super modifier.
    pub fn uses_win_key(&self) -> bool {
        self.win
    }
}

impl Default for ShortcutChord {
    fn default() -> Self {
        Self::default_summon()
    }
}

impl fmt::Display for ShortcutChord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.canonical())
    }
}

impl std::str::FromStr for ShortcutChord {
    type Err = ShortcutError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// The result of feeding one key event to [`ShortcutRecorder`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecorderEvent {
    /// A modifier was pressed or released; `display` lists held modifiers.
    ModifiersChanged {
        /// Canonical modifier labels currently held, joined with `+`.
        display: String,
    },
    /// A non-modifier key arrived without a modifier and was ignored.
    Pending {
        /// Canonical modifier labels currently held, joined with `+`.
        display: String,
    },
    /// A complete, valid chord was recorded.
    Completed(ShortcutChord),
    /// The event cannot be used as a global shortcut.
    Rejected {
        /// Human-readable reason for the UI.
        reason: String,
    },
}

/// A deterministic onboarding recorder.
///
/// Feed native key-down/key-up events in order. The recorder owns no input
/// hook and never simulates a key; the caller supplies virtual-key codes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShortcutRecorder {
    ctrl: bool,
    alt: bool,
    shift: bool,
    win: bool,
}

impl ShortcutRecorder {
    /// Creates an empty recorder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears all held modifiers.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Returns `true` while at least one modifier is held.
    pub fn is_recording(&self) -> bool {
        self.ctrl || self.alt || self.shift || self.win
    }

    /// Returns the canonical labels of the modifiers currently held.
    pub fn pressed_display(&self) -> String {
        let mut labels = Vec::with_capacity(4);
        if self.ctrl {
            labels.push("Ctrl");
        }
        if self.alt {
            labels.push("Alt");
        }
        if self.shift {
            labels.push("Shift");
        }
        if self.win {
            labels.push("Super");
        }
        labels.join("+")
    }

    /// Feeds one key-down event.
    pub fn key_down(&mut self, virtual_key: u32) -> RecorderEvent {
        match virtual_key {
            VK_CONTROL => {
                self.ctrl = true;
                return RecorderEvent::ModifiersChanged {
                    display: self.pressed_display(),
                };
            }
            VK_MENU => {
                self.alt = true;
                return RecorderEvent::ModifiersChanged {
                    display: self.pressed_display(),
                };
            }
            VK_SHIFT => {
                self.shift = true;
                return RecorderEvent::ModifiersChanged {
                    display: self.pressed_display(),
                };
            }
            VK_LWIN | VK_RWIN => {
                self.win = true;
                return RecorderEvent::ModifiersChanged {
                    display: self.pressed_display(),
                };
            }
            _ => {}
        }

        let Some(key) = ShortcutKey::from_virtual_key(virtual_key) else {
            return RecorderEvent::Rejected {
                reason: "that key cannot be used in a global shortcut".to_owned(),
            };
        };
        if !self.is_recording() {
            return RecorderEvent::Rejected {
                reason: "hold at least one modifier before the key".to_owned(),
            };
        }

        match ShortcutChord::new(self.ctrl, self.alt, self.shift, self.win, key) {
            Ok(chord) => RecorderEvent::Completed(chord),
            Err(error) => RecorderEvent::Rejected {
                reason: error.to_string(),
            },
        }
    }

    /// Feeds one key-up event. Returns `true` when a modifier was released.
    pub fn key_up(&mut self, virtual_key: u32) -> bool {
        match virtual_key {
            VK_CONTROL => std::mem::take(&mut self.ctrl),
            VK_MENU => std::mem::take(&mut self.alt),
            VK_SHIFT => std::mem::take(&mut self.shift),
            VK_LWIN | VK_RWIN => std::mem::take(&mut self.win),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_settings_corpus_and_normalizes_aliases() {
        let cases = [
            ("Ctrl+Space", "Ctrl+Space"),
            ("Ctrl+Shift+K", "Ctrl+Shift+K"),
            ("Alt+F4", "Alt+F4"),
            ("Super+Enter", "Super+Enter"),
            ("Control+Alt+1", "Ctrl+Alt+1"),
            ("Ctrl+PageDown", "Ctrl+PageDown"),
            ("shift+meta+z", "Shift+Super+Z"),
            ("windows+option+esc", "Alt+Super+Escape"),
        ];

        for (input, canonical) in cases {
            let chord = ShortcutChord::parse(input)
                .unwrap_or_else(|error| panic!("{input:?} should parse: {error}"));
            assert_eq!(chord.canonical(), canonical, "input {input:?}");
            assert_eq!(chord.to_string(), canonical);
        }
    }

    #[test]
    fn rejects_invalid_shortcut_corpus() {
        let invalid = [
            "",
            "Space",
            "Ctrl+",
            "+Space",
            "Ctrl+Shift",
            "Ctrl+Ctrl+K",
            "Ctrl+Control+K",
            "Hyper+K",
            "Ctrl+K+J",
            "Ctrl+NotAKey",
            "Ctrl+F25",
            "Ctrl+Space\n",
            "Ctrl+Control+Alt+Shift+Super+K",
        ];
        for input in invalid {
            assert!(
                ShortcutChord::parse(input).is_err(),
                "{input:?} must be rejected"
            );
        }
    }

    #[test]
    fn default_summon_is_ctrl_space() {
        let chord = ShortcutChord::default_summon();
        assert_eq!(chord.canonical(), "Ctrl+Space");
        assert_eq!(chord.win32_modifiers() & 0x000F, 0x0002);
        assert_eq!(chord.win32_modifiers() & MOD_NOREPEAT, MOD_NOREPEAT);
        assert_eq!(chord.win32_virtual_key(), VK_SPACE);
        assert!(!chord.uses_win_key());
        assert_eq!(chord.modifier_labels(), vec!["Ctrl"]);
    }

    #[test]
    fn recorder_requires_a_modifier_and_completes_deterministically() {
        let mut recorder = ShortcutRecorder::new();

        assert_eq!(
            recorder.key_down(VK_SPACE),
            RecorderEvent::Rejected {
                reason: "hold at least one modifier before the key".to_owned()
            }
        );

        assert_eq!(
            recorder.key_down(VK_CONTROL),
            RecorderEvent::ModifiersChanged {
                display: "Ctrl".to_owned()
            }
        );
        assert_eq!(
            recorder.key_down(0x4B),
            RecorderEvent::Completed(ShortcutChord::parse("Ctrl+K").expect("valid fixture"))
        );
        assert!(recorder.key_up(VK_CONTROL));
        assert!(!recorder.is_recording());
        assert!(!recorder.key_up(VK_CONTROL), "key-up is idempotent");
    }

    #[test]
    fn recorder_maps_function_and_navigation_keys() {
        let mut recorder = ShortcutRecorder::new();
        recorder.key_down(VK_MENU);
        recorder.key_down(VK_SHIFT);
        assert_eq!(
            recorder.key_down(VK_F24),
            RecorderEvent::Completed(ShortcutChord::parse("Alt+Shift+F24").expect("valid fixture"))
        );

        recorder.clear();
        recorder.key_down(VK_LWIN);
        assert_eq!(
            recorder.key_down(VK_NEXT),
            RecorderEvent::Completed(
                ShortcutChord::parse("Super+PageDown").expect("valid fixture")
            )
        );

        assert_eq!(
            recorder.key_down(0xE5),
            RecorderEvent::Rejected {
                reason: "that key cannot be used in a global shortcut".to_owned()
            }
        );
    }

    #[test]
    fn virtual_key_round_trip_covers_the_supported_set() {
        let keys = [
            ShortcutKey::Character('Z'),
            ShortcutKey::Character('0'),
            ShortcutKey::Function(1),
            ShortcutKey::Function(24),
            ShortcutKey::Space,
            ShortcutKey::Enter,
            ShortcutKey::Escape,
            ShortcutKey::Tab,
            ShortcutKey::Delete,
            ShortcutKey::Insert,
            ShortcutKey::Home,
            ShortcutKey::End,
            ShortcutKey::PageUp,
            ShortcutKey::PageDown,
            ShortcutKey::Up,
            ShortcutKey::Down,
            ShortcutKey::Left,
            ShortcutKey::Right,
        ];

        for key in keys {
            assert_eq!(
                ShortcutKey::from_virtual_key(key.virtual_key()),
                Some(key),
                "{key:?}"
            );
            let parsed = ShortcutKey::parse(&key.canonical_name());
            assert_eq!(parsed, Some(key), "{key:?}");
        }
    }
}
