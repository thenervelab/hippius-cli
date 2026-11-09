//! Base-unit amounts shown in credits, in text mode only.
//!
//! The compute billing reads send amounts in credit base units (swagger: "10^18 = 1
//! credit"), beside a balance in credits, and 1 credit is 1 USD. A person reading
//! `burn_rate_per_hour  1500000000000000000` next to `balance  12.50` cannot compare them,
//! so text mode shows each amount the command names in credits, exactly (a decimal shift,
//! never a float), and a `note:` says which fields it changed. The JSON document keeps
//! every amount as the server sent it (tenets 7 and 14).
//!
//! Only fields swagger or the console documents as base units are named. The summary's
//! `charge_cap` object is left as sent, because neither documents its fields, and a ratio
//! read as an amount would print a wrong number.

use std::cmp::Ordering;

use hippius::phrase;
use serde_json::Value;

use crate::context::Context;
use crate::output::Mode;

/// Base units per credit, as a power of ten: swagger's "10^18 = 1 credit", and the
/// `decimals` the console reads from the same answers.
pub(crate) const DECIMALS: u32 = 18;

/// Where an amount sits in a document: object keys, with `*` for every element of an
/// array.
pub(crate) type AmountPath = &'static [&'static str];

/// `document` as text mode shows it: every integer string at one of `amounts` written in
/// credits, and a note naming the fields that changed. In JSON mode, `document` unchanged.
///
/// A document whose `decimals` is not 18 is left as sent, with no note: its amounts are
/// not in the base units swagger documents, and dividing by the wrong power would print a
/// wrong price. A missing `decimals` is read as swagger's 18.
pub(crate) fn shown_in_credits(
    ctx: &Context<'_>,
    mut document: Value,
    amounts: &[AmountPath],
) -> Value {
    if matches!(ctx.mode(), Mode::Json) {
        return document;
    }
    let decimals = document.get("decimals");
    if decimals.is_some_and(|d| d.as_u64() != Some(u64::from(DECIMALS))) {
        return document;
    }
    let mut changed: Vec<String> = Vec::new();
    for path in amounts {
        // An empty path names no field, so no note names it.
        if convert(&mut document, path)
            && let Some(field) = path.last()
        {
            let name = phrase::quote(field);
            if !changed.contains(&name) {
                changed.push(name);
            }
        }
    }
    if !changed.is_empty() {
        let verb = if changed.len() == 1 { "is" } else { "are" };
        ctx.note(&format!(
            "{} {verb} in credits here (1 credit = 1 USD); the JSON output keeps base units \
             (10^18 per credit)",
            phrase::join(&changed, "and")
        ));
    }
    document
}

/// Writes every integer string at `path` in `value` in credits, and says whether one was.
fn convert(value: &mut Value, path: &[&str]) -> bool {
    let Some((first, rest)) = path.split_first() else {
        let Value::String(text) = value else {
            return false;
        };
        let Some(credits) = to_credits(text) else {
            return false;
        };
        *text = credits;
        return true;
    };
    match (*first, value) {
        ("*", Value::Array(items)) => items
            .iter_mut()
            .fold(false, |any, item| convert(item, rest) | any),
        (key, Value::Object(map)) => map.get_mut(key).is_some_and(|v| convert(v, rest)),
        _ => false,
    }
}

/// `base_units`, an integer in decimal digits with an optional `-`, as credits: the digits
/// shifted [`DECIMALS`] places, with no trailing zeros after the point and no point for a
/// whole number. `None` for any other text, which is left as sent.
pub(crate) fn to_credits(base_units: &str) -> Option<String> {
    let (sign, digits) = match base_units.strip_prefix('-') {
        Some(digits) => ("-", digits),
        None => ("", base_units),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let digits = digits.trim_start_matches('0');
    // `-0` is zero, which has no sign.
    let sign = if digits.is_empty() { "" } else { sign };
    let padded = format!("{digits:0>width$}", width = DECIMALS as usize + 1);
    let (whole, fraction) = padded.split_at(padded.len() - DECIMALS as usize);
    let fraction = fraction.trim_end_matches('0');
    Some(if fraction.is_empty() {
        format!("{sign}{whole}")
    } else {
        format!("{sign}{whole}.{fraction}")
    })
}

/// Compares two credit amounts exactly, as decimal digits, so `50`, `50.0`, and `050` are the
/// same cap and no float rounding decides whether a change is a raise. `None` when `old`, the
/// server's cap, is not a decimal amount; `new` passed a credits flag's parser.
pub(crate) fn compare(new: &str, old: &str) -> Option<Ordering> {
    let (new, old) = (amount(new)?, amount(old)?);
    Some(
        new.0
            .len()
            .cmp(&old.0.len())
            .then_with(|| new.0.cmp(old.0))
            .then_with(|| new.1.cmp(old.1)),
    )
}

/// A decimal amount as its whole digits without leading zeros and its fraction digits without
/// trailing zeros, or `None` for anything else.
pub(crate) fn amount(text: &str) -> Option<(&str, &str)> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
    let point_without_fraction = text.contains('.') && fraction.is_empty();
    (!whole.is_empty() && !point_without_fraction && digits(whole) && digits(fraction)).then(|| {
        (
            whole.trim_start_matches('0'),
            fraction.trim_end_matches('0'),
        )
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn base_units_become_credits_exactly() {
        for (base_units, credits) in [
            ("0", "0"),
            ("-0", "0"),
            ("000", "0"),
            ("1", "0.000000000000000001"),
            ("1000000000000000000", "1"),
            ("1500000000000000000", "1.5"),
            ("131506849315068492", "0.131506849315068492"),
            ("123456789012345678901234", "123456.789012345678901234"),
            ("-2500000000000000000", "-2.5"),
            ("0012000000000000000000", "12"),
        ] {
            assert_eq!(
                to_credits(base_units).as_deref(),
                Some(credits),
                "{base_units}"
            );
        }
        for not_an_integer in ["", "-", "1.5", "12.50", " 1", "1e18", "+1", "--1", "0x10"] {
            assert_eq!(to_credits(not_an_integer), None, "{not_an_integer:?}");
        }
    }

    /// Against integer arithmetic, over values that cross every digit boundary and the
    /// `u128` range: the whole part is the quotient, the fraction the remainder.
    #[test]
    fn credits_agree_with_integer_division() {
        let scale = 10_u128.pow(DECIMALS);
        let mut values = vec![u128::MAX, u128::MAX - 1, scale - 1, scale, scale + 1];
        for power in 0..39 {
            let base = 10_u128.pow(power);
            values.extend([base, base.saturating_mul(7).saturating_add(3), base - 1]);
        }
        for value in values {
            let credits = to_credits(&value.to_string()).expect("digits are an integer");
            let (whole, fraction) = credits.split_once('.').unwrap_or((&credits, ""));
            assert_eq!(whole, (value / scale).to_string(), "{value}");
            let remainder = format!("{:0>18}", value % scale);
            assert_eq!(fraction, remainder.trim_end_matches('0'), "{value}");
        }
    }

    #[test]
    fn only_the_named_paths_change() {
        let mut doc = json!({
            "total": "1000000000000000000",
            "count": "5",
            "hours": [
                {"total": "2", "lines": [{"amount": "3", "quantity": "4"}]},
                {"total": 7},
                "x"
            ],
            "nested": {"amount": "9"}
        });
        assert!(convert(&mut doc, &["total"]));
        assert!(convert(&mut doc, &["hours", "*", "total"]));
        assert!(convert(&mut doc, &["hours", "*", "lines", "*", "amount"]));
        assert!(!convert(&mut doc, &["missing"]));
        assert!(!convert(&mut doc, &["total", "*"]));
        assert_eq!(
            doc,
            json!({
                "total": "1",
                "count": "5",
                "hours": [
                    {"total": "0.000000000000000002", "lines": [
                        {"amount": "0.000000000000000003", "quantity": "4"}
                    ]},
                    {"total": 7},
                    "x"
                ],
                "nested": {"amount": "9"}
            })
        );
    }
}
