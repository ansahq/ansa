//! Small, strict hexadecimal codec for ciphertext serialization.
//!
//! The vault only needs to move opaque bytes through a JSON field. This module
//! does not encode or decode any secret material itself.

const DIGITS: &[u8; 16] = b"0123456789abcdef";

/// Encode bytes as lowercase hexadecimal.
pub(crate) fn encode(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

/// Decode strict hexadecimal, accepting either case.
///
/// Returns `None` for odd lengths or any non-hexadecimal byte.
pub(crate) fn decode(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let mut output = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.as_chunks::<2>().0 {
        let high = nibble(pair[0])?;
        let low = nibble(pair[1])?;
        output.push((high << 4) | low);
    }
    Some(output)
}

fn nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    #[test]
    fn round_trip_and_reject_invalid() {
        let bytes = [0u8, 1, 15, 16, 127, 128, 255];
        assert_eq!(encode(&bytes), "00010f107f80ff");
        assert_eq!(decode("00010f107f80ff"), Some(bytes.to_vec()));
        assert_eq!(decode("00010F107F80FF"), Some(bytes.to_vec()));
        assert_eq!(decode("0"), None);
        assert_eq!(decode("zz"), None);
        assert_eq!(decode("0g"), None);
    }
}
