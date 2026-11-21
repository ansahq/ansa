//! Scheme, path and opener policy for resolved quicklink targets.
//!
//! Targets are classified as either an `http`/`https` URL or an absolute local
//! path. Everything else is rejected before a host sees it. Local paths are
//! opt-in through [`OpenPolicy::with_local_paths`]; UNC/network paths, device
//! namespace paths and executable/script extensions have separate opt-ins and
//! are rejected by default. `file:` is always rejected as a URL scheme — local
//! files are opened as plain absolute paths, not URI schemes.
//!
//! This module produces data only: an [`OpenRequest`] plus an
//! [`OpenerAvailability`] probe. It never spawns a process, invokes a shell or
//! contacts the network.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::text::{is_unsafe_format_character, is_unsafe_target_whitespace};

/// Schemes that are always rejected, independent of policy.
pub const BLOCKED_SCHEMES: &[&str] = &["javascript", "data", "vbscript", "file", "shell"];

/// Extensions treated as executable, script, installer or active content.
///
/// This list is used only to choose a clearer error message. The security gate
/// is the [`SAFE_DEFAULT_APP_EXTENSIONS`] allowlist: every other extension is
/// denied for [`Opener::DefaultApp`] unless the policy opts in.
pub const KNOWN_EXECUTABLE_EXTENSIONS: &[&str] = &[
    "exe",
    "bat",
    "cmd",
    "com",
    "scr",
    "msi",
    "msp",
    "mst",
    "pif",
    "scf",
    "msc",
    "cpl",
    "dll",
    "lnk",
    "reg",
    "inf",
    "ins",
    "isp",
    "gadget",
    "application",
    "appref-ms",
    "hta",
    "mshta",
    "vbs",
    "vbe",
    "vb",
    "js",
    "jse",
    "wsf",
    "wsh",
    "ws",
    "wsc",
    "ps1",
    "psm1",
    "psd1",
    "py",
    "pyw",
    "pyc",
    "pyo",
    "jar",
    "pl",
    "pm",
    "rb",
    "sh",
    "bash",
    "zsh",
    "command",
    "tool",
    "workflow",
    "settingcontent-ms",
    "search-ms",
    "diagcab",
    "xll",
    "xla",
    "xlam",
    "xlsm",
    "docm",
    "dotm",
    "pptm",
    "potm",
    "ppsm",
    "sldm",
];

/// Extensions that are safe to hand to [`Opener::DefaultApp`] by default.
///
/// This is an allowlist of ordinary documents, images, audio, video and
/// archives. Macro-enabled office formats and every unknown extension are not
/// included; they require explicit [`OpenPolicy::with_unknown_file_types`] or
/// [`OpenPolicy::with_executables`]. Paths without an extension (folders) and
/// paths ending in a separator are treated as folders and allowed.
pub const SAFE_DEFAULT_APP_EXTENSIONS: &[&str] = &[
    // Documents and text/data.
    "txt", "md", "markdown", "rst", "pdf", "rtf", "doc", "docx", "odt", "xls", "xlsx", "ods", "csv",
    "tsv", "ppt", "pptx", "odp", "epub", "mobi", "azw3", "log", "json", "xml", "yaml", "yml",
    "toml", "ini", "cfg", "tex", "html", "htm", "xhtml", // Images.
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "tif", "tiff", "ico", "heic", "avif", "svg",
    // Audio.
    "mp3", "wav", "flac", "aac", "ogg", "m4a", "wma", "opus", // Video.
    "mp4", "mkv", "avi", "mov", "wmv", "webm", "m4v", "mpg", "mpeg", // Archives.
    "zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "xz", "zst",
];

/// The kind of target a quicklink resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    /// An `http` or `https` URL.
    WebUrl,
    /// An absolute filesystem path.
    LocalPath,
}

impl TargetKind {
    /// Stable label.
    pub fn label(self) -> &'static str {
        match self {
            Self::WebUrl => "web_url",
            Self::LocalPath => "local_path",
        }
    }
}

impl fmt::Display for TargetKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// Where a resolved target may be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Opener {
    /// The system web browser; only accepts [`TargetKind::WebUrl`].
    Browser,
    /// The system file manager; only accepts [`TargetKind::LocalPath`].
    FileManager,
    /// The host's registered handler for either target kind.
    DefaultApp,
}

impl Opener {
    /// Stable serialized identifier.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Browser => "browser",
            Self::FileManager => "file_manager",
            Self::DefaultApp => "default_app",
        }
    }

    /// User-facing name used in errors and previews.
    pub fn label(self) -> &'static str {
        match self {
            Self::Browser => "Browser",
            Self::FileManager => "File Manager",
            Self::DefaultApp => "Default Application",
        }
    }

    /// Whether this opener accepts the given target kind.
    pub fn supports(self, kind: TargetKind) -> bool {
        match self {
            Self::Browser => kind == TargetKind::WebUrl,
            Self::FileManager => kind == TargetKind::LocalPath,
            Self::DefaultApp => true,
        }
    }
}

impl fmt::Display for Opener {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// Policy controlling which target kinds a quicklink may resolve to.
///
/// The default is web-only: `http` and `https`. Each broader capability is a
/// separate explicit opt-in:
///
/// * [`OpenPolicy::with_local_paths`] — drive-absolute and POSIX-rooted paths;
/// * [`OpenPolicy::with_network_paths`] — UNC and `//host/share` paths;
/// * [`OpenPolicy::with_device_paths`] — `\\?\` and `\\.\` device namespace;
/// * [`OpenPolicy::with_executables`] — all local files through
///   [`Opener::DefaultApp`], including executables and unknown types;
/// * [`OpenPolicy::with_unknown_file_types`] — unknown, non-executable file
///   types through [`Opener::DefaultApp`], while known executables stay blocked.
///
/// Missing serde fields default to `false`, so older serialized policies stay
/// restrictive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenPolicy {
    /// Whether `http`/`https` URLs are allowed.
    pub allow_web: bool,
    /// Whether ordinary absolute local paths are allowed.
    pub allow_local_paths: bool,
    /// Whether UNC/network paths are allowed.
    #[serde(default)]
    pub allow_network_paths: bool,
    /// Whether Windows device-namespace paths are allowed.
    #[serde(default)]
    pub allow_device_paths: bool,
    /// Whether [`Opener::DefaultApp`] may open any local file, including
    /// executables and unknown types.
    #[serde(default)]
    pub allow_executables: bool,
    /// Whether [`Opener::DefaultApp`] may open unknown, non-safe file types.
    #[serde(default)]
    pub allow_unknown_file_types: bool,
}

impl OpenPolicy {
    /// The default policy: web URLs only.
    pub fn web_only() -> Self {
        Self {
            allow_web: true,
            allow_local_paths: false,
            allow_network_paths: false,
            allow_device_paths: false,
            allow_executables: false,
            allow_unknown_file_types: false,
        }
    }

    /// Allows web URLs and ordinary absolute local paths.
    pub fn with_local_paths() -> Self {
        Self {
            allow_web: true,
            allow_local_paths: true,
            allow_network_paths: false,
            allow_device_paths: false,
            allow_executables: false,
            allow_unknown_file_types: false,
        }
    }

    /// Additionally allows UNC/network paths.
    ///
    /// Call this on a policy that already allows local paths, for example
    /// `OpenPolicy::with_local_paths().with_network_paths()`.
    pub fn with_network_paths(mut self) -> Self {
        self.allow_network_paths = true;
        self
    }

    /// Additionally allows device-namespace paths.
    pub fn with_device_paths(mut self) -> Self {
        self.allow_device_paths = true;
        self
    }

    /// Additionally allows executables/scripts through [`Opener::DefaultApp`].
    ///
    /// This is the broad opt-in: it also allows unknown file types.
    pub fn with_executables(mut self) -> Self {
        self.allow_executables = true;
        self
    }

    /// Additionally allows unknown, non-executable file types through
    /// [`Opener::DefaultApp`].
    pub fn with_unknown_file_types(mut self) -> Self {
        self.allow_unknown_file_types = true;
        self
    }

    /// Whether local paths are enabled.
    pub fn allows_local_paths(self) -> bool {
        self.allow_local_paths
    }

    /// Whether web URLs are enabled.
    pub fn allows_web(self) -> bool {
        self.allow_web
    }

    /// Whether network paths are enabled.
    pub fn allows_network_paths(self) -> bool {
        self.allow_network_paths
    }

    /// Whether device-namespace paths are enabled.
    pub fn allows_device_paths(self) -> bool {
        self.allow_device_paths
    }

    /// Whether executables/scripts are enabled for the default application.
    pub fn allows_executables(self) -> bool {
        self.allow_executables
    }

    /// Whether unknown non-executable file types are enabled for the default
    /// application.
    pub fn allows_unknown_file_types(self) -> bool {
        self.allow_unknown_file_types
    }
}

impl Default for OpenPolicy {
    fn default() -> Self {
        Self::web_only()
    }
}

/// A resolved, policy-checked target and its chosen opener.
///
/// This is inert data handed to the host. Constructing it does not open
/// anything; the host is responsible for binding it to an actual adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenRequest {
    /// Exact target string; never shell-interpreted.
    pub target: String,
    /// Opener the user selected.
    pub opener: Opener,
}

/// Why a target was rejected by the scheme/path policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetError {
    Empty,
    SurroundingWhitespace,
    ControlCharacter { character: char, position: usize },
    UnsafeFormatCharacter { character: char, position: usize },
    UnsafeWhitespace { character: char, position: usize },
    BlockedScheme { scheme: String },
    UnapprovedScheme { scheme: String },
    WebDisallowed,
    LocalPathsDisallowed,
    NetworkPathsDisallowed,
    DevicePathsDisallowed,
    ExecutableDisallowed { extension: String },
    UnknownFileTypeDisallowed { extension: String },
    RelativeTarget,
    NotAbsolutePath,
    PathTraversal { position: usize },
    AmbiguousPathSegment { segment: String, position: usize },
    OpenerMismatch { opener: Opener, kind: TargetKind },
    InvalidUrl { reason: String },
    InvalidUrlCharacter { character: char, position: usize },
    InvalidPercentEscape { position: usize },
    EncodedControlCharacter { byte: u8, position: usize },
    UserInfoNotAllowed,
    InvalidHost { reason: String },
    InvalidPort { port: String },
}

impl fmt::Display for TargetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("the resolved target is empty"),
            Self::SurroundingWhitespace => formatter.write_str(
                "the resolved target starts or ends with whitespace; remove it or percent-encode it",
            ),
            Self::ControlCharacter {
                character,
                position,
            } => write!(
                formatter,
                "the resolved target contains control character {character:?} at byte {position}"
            ),
            Self::UnsafeFormatCharacter {
                character,
                position,
            } => write!(
                formatter,
                "the resolved target contains invisible format character U+{:04X} at byte {position}",
                *character as u32
            ),
            Self::UnsafeWhitespace {
                character,
                position,
            } => write!(
                formatter,
                "the resolved target contains Unicode whitespace/format character U+{:04X} at byte {position}",
                *character as u32
            ),
            Self::BlockedScheme { scheme } => write!(
                formatter,
                "the '{scheme}:' scheme is blocked; quicklinks may only open http(s) URLs or approved absolute local paths"
            ),
            Self::UnapprovedScheme { scheme } => write!(
                formatter,
                "the '{scheme}:' scheme is not allowed; only http and https URLs are permitted"
            ),
            Self::WebDisallowed => formatter.write_str(
                "this policy does not allow web URLs; enable web links or use a local path",
            ),
            Self::LocalPathsDisallowed => formatter.write_str(
                "this policy does not allow local paths; enable local paths in quicklink settings",
            ),
            Self::NetworkPathsDisallowed => formatter.write_str(
                "this policy does not allow UNC/network paths; enable network paths explicitly",
            ),
            Self::DevicePathsDisallowed => formatter.write_str(
                "this policy does not allow device namespace paths (\\\\?\\ or \\\\.\\\\)",
            ),
            Self::ExecutableDisallowed { extension } => write!(
                formatter,
                "the Default Application opener will not run a .{extension} file unless executables are explicitly allowed; reveal it in the file manager or allow executables"
            ),
            Self::UnknownFileTypeDisallowed { extension } => write!(
                formatter,
                "the Default Application opener does not open unknown .{extension} files by default; allow unknown file types, allow executables, or reveal it in the file manager"
            ),
            Self::RelativeTarget => formatter.write_str(
                "the target is neither an http(s) URL nor an absolute path; start URLs with https:// or use a full path",
            ),
            Self::NotAbsolutePath => formatter.write_str(
                "the file manager opener requires an absolute path such as C:\\folder, \\\\server\\share or /folder",
            ),
            Self::PathTraversal { position } => write!(
                formatter,
                "the local path contains a '..' segment at byte {position}; parent traversal is not allowed"
            ),
            Self::AmbiguousPathSegment { segment, position } => write!(
                formatter,
                "the local path contains the ambiguous dot/space segment {segment:?} at byte {position}"
            ),
            Self::OpenerMismatch { opener, kind } => write!(
                formatter,
                "the {opener} opener cannot open a {kind}; choose a compatible opener"
            ),
            Self::InvalidUrl { reason } => write!(formatter, "invalid URL: {reason}"),
            Self::InvalidUrlCharacter {
                character,
                position,
            } => write!(
                formatter,
                "URL contains unencoded character {character:?} at byte {position}; percent-encode it or use an argument with URL encoding"
            ),
            Self::InvalidPercentEscape { position } => write!(
                formatter,
                "URL contains an incomplete percent escape at byte {position}; use % followed by two hex digits"
            ),
            Self::EncodedControlCharacter { byte, position } => write!(
                formatter,
                "URL contains percent-encoded control byte {byte:#04X} at byte {position}"
            ),
            Self::UserInfoNotAllowed => formatter.write_str(
                "URLs with user information (user:password@host) are not allowed in quicklinks; credentials would be stored in plaintext",
            ),
            Self::InvalidHost { reason } => write!(formatter, "invalid URL host: {reason}"),
            Self::InvalidPort { port } => write!(
                formatter,
                "invalid URL port {port:?}; use a number between 1 and 65535"
            ),
        }
    }
}

impl std::error::Error for TargetError {}

/// Reports whether an opener is currently available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenerAvailability {
    Available,
    Unavailable { reason: String },
}

impl OpenerAvailability {
    /// True only when the opener is usable.
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }
}

/// Host-supplied, fresh availability probe.
///
/// Implementations must not claim an opener works when the host cannot route
/// it. A preview calls the probe at preview time and refuses to save an
/// unavailable opener with an actionable error.
pub trait OpenerProbe {
    /// Probes one opener. The result must reflect current host state.
    fn probe(&self, opener: Opener) -> OpenerAvailability;
}

/// A deterministic probe for tests and headless hosts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StaticOpenerProbe {
    unavailable: BTreeMap<Opener, String>,
}

impl StaticOpenerProbe {
    /// A probe that reports every opener as available.
    pub fn all_available() -> Self {
        Self::default()
    }

    /// Marks one opener unavailable with a user-facing reason.
    pub fn with_unavailable(mut self, opener: Opener, reason: impl Into<String>) -> Self {
        self.unavailable.insert(opener, reason.into());
        self
    }
}

impl OpenerProbe for StaticOpenerProbe {
    fn probe(&self, opener: Opener) -> OpenerAvailability {
        match self.unavailable.get(&opener) {
            Some(reason) => OpenerAvailability::Unavailable {
                reason: reason.clone(),
            },
            None => OpenerAvailability::Available,
        }
    }
}

/// True when the target begins with an absolute local-path form.
///
/// Accepted: Windows drive-absolute (`C:\...`, `C:/...`), UNC
/// (`\\server\share`, `//server/share`), device namespace (`\\?\...`,
/// `\\.\...`) and POSIX-rooted (`/...`). A drive-relative form such as
/// `C:folder` is *not* absolute.
pub fn is_absolute_local_path(target: &str) -> bool {
    local_path_form(target).is_some()
}

/// True for UNC and `//host/share` network paths, including `\\?\UNC\...`.
pub fn is_network_path(target: &str) -> bool {
    if is_device_path(target) {
        let rest = &target[4..];
        return rest.get(..4).is_some_and(|prefix| {
            prefix.eq_ignore_ascii_case("UNC\\") || prefix.eq_ignore_ascii_case("UNC/")
        });
    }
    target.starts_with("\\\\") || target.starts_with("//")
}

/// True for Windows device-namespace paths (`\\?\...`, `\\.\...`, and the
/// forward-slash spellings).
pub fn is_device_path(target: &str) -> bool {
    target.starts_with("\\\\?\\")
        || target.starts_with("\\\\.\\")
        || target.starts_with("//?/")
        || target.starts_with("//./")
}

/// Extracts a lowercase RFC 3986 scheme from the start of `target`.
pub fn scheme_of(target: &str) -> Option<String> {
    let mut characters = target.char_indices();
    let (_, first) = characters.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    for (_, character) in characters {
        if character == ':' {
            let end = target.find(':')?;
            return Some(target[..end].to_ascii_lowercase());
        }
        if !character.is_ascii_alphanumeric() && !matches!(character, '+' | '-' | '.') {
            return None;
        }
    }
    None
}

/// True for the only schemes quicklinks may open as URLs.
pub fn is_web_scheme(scheme: &str) -> bool {
    scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
}

/// Returns the lowercased extension of the final path segment.
///
/// Trailing spaces/dots and Windows alternate-data-stream suffixes are ignored
/// before matching, because Windows normalizes them away. `None` means the path
/// has no extension (treated as a folder) or ends in a separator.
pub fn local_file_extension(target: &str) -> Option<String> {
    let name = target.split(['/', '\\']).next_back()?;
    let name = name.split(':').next().unwrap_or(name);
    let name = name.trim_end_matches([' ', '.']);
    let (_, extension) = name.rsplit_once('.')?;
    let extension = extension.to_ascii_lowercase();
    (!extension.is_empty()).then_some(extension)
}

/// True when the extension is safe for [`Opener::DefaultApp`] by default.
pub fn is_safe_default_app_extension(extension: &str) -> bool {
    SAFE_DEFAULT_APP_EXTENSIONS
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(extension))
}

/// True when the extension is a known executable, script, installer or active
/// content type. Unknown extensions are also denied by default.
pub fn is_known_executable_extension(extension: &str) -> bool {
    KNOWN_EXECUTABLE_EXTENSIONS
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(extension))
}

fn validate_default_app_file(target: &str, policy: OpenPolicy) -> Result<(), TargetError> {
    let Some(extension) = local_file_extension(target) else {
        // No extension or a trailing separator: treat as a folder/reveal.
        // This crate is host-neutral and does not stat the filesystem, so the
        // host should verify directory status before opening.
        return Ok(());
    };
    if is_safe_default_app_extension(&extension) {
        return Ok(());
    }
    if policy.allow_executables {
        return Ok(());
    }
    if is_known_executable_extension(&extension) {
        return Err(TargetError::ExecutableDisallowed { extension });
    }
    if policy.allow_unknown_file_types {
        return Ok(());
    }
    Err(TargetError::UnknownFileTypeDisallowed { extension })
}

/// Classifies and validates a target against the opener and policy.
///
/// Order of checks: target syntax, target kind, opener compatibility, path-kind
/// opt-ins, then executable policy. Scheme blocklists are enforced before
/// policy, so a permissive local policy never makes `file:`, `javascript:` or
/// `data:` openable.
pub fn resolve_target(
    target: &str,
    opener: Opener,
    policy: OpenPolicy,
) -> Result<TargetKind, TargetError> {
    if target.is_empty() {
        return Err(TargetError::Empty);
    }
    if target.trim() != target {
        return Err(TargetError::SurroundingWhitespace);
    }
    for (position, character) in target.char_indices() {
        if character == '\0' || character.is_control() {
            return Err(TargetError::ControlCharacter {
                character,
                position,
            });
        }
        if is_unsafe_format_character(character) {
            return Err(TargetError::UnsafeFormatCharacter {
                character,
                position,
            });
        }
        if is_unsafe_target_whitespace(character) {
            return Err(TargetError::UnsafeWhitespace {
                character,
                position,
            });
        }
    }

    if is_absolute_local_path(target) {
        validate_local_path(target)?;
        if !opener.supports(TargetKind::LocalPath) {
            return Err(TargetError::OpenerMismatch {
                opener,
                kind: TargetKind::LocalPath,
            });
        }
        if is_device_path(target) && !policy.allow_device_paths {
            return Err(TargetError::DevicePathsDisallowed);
        }
        if is_network_path(target) && !policy.allow_network_paths {
            return Err(TargetError::NetworkPathsDisallowed);
        }
        if !policy.allow_local_paths {
            return Err(TargetError::LocalPathsDisallowed);
        }
        if opener == Opener::DefaultApp {
            validate_default_app_file(target, policy)?;
        }
        return Ok(TargetKind::LocalPath);
    }

    if is_drive_relative(target) {
        return Err(TargetError::NotAbsolutePath);
    }

    if let Some(scheme) = scheme_of(target) {
        if BLOCKED_SCHEMES
            .iter()
            .any(|blocked| scheme.eq_ignore_ascii_case(blocked))
        {
            return Err(TargetError::BlockedScheme { scheme });
        }
        if !is_web_scheme(&scheme) {
            return Err(TargetError::UnapprovedScheme { scheme });
        }
        if !opener.supports(TargetKind::WebUrl) {
            return Err(TargetError::OpenerMismatch {
                opener,
                kind: TargetKind::WebUrl,
            });
        }
        if !policy.allow_web {
            return Err(TargetError::WebDisallowed);
        }
        validate_web_url(&scheme, target)?;
        return Ok(TargetKind::WebUrl);
    }

    if opener == Opener::FileManager || policy.allow_local_paths {
        Err(TargetError::NotAbsolutePath)
    } else {
        Err(TargetError::RelativeTarget)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalPathForm {
    Drive,
    Rooted,
    Network,
    Device,
}

fn local_path_form(target: &str) -> Option<LocalPathForm> {
    if is_device_path(target) {
        return Some(LocalPathForm::Device);
    }
    if target.starts_with("\\\\") || target.starts_with("//") {
        return Some(LocalPathForm::Network);
    }
    let bytes = target.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
    {
        return Some(LocalPathForm::Drive);
    }
    if target.starts_with('/') {
        return Some(LocalPathForm::Rooted);
    }
    None
}

fn is_drive_relative(target: &str) -> bool {
    let bytes = target.as_bytes();
    bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || (bytes[2] != b'\\' && bytes[2] != b'/'))
}

fn validate_local_path(target: &str) -> Result<(), TargetError> {
    let path = path_after_namespace(target);
    let mut offset = target.len().saturating_sub(path.len());
    for segment in path.split(['/', '\\']) {
        if segment == ".." {
            return Err(TargetError::PathTraversal { position: offset });
        }
        if !segment.is_empty()
            && segment
                .chars()
                .all(|character| matches!(character, '.' | ' '))
        {
            return Err(TargetError::AmbiguousPathSegment {
                segment: segment.to_owned(),
                position: offset,
            });
        }
        offset += segment.len() + 1;
    }
    Ok(())
}

fn path_after_namespace(target: &str) -> &str {
    for prefix in ["\\\\?\\", "\\\\.\\", "//?/", "//./"] {
        if let Some(stripped) = target.strip_prefix(prefix) {
            return stripped;
        }
    }
    target
}

fn validate_web_url(scheme: &str, target: &str) -> Result<(), TargetError> {
    let rest = &target[scheme.len() + 1..];
    let Some(after_scheme) = rest.strip_prefix("//") else {
        return Err(TargetError::InvalidUrl {
            reason: format!("'{scheme}:' URLs must be followed by '//' and a host"),
        });
    };
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    if authority.is_empty() {
        return Err(TargetError::InvalidUrl {
            reason: "the URL has no host".to_owned(),
        });
    }
    if authority.contains('@') {
        return Err(TargetError::UserInfoNotAllowed);
    }
    validate_authority(authority)?;
    validate_url_characters(scheme, target)
}

fn validate_authority(authority: &str) -> Result<(), TargetError> {
    let (host, port, bracketed) = if let Some(rest) = authority.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            return Err(TargetError::InvalidHost {
                reason: "missing ']' in bracketed IPv6 host".to_owned(),
            });
        };
        let host = &rest[..end];
        let suffix = &rest[end + 1..];
        let port = match suffix {
            "" => None,
            other => match other.strip_prefix(':') {
                Some(port) => Some(port),
                None => {
                    return Err(TargetError::InvalidHost {
                        reason: "unexpected text after a bracketed IPv6 host".to_owned(),
                    });
                }
            },
        };
        validate_ipv6_host(host)?;
        (host, port, true)
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        if host.contains(':') {
            return Err(TargetError::InvalidHost {
                reason: "IPv6 hosts must be enclosed in '[' and ']'".to_owned(),
            });
        }
        (host, Some(port), false)
    } else {
        (authority, None, false)
    };

    if !bracketed {
        validate_host(host)?;
    }
    if let Some(port) = port {
        validate_port(port)?;
    }
    Ok(())
}

fn validate_host(host: &str) -> Result<(), TargetError> {
    if host.is_empty() {
        return Err(TargetError::InvalidHost {
            reason: "host is empty".to_owned(),
        });
    }
    if host.len() > 253 {
        return Err(TargetError::InvalidHost {
            reason: "host is longer than 253 bytes".to_owned(),
        });
    }
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty() || host.chars().all(|character| character == '.') {
        return Err(TargetError::InvalidHost {
            reason: "host must contain a name, not only dots".to_owned(),
        });
    }
    for label in host.split('.') {
        if label.is_empty() {
            return Err(TargetError::InvalidHost {
                reason: "host contains an empty label".to_owned(),
            });
        }
        if label.len() > 63 {
            return Err(TargetError::InvalidHost {
                reason: "host label is longer than 63 bytes".to_owned(),
            });
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(TargetError::InvalidHost {
                reason: "host label must not start or end with '-'".to_owned(),
            });
        }
        for character in label.chars() {
            if character.is_ascii() {
                if !character.is_ascii_alphanumeric() && !matches!(character, '-' | '_') {
                    return Err(TargetError::InvalidHost {
                        reason: format!("host contains invalid character {character:?}"),
                    });
                }
            } else if is_unsafe_format_character(character) {
                return Err(TargetError::UnsafeFormatCharacter {
                    character,
                    position: 0,
                });
            }
        }
    }
    Ok(())
}

fn validate_ipv6_host(host: &str) -> Result<(), TargetError> {
    if host.is_empty() {
        return Err(TargetError::InvalidHost {
            reason: "bracketed IPv6 host is empty".to_owned(),
        });
    }
    if host.contains(":::") || host.matches("::").count() > 1 {
        return Err(TargetError::InvalidHost {
            reason: "bracketed IPv6 host has malformed '::' compression".to_owned(),
        });
    }
    if host.starts_with(':') && !host.starts_with("::") {
        return Err(TargetError::InvalidHost {
            reason: "bracketed IPv6 host has a single leading ':'".to_owned(),
        });
    }
    if host.ends_with(':') && !host.ends_with("::") {
        return Err(TargetError::InvalidHost {
            reason: "bracketed IPv6 host has a single trailing ':'".to_owned(),
        });
    }

    let compressed = host.contains("::");
    let mut groups = 0_usize;
    for group in host.split(':') {
        if group.is_empty() {
            continue;
        }
        if group.contains('.') {
            validate_ipv4_tail(group)?;
            groups += 2;
            continue;
        }
        if group.len() > 4 || !group.chars().all(|character| character.is_ascii_hexdigit()) {
            return Err(TargetError::InvalidHost {
                reason: "bracketed host contains an invalid IPv6 group".to_owned(),
            });
        }
        groups += 1;
    }
    if compressed {
        if groups >= 8 {
            return Err(TargetError::InvalidHost {
                reason: "compressed IPv6 host has too many groups".to_owned(),
            });
        }
    } else if groups != 8 {
        return Err(TargetError::InvalidHost {
            reason: "IPv6 host must contain exactly eight groups".to_owned(),
        });
    }
    Ok(())
}

fn validate_ipv4_tail(host: &str) -> Result<(), TargetError> {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() != 4 {
        return Err(TargetError::InvalidHost {
            reason: "embedded IPv4 tail must have four octets".to_owned(),
        });
    }
    for part in parts {
        if part.is_empty()
            || !part.bytes().all(|byte| byte.is_ascii_digit())
            || part.parse::<u16>().map_or(true, |value| value > 255)
        {
            return Err(TargetError::InvalidHost {
                reason: "embedded IPv4 tail contains an invalid octet".to_owned(),
            });
        }
    }
    Ok(())
}

fn validate_port(port: &str) -> Result<(), TargetError> {
    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(TargetError::InvalidPort {
            port: port.to_owned(),
        });
    }
    match port.parse::<u32>() {
        Ok(value) if (1..=65_535).contains(&value) => Ok(()),
        _ => Err(TargetError::InvalidPort {
            port: port.to_owned(),
        }),
    }
}

fn validate_url_characters(scheme: &str, target: &str) -> Result<(), TargetError> {
    let bytes = target.as_bytes();
    let mut index = scheme.len() + 1;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%' {
            let valid = index + 2 < bytes.len()
                && bytes[index + 1].is_ascii_hexdigit()
                && bytes[index + 2].is_ascii_hexdigit();
            if !valid {
                return Err(TargetError::InvalidPercentEscape { position: index });
            }
            let high = hex_value(bytes[index + 1]);
            let low = hex_value(bytes[index + 2]);
            let decoded = high * 16 + low;
            if decoded < 0x20 || decoded == 0x7f {
                return Err(TargetError::EncodedControlCharacter {
                    byte: decoded,
                    position: index,
                });
            }
            index += 3;
            continue;
        }
        if byte < 0x80 {
            if is_url_byte(byte) {
                index += 1;
                continue;
            }
            return Err(TargetError::InvalidUrlCharacter {
                character: char::from(byte),
                position: index,
            });
        }
        let character = target[index..]
            .chars()
            .next()
            .unwrap_or(char::REPLACEMENT_CHARACTER);
        if character.is_control() {
            return Err(TargetError::ControlCharacter {
                character,
                position: index,
            });
        }
        if is_unsafe_format_character(character) {
            return Err(TargetError::UnsafeFormatCharacter {
                character,
                position: index,
            });
        }
        index += character.len_utf8();
    }
    Ok(())
}

fn hex_value(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => 0,
    }
}

fn is_url_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'.'
                | b'_'
                | b'~'
                | b':'
                | b'/'
                | b'?'
                | b'#'
                | b'['
                | b']'
                | b'@'
                | b'!'
                | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
        )
}
