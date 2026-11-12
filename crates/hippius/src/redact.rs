//! Keeping secrets out of what `hipc` prints: the spellings a secret may come back in, and the
//! redaction of those spellings from text and from JSON values (tenet 9). This is the one
//! redactor; every caller, in either crate, goes through it.
//!
//! The rules, whatever the secret:
//!
//! - A secret is redacted in every spelling an answer or an error may repeat it in
//!   ([`redactions`], [`token_spellings`]), longest first.
//! - Text is redacted in one pass ([`redact_all`]): every match of every spelling is found
//!   first, and each span of matches that overlap becomes one `<redacted>`, so a short secret
//!   never cuts a longer one or a marker.
//! - A field name or a number is the server's, not free text: it is replaced whole, and only
//!   when it may be the secret ([`redact_name`], [`number_holds`]), so a short secret does not
//!   turn every name and count into markers. Two names that redact alike are both kept
//!   ([`insert_unique`]).

use serde_json::{Map, Value};
use zeroize::{Zeroize as _, Zeroizing};

use crate::encode::{lowercase_escapes, percent_encode, percent_encode_keeping};

/// `value` with every field named `name` removed, at any depth of objects and arrays, and
/// every other field kept: for a one-time secret that only the call issuing it may print
/// (tenet 9), and for the server's error body, where a secret has no reason to be
/// (`Client::send`). Removed at every level, not only the top, so a response that nests
/// the object, or lists several, cannot print it if its shape changes.
pub fn without_field(value: Value, name: &str) -> Value {
    without_fields(value, &|key| key == name)
}

/// `value` with every field whose name `withheld` picks removed, at any depth of objects and
/// arrays, as [`without_field`] removes one name: for a secret that more than one spelling
/// may name.
pub fn without_fields(value: Value, withheld: &dyn Fn(&str) -> bool) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .filter(|(key, _)| !withheld(key))
                .map(|(key, value)| (key, without_fields(value, withheld)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| without_fields(item, withheld))
                .collect(),
        ),
        other => other,
    }
}

/// How the name of a field that holds a secret in a Hippius answer ends, folded as
/// [`is_secret_field`] folds a name: `client_secret`, `robot_secret`, and `secret` all end in
/// `secret`, and `netbird_setup_key` in `setupkey`. A name that ends in `token` is a secret
/// too unless it is the bare word, which [`is_secret_field`] says why.
const SECRET_FIELD_ENDINGS: [&str; 11] = [
    "secret",
    "password",
    "passphrase",
    "credentials",
    "ticket",
    "joincommand",
    "setupkey",
    "secretkey",
    "privatekey",
    "accesskey",
    "apikey",
];

/// Whether a field named `name` holds a secret (tenet 9), in any spelling the server may
/// give it: letter case, `_`, `-`, and camelCase are folded away, so `authToken`,
/// `Auth-Token`, and `auth_token` are one name, as `AccessKey` and `access_key` are. A name
/// is a secret when it ends as one of [`SECRET_FIELD_ENDINGS`] does, or ends in `token` after
/// another word. A bare `token` is deliberately kept: the API keys its validation errors by
/// field name (`{"token": ["Invalid invite."]}`), and dropping it would hide why an invite
/// was refused; a route whose answer holds a real `token` needs `--reveal`. Matching the
/// ending fails closed for a new spelling such as `user_password`, at the cost of dropping a
/// name that only ends like a secret.
pub(crate) fn is_secret_field(name: &str) -> bool {
    let folded: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    SECRET_FIELD_ENDINGS
        .iter()
        .any(|ending| folded.ends_with(ending))
        || (folded.len() > "token".len() && folded.ends_with("token"))
}

/// What [`redact_value`] writes in place of the secret.
const REDACTED: &str = "<redacted>";

/// `text` with every occurrence of each of `secrets` replaced by `<redacted>`. Every place any
/// secret matches is found first, overlapping matches included, and matches that overlap are
/// one span with one marker: so neither secret of two that overlap in the text shows a part of
/// itself, a shorter secret is never replaced inside a longer one that holds it, and none is
/// replaced inside a `<redacted>` another wrote, as a pass per secret would do for a secret
/// such as `d`. With one secret whose matches cannot overlap it is [`str::replace`]. The text
/// that held a secret is cleared, not left in freed memory.
///
/// # Panics
///
/// When a secret is empty, which would match between every two characters.
pub fn redact_all<S: AsRef<str>>(text: String, secrets: &[S]) -> String {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for secret in secrets {
        let secret = secret.as_ref();
        assert!(!secret.is_empty(), "an empty secret cannot be redacted");
        let mut from = 0;
        while let Some(at) = text[from..].find(secret) {
            let start = from + at;
            spans.push((start, start + secret.len()));
            // The next match may start inside this one: one character on.
            from = start + text[start..].chars().next().map_or(1, char::len_utf8);
        }
    }
    if spans.is_empty() {
        return text;
    }
    spans.sort_unstable();
    let mut out = String::with_capacity(text.len());
    let mut written = 0;
    let mut spans = spans.into_iter().peekable();
    while let Some((start, mut end)) = spans.next() {
        // Every later span that starts before this one ends is part of it.
        while let Some(&(next, next_end)) = spans.peek() {
            if next >= end {
                break;
            }
            end = end.max(next_end);
            spans.next();
        }
        out.push_str(&text[written..start]);
        out.push_str(REDACTED);
        written = end;
    }
    out.push_str(&text[written..]);
    drop(Zeroizing::new(text));
    out
}

/// `value` with `secret` redacted at any depth: for a secret the caller sent, such as an invite
/// token, which an answer has no reason to repeat but must never print if it does (tenet 9).
/// Every occurrence of it in a string is replaced by `<redacted>` ([`redact_all`]). A field
/// name or a number is replaced whole, by `<redacted>`, when it may be the secret
/// ([`redact_name`], `number_holds`): a name or a number is the server's, not free text, so
/// a short secret does not turn every name and count it occurs in into markers. Two names
/// that redact to the same text are both kept ([`insert_unique`]).
///
/// A number becomes the string `<redacted>`, so a redacted copy can hold a string where the
/// answer held a number.
///
/// # Panics
///
/// When `secret` is empty, which would match between every two characters.
pub fn redact_value(value: Value, secret: &str) -> Value {
    redact_value_all(value, &[secret])
}

/// [`redact_value`] of each of `secrets` at once: every string in one [`redact_all`] pass, and
/// a field name or a number that may be any of them.
///
/// # Panics
///
/// When a secret is empty, which would match between every two characters.
pub fn redact_value_all<S: AsRef<str>>(value: Value, secrets: &[S]) -> Value {
    // Checked here too, so a value with no string still refuses an empty secret.
    assert!(
        secrets.iter().all(|secret| !secret.as_ref().is_empty()),
        "an empty secret cannot be redacted"
    );
    redacted_value(value, secrets)
}

/// [`redact_value_all`] once its secrets are checked.
fn redacted_value<S: AsRef<str>>(value: Value, secrets: &[S]) -> Value {
    match value {
        Value::String(text) => Value::String(redact_all(text, secrets)),
        Value::Object(fields) => {
            let mut redacted = Map::new();
            for (key, value) in fields {
                let key = redact_name(key, secrets);
                insert_unique(&mut redacted, &key, redacted_value(value, secrets));
            }
            Value::Object(redacted)
        }
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| redacted_value(item, secrets))
                .collect(),
        ),
        Value::Number(number)
            if secrets
                .iter()
                .any(|secret| number_holds(&number.to_string(), secret.as_ref())) =>
        {
            Value::from(REDACTED)
        }
        other => other,
    }
}

/// Inserts `value` under `key`, or under `key (2)`, `key (3)`, and so on when a redacted key
/// already took it, so no field is lost when two names redact to the same text.
pub fn insert_unique(fields: &mut Map<String, Value>, key: &str, value: Value) {
    let mut name = key.to_owned();
    let mut n = 2;
    while fields.contains_key(&name) {
        name = format!("{key} ({n})");
        n += 1;
    }
    fields.insert(name, value);
}

/// How long a secret must be, in bytes, before a field name, a number, or a text of a checked
/// shape that only holds it is redacted; below it, only one that is the secret is. Arbitrary:
/// eight characters are rare by chance in a field name, a count, an id, or a size, and a
/// token that short is weak anyway.
const LONG_SECRET: usize = 8;

/// Whether `text`, a field name or a text whose shape the caller checked (an id, a status
/// word), may be one of `secrets`, so that it is printed only as `<redacted>`: it is one of
/// them, or holds one of eight bytes or more.
pub fn holds_secret<S: AsRef<str>>(text: &str, secrets: &[S]) -> bool {
    secrets
        .iter()
        .map(AsRef::as_ref)
        .any(|secret| secret == text || (secret.len() >= LONG_SECRET && text.contains(secret)))
}

/// The field name `name` as a redacted copy prints it: `<redacted>` when it may be one of
/// `secrets` ([`holds_secret`]), else as it came. The name replaced is cleared.
pub fn redact_name<S: AsRef<str>>(name: String, secrets: &[S]) -> String {
    if holds_secret(&name, secrets) {
        drop(Zeroizing::new(name));
        REDACTED.to_owned()
    } else {
        name
    }
}

/// Whether the JSON number `text` may be the all-digit `secret`, as an all-digit secret echoed
/// unquoted would be: the rule for a number in any redacted copy of an answer. Both are
/// compared as decimal values: the number without its sign, with its exponent applied, and
/// without leading zeros before its point or trailing zeros after it (`-12345.0` and
/// `1.2345e4` are `12345`, and `10` stays `10`), and the secret without its leading zeros. A
/// number whose digits only hold the secret's may be it once the secret has [`LONG_SECRET`]
/// digits or more.
pub(crate) fn number_holds(text: &str, secret: &str) -> bool {
    if secret.is_empty() || !secret.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let secret = canonical_digits(secret);
    let mantissa = text.split(['e', 'E']).next().unwrap_or(text);
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    decimal(text).as_deref() == Some(secret)
        || (secret.len() >= LONG_SECRET && digits.contains(secret))
}

/// `digits` without leading zeros, or `0` when nothing else is left.
fn canonical_digits(digits: &str) -> &str {
    match digits.trim_start_matches('0') {
        "" => "0",
        trimmed => trimmed,
    }
}

/// The most zeros an exponent may add to a number [`decimal`] writes out: a secret is far
/// shorter, so a longer number cannot be one. Arbitrary, and bounds the allocation.
const MAX_EXPONENT_ZEROS: usize = 1024;

/// The JSON number `text` as a plain decimal, without its sign, with its exponent applied, and
/// without leading zeros before the point or trailing zeros after it: `-1.50e2` is `150`,
/// `0.0120` is `0.012`, and `0` is `0`. `None` for text that is not a JSON number, or one whose
/// exponent would add more than [`MAX_EXPONENT_ZEROS`] zeros.
fn decimal(text: &str) -> Option<String> {
    let text = text.strip_prefix('-').unwrap_or(text);
    let (mantissa, exponent) = match text.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i64>().ok()?),
        None => (text, 0),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{whole}{fraction}");
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Where the point falls in `digits`, once the exponent moves it.
    let point = i64::try_from(whole.len()).ok()?.checked_add(exponent)?;
    let len = i64::try_from(digits.len()).ok()?;
    let (whole, fraction) = if point <= 0 {
        let zeros = usize::try_from(-point).ok()?;
        if zeros > MAX_EXPONENT_ZEROS {
            return None;
        }
        (String::new(), format!("{}{digits}", "0".repeat(zeros)))
    } else if point >= len {
        let zeros = usize::try_from(point - len).ok()?;
        if zeros > MAX_EXPONENT_ZEROS {
            return None;
        }
        (format!("{digits}{}", "0".repeat(zeros)), String::new())
    } else {
        let at = usize::try_from(point).ok()?;
        (digits[..at].to_owned(), digits[at..].to_owned())
    };
    let whole = canonical_digits(&whole);
    Some(match fraction.trim_end_matches('0') {
        "" => whole.to_owned(),
        fraction => format!("{whole}.{fraction}"),
    })
}

/// The ways an error can spell `token`: as it is, as [`crate::phrase::quote`] writes
/// it in a message (a backtick as `\u{60}`), and percent-encoded with uppercase or
/// lowercase hex, as a URL in a redirect or a body may carry it. The longest come first, so
/// no shorter spelling is replaced inside a longer one before the longer one is.
pub(crate) fn token_spellings(token: &str) -> Vec<String> {
    let encoded = percent_encode(token);
    longest_first(vec![
        token.to_owned(),
        token.replace('`', r"\u{60}"),
        lowercase_escapes(&encoded),
        encoded,
    ])
}

/// The texts to redact for `secrets`, longest first, for [`crate::redact_all`]: each in every
/// spelling an answer or an error may repeat it in, such as percent-encoded in a URL or
/// escaped in a JSON string. An empty secret redacts nothing. They are cleared when dropped.
pub fn redactions<'a>(secrets: impl IntoIterator<Item = &'a str>) -> Zeroizing<Vec<String>> {
    Zeroizing::new(longest_first(
        secrets.into_iter().flat_map(secret_spellings).collect(),
    ))
}

/// Every spelling of `text` an answer or an error may repeat it in, for a redaction: the
/// spellings the API token is redacted in (as it is, with a backtick as `\u{60}`, and
/// percent-encoded in either hex case), percent-encoded with `/` kept, in either hex case, as
/// the body of a JSON string, once and twice escaped and with every non-ASCII character as
/// `\uXXXX` (Python's `json.dumps`), and as `phrase::quote`, Rust's `{:?}`, and Python's
/// `repr` write it. An empty spelling is left out, since it cannot be redacted. Clearing the
/// copies is best effort: the spellings returned are cleared by the caller's `Zeroizing`, but
/// a `String` that grows while one is built (the escapes, `phrase::quote`) frees each smaller
/// buffer as it is.
pub(crate) fn secret_spellings(text: &str) -> Vec<String> {
    let mut spellings = token_spellings(text);
    let escaped = json_body(text);
    spellings.push(json_body(&escaped));
    spellings.push(ascii_escapes(&escaped, false));
    spellings.push(ascii_escapes(&escaped, true));
    spellings.push(escaped);
    // Python's `urllib.parse.quote` keeps `/` by default, so a server written in it encodes
    // the rest and leaves the slashes.
    let quoted = percent_encode_keeping_slash(text);
    spellings.push(lowercase_escapes(&quoted));
    spellings.push(quoted);
    // How `phrase::quote` writes it inside a message: a control character as a Rust escape
    // (`\u{1b}`) and a backtick as `\u{60}`, so a message that quotes the value is redacted too.
    let in_quote = Zeroizing::new(crate::phrase::quote(text));
    spellings.push(in_quote[1..in_quote.len() - 1].to_owned());
    // As Rust's `{:?}` (`\0`) and Python's `repr` (`\x00`) write it, without the quotes, as a
    // server or a message may quote it.
    let debug = Zeroizing::new(format!("{text:?}"));
    spellings.push(debug[1..debug.len() - 1].to_owned());
    spellings.push(python_escapes(text));
    spellings.retain(|spelling| !spelling.is_empty());
    spellings
}

/// `text` with the escapes of Python's `repr`, without the quotes: `\\`, `\n`, `\r`, `\t`,
/// and every character Python does not print as `\xNN`, `\uNNNN`, or `\UNNNNNNNN`.
fn python_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let code = u32::from(c);
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if python_prints(c) => out.push(c),
            _ if code < 0x100 => out.push_str(&format!("\\x{code:02x}")),
            _ if code < 0x1_0000 => out.push_str(&format!("\\u{code:04x}")),
            _ => out.push_str(&format!("\\U{code:08x}")),
        }
    }
    out
}

/// Whether Python's `str.isprintable` holds for `c`, as far as a short table can say: not a
/// control (Cc), a format character (Cf), a separator other than the space (Zs, Zl, Zp), or a
/// private-use character (Co). Unassigned characters, which Python also escapes, are not
/// known here and pass as printable.
fn python_prints(c: char) -> bool {
    !(c.is_control()
        || matches!(
            c,
            '\u{a0}'
                | '\u{ad}'
                | '\u{600}'..='\u{605}'
                | '\u{61c}'
                | '\u{6dd}'
                | '\u{70f}'
                | '\u{1680}'
                | '\u{180e}'
                | '\u{2000}'..='\u{200f}'
                | '\u{2028}'..='\u{202f}'
                | '\u{205f}'..='\u{2064}'
                | '\u{2066}'..='\u{206f}'
                | '\u{3000}'
                | '\u{e000}'..='\u{f8ff}'
                | '\u{feff}'
                | '\u{fff9}'..='\u{fffb}'
                | '\u{e0001}'
                | '\u{e0020}'..='\u{e007f}'
                | '\u{f0000}'..='\u{10ffff}'
        ))
}

/// A JSON string body with every non-ASCII character as `\uXXXX`, a pair of surrogates
/// beyond the first plane: in lowercase hex as Python's `json.dumps` writes it by default, or
/// in uppercase (`\uD83D`) as other encoders do.
fn ascii_escapes(body: &str, upper: bool) -> String {
    let mut out = String::with_capacity(body.len());
    for c in body.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            for unit in c.encode_utf16(&mut [0; 2]) {
                if upper {
                    out.push_str(&format!("\\u{unit:04X}"));
                } else {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out
}

/// `texts` longest first, each once, so a secret that holds another is redacted whole before
/// the shorter one could cut it. A copy dropped as a duplicate is cleared first.
pub(crate) fn longest_first(mut texts: Vec<String>) -> Vec<String> {
    texts.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    texts.dedup_by(|later, earlier| {
        let same = later == earlier;
        if same {
            later.zeroize();
        }
        same
    });
    texts
}

/// `text` as the body of a JSON string: escaped, without the quotes. The quoted copy is
/// cleared when dropped.
fn json_body(text: &str) -> String {
    let quoted = Zeroizing::new(Value::from(text).to_string());
    quoted[1..quoted.len() - 1].to_owned()
}

/// `text` percent-encoded outside the unreserved set of RFC 3986 and `/`, with uppercase hex,
/// as Python's `urllib.parse.quote` writes it.
fn percent_encode_keeping_slash(text: &str) -> String {
    percent_encode_keeping(text, b"/")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A field's name is a secret's in any spelling: letter case, `_`, `-`, and camelCase
    /// fold away, and a name that ends as a secret's does is one, a bare `token` excepted.
    #[test]
    fn a_secret_field_is_known_by_how_its_name_ends_in_any_spelling() {
        for name in [
            "secret",
            "robot_secret",
            "Client-Secret",
            "authToken",
            "AUTH_TOKEN",
            "access_token",
            "refreshToken",
            "APIKey",
            "AccessKey",
            "aws_secret_access_key",
            "netbird_setup_key",
            "newPassword",
            "credentials",
            "ticket",
            "join_command",
            "passphrase",
            "private-key",
        ] {
            assert!(is_secret_field(name), "{name}");
        }
        for name in [
            "token",
            "Token",
            "key",
            "access_key_id",
            "secret_name",
            "password_set",
            "id",
            "",
        ] {
            assert!(!is_secret_field(name), "{name}");
        }
    }

    /// All secrets at once, whatever order they come in: a short secret cuts neither a longer
    /// one that holds it nor a marker already written.
    #[test]
    fn several_secrets_are_redacted_at_once() {
        let secrets = ["a", "dacted"];
        assert_eq!(
            redact_all("dacted a b".to_owned(), &secrets),
            "<redacted> <redacted> b"
        );
        assert_eq!(redact_all("ééa".to_owned(), &secrets), "éé<redacted>");
        // Two secrets that start at the same place: the longer is taken whole.
        assert_eq!(
            redact_all("tok-1 tok".to_owned(), &["tok", "tok-1"]),
            "<redacted> <redacted>"
        );
        // Two secrets that overlap in the text are one span, so neither shows a part of
        // itself, whichever starts first.
        assert_eq!(
            redact_all("<xaBCDEFG>".to_owned(), &["xa", "aBCDEFG"]),
            "<<redacted>>"
        );
        assert_eq!(
            redact_all("aBCxyz".to_owned(), &["Cxyz", "aBC"]),
            "<redacted>"
        );
        // A secret that overlaps itself is one span too; apart, each match is its own.
        assert_eq!(
            redact_all("aaa b aa".to_owned(), &["aa"]),
            "<redacted> b <redacted>"
        );
        assert_eq!(
            redact_all("abab".to_owned(), &["ab"]),
            "<redacted><redacted>"
        );
        // Two field names that redact to the same text are both kept.
        assert_eq!(
            redact_value_all(json!({"acted": 1, "d": 2}), &["acted", "d"]),
            json!({"<redacted>": 1, "<redacted> (2)": 2})
        );
        assert_eq!(redact_all("x".to_owned(), &[] as &[&str]), "x");
        // A pass per secret would cut the marker the first one wrote.
        assert_eq!(
            redact_all("tok-1".to_owned(), &["tok-1", "d"]),
            "<redacted>"
        );
        let value = json!({"tok-1 d": ["tok-1", 12_345_678], "n": 7});
        assert_eq!(
            redact_value_all(value, &["tok-1", "d", "12345678"]),
            json!({"tok-1 d": ["<redacted>", "<redacted>"], "n": 7})
        );
    }

    /// A field name is the server's, so it is replaced whole, and only when it may be a
    /// secret: when it is one, or holds one of eight bytes or more. A name that holds only a
    /// short secret is kept, as a number that holds only a short one is.
    #[test]
    fn a_field_name_is_redacted_whole_when_it_may_be_a_secret() {
        let secrets = ["a", "tok-1234"];
        let value = json!({
            "a": 1,
            "name": 2,
            "x tok-1234 y": 3,
            "tok-123": 4,
            "nested": [{"tok-1234": 5}],
        });
        assert_eq!(
            redact_value_all(value, &secrets),
            json!({
                "<redacted>": 1,
                "name": 2,
                "<redacted> (2)": 3,
                "tok-123": 4,
                "nested": [{"<redacted>": 5}],
            })
        );
        assert_eq!(redact_name("name".to_owned(), &secrets), "name");
        assert_eq!(redact_name("a".to_owned(), &secrets), "<redacted>");
        assert!(holds_secret("xtok-1234", &secrets));
        assert!(!holds_secret("tok-123", &secrets));
        assert!(!holds_secret("x", &[] as &[&str]));
    }

    #[test]
    #[should_panic(expected = "an empty secret cannot be redacted")]
    fn an_empty_secret_among_several_panics() {
        drop(redact_all("x".to_owned(), &["a", ""]));
    }

    #[test]
    fn a_secret_is_redacted_in_every_string_and_field_name() {
        let value = json!({
            "token": "tok-1",
            "error": "invite tok-1 is spent",
            "nested": [{"tok-1": 1, "n": 2}, "x tok-1 tok-1"],
            "ok": true,
            "none": null
        });
        assert_eq!(
            redact_value(value, "tok-1"),
            json!({
                "token": "<redacted>",
                "error": "invite <redacted> is spent",
                "nested": [{"<redacted>": 1, "n": 2}, "x <redacted> <redacted>"],
                "ok": true,
                "none": null
            })
        );
        assert_eq!(
            redact_value(json!(12), "1"),
            json!(12),
            "a number that only holds the secret's digits is kept"
        );
        assert_eq!(
            redact_value(json!({"n": [12345]}), "12345"),
            json!({"n": ["<redacted>"]}),
            "a number that is the secret is redacted"
        );
        let spelled: Value =
            serde_json::from_str("[-12345.0, 12345e0, 1.2345e4, 0.0012345, 123456]").unwrap();
        assert_eq!(
            redact_value(spelled, "12345"),
            json!([
                "<redacted>",
                "<redacted>",
                "<redacted>",
                0.001_234_5,
                123_456
            ]),
            "a sign, a zero tail, or an exponent do not hide the secret; another value that \
             holds a short one is kept"
        );
        let long: Value = serde_json::from_str("[9867530912345, 1.86753091]").unwrap();
        assert_eq!(
            redact_value(long, "86753091"),
            json!(["<redacted>", "<redacted>"]),
            "a secret of eight digits or more is redacted inside any number"
        );
        assert_eq!(
            redact_value(json!(12_345_678), "abcdefgh"),
            json!(12_345_678)
        );
    }

    /// A number is the secret only when their values are equal: zeros that change the value
    /// are kept, so a secret `1000` is not the count `1` or `10`.
    #[test]
    fn a_number_is_compared_with_a_secret_by_its_value() {
        for (number, secret) in [
            ("1000", "1000"),
            ("1000.0", "1000"),
            ("1e3", "1000"),
            ("1E+3", "01000"),
            ("100000e-2", "1000"),
            ("-1000", "1000"),
            ("0", "0"),
            ("0.0", "00"),
            ("0e5", "0"),
        ] {
            assert!(number_holds(number, secret), "{number} {secret}");
        }
        for (number, secret) in [
            ("1", "1000"),
            ("10", "1000"),
            ("0.01", "1000"),
            ("10", "1"),
            ("0", "1000"),
            ("1.5", "15"),
            ("1e-2000", "0"),
            ("1e2000", "1"),
        ] {
            assert!(!number_holds(number, secret), "{number} {secret}");
        }
        assert_eq!(decimal("-1.50e2").as_deref(), Some("150"));
        assert_eq!(decimal("0.0120").as_deref(), Some("0.012"));
        assert_eq!(decimal("12.5e-3").as_deref(), Some("0.0125"));
        assert_eq!(decimal("x"), None);
        assert_eq!(decimal("1e"), None);
    }

    #[test]
    #[should_panic(expected = "an empty secret cannot be redacted")]
    fn an_empty_secret_is_a_bug() {
        drop(redact_value(json!(1), ""));
    }

    #[test]
    fn the_spellings_include_the_rust_and_python_escapes() {
        assert_eq!(
            python_escapes("a\\b\n\r\t\u{1b}\u{7f}é\u{85}\u{a0}\u{ad}\u{200b}\u{e0041}\u{f0001}"),
            r"a\\b\n\r\t\x1b\x7fé\x85\xa0\xad\u200b\U000e0041\U000f0001"
        );
        assert_eq!(ascii_escapes("é\u{1f600}x", false), r"\u00e9\ud83d\ude00x");
        assert!(secret_spellings("\u{1f600}").contains(&r"\uD83D\uDE00".to_owned()));
        assert!(secret_spellings("é\u{1b}").contains(&r"\u00e9\u001b".to_owned()));
        assert_eq!(
            longest_first(vec!["a".into(), "bb".into(), "a".into()]),
            ["bb", "a"]
        );
        let spellings = secret_spellings("x\u{0}`");
        for spelling in [r"x\u{0}\u{60}", r"x\0`", r"x\x00`", r"x\u0000`"] {
            assert!(spellings.contains(&spelling.to_owned()), "{spelling}");
        }
    }
}
