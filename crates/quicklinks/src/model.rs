//! Quicklink model and the preview/commit gate.
//!
//! A [`Quicklink`] is inert data: a stable identity, presentation metadata, a
//! structured [`Template`] and a preferred [`Opener`]. Opening is always gated
//! by [`Preview`]: the host resolves arguments and probes the opener, shows the
//! exact target and effect, and only then asks the library to commit. A stale
//! preview is rejected, so the saved record can never differ from the target
//! the user saw.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::id::{QuicklinkId, fnv1a_128_hex};
use crate::policy::{
    OpenPolicy, OpenRequest, Opener, OpenerAvailability, OpenerProbe, TargetError, TargetKind,
    is_absolute_local_path, is_web_scheme, resolve_target, scheme_of,
};
use crate::template::{ArgSpec, ResolveError, Template};
use crate::text::find_unsafe_format;

/// Maximum title length in characters.
pub const MAX_TITLE_CHARS: usize = 200;
/// Maximum alias length in characters (aligned with settings alias prefixes).
pub const MAX_ALIAS_CHARS: usize = 32;
/// Maximum number of tags on one quicklink.
pub const MAX_TAGS: usize = 32;
/// Maximum tag length in characters.
pub const MAX_TAG_CHARS: usize = 64;

/// A saved quicklink.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quicklink {
    id: QuicklinkId,
    title: String,
    alias: Option<String>,
    tags: Vec<String>,
    template: Template,
    opener: Opener,
    enabled: bool,
}

impl Quicklink {
    /// Creates an enabled browser quicklink with no alias or tags.
    pub fn new(id: QuicklinkId, title: impl Into<String>, template: Template) -> Self {
        Self {
            id,
            title: title.into().trim().to_owned(),
            alias: None,
            tags: Vec::new(),
            template,
            opener: Opener::Browser,
            enabled: true,
        }
    }

    /// Sets or clears the alias; surrounding whitespace is trimmed and an
    /// empty alias becomes `None`.
    pub fn with_alias(mut self, alias: impl Into<String>) -> Self {
        let alias = alias.into();
        let alias = alias.trim();
        self.alias = if alias.is_empty() {
            None
        } else {
            Some(alias.to_owned())
        };
        self
    }

    /// Clears the alias.
    pub fn without_alias(mut self) -> Self {
        self.alias = None;
        self
    }

    /// Replaces the tags after trimming, dropping empties and deduplicating
    /// case-insensitively. Tags are sorted for deterministic export.
    pub fn with_tags<I, S>(mut self, tags: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.tags = normalize_tags(tags.into_iter().map(Into::into));
        self
    }

    /// Selects the preferred opener.
    pub fn with_opener(mut self, opener: Opener) -> Self {
        self.opener = opener;
        self
    }

    /// Enables or disables the quicklink.
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Stable identity.
    pub fn id(&self) -> &QuicklinkId {
        &self.id
    }

    /// Display title; never used to derive the identity.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Optional exact launcher alias.
    pub fn alias(&self) -> Option<&str> {
        self.alias.as_deref()
    }

    /// Sorted, unique tags.
    pub fn tags(&self) -> &[String] {
        &self.tags
    }

    /// Structured template and argument schema.
    pub fn template(&self) -> &Template {
        &self.template
    }

    /// Declared arguments for this quicklink's template.
    pub fn arguments(&self) -> &[ArgSpec] {
        self.template.arguments()
    }

    /// Preferred opener.
    pub fn opener(&self) -> Opener {
        self.opener
    }

    /// Whether the quicklink is enabled for search and opening.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Returns a canonical copy with trimmed title/alias and sorted tags.
    ///
    /// Import and preview both canonicalise first, so exported data is stable
    /// and a committed record matches the previewed candidate byte for byte.
    pub fn normalized(&self) -> Self {
        let mut normalized = self.clone();
        normalized.title = self.title.trim().to_owned();
        normalized.alias = self
            .alias
            .as_deref()
            .map(str::trim)
            .filter(|alias| !alias.is_empty())
            .map(str::to_owned);
        normalized.tags = normalize_tags(self.tags.iter().cloned());
        normalized
    }

    /// Validates model invariants that serde cannot express.
    pub fn validate(&self) -> Result<(), ModelError> {
        if self.title.trim().is_empty() {
            return Err(ModelError::EmptyTitle);
        }
        if self.title.chars().count() > MAX_TITLE_CHARS {
            return Err(ModelError::TitleTooLong {
                limit: MAX_TITLE_CHARS,
            });
        }
        if self.title.chars().any(char::is_control) {
            return Err(ModelError::ControlInTitle);
        }
        if let Some((position, character)) = find_unsafe_format(&self.title) {
            return Err(ModelError::UnsafeFormatInTitle {
                character,
                position,
            });
        }
        if let Some(alias) = &self.alias {
            let alias = alias.trim();
            if alias.is_empty() {
                return Err(ModelError::EmptyAlias);
            }
            if alias.chars().count() > MAX_ALIAS_CHARS {
                return Err(ModelError::AliasTooLong {
                    limit: MAX_ALIAS_CHARS,
                });
            }
            if alias.chars().any(char::is_control) {
                return Err(ModelError::ControlInAlias);
            }
            if let Some((position, character)) = find_unsafe_format(alias) {
                return Err(ModelError::UnsafeFormatInAlias {
                    character,
                    position,
                });
            }
            let mut characters = alias.char_indices();
            match characters.next() {
                Some((_, first)) if first.is_ascii_alphanumeric() => {}
                _ => return Err(ModelError::AliasMustStartAlphanumeric),
            }
            for (position, character) in characters {
                if !character.is_ascii_alphanumeric() && !matches!(character, '.' | '_' | '-') {
                    return Err(ModelError::InvalidAliasCharacter {
                        character,
                        position,
                    });
                }
            }
        }
        if self.tags.len() > MAX_TAGS {
            return Err(ModelError::TooManyTags { limit: MAX_TAGS });
        }
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for tag in &self.tags {
            if tag.trim().is_empty() {
                return Err(ModelError::EmptyTag);
            }
            if tag.chars().count() > MAX_TAG_CHARS {
                return Err(ModelError::TagTooLong {
                    limit: MAX_TAG_CHARS,
                });
            }
            if tag.chars().any(char::is_control) {
                return Err(ModelError::ControlInTag);
            }
            if let Some((position, character)) = find_unsafe_format(tag) {
                return Err(ModelError::UnsafeFormatInTag {
                    character,
                    position,
                });
            }
            if !seen.insert(tag.to_lowercase()) {
                return Err(ModelError::DuplicateTag { tag: tag.clone() });
            }
        }
        validate_literal_prefix(&self.template.static_prefix())?;
        Ok(())
    }
}

fn normalize_tags<I>(tags: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut normalized: Vec<String> = Vec::new();
    for tag in tags {
        let tag = tag.trim();
        if tag.is_empty() {
            continue;
        }
        let key = tag.to_lowercase();
        if seen.insert(key) {
            normalized.push(tag.to_owned());
        }
    }
    normalized.sort_by(|left, right| {
        left.to_lowercase()
            .cmp(&right.to_lowercase())
            .then_with(|| left.cmp(right))
    });
    normalized
}

fn validate_literal_prefix(prefix: &str) -> Result<(), ModelError> {
    let prefix = prefix.trim_start();
    if prefix.is_empty() || is_absolute_local_path(prefix) {
        return Ok(());
    }
    if let Some(scheme) = scheme_of(prefix)
        && !is_web_scheme(&scheme)
    {
        return Err(ModelError::UnsafeLiteralScheme { scheme });
    }
    Ok(())
}

/// Why a quicklink record itself is invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    EmptyTitle,
    TitleTooLong { limit: usize },
    ControlInTitle,
    UnsafeFormatInTitle { character: char, position: usize },
    EmptyAlias,
    AliasTooLong { limit: usize },
    ControlInAlias,
    UnsafeFormatInAlias { character: char, position: usize },
    AliasMustStartAlphanumeric,
    InvalidAliasCharacter { character: char, position: usize },
    TooManyTags { limit: usize },
    EmptyTag,
    TagTooLong { limit: usize },
    ControlInTag,
    UnsafeFormatInTag { character: char, position: usize },
    DuplicateTag { tag: String },
    UnsafeLiteralScheme { scheme: String },
}

impl fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTitle => formatter.write_str("quicklink title must not be empty"),
            Self::TitleTooLong { limit } => {
                write!(
                    formatter,
                    "quicklink title is longer than {limit} characters"
                )
            }
            Self::ControlInTitle => {
                formatter.write_str("quicklink title contains a control character")
            }
            Self::UnsafeFormatInTitle {
                character,
                position,
            } => write!(
                formatter,
                "quicklink title contains invisible format character U+{:04X} at byte {position}",
                *character as u32
            ),
            Self::EmptyAlias => formatter.write_str("quicklink alias must not be empty"),
            Self::AliasTooLong { limit } => {
                write!(
                    formatter,
                    "quicklink alias is longer than {limit} characters"
                )
            }
            Self::ControlInAlias => {
                formatter.write_str("quicklink alias contains a control character")
            }
            Self::UnsafeFormatInAlias {
                character,
                position,
            } => write!(
                formatter,
                "quicklink alias contains invisible format character U+{:04X} at byte {position}",
                *character as u32
            ),
            Self::AliasMustStartAlphanumeric => {
                formatter.write_str("quicklink alias must start with an ASCII letter or digit")
            }
            Self::InvalidAliasCharacter {
                character,
                position,
            } => write!(
                formatter,
                "quicklink alias contains {character:?} at byte {position}; use A-Z, a-z, 0-9, '.', '_' or '-'"
            ),
            Self::TooManyTags { limit } => {
                write!(formatter, "quicklink has more than {limit} tags")
            }
            Self::EmptyTag => formatter.write_str("quicklink tag must not be empty"),
            Self::TagTooLong { limit } => {
                write!(formatter, "quicklink tag is longer than {limit} characters")
            }
            Self::ControlInTag => formatter.write_str("quicklink tag contains a control character"),
            Self::UnsafeFormatInTag {
                character,
                position,
            } => write!(
                formatter,
                "quicklink tag contains invisible format character U+{:04X} at byte {position}",
                *character as u32
            ),
            Self::DuplicateTag { tag } => {
                write!(formatter, "quicklink repeats the tag {tag:?}")
            }
            Self::UnsafeLiteralScheme { scheme } => write!(
                formatter,
                "template starts with the disallowed '{scheme}:' scheme; only http(s) URLs or absolute paths are allowed"
            ),
        }
    }
}

impl std::error::Error for ModelError {}

/// A deterministic fingerprint of one previewed proposal.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Fingerprint(String);

impl Fingerprint {
    /// Lowercase hex digest.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A resolved, probed proposal awaiting commit.
///
/// Fields are private: the only constructor is [`Preview::build`] (called by
/// `Library::preview`). `Library::commit` therefore knows the stored record is
/// exactly the one that was previewed.
#[derive(Debug, Clone)]
pub struct Preview {
    candidate: Quicklink,
    request: OpenRequest,
    kind: TargetKind,
    effect: String,
    fingerprint: Fingerprint,
    revision: u64,
}

impl Preview {
    pub(crate) fn build(
        candidate: Quicklink,
        values: &std::collections::BTreeMap<String, String>,
        policy: OpenPolicy,
        probe: &dyn OpenerProbe,
        revision: u64,
    ) -> Result<Self, PreviewError> {
        let candidate = candidate.normalized();
        candidate.validate()?;
        let target = candidate.template().resolve_text(values)?;
        let kind = resolve_target(&target, candidate.opener(), policy)?;
        match probe.probe(candidate.opener()) {
            OpenerAvailability::Available => {}
            OpenerAvailability::Unavailable { reason } => {
                return Err(PreviewError::OpenerUnavailable {
                    opener: candidate.opener(),
                    reason,
                });
            }
        }
        let opener = candidate.opener();
        let effect = describe_effect(opener, kind, &target, candidate.enabled());
        let fingerprint = fingerprint_of(&candidate, &target, revision)?;
        Ok(Self {
            candidate,
            request: OpenRequest { target, opener },
            kind,
            effect,
            fingerprint,
            revision,
        })
    }

    /// The exact record that commit will store.
    pub fn candidate(&self) -> &Quicklink {
        &self.candidate
    }

    /// Inert request data for the host.
    pub fn request(&self) -> &OpenRequest {
        &self.request
    }

    /// Exact resolved target.
    pub fn target(&self) -> &str {
        &self.request.target
    }

    /// Selected opener.
    pub fn opener(&self) -> Opener {
        self.request.opener
    }

    /// Target classification.
    pub fn target_kind(&self) -> TargetKind {
        self.kind
    }

    /// Human-readable description of what opening will do.
    pub fn effect(&self) -> &str {
        &self.effect
    }

    /// Deterministic proposal fingerprint.
    pub fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }

    /// Library revision this preview was built against.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Recomputes the fingerprint and reports whether it still matches.
    ///
    /// This is a consistency check for commit and for hosts that retain a
    /// preview: a mismatch means the previewed data and its digest diverged.
    pub fn verify_fingerprint(&self) -> bool {
        fingerprint_of(&self.candidate, &self.request.target, self.revision)
            .is_ok_and(|computed| computed == self.fingerprint)
    }

    /// Test-only corruption hook for the otherwise unreachable mismatch branch.
    #[cfg(test)]
    pub(crate) fn corrupt_fingerprint_for_test(&mut self) {
        self.fingerprint = Fingerprint("0".repeat(32));
    }
}

fn describe_effect(opener: Opener, kind: TargetKind, target: &str, enabled: bool) -> String {
    let action = match (opener, kind) {
        (Opener::Browser, _) => "Open in the system browser",
        (Opener::FileManager, _) => "Reveal in the file manager",
        (Opener::DefaultApp, TargetKind::WebUrl) => "Open in the default application",
        (Opener::DefaultApp, TargetKind::LocalPath) => "Open with the default application",
    };
    if enabled {
        format!("{action}: {target}")
    } else {
        format!(
            "Disabled. When enabled, {}: {target}",
            action.to_lowercase()
        )
    }
}

fn fingerprint_of(
    candidate: &Quicklink,
    target: &str,
    revision: u64,
) -> Result<Fingerprint, PreviewError> {
    let canonical =
        serde_json::to_string(candidate).map_err(|error| PreviewError::Fingerprint {
            message: error.to_string(),
        })?;
    let mut material = canonical.into_bytes();
    material.push(0);
    material.extend_from_slice(target.as_bytes());
    material.push(0);
    material.extend_from_slice(candidate.opener().slug().as_bytes());
    material.push(0);
    material.extend_from_slice(&revision.to_le_bytes());
    Ok(Fingerprint(fnv1a_128_hex(&material)))
}

/// Why a preview could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewError {
    Model(ModelError),
    Resolve(ResolveError),
    Target(TargetError),
    AliasTaken {
        alias: String,
        existing: QuicklinkId,
    },
    OpenerUnavailable {
        opener: Opener,
        reason: String,
    },
    LimitReached {
        limit: usize,
    },
    Fingerprint {
        message: String,
    },
}

impl fmt::Display for PreviewError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Model(error) => error.fmt(formatter),
            Self::Resolve(error) => error.fmt(formatter),
            Self::Target(error) => error.fmt(formatter),
            Self::AliasTaken { alias, existing } => write!(
                formatter,
                "alias {alias:?} is already used by {existing}; choose another alias"
            ),
            Self::OpenerUnavailable { opener, reason } => write!(
                formatter,
                "the {opener} opener is unavailable: {reason}; register a handler for it or choose another opener"
            ),
            Self::LimitReached { limit } => {
                write!(
                    formatter,
                    "the library is full; remove a quicklink before adding more than {limit}"
                )
            }
            Self::Fingerprint { message } => {
                write!(formatter, "could not fingerprint the preview: {message}")
            }
        }
    }
}

impl std::error::Error for PreviewError {}

impl From<ModelError> for PreviewError {
    fn from(error: ModelError) -> Self {
        Self::Model(error)
    }
}

impl From<ResolveError> for PreviewError {
    fn from(error: ResolveError) -> Self {
        Self::Resolve(error)
    }
}

impl From<TargetError> for PreviewError {
    fn from(error: TargetError) -> Self {
        Self::Target(error)
    }
}
