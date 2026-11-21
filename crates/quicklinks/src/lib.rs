//! Structured quicklinks with typed arguments, correct encoding and safe openers.
//!
//! This crate owns the model, validation and preview gate for user-defined
//! quicklinks. It performs no rendering, persistence, network access or process
//! launch: it produces inert [`OpenRequest`] data and availability results for a
//! host adapter to act on.
//!
//! Safety properties enforced here:
//!
//! * templates substitute values exactly once, so argument content such as
//!   `{other}` or `$(command)` is never interpreted a second time;
//! * only `http`/`https` URLs and explicitly enabled absolute local paths are
//!   accepted; `javascript:`, `data:`, `file:`, `vbscript:`, `shell:` and every
//!   unapproved scheme are rejected before an opener is considered;
//! * local paths have separate opt-ins for UNC/network paths and device
//!   namespace paths. [`Opener::DefaultApp`] only opens an allowlist of ordinary
//!   documents/media/archives; executables and unknown file types need explicit
//!   [`OpenPolicy::with_executables`] / [`OpenPolicy::with_unknown_file_types`]
//!   opt-ins, while [`Opener::FileManager`] may still reveal any path;
//! * invisible bidi/zero-width format controls and Unicode whitespace/soft
//!   hyphens are rejected in display-sensitive text and resolved targets;
//! * a preview resolves the exact target, describes the effect, probes opener
//!   availability and is the only thing [`Library::commit`] will save; stale
//!   previews fail with [`CommitError::Conflict`] and the fingerprint is checked
//!   again at commit;
//! * imports apply the same target policy to each item (with its defaults) and
//!   report per-item failures; merge mode never overwrites and never exceeds
//!   [`MAX_QUICKLINKS`].
//!
//! Identities are `quicklink:<32 hex>` and derive deterministically from a
//! user-provided stable slug, never from display text.

#![forbid(unsafe_code)]
#![deny(missing_debug_implementations)]

mod encoding;
mod id;
mod library;
mod model;
mod policy;
mod portable;
mod template;
mod text;

#[cfg(test)]
mod tests;

pub use encoding::{EncodeError, encode_component, is_unreserved};
pub use id::{ID_PREFIX, IdError, MAX_SLUG_CHARS, QuicklinkId};
pub use library::{CommitError, Library, LibraryError, MAX_QUICKLINKS};
pub use model::{
    Fingerprint, MAX_ALIAS_CHARS, MAX_TAG_CHARS, MAX_TAGS, MAX_TITLE_CHARS, ModelError, Preview,
    PreviewError, Quicklink,
};
pub use policy::{
    BLOCKED_SCHEMES, KNOWN_EXECUTABLE_EXTENSIONS, OpenPolicy, OpenRequest, Opener,
    OpenerAvailability, OpenerProbe, SAFE_DEFAULT_APP_EXTENSIONS, StaticOpenerProbe, TargetError,
    TargetKind, is_absolute_local_path, is_device_path, is_known_executable_extension,
    is_network_path, is_safe_default_app_extension, is_web_scheme, local_file_extension,
    resolve_target, scheme_of,
};
pub use portable::{
    ExportError, ImportItemError, ImportMode, ImportOptions, ImportReport, MAX_IMPORT_BYTES,
    MIN_SUPPORTED_SCHEMA_VERSION, SCHEMA_VERSION,
};
pub use template::{
    ArgKind, ArgSpec, ArgumentValue, Encoding, MAX_ARGUMENT_NAME_CHARS, MAX_ARGUMENTS,
    MAX_CHOICE_CHARS, MAX_CHOICES, MAX_RESOLVED_TARGET_BYTES, MAX_RESOLVED_TARGET_CHARS,
    MAX_TEMPLATE_CHARS, Part, ResolveError, Template, TemplateError, ValueError,
};
