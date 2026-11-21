//! Stable quicklink identities.
//!
//! An identity is `quicklink:<32 lowercase hex digits>`, derived with a fixed
//! FNV-1a 128-bit hash from a user-provided *stable slug* (or an equivalent
//! creation ID). Display titles, aliases and template text never contribute, so
//! renaming a quicklink cannot change its identity. The algorithm and output
//! length are part of the persisted format; changing them requires a migration.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// Prefix of every canonical quicklink identity.
pub const ID_PREFIX: &str = "quicklink:";

/// Maximum number of characters accepted in a stable slug.
pub const MAX_SLUG_CHARS: usize = 128;

const HEX_LEN: usize = 32;

// FNV-1a 128-bit parameters (public domain algorithm).
const FNV_OFFSET_BASIS: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
const FNV_PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

/// A stable, opaque quicklink identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QuicklinkId(String);

/// Reasons a quicklink identity could not be created or parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdError {
    /// A stable slug was empty.
    EmptySlug,
    /// A stable slug exceeded [`MAX_SLUG_CHARS`].
    SlugTooLong { limit: usize },
    /// A stable slug contained a character outside `A-Z a-z 0-9 . _ -`.
    InvalidSlugCharacter { character: char, position: usize },
    /// A canonical identity did not start with `quicklink:`.
    MissingPrefix,
    /// The hex portion had the wrong length.
    BadHexLength { actual: usize },
    /// The hex portion contained a non-lowercase-hex digit.
    InvalidHexCharacter { character: char, position: usize },
}

impl fmt::Display for IdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySlug => {
                formatter.write_str("a stable slug is required to derive a quicklink identity")
            }
            Self::SlugTooLong { limit } => {
                write!(formatter, "stable slug is longer than {limit} characters")
            }
            Self::InvalidSlugCharacter {
                character,
                position,
            } => write!(
                formatter,
                "stable slug contains {character:?} at byte {position}; use only A-Z, a-z, 0-9, '.', '_' and '-'"
            ),
            Self::MissingPrefix => {
                write!(
                    formatter,
                    "quicklink identity must start with '{ID_PREFIX}'"
                )
            }
            Self::BadHexLength { actual } => write!(
                formatter,
                "quicklink identity hex part must be {HEX_LEN} digits, found {actual}"
            ),
            Self::InvalidHexCharacter {
                character,
                position,
            } => write!(
                formatter,
                "quicklink identity contains non-lowercase-hex character {character:?} at byte {position}"
            ),
        }
    }
}

impl std::error::Error for IdError {}

impl QuicklinkId {
    /// Derives an identity from a user-chosen stable slug.
    ///
    /// The slug is validated but is not stored in the identity. The same slug
    /// always derives the same identity on every platform and in every process.
    pub fn from_slug(slug: &str) -> Result<Self, IdError> {
        validate_slug(slug)?;
        Ok(Self(format!(
            "{ID_PREFIX}{}",
            fnv1a_128_hex(slug.as_bytes())
        )))
    }

    /// Derives an identity from a stable creation ID.
    ///
    /// This is the same derivation as [`QuicklinkId::from_slug`]; the distinct
    /// name documents that an opaque creation ID is also acceptable. It must
    /// follow the same character rules so exports stay readable.
    pub fn from_creation_id(creation_id: &str) -> Result<Self, IdError> {
        Self::from_slug(creation_id)
    }

    /// Parses a canonical `quicklink:<hex>` identity.
    pub fn parse(raw: &str) -> Result<Self, IdError> {
        let Some(hex) = raw.strip_prefix(ID_PREFIX) else {
            return Err(IdError::MissingPrefix);
        };
        if hex.len() != HEX_LEN {
            return Err(IdError::BadHexLength { actual: hex.len() });
        }
        for (position, byte) in hex.bytes().enumerate() {
            if !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte) {
                return Err(IdError::InvalidHexCharacter {
                    character: char::from(byte),
                    position,
                });
            }
        }
        Ok(Self(raw.to_owned()))
    }

    /// The canonical `quicklink:<hex>` representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The 32 lowercase hex digits without the namespace prefix.
    pub fn local_hex(&self) -> &str {
        self.0.strip_prefix(ID_PREFIX).unwrap_or(self.0.as_str())
    }
}

impl fmt::Display for QuicklinkId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for QuicklinkId {
    type Err = IdError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::parse(raw)
    }
}

impl TryFrom<&str> for QuicklinkId {
    type Error = IdError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl TryFrom<String> for QuicklinkId {
    type Error = IdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl Serialize for QuicklinkId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for QuicklinkId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(de::Error::custom)
    }
}

fn validate_slug(slug: &str) -> Result<(), IdError> {
    if slug.is_empty() {
        return Err(IdError::EmptySlug);
    }
    let mut count = 0_usize;
    for (position, character) in slug.char_indices() {
        count += 1;
        if !character.is_ascii_alphanumeric() && !matches!(character, '.' | '_' | '-') {
            return Err(IdError::InvalidSlugCharacter {
                character,
                position,
            });
        }
    }
    if count > MAX_SLUG_CHARS {
        return Err(IdError::SlugTooLong {
            limit: MAX_SLUG_CHARS,
        });
    }
    Ok(())
}

/// Lowercase hexadecimal FNV-1a 128-bit hash of arbitrary bytes.
pub(crate) fn fnv1a_128_hex(bytes: &[u8]) -> String {
    let mut hash = FNV_OFFSET_BASIS;
    for &byte in bytes {
        hash ^= u128::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    let mut hex = String::with_capacity(HEX_LEN);
    for shift in (0..HEX_LEN).rev() {
        let nibble = ((hash >> (shift * 4)) & 0x0f) as u8;
        hex.push(char::from(if nibble < 10 {
            b'0' + nibble
        } else {
            b'a' + (nibble - 10)
        }));
    }
    hex
}
