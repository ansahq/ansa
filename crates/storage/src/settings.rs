//! Revisioned, validated settings.

use serde::{Deserialize, Serialize};

use crate::error::StorageError;

/// Appearance preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// Follow the operating system.
    System,
    /// Force the light palette.
    Light,
    /// Force the dark palette.
    Dark,
}

/// How the launcher window should behave when invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchMode {
    /// Toggle visibility.
    Toggle,
    /// Show and focus without toggling.
    Show,
}

/// Known preference keys. Unknown keys are rejected instead of silently
/// persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingKey {
    /// Global activation shortcut, e.g. `Ctrl+Space`.
    Hotkey,
    /// Prefix used for command aliases.
    AliasPrefix,
    /// Appearance selection.
    Theme,
    /// Window invocation behaviour.
    LaunchMode,
    /// Maximum number of rendered results.
    MaximumResults,
}

impl SettingKey {
    /// Every supported key, useful for diagnostics and tests.
    pub const ALL: [Self; 5] = [
        Self::Hotkey,
        Self::AliasPrefix,
        Self::Theme,
        Self::LaunchMode,
        Self::MaximumResults,
    ];

    /// Stable database key.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hotkey => "hotkey",
            Self::AliasPrefix => "alias_prefix",
            Self::Theme => "theme",
            Self::LaunchMode => "launch_mode",
            Self::MaximumResults => "maximum_results",
        }
    }

    /// Parse a stable database key.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.as_str() == value)
    }

    /// The safe default used when a stored value is missing, corrupt or
    /// invalid.
    #[must_use]
    pub fn default_value(self) -> SettingValue {
        match self {
            Self::Hotkey => SettingValue::Hotkey("Ctrl+Space".to_owned()),
            Self::AliasPrefix => SettingValue::AliasPrefix("go".to_owned()),
            Self::Theme => SettingValue::Theme(Theme::System),
            Self::LaunchMode => SettingValue::LaunchMode(LaunchMode::Toggle),
            Self::MaximumResults => SettingValue::MaximumResults(50),
        }
    }

    /// Validate that `value` belongs to this key and satisfies its grammar.
    pub fn validate_value(self, value: &SettingValue) -> Result<(), StorageError> {
        match (self, value) {
            (Self::Hotkey, SettingValue::Hotkey(text)) => validate_hotkey(text),
            (Self::AliasPrefix, SettingValue::AliasPrefix(text)) => validate_alias(text),
            (Self::Theme, SettingValue::Theme(_)) => Ok(()),
            (Self::LaunchMode, SettingValue::LaunchMode(_)) => Ok(()),
            (Self::MaximumResults, SettingValue::MaximumResults(limit)) => {
                if (1..=500).contains(limit) {
                    Ok(())
                } else {
                    Err(StorageError::InvalidSetting {
                        key: self.as_str(),
                        reason: "must be between 1 and 500".to_owned(),
                    })
                }
            }
            (_, _) => Err(StorageError::InvalidSetting {
                key: self.as_str(),
                reason: "value type does not match the setting key".to_owned(),
            }),
        }
    }
}

/// A typed setting value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum SettingValue {
    /// Activation shortcut.
    Hotkey(String),
    /// Command alias prefix.
    AliasPrefix(String),
    /// Appearance.
    Theme(Theme),
    /// Invocation behaviour.
    LaunchMode(LaunchMode),
    /// Bounded result count.
    MaximumResults(u32),
}

/// Result of reading a setting.
#[derive(Debug)]
pub struct SettingSnapshot {
    /// Logical key.
    pub key: SettingKey,
    /// Parsed value, or the safe default when `diagnostic` is set.
    pub value: SettingValue,
    /// Stored revision, present whenever a row exists.
    pub revision: Option<u64>,
    /// Last update time, present whenever a row exists.
    pub updated_at: Option<i64>,
    /// Present when the stored row could not be used. The row is never
    /// modified by a read.
    pub diagnostic: Option<StorageError>,
}

pub(crate) fn encode_value(key: SettingKey, value: &SettingValue) -> Result<String, StorageError> {
    key.validate_value(value)?;
    serde_json::to_string(value).map_err(|err| StorageError::InvalidSetting {
        key: key.as_str(),
        reason: err.to_string(),
    })
}

pub(crate) fn decode_value(key: SettingKey, json: &str) -> Result<SettingValue, StorageError> {
    let value: SettingValue = serde_json::from_str(json)
        .map_err(|_| StorageError::InvalidStoredValue { key: key.as_str() })?;
    key.validate_value(&value)
        .map_err(|_| StorageError::InvalidStoredValue { key: key.as_str() })?;
    Ok(value)
}

fn validate_alias(value: &str) -> Result<(), StorageError> {
    let length = value.len();
    if !(1..=32).contains(&length) {
        return Err(StorageError::InvalidSetting {
            key: SettingKey::AliasPrefix.as_str(),
            reason: "alias must be 1..=32 bytes".to_owned(),
        });
    }
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(StorageError::InvalidSetting {
            key: SettingKey::AliasPrefix.as_str(),
            reason: "alias must not be empty".to_owned(),
        });
    };
    if !first.is_ascii_alphanumeric() {
        return Err(StorageError::InvalidSetting {
            key: SettingKey::AliasPrefix.as_str(),
            reason: "alias must start with an ASCII letter or digit".to_owned(),
        });
    }
    if chars.any(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))) {
        return Err(StorageError::InvalidSetting {
            key: SettingKey::AliasPrefix.as_str(),
            reason: "alias may only contain ASCII letters, digits, '.', '_' and '-'".to_owned(),
        });
    }
    Ok(())
}

fn validate_hotkey(value: &str) -> Result<(), StorageError> {
    if value.is_empty() || value.len() > 64 {
        return Err(StorageError::InvalidSetting {
            key: SettingKey::Hotkey.as_str(),
            reason: "hotkey must be 1..=64 bytes".to_owned(),
        });
    }
    if value.chars().any(char::is_control) {
        return Err(StorageError::InvalidSetting {
            key: SettingKey::Hotkey.as_str(),
            reason: "hotkey must not contain control characters".to_owned(),
        });
    }

    let parts: Vec<&str> = value.split('+').collect();
    if parts.len() < 2 || parts.len() > 5 {
        return Err(StorageError::InvalidSetting {
            key: SettingKey::Hotkey.as_str(),
            reason: "hotkey must have one to four modifiers plus a key".to_owned(),
        });
    }
    if parts.iter().any(|part| part.is_empty()) {
        return Err(StorageError::InvalidSetting {
            key: SettingKey::Hotkey.as_str(),
            reason: "hotkey contains an empty component".to_owned(),
        });
    }

    let (key, modifiers) = parts
        .split_last()
        .ok_or_else(|| StorageError::InvalidSetting {
            key: SettingKey::Hotkey.as_str(),
            reason: "hotkey is empty".to_owned(),
        })?;
    let mut seen = Vec::with_capacity(modifiers.len());
    for modifier in modifiers {
        let Some(normalized) = normalize_modifier(modifier) else {
            return Err(StorageError::InvalidSetting {
                key: SettingKey::Hotkey.as_str(),
                reason: "every component before the key must be a modifier".to_owned(),
            });
        };
        if seen.contains(&normalized) {
            return Err(StorageError::InvalidSetting {
                key: SettingKey::Hotkey.as_str(),
                reason: "hotkey contains a duplicate modifier".to_owned(),
            });
        }
        seen.push(normalized);
    }
    if seen.is_empty() {
        return Err(StorageError::InvalidSetting {
            key: SettingKey::Hotkey.as_str(),
            reason: "hotkey needs at least one modifier".to_owned(),
        });
    }
    if !is_valid_key(key) {
        return Err(StorageError::InvalidSetting {
            key: SettingKey::Hotkey.as_str(),
            reason: "hotkey key component is not recognised".to_owned(),
        });
    }
    Ok(())
}

fn normalize_modifier(value: &str) -> Option<&'static str> {
    match value.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some("ctrl"),
        "shift" => Some("shift"),
        "alt" | "option" => Some("alt"),
        "super" | "meta" | "cmd" | "command" | "win" | "windows" => Some("super"),
        _ => None,
    }
}

fn is_valid_key(value: &str) -> bool {
    if value.chars().count() == 1 {
        return value
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric());
    }
    let lower = value.to_ascii_lowercase();
    if let Some(digits) = lower.strip_prefix('f')
        && let Ok(number) = digits.parse::<u8>()
    {
        return (1..=24).contains(&number);
    }
    matches!(
        lower.as_str(),
        "space"
            | "enter"
            | "return"
            | "esc"
            | "escape"
            | "tab"
            | "backspace"
            | "delete"
            | "insert"
            | "home"
            | "end"
            | "pageup"
            | "pagedown"
            | "up"
            | "down"
            | "left"
            | "right"
    )
}
