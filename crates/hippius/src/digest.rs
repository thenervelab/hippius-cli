//! The SHA-256 of bytes `hipc` sends, for a command that checks the server hashed the same
//! bytes: an App Engine compose is measured into its VM by that hash, so a compose the server
//! read differently would launch something other than the file the caller named.

use sha2::{Digest as _, Sha256};

/// The SHA-256 of `bytes`, as 64 lowercase hex digits.
pub fn sha256_hex(bytes: &[u8]) -> String {
    crate::encode::hex(&Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_is_lowercase_hex_of_the_exact_bytes() {
        // FIPS 180-2, appendix B.1, and the empty message.
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
