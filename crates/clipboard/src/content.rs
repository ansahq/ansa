//! Decrypted clipboard content.
//!
//! [`ClipboardContent`] is the primary type that owns accepted plaintext. It is
//! not `Clone`, its `Debug` implementation prints only shape information, and
//! it overwrites its buffers on drop. Rejected [`crate::filter::SnapshotContent`]
//! and adapter scratch buffers also wipe on a best-effort basis, but copies
//! made by the caller (for example by formatting a preview) are outside that
//! guarantee, so callers should keep previews bounded and turn the store lock
//! on when done.

use std::fmt;

use crate::model::ClipboardFormat;

/// Decrypted text or file-list content.
#[derive(PartialEq, Eq)]
pub enum ClipboardContent {
    /// Unicode text.
    Text(String),
    /// Absolute or relative paths from a file list.
    FileList(Vec<String>),
}

impl ClipboardContent {
    /// Construct text content.
    #[must_use]
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }

    /// Construct file-list content.
    #[must_use]
    pub fn file_list(paths: Vec<String>) -> Self {
        Self::FileList(paths)
    }

    /// The corresponding format.
    #[must_use]
    pub fn format(&self) -> ClipboardFormat {
        match self {
            Self::Text(_) => ClipboardFormat::Text,
            Self::FileList(_) => ClipboardFormat::FileList,
        }
    }

    /// Payload size in bytes, excluding any envelope.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        match self {
            Self::Text(value) => value.len(),
            Self::FileList(paths) => paths
                .iter()
                .fold(0usize, |total, path| total.saturating_add(path.len())),
        }
    }

    /// Number of file paths, or `None` for text.
    #[must_use]
    pub fn file_count(&self) -> Option<usize> {
        match self {
            Self::Text(_) => None,
            Self::FileList(paths) => Some(paths.len()),
        }
    }

    /// A bounded preview for display or search results.
    ///
    /// At most `max_chars` Unicode scalar values are returned; a truncated
    /// result ends with `...`. This necessarily creates a plaintext copy for the
    /// caller.
    #[must_use]
    pub fn preview(&self, max_chars: usize) -> String {
        match self {
            Self::Text(value) => truncate(value, max_chars),
            Self::FileList(paths) => {
                let mut joined = String::new();
                for path in paths {
                    if !joined.is_empty() {
                        joined.push('\n');
                    }
                    joined.push_str(path);
                    if joined.chars().count() >= max_chars {
                        break;
                    }
                }
                truncate(&joined, max_chars)
            }
        }
    }

    /// Overwrite the owned buffers.
    pub fn wipe(&mut self) {
        match self {
            Self::Text(value) => wipe_string(value),
            Self::FileList(paths) => {
                for path in paths {
                    wipe_string(path);
                }
            }
        }
    }
}

impl fmt::Debug for ClipboardContent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(value) => formatter
                .debug_struct("ClipboardContent::Text")
                .field("characters", &value.chars().count())
                .finish_non_exhaustive(),
            Self::FileList(paths) => formatter
                .debug_struct("ClipboardContent::FileList")
                .field("files", &paths.len())
                .finish_non_exhaustive(),
        }
    }
}

impl Drop for ClipboardContent {
    fn drop(&mut self) {
        self.wipe();
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut output = String::new();
    for (index, character) in value.chars().enumerate() {
        if index >= max_chars {
            output.push_str("...");
            break;
        }
        output.push(character);
    }
    output
}

pub(crate) fn wipe_bytes(bytes: &mut [u8]) {
    // SAFETY: `bytes` is a valid, exclusively borrowed initialized buffer.
    // Volatile writes prevent the compiler from eliding the overwrite.
    unsafe {
        for byte in bytes {
            std::ptr::write_volatile(byte, 0);
        }
    }
}

#[cfg(windows)]
pub(crate) fn wipe_utf16(units: &mut [u16]) {
    // SAFETY: `units` is a valid, exclusively borrowed initialized buffer.
    // Volatile writes prevent the compiler from eliding the overwrite.
    unsafe {
        for unit in units {
            std::ptr::write_volatile(unit, 0);
        }
    }
}

pub(crate) fn wipe_string(value: &mut str) {
    // SAFETY: the string slice is initialized, exclusively borrowed and `len`
    // bytes long. Writing zero bytes keeps every byte valid UTF-8 (NUL is a
    // valid scalar value) and does not change the length. Volatile writes
    // prevent the compiler from eliding the overwrite.
    unsafe {
        for byte in value.as_bytes_mut() {
            std::ptr::write_volatile(byte, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_preview_do_not_echo_unbounded_content() {
        let content = ClipboardContent::text("correct horse battery staple");
        assert!(!format!("{content:?}").contains("horse"));
        assert_eq!(content.preview(7), "correct...");
        assert_eq!(content.preview(100), "correct horse battery staple");
    }

    #[test]
    fn file_list_preview_is_bounded() {
        let content =
            ClipboardContent::file_list(vec!["C:\\one.txt".to_owned(), "C:\\two.txt".to_owned()]);
        assert_eq!(content.file_count(), Some(2));
        assert!(content.preview(10).chars().count() <= 13);
        assert_eq!(content.preview(200), "C:\\one.txt\nC:\\two.txt".to_owned());
    }

    #[test]
    fn wiping_overwrites_the_buffer() {
        let mut content = ClipboardContent::text("sentinel-secret-value");
        content.wipe();
        match &content {
            ClipboardContent::Text(value) => {
                assert!(value.bytes().all(|byte| byte == 0));
            }
            ClipboardContent::FileList(_) => panic!("wrong variant"),
        }
    }
}
