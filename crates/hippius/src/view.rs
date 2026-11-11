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
//! an object reads as a view with every field `None` ([`view_of`]). A view inside a view is
//! read the same way ([`nested`], [`each`]), as `answer.pointer(..)` reads it. A decision that
//! tells a missing field from a `null` one, or from one of another type, reads a [`Slot`]. A
//! view a command depends on whole is read with `deserialize` instead, and its error is the
//! command's `internal`.

use serde::de::{Deserialize, DeserializeOwned, Deserializer};
use serde_json::Value;

/// The tolerant view of `answer`: `field` for each field, and every field `None` when
/// `answer` is not an object, as `answer.get(..)` reads it. An array is not read by position,
/// as serde would read it into a struct.
pub fn view_of<T: DeserializeOwned + Default>(answer: &Value) -> T {
    if !answer.is_object() {
        return T::default();
    }
    T::deserialize(answer).unwrap_or_default()
}

/// What an answer holds under one key, for a decision that tells a missing field from a
/// `null` one, or from one that is not a `T`. With `#[serde(default)]`, a missing key is
/// [`Slot::Missing`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Slot<T> {
    /// The answer has no such key.
    #[default]
    Missing,
    /// The key holds `null`.
    Null,
    /// The key holds a `T`.
    Value(T),
    /// The key holds something else.
    Other,
}

impl<T> Slot<T> {
    /// The `T` the key holds, if it holds one.
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Value(value) => Some(value),
            Self::Missing | Self::Null | Self::Other => None,
        }
    }

    /// Whether the key holds anything but `null`, as `answer.get(key).is_some_and(|v|
    /// !v.is_null())` reads it.
    pub fn is_set(&self) -> bool {
        matches!(self, Self::Value(_) | Self::Other)
    }
}

impl<'de, T: DeserializeOwned> Deserialize<'de> for Slot<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Value::deserialize(deserializer)? {
            Value::Null => Self::Null,
            value => T::deserialize(value).map_or(Self::Other, Self::Value),
        })
    }
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

/// A value the API sends as text or as a number, read as text: a string as it is, a number as
/// its digits (exact under `arbitrary_precision`), as `view::text_or_number` reads one. Anything
/// else does not read, so in a [`Slot`] it is [`Slot::Other`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextOrNumber(pub String);

impl<'de> Deserialize<'de> for TextOrNumber {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        text_or_number(deserializer)?
            .map(Self)
            .ok_or_else(|| serde::de::Error::custom("neither text nor a number"))
    }
}

/// Reads a view inside a view: an object as [`view_of`] reads it, and `None` for anything
/// else, as `answer.pointer("/outer/inner")` finds nothing under an outer field that is not an
/// object.
pub(crate) fn nested<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    let value = Value::deserialize(deserializer)?;
    Ok(value.is_object().then(|| view_of(&value)))
}

/// Reads a list of views, each item as [`view_of`] reads it, so an item that is not an object
/// is a view with every field `None`, as iterating `answer["items"]` and reading each with
/// `get` would see it; `None` when the field is not an array.
pub(crate) fn each<'de, D, T>(deserializer: D) -> Result<Option<Vec<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    Ok(match Value::deserialize(deserializer)? {
        Value::Array(items) => Some(items.iter().map(view_of).collect()),
        _ => None,
    })
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

    #[derive(Debug, Default, Deserialize)]
    struct Outer {
        #[serde(default, deserialize_with = "nested")]
        inner: Option<Probe>,
        #[serde(default, deserialize_with = "each")]
        items: Option<Vec<Probe>>,
        #[serde(default)]
        slot: Slot<String>,
    }

    /// An array is never read by position, at the top or inside: `pointer` finds nothing in
    /// one, so neither does a view.
    #[test]
    fn an_array_is_not_read_by_position() {
        let read: Probe = view_of(&json!(["n", 7]));
        assert_eq!((read.name, read.count), (None, None));
        let read: Outer = view_of(&json!({"inner": ["n", 7]}));
        assert!(read.inner.is_none());
        let read: Outer = view_of(&json!({"inner": {"name": "n"}}));
        assert_eq!(
            read.inner.and_then(|inner| inner.name).as_deref(),
            Some("n")
        );
    }

    /// Each item of a list is read on its own, so one that is not an object is a view with
    /// every field `None`, and the list keeps its length; a list that is not an array is `None`.
    #[test]
    fn each_item_of_a_list_is_read_on_its_own() {
        let read: Outer = view_of(&json!({"items": [{"name": "a"}, "b", {"name": 3}]}));
        let names: Vec<Option<String>> = read
            .items
            .unwrap()
            .into_iter()
            .map(|item| item.name)
            .collect();
        assert_eq!(names, [Some("a".to_owned()), None, None]);
        let read: Outer = view_of(&json!({"items": {"name": "a"}}));
        assert!(read.items.is_none());
    }

    /// A slot tells a missing key, `null`, a value of its type, and anything else apart.
    #[test]
    fn a_slot_tells_missing_from_null_from_another_type() {
        let slot = |answer: Value| view_of::<Outer>(&answer).slot;
        assert_eq!(slot(json!({})), Slot::Missing);
        assert_eq!(slot(json!({"slot": null})), Slot::Null);
        assert_eq!(slot(json!({"slot": "x"})), Slot::Value("x".to_owned()));
        assert_eq!(slot(json!({"slot": 5})), Slot::Other);
        assert_eq!(
            slot(json!({"slot": "x"})).value().map(String::as_str),
            Some("x")
        );
        assert_eq!(slot(json!({"slot": 5})).value(), None);
        let set: Vec<bool> = [
            json!({}),
            json!({"slot": null}),
            json!({"slot": "x"}),
            json!({"slot": 5}),
        ]
        .into_iter()
        .map(|answer| slot(answer).is_set())
        .collect();
        assert_eq!(set, [false, false, true, true]);
    }

    /// Text or a number reads as text; anything else does not read.
    #[test]
    fn text_or_a_number_reads_as_text() {
        let read = |answer: Value| view_of::<Amounts>(&answer).cap;
        assert_eq!(
            read(json!({"cap": "1.5"})),
            Slot::Value(TextOrNumber("1.5".to_owned()))
        );
        assert_eq!(
            read(json!({"cap": 20_000_000_000_000_000_000_u128})),
            Slot::Value(TextOrNumber("20000000000000000000".to_owned()))
        );
        assert_eq!(read(json!({"cap": true})), Slot::Other);
        assert_eq!(read(json!({"cap": null})), Slot::Null);
    }

    #[derive(Debug, Default, Deserialize)]
    struct Amounts {
        #[serde(default)]
        cap: Slot<TextOrNumber>,
    }
}
