//! Rejection of invisible Unicode format controls.
//!
//! Bidi overrides and zero-width controls can make a consequential target or a
//! list title render differently from its bytes. These characters are rejected
//! in display-sensitive text. Language/emoji joiners (`U+200C`, `U+200D`) stay
//! allowed because they are required to render legitimate text.

/// Returns `true` for bidi and zero-width format characters that can spoof
/// displayed text.
pub(crate) fn is_unsafe_format_character(character: char) -> bool {
    matches!(
        character as u32,
        0x061c // Arabic letter mark
            | 0x200b // zero-width space
            | 0x200e // left-to-right mark
            | 0x200f // right-to-left mark
            | 0x202a..=0x202e // bidi embedding/override controls
            | 0x2060 // word joiner
            | 0x2066..=0x2069 // bidi isolates
            | 0xfeff // zero-width no-break space / BOM
    )
}

/// Finds the first unsafe format character and its byte position.
pub(crate) fn find_unsafe_format(text: &str) -> Option<(usize, char)> {
    text.char_indices()
        .find(|(_, character)| is_unsafe_format_character(*character))
}

/// Returns `true` for Unicode whitespace and soft hyphen that must not appear
/// inside a target. An ordinary ASCII space is allowed because local paths may
/// contain it; URL validation rejects it separately.
pub(crate) fn is_unsafe_target_whitespace(character: char) -> bool {
    character != ' ' && (character.is_whitespace() || character == '\u{00ad}')
}
