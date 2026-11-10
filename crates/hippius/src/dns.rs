//! The DNS provider credential that `hipc cdn hostnames dns-setup` sends once, read from a
//! private file.
//!
//! Swagger (`cdn_zones_hostnames_dns-setup_create`) takes the credential in the body, for that
//! request only: the server never stores, logs, or echoes it. A token in argv is visible to
//! every user through `ps` (tenet 9), so it comes only from a file, held to the checks of the
//! config file: a regular file of the current user with no bits for the group or others, not
//! a symlink, at most 1 MiB, in a directory no one else can write.
//!
//! # The file
//!
//! A JSON object with the fields swagger documents for the provider, all strings:
//!
//! - Cloudflare: `api_token`, and optionally `zone_id`.
//! - Route 53: `access_key_id` and `secret_access_key`, and optionally `session_token` and
//!   `hosted_zone_id`.
//!
//! Any other field is refused rather than sent, so a file meant for the other provider, or an
//! AWS credentials file, fails here, and so is a field given twice, which a JSON parser would
//! settle by keeping one silently. No message and no `Debug` output quotes what the file
//! holds, not even a field name it should not have. The values are cleared when dropped, as
//! far as the JSON parser lets (the `kubernetes` module says what that misses).

use std::fmt;
use std::path::Path;

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::{Map, Value};
use zeroize::{Zeroize as _, Zeroizing};

use crate::client::{lowercase_escapes, percent_encode_keeping, token_spellings};
use crate::error::{Error, Result};
use crate::phrase::quote_path;
use crate::private_file::{Guarded, NamedBy, PrivateFile};

/// The DNS providers swagger lists for the setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsProvider {
    /// Cloudflare DNS, reached with an API token.
    Cloudflare,
    /// Amazon Route 53, reached with an AWS access key.
    Route53,
}

impl DnsProvider {
    /// The value the API takes, which is also the value of `--provider`.
    pub fn code(self) -> &'static str {
        match self {
            Self::Cloudflare => "cloudflare",
            Self::Route53 => "route53",
        }
    }

    /// The fields the provider requires, then the ones it takes besides.
    fn fields(self) -> (&'static [&'static str], &'static [&'static str]) {
        match self {
            Self::Cloudflare => (&["api_token"], &["zone_id"]),
            Self::Route53 => (
                &["access_key_id", "secret_access_key"],
                &["session_token", "hosted_zone_id"],
            ),
        }
    }
}

/// A provider credential, loaded from its file. Its `Debug` names the provider only.
pub struct DnsCredentials {
    provider: DnsProvider,
    /// The fields of the file, in the order [`DnsProvider::fields`] lists them.
    fields: Vec<(&'static str, Zeroizing<String>)>,
}

impl fmt::Debug for DnsCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DnsCredentials")
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}

impl DnsCredentials {
    /// Reads the credential for `provider` from the file at `path`, named by `flag`, and checks
    /// it: the file as the config file is checked, then its fields (module docs). Sends
    /// nothing.
    ///
    /// # Errors
    ///
    /// `usage` for a file that is missing, a symlink, not a regular file, another user's, open
    /// to the group or others, larger than 1 MiB, or not a JSON object of the provider's
    /// fields. No message quotes what the file holds.
    pub fn load(path: &Path, flag: &'static str, provider: DnsProvider) -> Result<Self> {
        let guarded = Guarded {
            what: "the DNS credentials file",
            holds: "a DNS provider credential",
            named_by: NamedBy::Flag(flag),
        };
        let refuse = |problem: &str| {
            Error::usage(
                format!("the DNS credentials file {} {problem}", quote_path(path)),
                file_hint(flag, provider),
            )
        };
        let mut file = match load_object(path, guarded, &file_hint(flag, provider))? {
            ObjectFile::Object(file) => file,
            ObjectFile::Repeated => return Err(refuse("names a field twice")),
            ObjectFile::NotAnObject => return Err(refuse("is not a JSON object")),
        };
        let (required, optional) = provider.fields();
        let mut fields = Vec::new();
        for &name in required.iter().chain(optional) {
            match take_text(&mut file, name) {
                Taken::Text(text) => fields.push((name, text)),
                Taken::Missing if optional.contains(&name) => {}
                Taken::Missing => {
                    return Err(refuse(&format!(
                        "has no `{name}`, which `--provider {}` needs",
                        provider.code()
                    )));
                }
                Taken::NotText => {
                    return Err(refuse(&format!(
                        "sets `{name}` to something other than a non-empty string"
                    )));
                }
            }
        }
        // What is left was not asked for. Its name is not quoted: a misplaced secret can sit
        // in a field name as well as in a value. It is cleared on the way out ([`Fields`]).
        if !file.is_empty() {
            return Err(refuse(&format!(
                "has a field that `--provider {}` does not take",
                provider.code()
            )));
        }
        Ok(Self { provider, fields })
    }

    /// The `credentials` object of the request body.
    pub fn body(&self) -> Value {
        Value::Object(
            self.fields
                .iter()
                .map(|(name, text)| ((*name).to_owned(), Value::from(text.as_str())))
                .collect(),
        )
    }

    /// The `credentials` object a dry run shows: the file's field names, each with a
    /// placeholder, so the dry run says which fields go out and holds no value.
    pub fn placeholder_body(&self) -> Value {
        Value::Object(
            self.fields
                .iter()
                .map(|(name, _)| {
                    let placeholder = format!("<the {name} from --credentials-file>");
                    ((*name).to_owned(), Value::from(placeholder))
                })
                .collect(),
        )
    }

    /// The texts to redact from anything an answer or an error repeats, longest first, so a
    /// secret that holds another is redacted whole: each secret field's value in the spellings
    /// the API token is redacted in (as it is, with a backtick as `\u{60}`, and percent-encoded
    /// in either hex case), percent-encoded with `/` kept, in either hex case, and as the body
    /// of a JSON string, once and twice escaped, as an error may quote it. The ids
    /// (`zone_id`, `hosted_zone_id`) are not secrets and are left alone: a short id would
    /// otherwise mangle every answer that happens to contain its characters. They are cleared
    /// when dropped, as `hippius::AppSecrets::redactions` are.
    pub fn redactions(&self) -> Zeroizing<Vec<String>> {
        redactions(
            self.fields
                .iter()
                .filter(|(name, _)| SECRET_FIELDS.contains(name))
                .map(|(_, text)| text.as_str()),
        )
    }
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
/// `repr` write it. An empty spelling is left out, since it cannot be redacted. The
/// temporaries built on the way are cleared when dropped.
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

/// The fields whose values are secrets. `access_key_id` is less secret than its key, but
/// it is half of the pair and has no reason to be printed.
const SECRET_FIELDS: [&str; 4] = [
    "api_token",
    "access_key_id",
    "secret_access_key",
    "session_token",
];

/// A private JSON file a flag names, as [`load_object`] read it.
pub(crate) enum ObjectFile {
    /// The object names a field twice: which copy counts would depend on the parser.
    Repeated,
    /// The file is not one JSON object.
    NotAnObject,
    Object(Fields),
}

/// Reads the private file at `path` ([`PrivateFile::load`], held to its checks) as one JSON
/// object, for the DNS credentials, App Engine secrets, and cluster key files. The bytes read
/// are cleared on the way out, whatever the outcome; each caller words its own refusals of
/// what the file holds.
///
/// # Errors
///
/// The refusals of [`PrivateFile::load`], and `usage` with `hint` when nothing is at the path.
/// That message does not repeat the path: a secret passed in place of the file that holds it
/// names no file, and quoting it would copy the secret into a log (tenet 9).
pub(crate) fn load_object(path: &Path, guarded: Guarded, hint: &str) -> Result<ObjectFile> {
    let Some(bytes) = PrivateFile::load(path, guarded)? else {
        return Err(Error::usage(
            format!(
                "{} {} names does not exist; the path is not repeated here, in case it is a \
                 secret passed in place of the file",
                guarded.what,
                guarded.named_by.name()
            ),
            hint,
        ));
    };
    let bytes = Zeroizing::new(bytes);
    Ok(match serde_json::from_slice::<UniqueObject>(&bytes) {
        Ok(UniqueObject::Object(fields)) => ObjectFile::Object(fields),
        Ok(UniqueObject::Repeated) => ObjectFile::Repeated,
        Err(_) => ObjectFile::NotAnObject,
    })
}

/// The file as a JSON object, or the fact that it names a field twice. Any other JSON value is
/// a deserialization error. What was read is cleared when either is dropped or the read
/// fails: a field named twice and its earlier value included.
pub(crate) enum UniqueObject {
    Object(Fields),
    Repeated,
}

/// The fields of a private file's JSON object, every name and string in them cleared when
/// dropped ([`clear_owned`]), so a refusal that returns before a field is taken out leaves no
/// copy of it in freed memory.
pub(crate) struct Fields(Map<String, Value>);

impl std::ops::Deref for Fields {
    type Target = Map<String, Value>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Fields {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for Fields {
    fn drop(&mut self) {
        clear_owned(Value::Object(std::mem::take(&mut self.0)));
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

impl<'de> Deserialize<'de> for UniqueObject {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct Object;
        impl<'de> Visitor<'de> for Object {
            type Value = UniqueObject;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<UniqueObject, A::Error> {
                // Cleared when dropped, so a syntax error later in the file, which returns
                // here, leaves nothing read before it behind.
                let mut fields = Fields(Map::new());
                let mut repeated = false;
                while let Some((name, value)) = map.next_entry::<String, Value>()? {
                    // The whole object is still read, so a syntax error later in the file
                    // is reported as one. A second copy is not kept: it is cleared now, and
                    // the first when `fields` is dropped.
                    if fields.contains_key(&name) {
                        repeated = true;
                        drop(Zeroizing::new(name));
                        clear_owned(value);
                    } else {
                        fields.insert(name, value);
                    }
                }
                Ok(if repeated {
                    UniqueObject::Repeated
                } else {
                    UniqueObject::Object(fields)
                })
            }
        }
        deserializer.deserialize_map(Object)
    }
}

/// A string field taken out of the file.
enum Taken {
    Text(Zeroizing<String>),
    Missing,
    NotText,
}

/// Takes `name` out of `file`: a non-empty string, kept to be cleared on drop. Any other value
/// is cleared now.
fn take_text(file: &mut Map<String, Value>, name: &str) -> Taken {
    match file.shift_remove(name) {
        None => Taken::Missing,
        Some(Value::String(text)) if !text.is_empty() => Taken::Text(Zeroizing::new(text)),
        Some(other) => {
            clear_owned(other);
            Taken::NotText
        }
    }
}

/// The hint of a refused file: what it must hold.
fn file_hint(flag: &str, provider: DnsProvider) -> String {
    let (required, optional) = provider.fields();
    let list = |names: &[&str]| {
        names
            .iter()
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>()
            .join(" and ")
    };
    format!(
        "pass `{flag}` a mode-0600 JSON file with {}, and optionally {}",
        list(required),
        list(optional)
    )
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

    #[test]
    fn debug_names_the_provider_and_no_value() {
        let credentials = DnsCredentials {
            provider: DnsProvider::Cloudflare,
            fields: vec![("api_token", Zeroizing::new("cf-secret".to_owned()))],
        };
        let shown = format!("{credentials:?}");
        assert!(shown.contains("Cloudflare"), "{shown}");
        assert!(!shown.contains("cf-secret"), "{shown}");
    }

    /// Only the secret fields are redacted, longest first, each also as a JSON string body and
    /// percent-encoded.
    #[test]
    fn redactions_cover_the_secret_fields_in_every_form_longest_first() {
        let credentials = DnsCredentials {
            provider: DnsProvider::Route53,
            fields: vec![
                ("access_key_id", Zeroizing::new("AK".to_owned())),
                ("secret_access_key", Zeroizing::new("s/\"`1".to_owned())),
                ("hosted_zone_id", Zeroizing::new("Z".to_owned())),
            ],
        };
        assert_eq!(
            *credentials.redactions(),
            [
                "s%2F%22%601",
                "s%2f%22%601",
                "s/\"\\u{60}1",
                "s/%22%601",
                "s/\\\\\\\"`1",
                "s/\\\"`1",
                "s/\"`1",
                "AK"
            ]
        );
        assert_eq!(
            credentials.placeholder_body(),
            serde_json::json!({
                "access_key_id": "<the access_key_id from --credentials-file>",
                "secret_access_key": "<the secret_access_key from --credentials-file>",
                "hosted_zone_id": "<the hosted_zone_id from --credentials-file>",
            })
        );
    }

    #[test]
    fn the_hint_lists_the_fields_of_the_provider() {
        assert_eq!(
            file_hint("--credentials-file", DnsProvider::Route53),
            "pass `--credentials-file` a mode-0600 JSON file with `access_key_id` and \
             `secret_access_key`, and optionally `session_token` and `hosted_zone_id`"
        );
    }
}
