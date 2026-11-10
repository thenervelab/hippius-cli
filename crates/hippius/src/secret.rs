//! Clearing JSON values that hold secrets, so a value read from a private file or sent once
//! leaves no copy in freed memory (tenet 8: best effort, as far as `serde_json` lets).

use serde_json::Value;
use zeroize::Zeroizing;

/// Drops `value` with every string in it and every field name cleared, at any depth, for a
/// value read from a private file that is not kept. A number is not cleared: `serde_json`
/// gives no way to reach its digits.
pub(crate) fn clear_owned(value: Value) {
    match value {
        Value::String(text) => drop(Zeroizing::new(text)),
        Value::Array(items) => items.into_iter().for_each(clear_owned),
        Value::Object(fields) => {
            for (name, value) in fields {
                drop(Zeroizing::new(name));
                clear_owned(value);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every string, at any depth, and every name is dropped through `Zeroizing`; numbers and
    /// the rest are dropped as they are.
    #[test]
    fn a_value_of_any_shape_is_dropped_cleared() {
        clear_owned(serde_json::json!({"a": [1, "s", {"k": "v"}, null, true], "b": 2.5}));
    }
}
