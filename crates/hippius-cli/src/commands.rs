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
pub(crate) mod gamma;
pub(crate) mod halpha;
pub(crate) mod hub;
pub(crate) mod list;
pub(crate) mod network;
pub(crate) mod poll;
pub(crate) mod projects;
pub(crate) mod referrals;
pub(crate) mod replace;
pub(crate) mod s3;
pub(crate) mod spend_cap;
pub(crate) mod support;

use std::io::Read;
use std::os::fd::AsFd as _;
use std::path::{Path, PathBuf};

use hippius::routes::{self, Change, Route, SettingChangeBody};
use hippius::{
    Client, ConfigFile, Error, Overrides, Page, Request, Response, Result, Source, phrase, view_of,
};
use serde_json::{Map, Value};
use zeroize::{Zeroize as _, Zeroizing};

use crate::cli::{Funding, PlanSetting, StorageCap};
use crate::context::{self, ConfirmedStep, Context, DryRun, Gate, Step, Write, WritePlan};

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
/// given, so a copied line acts where this one did, then says `why`. When one of those
/// cannot be written as typed ([`Context::next_command`]), it names the bare command.
pub(crate) fn run_hint(ctx: &Context<'_>, words: &str, why: &str) -> String {
    let command = ctx
        .next_command(words)
        .unwrap_or_else(|| format!("hipc {words}"));
    format!("run `{command}` {why}")
}

/// The error for a read, named by its action, whose answer has a shape `hipc` does not read
/// (`internal`): every command that reads a field it depends on from an answer.
pub(crate) fn unreadable(action: &str) -> Error {
    Error::internal(format!(
        "cannot {action}: the API answered with a shape `hipc` does not read"
    ))
}

/// What a successful command prints.
#[derive(Debug)]
pub(crate) enum Printed {
    /// A JSON document, rendered by the output mode.
    Document(Value),
    /// The list envelope (`TENETS.md` tenet 6) of a command that prints one, or the bare
    /// array `--all` prints: every list command, and `hipc hub subscription get`, which reads
    /// the subscriptions whole as a list. Only such a command returns it, so text mode prints a
    /// table and the `--page-token` hint for this and never for a `get` whose answer happens
    /// to hold `items` and `next_page_token`.
    List(Value),
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

/// What [`Input::open`] opened: stdin, or a regular file that passed the checks.
enum Opened {
    Stdin,
    File(std::fs::File),
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
    /// [`Input::read`], a regular file and not the config or an AWS file. The path is resolved
    /// before the target is opened, so a directory on it swapped for a symlink in between is
    /// followed; what holds is the check on the opened descriptor, which refuses the config and
    /// AWS files by device and inode whatever path reached them.
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
        let mut bytes = Vec::new();
        match self.open(path)? {
            Opened::Stdin => self.take(std::io::stdin().lock(), limit, &mut bytes)?,
            Opened::File(file) => self.take(file, limit, &mut bytes)?,
        }
        Ok(bytes)
    }

    /// Opens `path` for [`Input::read`], held to its checks.
    fn open(&self, path: &Path) -> Result<Opened> {
        if path == Path::new("-") {
            // A pipe or a terminal is never one of the two files; a redirect from one is.
            let opened = std::io::stdin()
                .as_fd()
                .try_clone_to_owned()
                .ok()
                .map(std::fs::File::from);
            self.refuse_secret_store(opened.and_then(|file| file.metadata().ok()).as_ref())?;
            return Ok(Opened::Stdin);
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
        Ok(Opened::File(file))
    }

    /// Refuses the file whose metadata is `opened` when it is the hipc config file, which holds the
    /// token, or the AWS credentials or config file, which can hold S3 keys: a file flag sends what
    /// it reads, to the API, to support staff, or into a published page. Each is matched by device
    /// and inode, so a hard link or another spelling of its path is refused too. The files at the
    /// default paths are refused as well as the ones the variables name, since either may hold a
    /// secret. A file that cannot be inspected (`None`) is not refused: the read then fails, or
    /// reads a file that is not one of them.
    fn refuse_secret_store(&self, opened: Option<&std::fs::Metadata>) -> Result<()> {
        use std::os::unix::fs::MetadataExt as _;
        let home = context::env_os("HOME").map(PathBuf::from);
        let config = [
            context::config_path().ok(),
            hippius::config_path(None, home.clone()).ok(),
        ];
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

    /// At most `limit` bytes of `reader`, read into `bytes`, refusing more.
    fn take(&self, reader: impl Read, limit: u64, bytes: &mut Vec<u8>) -> Result<()> {
        reader
            .take(limit + 1)
            .read_to_end(bytes)
            .map_err(|e| self.unreadable(&e))?;
        if bytes.len() as u64 > limit {
            return Err(self.too_large(limit));
        }
        Ok(())
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

    /// Reads a secret, at most [`MAX_SECRET_FILE_BYTES`] of UTF-8 text, from a file held to
    /// the checks of [`Input::read`] or from stdin, into a buffer that is cleared when dropped.
    /// stdin is read from the descriptor itself, not through the standard library's stdin
    /// buffer, which would keep a copy for the life of the process. Every read goes into a
    /// buffer that has its full size before the read, so a read never moves the secret and
    /// leaves a copy behind. A path that cannot be looked up is refused without being quoted
    /// ([`Input::unquoted_if_missing`]).
    pub(crate) fn read_secret(&self) -> Result<Zeroizing<String>> {
        let mut bytes = self.read_cleared(MAX_SECRET_FILE_BYTES)?;
        String::from_utf8(std::mem::take(&mut *bytes))
            .map(Zeroizing::new)
            .map_err(|refused| {
                refused.into_bytes().zeroize();
                self.not_utf8()
            })
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

    /// [`Input::read_secret`] as bytes, at most `limit`, for a document that holds a secret.
    pub(crate) fn read_cleared(&self, limit: u64) -> Result<Zeroizing<Vec<u8>>> {
        self.unquoted_if_missing()?;
        // The capacity holds the most `take` lets through, so the read never grows the buffer.
        // Cleared when dropped, so a refusal clears what was read.
        let mut bytes = Zeroizing::new(Vec::with_capacity(
            usize::try_from(limit + 1).expect("a secret's limit fits a usize"),
        ));
        match self.open(self.path)? {
            Opened::Stdin => {
                let stdin = std::io::stdin();
                self.take(crate::hidden::Unbuffered(stdin.as_fd()), limit, &mut bytes)?;
            }
            Opened::File(file) => self.take(file, limit, &mut bytes)?,
        }
        Ok(bytes)
    }

    /// [`Input::read_secret`] of stdin, for a secret that must not outlive its use, such as
    /// a hotkey phrase piped in.
    pub(crate) fn read_cleared_stdin(&self) -> Result<Zeroizing<String>> {
        debug_assert_eq!(self.path, Path::new("-"), "only stdin");
        self.read_secret()
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

impl From<DryRun> for Printed {
    fn from(dry_run: DryRun) -> Self {
        Self::Document(dry_run.into())
    }
}

/// The write a command decided on after its reads, and whether it asks about it: what
/// [`read_then_write`] sends.
#[derive(Debug)]
pub(crate) struct Decided {
    write: Request,
    /// The question's action and consequence ([`crate::context::Question`]), or `None` for a
    /// write the command's row does not ask about this time ([`Context::unasked`]).
    question: Option<(String, Option<String>)>,
}

impl Decided {
    /// `write`, asked about with its own action ([`Request::action`]) and `consequence`.
    pub(crate) fn asked(write: Request, consequence: Option<&str>) -> Self {
        let action = write.action();
        Self::asked_as(write, action, consequence)
    }

    /// `write`, asked about as `action`, which names what the reads found, with
    /// `consequence`.
    pub(crate) fn asked_as(write: Request, action: String, consequence: Option<&str>) -> Self {
        Self {
            write,
            question: Some((action, consequence.map(str::to_owned))),
        }
    }

    /// `write`, sent without a question: the command's row asks only when a condition holds,
    /// and the reads showed that it does not ([`Context::unasked`]).
    pub(crate) fn unasked(write: Request) -> Self {
        Self {
            write,
            question: None,
        }
    }
}

/// A write that reads first, in the order of tenet 5: under `--dry-run` the document of
/// `plan`, with nothing sent; otherwise the write's own refusals and the token check, then
/// `decide`, which sends the reads and returns the write and its question, then the question,
/// then the write. What `render` makes of the write's outcome is printed; it maps the write's
/// own error, which no step before it can produce.
///
/// # Errors
///
/// The refusals of [`Context::check_write`] and [`Context::confirm_write`], the error of
/// `decide`, and the error `render` returns.
pub(crate) fn read_then_write(
    ctx: &Context<'_>,
    client: &Client,
    plan: &WritePlan<'_>,
    decide: impl FnOnce() -> Result<Decided>,
    render: impl FnOnce(Result<Response>) -> Result<Value>,
) -> Result<Printed> {
    let checked = match ctx.check_write(client, plan)? {
        Write::DryRun(doc) => return Ok(doc.into()),
        Write::Checked(checked) => checked,
    };
    let Decided { write, question } = decide()?;
    let confirmed = match question {
        Some((action, consequence)) => {
            ctx.confirm_write(checked, &action, consequence.as_deref())?
        }
        None => ctx.unasked(checked),
    };
    render(ctx.send_confirmed(client, &write, &confirmed)).map(Printed::Document)
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
        Step::DryRun(doc) => Ok(doc.into()),
    }
}

/// A catalog the server owns, and the code a write takes from it (owner decision D5).
#[derive(Debug)]
pub(crate) struct Catalog<'a> {
    pub(crate) read: Request,
    pub(crate) codes: fn(&Value) -> Vec<String>,
    /// What a code names, with its article: `an S3 plan`.
    pub(crate) what: &'a str,
    /// The code the write sends.
    pub(crate) wanted: &'a str,
    /// The command that lists the catalog, for the refusal's hint.
    pub(crate) listed_by: &'a str,
}

impl Catalog<'_> {
    /// Sends the catalog read and refuses (`usage`) a [`Catalog::wanted`] it does not list,
    /// naming what it does list.
    ///
    /// # Errors
    ///
    /// The read's error, an answer that is not JSON, and the refusal.
    pub(crate) fn check(&self, ctx: &Context<'_>, client: &Client) -> Result<()> {
        let codes = (self.codes)(&ctx.send(client, &self.read)?.json()?);
        if codes.iter().any(|code| code == self.wanted) {
            return Ok(());
        }
        let offered = if codes.is_empty() {
            "the server lists none".to_owned()
        } else {
            let quoted: Vec<String> = codes.iter().map(phrase::quote).collect();
            format!("the server offers {}", phrase::join(&quoted, "and"))
        };
        Err(Error::usage(
            format!(
                "{} `{}` is not one the server offers: {offered}",
                self.what, self.wanted
            ),
            format!("run `{}` to see them", self.listed_by),
        ))
    }
}

/// Sends `write`, confirmed with `consequence`, once `catalog`, when given, lists the code it
/// sends, as `hipc hub subscription subscribe` resolves its plan: the question first, then the
/// catalog read, then the write. A code the catalog does not list is refused (`usage`) after
/// the read, and nothing is written ([`Catalog::check`]). A dry run reads nothing and lists
/// the read and the write. Without a catalog this is [`Context::call_confirmed`].
pub(crate) fn send_from_catalog(
    ctx: &Context<'_>,
    client: &Client,
    catalog: Option<&Catalog<'_>>,
    write: &Request,
    consequence: Option<&str>,
) -> Result<ConfirmedStep> {
    let Some(catalog) = catalog else {
        return ctx.call_confirmed(client, write, consequence);
    };
    let plan = WritePlan::new(&[&catalog.read], write);
    let checked = match ctx.check_write(client, &plan)? {
        Write::DryRun(doc) => return Ok(ConfirmedStep::DryRun(doc)),
        Write::Checked(checked) => checked,
    };
    let confirmed = ctx.confirm_write(checked, &write.action(), consequence)?;
    catalog.check(ctx, client)?;
    let response = ctx.send_confirmed(client, write, &confirmed)?;
    Ok(ConfirmedStep::Sent(response, confirmed))
}

/// Sends `request`, a GET of a list, and prints what `render` makes of the answer, the list
/// envelope, as a list ([`Printed::List`]), or the dry-run document.
pub(crate) fn print_list_with(
    ctx: &Context<'_>,
    client: &Client,
    request: &Request,
    render: impl FnOnce(Response) -> Result<Value>,
) -> Result<Printed> {
    match ctx.call(client, request, Gate::Open)? {
        Step::Sent(response) => render(response).map(Printed::List),
        Step::DryRun(doc) => Ok(doc.into()),
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
    print_list_with(ctx, client, request, |response| {
        hippius::page(response.json()?)?.complete_envelope(request.target())
    })
}

/// `route`, the PATCH of a Drive or S3 subscription, that keeps the plan: `--pay-from` or
/// `--storage-cap`, sent as [`SettingChangeBody`] writes it. The command's `change` group takes
/// one of them when `--plan` is absent. `service` names the plan in the question, as `Drive` or
/// `S3`. Both change what later renewals or storage cost, so both are confirmed, and the action
/// an agent reads in the refusal names the money.
pub(crate) fn plan_setting(
    route: &Route<0>,
    service: &str,
    setting: &PlanSetting,
) -> Result<(Request, Gate<'static>)> {
    let (change, action, consequence) = if let Some(funding) = setting.pay_from {
        let action = match funding {
            Funding::Credits => format!("pay each renewal of the {service} plan from the balance"),
            Funding::Card => {
                format!("charge each renewal of the {service} plan to the card saved at Stripe")
            }
        };
        let change = SettingChangeBody {
            funding: Some(funding.code()),
            ..SettingChangeBody::default()
        };
        (change, action, "it changes how every later renewal is paid")
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
            StorageCap::Bytes(bytes) => (Change::Set(bytes), format!("`{bytes}` bytes")),
            StorageCap::Default => (
                Change::Clear,
                "the default cap of twice the plan's storage".to_owned(),
            ),
        };
        let action =
            format!("bill the storage of the {service} plan past its quota, up to {up_to}");
        let change = SettingChangeBody {
            storage_cap_bytes: cap,
            ..SettingChangeBody::default()
        };
        (change, action, "storage past the quota is billed")
    };
    let request = Request::to(route, []).json(&change).about(action);
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
            fields.shift_remove(NEXT_STEP);
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
/// needs no shell quoting. It repeats the run's profile, origin, and selectors, quoted where
/// a shell needs it, and is left out when one of them is not printable ASCII
/// ([`Context::next_command`]). A field of that name in the answer is dropped first
/// ([`drop_next_step`]).
pub(crate) fn subscribed(ctx: &Context<'_>, response: &Response, service: &str) -> Result<Value> {
    let mut body = response.document()?;
    drop_next_step(&mut body);
    if response.status() != 202 {
        return Ok(body);
    }
    let checkout: routes::billing::Checkout = view_of(&body);
    let next_step = checkout
        .intent_id
        .as_deref()
        .and_then(|id| crate::cli::as_argument(id, crate::cli::intent_id))
        .and_then(|id| ctx.next_command(&format!("{service} subscription checkout {id}")));
    // The URL is not repeated here: it is on stdout, and a quoted URL would have its query
    // values, which can hold the checkout session's secret, redacted (tenet 9).
    let start = if checkout.checkout_url.is_some() {
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
/// `dry_run` first, then `action`, what it would do, then `fields`, the fields of the document
/// a real run prints, which name the profile it would change. `fields` is an object by its
/// type, so none of them can be dropped.
pub(crate) fn local_dry_run(action: &str, fields: Map<String, Value>) -> Value {
    let mut doc = object([("dry_run", true.into()), ("action", action.into())]);
    doc.extend(fields);
    Value::Object(doc)
}

/// A JSON object of `fields`, in order: a document `hipc` builds whole.
pub(crate) fn object<const N: usize>(fields: [(&str, Value); N]) -> Map<String, Value> {
    fields
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
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
    let document = |profile: String| object([("profile", profile.into()), (field, value.into())]);
    if ctx.dry_run() {
        let profile = profile(&ConfigFile::load(path)?, &overrides)?;
        return Ok(Printed::Document(local_dry_run(action, document(profile))));
    }
    let profile = ConfigFile::edit(path, |config| {
        let profile = profile(config, &overrides)?;
        set(config, &profile);
        Ok(profile)
    })?;
    Ok(Printed::Document(Value::Object(document(profile))))
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
    print_list_with(ctx, client, request, |response| {
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
/// `internal` when the answer is an object without a `key` list, has a `next` link (a later
/// page this command does not read), or has any other shape.
pub(crate) fn keyed_list(answer: Value, request: &Request, key: &str) -> Result<Value> {
    let Value::Object(body) = answer else {
        return hippius::page(answer)?.complete_envelope(request.target());
    };
    let Some(page) = keyed_page(body, key) else {
        return Err(Error::internal(format!(
            "cannot {}: the API answered without a `{key}` list",
            request.action()
        )));
    };
    page?.complete_envelope(request.target())
}

/// The list an answer `body` holds under `key`, with `body`'s other fields beside it
/// ([`hippius::page_beside`]), or `None` when `key` is not a list: [`keyed_list`], and the
/// accounts answer, which is read the same way and named in its own error.
pub(crate) fn keyed_page(mut body: Map<String, Value>, key: &str) -> Option<Result<Page>> {
    let Some(Value::Array(items)) = body.shift_remove(key) else {
        return None;
    };
    Some(hippius::page_beside(items, body))
}

/// `hipc status`: `GET /api/services/status/` (swagger `services_status_list`), the state of
/// each product, printed as sent. Swagger documents it under the API's bearer security, for
/// the signed-in user, so it needs a token as every account read does; it is not a public
/// status page. The route is in no scoped or personal list, so a membership is refused rather
/// than answered with the caller's own products (tenet 12).
pub(crate) fn status(ctx: &Context<'_>) -> Result<Printed> {
    let request =
        Request::to(&routes::support::SERVICES_STATUS, []).about("get the state of each product");
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use hippius::Code;

    use super::*;

    /// The files under `dir`, at any depth.
    fn rust_files(dir: &Path, files: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("the source tree is readable") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                rust_files(&path, files);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }

    /// A command builds every request from a route constant (`Request::to`), so only the
    /// table spells a path and the row decides its policy. Two exceptions build one from a
    /// written target: `hipc api`, which sends the route it is given, and the one dry-run
    /// stand-in a route cannot build, the cancel `hipc hub subscription subscribe --replace`
    /// may send first, whose `{subscription_id}` `Route::path` would percent-encode.
    #[test]
    fn only_hipc_api_builds_a_request_from_a_written_target() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let allowed = [("commands/api.rs", None), ("commands/hub.rs", Some(1))];
        // Built here, so this file does not hold what it looks for.
        let written = ["new", "get"].map(|constructor| format!("Request::{constructor}("));
        let mut files = Vec::new();
        rust_files(&src, &mut files);
        let counted: Vec<(String, usize, Option<usize>)> = files
            .iter()
            .map(|file| {
                let text = std::fs::read_to_string(file).expect("a source file is UTF-8");
                let count = written.iter().map(|call| text.matches(call).count()).sum();
                let name = file
                    .strip_prefix(&src)
                    .expect("under src")
                    .to_string_lossy()
                    .replace('\\', "/");
                let limit = allowed
                    .iter()
                    .find(|(allowed, _)| *allowed == name)
                    .map_or(Some(0), |&(_, limit)| limit);
                (name, count, limit)
            })
            .collect();
        let over: Vec<&(String, usize, Option<usize>)> = counted
            .iter()
            .filter(|(_, count, limit)| limit.is_some_and(|limit| *count > limit))
            .collect();
        assert_eq!(over, Vec::<&(String, usize, Option<usize>)>::new());
    }

    /// `dry_run` comes first, then `action`, then every field of the real run's document, in
    /// order: the fields are an object by their type, so none can be dropped.
    #[test]
    fn a_local_dry_run_keeps_every_field_after_its_action() {
        let fields = object([("profile", "default".into()), ("token_stored", true.into())]);
        assert_eq!(
            local_dry_run("store_token", fields).to_string(),
            r#"{"dry_run":true,"action":"store_token","profile":"default","token_stored":true}"#
        );
    }

    /// The `change` group of clap guarantees one setting; without one, the PATCH is refused
    /// as `usage` rather than a panic.
    #[test]
    fn a_plan_setting_with_nothing_to_change_is_usage() {
        let setting = PlanSetting {
            pay_from: None,
            storage_cap: None,
        };
        let error = plan_setting(&routes::s3::SUBSCRIPTION_CHANGE, "S3", &setting).unwrap_err();
        assert_eq!(error.code(), Code::Usage);
        assert_eq!(
            error.message(),
            "nothing to change in the S3 plan was given"
        );
        let card = PlanSetting {
            pay_from: Some(Funding::Card),
            storage_cap: None,
        };
        let (request, _) = plan_setting(&routes::s3::SUBSCRIPTION_CHANGE, "S3", &card).unwrap();
        assert_eq!(
            request.action(),
            "charge each renewal of the S3 plan to the card saved at Stripe"
        );
    }
}
