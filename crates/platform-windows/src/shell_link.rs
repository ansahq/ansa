//! Bounded, memory-safe parser for the Windows Shell Link (`.lnk`) format.
//!
//! Only the parts needed to resolve a local launch target are decoded:
//!
//! * the fixed 76-byte `ShellLinkHeader` (validated header size and CLSID);
//! * `LinkInfo` when present, preferring the Unicode local base path and
//!   appending the common path suffix;
//! * `StringData` for the working directory, command-line arguments and icon
//!   location.
//!
//! `LinkTargetIDList` payloads are skipped rather than interpreted. Shortcuts
//! that only carry an ID list (for example some packaged-app or shell-folder
//! links) therefore resolve to no local path; the shell still handles them when
//! the `.lnk` itself is launched.
//!
//! The parser never trusts a length or offset from the file before checking it
//! against the remaining bytes. Truncated or inconsistent input returns
//! [`ShellLinkError`] instead of panicking, and no allocation is sized directly
//! from an unvalidated header field. Input files are capped at
//! [`MAX_SHELL_LINK_BYTES`].
//!
//! References: MS-SHLLINK `ShellLinkHeader`, `LinkInfo` and `StringData`.

use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

pub(crate) const MAX_SHELL_LINK_BYTES: u64 = 4 * 1024 * 1024;

const HEADER_SIZE: usize = 0x4C;
const LINK_INFO_MIN_HEADER: usize = 0x1C;
const LINK_INFO_UNICODE_HEADER: usize = 0x24;

const FLAG_HAS_LINK_TARGET_ID_LIST: u32 = 0x0000_0001;
const FLAG_HAS_LINK_INFO: u32 = 0x0000_0002;
const FLAG_HAS_NAME: u32 = 0x0000_0004;
const FLAG_HAS_RELATIVE_PATH: u32 = 0x0000_0008;
const FLAG_HAS_WORKING_DIR: u32 = 0x0000_0010;
const FLAG_HAS_ARGUMENTS: u32 = 0x0000_0020;
const FLAG_HAS_ICON_LOCATION: u32 = 0x0000_0040;
const FLAG_IS_UNICODE: u32 = 0x0000_0080;

const LINK_INFO_VOLUME_ID_AND_LOCAL_BASE_PATH: u32 = 0x0000_0001;

/// The `00021401-0000-0000-C000-000000000046` CLSID in on-disk byte order.
const LINK_CLSID: [u8; 16] = [
    0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
];

/// Why a `.lnk` file could not be decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShellLinkError {
    message: String,
}

impl ShellLinkError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ShellLinkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ShellLinkError {}

/// Local launch metadata recovered from a `.lnk` file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ShellLinkInfo {
    /// Absolute local target path stored in `LinkInfo`, when present.
    pub(crate) target_path: Option<PathBuf>,
    /// Relative path stored in `StringData`, when present. It is resolved
    /// against the link's working directory and parent directory by
    /// [`resolve_shell_link_file`].
    pub(crate) relative_path: Option<String>,
    /// Local path reconstructed from a file-system `LinkTargetIDList`, when the
    /// item list uses the recognized drive/folder/file forms.
    pub(crate) id_list_path: Option<PathBuf>,
    /// Working directory requested by the shortcut, when present.
    pub(crate) working_directory: Option<PathBuf>,
    /// Command-line arguments stored as a single string, never re-parsed here.
    pub(crate) arguments: Option<String>,
    /// Raw icon location string such as `C:\App\app.exe,0`.
    pub(crate) icon_location: Option<String>,
}

/// A parsed shortcut plus a deterministic target-resolution attempt list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ResolvedShellLink {
    /// The parsed metadata.
    pub(crate) info: ShellLinkInfo,
    /// The first existing local candidate, or the `LinkInfo` target when one
    /// was stored (the caller checks its existence).
    pub(crate) target_path: Option<PathBuf>,
    /// Deterministic candidates that were considered, in order. Empty when the
    /// shortcut only carries a `LinkTargetIDList` and has no local path data.
    pub(crate) attempted: Vec<PathBuf>,
}

/// Reads and parses a `.lnk` file with a hard byte cap.
pub(crate) fn parse_shell_link_file(path: &Path) -> Result<ShellLinkInfo, ShellLinkError> {
    let metadata = fs::metadata(path)
        .map_err(|error| ShellLinkError::new(format!("cannot read shortcut metadata: {error}")))?;
    if !metadata.is_file() {
        return Err(ShellLinkError::new("shortcut is not a regular file"));
    }
    if metadata.len() > MAX_SHELL_LINK_BYTES {
        return Err(ShellLinkError::new(format!(
            "shortcut is larger than the {MAX_SHELL_LINK_BYTES}-byte limit"
        )));
    }

    let bytes = fs::read(path).map_err(|error| match error.kind() {
        ErrorKind::PermissionDenied => {
            ShellLinkError::new("permission denied while reading the shortcut")
        }
        _ => ShellLinkError::new(format!("cannot read shortcut: {error}")),
    })?;
    if bytes.len() as u64 > MAX_SHELL_LINK_BYTES {
        return Err(ShellLinkError::new(format!(
            "shortcut is larger than the {MAX_SHELL_LINK_BYTES}-byte limit"
        )));
    }
    parse_shell_link(&bytes)
}

/// Parses a shortcut and resolves its local target with deterministic candidate
/// ordering.
///
/// A `LinkInfo` target is authoritative. Otherwise, when `StringData` carries a
/// relative path, candidates are ordered by trust:
///
/// 1. `RelativePath` itself when it is absolute,
/// 2. a decoded file-system `LinkTargetIDList` target,
/// 3. a matching executable icon location,
/// 4. `WorkingDir + RelativePath`,
/// 5. `.lnk` parent directory + `RelativePath`.
///
/// The decoded ID list and icon executable are the shell's own target data, so
/// they must win over a same-named executable that merely happens to exist in
/// the working directory. `WorkingDir` is a launch context, not authoritative
/// target data. The first candidate that exists as a file wins. If none exists,
/// the result keeps `target_path = None` and records every attempted path so
/// callers can report an actionable broken-target reason. Shortcuts with no
/// local path data at all return no attempts and remain launchable through the
/// shell.
pub(crate) fn resolve_shell_link_file(path: &Path) -> Result<ResolvedShellLink, ShellLinkError> {
    let info = parse_shell_link_file(path)?;

    if let Some(target) = &info.target_path {
        let target = clean_candidate(target);
        return Ok(ResolvedShellLink {
            info,
            target_path: Some(target.clone()),
            attempted: vec![target],
        });
    }

    let Some(relative) = info.relative_path.as_deref() else {
        return Ok(ResolvedShellLink {
            info,
            target_path: None,
            attempted: Vec::new(),
        });
    };

    let relative = Path::new(relative);
    let mut attempted = Vec::with_capacity(5);

    // High-trust target data first. These describe the shell's actual target;
    // they must not be overridden by a coincidental same-name file in
    // WorkingDir. Each candidate is still checked for existence before use.
    if relative.is_absolute() {
        attempted.push(relative.to_path_buf());
    }
    if let Some(id_list_path) = &info.id_list_path {
        attempted.push(id_list_path.clone());
    }
    // Some shortcuts (notably Riot Client) carry a stale RelativePath while
    // their icon location is the real executable. Only accept an executable
    // whose file name matches the expected relative target, so a generic icon
    // cannot redirect the launch.
    if let Some(icon) = &info.icon_location {
        if let Some(candidate) = icon_target_candidate(icon, relative) {
            attempted.push(candidate);
        }
    }

    // Context-derived candidates are lower trust than the shell's own target
    // data, but still useful for ordinary relative shortcuts.
    if !relative.is_absolute() {
        if let Some(working_directory) = &info.working_directory {
            attempted.push(working_directory.join(relative));
        }
        if let Some(parent) = path.parent() {
            attempted.push(parent.join(relative));
        }
    }

    let mut seen = std::collections::HashSet::new();
    attempted.retain(|candidate| seen.insert(crate::start_menu::normalize_path_key(candidate)));

    let target_path = attempted
        .iter()
        .find(|candidate| candidate.is_file())
        .map(|candidate| clean_candidate(candidate));

    Ok(ResolvedShellLink {
        info,
        target_path,
        attempted,
    })
}

/// Reconstructs a local path from a file-system `LinkTargetIDList`.
///
/// The shell uses several item layouts. Only the drive/folder/file forms
/// (`0x2F`, `0x31`, `0x32`) are decoded; any other item shape makes the whole
/// list unusable, in which case `None` is returned and the shell still resolves
/// the `.lnk` itself. Names are ANSI for these forms. The result is only a
/// candidate: callers check that it exists before trusting it.
fn parse_id_list_path(id_list: &[u8]) -> Option<PathBuf> {
    const MAX_ITEMS: usize = 64;
    const MAX_NAME_BYTES: usize = 1024;

    let mut position = 0;
    let mut root: Option<String> = None;
    let mut components: Vec<String> = Vec::new();
    let mut items = 0;

    while position + 2 <= id_list.len() {
        let size = usize::from(u16::from_le_bytes([
            id_list[position],
            id_list[position + 1],
        ]));
        if size == 0 {
            break;
        }
        if size < 2 || position + size > id_list.len() {
            return None;
        }
        items += 1;
        if items > MAX_ITEMS {
            return None;
        }

        let item = &id_list[position + 2..position + size];
        let (kind, name_offset) = match item.first().copied()? {
            // Root-folder placeholder; the following 0x2F item carries the
            // drive/UNC root in the forms this parser understands.
            0x1F => {
                position += size;
                continue;
            }
            0x2F => (0x2F, 1),
            0x31 | 0x32 => (item[0], 12),
            _ => return None,
        };

        let tail = item.get(name_offset..)?;
        let end = tail.iter().position(|byte| *byte == 0)?;
        if end == 0 || end > MAX_NAME_BYTES {
            return None;
        }
        let name = decode_ansi(&tail[..end]);
        if kind == 0x2F {
            root = Some(name);
        } else {
            components.push(name);
        }

        position += size;
    }

    if items == 0 || components.is_empty() {
        return None;
    }

    let mut path = PathBuf::from(root?);
    for component in components {
        path.push(component);
    }
    (!path.as_os_str().is_empty()).then_some(path)
}

/// Returns the icon-location path as a target candidate when it exists, is an
/// executable and matches the relative target's file name.
fn icon_target_candidate(icon_location: &str, relative: &Path) -> Option<PathBuf> {
    let without_index = icon_location
        .rsplit_once(',')
        .map(|(path, index)| {
            if index.trim().parse::<i32>().is_ok() {
                path
            } else {
                icon_location
            }
        })
        .unwrap_or(icon_location);
    let candidate = PathBuf::from(expand_environment_variables(without_index.trim()));
    if !candidate.is_absolute() || !candidate.is_file() {
        return None;
    }
    if !candidate
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return None;
    }
    let expected = relative.file_name()?.to_str()?;
    let actual = candidate.file_name()?.to_str()?;
    actual.eq_ignore_ascii_case(expected).then_some(candidate)
}

/// Lexically removes `.`/`..` components without touching the filesystem.
fn clean_candidate(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut cleaned = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                cleaned.pop();
            }
            other => cleaned.push(other.as_os_str()),
        }
    }
    cleaned
}

/// Parses an in-memory `.lnk` image.
///
/// This function is pure and always either returns complete metadata or a
/// descriptive error; it never reads past `bytes`.
pub(crate) fn parse_shell_link(bytes: &[u8]) -> Result<ShellLinkInfo, ShellLinkError> {
    if bytes.len() < HEADER_SIZE {
        return Err(ShellLinkError::new(format!(
            "shortcut header is truncated ({} of {HEADER_SIZE} bytes)",
            bytes.len()
        )));
    }

    let mut reader = Reader::new(bytes);

    let header_size = reader.u32()? as usize;
    if header_size != HEADER_SIZE {
        return Err(ShellLinkError::new(format!(
            "unsupported ShellLinkHeader size {header_size:#x}"
        )));
    }

    let clsid = reader.take(16)?;
    if clsid != LINK_CLSID {
        return Err(ShellLinkError::new(
            "shortcut does not carry the Shell Link CLSID",
        ));
    }

    let flags = reader.u32()?;
    // FileAttributes, three FILETIMEs, FileSize, IconIndex, ShowCommand, HotKey
    // and reserved fields are validated only for presence.
    reader.skip(4 + 8 + 8 + 8 + 4 + 4 + 4 + 2 + 2 + 4 + 4)?;

    let id_list_path = if flags & FLAG_HAS_LINK_TARGET_ID_LIST != 0 {
        let id_list_size = usize::from(reader.u16()?);
        parse_id_list_path(reader.take(id_list_size)?)
    } else {
        None
    };

    let target_path = if flags & FLAG_HAS_LINK_INFO != 0 {
        parse_link_info(reader.remaining_slice())?
    } else {
        None
    };

    // Advance past LinkInfo using its own declared size. The helper above
    // validates that size, so this cannot run past the buffer.
    if flags & FLAG_HAS_LINK_INFO != 0 {
        let link_info_size = peek_u32(reader.remaining_slice())? as usize;
        reader.skip(link_info_size)?;
    }

    let mut name = None;
    let mut relative_path = None;
    let mut working_directory = None;
    let mut arguments = None;
    let mut icon_location = None;
    let unicode = flags & FLAG_IS_UNICODE != 0;

    for (flag, slot) in [
        (FLAG_HAS_NAME, &mut name),
        (FLAG_HAS_RELATIVE_PATH, &mut relative_path),
        (FLAG_HAS_WORKING_DIR, &mut working_directory),
        (FLAG_HAS_ARGUMENTS, &mut arguments),
        (FLAG_HAS_ICON_LOCATION, &mut icon_location),
    ] {
        if flags & flag != 0 {
            *slot = Some(read_string_data(&mut reader, unicode)?);
        }
    }

    Ok(ShellLinkInfo {
        target_path: target_path.filter(|path| !path.as_os_str().is_empty()),
        relative_path: relative_path.filter(|value| !value.is_empty()),
        id_list_path,
        working_directory: working_directory
            .filter(|value| !value.is_empty())
            .map(PathBuf::from),
        arguments: arguments.filter(|value| !value.is_empty()),
        icon_location: icon_location.filter(|value| !value.is_empty()),
    })
}

fn parse_link_info(block: &[u8]) -> Result<Option<PathBuf>, ShellLinkError> {
    if block.len() < LINK_INFO_MIN_HEADER {
        return Err(ShellLinkError::new("LinkInfo structure is truncated"));
    }

    let size = peek_u32(block)? as usize;
    if size < LINK_INFO_MIN_HEADER {
        return Err(ShellLinkError::new(format!(
            "LinkInfo size {size} is smaller than its header"
        )));
    }
    if size > block.len() {
        return Err(ShellLinkError::new(format!(
            "LinkInfo size {size} exceeds the remaining {}-byte shortcut data",
            block.len()
        )));
    }
    let info = &block[..size];

    let header_size = peek_u32_at(info, 4)? as usize;
    if header_size < LINK_INFO_MIN_HEADER || header_size > size {
        return Err(ShellLinkError::new(format!(
            "LinkInfo header size {header_size:#x} is invalid for a {size:#x}-byte structure"
        )));
    }

    let flags = peek_u32_at(info, 8)?;
    if flags & LINK_INFO_VOLUME_ID_AND_LOCAL_BASE_PATH == 0 {
        // A common-network-relative or volume-less link has no local path.
        return Ok(None);
    }

    let local_base_offset = peek_u32_at(info, 16)? as usize;
    let common_suffix_offset = peek_u32_at(info, 24)? as usize;

    let mut base = None;
    if header_size >= LINK_INFO_UNICODE_HEADER && info.len() >= LINK_INFO_UNICODE_HEADER {
        let unicode_base_offset = peek_u32_at(info, 28)? as usize;
        let unicode_suffix_offset = peek_u32_at(info, 32)? as usize;
        if unicode_base_offset != 0 {
            let mut value = read_utf16_string(info, unicode_base_offset)?;
            if unicode_suffix_offset != 0 {
                value.push_str(&read_utf16_string(info, unicode_suffix_offset)?);
            }
            base = Some(value);
        }
    }

    if base.is_none() && local_base_offset != 0 {
        let mut value = read_ansi_string(info, local_base_offset)?;
        if common_suffix_offset != 0 {
            value.push_str(&read_ansi_string(info, common_suffix_offset)?);
        }
        base = Some(value);
    }

    let base = match base {
        Some(value) if !value.is_empty() => value,
        _ => return Ok(None),
    };

    Ok(Some(PathBuf::from(expand_environment_variables(&base))))
}

fn read_string_data(reader: &mut Reader<'_>, unicode: bool) -> Result<String, ShellLinkError> {
    let count = usize::from(reader.u16()?);
    if unicode {
        let byte_count = count
            .checked_mul(2)
            .ok_or_else(|| ShellLinkError::new("StringData character count overflows"))?;
        let bytes = reader.take(byte_count)?;
        decode_utf16_le(bytes)
    } else {
        let bytes = reader.take(count)?;
        Ok(decode_ansi(bytes))
    }
}

fn read_ansi_string(block: &[u8], offset: usize) -> Result<String, ShellLinkError> {
    let tail = block
        .get(offset..)
        .ok_or_else(|| ShellLinkError::new("LinkInfo string offset is out of bounds"))?;
    let end = tail
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| ShellLinkError::new("LinkInfo string is not NUL-terminated"))?;
    Ok(decode_ansi(&tail[..end]))
}

fn read_utf16_string(block: &[u8], offset: usize) -> Result<String, ShellLinkError> {
    let tail = block
        .get(offset..)
        .ok_or_else(|| ShellLinkError::new("LinkInfo Unicode string offset is out of bounds"))?;

    let mut units = Vec::new();
    let mut index = 0;
    loop {
        let low = *tail
            .get(index)
            .ok_or_else(|| ShellLinkError::new("LinkInfo Unicode string is truncated"))?;
        let high = *tail
            .get(index + 1)
            .ok_or_else(|| ShellLinkError::new("LinkInfo Unicode string is truncated"))?;
        let unit = u16::from_le_bytes([low, high]);
        if unit == 0 {
            break;
        }
        units.push(unit);
        index += 2;
    }
    Ok(String::from_utf16_lossy(&units))
}

fn decode_utf16_le(bytes: &[u8]) -> Result<String, ShellLinkError> {
    if bytes.len() % 2 != 0 {
        return Err(ShellLinkError::new(
            "Unicode StringData has an odd byte length",
        ));
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    Ok(String::from_utf16_lossy(&units))
}

fn decode_ansi(bytes: &[u8]) -> String {
    // The shell stores this in the active ANSI code page. Byte-preserving
    // Latin-1 decoding keeps ASCII paths exact and never truncates data; a
    // non-ASCII path may not match the shell's code page and is then reported
    // as a missing target rather than being launched.
    bytes.iter().map(|byte| char::from(*byte)).collect()
}

fn peek_u32(block: &[u8]) -> Result<u32, ShellLinkError> {
    peek_u32_at(block, 0)
}

fn peek_u32_at(block: &[u8], offset: usize) -> Result<u32, ShellLinkError> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| ShellLinkError::new("shortcut structure offset overflows"))?;
    let bytes = block
        .get(offset..end)
        .ok_or_else(|| ShellLinkError::new("shortcut structure is truncated"))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Expands `%NAME%` references using the process environment.
///
/// The expansion is bounded and only matches short names built from ASCII
/// alphanumerics, underscore, parentheses and dots. Unknown variables are left
/// untouched so a target with a literal percent sign still fails closed at the
/// existence check rather than being silently rewritten.
fn expand_environment_variables(value: &str) -> String {
    let mut expanded = String::with_capacity(value.len());
    let mut rest = value;

    while let Some(start) = rest.find('%') {
        expanded.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            expanded.push_str(&rest[start..]);
            return expanded;
        };
        let name = &after[..end];
        let usable = !name.is_empty()
            && name.len() <= 64
            && name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "_().".contains(character));

        if usable {
            match std::env::var(name) {
                Ok(replacement) if !replacement.contains('\0') => expanded.push_str(&replacement),
                _ => {
                    expanded.push('%');
                    expanded.push_str(name);
                    expanded.push('%');
                }
            }
        } else {
            expanded.push('%');
            expanded.push_str(name);
            expanded.push('%');
        }
        rest = &after[end + 1..];
    }

    expanded.push_str(rest);
    expanded
}

/// Builds a minimal valid ANSI `.lnk` fixture pointing at `target`.
///
/// Shared with the discovery tests so they exercise the real parser instead of
/// hand-written placeholder files.
#[cfg(test)]
pub(crate) fn minimal_link_fixture(target: &str) -> Vec<u8> {
    let mut link_info = Vec::new();
    link_info.extend_from_slice(&0_u32.to_le_bytes()); // LinkInfoSize, patched
    link_info.extend_from_slice(&0x1C_u32.to_le_bytes()); // LinkInfoHeaderSize
    link_info.extend_from_slice(&LINK_INFO_VOLUME_ID_AND_LOCAL_BASE_PATH.to_le_bytes());
    link_info.extend_from_slice(&0_u32.to_le_bytes()); // VolumeIDOffset
    link_info.extend_from_slice(&0x1C_u32.to_le_bytes()); // LocalBasePathOffset
    link_info.extend_from_slice(&0_u32.to_le_bytes()); // CommonNetworkRelativeLinkOffset
    link_info.extend_from_slice(&0_u32.to_le_bytes()); // CommonPathSuffixOffset
    link_info.extend_from_slice(target.as_bytes());
    link_info.push(0);
    let size = link_info.len() as u32;
    link_info[..4].copy_from_slice(&size.to_le_bytes());

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
    bytes.extend_from_slice(&LINK_CLSID);
    bytes.extend_from_slice(&FLAG_HAS_LINK_INFO.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes()); // FileAttributes
    bytes.extend_from_slice(&[0; 24]); // creation/access/write times
    bytes.extend_from_slice(&0_u32.to_le_bytes()); // FileSize
    bytes.extend_from_slice(&0_u32.to_le_bytes()); // IconIndex
    bytes.extend_from_slice(&1_u32.to_le_bytes()); // ShowCommand
    bytes.extend_from_slice(&0_u16.to_le_bytes()); // HotKey
    bytes.extend_from_slice(&0_u16.to_le_bytes()); // Reserved1
    bytes.extend_from_slice(&0_u32.to_le_bytes()); // Reserved2
    bytes.extend_from_slice(&0_u32.to_le_bytes()); // Reserved3
    bytes.extend_from_slice(&link_info);
    bytes
}

/// Builds a minimal ANSI `.lnk` fixture with arguments and working directory.
#[cfg(test)]
pub(crate) fn link_with_args_and_working_fixture(
    target: &str,
    arguments: &str,
    working_directory: &str,
) -> Vec<u8> {
    let mut bytes = minimal_link_with_args_fixture(target, arguments);
    // Add HasWorkingDir. StringData order is WorkingDir before Arguments, so
    // insert the working-directory string before the existing argument string.
    let flags = FLAG_HAS_LINK_INFO | FLAG_HAS_ARGUMENTS | FLAG_HAS_WORKING_DIR;
    bytes[20..24].copy_from_slice(&flags.to_le_bytes());
    let argument_start = bytes.len() - (2 + arguments.len());
    let mut working = Vec::new();
    working.extend_from_slice(
        &u16::try_from(working_directory.len())
            .expect("fixture working-directory length")
            .to_le_bytes(),
    );
    working.extend_from_slice(working_directory.as_bytes());
    bytes.splice(argument_start..argument_start, working);
    bytes
}

/// Builds a minimal ANSI `.lnk` fixture with arguments, used by identity tests.
#[cfg(test)]
pub(crate) fn minimal_link_with_args_fixture(target: &str, arguments: &str) -> Vec<u8> {
    let mut bytes = minimal_link_fixture(target);
    // Add HasArguments to LinkFlags (offset 20).
    let flags = FLAG_HAS_LINK_INFO | FLAG_HAS_ARGUMENTS;
    bytes[20..24].copy_from_slice(&flags.to_le_bytes());
    bytes.extend_from_slice(
        &u16::try_from(arguments.len())
            .expect("fixture argument length")
            .to_le_bytes(),
    );
    bytes.extend_from_slice(arguments.as_bytes());
    bytes
}

/// Builds a minimal shortcut with no `LinkInfo`, only a relative path and
/// working directory. Used to reproduce shortcuts such as Riot Client that rely
/// on `StringData` relative resolution.
#[cfg(test)]
pub(crate) fn relative_link_fixture(relative: &str, working_directory: &str) -> Vec<u8> {
    relative_link_with_icon_fixture(relative, working_directory, None)
}

#[cfg(test)]
pub(crate) fn relative_link_with_icon_fixture(
    relative: &str,
    working_directory: &str,
    icon_location: Option<&str>,
) -> Vec<u8> {
    let mut flags = FLAG_HAS_RELATIVE_PATH | FLAG_HAS_WORKING_DIR;
    if icon_location.is_some() {
        flags |= FLAG_HAS_ICON_LOCATION;
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
    bytes.extend_from_slice(&LINK_CLSID);
    bytes.extend_from_slice(&flags.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes()); // FileAttributes
    bytes.extend_from_slice(&[0; 24]); // creation/access/write times
    bytes.extend_from_slice(&0_u32.to_le_bytes()); // FileSize
    bytes.extend_from_slice(&0_u32.to_le_bytes()); // IconIndex
    bytes.extend_from_slice(&1_u32.to_le_bytes()); // ShowCommand
    bytes.extend_from_slice(&0_u16.to_le_bytes()); // HotKey
    bytes.extend_from_slice(&0_u16.to_le_bytes()); // Reserved1
    bytes.extend_from_slice(&0_u32.to_le_bytes()); // Reserved2
    bytes.extend_from_slice(&0_u32.to_le_bytes()); // Reserved3

    for value in [Some(relative), Some(working_directory), icon_location]
        .into_iter()
        .flatten()
    {
        bytes.extend_from_slice(
            &u16::try_from(value.len())
                .expect("fixture string length")
                .to_le_bytes(),
        );
        bytes.extend_from_slice(value.as_bytes());
    }
    bytes
}

struct Reader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    fn remaining_slice(&self) -> &'a [u8] {
        &self.data[self.position.min(self.data.len())..]
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], ShellLinkError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or_else(|| ShellLinkError::new("shortcut structure length overflows"))?;
        let bytes = self
            .data
            .get(self.position..end)
            .ok_or_else(|| ShellLinkError::new("shortcut structure is truncated"))?;
        self.position = end;
        Ok(bytes)
    }

    fn skip(&mut self, length: usize) -> Result<(), ShellLinkError> {
        self.take(length).map(|_| ())
    }

    fn u16(&mut self) -> Result<u16, ShellLinkError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, ShellLinkError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_u16(bytes: &mut Vec<u8>, value: u16) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn push_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn push_utf16(bytes: &mut Vec<u8>, value: &str) {
        for unit in value.encode_utf16() {
            push_u16(bytes, unit);
        }
    }

    /// Builds a minimal `.lnk` with LinkInfo and optional StringData sections.
    fn build_link(
        target: &str,
        unicode_target: bool,
        working_directory: Option<&str>,
        arguments: Option<&str>,
        icon: Option<&str>,
    ) -> Vec<u8> {
        let mut flags = FLAG_HAS_LINK_INFO;
        if working_directory.is_some() {
            flags |= FLAG_HAS_WORKING_DIR;
        }
        if arguments.is_some() {
            flags |= FLAG_HAS_ARGUMENTS;
        }
        if icon.is_some() {
            flags |= FLAG_HAS_ICON_LOCATION;
        }
        if unicode_target {
            flags |= FLAG_IS_UNICODE;
        }

        let mut link_info = Vec::new();
        let header_size = if unicode_target { 0x24_u32 } else { 0x1C_u32 };
        push_u32(&mut link_info, 0); // LinkInfoSize, patched below.
        push_u32(&mut link_info, header_size);
        push_u32(&mut link_info, LINK_INFO_VOLUME_ID_AND_LOCAL_BASE_PATH);
        push_u32(&mut link_info, 0); // VolumeIDOffset
        if unicode_target {
            push_u32(&mut link_info, 0); // LocalBasePathOffset (ANSI)
            push_u32(&mut link_info, 0); // CommonNetworkRelativeLinkOffset
            push_u32(&mut link_info, 0); // CommonPathSuffixOffset (ANSI)
            push_u32(&mut link_info, 0x24); // LocalBasePathOffsetUnicode
            push_u32(&mut link_info, 0); // CommonPathSuffixOffsetUnicode
            push_utf16(&mut link_info, target);
            push_u16(&mut link_info, 0);
        } else {
            push_u32(&mut link_info, 0x1C); // LocalBasePathOffset
            push_u32(&mut link_info, 0); // CommonNetworkRelativeLinkOffset
            push_u32(&mut link_info, 0); // CommonPathSuffixOffset
            link_info.extend_from_slice(target.as_bytes());
            link_info.push(0);
        }
        let size = link_info.len() as u32;
        link_info[..4].copy_from_slice(&size.to_le_bytes());

        let mut bytes = Vec::new();
        push_u32(&mut bytes, HEADER_SIZE as u32);
        bytes.extend_from_slice(&LINK_CLSID);
        push_u32(&mut bytes, flags);
        push_u32(&mut bytes, 0); // FileAttributes
        bytes.extend_from_slice(&[0; 24]); // creation/access/write times
        push_u32(&mut bytes, 0); // FileSize
        push_u32(&mut bytes, 0); // IconIndex
        push_u32(&mut bytes, 1); // ShowCommand
        push_u16(&mut bytes, 0); // HotKey
        push_u16(&mut bytes, 0); // Reserved1
        push_u32(&mut bytes, 0); // Reserved2
        push_u32(&mut bytes, 0); // Reserved3
        bytes.extend_from_slice(&link_info);

        for value in [working_directory, arguments, icon].into_iter().flatten() {
            if unicode_target {
                let units: Vec<u16> = value.encode_utf16().collect();
                push_u16(
                    &mut bytes,
                    u16::try_from(units.len()).expect("fixture string length"),
                );
                for unit in units {
                    push_u16(&mut bytes, unit);
                }
            } else {
                push_u16(
                    &mut bytes,
                    u16::try_from(value.len()).expect("fixture string length"),
                );
                bytes.extend_from_slice(value.as_bytes());
            }
        }

        bytes
    }

    #[test]
    fn parses_ansi_link_info_and_string_data() {
        let bytes = build_link(
            r"C:\Program Files\Example\example.exe",
            false,
            Some(r"C:\Program Files\Example"),
            Some("--profile work"),
            Some(r"C:\Program Files\Example\icon.ico,3"),
        );

        let info = parse_shell_link(&bytes).expect("valid shortcut");
        assert_eq!(
            info.target_path.as_deref(),
            Some(Path::new(r"C:\Program Files\Example\example.exe"))
        );
        assert_eq!(
            info.working_directory.as_deref(),
            Some(Path::new(r"C:\Program Files\Example"))
        );
        assert_eq!(info.arguments.as_deref(), Some("--profile work"));
        assert_eq!(
            info.icon_location.as_deref(),
            Some(r"C:\Program Files\Example\icon.ico,3")
        );
    }

    #[test]
    fn prefers_the_unicode_local_base_path() {
        let bytes = build_link(
            r"C:\Program Files\Exämple\example.exe",
            true,
            None,
            None,
            None,
        );

        let info = parse_shell_link(&bytes).expect("valid shortcut");
        assert_eq!(
            info.target_path.as_deref(),
            Some(Path::new(r"C:\Program Files\Exämple\example.exe"))
        );
    }

    #[test]
    fn handles_a_link_without_link_info() {
        let mut bytes = build_link(r"C:\App\app.exe", false, None, None, None);
        // Clear HasLinkInfo and drop the LinkInfo block byte-for-byte.
        let flags_offset = 4 + 16;
        bytes[flags_offset..flags_offset + 4].copy_from_slice(&0_u32.to_le_bytes());
        bytes.truncate(HEADER_SIZE);

        let info = parse_shell_link(&bytes).expect("header-only shortcut");
        assert_eq!(info.target_path, None);
    }

    #[test]
    fn rejects_truncated_and_malformed_headers() {
        let valid = build_link(r"C:\App\app.exe", false, None, None, None);

        assert!(parse_shell_link(&[]).is_err());
        assert!(parse_shell_link(&valid[..10]).is_err());

        let mut wrong_size = valid.clone();
        wrong_size[..4].copy_from_slice(&0x50_u32.to_le_bytes());
        assert!(parse_shell_link(&wrong_size).is_err());

        let mut wrong_clsid = valid.clone();
        wrong_clsid[4] ^= 0xFF;
        assert!(parse_shell_link(&wrong_clsid).is_err());
    }

    #[test]
    fn rejects_out_of_range_link_info_and_strings() {
        let mut out_of_range = build_link(r"C:\App\app.exe", false, None, None, None);
        let link_info_start = HEADER_SIZE;
        out_of_range[link_info_start..link_info_start + 4]
            .copy_from_slice(&0xFFFF_u32.to_le_bytes());
        assert!(parse_shell_link(&out_of_range).is_err());

        let mut bad_offset = build_link(r"C:\App\app.exe", false, None, None, None);
        bad_offset[link_info_start + 16..link_info_start + 20]
            .copy_from_slice(&0xFFFF_u32.to_le_bytes());
        assert!(parse_shell_link(&bad_offset).is_err());
    }

    #[test]
    fn rejects_an_oversized_string_count() {
        let mut bytes = build_link(r"C:\App\app.exe", true, Some("dir"), None, None);
        let string_data_start = bytes.len() - (2 + "dir".encode_utf16().count() * 2);
        bytes[string_data_start..string_data_start + 2].copy_from_slice(&0xFFFF_u16.to_le_bytes());
        assert!(parse_shell_link(&bytes).is_err());
    }

    #[test]
    fn every_truncation_of_a_valid_fixture_is_handled_without_panicking() {
        let valid = build_link(
            r"C:\Program Files\Example\example.exe",
            true,
            Some(r"C:\Program Files\Example"),
            Some("--flag"),
            Some(r"C:\icons\example.ico,1"),
        );

        for length in 0..valid.len() {
            let _ = parse_shell_link(&valid[..length]);
        }
        assert!(parse_shell_link(&valid).is_ok());
    }

    #[test]
    fn unknown_environment_variables_are_preserved_and_known_ones_expand() {
        let system_root = std::env::var("SystemRoot").expect("SystemRoot is set on Windows");
        let expanded = expand_environment_variables(r"%SystemRoot%\app.exe");
        assert_eq!(expanded, format!(r"{system_root}\app.exe"));

        let untouched = expand_environment_variables(r"%NOT_A_REAL_VARIABLE_123%\app.exe");
        assert_eq!(untouched, r"%NOT_A_REAL_VARIABLE_123%\app.exe");
    }

    fn id_list_item(kind: u8, name: &str) -> Vec<u8> {
        let name_offset = if kind == 0x2F { 1 } else { 12 };
        let mut item = Vec::new();
        item.push(kind);
        item.extend(std::iter::repeat(0_u8).take(name_offset - 1));
        item.extend_from_slice(name.as_bytes());
        item.push(0);
        let mut bytes = Vec::new();
        push_u16(
            &mut bytes,
            u16::try_from(item.len() + 2).expect("item size"),
        );
        bytes.extend_from_slice(&item);
        bytes
    }

    fn id_list_link(items: &[Vec<u8>]) -> Vec<u8> {
        let mut id_list = Vec::new();
        for item in items {
            id_list.extend_from_slice(item);
        }
        push_u16(&mut id_list, 0);

        let mut bytes = Vec::new();
        push_u32(&mut bytes, HEADER_SIZE as u32);
        bytes.extend_from_slice(&LINK_CLSID);
        push_u32(&mut bytes, FLAG_HAS_LINK_TARGET_ID_LIST);
        push_u32(&mut bytes, 0); // FileAttributes
        bytes.extend_from_slice(&[0; 24]);
        push_u32(&mut bytes, 0); // FileSize
        push_u32(&mut bytes, 0); // IconIndex
        push_u32(&mut bytes, 1); // ShowCommand
        push_u16(&mut bytes, 0); // HotKey
        push_u16(&mut bytes, 0); // Reserved1
        push_u32(&mut bytes, 0); // Reserved2
        push_u32(&mut bytes, 0); // Reserved3
        push_u16(
            &mut bytes,
            u16::try_from(id_list.len()).expect("id list size"),
        );
        bytes.extend_from_slice(&id_list);
        bytes
    }

    fn id_list_relative_link(items: &[Vec<u8>], relative: &str, working: &str) -> Vec<u8> {
        let mut id_list = Vec::new();
        for item in items {
            id_list.extend_from_slice(item);
        }
        push_u16(&mut id_list, 0);

        let flags = FLAG_HAS_LINK_TARGET_ID_LIST | FLAG_HAS_RELATIVE_PATH | FLAG_HAS_WORKING_DIR;
        let mut bytes = Vec::new();
        push_u32(&mut bytes, HEADER_SIZE as u32);
        bytes.extend_from_slice(&LINK_CLSID);
        push_u32(&mut bytes, flags);
        push_u32(&mut bytes, 0);
        bytes.extend_from_slice(&[0; 24]);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 1);
        push_u16(&mut bytes, 0);
        push_u16(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        push_u16(
            &mut bytes,
            u16::try_from(id_list.len()).expect("id list size"),
        );
        bytes.extend_from_slice(&id_list);
        for value in [relative, working] {
            push_u16(
                &mut bytes,
                u16::try_from(value.len()).expect("string length"),
            );
            bytes.extend_from_slice(value.as_bytes());
        }
        bytes
    }

    #[test]
    fn an_existing_id_list_target_wins_over_a_same_named_working_dir_file() {
        let root = std::env::temp_dir().join(format!(
            "ansa-shell-link-precedence-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let correct = root.join("Real").join("Tool.exe");
        let decoy = root.join("Wrong").join("Tool.exe");
        for path in [&correct, &decoy] {
            fs::create_dir_all(path.parent().expect("parent")).expect("create tree");
            fs::write(path, b"fixture").expect("write fixture");
        }

        let mut items = Vec::new();
        let components: Vec<_> = correct.components().collect();
        for component in &components {
            match component {
                std::path::Component::Prefix(prefix) => items.push(id_list_item(
                    0x2F,
                    &format!("{}\\", prefix.as_os_str().to_string_lossy()),
                )),
                std::path::Component::Normal(name) => {
                    items.push(id_list_item(0x31, &name.to_string_lossy()));
                }
                _ => {}
            }
        }
        // The final component is the file, not a folder.
        items.pop().expect("file component");
        let file_name = correct.file_name().expect("file name");
        items.push(id_list_item(0x32, &file_name.to_string_lossy()));

        let link = root.join("Programs").join("Tool.lnk");
        fs::create_dir_all(link.parent().expect("link parent")).expect("create link tree");
        fs::write(
            &link,
            id_list_relative_link(&items, "Tool.exe", &root.join("Wrong").to_string_lossy()),
        )
        .expect("write shortcut");

        let resolved = resolve_shell_link_file(&link).expect("resolve shortcut");
        let _ = fs::remove_dir_all(&root);

        assert_eq!(
            resolved.target_path,
            Some(correct),
            "the decoded IDList target must outrank the WorkingDir same-name file"
        );
    }

    #[test]
    fn reconstructs_a_local_path_from_link_target_id_list() {
        let items = vec![
            id_list_item(0x2F, "C:\\"),
            id_list_item(0x31, "Riot Games"),
            id_list_item(0x31, "Riot Client"),
            id_list_item(0x32, "RiotClientServices.exe"),
        ];
        let bytes = id_list_link(&items);
        let info = parse_shell_link(&bytes).expect("valid id-list shortcut");
        assert_eq!(
            info.id_list_path.as_deref(),
            Some(Path::new(
                r"C:\Riot Games\Riot Client\RiotClientServices.exe"
            ))
        );
    }

    #[test]
    fn an_unknown_id_list_item_shape_is_not_trusted() {
        let items = vec![id_list_item(0x2F, "C:\\"), id_list_item(0x99, "Mystery")];
        assert_eq!(parse_id_list_path(&id_list_link(&items)), None);
    }

    #[test]
    fn resolves_a_riot_style_relative_path_against_the_link_parent() {
        let root = std::env::temp_dir().join(format!(
            "ansa-shell-link-relative-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let link_directory = root.join("Riot Games");
        let target = link_directory
            .join("Riot Client")
            .join("RiotClientServices.exe");
        fs::create_dir_all(target.parent().expect("target parent")).expect("create target tree");
        fs::write(&target, b"fixture").expect("write target");
        let link = link_directory.join("Riot Client.lnk");
        fs::write(
            &link,
            relative_link_fixture(
                r"..\Riot Games\Riot Client\RiotClientServices.exe",
                r"C:/Riot Games/Riot Client",
            ),
        )
        .expect("write shortcut");

        let resolved = resolve_shell_link_file(&link).expect("resolve shortcut");
        let _ = fs::remove_dir_all(&root);

        assert_eq!(resolved.target_path, Some(target));
        assert!(
            resolved.attempted.len() >= 2,
            "working-directory and link-parent candidates must both be attempted"
        );
        assert!(
            resolved
                .attempted
                .iter()
                .any(|candidate| candidate.to_string_lossy().contains("Riot Games")),
            "attempts: {:?}",
            resolved.attempted
        );
    }

    #[test]
    fn resolves_riot_style_relative_path_from_a_matching_icon_location() {
        let root = std::env::temp_dir().join(format!(
            "ansa-shell-link-icon-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        let target = root
            .join("Riot Games")
            .join("Riot Client")
            .join("RiotClientServices.exe");
        fs::create_dir_all(target.parent().expect("target parent")).expect("create target tree");
        fs::write(&target, b"fixture").expect("write target");
        let link = root
            .join("Programs")
            .join("Riot Games")
            .join("Riot Client.lnk");
        fs::create_dir_all(link.parent().expect("link parent")).expect("create link tree");
        fs::write(
            &link,
            relative_link_with_icon_fixture(
                r"..\Riot Games\Riot Client\RiotClientServices.exe",
                r"C:/Riot Games/Riot Client",
                Some(&format!("{},0", target.display())),
            ),
        )
        .expect("write shortcut");

        let resolved = resolve_shell_link_file(&link).expect("resolve shortcut");
        let _ = fs::remove_dir_all(&root);

        assert_eq!(resolved.target_path, Some(target));
        assert!(resolved.attempted.len() >= 3);
    }

    #[test]
    fn records_all_relative_attempts_when_none_exists() {
        let root = std::env::temp_dir().join(format!(
            "ansa-shell-link-relative-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&root).expect("create root");
        let link = root.join("Missing.lnk");
        fs::write(
            &link,
            relative_link_fixture(r"..\Missing\App.exe", r"C:\Does\Not\Exist"),
        )
        .expect("write shortcut");

        let resolved = resolve_shell_link_file(&link).expect("resolve shortcut");
        let _ = fs::remove_dir_all(&root);

        assert_eq!(resolved.target_path, None);
        assert_eq!(resolved.attempted.len(), 2);
        assert!(resolved.info.relative_path.is_some());
    }

    #[test]
    fn a_non_file_target_is_rejected_by_the_file_reader() {
        let path = std::env::temp_dir().join("ansa-shell-link-directory");
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create fixture directory");
        let result = parse_shell_link_file(&path);
        let _ = fs::remove_dir_all(&path);
        assert!(result.is_err());
    }
}
