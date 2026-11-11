//! `hipc compute app-engine`: the reads of App Engine, the two POSTs that check and fix a
//! compose file without launching anything, and the launch ([`create`]).
//!
//! Every route is under `/api/compute/`, so it is account-scoped and takes a project, and a
//! billing selection is refused (tenet 12). `GET info/` gates the feature per account: every
//! other route answers 403 `app-engine-unavailable` outside it, and that refusal names `hipc
//! compute app-engine get`, which says why; `get` itself gets no such hint.
//!
//! The compose's SHA-256 is part of the app's launch measurement, so the file is sent byte for
//! byte as the JSON string `compose`: nothing is trimmed, and a byte-order mark or a carriage
//! return is sent as it is, for the server's own rule to refuse, with a warning before it is
//! sent. A file that is not UTF-8 is refused before anything is sent, since it cannot travel as
//! a JSON string unchanged, and so is a symlink or anything but a regular file, since a link
//! to the config file would upload its token. Every answer must carry the `compose_sha256` of
//! the bytes it should hash, the check the console makes before it offers a launch; without
//! it the answer is not trusted.
//!
//! Only the launch sends a secret, from a checked file, and [`create`] says how it is kept out
//! of every output. validate's optional `secrets` map is never sent, and no answer holds a
//! secret value (only `secret_names`).

mod create;

use std::path::Path;

use hippius::routes::{Route, compute};
use hippius::{Error, Request, Response, Result, phrase};
use serde_json::{Value, json};

use crate::cli::{AppEngine, AppEngineApps, GuestComponents};
use crate::commands::files::write_output_file;
use crate::commands::{Input, Printed, print_list_with, print_with};
use crate::context::{Context, Gate};

/// The largest compose file read. The server's limit is lower (`max_compose_bytes` in `hipc
/// compute app-engine get`) and it refuses a larger one itself;
/// this cap, the config file's, only stops a wrong path (a log, an image) from being read whole.
const MAX_COMPOSE_BYTES: u64 = 1024 * 1024;

/// The command that lists the `vm_id`s, for the hint of a 404 on an app route.
const APPS: &str = "hipc compute app-engine apps list";

pub(crate) fn run(ctx: &Context<'_>, command: &AppEngine) -> Result<Printed> {
    match command {
        // `get` is the command the unavailable hint names, so it is sent without that hint.
        AppEngine::Get => {
            let request =
                Request::to(&compute::APP_ENGINE_INFO, []).about("get the App Engine limits");
            send(ctx, &request, |response| {
                let info = response.document()?;
                if info.get("enabled") == Some(&Value::Bool(false)) {
                    let why = match info.get("reason").and_then(Value::as_str) {
                        Some(reason) => format!(": the API says {}", phrase::quote(reason)),
                        None => String::new(),
                    };
                    ctx.warning(&format!("the account may not use App Engine{why}"));
                }
                Ok(info)
            })
        }
        AppEngine::Validate { compose_file } => validate(ctx, compose_file),
        AppEngine::Fix {
            compose_file,
            output_file,
        } => fix(ctx, compose_file, output_file.as_deref()),
        // Swagger types the list as a bare array with no page parameters, so it is read whole
        // and printed as the list envelope, as the VM list is.
        AppEngine::Apps(AppEngineApps::List) => {
            let request =
                Request::to(&compute::APP_ENGINE_APPS, []).about("list the App Engine apps");
            print_list_with(ctx, &ctx.load_authed_client()?, &request, |response| {
                hippius::page(response.json()?)?.complete_envelope(request.target())
            })
            .map_err(unavailable_hint)
        }
        AppEngine::Apps(AppEngineApps::Create(args)) => create::create(ctx, args),
        AppEngine::Apps(AppEngineApps::Get { vm_id }) => {
            let request = Request::to(&compute::APP_ENGINE_APP, [vm_id])
                .about(format!("get the app `{vm_id}`"))
                .listed_by(APPS);
            show(ctx, &request, |response| {
                let app = response.document()?;
                // The server writes `verify_command`, and a hostile origin could make it any
                // shell text, so a person is told to read it before running it.
                if app
                    .pointer("/evidence/verify_command")
                    .is_some_and(|c| !c.is_null())
                {
                    ctx.warning(
                        "`evidence.verify_command` comes from the API: read it before running \
                         it, and never pipe it to a shell",
                    );
                }
                Ok(app)
            })
        }
        AppEngine::Apps(AppEngineApps::Runtime { vm_id }) => {
            let request = Request::to(&compute::APP_ENGINE_RUNTIME, [vm_id])
                .about(format!("get the runtime of the app `{vm_id}`"))
                .listed_by(APPS);
            show(ctx, &request, |response| {
                let runtime = response.document()?;
                // Every successful answer is unverified, so this is a note to read, not a
                // warning of something wrong.
                ctx.note(&format!(
                    "the VM of `{vm_id}` wrote this answer, and Hippius did not verify it: it \
                     is no proof of what runs"
                ));
                Ok(runtime)
            })
        }
        AppEngine::GuestComponents(GuestComponents::Get { vm_id }) => {
            let request = Request::to(&compute::APP_ENGINE_GUEST_COMPONENTS, [vm_id])
                .about(format!("get the guest components of the app `{vm_id}`"))
                .listed_by(APPS);
            show(ctx, &request, |response| {
                let components = response.document()?;
                if components.get("needs_operator") == Some(&Value::Bool(true)) {
                    ctx.warning(&format!(
                        "an earlier upgrade of `{vm_id}` failed, and Hippius must recover its VM"
                    ));
                }
                Ok(components)
            })
        }
        AppEngine::GuestComponents(GuestComponents::UpdateStatus { vm_id }) => {
            let request = Request::to(&compute::APP_ENGINE_GUEST_UPGRADE, [vm_id])
                .about(format!("get the guest upgrade of the app `{vm_id}`"))
                .listed_by(APPS);
            show(ctx, &request, |response| {
                let upgrade = response.document()?;
                if upgrade.get("job") == Some(&Value::Null) {
                    ctx.note(&format!("`{vm_id}` was never upgraded"));
                }
                Ok(upgrade)
            })
        }
    }
}

/// [`send`], with the hint that says why on a 403 `app-engine-unavailable`.
fn show(
    ctx: &Context<'_>,
    request: &Request,
    render: impl FnOnce(&Response) -> Result<Value>,
) -> Result<Printed> {
    send(ctx, request, render).map_err(unavailable_hint)
}

/// Sends `request`, a GET or a POST that changes nothing, and prints what `render` makes of
/// the answer.
fn send(
    ctx: &Context<'_>,
    request: &Request,
    render: impl FnOnce(&Response) -> Result<Value>,
) -> Result<Printed> {
    let client = ctx.load_authed_client()?;
    print_with(ctx, &client, request, Gate::Open, |response| {
        render(&response)
    })
}

/// `error` with the hint to read `GET info/` when the server refused it because the account
/// may not use App Engine; any other error unchanged.
fn unavailable_hint(error: Error) -> Error {
    if super::refused_as(&error, 403, "app-engine-unavailable") {
        error.with_hint("run `hipc compute app-engine get` to see whether the account may use it")
    } else {
        error
    }
}

/// A compose file as it was read: its exact text, its SHA-256, and how a message names it.
struct Compose {
    text: String,
    sha256: String,
    /// "the compose file `<path>`", or "the compose from stdin".
    name: String,
}

/// Reads the compose file `--compose-file` names, or stdin for `-`: at most
/// [`MAX_COMPOSE_BYTES`] of UTF-8 text, unchanged, from a regular file that is not a symlink.
fn read_compose(path: &Path) -> Result<Compose> {
    let input = Input {
        path,
        flag: "--compose-file",
        what: "compose",
    };
    let text = input.read_text(MAX_COMPOSE_BYTES)?;
    let name = if path == Path::new("-") {
        "the compose from stdin".to_owned()
    } else {
        format!("the compose file {}", input.name())
    };
    Ok(Compose {
        sha256: hippius::sha256_hex(text.as_bytes()),
        text,
        name,
    })
}

impl Compose {
    /// The POST to `route` with this compose as its only field. It changes nothing on the
    /// server, so a stored billing account is skipped rather than refused, and a failure is a
    /// read's.
    fn request(&self, route: &Route<0>, verb: &str) -> Request {
        Request::to(route, [])
            .json(&json!({ "compose": self.text }))
            .changes_nothing()
            .about(format!("{verb} {}", self.name))
    }

    /// Warns about the bytes an editor adds unseen, which hipc sends as they are and which
    /// change the hash: a byte-order mark and a carriage return. It runs before the request,
    /// so the warning stands beside a refusal of the server's that they cause.
    fn warn_of_invisible_bytes(&self, ctx: &Context<'_>) {
        if self.text.starts_with('\u{feff}') {
            ctx.warning(&format!(
                "{} starts with a byte-order mark, which hipc sends as it is",
                self.name
            ));
        }
        if self.text.contains('\r') {
            ctx.warning(&format!(
                "{} holds a carriage return, which hipc sends as it is",
                self.name
            ));
        }
    }
}

/// `POST validate/` with the compose alone. A refused compose is the server's 400, whose
/// `issues` stay in `error.details` and which text mode lists line by line.
fn validate(ctx: &Context<'_>, file: &Path) -> Result<Printed> {
    let compose = read_compose(file)?;
    compose.warn_of_invisible_bytes(ctx);
    let request = compose.request(&compute::APP_ENGINE_VALIDATE, "validate");
    show(ctx, &request, |response| {
        let parsed = response.document()?;
        check_hash(
            response,
            &parsed,
            &compose.sha256,
            Hashed::Sent(&compose.name),
        )?;
        Ok(parsed)
    })
}

/// `POST fix/` with the compose alone. The answer must hold a `fixed_compose` string whose
/// SHA-256 is its `compose_sha256`; it is then printed whole, and with `--output-file`,
/// `fixed_compose` also goes byte for byte to a new file of mode 0600.
fn fix(ctx: &Context<'_>, file: &Path, output: Option<&Path>) -> Result<Printed> {
    let compose = read_compose(file)?;
    compose.warn_of_invisible_bytes(ctx);
    let request = compose.request(&compute::APP_ENGINE_FIX, "fix");
    show(ctx, &request, |response| {
        let fixed = response.document()?;
        let Some(text) = fixed.get("fixed_compose").and_then(Value::as_str) else {
            return Err(response.refused("the API answered without a `fixed_compose` string"));
        };
        let sha256 = hippius::sha256_hex(text.as_bytes());
        check_hash(response, &fixed, &sha256, Hashed::Fixed)?;
        if let Some(output) = output {
            write_output_file(output, text.as_bytes())?;
            ctx.note(&format!(
                "wrote `fixed_compose` to {}",
                phrase::quote(output.display())
            ));
        }
        for change in semantic_changes(&fixed) {
            ctx.warning(&change);
        }
        if fixed.get("valid") == Some(&Value::Bool(false)) {
            ctx.warning(
                "`fixed_compose` is still not valid: `unresolved` lists what was not fixed",
            );
        }
        Ok(fixed)
    })
}

/// A warning for each change of a fix that alters what runs (`semantic` true), so a person
/// reviews it before using `fixed_compose`.
fn semantic_changes(fixed: &Value) -> Vec<String> {
    let Some(Value::Array(changes)) = fixed.get("changes") else {
        return Vec::new();
    };
    changes
        .iter()
        .filter(|change| change.get("semantic") == Some(&Value::Bool(true)))
        .map(|change| {
            let at = change
                .get("line")
                .and_then(Value::as_u64)
                .map_or_else(String::new, |line| format!(" at line {line}"));
            let says = change
                .get("description")
                .and_then(Value::as_str)
                .map_or_else(String::new, |text| {
                    format!(": the API says {}", phrase::quote(text))
                });
            format!("the change{at} alters what runs{says}")
        })
        .collect()
}

/// What an answer's `compose_sha256` must be the SHA-256 of.
#[derive(Clone, Copy)]
enum Hashed<'a> {
    /// The compose hipc sent, named as a message names it. A mismatch means the server read
    /// other bytes, and a launch would measure something other than the file.
    Sent(&'a str),
    /// The answer's own `fixed_compose`. A match proves only that the two fields agree, not
    /// what the server read, so a mismatch says no more than that.
    Fixed,
}

/// Refuses an answer whose `compose_sha256` is not `expected`, the SHA-256 of `hashed`. An
/// answer without the field, or with a null, is refused too, since nothing then ties the
/// answer to those bytes.
fn check_hash(
    response: &Response,
    answer: &Value,
    expected: &str,
    hashed: Hashed<'_>,
) -> Result<()> {
    let what = match hashed {
        Hashed::Sent(name) => name,
        Hashed::Fixed => "`fixed_compose`",
    };
    match answer.get("compose_sha256") {
        // `expected` is 64 lowercase hex digits, so a match is too, in either case.
        Some(Value::String(hash)) if hash.eq_ignore_ascii_case(expected) => Ok(()),
        None | Some(Value::Null) => Err(response.refused(&format!(
            "the API answered without the `compose_sha256` of {what}"
        ))),
        Some(other) => {
            let hash = other
                .as_str()
                .map_or_else(|| other.to_string(), str::to_owned);
            let hash = phrase::quote(hash);
            Err(response.refused(&match hashed {
                Hashed::Sent(_) => format!(
                    "the API hashed other bytes than {what}: its `compose_sha256` is {hash}, \
                     and the SHA-256 hipc computed is `{expected}`"
                ),
                Hashed::Fixed => format!(
                    "the API's `compose_sha256` does not match its {what}: it is {hash}, and \
                     the SHA-256 of {what} is `{expected}`"
                ),
            }))
        }
    }
}
