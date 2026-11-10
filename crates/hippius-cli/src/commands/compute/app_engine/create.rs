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

use std::time::{Duration, Instant};

use hippius::{AppSecrets, Code, Error, Method, Request, Result, phrase};
use serde_json::{Map, Value};

use super::{APPS, Compose, Hashed, app_path, check_hash, path, read_compose, unavailable_hint};
use crate::cli::{AppCreate, DiskTier, FailoverMode, GuestPoweroff};
use crate::commands::Printed;
use crate::commands::compute::{
    POLL_INTERVAL, STAYS_STOPPED, backups_clause, balance_hint, launch_wait,
};
use crate::commands::poll::{Next, Schedule, Wait, poll_until};
use crate::context::{Context, Write};

/// The flag that names the secrets file, in its placeholders and hints.
const SECRETS_FLAG: &str = "--secrets-file";

pub(super) fn create(ctx: &Context<'_>, args: &AppCreate) -> Result<Printed> {
    let timeout = launch_wait(ctx, args.wait, args.no_wait, args.wait_timeout);
    let compose = read_compose(&args.compose_file)?;
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
    let launch = |body: &Value| {
        Request::new(Method::Post, path("apps/"))
            .json(body)
            .about(&action)
            .listed_by(APPS)
    };
    let validate = compose.request("validate/", "validate");
    let draft = launch(&create_body(
        args,
        &compose,
        secrets.as_ref().map(AppSecrets::placeholder_body),
    ));
    let checked = match ctx.check_write(&client, &draft)? {
        Write::DryRun(mut doc) => {
            doc["requests"] = ctx.describe_each(&client, &[&validate, &draft])?;
            return Ok(Printed::Document(doc));
        }
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
    ctx.confirm_write(checked, &question, None)?;
    let redactions = secrets.as_ref().map(AppSecrets::redactions);
    let redactions: &[String] = redactions.as_ref().map_or(&[], |texts| texts.as_slice());
    let mut body = create_body(args, &compose, secrets.as_ref().map(AppSecrets::body));
    let request = launch(&body);
    // The body's copies of the values are cleared once it is serialized, and the file's with
    // `secrets`; the serialized bytes the request holds are not.
    hippius::clear_strings(&mut body);
    drop(secrets);
    let response = ctx
        .send(&client, &request)
        .map_err(|error| sanitized(balance_hint(unavailable_hint(error)), &action, redactions))?;
    let launched = response
        .document()
        .map_err(|error| sanitized(error, &action, redactions))?;
    // The 202 is the app, with the compose facts; when it names a hash, it must be the file's,
    // or the launch measures other bytes. The app exists by then, so it is printed.
    match launched.get("compose_sha256") {
        None | Some(Value::Null) => {}
        Some(Value::String(hash)) if hash.eq_ignore_ascii_case(&compose.sha256) => {}
        Some(_) => {
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
    if launched.get("vm_id").and_then(Value::as_str).is_none() {
        return Err(Error::internal("the launch answer has no `vm_id`")
            .with_hint(format!("run `{APPS}` to find the new app"))
            .after_write(printable(&launched, redactions)));
    }
    match timeout {
        Some(timeout) => wait(ctx, &client, launched, timeout, redactions),
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
    if let Some(tier) = args.os_disk_tier {
        let tier = tier.code();
        more.push(format!(
            "an OS disk on tier `{tier}` or faster, billed at the `{tier}` price that `hipc \
             compute prices` shows (`vm_by_os_disk_tier`)"
        ));
    }
    if let Some(interval) = args.backup_interval {
        more.push(backups_clause(interval, args.failover));
    }
    if args.on_guest_poweroff == Some(GuestPoweroff::Stop) {
        more.push(STAYS_STOPPED.to_owned());
    }
    if !more.is_empty() {
        action.push_str(&format!(", with {}", more.join(", and with ")));
    }
    action
}

/// The launch body: the flavor, the compose byte for byte, `secrets` when there are any, and
/// exactly the flags given. `public_ip` is never sent: the server's default, true, is the
/// only value it takes.
fn create_body(args: &AppCreate, compose: &Compose, secrets: Option<Value>) -> Value {
    let mut body = Map::new();
    body.insert("flavor".to_owned(), args.flavor.clone().into());
    body.insert("compose".to_owned(), compose.text.clone().into());
    if let Some(secrets) = secrets {
        body.insert("secrets".to_owned(), secrets);
    }
    for (key, value) in [
        ("name", args.name.as_deref()),
        ("region", args.region.as_deref()),
        ("os_disk_tier", args.os_disk_tier.map(DiskTier::code)),
        ("placement_group", args.placement_group.as_deref()),
        (
            "on_guest_poweroff",
            args.on_guest_poweroff.map(GuestPoweroff::code),
        ),
        ("failover_mode", args.failover.map(FailoverMode::code)),
    ] {
        if let Some(value) = value {
            body.insert(key.to_owned(), value.into());
        }
    }
    if let Some(interval) = args.backup_interval {
        body.insert("backup_interval_s".to_owned(), interval.seconds().into());
    }
    Value::Object(body)
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
    let declared: Vec<&str> = match parsed.get("secret_names") {
        Some(Value::Array(names)) if names.iter().all(Value::is_string) => {
            names.iter().filter_map(Value::as_str).collect()
        }
        other => {
            let answered = if other.is_none_or(Value::is_null) {
                "without `secret_names`"
            } else {
                "a `secret_names` that is not a list of names"
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

/// `text` with every text of `redactions` replaced by `<redacted>`, one character included, in
/// one pass from the left that takes the longest text matching at each place: `redactions` is
/// longest first, so a short value never cuts a marker an earlier one wrote.
fn redact_text(text: &str, redactions: &[String]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if let Some(secret) = redactions
            .iter()
            .find(|secret| rest.starts_with(secret.as_str()))
        {
            out.push_str("<redacted>");
            rest = &rest[secret.len()..];
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// Whether `text`, which has a shape hipc checks, may still be printed as it came: it is no
/// text of `redactions` and holds none of 8 bytes or more. A shape such as 40 hex digits fits
/// a key as well as an id, so the shape alone is no proof that it is not a value sent.
fn holds_no_secret(text: &str, redactions: &[String]) -> bool {
    !redactions
        .iter()
        .any(|secret| secret == text || (secret.len() >= 8 && text.contains(secret.as_str())))
}

/// `text`, which [`shaped`] or an error field's own check passed, as printed: as it came, or
/// `<redacted>` whole when it may be a value sent ([`holds_no_secret`]).
fn checked_text(text: String, redactions: &[String]) -> Value {
    if holds_no_secret(&text, redactions) {
        Value::String(text)
    } else {
        Value::from("<redacted>")
    }
}

/// `number` as printed: `<redacted>` when it spells an all-digit value sent, or holds one of
/// 8 digits or more (`hippius::number_holds`), else as it came.
fn printed_number(number: &Value, redactions: &[String]) -> Value {
    let text = number.to_string();
    if redactions
        .iter()
        .any(|secret| hippius::number_holds(&text, secret))
    {
        Value::from("<redacted>")
    } else {
        number.clone()
    }
}

/// The copy of an answer that is printed. Every decision reads the answer as it came; this
/// copy is only shown. A top-level field hipc checked by its shape ([`shaped`]), and a name in
/// the top-level `secret_names`, is printed as it came unless it may be a value sent
/// ([`checked_text`]). Every other string and number is redacted ([`redact_text`],
/// [`printed_number`]), and every key that may be a value ([`printed_key`]); two keys that
/// redact to one are told apart by a number. With no secrets sent the answer is printed as it came.
fn printable(value: &Value, redactions: &[String]) -> Value {
    if redactions.is_empty() {
        return value.clone();
    }
    let Value::Object(fields) = value else {
        return printed(value, redactions);
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
                    other => printed(other, redactions),
                })
                .collect(),
            other => printed(other, redactions),
        };
        insert_unique(&mut out, &printed_key(name, redactions), item);
    }
    Value::Object(out)
}

/// `value` with every string, key, and number redacted, at any depth.
fn printed(value: &Value, redactions: &[String]) -> Value {
    match value {
        Value::String(text) => Value::String(redact_text(text, redactions)),
        Value::Number(_) => printed_number(value, redactions),
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| printed(item, redactions)).collect())
        }
        Value::Object(fields) => {
            let mut out = Map::new();
            for (name, item) in fields {
                insert_unique(
                    &mut out,
                    &printed_key(name, redactions),
                    printed(item, redactions),
                );
            }
            Value::Object(out)
        }
        _ => value.clone(),
    }
}

/// `key` as printed: a field name is the server's, not free text, so it is redacted whole
/// only when it may be a value sent ([`holds_no_secret`]); a short value would otherwise
/// rewrite every name it occurs in.
fn printed_key(key: &str, redactions: &[String]) -> String {
    if holds_no_secret(key, redactions) {
        key.to_owned()
    } else {
        "<redacted>".to_owned()
    }
}

/// Inserts `value` under `key`, or under `key (2)`, `key (3)`, and so on when a redacted key
/// already took it, so no field is lost.
fn insert_unique(fields: &mut Map<String, Value>, key: &str, value: Value) {
    let mut name = key.to_owned();
    let mut n = 2;
    while fields.contains_key(&name) {
        name = format!("{key} ({n})");
        n += 1;
    }
    fields.insert(name, value);
}

/// Whether `text`, under the top-level `key`, has the one shape hipc reads that key as: a
/// `vm_id` by the rule of the argument, a `compose_sha256` of 64 hex digits, a status or phase
/// token, or a kebab `error`.
fn shaped(key: &str, text: &str) -> bool {
    match key {
        "vm_id" => crate::cli::vm_id(text).is_ok(),
        "compose_sha256" => text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit()),
        "status" | "boot_phase" | "power_state" => is_token(text),
        "error" => is_kebab(text),
        _ => false,
    }
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
/// a request id stays only as an id that holds no value ([`holds_no_secret`]); and the message
/// of any status outside 2xx (a redirect, whose target may hold anything, or a refusal) is
/// built from those fields alone, never from the server's own text, so a field that was
/// dropped cannot reach it. A failure with no status, or a 2xx hipc could not use, keeps a
/// message of hipc's own, with every value redacted from it all the same.
fn sanitized(error: Error, action: &str, redactions: &[String]) -> Error {
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
        _ => redact_text(error.message(), redactions),
    };
    error
        .with_details_kept(|_| kept)
        .with_request_id_kept(|id| {
            (is_request_id(&id) && holds_no_secret(&id, redactions)).then_some(id)
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
/// `message` and `hint`. A number is redacted as [`printed_number`] does. A body that is not an
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
                Value::String(redact_text(&text, redactions))
            }
            ("line", line) if line.is_u64() => printed_number(&line, redactions),
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
            ("line" | "column", number) if number.is_u64() => printed_number(&number, redactions),
            ("message" | "hint", Value::String(text)) => {
                Value::String(redact_text(&text, redactions))
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

/// Where a launch stands, from the app's `status`, `boot_phase`, and `boot_stalled`.
#[derive(Debug, PartialEq, Eq)]
enum Launch {
    /// Both are `running`.
    Done,
    /// A status the app does not come back from: `failed` (unless `boot_stalled`),
    /// `decommissioning`, or `decommissioned` (the VM's terminal statuses), or a `boot_phase`
    /// of `failed`. Swagger documents neither field's values, so these are the VM's,
    /// unverified on an app.
    Ended(&'static str),
    /// Anything else, a missing field or a value this version does not know included. A VM
    /// `failed` with `boot_stalled` true returns to `running` by itself once the guest signals
    /// (swagger, on the VM), and an app is a VM, so it is waited on as `vms create` waits.
    Waiting,
}

fn launch_state(app: &Value) -> Launch {
    let field = |key| app.get(key).and_then(Value::as_str);
    let stalled = app.get("boot_stalled").and_then(Value::as_bool) == Some(true);
    match (field("status"), field("boot_phase")) {
        (Some("running"), Some("running")) => Launch::Done,
        (Some("failed"), _) if stalled => Launch::Waiting,
        (Some("decommissioning"), _) => Launch::Ended("decommissioning"),
        (Some("decommissioned"), _) => Launch::Ended("decommissioned"),
        (Some("failed"), _) | (_, Some("failed")) => Launch::Ended("failed"),
        _ => Launch::Waiting,
    }
}

/// Polls the launched app until its status and boot phase are both `running`, it ends, or
/// `timeout` has passed. Every decision reads the documents as they came; every document
/// printed, on success or on a failure, is the [`printable`] copy. Every failure prints the
/// last app document seen, so `vm_id` is not lost, and is never retryable: a retry of the
/// launch would launch, and bill, a second app. It shares the VM launch's loop
/// ([`poll_until`]), with its poll interval, its tolerance of passing outages, and its
/// respect for `Retry-After`.
fn wait(
    ctx: &Context<'_>,
    client: &hippius::Client,
    launched: Value,
    timeout: Duration,
    redactions: &[String],
) -> Result<Printed> {
    let print = |app: &Value| printable(app, redactions);
    // `create` refused an answer without one before the wait.
    let vm_id = launched["vm_id"].as_str().unwrap_or_default().to_owned();
    // The id is quoted only once it passes the rule of the argument, as the poll URL needs,
    // and holds no value sent: the printable copy redacts such an id, and the note, the poll's
    // `--debug` line, the error, and the hint would otherwise print it raw on stderr.
    let pollable = if holds_no_secret(&vm_id, redactions) {
        crate::cli::vm_id(&vm_id)
    } else {
        Err("it may be a secret value sent".to_owned())
    };
    let app_named = match &pollable {
        Ok(_) => format!("the app {}", phrase::quote(&vm_id)),
        Err(_) => "the app".to_owned(),
    };
    let hint = match pollable
        .as_ref()
        .ok()
        .and_then(|id| crate::cli::as_argument(id, crate::cli::vm_id))
    {
        Some(id) => format!(
            "run {} to see where the launch stands",
            phrase::quote(format!("hipc compute app-engine apps get {id}"))
        ),
        None => format!("run `{APPS}` to find the new app"),
    };
    if launch_state(&launched) == Launch::Waiting {
        if let Err(rule) = pollable {
            let message =
                format!("the launch answer names the app by an id `hipc` cannot poll: {rule}");
            return Err(Error::internal(message)
                .with_hint(hint)
                .after_write(print(&launched)));
        }
        ctx.note(&format!(
            "launched {app_named}; waiting until it is running"
        ));
    }
    let request = Request::get(app_path(&vm_id, "")).about(format!("poll {app_named}"));
    let schedule = Schedule {
        first: POLL_INTERVAL,
        interval: POLL_INTERVAL,
        deadline: Instant::now() + timeout,
    };
    let launch = AppLaunch {
        app: launched,
        vm_id: &vm_id,
        app_named: &app_named,
        hint: &hint,
        action: request.action(),
        redactions,
        timeout,
    };
    poll_until(ctx, client, &request, schedule, launch)
}

/// The wait of an App Engine launch: the last app document seen, and how its messages name
/// it. Every document it prints is the [`printable`] copy.
struct AppLaunch<'a> {
    app: Value,
    vm_id: &'a str,
    app_named: &'a str,
    hint: &'a str,
    /// The poll's action, for an error the poll answered ([`sanitized`]).
    action: String,
    redactions: &'a [String],
    timeout: Duration,
}

impl AppLaunch<'_> {
    fn print(&self) -> Value {
        printable(&self.app, self.redactions)
    }
}

impl Wait for AppLaunch<'_> {
    type Output = Printed;

    fn next(&mut self) -> Next<Printed> {
        match launch_state(&self.app) {
            Launch::Done => Next::Stop(Ok(Printed::Document(self.print()))),
            Launch::Ended(state) => {
                let message = format!("{} reached `{state}` instead of `running`", self.app_named);
                Next::Stop(Err(Error::new(Code::PartialFailure, message)
                    .with_hint(self.hint)
                    .after_write(self.print())))
            }
            Launch::Waiting => Next::Poll,
        }
    }

    fn take(&mut self, polled: Value) -> Result<()> {
        // A document for another app, or for none, is not this launch.
        match polled.get("vm_id").and_then(Value::as_str) {
            Some(id) if id == self.vm_id => {
                self.app = polled;
                Ok(())
            }
            other => {
                let named = match other {
                    Some(other)
                        if crate::cli::vm_id(other).is_ok()
                            && holds_no_secret(other, self.redactions) =>
                    {
                        format!("the app {}", phrase::quote(other))
                    }
                    Some(_) => "another app".to_owned(),
                    None => "no app".to_owned(),
                };
                let message = format!(
                    "the poll of {} answered with the document of {named}",
                    self.app_named
                );
                Err(Error::internal(message)
                    .with_hint(self.hint)
                    .after_write(self.print()))
            }
        }
    }

    fn timed_out(self) -> Result<Printed> {
        let message = format!(
            "{} was not running after {}",
            self.app_named,
            phrase::count(self.timeout.as_secs(), "second")
        );
        Err(Error::new(Code::Timeout, message)
            .with_hint(self.hint)
            .after_write(self.print()))
    }

    // A poll is a read, so polling on after a passing outage cannot launch twice.
    fn failed(self, error: Error) -> Error {
        sanitized(error, &self.action, self.redactions)
            .with_hint(self.hint)
            .after_write(self.print())
    }
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

    /// One pass from the left, longest first: a short value cannot cut a marker already
    /// written, and a longer value that holds a shorter one is redacted whole.
    #[test]
    fn redaction_is_one_pass_longest_first() {
        let secrets = ["dacted".to_owned(), "a".to_owned()];
        assert_eq!(
            redact_text("dacted a b", &secrets),
            "<redacted> <redacted> b"
        );
        assert_eq!(
            redact_text("<redacted>", &["d".to_owned()]),
            "<re<redacted>acte<redacted>>"
        );
        assert_eq!(redact_text("ééa", &secrets), "éé<redacted>");
        assert_eq!(redact_text("x", &[]), "x");
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

    #[test]
    fn a_launch_is_done_only_when_status_and_boot_phase_are_running() {
        let state = |app: Value| format!("{:?}", launch_state(&app));
        assert_eq!(
            state(json!({"status": "running", "boot_phase": "running"})),
            "Done"
        );
        for app in [
            json!({"status": "running", "boot_phase": "pulling"}),
            json!({"status": "running"}),
            json!({"status": "provisioning", "boot_phase": "running"}),
            json!({}),
            json!({"status": 1, "boot_phase": null}),
            json!({"status": "failed", "boot_stalled": true}),
        ] {
            assert_eq!(state(app.clone()), "Waiting", "{app}");
        }
        for (app, ended) in [
            (
                json!({"status": "failed", "boot_phase": "running"}),
                "failed",
            ),
            (json!({"status": "decommissioning"}), "decommissioning"),
            (json!({"status": "decommissioned"}), "decommissioned"),
            (
                json!({"status": "running", "boot_phase": "failed"}),
                "failed",
            ),
        ] {
            assert_eq!(state(app.clone()), format!("Ended({ended:?})"), "{app}");
        }
    }
}
