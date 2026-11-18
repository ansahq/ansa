//! Minimal JSON writing helpers. The harness has no serialization dependency.

/// Escapes a string for a JSON string literal (without the surrounding quotes).
pub fn escape(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            character if (character as u32) < 0x20 => {
                output.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => output.push(character),
        }
    }
    output
}

/// Returns a quoted and escaped JSON string.
pub fn string(value: &str) -> String {
    format!("\"{}\"", escape(value))
}

/// Formats a number for JSON. Non-finite values are emitted as `null` rather
/// than the invalid JSON tokens `NaN`/`inf`.
pub fn number(value: f64) -> String {
    if value.is_finite() {
        let mut rendered = format!("{value:.3}");
        while rendered.contains('.') && rendered.ends_with('0') {
            rendered.pop();
        }
        if rendered.ends_with('.') {
            rendered.push('0');
        }
        rendered
    } else {
        "null".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_quotes_controls_and_backslashes() {
        assert_eq!(escape("a\"b\\c\n\td"), "a\\\"b\\\\c\\n\\td");
        assert_eq!(string("Ansa"), "\"Ansa\"");
        assert_eq!(escape("\u{01}"), "\\u0001");
        // Non-ASCII passes through as UTF-8; JSON permits it.
        assert_eq!(escape("café"), "café");
    }

    #[test]
    fn numbers_are_valid_json() {
        assert_eq!(number(1.0), "1.0");
        assert_eq!(number(1.23456), "1.235");
        assert_eq!(number(0.0), "0.0");
        assert_eq!(number(f64::NAN), "null");
        assert_eq!(number(f64::INFINITY), "null");
    }
}
