//! One module per command group. Each command returns what it prints, or the error that
//! ends it, and never writes output itself.

pub(crate) mod account;
pub(crate) mod api;
pub(crate) mod auth;
pub(crate) mod billing;
pub(crate) mod cdn;
pub(crate) mod compute;
pub(crate) mod credits;
pub(crate) mod drive;
pub(crate) mod files;
pub(crate) mod halpha;
pub(crate) mod hub;
pub(crate) mod list;
pub(crate) mod network;
pub(crate) mod projects;
pub(crate) mod referrals;
pub(crate) mod replace;
pub(crate) mod s3;
pub(crate) mod support;

use std::collections::HashSet;
use std::io::Read;
use std::os::fd::AsFd as _;
use std::path::{Path, PathBuf};

use hippius::{
    Client, ConfigFile, Error, Method, Overrides, Request, Response, Result, Source, phrase,
};
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::cli::{Funding, PlanSetting, StorageCap};
use crate::context::{self, Context, Gate, Step};

/// Whether `error` is the API's refusal of a call made before `route` set the account's service up:
/// a 403 whose `detail` is exactly `You must call <route> first.`, the sentence both setup routes
/// (the Hub's and the network's) send. The answer has no code of its own, so the sentence decides,
/// whole: another refusal that only names the route, such as one about who may call it, is not this
/// one, and if the wording changes the error stays as the server gave it. A 403 that carries an
/// account or grant code has that code rather than `permission`, and is left alone.
pub(crate) fn set_up_first(error: &Error, route: &str) -> bool {
    error.code() == hippius::Code::Permission
        && error.http_status() == Some(403)
        && error
            .details()
            .and_then(|body| body.get("detail"))
            .and_then(Value::as_str)
            .is_some_and(|detail| detail == format!("You must call {route} first."))
}

/// A hint that runs `hipc <words>` with the profile, origin, and selectors this run was
/// given, so a copied line acts where this one did, then says `why`. Without a plain form
/// of those ([`Context::next_command`]), it names the bare command.
pub(crate) fn run_hint(ctx: &Context<'_>, words: &str, why: &str) -> String {
    let command = ctx
        .next_command(words)
        .unwrap_or_else(|| format!("hipc {words}"));
    format!("run `{command}` {why}")
}

/// What a successful command prints.
#[derive(Debug)]
pub(crate) enum Printed {
    /// A JSON document, rendered by the output mode.
    Document(Value),
    /// Bytes hipc wrote that are the output itself, in every mode: a completion script, or
    /// the schema (which exists for programs, so it is JSON even on a terminal).
    Raw(Vec<u8>),
    /// The body `hipc api` received. Byte for byte on a pipe in JSON mode (tenet 7).
    /// Whenever a person may read stdout (text mode, or a terminal in any mode), its
    /// control and invisible characters are escaped, so a value from the server cannot
    /// write the clipboard (OSC 52) or retitle the window.
    Body(Vec<u8>),
}

/// The largest secret file read, in bytes. Tokens and S3 secrets are under 100 characters;
/// the limit is arbitrary, and stops a wrong path (a log, a disk image) from being read
/// whole.
const MAX_SECRET_FILE_BYTES: u64 = 64 * 1024;

/// A file a flag names, or stdin for `-`, and what it holds. Messages about it name the
/// flag and the file, and never quote what is in it.
#[derive(Debug)]
pub(crate) struct Input<'a> {
    pub(crate) path: &'a Path,
    /// The flag that names the file, such as `--token-file`.
    pub(crate) flag: &'static str,
    /// What the file holds, such as `token`.
    pub(crate) what: &'static str,
}

impl Input<'_> {
    /// Where the input comes from, for a message about what it holds.
    pub(crate) fn source(&self) -> Source {
        Source::File {
            flag: self.flag,
            path: self.path.to_owned(),
        }
    }

    /// The input as a message names it: `stdin` for `-`, else the path in backticks.
    pub(crate) fn name(&self) -> String {
        self.source().to_string()
    }

    /// Reads at most `limit` bytes. A path other than `-` must name a regular file: a
    /// symlink is not followed, since one to the config file would send its token, and a
    /// FIFO or a device is refused, since its read could wait for ever. The hipc config file
    /// and the AWS credentials file are refused whatever path or link names them, and so is
    /// stdin when it is one of them ([`Input::refuse_secret_store`]).
    pub(crate) fn read(&self, limit: u64) -> Result<Vec<u8>> {
        self.read_from(self.path, limit)
    }

    /// [`Input::read_text`], but a symlink is followed, for a file a dotfile manager links,
    /// such as `~/.ssh/id_ed25519.pub`: its target is held to the checks of
    /// [`Input::read`], a regular file and not the config or an AWS file.
    pub(crate) fn read_text_following_links(&self, limit: u64) -> Result<String> {
        let bytes = if self.path == Path::new("-") {
            self.read(limit)?
        } else {
            let target = std::fs::canonicalize(self.path).map_err(|e| self.unreadable(&e))?;
            self.read_from(&target, limit)?
        };
        self.utf8(bytes)
    }

    /// [`Input::read`] of `path`, the input's path or the target it links to.
    fn read_from(&self, path: &Path, limit: u64) -> Result<Vec<u8>> {
        if path == Path::new("-") {
            let stdin = std::io::stdin();
            // A pipe or a terminal is never one of the two files; a redirect from one is.
            let opened = stdin
                .as_fd()
                .try_clone_to_owned()
                .ok()
                .map(std::fs::File::from);
            self.refuse_secret_store(opened.and_then(|file| file.metadata().ok()).as_ref())?;
            return self.take(stdin.lock(), limit);
        }
        // `-` is named because `/dev/stdin` and `<(...)` are refused too: on Linux the path
        // is a symlink into `/proc`, on macOS a pipe.
        let hint = || format!("pass a regular file to `{}`, or `-` for stdin", self.flag);
        let file = hippius::open_regular(path).map_err(|e| match e {
            hippius::OpenRegularError::Symlink => Error::usage(
                format!(
                    "{} is a symlink, and a symlink is never followed",
                    self.name()
                ),
                hint(),
            ),
            hippius::OpenRegularError::NotRegular => {
                Error::usage(format!("{} is not a regular file", self.name()), hint())
            }
            hippius::OpenRegularError::Io(e) => self.unreadable(&e),
        })?;
        self.refuse_secret_store(file.metadata().ok().as_ref())?;
        self.take(file, limit)
    }

    /// [`Input::read_text`]: every file flag reads a regular file only, or stdin for `-`
    /// ([`Input::read`]).
    pub(crate) fn read_regular_text(&self, limit: u64) -> Result<String> {
        self.read_text(limit)
    }

    /// Refuses the file whose metadata is `opened` when it is the hipc config file, which
    /// holds the token, or the AWS credentials or config file, which can hold S3 keys: a file
    /// flag sends what it reads, to the API, to support staff, or into a published page. Each
    /// is matched by device and inode, so a hard link or another spelling of its path is
    /// refused too. The files at the default
    /// paths are refused as well as the ones the variables name, since either may hold a
    /// secret. A file that cannot be inspected (`None`) is not refused: the read then fails,
    /// or reads a file that is not one of them.
    fn refuse_secret_store(&self, opened: Option<&std::fs::Metadata>) -> Result<()> {
        use std::os::unix::fs::MetadataExt as _;
        let home = context::env_os("HOME").map(PathBuf::from);
        let config = [
            context::config_path().ok(),
            hippius::config_path(None, home.clone()).ok(),
        ];
        // The AWS config file can hold keys too, so it is refused with the credentials file.
        let aws = home.as_ref().map(|home| home.join(".aws"));
        let credentials = [
            context::env_os("AWS_SHARED_CREDENTIALS_FILE").map(PathBuf::from),
            aws.as_ref().map(|aws| aws.join("credentials")),
            context::env_os("AWS_CONFIG_FILE").map(PathBuf::from),
            aws.map(|aws| aws.join("config")),
        ];
        let stores = config
            .into_iter()
            .map(|path| (path, "the hipc config file, which holds the token"))
            .chain(
                credentials
                    .into_iter()
                    .map(|path| (path, "an AWS CLI file, which can hold S3 keys")),
            );
        for (path, store) in stores {
            let same = opened.is_some_and(|opened| {
                path.and_then(|path| std::fs::metadata(path).ok())
                    .is_some_and(|m| m.dev() == opened.dev() && m.ino() == opened.ino())
            });
            if same {
                return Err(Error::usage(
                    format!(
                        "{} is {store}, and `{}` never reads it",
                        self.name(),
                        self.flag
                    ),
                    format!(
                        "pass the file that holds the {} to `{}`",
                        self.what, self.flag
                    ),
                ));
            }
        }
        Ok(())
    }

    /// At most `limit` bytes of `reader`, refusing more.
    fn take(&self, reader: impl Read, limit: u64) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        reader
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| self.unreadable(&e))?;
        if bytes.len() as u64 > limit {
            return Err(self.too_large(limit));
        }
        Ok(bytes)
    }

    fn too_large(&self, limit: u64) -> Error {
        Error::usage(
            format!(
                "{} is larger than {limit} bytes, the most hipc reads for the {}",
                self.name(),
                self.what
            ),
            format!(
                "pass the file that holds the {} to `{}`",
                self.what, self.flag
            ),
        )
    }

    fn unreadable(&self, error: &std::io::Error) -> Error {
        Error::usage(
            format!(
                "cannot read the {} from {}: {}",
                self.what,
                self.name(),
                phrase::io_reason(error)
            ),
            format!("pass a file that can be read to `{}`", self.flag),
        )
    }

    /// Reads a secret, at most [`MAX_SECRET_FILE_BYTES`] of UTF-8 text. A path that cannot
    /// be looked up is refused without being quoted ([`Input::unquoted_if_missing`]).
    pub(crate) fn read_secret(&self) -> Result<String> {
        self.unquoted_if_missing()?;
        self.read_text(MAX_SECRET_FILE_BYTES)
    }

    /// Refuses a path that names no file, such as one that does not exist, without quoting
    /// it: a secret passed in place of the file that holds it is no file, and a message that
    /// quoted the path would copy the secret into a log or a transcript. stdin (`-`) passes.
    fn unquoted_if_missing(&self) -> Result<()> {
        if self.path == Path::new("-") {
            return Ok(());
        }
        match std::fs::symlink_metadata(self.path) {
            Ok(_) => Ok(()),
            Err(error) => Err(Error::usage(
                format!(
                    "cannot read the {} from the file `{}` names: {}; the path is not repeated \
                     here, in case it is the {} itself",
                    self.what,
                    self.flag,
                    phrase::io_reason(&error),
                    self.what
                ),
                format!(
                    "pass `{}` the path of a file that holds the {}",
                    self.flag, self.what
                ),
            )),
        }
    }

    /// [`Input::read_secret`] of stdin into a buffer that is cleared when dropped, for a
    /// secret that must not outlive its use, such as an access key phrase piped in. The bytes
    /// are read from the descriptor itself, not through the standard library's stdin buffer,
    /// which would keep a copy for the life of the process, into a buffer that has its full
    /// size before the read, so a read never moves the secret and leaves a copy behind.
    pub(crate) fn read_cleared_stdin(&self) -> Result<Zeroizing<String>> {
        let limit = MAX_SECRET_FILE_BYTES;
        // The capacity holds the most `take` lets through, so the read never grows the buffer.
        let mut bytes = Zeroizing::new(Vec::with_capacity(
            usize::try_from(limit + 1).expect("64 KiB fits a usize"),
        ));
        let stdin = std::io::stdin();
        let read = crate::hidden::Unbuffered(std::os::fd::AsFd::as_fd(&stdin))
            .take(limit + 1)
            .read_to_end(&mut bytes);
        read.map_err(|e| self.unreadable(&e))?;
        if bytes.len() as u64 > limit {
            return Err(self.too_large(limit));
        }
        if std::str::from_utf8(&bytes).is_err() {
            return Err(self.not_utf8());
        }
        Ok(Zeroizing::new(
            String::from_utf8(std::mem::take(&mut *bytes)).expect("checked: UTF-8"),
        ))
    }

    /// Reads at most `limit` bytes of UTF-8 text.
    pub(crate) fn read_text(&self, limit: u64) -> Result<String> {
        let bytes = self.read(limit)?;
        self.utf8(bytes)
    }

    fn utf8(&self, bytes: Vec<u8>) -> Result<String> {
        String::from_utf8(bytes).map_err(|_| self.not_utf8())
    }

    fn not_utf8(&self) -> Error {
        Error::usage(
            format!("the {} from {} is not UTF-8 text", self.what, self.name()),
            format!("pass a file of UTF-8 text to `{}`", self.flag),
        )
    }

    /// Reads at most `limit` bytes of JSON and returns the bytes unchanged, to be sent as they
    /// are, beside the value they parse to, for a command that checks the value first.
    pub(crate) fn read_json(&self, limit: u64) -> Result<(Vec<u8>, Value)> {
        let bytes = self.read(limit)?;
        let value = serde_json::from_slice(&bytes).map_err(|e| self.not_json(&e))?;
        Ok((bytes, value))
    }

    /// Reads at most `limit` bytes of JSON and returns the value, for a command that sends
    /// it inside a body of its own.
    pub(crate) fn read_value(&self, limit: u64) -> Result<Value> {
        let bytes = self.read(limit)?;
        serde_json::from_slice(&bytes).map_err(|e| self.not_json(&e))
    }

    fn not_json(&self, error: &serde_json::Error) -> Error {
        Error::usage(
            format!(
                "the {} from {} is not JSON: {error}",
                self.what,
                self.name()
            ),
            format!("pass a JSON file to `{}`", self.flag),
        )
    }
}

/// Refuses a write that may publish a private bucket unless `--public` was given: `exposure`
/// says what would become readable, as the start of the refusal. Under `--dry-run` nothing
/// is sent, so nothing is published and the flag is not needed. It comes before the config
/// file is read, beside `--reveal` (tenet 5).
pub(crate) fn require_public(ctx: &Context<'_>, public: bool, exposure: &str) -> Result<()> {
    if public || ctx.dry_run() {
        return Ok(());
    }
    Err(Error::public_required(
        format!("{exposure}, and `--public` was not passed"),
        ctx.args(),
    ))
}

/// Sends `request` through the `--dry-run` and confirmation gate, and prints what `render`
/// makes of the answer, or the dry-run document: the helper for a command that sends one
/// request and prints what it makes of the answer.
pub(crate) fn print_with(
    ctx: &Context<'_>,
    client: &Client,
    request: &Request,
    gate: Gate<'_>,
    render: impl FnOnce(Response) -> Result<Value>,
) -> Result<Printed> {
    match ctx.call(client, request, gate)? {
        Step::Sent(response) => render(response).map(Printed::Document),
        Step::DryRun(doc) => Ok(Printed::Document(doc)),
    }
}

/// Prints the server's body as it was sent. An empty 2xx is `internal`, `partial` after a
/// write: a command whose route answers 204 uses [`print_body_or`].
pub(crate) fn print_body(
    ctx: &Context<'_>,
    client: &Client,
    request: &Request,
    gate: Gate<'_>,
) -> Result<Printed> {
    print_with(ctx, client, request, gate, |response| response.document())
}

/// Prints the server's body, or `done` when the server answered with no body (204).
pub(crate) fn print_body_or(
    ctx: &Context<'_>,
    client: &Client,
    request: &Request,
    gate: Gate<'_>,
    done: Value,
) -> Result<Printed> {
    print_with(ctx, client, request, gate, |response| {
        let body = response.json()?;
        Ok(if body.is_null() { done } else { body })
    })
}

/// `request`, a GET whose answer is a list, printed as the list envelope (`TENETS.md`
/// tenet 6).
pub(crate) fn print_envelope(
    ctx: &Context<'_>,
    client: &Client,
    request: &Request,
) -> Result<Printed> {
    print_with(ctx, client, request, Gate::Open, |response| {
        hippius::page(response.json()?)?.complete_envelope(request.target())
    })
}

/// The PATCH of `path`, a Drive or S3 subscription, that keeps the plan: `--pay-from` or
/// `--storage-cap`. The command's `change` group takes one of them when `--plan` is
/// absent. `service` names the plan in the question, as `Drive` or `S3`. Both change what
/// later renewals or storage cost, so both are confirmed, and the action an agent reads in
/// the refusal names the money.
pub(crate) fn plan_setting(
    path: &str,
    service: &str,
    setting: &PlanSetting,
) -> Result<(Request, Gate<'static>)> {
    let (body, action, consequence) = if let Some(funding) = setting.pay_from {
        let action = match funding {
            Funding::Credits => format!("pay each renewal of the {service} plan from the balance"),
            Funding::Card => {
                format!("charge each renewal of the {service} plan to the card saved at Stripe")
            }
        };
        let body = json!({ "funding": funding.code() });
        (body, action, "it changes how every later renewal is paid")
    } else {
        // The `change` group takes `--plan`, `--pay-from`, or `--storage-cap`, and the
        // caller handled `--plan`; this holds if the group ever changes.
        let cap = setting.storage_cap.ok_or_else(|| {
            Error::usage(
                format!("nothing to change in the {service} plan was given"),
                "pass `--plan`, `--pay-from`, or `--storage-cap`",
            )
        })?;
        let (cap, up_to) = match cap {
            StorageCap::Bytes(bytes) => (Value::from(bytes), format!("`{bytes}` bytes")),
            StorageCap::Default => (
                Value::Null,
                "the default cap of twice the plan's storage".to_owned(),
            ),
        };
        let action =
            format!("bill the storage of the {service} plan past its quota, up to {up_to}");
        let body = json!({ "storage_cap_bytes": cap });
        (body, action, "storage past the quota is billed")
    };
    let request = Request::new(Method::Patch, path).json(&body).about(action);
    Ok((
        request,
        Gate::Confirm {
            consequence: Some(consequence),
        },
    ))
}

/// The top-level field tenet 3 reserves for the `hipc` command that goes on with what a
/// person completes in a browser. Agents are told to run it, so only `hipc` sets it, on
/// three flows: a Drive or S3 card subscribe ([`subscribed`]), `hipc billing auto-reload
/// card-setup`, and `hipc cdn hostnames domain-connect`.
pub(crate) const NEXT_STEP: &str = "next_step";

/// Removes every [`NEXT_STEP`] from `document`, a server's answer, at any depth, a list's
/// items included: a hostile or broken origin could put a command there, `--yes`
/// included, that an agent would run. The run prints every document through this, but for
/// the three flows that set the field, which call it on the answer before they add their
/// own.
pub(crate) fn drop_next_step(document: &mut Value) {
    match document {
        Value::Object(fields) => {
            fields.remove(NEXT_STEP);
            fields.values_mut().for_each(drop_next_step);
        }
        Value::Array(items) => items.iter_mut().for_each(drop_next_step),
        _ => {}
    }
}

/// The document of a Drive or S3 subscribe: the server's body. A 202 is a card checkout
/// (both services answer the same shape), which waits for a person to pay: its document
/// gains `next_step`, the command that reads the checkout's progress, and text mode gets a
/// note that nothing is bought until the checkout is paid.
///
/// `next_step` is the field tenet 3 allows on a document that waits for a person:
/// JSON mode prints no note, so without it an agent would not learn how the flow goes on.
/// It is always a `hipc` command that runs as printed, so it is added only for an
/// `intent_id` the checkout command accepts and that cannot be read as a flag; such an id
/// needs no shell quoting. It repeats the run's profile, origin, and selectors, and is left
/// out when one of them is not a plain word ([`Context::next_command`]). A field of that
/// name in the answer is dropped first ([`drop_next_step`]).
pub(crate) fn subscribed(ctx: &Context<'_>, response: &Response, service: &str) -> Result<Value> {
    let mut body = response.document()?;
    drop_next_step(&mut body);
    if response.status() != 202 {
        return Ok(body);
    }
    let next_step = body
        .get("intent_id")
        .and_then(Value::as_str)
        .and_then(|id| crate::cli::as_argument(id, crate::cli::intent_id))
        .and_then(|id| ctx.next_command(&format!("{service} subscription checkout {id}")));
    // The URL is not repeated here: it is on stdout, and a quoted URL would have its query
    // values, which can hold the checkout session's secret, redacted (tenet 9).
    let start = if body.get("checkout_url").and_then(Value::as_str).is_some() {
        "nothing has been bought yet: the plan starts once the checkout at `checkout_url` in \
         the output is paid"
    } else {
        "nothing has been bought yet: the card checkout has started"
    };
    match (next_step, &mut body) {
        (Some(command), Value::Object(fields)) => {
            ctx.note(&format!("{start}; `{command}` shows its progress"));
            fields.insert(NEXT_STEP.to_owned(), Value::from(command));
        }
        _ => ctx.note(start),
    }
    Ok(body)
}

/// The `--dry-run` document of a command that changes a local file and sends nothing:
/// `dry_run` first, then `action`, what it would do, then the fields of the document a real
/// run prints, which name the profile it would change.
pub(crate) fn local_dry_run(action: &str, fields: Value) -> Value {
    let mut doc = serde_json::Map::new();
    doc.insert("dry_run".to_owned(), Value::Bool(true));
    doc.insert("action".to_owned(), action.into());
    if let Value::Object(fields) = fields {
        doc.extend(fields);
    }
    Value::Object(doc)
}

/// Stores a selection in the profile with `set`, and prints `{"profile", <field>: <value>}`:
/// `hipc account use`, `hipc account projects use`, and `hipc account billing-accounts use`. No
/// request is sent; a dry run writes nothing, and its `action` is `action`. `profile` picks
/// the profile from the file and the overrides, or refuses: [`ConfigFile::profile_name`], or
/// [`ConfigFile::selection_profile`] for a project or billing account.
pub(crate) fn store_selection(
    ctx: &Context<'_>,
    action: &str,
    (field, value): (&str, &str),
    profile: fn(&ConfigFile, &Overrides) -> Result<String>,
    set: impl FnOnce(&mut ConfigFile, &str),
) -> Result<Printed> {
    let path = context::config_path()?;
    let overrides = ctx.overrides()?;
    if ctx.dry_run() {
        let profile = profile(&ConfigFile::load(path)?, &overrides)?;
        return Ok(Printed::Document(local_dry_run(
            action,
            json!({ "profile": profile, field: value }),
        )));
    }
    let profile = ConfigFile::edit(path, |config| {
        let profile = profile(config, &overrides)?;
        set(config, &profile);
        Ok(profile)
    })?;
    Ok(Printed::Document(
        json!({ "profile": profile, field: value }),
    ))
}

/// The `--dry-run` document of a command that would send several requests: `dry_run`
/// first, then the requests of each dry-run document in `documents`, in order. Each comes
/// from [`Context::call`], whose `requests` names only its own request.
pub(crate) fn dry_run_requests(documents: Vec<Value>) -> Value {
    let requests: Vec<Value> = documents
        .into_iter()
        .flat_map(|mut doc| {
            doc["requests"]
                .as_array_mut()
                .map(std::mem::take)
                .unwrap_or_default()
        })
        .collect();
    let mut doc = serde_json::Map::new();
    doc.insert("dry_run".to_owned(), Value::Bool(true));
    doc.insert("requests".to_owned(), Value::Array(requests));
    Value::Object(doc)
}

/// One failed part of a partial failure, for `error.details`: the part's name under `key`,
/// then its error's `code`, `http_status`, `message`, and `hint`. Text mode prints each
/// part as `error: <name>: <message>`, reading the name from `service` or `step`, the two
/// keys the callers pass.
pub(crate) fn failed_part(key: &str, name: &str, error: &Error) -> Value {
    let mut part = serde_json::Map::new();
    part.insert(key.to_owned(), name.into());
    part.insert("code".to_owned(), error.code().as_str().into());
    part.insert("http_status".to_owned(), error.http_status().into());
    part.insert("message".to_owned(), error.message().into());
    part.insert("hint".to_owned(), error.hint().into());
    Value::Object(part)
}

/// `request`, a GET whose list the API answers inside an object, under `key`, printed by
/// [`keyed_list`].
pub(crate) fn print_keyed_list(
    ctx: &Context<'_>,
    client: &Client,
    request: &Request,
    key: &str,
) -> Result<Printed> {
    print_with(ctx, client, request, Gate::Open, |response| {
        keyed_list(response.json()?, request, key)
    })
}

/// `request`'s answer, a list answered as an object that holds the items under `key`, such as
/// `{buckets, count, ...}` or `{zones, beta}`: the envelope, with the object's other
/// fields kept beside `items` ([`hippius::page_beside`]). A bare array is read as the
/// list too, for a route whose swagger documents one. No such route pages, so the list is
/// read whole (tenet 6).
///
/// # Errors
///
/// `internal` for an object without a `key` list, for a `next` link (a later page this
/// command does not read), or for any other shape.
pub(crate) fn keyed_list(answer: Value, request: &Request, key: &str) -> Result<Value> {
    let Value::Object(mut body) = answer else {
        return hippius::page(answer)?.complete_envelope(request.target());
    };
    let Some(Value::Array(items)) = body.remove(key) else {
        return Err(Error::internal(format!(
            "cannot {}: the API answered without a `{key}` list",
            request.action()
        )));
    };
    hippius::page_beside(items, body)?.complete_envelope(request.target())
}

/// `hipc status`: `GET /api/services/status/` (swagger `services_status_list`), the state of
/// each product, printed as sent. Swagger documents it under the API's bearer security, for
/// the signed-in user, so it needs a token as every account read does; it is not a public
/// status page. The route is in no scoped or personal list, so a membership is refused rather
/// than answered with the caller's own products (tenet 12).
pub(crate) fn status(ctx: &Context<'_>) -> Result<Printed> {
    let request = Request::get("/services/status/").about("get the state of each product");
    print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
}

/// Refuses a call that returns a secret unless `--reveal` was given (tenet 9).
/// Under `--dry-run` nothing is sent, so nothing is revealed and the flag is not needed.
pub(crate) fn require_reveal(ctx: &Context<'_>, reveal: bool) -> Result<()> {
    require_reveal_or_file(ctx, reveal, false)
}

/// [`require_reveal`] for a command that can write the secret to new private files the
/// caller named (`--secret-file`, `--credentials-dir`): naming them asks for the secret as
/// plainly as `--reveal` does, and keeps it off stdout, so `to_file` counts as the reveal.
pub(crate) fn require_reveal_or_file(ctx: &Context<'_>, reveal: bool, to_file: bool) -> Result<()> {
    if reveal || to_file || ctx.dry_run() {
        Ok(())
    } else {
        Err(Error::reveal_required(ctx.args()))
    }
}

/// The most pages one command follows. Arbitrary: far more than any account's list holds
/// at the API's page sizes, and it stops a server that keeps answering a new `next` from
/// turning one command into a request storm and an unbounded buffer.
const MAX_PAGES: usize = 1000;

/// The most bytes of answers one list keeps across its pages (`s3 tokens list` reads two
/// lists, each held to this). Arbitrary: every page is
/// held until the list ends, and each may be up to the client's 32 MiB body cap, so the
/// page cap alone would allow gigabytes. No account's list comes near this.
const MAX_LIST_BYTES: usize = 64 * 1024 * 1024;

/// The pages one list has followed. Every page loop reports each page it read and admits
/// each `next` through it, so a link back to a page already read, a page past
/// [`MAX_PAGES`], or a list past [`MAX_LIST_BYTES`] is refused the same way everywhere.
#[derive(Debug)]
pub(crate) struct Pages {
    seen: HashSet<String>,
    bytes: usize,
}

impl Pages {
    /// A list whose first page is `first`: a page number, a cursor, or a URL for `hipc api`.
    pub(crate) fn starting_at(first: &str) -> Self {
        Self {
            seen: HashSet::from([first.to_owned()]),
            bytes: 0,
        }
    }

    /// Counts a page of `bytes` that was read.
    ///
    /// # Errors
    ///
    /// `internal` once the list holds more than [`MAX_LIST_BYTES`].
    pub(crate) fn read(&mut self, bytes: usize) -> Result<()> {
        self.bytes = self.bytes.saturating_add(bytes);
        if self.bytes > MAX_LIST_BYTES {
            return Err(Error::internal(format!(
                "the list is larger than {} MiB across its pages, so reading stopped",
                MAX_LIST_BYTES / (1024 * 1024)
            )));
        }
        Ok(())
    }

    /// Admits the next page.
    ///
    /// # Errors
    ///
    /// `internal` for a page already read (a loop) or one past [`MAX_PAGES`].
    pub(crate) fn admit(&mut self, next: &str) -> Result<()> {
        if self.seen.len() >= MAX_PAGES {
            return Err(Error::internal(format!(
                "the list goes on past {MAX_PAGES} pages, so reading stopped"
            )));
        }
        if !self.seen.insert(next.to_owned()) {
            // A URL (`hipc api`'s next link) shows no query values (tenet 9); a page number or
            // an opaque cursor is data, quoted whole, since cutting it at a `?` or `#` would
            // name another token.
            let shown = if next.contains("://") {
                hippius::without_query_values(next)
            } else {
                next.to_owned()
            };
            return Err(Error::internal(format!(
                "the API's next page leads back to {}, which was already read",
                phrase::quote(shown)
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use hippius::Code;

    use super::*;

    /// The `change` group of clap guarantees one setting; without one, the PATCH is refused
    /// as `usage` rather than a panic.
    #[test]
    fn a_plan_setting_with_nothing_to_change_is_usage() {
        let setting = PlanSetting {
            pay_from: None,
            storage_cap: None,
        };
        let error = plan_setting("/s3/subscription/", "S3", &setting).unwrap_err();
        assert_eq!(error.code(), Code::Usage);
        assert_eq!(
            error.message(),
            "nothing to change in the S3 plan was given"
        );
    }

    #[test]
    fn pages_refuse_a_loop_a_page_past_the_cap_and_a_list_past_its_bytes() {
        let mut pages = Pages::starting_at("1");
        assert_eq!(pages.admit("1").unwrap_err().code(), Code::Internal);
        pages.admit("2").unwrap();
        pages.read(MAX_LIST_BYTES).unwrap();
        assert_eq!(pages.read(1).unwrap_err().code(), Code::Internal);
        let mut pages = Pages::starting_at("1");
        for n in 2..=MAX_PAGES {
            pages.admit(&n.to_string()).unwrap();
        }
        assert_eq!(pages.admit("x").unwrap_err().code(), Code::Internal);
    }

    #[test]
    fn a_loop_quotes_a_cursor_whole_and_a_url_without_its_query_values() {
        let looped = |next: &str| {
            let mut pages = Pages::starting_at(next);
            pages.admit(next).unwrap_err().message().to_owned()
        };
        assert_eq!(
            looped("a?b=c#d`e\n"),
            "the API's next page leads back to `a?b=c#d\\u{60}e\\n`, which was already read"
        );
        assert_eq!(
            looped("https://api.example/x/?page=2&key=s3cr3t"),
            "the API's next page leads back to `https://api.example/x/?page=<redacted>&key=<redacted>`, which was already read"
        );
    }
}
