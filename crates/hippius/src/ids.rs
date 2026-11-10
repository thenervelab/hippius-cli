//! Random ids `hipc` makes for a request: a version-4 UUID, as a cluster operation's request id
//! and as an idempotency key.

use crate::error::{Error, Result};
use crate::phrase::quote;

/// A new random idempotency key: a lowercase version-4 UUID.
///
/// # Errors
///
/// `internal` when the system gives no random bytes.
pub fn new_idempotency_key() -> Result<String> {
    random_uuid("an idempotency key")
}

/// A new random lowercase version-4 UUID, as `purpose` ("a request id") names it in the
/// refusal when the system gives no random bytes.
pub(crate) fn random_uuid(purpose: &str) -> Result<String> {
    let mut random = [0_u8; 16];
    refused_without_random(getrandom::getrandom(&mut random), purpose)?;
    Ok(uuid_v4(random))
}

/// A lowercase version-4 UUID made of `random`: the version nibble set to 4 and the variant
/// bits to `10`, as RFC 9562 section 5.4 says.
fn uuid_v4(mut random: [u8; 16]) -> String {
    random[6] = (random[6] & 0x0f) | 0x40;
    random[8] = (random[8] & 0x3f) | 0x80;
    let hex = crate::encode::hex(&random);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// `filled`, the outcome of reading random bytes for `purpose`, with the system's refusal as
/// an `internal` error that names the purpose.
fn refused_without_random(
    filled: std::result::Result<(), getrandom::Error>,
    purpose: &str,
) -> Result<()> {
    filled.map_err(|error| {
        Error::internal(format!(
            "cannot read random bytes for {purpose}: the system says {}",
            quote(error)
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;

    #[test]
    fn a_uuid_gets_the_version_and_variant_bits() {
        assert_eq!(uuid_v4([0; 16]), "00000000-0000-4000-8000-000000000000");
        assert_eq!(uuid_v4([0xff; 16]), "ffffffff-ffff-4fff-bfff-ffffffffffff");
    }

    #[test]
    fn no_random_bytes_is_internal_and_names_its_purpose() {
        for purpose in ["a request id", "an idempotency key"] {
            let error =
                refused_without_random(Err(getrandom::Error::UNSUPPORTED), purpose).unwrap_err();
            assert_eq!(error.code(), Code::Internal);
            let expected = format!("cannot read random bytes for {purpose}: the system says `");
            assert_eq!(error.message().get(..expected.len()), Some(&*expected));
        }
    }

    #[test]
    fn idempotency_keys_are_new_v4_uuids() {
        let first = new_idempotency_key().unwrap();
        assert_eq!(first.len(), 36);
        assert_eq!(&first[14..15], "4");
        assert_ne!(first, new_idempotency_key().unwrap());
    }
}
