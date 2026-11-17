//! `hipc compute app-engine apps update`: `PUT .../compose/` with a new compose, new secrets,
//! or both.
//!
//! The order (tenet 5): argv; the compose file and the secrets file; the dry run, which sends
//! nothing and shows a placeholder for each value; the token; `GET` the app, for the secret
//! names it stores; with a compose, `POST validate/` (which changes nothing) and the check
//! that its hash is the file's; the check that a name in the file is declared, and that a
//! declared name is either stored or in the file; the question; the update. The values given
//! replace the stored ones; the rest are kept. A secret value goes into the body only once
//! the update is confirmed. Decisions read the answer as it came; only the printed copy is
//! redacted, by the helpers [`super::create`] uses for a launch.

use std::fmt;

use hippius::routes::compute::{self, Vm};
use hippius::{
    AppSecrets, Client, Error, Request, Response, Result, SecretValue, Slot, phrase, view_of,
};
use serde_json::Value;

use super::create::{SECRETS_FLAG, printable, sanitized};
use super::{APPS_LIST_COMMAND, Compose, Hashed, check_hash, read_compose, unavailable_hint};
use crate::cli::AppUpdate;
use crate::commands::Printed;
use crate::context::{Confirmed, Context, Write, WritePlan};

/// What the question says a compose change does to the running app. Secrets, when sent too,
/// add the clause after the semicolon.
const COMPOSE_CONSEQUENCE: &str = "the VM stops and starts again, and its volumes are kept, \
     and the launch measurement then holds this compose's SHA-256, so attestation proves this \
     file";

/// What the question says a secrets-only change does. An update relaunches, so the
/// measurement changes; no compose is sent, so the compose hash in it stays.
const SECRETS_CONSEQUENCE: &str = "the VM stops and starts again, and its volumes are kept, \
     and the launch measurement changes, but the compose hash in it stays the one the app \
     runs now; the values given replace the stored ones; the rest are kept";

const BOTH_CONSEQUENCE: &str = "the VM stops and starts again, and its volumes are kept, and \
     the launch measurement then holds this compose's SHA-256, so attestation proves this \
     file; the values given replace the stored ones; the rest are kept";

/// Which of the two files the update sends. An update with neither is refused before this
/// exists.
#[derive(Debug, Clone, Copy)]
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
/// only once `load` has refused an update that sends neither. Its `Debug` names the compose
/// file and counts the secrets: the file text and the values are not shown.
struct Loaded<'a> {
    vm_id: &'a str,
    compose: Option<Compose>,
    secrets: Option<AppSecrets>,
    change: Change,
}

impl Loaded<'_> {
    /// What a refusal names: the compose file, or the app when no compose is sent.
    fn names_source(&self) -> String {
        match &self.compose {
            Some(compose) => compose.name.clone(),
            None => format!("the app `{}`", self.vm_id),
        }
    }
}

impl fmt::Debug for Loaded<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let secret_count = self.secrets.as_ref().map(|secrets| secrets.names().count());
        f.debug_struct("Loaded")
            .field("vm_id", &self.vm_id)
            .field(
                "compose",
                &self.compose.as_ref().map(|compose| compose.name.as_str()),
            )
            .field("secret_count", &secret_count)
            .field("change", &self.change)
            .finish_non_exhaustive()
    }
}

/// The reads before the question. The app is always read; validate is sent only with a
/// compose. Built once and reused for the dry run and the sends.
struct Reads {
    app: Request,
    validate: Option<Request>,
}

impl Reads {
    /// The reads a dry run lists, in the order a real run sends them.
    fn before(&self) -> Vec<&Request> {
        let mut before = vec![&self.app];
        if let Some(validate) = &self.validate {
            before.push(validate);
        }
        before
    }
}

/// The app read, and validate of the new compose when one is sent.
fn reads_of(loaded: &Loaded<'_>) -> Reads {
    let validate = loaded
        .compose
        .as_ref()
        .map(|compose| compose.request(&compute::APP_ENGINE_VALIDATE, "validate"));
    Reads {
        app: app_read(loaded.vm_id),
        validate,
    }
}

/// `GET` the app, so a missing one is `not_found` with the list hint before the question,
/// and so the stored secret names are known before any value is sent.
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
    let reads = reads_of(&loaded);
    let before = reads.before();
    let placeholders = loaded.secrets.as_ref().map(AppSecrets::placeholder_body);
    let draft = put(
        loaded.vm_id,
        loaded.compose.as_ref(),
        placeholders.as_ref(),
        &action,
    );
    let checked = match ctx.check_write(&client, &WritePlan::new(&before, &draft))? {
        // The app is read before the question, for the secret names it stores.
        Write::DryRun(doc) => {
            return Ok(doc.reads_first(ctx.describe(&client, &reads.app)?).into());
        }
        Write::Checked(checked) => checked,
    };
    let answers = declared(ctx, &client, &loaded, &reads)?;
    let source = loaded.names_source();
    check_merged_secrets(
        &answers.app,
        answers.validated.as_ref(),
        loaded.secrets.as_ref(),
        loaded.vm_id,
        &source,
    )?;
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

/// The app, and the validate answer when a compose was sent.
struct Declared {
    app: Value,
    validated: Option<Value>,
}

/// Sends [`Reads`]: the app, then validate when a compose is sent. A compose's hash must be
/// the file's. A 403 outside the feature names `app-engine get`.
fn declared(
    ctx: &Context<'_>,
    client: &Client,
    loaded: &Loaded<'_>,
    reads: &Reads,
) -> Result<Declared> {
    let app = send_document(ctx, client, &reads.app)?;
    let validated = match &reads.validate {
        None => None,
        Some(request) => {
            let response = ctx.send(client, request).map_err(unavailable_hint)?;
            let parsed = response.document().map_err(unavailable_hint)?;
            // `reads_of` builds validate only from a loaded compose.
            let compose = loaded
                .compose
                .as_ref()
                .expect("validate is sent only when a compose was loaded");
            check_hash(
                &response,
                &parsed,
                &compose.sha256,
                Hashed::Sent(&compose.name),
            )
            .map_err(unavailable_hint)?;
            Some(parsed)
        }
    };
    Ok(Declared { app, validated })
}

fn send_document(ctx: &Context<'_>, client: &Client, request: &Request) -> Result<Value> {
    ctx.send(client, request)
        .and_then(|response| response.document())
        .map_err(unavailable_hint)
}

/// Refuses names the update must not send. The API merges: a value in the file replaces the
/// stored one, and a name left out keeps its value. So the file may give a subset. It may
/// not give a name `source` does not declare (the new compose, or the app when no compose is
/// sent), and a declared name that is neither stored nor in the file has no value to merge.
/// An answer whose `secret_names` is missing, null, or not a list of strings is internal.
/// A name is quoted only when it is declared: one that is not may be a value pasted in the
/// wrong place.
fn check_merged_secrets(
    app: &Value,
    validated: Option<&Value>,
    secrets: Option<&AppSecrets>,
    vm_id: &str,
    source: &str,
) -> Result<()> {
    let stored = names_of(app, &format!("the app `{vm_id}`"))?;
    let declared = match validated {
        Some(validated) => names_of(validated, source)?,
        None => stored.clone(),
    };
    let given: Vec<&str> = secrets
        .map(|secrets| secrets.names().collect())
        .unwrap_or_default();
    let declared_names: Vec<&str> = declared.iter().map(String::as_str).collect();
    let undeclared = given
        .iter()
        .filter(|name| !declared_names.contains(name))
        .count();
    if undeclared > 0 {
        return Err(undeclared_names(undeclared, source, &declared_names));
    }
    let stored_names: Vec<&str> = stored.iter().map(String::as_str).collect();
    if let Some(name) = declared_names
        .iter()
        .find(|name| !stored_names.contains(name) && !given.contains(name))
    {
        return Err(neither_stored_nor_given(source, name, secrets.is_some()));
    }
    Ok(())
}

/// `secret_names` of `parsed`, or internal when nothing can be checked.
fn names_of(parsed: &Value, source: &str) -> Result<Vec<String>> {
    let names = view_of::<compute::Validated>(parsed).secret_names;
    match names {
        Slot::Value(names) => Ok(names),
        missing @ (Slot::Missing | Slot::Null | Slot::Other) => {
            let answered = if missing.is_set() {
                "a `secret_names` that is not a list of names"
            } else {
                "without `secret_names`"
            };
            Err(Error::internal(format!(
                "cannot check the declared secrets: the API's check of {source} answered \
                 {answered}"
            )))
        }
    }
}

fn undeclared_names(undeclared: usize, source: &str, declared: &[&str]) -> Error {
    let declares = if declared.is_empty() {
        "none".to_owned()
    } else {
        declared
            .iter()
            .map(phrase::quote)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let count = u64::try_from(undeclared).expect("a secret count fits in u64");
    Error::usage(
        format!(
            "the secrets file gives {} that {source} does not declare",
            phrase::count(count, "secret"),
        ),
        format!(
            "remove from the secrets file every name {source} does not declare; it declares \
             {declares}"
        ),
    )
}

fn neither_stored_nor_given(source: &str, name: &str, file_given: bool) -> Error {
    let name = phrase::quote(name);
    let hint = if file_given {
        format!(
            "add {name} and its value to the file `{SECRETS_FLAG}` names; a name the app \
             already stores may be left out"
        )
    } else {
        format!(
            "pass `{SECRETS_FLAG}` a mode-0600 JSON object with a value for {name}; a name the \
             app already stores may be left out"
        )
    };
    Error::usage(
        format!(
            "{source} declares the secret {name}, which is neither stored nor given in \
             `{SECRETS_FLAG}`"
        ),
        hint,
    )
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
        assert!(!COMPOSE_CONSEQUENCE.contains("secret"));
        assert!(SECRETS_CONSEQUENCE.contains(
            "the launch measurement changes, but the compose hash in it stays the one the app \
             runs now"
        ));
        assert!(
            SECRETS_CONSEQUENCE
                .contains("the values given replace the stored ones; the rest are kept")
        );
        assert!(
            BOTH_CONSEQUENCE
                .contains("the values given replace the stored ones; the rest are kept")
        );
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

    /// A stored name may be left out. A declared name with no stored value and no file is
    /// usage. A `secret_names` hipc cannot read is internal, and the first such name in the
    /// declared list is the one reported.
    #[test]
    fn a_stored_secret_may_be_omitted_and_a_new_one_needs_a_value() {
        let app = json!({ "secret_names": ["DB_PASSWORD", "API_KEY"] });
        let same = json!({ "secret_names": ["DB_PASSWORD", "API_KEY"] });
        let source = "the compose file `compose.yaml`";
        check_merged_secrets(&app, Some(&same), None, "vm-1a2b", source).unwrap();
        check_merged_secrets(&app, None, None, "vm-1a2b", "the app `vm-1a2b`").unwrap();

        let added = json!({ "secret_names": ["DB_PASSWORD", "NEW_SECRET"] });
        let error = check_merged_secrets(&app, Some(&added), None, "vm-1a2b", source).unwrap_err();
        let message = error.message();
        assert!(message.contains(source), "{message}");
        assert!(message.contains("`NEW_SECRET`"), "{message}");
        assert!(message.contains("neither stored nor given"), "{message}");
        assert!(!message.contains("`DB_PASSWORD`"), "{message}");

        let bare = json!({ "vm_id": "vm-1a2b" });
        let error = check_merged_secrets(&bare, None, None, "vm-1a2b", source).unwrap_err();
        let message = error.message();
        assert!(message.contains("without `secret_names`"), "{message}");
        let odd = json!({ "secret_names": "DB_PASSWORD" });
        let error = check_merged_secrets(&app, Some(&odd), None, "vm-1a2b", source).unwrap_err();
        let message = error.message();
        assert!(message.contains("not a list of names"), "{message}");
    }

    /// A compose that declares no secret says so in the hint, and a declared name with no
    /// stored value and no secrets file at all asks for the file.
    #[test]
    fn the_secret_refusals_name_what_is_declared_and_what_to_pass() {
        let source = "the compose file `compose.yaml`";
        let error = undeclared_names(2, source, &[]);
        assert_eq!(
            error.message(),
            "the secrets file gives 2 secrets that the compose file `compose.yaml` does not \
             declare"
        );
        assert!(
            error
                .hint()
                .is_some_and(|hint| hint.ends_with("it declares none")),
            "{error:?}"
        );
        let error = neither_stored_nor_given(source, "DB_PASSWORD", false);
        assert!(
            error
                .hint()
                .is_some_and(|hint| hint.starts_with(&format!("pass `{SECRETS_FLAG}` a mode-0600"))),
            "{error:?}"
        );
    }

    /// `Debug` names the compose file and counts secrets. The file text is not shown.
    #[test]
    fn loaded_debug_names_the_compose_file_and_not_its_text() {
        let loaded = Loaded {
            vm_id: "vm-1a2b",
            compose: Some(Compose {
                text: "services:\n  web:\n    image: secret-image\n".to_owned(),
                sha256: "ab".repeat(32),
                name: "the compose file `compose.yaml`".to_owned(),
            }),
            secrets: None,
            change: Change::Compose,
        };
        let shown = format!("{loaded:?}");
        assert!(shown.contains("vm-1a2b"), "{shown}");
        assert!(shown.contains("compose.yaml"), "{shown}");
        assert!(shown.contains("secret_count"), "{shown}");
        assert!(!shown.contains("secret-image"), "{shown}");
    }
}
