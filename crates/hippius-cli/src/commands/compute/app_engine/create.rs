//! `hipc compute app-engine apps create`: `POST validate/`, then `POST apps/`, then a wait.
//!
//! The order (tenet 5): argv; the compose file; the secrets file, loaded and checked; the dry
//! run, which sends nothing and shows a placeholder for each value; the token; `POST
//! validate/` with the compose alone, which changes nothing; the local checks of its answer
//! (the hash, and that the secrets given are the ones the compose declares); the question;
//! the launch. So a refused compose, a wrong hash, or a missing or undeclared secret stops
//! before money is spent and before any value leaves the machine.
//!
//! A secret value goes in the launch's body and nowhere else. The dry run holds placeholders,
//! and `--debug` prints no body (it never does). Every decision hipc makes on an answer (the
//! hash, the id, the status, the error code) reads the answer as it came; only what is printed
//! is redacted ([`printable`]), so a short value such as `a` cannot change an id or a status.
//! In what is printed, every value, in each spelling the DNS credential is redacted in, is
//! redacted from free text, and the fields hipc checked by their shape (an id, a hash, a
//! status, a kebab code) are printed as they came. An error of the launch keeps only the
//! fields of the server's body that say what to fix ([`kept_fields`]), and its message is
//! built from those alone ([`sanitized`]). The server says it never repeats a value, so this
//! is a second line of defence.

use hippius::routes::compute::{self, Vm};
use hippius::{AppSecrets, Error, Request, Result, SecretValue, Slot, phrase, view_of};
use serde_json::{Map, Value};

use super::{APPS, Compose, Hashed, check_hash, read_compose, unavailable_hint};
use crate::cli::{AppCreate, DiskTier, FailoverMode, GuestPoweroff};
use crate::commands::Printed;
use crate::commands::compute::launch::{
    Kind, STAYS_STOPPED, backups_clause, balance_hint, launch_wait, os_disk_clause, wait,
};
use crate::context::{Context, Write, WritePlan};

/// The flag that names the secrets file, in its placeholders and hints.
const SECRETS_FLAG: &str = "--secrets-file";

pub(super) fn create(ctx: &Context<'_>, args: &AppCreate) -> Result<Printed> {
    let timeout = launch_wait(ctx, &args.launch);
    let compose = read_compose(&args.compose.path)?;
    let secrets = args
        .secrets_file
        .as_deref()
        .map(|path| AppSecrets::load(path, SECRETS_FLAG))
        .transpose()?;
    // `{}` sends no `secrets` at all, as a launch without the flag does.
    let secrets = secrets.filter(|secrets| !secrets.is_empty());
    compose.warn_of_invisible_bytes(ctx);
    let client = ctx.load_authed_client()?;
    let action = launch_action(args);
    let launch = |secrets: Option<&Value>| {
        Request::to(&compute::APP_ENGINE_APP_CREATE, [])
            .json(&create_body(args, &compose, secrets))
            .about(&action)
            .listed_by(APPS)
    };
    let validate = compose.request(&compute::APP_ENGINE_VALIDATE, "validate");
    let placeholders = secrets.as_ref().map(AppSecrets::placeholder_body);
    let draft = launch(placeholders.as_ref());
    // Not `read_then_write`: the secret values go into a body only once the launch is
    // confirmed, and its answer is checked against the compose after it is sent.
    let checked = match ctx.check_write(&client, &WritePlan::new(&[&validate], &draft))? {
        Write::DryRun(doc) => return Ok(doc.into()),
        Write::Checked(checked) => checked,
    };
    let parsed = ctx
        .send(&client, &validate)
        .and_then(|response| {
            let parsed = response.document()?;
            check_hash(
                &response,
                &parsed,
                &compose.sha256,
                Hashed::Sent(&compose.name),
            )?;
            Ok(parsed)
        })
        .map_err(unavailable_hint)?;
    check_secret_names(&parsed, secrets.as_ref(), &compose)?;
    let question = format!("{action}, billed per hour from the balance");
    let confirmed = ctx.confirm_write(checked, &question, None)?;
    let redactions = secrets.as_ref().map(AppSecrets::redactions);
    let redactions: &[String] = redactions.as_ref().map_or(&[], |texts| texts.as_slice());
    // The body's copies of the values are cleared once it is serialized, the file's with
    // `secrets`, and the request's bytes when it is dropped.
    let values = secrets.as_ref().map(AppSecrets::body);
    let request = launch(values.as_ref().map(SecretValue::as_value));
    drop(values);
    drop(secrets);
    let response = ctx
        .send_confirmed(&client, &request, &confirmed)
        .map_err(|error| sanitized(balance_hint(unavailable_hint(error)), &action, redactions))?;
    let launched = response
        .document()
        .map_err(|error| sanitized(error, &action, redactions))?;
    // The 202 is the app, with the compose facts; when it names a hash, it must be the file's,
    // or the launch measures other bytes. The app exists by then, so it is printed.
    let app: Vm = view_of(&launched);
    match app.compose_sha256 {
        Slot::Missing | Slot::Null => {}
        Slot::Value(hash) if hash.eq_ignore_ascii_case(&compose.sha256) => {}
        Slot::Value(_) | Slot::Other => {
            let why = format!(
                "the API hashed other bytes than {}: the `compose_sha256` of its launch answer is \
                 not `{}`, the SHA-256 hipc computed",
                compose.name, compose.sha256
            );
            return Err(sanitized(response.refused(&why), &action, redactions)
                .after_write(printable(&launched, redactions)));
        }
    }
    // An answer without a `vm_id` names no app to follow or delete, whether or not hipc
    // waits, so both are internal, with the answer printed so what it holds is not lost.
    if app.vm_id.is_none() {
        return Err(Error::internal("the launch answer has no `vm_id`")
            .with_hint(format!("run `{APPS}` to find the new app"))
            .after_write(printable(&launched, redactions)));
    }
    match timeout {
        Some(timeout) => wait(ctx, &client, launched, Kind::App { redactions }, timeout),
        None => Ok(Printed::Document(printable(&launched, redactions))),
    }
}

/// What the launch does, for the question and the refusal: the flavor, the address every app
/// gets, and the options that bill more.
fn launch_action(args: &AppCreate) -> String {
    let mut action = format!(
        "launch an app VM of flavor {} with a public IPv4",
        phrase::quote(&args.flavor)
    );
    if let Some(region) = &args.region {
        action.push_str(&format!(" in `{region}`"));
    }
    let mut more = Vec::new();
    if let Some(tier) = args.launch.os_disk_tier {
        more.push(os_disk_clause(tier));
    }
    if let Some(interval) = args.launch.backup_interval {
        more.push(backups_clause(interval, args.launch.failover));
    }
    if args.launch.on_guest_poweroff == Some(GuestPoweroff::Stop) {
        more.push(STAYS_STOPPED.to_owned());
    }
    if !more.is_empty() {
        action.push_str(&format!(", with {}", more.join(", and with ")));
    }
    action
}

/// The launch body ([`compute::AppCreateBody`]), with `secrets` when there are any: the
/// values, or their placeholders in a dry run.
fn create_body<'a>(
    args: &'a AppCreate,
    compose: &'a Compose,
    secrets: Option<&'a Value>,
) -> compute::AppCreateBody<'a> {
    compute::AppCreateBody {
        flavor: &args.flavor,
        compose: &compose.text,
        secrets,
        name: args.name.as_deref(),
        region: args.region.as_deref(),
        os_disk_tier: args.launch.os_disk_tier.map(DiskTier::code),
        placement_group: args.launch.placement_group.as_deref(),
        on_guest_poweroff: args.launch.on_guest_poweroff.map(GuestPoweroff::code),
        failover_mode: args.launch.failover.map(FailoverMode::code),
        backup_interval_s: args.launch.backup_interval,
    }
}

/// Refuses a launch whose secrets are not the ones the compose declares, as validate read
/// them (`secret_names`): a declared name with no value (which the server would refuse with
/// `app-secret-missing`, after the question), or a value for a name the compose does not
/// declare, which the console's launch drops without a word and hipc does not send. An answer whose
/// `secret_names` is missing, null, or not a list of strings is internal, since nothing could
/// then be checked. A name is quoted only when the compose declares it: one it does not may
/// be a value pasted in the wrong place.
fn check_secret_names(
    parsed: &Value,
    secrets: Option<&AppSecrets>,
    compose: &Compose,
) -> Result<()> {
    let names = view_of::<compute::Validated>(parsed).secret_names;
    let declared: Vec<&str> = match &names {
        Slot::Value(names) => names.iter().map(String::as_str).collect(),
        missing @ (Slot::Missing | Slot::Null | Slot::Other) => {
            let answered = if missing.is_set() {
                "a `secret_names` that is not a list of names"
            } else {
                "without `secret_names`"
            };
            return Err(Error::internal(format!(
                "cannot check the declared secrets: the API's check of {} answered {answered}",
                compose.name
            )));
        }
    };
    let given: Vec<&str> = secrets.map(|s| s.names().collect()).unwrap_or_default();
    if let Some(name) = declared.iter().find(|name| !given.contains(name)) {
        let name = phrase::quote(name);
        let hint = if secrets.is_some() {
            format!("add {name} and its value to the file `{SECRETS_FLAG}` names")
        } else {
            format!(
                "pass `{SECRETS_FLAG}` a mode-0600 JSON object with a value for each secret the \
                 compose declares"
            )
        };
        return Err(Error::usage(
            format!(
                "{} declares the secret {name}, and `{SECRETS_FLAG}` gives it no value",
                compose.name
            ),
            hint,
        ));
    }
    let undeclared = given.iter().filter(|name| !declared.contains(name)).count();
    if undeclared > 0 {
        let declares = if declared.is_empty() {
            "none".to_owned()
        } else {
            declared
                .iter()
                .map(phrase::quote)
                .collect::<Vec<_>>()
                .join(", ")
        };
        return Err(Error::usage(
            format!(
                "the secrets file gives {} that {} does not declare",
                phrase::count(undeclared as u64, "secret"),
                compose.name
            ),
            format!(
                "remove from the secrets file every name the compose does not declare under \
                 `x-hippius.secrets`; it declares {declares}"
            ),
        ));
    }
    Ok(())
}

/// `text`, which [`shaped`] or an error field's own check passed, as printed: as it came, or
/// `<redacted>` whole when it may be a value sent ([`hippius::holds_secret`]): a shape such as
/// 40 hex digits fits a key as well as an id, so the shape alone is no proof that it is not
/// a value sent.
fn checked_text(text: String, redactions: &[String]) -> Value {
    if hippius::holds_secret(&text, redactions) {
        Value::from("<redacted>")
    } else {
        Value::String(text)
    }
}

/// The copy of an answer that is printed. Every decision reads the answer as it came; this
/// copy is only shown. A top-level field hipc checked by its shape ([`shaped`]), and a name in
/// the top-level `secret_names`, is printed as it came unless it may be a value sent
/// ([`checked_text`]). Everything else is redacted as [`hippius::redact_value_all`] redacts
/// it. With no secrets sent the answer is printed as it came.
pub(in crate::commands::compute) fn printable(value: &Value, redactions: &[String]) -> Value {
    if redactions.is_empty() {
        return value.clone();
    }
    let Value::Object(fields) = value else {
        return hippius::redact_value_all(value.clone(), redactions);
    };
    let mut out = Map::new();
    for (name, item) in fields {
        let item = match item {
            Value::String(text) if shaped(name, text) => checked_text(text.clone(), redactions),
            Value::Array(names) if name == "secret_names" => names
                .iter()
                .map(|item| match item {
                    Value::String(text) if hippius::is_secret_name(text) => {
                        checked_text(text.clone(), redactions)
                    }
                    other => hippius::redact_value_all(other.clone(), redactions),
                })
                .collect(),
            other => hippius::redact_value_all(other.clone(), redactions),
        };
        let name = hippius::redact_name(name.clone(), redactions);
        hippius::insert_unique(&mut out, &name, item);
    }
    Value::Object(out)
}

/// The top-level fields of an app answer ([`compute::Vm`]) that [`printable`] shows as they
/// came, each with the one shape it is read as. A field the API adds is not here, so it is
/// redacted like free text: adding one is a review of why its shape cannot hold a value sent.
const SHAPED: [(&str, Fits); 6] = [
    // The id the poll, the note, and the hint name, held to the rule of the argument.
    ("vm_id", is_vm_id),
    // What the launch is measured by, compared with the hash of the file sent.
    ("compose_sha256", is_sha256),
    // Status words the wait reads.
    ("status", is_token),
    ("boot_phase", is_token),
    ("power_state", is_token),
    // The slug of a failure, which a person searches for.
    ("error", is_kebab),
];

/// Whether a field's text has the one shape [`SHAPED`] reads it as.
type Fits = fn(&str) -> bool;

/// Whether `text`, under the top-level `key`, has the one shape [`SHAPED`] reads that key as.
fn shaped(key: &str, text: &str) -> bool {
    SHAPED
        .iter()
        .any(|(field, fits)| *field == key && fits(text))
}

/// Whether `text` is a `vm_id` by the rule of the argument.
fn is_vm_id(text: &str) -> bool {
    crate::cli::VmId::parse(text).is_ok()
}

/// Whether `text` is a SHA-256 of 64 hex digits.
fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether `text` is a status word such as `running` or `pulling_images`: 1 to 64 lowercase
/// ASCII letters, digits, `_`, and `-`.
fn is_token(text: &str) -> bool {
    (1..=64).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// A failure of a request sent after the secrets: the launch, or a poll of the app. With no
/// secrets sent it is unchanged. Otherwise, of the server's body, only [`kept_fields`] stays;
/// a request id stays only as an id that holds no value ([`hippius::holds_secret`]); and the message
/// of any status outside 2xx (a redirect, whose target may hold anything, or a refusal) is
/// built from those fields alone, never from the server's own text, so a field that was
/// dropped cannot reach it. A failure with no status, or a 2xx hipc could not use, keeps a
/// message of hipc's own, with every value redacted from it all the same.
pub(in crate::commands::compute) fn sanitized(
    error: Error,
    action: &str,
    redactions: &[String],
) -> Error {
    if redactions.is_empty() {
        return error;
    }
    let kept = error
        .details()
        .cloned()
        .and_then(|details| kept_fields(details, redactions));
    let message = match error.http_status() {
        Some(status) if !(200..300).contains(&status) => {
            refusal_message(action, status, kept.as_ref().unwrap_or(&Value::Null))
        }
        _ => hippius::redact_all(error.message().to_owned(), redactions),
    };
    error
        .with_details_kept(|_| kept)
        .with_request_id_kept(|id| {
            (is_request_id(&id) && !hippius::holds_secret(&id, redactions)).then_some(id)
        })
        .with_message(message)
}

/// The message of a failure from what [`kept_fields`] kept: a redirect says so without its
/// target; a refusal gives the server's kebab `error` or `code`, else its redacted `detail`, else only
/// the status.
fn refusal_message(action: &str, status: u16, kept: &Value) -> String {
    if (300..400).contains(&status) {
        return format!(
            "cannot {action}: the API answered with a redirect (status `{status}`); its target \
             is not shown, since it may repeat a secret"
        );
    }
    let says = |key| kept.get(key).and_then(Value::as_str);
    match (says("error").or_else(|| says("code")), says("detail")) {
        (Some(code), _) => format!("cannot {action}: the API says {}", phrase::quote(code)),
        (None, Some(detail)) => format!("cannot {action}: the API says {}", phrase::quote(detail)),
        (None, None) => format!(
            "cannot {action}: the API answered with status `{status}`; its words are not \
             shown, since they may repeat a secret"
        ),
    }
}

/// Whether a request id is an id: 1 to 128 ASCII letters, digits, `-`, and `_`.
fn is_request_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The fields of an error body kept when the request carried secrets: `error` or `code` when it is a
/// kebab code ([`checked_text`]), `detail` and `source` as redacted text, `line` as a number,
/// and in `issues`, each one's `rule` (a kebab code), `line` and `column`, and its redacted
/// `message` and `hint`. A number is redacted as [`hippius::redact_value_all`] does. A body that is not an
/// object keeps nothing.
fn kept_fields(details: Value, redactions: &[String]) -> Option<Value> {
    let Value::Object(fields) = details else {
        return None;
    };
    let mut kept = Map::new();
    for (key, value) in fields {
        let value = match (key.as_str(), value) {
            // Compute error bodies carry their slug in `error` or `code` (`refused_as`).
            ("error" | "code", Value::String(code)) if is_kebab(&code) => {
                checked_text(code, redactions)
            }
            ("detail" | "source", Value::String(text)) => {
                Value::String(hippius::redact_all(text, redactions))
            }
            ("line", line) if line.is_u64() => hippius::redact_value_all(line, redactions),
            ("issues", Value::Array(issues)) => issues
                .into_iter()
                .filter_map(|issue| kept_issue(issue, redactions))
                .collect(),
            _ => continue,
        };
        kept.insert(key, value);
    }
    Some(Value::Object(kept))
}

/// The fields of one issue that [`kept_fields`] keeps, or `None` for an issue that is not an
/// object.
fn kept_issue(issue: Value, redactions: &[String]) -> Option<Value> {
    let Value::Object(fields) = issue else {
        return None;
    };
    let mut kept = Map::new();
    for (key, value) in fields {
        let value = match (key.as_str(), value) {
            ("rule", Value::String(code)) if is_kebab(&code) => checked_text(code, redactions),
            ("line" | "column", number) if number.is_u64() => {
                hippius::redact_value_all(number, redactions)
            }
            ("message" | "hint", Value::String(text)) => {
                Value::String(hippius::redact_all(text, redactions))
            }
            _ => continue,
        };
        kept.insert(key, value);
    }
    Some(Value::Object(kept))
}

/// Whether `text` is a kebab code such as `app-secret-missing`: lowercase ASCII words of
/// letters and digits joined by single `-`.
fn is_kebab(text: &str) -> bool {
    !text.is_empty()
        && text.split('-').all(|word| {
            !word.is_empty()
                && word
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_kebab_code_is_lowercase_words_joined_by_single_hyphens() {
        for code in ["app-secret-missing", "a", "x9-y", "app2"] {
            assert!(is_kebab(code), "{code}");
        }
        for code in ["", "-a", "a-", "a--b", "App-x", "a_b", "a b", "a-é"] {
            assert!(!is_kebab(code), "{code}");
        }
    }

    /// Only the fields that say what to fix stay, each of the type it must have.
    #[test]
    fn an_error_body_keeps_only_its_documented_fields() {
        let secret = ["s3cr3t".to_owned()];
        let body = json!({
            "error": "app-secrets-invalid",
            "detail": "bad s3cr3t",
            "line": 3,
            "source": "secrets",
            "echo": "anything",
            "issues": [
                {"rule": "secret-name", "line": 1, "column": 2, "message": "m s3cr3t",
                 "hint": "h", "value": "v"},
                {"rule": "Not Kebab", "line": "1", "column": -1, "message": 5},
                "not an object",
            ],
        });
        assert_eq!(
            kept_fields(body, &secret),
            Some(json!({
                "error": "app-secrets-invalid",
                "detail": "bad <redacted>",
                "line": 3,
                "source": "secrets",
                "issues": [
                    {"rule": "secret-name", "line": 1, "column": 2, "message": "m <redacted>",
                     "hint": "h"},
                    {},
                ],
            }))
        );
        let odd = json!({"error": "Some Words", "detail": 1, "line": -2, "source": null,
                         "issues": "x"});
        assert_eq!(kept_fields(odd, &secret), Some(json!({})));
        assert_eq!(kept_fields(json!("text"), &secret), None);
        assert_eq!(kept_fields(json!([]), &secret), None);
    }

    /// The printed copy keeps a top-level field hipc checked by its shape and redacts the rest,
    /// a one-character value included; a key or a number is redacted only when it may be a
    /// value.
    #[test]
    fn the_printed_copy_redacts_free_text_and_keeps_checked_shapes() {
        let short = ["a".to_owned(), "n".to_owned(), "-".to_owned()];
        let app = json!({
            "vm_id": "vm-1a2b",
            "compose_sha256": "ab".repeat(32),
            "status": "running",
            "boot_phase": "pulling-images",
            "error": "app-not-ready",
            "secret_names": ["API_KEY", "a-b", 3],
            "name": "a-n",
            "line": 7,
            "a": {"rule": "no-tag", "vm_id": "vm-1a2b", "on": true},
            "-": 1,
        });
        assert_eq!(
            printable(&app, &short),
            json!({
                "vm_id": "vm-1a2b",
                "compose_sha256": "ab".repeat(32),
                "status": "running",
                "boot_phase": "pulling-images",
                "error": "app-not-ready",
                "secret_names": ["API_KEY", "<redacted><redacted>b", 3],
                "name": "<redacted><redacted><redacted>",
                "line": 7,
                // Below the top level nothing is taken for its shape.
                "<redacted>": {
                    "rule": "<redacted>o<redacted>t<redacted>g",
                    "vm_id": "vm<redacted>1<redacted>2b",
                    "on": true,
                },
                "<redacted> (2)": 1,
            })
        );
        assert_eq!(printable(&app, &[]), app);
        assert_eq!(printable(&json!(["a"]), &short), json!(["<redacted>"]));
        for (key, text) in [
            ("compose_sha256", "ab"),
            ("compose_sha256", &"zz".repeat(32)),
            ("status", "Running"),
            ("status", ""),
            ("error", "Not-Kebab"),
            ("rule", "no-tag"),
            ("other", "x"),
        ] {
            assert!(!shaped(key, text), "{key} {text}");
        }
        assert!(is_request_id("req_01-AB") && !is_request_id("a b") && !is_request_id(""));
    }

    /// A value that fits a field's shape is not printed in it: a 40-digit hex key as a
    /// `vm_id`, a kebab value as `error`, a name, or any field holding one of 8 bytes or more.
    /// A number that spells an all-digit value, or holds one of 8 digits or more, is redacted.
    #[test]
    fn a_value_that_fits_a_shape_is_redacted_whole() {
        let key = "0123456789abcdef0123456789abcdef01234567".to_owned();
        let secrets = [
            key.clone(),
            "use-me".to_owned(),
            "SECRET_NAME".to_owned(),
            "4242".to_owned(),
            "12345678".to_owned(),
        ];
        let app = json!({
            "vm_id": key,
            "status": format!("x{key}"),
            "error": "use-me",
            "boot_phase": "use-me-not",
            "secret_names": ["SECRET_NAME", "OTHER"],
            "count": 4242,
            "size": 9_123_456_789_u64,
            "line": 42,
        });
        assert_eq!(
            printable(&app, &secrets),
            json!({
                "vm_id": "<redacted>",
                "status": "<redacted>",
                "error": "<redacted>",
                "boot_phase": "use-me-not",
                "secret_names": ["<redacted>", "OTHER"],
                "count": "<redacted>",
                "size": "<redacted>",
                "line": 42,
            })
        );
        let body = json!({
            "error": "use-me",
            "line": 4242,
            "issues": [{"rule": "use-me", "line": 12_345_678, "column": 1}],
        });
        assert_eq!(
            kept_fields(body, &secrets),
            Some(json!({
                "error": "<redacted>",
                "line": "<redacted>",
                "issues": [{"rule": "<redacted>", "line": "<redacted>", "column": 1}],
            }))
        );
    }

    /// A refusal's message comes from the kept code, else the kept detail, else the status.
    #[test]
    fn a_refusal_message_holds_only_kept_fields() {
        let says = |kept: Value| refusal_message("launch it", 400, &kept);
        assert_eq!(
            refusal_message("launch it", 302, &json!({"error": "moved"})),
            "cannot launch it: the API answered with a redirect (status `302`); its target is \
             not shown, since it may repeat a secret"
        );
        assert_eq!(
            says(json!({"error": "bad-x", "detail": "d"})),
            "cannot launch it: the API says `bad-x`"
        );
        assert_eq!(
            says(json!({"detail": "d\u{1b}"})),
            "cannot launch it: the API says `d\\u{1b}`"
        );
        assert_eq!(
            says(Value::Null),
            "cannot launch it: the API answered with status `400`; its words are not shown, \
             since they may repeat a secret"
        );
    }
}
