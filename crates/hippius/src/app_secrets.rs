//! The secret values `hipc compute app-engine apps create` sends once, read from a private
//! file.
//!
//! Swagger (`compute_app-engine_apps_create`) takes `secrets`, a map of name to string, in the
//! body. The server stores the values encrypted, hands each to the VM as
//! `/run/secrets/<NAME>`, and never shows one again: every answer lists `secret_names` only.
//! A value in argv is visible to every user through `ps` (tenet 9), so the values come only
//! from a file, held to the checks of the config file: a regular file of the current user
//! with no bits for the group or others, not a symlink, at most 1 MiB, in a directory no one
//! else can write. There is no stdin form: stdin cannot be checked that way.
//!
//! # The file
//!
//! One JSON object of name to string, the swagger shape. A value is sent exactly as the file
//! holds it: whitespace counts, and nothing is trimmed. A name given twice is refused, which a
//! JSON parser would settle by keeping one silently, and so is a value that is not a string.
//!
//! A message or a dry run names a secret only when the name is one the server takes (see
//! [`is_secret_name`]), so a value pasted where a name goes is not printed. No `Debug` output
//! holds a value, and the values are cleared when dropped, as far as the JSON parser lets
//! (the `kubernetes` module says what that misses).

use std::fmt;
use std::path::Path;

use serde_json::{Map, Value};
use zeroize::{Zeroize as _, Zeroizing};

use crate::error::{Error, Result};
use crate::phrase::quote_path;
use crate::private_file::{Guarded, NamedBy};
use crate::private_object::load_private_object;
use crate::secret::clear_owned;

/// The secrets of an app, loaded from their file. Its `Debug` counts the secrets and shows
/// neither a name nor a value.
pub struct AppSecrets {
    /// The flag that named the file, for the placeholders and the hints.
    flag: &'static str,
    /// The secrets in the order the file lists them.
    secrets: Vec<(String, Zeroizing<String>)>,
}

impl fmt::Debug for AppSecrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppSecrets")
            .field("count", &self.secrets.len())
            .finish_non_exhaustive()
    }
}

impl AppSecrets {
    /// Reads the secrets from the file at `path`, named by `flag`, and checks it: the file as
    /// the config file is checked, then its shape (module docs). Sends nothing.
    ///
    /// # Errors
    ///
    /// `usage` for a file that is missing, a symlink, not a regular file, another user's, open
    /// to the group or others, in a directory others can write, larger than 1 MiB, or not one
    /// JSON object of name to string. No message quotes a value.
    pub fn load(path: &Path, flag: &'static str) -> Result<Self> {
        let guarded = Guarded {
            what: "the secrets file",
            holds: "secret values",
            named_by: NamedBy::Flag(flag),
        };
        let refuse = |problem: &str| {
            Error::usage(
                format!("the secrets file {} {problem}", quote_path(path)),
                file_hint(flag),
            )
        };
        let mut file = load_private_object(path, guarded, &file_hint(flag))?
            .map_err(|problem| refuse(&problem.describe("a secret")))?;
        let mut secrets = Vec::with_capacity(file.len());
        let mut not_text = None;
        for (name, value) in std::mem::take(&mut *file) {
            match value {
                Value::String(text) => secrets.push((name, Zeroizing::new(text))),
                // Every string is still taken, so each is cleared on the way out, and so is
                // anything else.
                other => {
                    clear_owned(other);
                    not_text = not_text.or(Some(name));
                }
            }
        }
        if let Some(name) = not_text {
            return Err(refuse(&format!(
                "sets {} to something other than a string",
                quote_name(&name)
            )));
        }
        Ok(Self { flag, secrets })
    }

    /// Whether the file holds no secret: `{}`.
    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty()
    }

    /// The names of the secrets, in the file's order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.secrets.iter().map(|(name, _)| name.as_str())
    }

    /// The `secrets` object of the request body.
    pub fn body(&self) -> Value {
        Value::Object(
            self.secrets
                .iter()
                .map(|(name, text)| (name.clone(), Value::from(text.as_str())))
                .collect(),
        )
    }

    /// The `secrets` object a dry run shows: each name with a placeholder, so the dry run says
    /// which secrets go out and holds no value. A name the server would not take is described
    /// by its place in the file instead, since it may be a value pasted in the wrong place.
    pub fn placeholder_body(&self) -> Value {
        let flag = self.flag;
        Value::Object(
            self.secrets
                .iter()
                .enumerate()
                .map(|(index, (name, _))| {
                    if is_secret_name(name) {
                        (
                            name.clone(),
                            Value::from(format!("<the {name} from {flag}>")),
                        )
                    } else {
                        let place = index + 1;
                        (
                            format!("<the name of secret {place} in {flag}, not printed>"),
                            Value::from(format!("<the value of secret {place} in {flag}>")),
                        )
                    }
                })
                .collect::<Map<_, _>>(),
        )
    }

    /// The texts to redact from anything an answer or an error repeats, longest first: every
    /// value in each spelling the DNS credential is redacted in
    /// (`hippius::DnsCredentials::redactions`). An empty value redacts nothing. They are
    /// cleared when dropped.
    pub fn redactions(&self) -> Zeroizing<Vec<String>> {
        crate::redact::redactions(self.secrets.iter().map(|(_, text)| text.as_str()))
    }
}

/// Clears every string in `value`, at any depth, in place: for a request body built from
/// [`AppSecrets::body`] once it is serialized, so its copies of the values are not left in
/// freed memory. The serialized bytes the request holds are not cleared.
pub fn clear_strings(value: &mut Value) {
    match value {
        Value::String(text) => text.zeroize(),
        Value::Array(items) => items.iter_mut().for_each(clear_strings),
        Value::Object(fields) => fields.values_mut().for_each(clear_strings),
        _ => {}
    }
}

/// Whether `name` is a name the server takes for a secret: an uppercase ASCII letter, then
/// uppercase letters, digits, and `_` (the server's secret-name rule, `^[A-Z][A-Z0-9_]*$`),
/// at most 128 bytes.
pub fn is_secret_name(name: &str) -> bool {
    name.len() <= 128
        && name.as_bytes().first().is_some_and(u8::is_ascii_uppercase)
        && name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

/// `name` in backticks when it is a secret's name ([`is_secret_name`]). Anything else may be
/// a value pasted in the wrong place, so it is described, not quoted.
pub(crate) fn quote_name(name: &str) -> String {
    if is_secret_name(name) {
        format!("`{name}`")
    } else {
        "a secret whose name hipc does not print".to_owned()
    }
}

/// The hint of a refused file: what it must hold.
fn file_hint(flag: &str) -> String {
    format!("pass `{flag}` a mode-0600 JSON object of each secret's name to its value")
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    use std::path::PathBuf;

    use super::*;

    const FLAG: &str = "--secrets-file";

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hipc-secrets-{name}-{}", std::process::id()));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    fn file(dir: &Path, text: &str) -> PathBuf {
        let path = dir.join("secrets.json");
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        path
    }

    fn refused(text: &str) -> Error {
        let dir = dir("refused");
        let error = AppSecrets::load(&file(&dir, text), FLAG).unwrap_err();
        std::fs::remove_dir_all(&dir).unwrap();
        error
    }

    /// Values are kept exactly, whitespace and all, in the file's order, and the placeholders
    /// and `Debug` name the secrets only.
    #[test]
    fn values_are_kept_exactly_and_shown_nowhere() {
        let dir = dir("exact");
        let text = r#"{"B_KEY":" padded \n","A_KEY":"","C":"x\u001b\u0000","bad key":"y"}"#;
        let path = file(&dir, text);
        let secrets = AppSecrets::load(&path, FLAG).unwrap();
        assert!(!secrets.is_empty());
        assert_eq!(
            secrets.names().collect::<Vec<_>>(),
            ["B_KEY", "A_KEY", "C", "bad key"]
        );
        assert_eq!(
            secrets.body(),
            serde_json::json!({"B_KEY": " padded \n", "A_KEY": "", "C": "x\u{1b}\u{0}",
                               "bad key": "y"})
        );
        // A name the server would not take is described by its place, not printed.
        assert_eq!(
            secrets.placeholder_body(),
            serde_json::json!({
                "B_KEY": "<the B_KEY from --secrets-file>",
                "A_KEY": "<the A_KEY from --secrets-file>",
                "C": "<the C from --secrets-file>",
                "<the name of secret 4 in --secrets-file, not printed>":
                    "<the value of secret 4 in --secrets-file>",
            })
        );
        let shown = format!("{secrets:?}");
        assert!(
            shown.contains("count: 4") && !shown.contains("B_KEY"),
            "{shown}"
        );
        // The empty value redacts nothing; the others come in every spelling, longest first.
        let redactions = secrets.redactions();
        assert!(redactions.iter().all(|text| !text.is_empty()));
        assert!(
            redactions.contains(&" padded \\n".to_owned()),
            "{redactions:?}"
        );
        assert!(
            redactions.contains(&"%20padded%20%0A".to_owned()),
            "{redactions:?}"
        );
        // `phrase::quote` writes control characters as Rust escapes.
        let texts: &Vec<String> = &redactions;
        assert!(texts.contains(&"x\\u{1b}\\u{0}".to_owned()), "{texts:?}");
        assert!(redactions.windows(2).all(|w| w[0].len() >= w[1].len()));
        let empty = AppSecrets::load(&file(&dir, "{}"), FLAG).unwrap();
        assert!(empty.is_empty());
        assert_eq!(*empty.redactions(), Vec::<String>::new());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_of_another_shape_is_refused_without_its_values() {
        for (text, says) in [
            ("[]", "is not a JSON object"),
            ("not json", "is not a JSON object"),
            (r#"{"A":"one","A":"two"}"#, "names a secret twice"),
            (
                r#"{"A":"s3cr3t","N":1}"#,
                "sets `N` to something other than a string",
            ),
            (
                r#"{"A":"s3cr3t","port":1}"#,
                "sets a secret whose name hipc does not print to something other",
            ),
            (
                r#"{"s3cr3t value":null}"#,
                "sets a secret whose name hipc does not print to something other",
            ),
        ] {
            let error = refused(text);
            assert_eq!(error.code(), crate::Code::Usage, "{text}");
            let message = error.message();
            assert!(message.contains(says), "{text}: {message}");
            assert!(!message.contains("s3cr3t"), "{message}");
            let hint = error.hint().unwrap();
            assert!(
                hint.starts_with("pass `--secrets-file` a mode-0600 JSON"),
                "{hint}"
            );
        }
    }

    #[test]
    fn a_missing_or_open_file_is_refused() {
        let dir = dir("guard");
        // A secret passed in place of the path names no file, and is not repeated.
        let missing = AppSecrets::load(Path::new("hunter2"), FLAG).unwrap_err();
        assert_eq!(
            missing.message(),
            "the secrets file `--secrets-file` names does not exist; the path is not repeated \
             here, in case it is a secret passed in place of the file"
        );
        assert_eq!(missing.hint(), Some(file_hint(FLAG).as_str()));
        let path = file(&dir, "{}");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let open = AppSecrets::load(&path, FLAG).unwrap_err();
        let message = open.message();
        assert!(
            message.contains("permission bits for the group"),
            "{message}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clear_strings_empties_every_string_at_any_depth() {
        let mut body = serde_json::json!({"a": "x", "b": ["y", {"c": "z"}], "n": 1});
        clear_strings(&mut body);
        assert_eq!(
            body,
            serde_json::json!({"a": "", "b": ["", {"c": ""}], "n": 1})
        );
    }

    #[test]
    fn only_a_plain_name_is_quoted() {
        assert_eq!(quote_name("DB_PASSWORD"), "`DB_PASSWORD`");
        assert_eq!(quote_name("A2_"), "`A2_`");
        assert_eq!(
            quote_name(&"A".repeat(128)),
            format!("`{}`", "A".repeat(128))
        );
        for name in [
            "",
            "db2",
            "2A",
            "_A",
            "A-B",
            "with space",
            "a`b",
            &"A".repeat(129),
        ] {
            assert_eq!(quote_name(name), "a secret whose name hipc does not print");
        }
    }
}
