//! What a command runs with: the global flags, the terminal, and the steps every request
//! takes in the same order (`TENETS.md` tenet 5).
//!
//! A request that changes state goes through `--dry-run` first, then the confirmation,
//! then the network. `--debug` prints one line before each request and one after it, with
//! the method, the URL with every query value redacted, and the status, and never a header
//! or a body.

use std::cell::Cell;
use std::ffi::OsString;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::time::Duration;

use hippius::{
    BillingId, Client, ConfigFile, Error, Given, Overrides, ProjectId, Request, Response, Result,
    Selection, Settings, SideEffects, Source, Token,
};
use serde_json::Value;

use crate::cli::Global;
use crate::output::{self, Mode, Terminal};

/// The outcome of [`Context::call`].
#[derive(Debug)]
pub(crate) enum Step {
    /// The request was sent and the answer was a 2xx.
    Sent(Response),
    /// `--dry-run`: this document describes the request, and nothing was sent.
    DryRun(Value),
}

/// Whether a request needs a confirmation before it is sent.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Gate<'a> {
    /// A read, or a write the contract does not gate.
    Open,
    /// A write that runs only with `--yes` or an answered prompt. The question names the
    /// request's action ([`hippius::Request::about`]), and `consequence`, when there is
    /// one, what follows from it, such as "it is billed until it is deleted".
    Confirm { consequence: Option<&'a str> },
}

/// What a confirmation asks about (tenet 15): the action, which the prompt and both
/// refusals name, and what follows from it, which only the prompt says.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Question<'a> {
    /// A verb phrase with its literals in backticks: "delete the VM `vm_1`".
    pub(crate) action: &'a str,
    pub(crate) consequence: Option<&'a str>,
}

/// The outcome of [`Context::check_write`].
#[derive(Debug)]
pub(crate) enum Write {
    /// `--dry-run`: this document describes the request, and nothing was sent.
    DryRun(Value),
    /// The write may be confirmed.
    Checked(Checked),
}

/// A write that passed its own refusals and the token check, for the one question
/// [`Context::confirm_write`] asks. A command may read between the two, after both checks:
/// the reads that name the target in the question, or nothing (tenet 5).
#[derive(Debug)]
#[must_use = "a checked write is asked about with `Context::confirm_write`"]
pub(crate) struct Checked {
    /// The account the write acts in when it is a membership, which the question names.
    account: Option<String>,
    /// The project the write acts in when it carries one, which the question names.
    project: Option<String>,
    /// The billing account the write acts on when it carries one, which the question names.
    billing: Option<String>,
    /// The stored billing account the write goes out without, since its route takes none,
    /// which the question names too ([`hippius::Client::skipped_billing`]).
    skipped_billing: Option<String>,
}

/// The flags, the terminal, and the output mode one command runs with.
#[derive(Debug)]
pub(crate) struct Context<'a> {
    global: &'a Global,
    terminal: &'a Terminal,
    mode: Mode,
    /// The words of the command line after `hipc`, for a hint that repeats it with a flag.
    args: &'a [String],
    /// A request that is not a read got a 2xx, or failed after it may have reached the
    /// server, so the server may have changed. A failure after that, such as a stdout that
    /// cannot be written, is `partial` (tenet 4).
    wrote: Cell<bool>,
}

impl<'a> Context<'a> {
    pub(crate) fn new(
        global: &'a Global,
        terminal: &'a Terminal,
        mode: Mode,
        args: &'a [String],
    ) -> Self {
        Self {
            global,
            terminal,
            mode,
            args,
            wrote: Cell::new(false),
        }
    }

    /// Prints a `note:` line for a person on stderr: a fact about a result that succeeded
    /// (tenet 15). Text mode only, and not with `--quiet`: JSON mode stays silent on stderr
    /// except for the error object (tenet 3).
    pub(crate) fn note(&self, text: &str) {
        self.aside("note", text);
    }

    /// Prints a `warning:` line, as [`Context::note`] does: the command did what it said, but
    /// may not do what the caller expects (tenet 15).
    pub(crate) fn warning(&self, text: &str) {
        self.aside("warning", text);
    }

    /// The `hipc <words>` a note names as the next step, with the `--profile`, `--api-url`,
    /// `--account`, `--project`, and `--billing` this run was given, by flag or by variable.
    /// Without them the next command would read another profile, origin, account, project, or
    /// billing account: a variable may have been set for this one command only, and repeating an
    /// exported one is harmless. `HIPPIUS_CONFIG` has no flag to repeat, so a one-shot one is not
    /// carried over.
    ///
    /// `None` when a value is not one plain word, which a shell could split, glob, or
    /// expand, or clap read as a flag: a note then names no command rather than one that
    /// fails. A selector override that is not a valid id is left off instead: a command
    /// that never reads it, such as `hipc auth login`, does not refuse it, and the next
    /// command would exit 2 on it.
    ///
    /// `None` too when [`Context::overrides`] refuses them. A note follows a write that has
    /// already happened, and its answer may hold a secret shown only once, so a note that
    /// cannot name its command is dropped rather than turned into an error that loses the
    /// answer.
    pub(crate) fn next_command(&self, words: &str) -> Option<String> {
        let overrides = self.overrides().ok()?;
        let account = overrides
            .account
            .filter(|given| Selection::parse(&given.value, &given.source).is_ok());
        let project = overrides
            .project
            .filter(|given| ProjectId::parse(&given.value, &given.source).is_ok());
        let billing = overrides
            .billing
            .filter(|given| BillingId::parse(&given.value, &given.source).is_ok());
        command_line(
            words,
            &[
                ("--profile", overrides.profile),
                ("--api-url", overrides.api_url),
                ("--account", account),
                ("--project", project),
                ("--billing", billing),
            ],
        )
    }

    /// [`Context::next_command`] without the account, for a next command that picks one:
    /// repeating the account the run acts in would name the account being left. The project
    /// and the billing account belong to that account, so they are left off too.
    ///
    /// # Errors
    ///
    /// The refusals of [`Context::overrides`].
    pub(crate) fn next_command_without_account(&self, words: &str) -> Result<Option<String>> {
        let overrides = self.overrides()?;
        Ok(command_line(
            words,
            &[
                ("--profile", overrides.profile),
                ("--api-url", overrides.api_url),
            ],
        ))
    }

    fn aside(&self, label: &str, text: &str) {
        if matches!(self.mode, Mode::Text) && !self.global.quiet {
            output::note(&format!("{label}: {text}\n"));
        }
    }

    /// The words of the command line after `hipc`, as given: for an exit-2 hint that repeats
    /// the command with the flag it lacks ([`hippius::Error::reveal_required`]).
    pub(crate) fn args(&self) -> &[String] {
        self.args
    }

    /// Whether `hipc` takes its reader to be a program ([`Terminal::reader_is_program`]), for a
    /// default that follows the one `--output auto` picks.
    pub(crate) fn reader_is_program(&self) -> bool {
        self.terminal.reader_is_program()
    }

    /// The resolved output mode, for a command that shows a value differently to a person.
    /// The JSON document never changes with it.
    pub(crate) fn mode(&self) -> Mode {
        self.mode
    }

    /// Whether a person at a terminal can answer a prompt ([`Terminal::can_prompt`]), for a
    /// command that asks for something other than a confirmation, such as the token
    /// `hipc auth login` reads when it is not given a file.
    pub(crate) fn can_prompt(&self) -> bool {
        self.terminal.can_prompt(self.mode, self.global.no_input)
    }

    pub(crate) fn dry_run(&self) -> bool {
        self.global.dry_run
    }

    /// Whether [`Context::confirm`] asks a person, rather than taking `--yes` or refusing: a
    /// command that read something before the question can read it again after a wait.
    pub(crate) fn will_ask(&self) -> bool {
        !self.global.yes && self.can_prompt()
    }

    /// The flag and environment overrides. A flag beats its variable, and an empty
    /// variable counts as unset.
    ///
    /// # Errors
    ///
    /// `usage` when `HIPPIUS_TOKEN` holds only whitespace, or a variable that is read is
    /// not UTF-8.
    pub(crate) fn overrides(&self) -> Result<Overrides> {
        // Which of the two set a value is kept with it, so an error about the value names
        // the one to fix.
        let flag_or_env = |flag: &Option<String>, flag_name, name| -> Result<Option<Given>> {
            Ok(match flag {
                Some(value) => Some(Given {
                    value: value.clone(),
                    source: Source::Flag(flag_name),
                }),
                None => env(name)?.map(|value| Given {
                    value,
                    source: Source::Variable(name),
                }),
            })
        };
        let token = Source::Variable("HIPPIUS_TOKEN");
        Ok(Overrides {
            profile: flag_or_env(&self.global.profile, "--profile", "HIPPIUS_PROFILE")?,
            api_url: flag_or_env(&self.global.api_url, "--api-url", "HIPPIUS_API_URL")?,
            account: flag_or_env(&self.global.account, "--account", "HIPPIUS_ACCOUNT")?,
            project: flag_or_env(&self.global.project, "--project", "HIPPIUS_PROJECT")?,
            billing: flag_or_env(&self.global.billing, "--billing", "HIPPIUS_BILLING")?,
            token: env("HIPPIUS_TOKEN")?
                .map(|text| Token::new(&text, &token))
                .transpose()?,
            token_origin: env("HIPPIUS_TOKEN_ORIGIN")?.map(|value| Given {
                value,
                source: Source::Variable("HIPPIUS_TOKEN_ORIGIN"),
            }),
        })
    }

    /// The settings the config file and the overrides give, and a client for them.
    pub(crate) fn settings_and_client(&self, config: &ConfigFile) -> Result<(Settings, Client)> {
        let settings = config.settings(&self.overrides()?)?;
        let client = Client::new(&settings, Duration::from_secs(self.global.request_timeout))?;
        Ok((settings, client))
    }

    /// A client for `target` that sends no token and no selection header, for a login.
    pub(crate) fn signed_out_client(&self, target: &Given) -> Result<Client> {
        Client::signed_out(target, Duration::from_secs(self.global.request_timeout))
    }

    /// [`Context::signed_out_client`] that sends `token`, to check a token a login answered.
    pub(crate) fn signed_in_client(&self, target: &Given, token: &Token) -> Result<Client> {
        Client::signed_in(
            target,
            token,
            Duration::from_secs(self.global.request_timeout),
        )
    }

    /// A client for the settings the config file and the overrides give.
    pub(crate) fn client(&self, config: &ConfigFile) -> Result<Client> {
        self.settings_and_client(config).map(|(_, client)| client)
    }

    /// A client for routes that need a token. The token is checked when a request would
    /// be sent, not here, so argv, `--reveal`, and `--dry-run` are checked first (tenet 5).
    pub(crate) fn authed_client(&self, config: &ConfigFile) -> Result<Client> {
        self.authed(config).map(|(_, client)| client)
    }

    /// Reads the config file and builds a client for routes that need a token. A command
    /// calls it after its argv checks and `--reveal`, which come before the config file is
    /// read (tenet 5).
    pub(crate) fn load_authed_client(&self) -> Result<Client> {
        self.authed_client(&config()?)
    }

    /// [`Context::authed_client`], with the settings it resolved, for a command that also
    /// reports or uses them.
    pub(crate) fn authed(&self, config: &ConfigFile) -> Result<(Settings, Client)> {
        let (settings, client) = self.settings_and_client(config)?;
        Ok((settings, client.requiring_token()))
    }

    /// Runs `request`: describes it under `--dry-run`, asks for the confirmation `gate`
    /// names, then sends it.
    pub(crate) fn call(&self, client: &Client, request: &Request, gate: Gate<'_>) -> Result<Step> {
        match gate {
            Gate::Open if self.global.dry_run => {
                return self.dry_run_document(client, request).map(Step::DryRun);
            }
            // An open request meets the same refusals in `send`.
            Gate::Open => {}
            Gate::Confirm { consequence } => match self.check_write(client, request)? {
                Write::DryRun(doc) => return Ok(Step::DryRun(doc)),
                Write::Checked(checked) => {
                    self.confirm_write(checked, &request.action(), consequence)?;
                }
            },
        }
        self.send(client, request).map(Step::Sent)
    }

    /// The checks before a write's question: [`Context::call`] makes them for one request,
    /// and a command that reads before or after its question calls this itself. `draft` is
    /// the write as the dry run describes it, with a placeholder where a value comes from a
    /// read; it has the path of every write the command then sends, because the account the
    /// question names is `draft`'s. Under `--dry-run` this is its document. Otherwise the
    /// write's own refusals come first (a membership the route cannot use), then the token,
    /// so a confirmed write never fails on either (tenet 5).
    pub(crate) fn check_write(&self, client: &Client, draft: &Request) -> Result<Write> {
        if self.global.dry_run {
            return self.dry_run_document(client, draft).map(Write::DryRun);
        }
        let account = client.account_header(draft)?;
        let project = client.project_header(draft)?;
        let billing = client.billing_header(draft)?;
        let skipped_billing = client.skipped_billing(draft)?;
        client.check_token()?;
        Ok(Write::Checked(Checked {
            account,
            project,
            billing,
            skipped_billing,
        }))
    }

    /// Asks the question of a write [`Context::check_write`] checked: `action` is what it
    /// does, as [`hippius::Request::about`] names it. A selected membership spends or
    /// deletes in someone else's account, and a selected project or billing account in
    /// another than the Default one, so the question names each the write carries.
    pub(crate) fn confirm_write(
        &self,
        checked: Checked,
        action: &str,
        consequence: Option<&str>,
    ) -> Result<()> {
        let action = scoped(action, checked);
        self.confirm(Question {
            action: &action,
            consequence,
        })
    }

    /// The `--dry-run` document for `request`. When a person may read it
    /// (`Terminal::person_reads_stdout`), who may paste it into a log or a chat, the query
    /// values in its `url` are redacted, as in every quoted URL (tenet 9); that includes a
    /// terminal in JSON mode. On a pipe the JSON document keeps
    /// them, so an agent sees exactly what would be sent.
    pub(crate) fn describe(&self, client: &Client, request: &Request) -> Result<Value> {
        let mut doc = client.describe(request)?;
        // `dry_run` is said once, at the top of the document, not in each request.
        if let Value::Object(fields) = &mut doc {
            fields.remove("dry_run");
        }
        if self.terminal.person_reads_stdout(self.mode)
            && let Some(Value::String(url)) = doc.get_mut("url")
        {
            *url = hippius::without_query_values(url);
        }
        Ok(doc)
    }

    /// The `--dry-run` document of a command that sends `request`: `dry_run`, then
    /// `requests`, every request a real run sends, in order, each its [`Context::describe`]
    /// document; for one request that is `request` alone. A command whose real run sends more
    /// replaces `requests` with [`Context::describe_each`]. So every dry run that would send
    /// anything has `requests`, and a reader finds the requests in one place (tenet 5).
    pub(crate) fn dry_run_document(&self, client: &Client, request: &Request) -> Result<Value> {
        let mut doc = serde_json::Map::new();
        doc.insert("dry_run".to_owned(), Value::Bool(true));
        doc.insert(
            "requests".to_owned(),
            Value::Array(vec![self.describe(client, request)?]),
        );
        Ok(Value::Object(doc))
    }

    /// The `requests` list of the dry run of a command whose real run sends more than one
    /// request: the [`Context::describe`] document of each of `requests`, in the order a
    /// real run sends them, reads included. A value only a read can give is a
    /// `<...>` placeholder in the request that uses it.
    pub(crate) fn describe_each(&self, client: &Client, requests: &[&Request]) -> Result<Value> {
        requests
            .iter()
            .map(|request| self.describe(client, request))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array)
    }

    /// Sends `request` with no dry-run check and no confirmation: for the later requests of
    /// a command that already passed both, such as the cancels and the subscribe after a
    /// confirmed `hub subscription subscribe --replace`. Prints the `--debug` lines.
    pub(crate) fn send(&self, client: &Client, request: &Request) -> Result<Response> {
        assert!(
            !self.global.dry_run,
            "Context::send runs only after the caller's --dry-run check"
        );
        let url = client.url(request)?;
        if self.global.debug {
            output::note(&format!(
                "debug: {} {}\n",
                request.method(),
                hippius::without_query_values(&url)
            ));
        }
        let sent = client.send(request);
        if may_have_written(request.is_read(), &sent) {
            self.wrote.set(true);
        }
        if self.global.debug {
            let status = match &sent {
                Ok(response) => response.status().to_string(),
                Err(error) => error.http_status().map_or_else(
                    || format!("no answer ({})", error.code().as_str()),
                    |s| s.to_string(),
                ),
            };
            output::note(&format!("debug: -> {status}\n"));
        }
        sent
    }

    /// Records a write that may have been done, as [`Context::send`] does, for a test.
    #[cfg(test)]
    pub(crate) fn mark_written(&self) {
        self.wrote.set(true);
    }

    /// What this command has done to the server so far.
    pub(crate) fn side_effects(&self) -> SideEffects {
        if self.wrote.get() {
            SideEffects::Partial
        } else {
            SideEffects::None
        }
    }

    /// Asks before a secret goes to `target` when it is not the default API origin,
    /// `https://api.hippius.com` (tenet 8). The question names the origin and where it came
    /// from: a hostile `.envrc` or an injected instruction to use "that origin" is how a secret
    /// would leave. `action` reads before the origin, as in "store the token for".
    ///
    /// # Errors
    ///
    /// `usage` when `target` is not a valid API origin, and the refusals of
    /// [`Context::confirm`].
    pub(crate) fn confirm_origin(
        &self,
        target: &Given,
        action: &str,
        consequence: &str,
    ) -> Result<()> {
        if hippius::is_default_origin(target)? {
            return Ok(());
        }
        self.confirm(Question {
            action: &format!(
                "{action} {}, named by {}",
                hippius::phrase::quote(&target.value),
                target.source
            ),
            consequence: Some(consequence),
        })
    }

    /// Asks for the confirmation tenet 5 requires before the question's action runs, for a
    /// command that changes a local file and sends nothing. A command that sends uses
    /// [`Context::call`], or [`Context::check_write`] and [`Context::confirm_write`], and
    /// [`Context::confirm_origin`] before a secret goes to an origin.
    pub(crate) fn confirm(&self, question: Question<'_>) -> Result<()> {
        if self.will_ask() {
            // What was typed before the question appeared answers nothing: a `y` typed while
            // an earlier read ran, or the second line of a paste that answered an earlier
            // question, would otherwise answer one the person has not seen yet.
            crate::hidden::discard_typed_ahead();
        }
        confirm_with(
            question,
            self.global.yes,
            self.can_prompt(),
            &mut io::stdin().lock(),
            &mut output::note,
        )
    }
}

/// `action`, then the account, the project, and the billing account `checked` carries, as in
/// ``delete the VM `vm_1` in the account `acct_1` and the project `proj_1` ``, then the stored
/// billing account it goes out without. Outside a project, a charge of such a write goes to
/// the Default billing account (tenet 12), so the question says so: the caller picked another.
fn scoped(action: &str, checked: Checked) -> String {
    let in_project = checked.project.is_some();
    let scopes: Vec<String> = [
        ("account", checked.account),
        ("project", checked.project),
        ("billing account", checked.billing),
    ]
    .into_iter()
    .filter_map(|(noun, id)| id.map(|id| format!("the {noun} `{id}`")))
    .collect();
    let mut scoped = if scopes.is_empty() {
        action.to_owned()
    } else {
        format!("{action} in {}", hippius::phrase::join(&scopes, "and"))
    };
    if let Some(id) = checked.skipped_billing {
        scoped.push_str(&format!(
            ", without the stored billing account `{id}`, which this route does not take"
        ));
        if !in_project {
            scoped.push_str(", so any balance charge goes to the Default billing account");
        }
    }
    scoped
}

/// Whether a request that got `sent` may have changed the server: a write that got a 2xx, or
/// one whose failure the library marks `partial` (a timeout or a lost connection after the
/// body went out, a 5xx, a redirect). Either way a later failure of the same run must not say
/// `none`, whose hint would invite running the write again.
fn may_have_written(is_read: bool, sent: &Result<Response>) -> bool {
    !is_read
        && sent.as_ref().map_or_else(
            |error| matches!(error.side_effects(), SideEffects::Partial),
            |_| true,
        )
}

/// `hipc`, each global given in `globals` as `flag value`, then `words`; `None` when a value
/// is not one plain word (see [`Context::next_command`]).
fn command_line(words: &str, globals: &[(&str, Option<Given>)]) -> Option<String> {
    let mut command = "hipc".to_owned();
    for (flag, given) in globals {
        let Some(given) = given else { continue };
        let plain = !given.value.starts_with('-')
            && given
                .value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".:/_-".contains(&b));
        if !plain {
            return None;
        }
        command.push_str(&format!(" {flag} {}", given.value));
    }
    Some(format!("{command} {words}"))
}

/// The config path: `HIPPIUS_CONFIG`, or `$HOME/.hippius/config.toml`.
pub(crate) fn config_path() -> Result<PathBuf> {
    hippius::config_path(
        env_os("HIPPIUS_CONFIG").map(PathBuf::from),
        env_os("HOME").map(PathBuf::from),
    )
}

/// The config file, loaded for reading.
pub(crate) fn config() -> Result<ConfigFile> {
    ConfigFile::load(config_path()?)
}

/// The confirmation decision, with the answer read from `input` and the prompt written by
/// `ask`: `--yes` passes, a person at a terminal is asked and must answer yes, and
/// everyone else is refused without a question. The two refusals read differently, so a
/// caller can tell "nobody could be asked" from "the person said no" (tenet 15).
fn confirm_with(
    question: Question<'_>,
    yes: bool,
    can_prompt: bool,
    input: &mut dyn BufRead,
    ask: &mut dyn FnMut(&str),
) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !can_prompt {
        return Err(Error::confirmation_required(question.action));
    }
    let prompt = match question.consequence {
        Some(consequence) => format!("this will {}; {consequence}", question.action),
        None => format!("this will {}", question.action),
    };
    ask(&format!("{prompt}; continue? [y/N] "));
    let mut answer = String::new();
    let read = input.read_line(&mut answer);
    // A read error or end of input is not a yes.
    let yes = read.is_ok() && matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes");
    if yes {
        return Ok(());
    }
    // A refused prompt ends its line, so the `error:` line starts one of its own: end of
    // input leaves the cursor after the prompt, and so does an answer typed before the
    // prompt appeared, which the terminal echoed ahead of it. After an answer typed at the
    // prompt this leaves one blank line, the accepted cost.
    ask("\n");
    Err(Error::confirmation_declined(question.action))
}

/// An environment variable as text. Unset and empty count as unset.
///
/// # Errors
///
/// `usage` when it is not UTF-8. Treating it as unset would act on the profile's stored
/// account or token instead of the one the caller set (tenet 8 precedence).
fn env(name: &str) -> Result<Option<String>> {
    env_os(name)
        .map(|value| {
            value.into_string().map_err(|_| {
                Error::usage(
                    format!("`{name}` is not UTF-8 text"),
                    format!("set `{name}` to UTF-8 text, or unset it"),
                )
            })
        })
        .transpose()
}

pub(crate) fn env_os(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use hippius::Code;

    use super::*;

    /// A write that failed after it may have reached the server counts as done, so a later
    /// failure says `partial`; a failed read and a write refused before it went out do not.
    #[test]
    fn a_write_that_may_have_been_sent_counts_as_written() {
        let partial =
            || Err(Error::new(Code::Timeout, "x").with_side_effects(SideEffects::Partial));
        let refused = || Err(Error::new(Code::Validation, "x"));
        assert!(may_have_written(false, &partial()));
        assert!(!may_have_written(true, &partial()));
        assert!(!may_have_written(false, &refused()));
    }

    const DELETE: Question<'static> = Question {
        action: "delete the VM `vm_1`",
        consequence: None,
    };

    /// The decision for `answer`, and everything the prompt wrote.
    fn ask(
        question: Question<'_>,
        yes: bool,
        can_prompt: bool,
        input: &mut dyn BufRead,
    ) -> (Result<()>, String) {
        let mut written = String::new();
        let decision = confirm_with(question, yes, can_prompt, input, &mut |text| {
            written.push_str(text);
        });
        (decision, written)
    }

    fn decide(yes: bool, can_prompt: bool, answer: &str) -> std::result::Result<(), Code> {
        ask(DELETE, yes, can_prompt, &mut answer.as_bytes())
            .0
            .map_err(|e| e.code())
    }

    #[test]
    fn yes_always_confirms_without_a_question() {
        assert_eq!(decide(true, false, ""), Ok(()));
        assert_eq!(decide(true, true, "no\n"), Ok(()));
        assert_eq!(ask(DELETE, true, true, &mut "".as_bytes()).1, "");
    }

    #[test]
    fn without_a_terminal_it_refuses_without_asking_or_reading() {
        let (decision, written) = ask(DELETE, false, false, &mut "yes\n".as_bytes());
        let refused = decision.unwrap_err();
        assert_eq!(refused.code(), Code::ConfirmationRequired);
        assert_eq!(
            refused.message(),
            "this would delete the VM `vm_1`, and `--yes` was not passed"
        );
        assert_eq!(refused.hint(), Some("pass `--yes` to confirm"));
        assert_eq!(written, "", "nobody was asked");
    }

    #[test]
    fn at_a_terminal_only_an_explicit_yes_confirms() {
        for answer in ["y\n", "Y\n", "yes\n", " YES \n", "yes"] {
            assert_eq!(decide(false, true, answer), Ok(()), "{answer:?}");
        }
        for answer in ["", "\n", "n\n", "no\n", "yep\n", "y es\n", "sure\n"] {
            assert_eq!(
                decide(false, true, answer),
                Err(Code::ConfirmationRequired),
                "{answer:?}"
            );
        }
    }

    /// The prompt names the action and what follows from it; the refusal of a person who
    /// said no reads differently from the refusal when nobody could be asked.
    #[test]
    fn the_prompt_names_the_action_and_its_consequence_and_a_no_reads_as_declined() {
        let cancel = Question {
            action: "cancel the S3 plan",
            consequence: Some("the account is on pay as you go"),
        };
        let (decision, written) = ask(cancel, false, true, &mut "n\n".as_bytes());
        assert_eq!(
            written,
            "this will cancel the S3 plan; the account is on pay as you go; \
             continue? [y/N] \n"
        );
        let refused = decision.unwrap_err();
        assert_eq!(
            refused.message(),
            "the prompt to cancel the S3 plan was not answered yes"
        );
        assert_eq!(refused.hint(), Some("pass `--yes` to confirm"));
        let (_, written) = ask(DELETE, false, true, &mut "y\n".as_bytes());
        assert_eq!(written, "this will delete the VM `vm_1`; continue? [y/N] ");
    }

    /// A refused prompt ends its line before the `error:` line; a yes leaves it alone.
    #[test]
    fn a_refused_prompt_ends_its_line() {
        for answer in ["", "no", "no\n"] {
            let (_, written) = ask(DELETE, false, true, &mut answer.as_bytes());
            assert!(written.ends_with("[y/N] \n"), "{answer:?}: {written:?}");
        }
        let (_, written) = ask(DELETE, false, true, &mut "yes\n".as_bytes());
        assert!(written.ends_with("[y/N] "), "{written:?}");
    }

    #[test]
    fn a_read_error_is_not_a_yes() {
        struct Broken;
        impl io::Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("closed"))
            }
        }
        let mut input = io::BufReader::new(Broken);
        let (decision, written) = ask(DELETE, false, true, &mut input);
        let refused = decision.unwrap_err();
        assert_eq!(refused.code(), Code::ConfirmationRequired);
        assert_eq!(refused.hint(), Some("pass `--yes` to confirm"));
        assert!(written.ends_with('\n'), "{written:?}");
    }
}
