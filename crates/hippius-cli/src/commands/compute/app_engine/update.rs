//! `hipc compute app-engine apps update`: `PUT .../compose/` with a new compose, new secrets,
//! or both.
//!
//! The order (tenet 5): argv; the compose file and the secrets file; the dry run, which sends
//! nothing and shows a placeholder for each value; the token; with a compose, `POST validate/`
//! (which changes nothing) and the local checks of its answer, or, with secrets alone, the read
//! of the app and the same check of its `secret_names`; the question; the update. A secret
//! value goes into the body only once the update is confirmed. Decisions read the answer as it
//! came; only the printed copy is redacted, by the helpers [`super::create`] uses for a launch.

use hippius::routes::compute::{self, Vm};
use hippius::{AppSecrets, Client, Error, Request, Response, Result, SecretValue, Slot, view_of};
use serde_json::Value;

use super::create::{SECRETS_FLAG, check_secret_names, printable, sanitized};
use super::{APPS_LIST_COMMAND, Compose, Hashed, check_hash, read_compose, unavailable_hint};
use crate::cli::AppUpdate;
use crate::commands::Printed;
use crate::context::{Confirmed, Context, Write, WritePlan};

/// What the question says a compose change does to the running app. Secrets, when sent too,
/// add the clause after the semicolon.
const COMPOSE_CONSEQUENCE: &str = "the VM stops and starts again, and its volumes are kept, \
     and the launch measurement then holds this compose's SHA-256, so attestation proves this \
     file";

/// What the question says a secrets-only change does. The compose hash in the measurement is
/// unchanged: this update sends no compose. Swagger's description of the route still says
/// "the new compose"; that the hash stays is this command's reading of an omitted compose.
const SECRETS_CONSEQUENCE: &str = "the VM stops and starts again, and its volumes are kept, \
     and the compose's SHA-256 in the launch measurement stays the one the app runs now; the \
     secret values are replaced and are not shown again";

const BOTH_CONSEQUENCE: &str = "the VM stops and starts again, and its volumes are kept, and \
     the launch measurement then holds this compose's SHA-256, so attestation proves this \
     file; the secret values are replaced and are not shown again";

/// Which of the two files the update sends. An update with neither is refused before this
/// exists.
#[derive(Clone, Copy)]
enum Change {
    Compose,
    Secrets,
    Both,
}

impl Change {
    /// What the update does, for the question and for a refusal that names no consequence.
    fn action(self, vm_id: &str) -> String {
        let what = match self {
            Self::Compose => "the compose",
            Self::Secrets => "the secrets",
            Self::Both => "the compose and the secrets",
        };
        format!("replace {what} of the app `{vm_id}`")
    }

    /// What the update does to the running app, said after the action in the question.
    fn consequence(self) -> &'static str {
        match self {
            Self::Compose => COMPOSE_CONSEQUENCE,
            Self::Secrets => SECRETS_CONSEQUENCE,
            Self::Both => BOTH_CONSEQUENCE,
        }
    }
}

/// The files of one update, after an empty secrets file has been dropped. `change` is set
/// only once `load` has refused an update that sends neither.
struct Loaded<'a> {
    vm_id: &'a str,
    compose: Option<Compose>,
    secrets: Option<AppSecrets>,
    change: Change,
}

impl Loaded<'_> {
    /// What [`check_secret_names`] names: the compose file, or the app when no compose is sent.
    fn names_source(&self) -> String {
        match &self.compose {
            Some(compose) => compose.name.clone(),
            None => format!("the app `{}`", self.vm_id),
        }
    }
}

/// The read that names the declared secrets: validate of the new compose, or the app itself
/// when only secrets change.
fn prelude(loaded: &Loaded<'_>) -> Request {
    match &loaded.compose {
        Some(compose) => compose.request(&compute::APP_ENGINE_VALIDATE, "validate"),
        None => app_read(loaded.vm_id),
    }
}

/// `GET` the app, so a missing one is `not_found` with the list hint before the question.
fn app_read(vm_id: &str) -> Request {
    Request::to(&compute::APP_ENGINE_APP, [&vm_id])
        .about(format!("get the app `{vm_id}`"))
        .listed_by(APPS_LIST_COMMAND)
}

/// The compose update, with `secrets` when there are any: the values, or their placeholders
/// in a dry run.
fn put(vm_id: &str, compose: Option<&Compose>, secrets: Option<&Value>, action: &str) -> Request {
    Request::to(&compute::APP_ENGINE_COMPOSE_UPDATE, [&vm_id])
        .json(&compute::AppComposeUpdateBody {
            compose: compose.map(|compose| compose.text.as_str()),
            secrets,
        })
        .about(action)
        .listed_by(APPS_LIST_COMMAND)
}

pub(super) fn update(ctx: &Context<'_>, args: &AppUpdate) -> Result<Printed> {
    let mut loaded = load(args)?;
    if let Some(compose) = &loaded.compose {
        compose.warn_of_invisible_bytes(ctx);
    }
    let client = ctx.load_authed_client()?;
    let change = loaded.change;
    let action = change.action(loaded.vm_id);
    let before = prelude(&loaded);
    let placeholders = loaded.secrets.as_ref().map(AppSecrets::placeholder_body);
    let draft = put(
        loaded.vm_id,
        loaded.compose.as_ref(),
        placeholders.as_ref(),
        &action,
    );
    let checked = match ctx.check_write(&client, &WritePlan::new(&[&before], &draft))? {
        Write::DryRun(doc) => return Ok(doc.into()),
        Write::Checked(checked) => checked,
    };
    let parsed = declared(ctx, &client, &loaded)?;
    check_secret_names(&parsed, loaded.secrets.as_ref(), &loaded.names_source())?;
    let confirmed = ctx.confirm_write(checked, &action, Some(change.consequence()))?;
    finish(ctx, &client, &mut loaded, &action, &confirmed)
}

/// Reads the compose and the secrets. An empty secrets object sends no secrets, and with no
/// compose either, that is a usage error before any request, a dry run included.
fn load(args: &AppUpdate) -> Result<Loaded<'_>> {
    let compose = args.compose_file.as_deref().map(read_compose).transpose()?;
    let secrets = args
        .secrets_file
        .as_deref()
        .map(|path| AppSecrets::load(path, SECRETS_FLAG))
        .transpose()?
        .filter(|secrets| !secrets.is_empty());
    let change = match (compose.is_some(), secrets.is_some()) {
        (true, false) => Change::Compose,
        (false, true) => Change::Secrets,
        (true, true) => Change::Both,
        (false, false) => {
            return Err(Error::usage(
                "an app update needs `--compose-file`, `--secrets-file`, or both",
                "pass `--compose-file` with the compose to run, or `--secrets-file` with the \
                 app's secrets",
            ));
        }
    };
    Ok(Loaded {
        vm_id: args.target.vm_id.as_str(),
        compose,
        secrets,
        change,
    })
}

/// Sends [`prelude`] and checks it: a compose's hash must be the file's, and the app read
/// needs no such check. A 403 outside the feature names `app-engine get`.
fn declared(ctx: &Context<'_>, client: &Client, loaded: &Loaded<'_>) -> Result<Value> {
    let request = prelude(loaded);
    ctx.send(client, &request)
        .and_then(|response| {
            let parsed = response.document()?;
            if let Some(compose) = &loaded.compose {
                check_hash(
                    &response,
                    &parsed,
                    &compose.sha256,
                    Hashed::Sent(&compose.name),
                )?;
            }
            Ok(parsed)
        })
        .map_err(unavailable_hint)
}

/// Confirmed: puts the update, then checks a compose hash the answer names. The values are
/// dropped once the body is serialized. Every failure is redacted with the launch's helpers.
fn finish(
    ctx: &Context<'_>,
    client: &Client,
    loaded: &mut Loaded<'_>,
    action: &str,
    confirmed: &Confirmed,
) -> Result<Printed> {
    let secrets = loaded.secrets.take();
    let redactions = secrets.as_ref().map(AppSecrets::redactions);
    let texts: &[String] = redactions.as_ref().map_or(&[], |texts| texts.as_slice());
    let values = secrets.as_ref().map(AppSecrets::body);
    let request = put(
        loaded.vm_id,
        loaded.compose.as_ref(),
        values.as_ref().map(SecretValue::as_value),
        action,
    );
    drop(values);
    drop(secrets);
    let response = ctx
        .send_confirmed(client, &request, confirmed)
        .map_err(|error| sanitized(unavailable_hint(error), action, texts))?;
    let updated = response
        .document()
        .map_err(|error| sanitized(error, action, texts))?;
    if let Some(compose) = &loaded.compose
        && !compose_hash_agrees(&updated, &compose.sha256)
    {
        return Err(wrong_hash(&response, compose, action, texts, &updated));
    }
    Ok(Printed::Document(printable(&updated, texts)))
}

/// Whether `answer`'s `compose_sha256`, when it names one, is `expected`. Missing and null
/// agree: the server stored the compose and did not repeat the hash.
fn compose_hash_agrees(answer: &Value, expected: &str) -> bool {
    match view_of::<Vm>(answer).compose_sha256 {
        Slot::Missing | Slot::Null => true,
        Slot::Value(hash) => hash.eq_ignore_ascii_case(expected),
        Slot::Other => false,
    }
}

/// The update measured other bytes than the file. The app is printed: the write went through.
fn wrong_hash(
    response: &Response,
    compose: &Compose,
    action: &str,
    redactions: &[String],
    updated: &Value,
) -> Error {
    let why = format!(
        "the API hashed other bytes than {}: the `compose_sha256` of its update answer is \
         not `{}`, the SHA-256 hipc computed",
        compose.name, compose.sha256
    );
    sanitized(response.refused(&why), action, redactions)
        .after_write(printable(updated, redactions))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_question_names_what_changes_and_what_the_relaunch_does() {
        assert_eq!(
            Change::Compose.action("vm-1a2b"),
            "replace the compose of the app `vm-1a2b`"
        );
        assert_eq!(
            Change::Secrets.action("vm-1a2b"),
            "replace the secrets of the app `vm-1a2b`"
        );
        assert_eq!(
            Change::Both.action("vm-1a2b"),
            "replace the compose and the secrets of the app `vm-1a2b`"
        );
        assert_eq!(Change::Compose.consequence(), COMPOSE_CONSEQUENCE);
        assert_eq!(Change::Secrets.consequence(), SECRETS_CONSEQUENCE);
        assert_eq!(Change::Both.consequence(), BOTH_CONSEQUENCE);
        assert!(COMPOSE_CONSEQUENCE.contains("stops and starts again"));
        assert!(COMPOSE_CONSEQUENCE.contains("SHA-256"));
        assert!(!COMPOSE_CONSEQUENCE.contains("secret values"));
        assert!(SECRETS_CONSEQUENCE.contains("stays the one the app runs now"));
        assert!(BOTH_CONSEQUENCE.contains("secret values are replaced"));
    }

    /// A missing or null hash agrees; another string, in either case, must be the file's;
    /// anything else does not.
    #[test]
    fn a_named_hash_must_be_the_files_and_a_missing_one_agrees() {
        let expected = "ab".repeat(32);
        let agrees = |answer: Value| compose_hash_agrees(&answer, &expected);
        assert!(agrees(json!({})));
        assert!(agrees(json!({ "compose_sha256": null })));
        assert!(agrees(json!({ "compose_sha256": expected.to_uppercase() })));
        assert!(!agrees(json!({ "compose_sha256": "cd".repeat(32) })));
        assert!(!agrees(json!({ "compose_sha256": 7 })));
    }
}
