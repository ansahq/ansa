//! Embedded, versioned emoji dataset with checksum verification.
//!
//! The dataset is compiled into the binary with [`include_str!`]. Loading
//! performs no network access. A dataset copy loaded through
//! [`load_from_json`] is verified with the same pinned FNV-1a 64 checksum
//! rule, so a tampered or stale file fails with
//! [`DatasetError::ChecksumMismatch`] instead of being silently accepted.

use crate::search::SearchIndex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::sync::OnceLock;
use unicode_segmentation::UnicodeSegmentation;

const EMBEDDED_JSON: &str = include_str!("../data/emoji.json");

/// Checksum recorded by the checked-in dataset. Changing the data requires
/// regenerating this value together with the file.
pub const PINNED_CHECKSUM: &str = "e80345152b0e31eb";

/// Maximum accepted byte length of a caller-supplied dataset document.
pub const MAX_DATASET_BYTES: usize = 1_048_576;

/// Maximum accepted number of entries in a caller-supplied dataset.
pub const MAX_DATASET_ENTRIES: usize = 10_000;

/// A dataset loading or validation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatasetError {
    /// The JSON text could not be parsed.
    Parse(String),
    /// The dataset could not be re-serialized for checksum computation.
    Serialize(String),
    /// The stored checksum does not match the canonical data or the pin.
    ChecksumMismatch { expected: String, actual: String },
    /// A caller-supplied document exceeds the byte budget.
    InputTooLarge { limit: usize },
    /// A caller-supplied dataset has too many entries.
    TooManyEntries { limit: usize },
    /// The dataset contains no entries.
    EmptyDataset,
    /// Two entries share an identity.
    DuplicateId { id: String },
    /// A base or variant value is not exactly one grapheme cluster.
    InvalidGrapheme { id: String, value: String },
    /// An entry has no accessible name.
    EmptyName { id: String },
    /// A variant has no accessible name.
    EmptyVariantName { id: String },
    /// Two variants of one entry have the same accessible name.
    DuplicateVariantName { id: String, name: String },
}

impl fmt::Display for DatasetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(message) => write!(f, "emoji dataset is not valid JSON: {message}"),
            Self::Serialize(message) => {
                write!(f, "emoji dataset could not be canonicalized: {message}")
            }
            Self::ChecksumMismatch { expected, actual } => write!(
                f,
                "emoji dataset checksum mismatch: expected {expected}, got {actual}"
            ),
            Self::InputTooLarge { limit } => {
                write!(f, "emoji dataset input is larger than {limit} bytes")
            }
            Self::TooManyEntries { limit } => {
                write!(f, "emoji dataset has more than {limit} entries")
            }
            Self::EmptyDataset => f.write_str("emoji dataset has no entries"),
            Self::DuplicateId { id } => write!(f, "emoji dataset has duplicate id {id:?}"),
            Self::InvalidGrapheme { id, value } => {
                write!(
                    f,
                    "emoji entry {id:?} is not one grapheme cluster: {value:?}"
                )
            }
            Self::EmptyName { id } => write!(f, "emoji entry {id:?} has no accessible name"),
            Self::EmptyVariantName { id } => {
                write!(
                    f,
                    "emoji entry {id:?} has a variant with no accessible name"
                )
            }
            Self::DuplicateVariantName { id, name } => write!(
                f,
                "emoji entry {id:?} has duplicate variant accessible name {name:?}"
            ),
        }
    }
}

impl std::error::Error for DatasetError {}

/// One explicit variant of an entry, such as a skin tone or a text
/// presentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variant {
    name: String,
    grapheme: String,
}

impl Variant {
    /// Accessible name, e.g. `"Waving Hand: dark skin tone"`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The exact grapheme to copy, always one cluster for checked-in data.
    pub fn grapheme(&self) -> &str {
        &self.grapheme
    }

    /// The name a screen reader should announce.
    pub fn accessible_name(&self) -> &str {
        &self.name
    }
}

/// One curated emoji or symbol entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    id: String,
    name: String,
    keywords: Vec<String>,
    group: String,
    base: String,
    variants: Vec<Variant>,
}

impl Entry {
    /// Stable identity used for recents and favorites.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Human and screen-reader name of the base grapheme.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Search keywords supplied by the curated CLDR-style data.
    pub fn keywords(&self) -> &[String] {
        &self.keywords
    }

    /// Coarse group such as `"Smileys & Emotion"`.
    pub fn group(&self) -> &str {
        &self.group
    }

    /// Default grapheme to copy.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Explicit skin-tone or presentation variants.
    pub fn variants(&self) -> &[Variant] {
        &self.variants
    }

    /// The accessible name for the base grapheme.
    pub fn accessible_name(&self) -> &str {
        &self.name
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawDataset {
    version: String,
    unicode_version: String,
    source: String,
    license: String,
    license_url: String,
    checksum: String,
    entries: Vec<Entry>,
}

/// The validated, indexed dataset.
#[derive(Debug)]
pub struct Dataset {
    version: String,
    unicode_version: String,
    source: String,
    license: String,
    license_url: String,
    checksum: String,
    entries: Vec<Entry>,
    index: Vec<SearchIndex>,
}

impl Dataset {
    fn from_json_with_expected(json: &str, expected: &str) -> Result<Self, DatasetError> {
        if json.len() > MAX_DATASET_BYTES {
            return Err(DatasetError::InputTooLarge {
                limit: MAX_DATASET_BYTES,
            });
        }
        let raw: RawDataset =
            serde_json::from_str(json).map_err(|error| DatasetError::Parse(error.to_string()))?;
        if raw.entries.len() > MAX_DATASET_ENTRIES {
            return Err(DatasetError::TooManyEntries {
                limit: MAX_DATASET_ENTRIES,
            });
        }
        let actual = canonical_checksum(&raw)?;
        if raw.checksum != actual {
            return Err(DatasetError::ChecksumMismatch {
                expected: raw.checksum,
                actual,
            });
        }
        if raw.checksum != expected {
            return Err(DatasetError::ChecksumMismatch {
                expected: expected.to_owned(),
                actual: raw.checksum,
            });
        }
        validate(&raw)?;
        let index = raw.entries.iter().map(SearchIndex::new).collect();
        Ok(Self {
            version: raw.version,
            unicode_version: raw.unicode_version,
            source: raw.source,
            license: raw.license,
            license_url: raw.license_url,
            checksum: raw.checksum,
            entries: raw.entries,
            index,
        })
    }

    /// Dataset version, independent of the Unicode release.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Unicode release the curated data was derived from.
    pub fn unicode_version(&self) -> &str {
        &self.unicode_version
    }

    /// Provenance note recorded with the data.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Dataset license name.
    pub fn license(&self) -> &str {
        &self.license
    }

    /// Dataset license URL.
    pub fn license_url(&self) -> &str {
        &self.license_url
    }

    /// Verified canonical checksum.
    pub fn checksum(&self) -> &str {
        &self.checksum
    }

    /// All entries in curated order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Number of curated entries.
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Looks up an entry by stable identity.
    pub fn find(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    pub(crate) fn index(&self) -> &[SearchIndex] {
        &self.index
    }
}

/// Library-owned dataset info suitable for an about or diagnostics surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostics {
    pub version: String,
    pub unicode_version: String,
    pub source: String,
    pub license: String,
    pub license_url: String,
    pub checksum: String,
    pub entry_count: usize,
}

/// Returns the embedded dataset, loading and verifying it once against
/// [`PINNED_CHECKSUM`].
pub fn dataset() -> Result<&'static Dataset, DatasetError> {
    static DATASET: OnceLock<Result<Dataset, DatasetError>> = OnceLock::new();
    DATASET
        .get_or_init(|| Dataset::from_json_with_expected(EMBEDDED_JSON, PINNED_CHECKSUM))
        .as_ref()
        .map_err(DatasetError::clone)
}

/// Loads a dataset copy from JSON pinned to [`PINNED_CHECKSUM`]. Intended for
/// tests and host tooling; no network access is performed. A tampered document
/// with a recomputed self-consistent checksum is rejected.
pub fn load_from_json(json: &str) -> Result<Dataset, DatasetError> {
    Dataset::from_json_with_expected(json, PINNED_CHECKSUM)
}

/// Loads a dataset copy pinned to an explicit checksum. Use this for fixture
/// or regeneration workflows that intentionally use a non-production dataset;
/// the document must still be internally consistent.
pub fn load_from_json_with_checksum(
    json: &str,
    expected_checksum: &str,
) -> Result<Dataset, DatasetError> {
    Dataset::from_json_with_expected(json, expected_checksum)
}

/// Recomputes the canonical FNV-1a 64 checksum for a JSON document without
/// validating or pinning it. Intended for regeneration tooling.
pub fn compute_checksum_for_json(json: &str) -> Result<String, DatasetError> {
    if json.len() > MAX_DATASET_BYTES {
        return Err(DatasetError::InputTooLarge {
            limit: MAX_DATASET_BYTES,
        });
    }
    let raw: RawDataset =
        serde_json::from_str(json).map_err(|error| DatasetError::Parse(error.to_string()))?;
    if raw.entries.len() > MAX_DATASET_ENTRIES {
        return Err(DatasetError::TooManyEntries {
            limit: MAX_DATASET_ENTRIES,
        });
    }
    canonical_checksum(&raw)
}

/// Returns dataset version, source, license, checksum and entry count.
pub fn diagnostics() -> Result<Diagnostics, DatasetError> {
    let dataset = dataset()?;
    Ok(Diagnostics {
        version: dataset.version().to_owned(),
        unicode_version: dataset.unicode_version().to_owned(),
        source: dataset.source().to_owned(),
        license: dataset.license().to_owned(),
        license_url: dataset.license_url().to_owned(),
        checksum: dataset.checksum().to_owned(),
        entry_count: dataset.entry_count(),
    })
}

/// Whether `text` is exactly one extended grapheme cluster.
pub fn is_single_grapheme(text: &str) -> bool {
    text.graphemes(true).count() == 1
}

fn validate(raw: &RawDataset) -> Result<(), DatasetError> {
    if raw.entries.is_empty() {
        return Err(DatasetError::EmptyDataset);
    }

    let mut ids: HashSet<&str> = HashSet::with_capacity(raw.entries.len());
    for entry in &raw.entries {
        if !ids.insert(&entry.id) {
            return Err(DatasetError::DuplicateId {
                id: entry.id.clone(),
            });
        }
        if entry.name.trim().is_empty() {
            return Err(DatasetError::EmptyName {
                id: entry.id.clone(),
            });
        }
        if !is_single_grapheme(&entry.base) {
            return Err(DatasetError::InvalidGrapheme {
                id: entry.id.clone(),
                value: entry.base.clone(),
            });
        }

        let mut names: HashSet<String> = HashSet::with_capacity(entry.variants.len());
        for variant in &entry.variants {
            if variant.name.trim().is_empty() {
                return Err(DatasetError::EmptyVariantName {
                    id: entry.id.clone(),
                });
            }
            if !is_single_grapheme(&variant.grapheme) {
                return Err(DatasetError::InvalidGrapheme {
                    id: entry.id.clone(),
                    value: variant.grapheme.clone(),
                });
            }
            if !names.insert(variant.name.to_lowercase()) {
                return Err(DatasetError::DuplicateVariantName {
                    id: entry.id.clone(),
                    name: variant.name.clone(),
                });
            }
        }
    }

    Ok(())
}

fn canonical_checksum(raw: &RawDataset) -> Result<String, DatasetError> {
    let canonical = RawDataset {
        version: raw.version.clone(),
        unicode_version: raw.unicode_version.clone(),
        source: raw.source.clone(),
        license: raw.license.clone(),
        license_url: raw.license_url.clone(),
        checksum: String::new(),
        entries: raw.entries.clone(),
    };
    let bytes = serde_json::to_vec(&canonical)
        .map_err(|error| DatasetError::Serialize(error.to_string()))?;
    Ok(format!("{:016x}", fnv1a_64(&bytes)))
}

/// FNV-1a 64 over UTF-8 bytes.
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
