//! RFC 3986 percent-encoding and literal path-segment rules.
//!
//! The encoder is implemented locally so that the crate has no encoding
//! dependency and so that the exact byte policy is testable: only RFC 3986
//! *unreserved* bytes (`A-Z a-z 0-9 - . _ ~`) pass through unchanged. Every
//! other byte of the UTF-8 encoding becomes `%XX` with uppercase hex digits.
//! A space is always `%20`; `+` is never produced.

use std::fmt;

const HEX: [u8; 16] = *b"0123456789ABCDEF";

/// Returns `true` for the RFC 3986 unreserved byte set.
pub fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

/// Percent-encodes one UTF-8 string as a single URI component.
///
/// This is suitable for both URL path segments and URL query values. It never
/// emits `+` for a space and never decodes or normalizes existing `%` escapes:
/// a literal percent sign becomes `%25`.
pub fn encode_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for &byte in value.as_bytes() {
        if is_unreserved(byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

/// Why a value could not be used as a literal (non-URL-encoded) path segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeError {
    /// The value contains NUL or another control character.
    ControlCharacter { character: char, position: usize },
    /// The value contains a path separator and could escape its segment.
    PathSeparator { character: char, position: usize },
    /// The value is exactly `..`, which traverses to the parent directory.
    ParentTraversal,
    /// The value contains an invisible bidi/zero-width format character.
    UnsafeFormatCharacter { character: char, position: usize },
}

impl fmt::Display for EncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ControlCharacter {
                character,
                position,
            } => write!(
                formatter,
                "literal path segment contains control character {character:?} at byte {position}"
            ),
            Self::PathSeparator {
                character,
                position,
            } => write!(
                formatter,
                "literal path segment contains path separator {character:?} at byte {position}; put separators in the template literal instead"
            ),
            Self::ParentTraversal => formatter.write_str(
                "literal path segment must not be '..'; parent traversal is not allowed",
            ),
            Self::UnsafeFormatCharacter {
                character,
                position,
            } => write!(
                formatter,
                "literal path segment contains invisible format character U+{:04X} at byte {position}",
                *character as u32
            ),
        }
    }
}

impl std::error::Error for EncodeError {}

/// Validates and returns a value used verbatim as one filesystem path segment.
///
/// Unlike [`encode_component`], this performs no percent-encoding: spaces,
/// quotes, percent signs and Unicode remain literal. Separators, control
/// characters and `..` are rejected because they would let one argument change
/// the directory structure of the resolved path.
pub(crate) fn encode_literal_segment(value: &str) -> Result<String, EncodeError> {
    for (position, character) in value.char_indices() {
        if character == '\0' || character.is_control() {
            return Err(EncodeError::ControlCharacter {
                character,
                position,
            });
        }
        if character == '/' || character == '\\' {
            return Err(EncodeError::PathSeparator {
                character,
                position,
            });
        }
        if crate::text::is_unsafe_format_character(character) {
            return Err(EncodeError::UnsafeFormatCharacter {
                character,
                position,
            });
        }
    }
    if value == ".." {
        return Err(EncodeError::ParentTraversal);
    }
    Ok(value.to_owned())
}
