//! Byte encodings `hipc` writes: lowercase hex, and percent-encoding as RFC 3986 defines it.

use std::fmt::Write as _;

/// `bytes` in lowercase hex.
pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(out, "{byte:02x}").expect("writing to a String does not fail");
    }
    out
}

/// Percent-encodes everything outside the unreserved set of RFC 3986, section 2.3.
pub(crate) fn percent_encode(text: &str) -> String {
    percent_encode_keeping(text, b"")
}

/// [`percent_encode`], with uppercase hex, that also leaves the bytes of `kept`, which are
/// ASCII, as they are.
pub(crate) fn percent_encode_keeping(text: &str, kept: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    debug_assert!(kept.is_ascii(), "only ASCII bytes are kept");
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) || kept.contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    out
}

/// `encoded`, a [`percent_encode`] result, with the two hex digits after each `%` in
/// lowercase. The unreserved characters between them keep their case.
pub(crate) fn lowercase_escapes(encoded: &str) -> String {
    let mut out = String::with_capacity(encoded.len());
    let mut hex_left = 0;
    for c in encoded.chars() {
        if hex_left > 0 {
            out.push(c.to_ascii_lowercase());
            hex_left -= 1;
        } else {
            out.push(c);
            if c == '%' {
                hex_left = 2;
            }
        }
    }
    out
}
