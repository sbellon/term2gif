//! Incremental UTF-8 decoding for PTY output.
//!
//! PTY reads can end in the middle of a multi-byte character. asciicast event
//! data must be valid UTF-8 text, so incomplete trailing bytes are carried over
//! to the next read, and genuinely invalid bytes become U+FFFD.

/// Appends `new` to `carry` and returns all text that can be decoded so far.
/// Bytes of an incomplete trailing sequence stay in `carry`.
pub fn decode_chunk(carry: &mut Vec<u8>, new: &[u8]) -> String {
    carry.extend_from_slice(new);

    let mut out = String::new();
    let mut pos = 0;

    loop {
        match std::str::from_utf8(&carry[pos..]) {
            Ok(s) => {
                out.push_str(s);
                pos = carry.len();
                break;
            }
            Err(e) => {
                let valid = e.valid_up_to();
                // The prefix up to `valid_up_to` is valid UTF-8 by definition.
                out.push_str(&String::from_utf8_lossy(&carry[pos..pos + valid]));
                pos += valid;

                match e.error_len() {
                    // Invalid sequence: replace it and keep going.
                    Some(n) => {
                        out.push('\u{FFFD}');
                        pos += n;
                    }
                    // Incomplete sequence at the end: wait for more bytes.
                    None => break,
                }
            }
        }
    }

    carry.drain(..pos);
    out
}

#[cfg(test)]
mod tests {
    use super::decode_chunk;

    #[test]
    fn plain_ascii() {
        let mut carry = Vec::new();
        assert_eq!(decode_chunk(&mut carry, b"hello"), "hello");
        assert_eq!(carry, b"");
    }

    #[test]
    fn multibyte_split_across_reads() {
        let bytes = "a€b".as_bytes(); // '€' is 3 bytes
        let mut carry = Vec::new();
        assert_eq!(decode_chunk(&mut carry, &bytes[..2]), "a");
        assert_eq!(carry.len(), 1);
        assert_eq!(decode_chunk(&mut carry, &bytes[2..3]), "");
        assert_eq!(decode_chunk(&mut carry, &bytes[3..]), "€b");
        assert_eq!(carry, b"");
    }

    #[test]
    fn invalid_byte_is_replaced() {
        let mut carry = Vec::new();
        assert_eq!(decode_chunk(&mut carry, b"a\xffb"), "a\u{FFFD}b");
        assert_eq!(carry, b"");
    }

    #[test]
    fn invalid_after_incomplete_start() {
        // 0xE2 starts a 3-byte sequence but is followed by ASCII.
        let mut carry = Vec::new();
        assert_eq!(decode_chunk(&mut carry, b"\xe2x"), "\u{FFFD}x");
        assert_eq!(carry, b"");
    }

    #[test]
    fn emoji_split_byte_by_byte() {
        let bytes = "🙂".as_bytes();
        let mut carry = Vec::new();
        let mut out = String::new();
        for b in bytes {
            out.push_str(&decode_chunk(&mut carry, &[*b]));
        }
        assert_eq!(out, "🙂");
        assert_eq!(carry, b"");
    }
}
