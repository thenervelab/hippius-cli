//! Narrow typed views of an answer: the few fields `hipc` decides on, each named and typed in
//! one place beside its route, and read from the answer's `Value`.
//!
//! A view is never printed: the document is the server's `Value`, as it came (tenets 3 and
//! 15). A view refuses no field it does not know, so a field the server adds changes nothing.
//! It uses neither `#[serde(flatten)]` nor an untagged or internally tagged enum: with
//! `serde_json`'s `arbitrary_precision`, which `hipc` needs to print a server number unrounded,
//! those buffer a number as a private map and fail to read it
//! (<https://github.com/serde-rs/json/issues/559>).
//!
//! Most views are tolerant, as the `Value::as_*` reads they replace were: a field that is
//! missing, `null`, or of another type reads as `None` ([`field`]), and an answer that is not
//! an object reads as a view with every field `None` ([`view_of`]). A view a command depends on
//! whole is read with `deserialize` instead, and its error is the command's `internal`.

use serde::Deserialize as _;
use serde::de::{DeserializeOwned, Deserializer};
use serde_json::Value;

/// The tolerant view of `answer`: `field` for each field, and every field `None` when
/// `answer` is not an object, as `answer.get(..)` reads it.
pub fn view_of<T: DeserializeOwned + Default>(answer: &Value) -> T {
    T::deserialize(answer).unwrap_or_default()
}

/// Reads a field as `T`, or `None` when it is not a `T`, as `Value::as_str` and its siblings
/// do. With `#[serde(default)]`, a missing field is `None` too. A number is read exactly or not
/// at all: `u64` takes an integer that fits and nothing else, as `Value::as_u64` does.
pub(crate) fn field<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = Value::deserialize(deserializer)?;
    Ok(T::deserialize(value).ok())
}

/// Reads a field as text: a string as it is, a number as its digits (exact under
/// `arbitrary_precision`), and `None` for anything else. An id the API sends as either is
/// read so, as the console reads `String(id)`.
pub(crate) fn text_or_number<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Value::deserialize(deserializer)? {
        Value::String(text) => Some(text),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    #[derive(Debug, Default, Deserialize)]
    #[expect(
        clippy::option_option,
        reason = "the probe tells `null` from a wrong type"
    )]
    struct Probe {
        #[serde(default, deserialize_with = "field")]
        name: Option<String>,
        #[serde(default, deserialize_with = "field")]
        count: Option<u64>,
        #[serde(default, deserialize_with = "field")]
        cap: Option<Option<String>>,
        #[serde(default, deserialize_with = "text_or_number")]
        id: Option<String>,
    }

    /// Each field reads as `Value::as_*` would: a wrong type is `None`, not an error, and a
    /// number past `u64` is `None`, not rounded.
    #[test]
    fn a_field_of_another_type_reads_as_none() {
        let read: Probe = view_of(&json!({
            "name": 5,
            "count": 18_446_744_073_709_551_616_u128,
            "cap": null,
            "id": 99_000_000_000_000_000_000_u128,
            "other": [1],
        }));
        assert_eq!(read.name, None);
        assert_eq!(read.count, None);
        assert_eq!(read.cap, Some(None));
        assert_eq!(read.id.as_deref(), Some("99000000000000000000"));

        let read: Probe = view_of(&json!({"name": "n", "count": 7, "cap": "1.00", "id": "p"}));
        assert_eq!(read.name.as_deref(), Some("n"));
        assert_eq!(read.count, Some(7));
        assert_eq!(read.cap, Some(Some("1.00".to_owned())));
        assert_eq!(read.id.as_deref(), Some("p"));

        for not_an_object in [json!([1]), json!("x"), Value::Null] {
            let read: Probe = view_of(&not_an_object);
            assert_eq!(
                (read.name, read.count, read.cap, read.id),
                (None, None, None, None)
            );
        }
        let read: Probe = view_of(&json!({"count": 1.0, "id": true}));
        assert_eq!((read.count, read.id), (None, None));
    }
}
