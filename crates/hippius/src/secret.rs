//! Clearing JSON values that hold secrets, so a value read from a private file or sent once
//! leaves no copy in freed memory (tenet 8: best effort, as far as `serde_json` lets).

use std::fmt;

use serde_json::Value;
use zeroize::Zeroizing;

/// A JSON value that holds secrets, such as a request body built from a secrets file or an
/// answer that carries a one-time secret: every string and field name in it is cleared when it
/// is dropped, so no caller has to remember to. `Debug` shows none of it.
pub struct SecretValue(Value);

impl SecretValue {
    /// Wraps `value`, which is cleared when the wrapper is dropped.
    pub fn new(value: Value) -> Self {
        Self(value)
    }

    /// The value, to read or to serialize.
    pub fn as_value(&self) -> &Value {
        &self.0
    }

    /// The value, to change in place: whatever it holds when dropped is cleared.
    pub fn as_value_mut(&mut self) -> &mut Value {
        &mut self.0
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        clear_owned(std::mem::take(&mut self.0));
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretValue").finish_non_exhaustive()
    }
}

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

    /// A secret value changes in place, and `Debug` shows nothing of it.
    #[test]
    fn a_secret_value_changes_in_place_and_shows_nothing() {
        let mut body =
            SecretValue::new(serde_json::json!({"provider": "cloudflare", "api_token": "cf"}));
        body.as_value_mut()["provider"] = Value::from("route53");
        assert_eq!(body.as_value()["provider"], "route53");
        let shown = format!("{body:?}");
        assert_eq!(shown, "SecretValue { .. }");
    }
}
