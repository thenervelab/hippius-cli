//! Every byte `hipc` writes to stdout and stderr goes through this module
//! (`TENETS.md` tenets 3 and 4).
//!
//! Success: the document on stdout. stderr holds only what a caller asked for or can use:
//! `--debug` lines, a prompt, and in text mode the notes from `Context::note`. Failure: in
//! JSON mode exactly one error object on stderr (after any `--debug` lines), in text mode
//! a short paragraph on stderr, and stdout empty, except the documented cases where
//! `Error::stdout` holds what was done (`TENETS.md` tenet 3).
//!
//! Text written for a person never carries a raw control character, nor a character that
//! changes how a line reads (`text::changes_how_text_reads`: bidi, zero-width, fillers):
//! a value cannot move the cursor, recolor the terminal, or disguise a name. JSON writes
//! the same characters as `\uXXXX`, except in an error's `message` and `hint`, which hold
//! the text-mode escapes, so they read exactly as the text-mode lines do (tenet 15). `hipc
//! api` prints the body byte for byte on a pipe in JSON mode (tenet 7), and escaped
//! whenever a person may read it (tenet 15).
//!
//! Output is written with `write_all`, not `println!`: `println!` panics when the reader
//! has closed the pipe, which would turn `hipc ... | head -1` into exit 101 and a panic
//! message.

use std::ffi::{OsStr, OsString};
use std::io::{self, IsTerminal as _, Write as _};
use std::process::ExitCode;

use clap::error::{ContextKind, ContextValue, ErrorKind};
use hippius::{Code, Error, SideEffects, phrase};
use serde_json::Value;

use crate::cli::{Global, OutputArg};

/// Whether a character changes how a line reads without being a control character, for a
/// value parser that refuses a name a person could not tell from another.
pub(crate) use text::changes_how_text_reads;

/// The resolved output mode.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Mode {
    Text,
    Json,
}

/// What the process environment says about the output mode and prompting: whether a CI
/// or agent variable is set, and whether stdin, stdout, and stderr are terminals.
#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is one independent fact about the process, read once at start"
)]
pub(crate) struct Terminal {
    /// A program, not a person, runs `hipc`, as `CI` or `AI_AGENT` says (`is_automation`).
    automation: bool,
    stdin_tty: bool,
    stdout_tty: bool,
    /// The prompt is written to stderr, so a person sees it only there.
    stderr_tty: bool,
}

impl Terminal {
    /// Reads `CI`, `AI_AGENT`, stdin, stdout, and stderr from the running process.
    pub(crate) fn detect() -> Self {
        Self {
            automation: is_automation(
                std::env::var_os("CI").as_deref(),
                std::env::var_os("AI_AGENT").as_deref(),
            ),
            stdin_tty: io::stdin().is_terminal(),
            stdout_tty: io::stdout().is_terminal(),
            stderr_tty: io::stderr().is_terminal(),
        }
    }

    /// Whether a person can answer a prompt: the output mode is text, stdin, stdout, and
    /// stderr are terminals, no CI or agent variable is set, and `--no-input` was not
    /// passed (tenets 5 and 15). JSON mode never prompts, so an agent that runs `hipc
    /// --json` inside a pty gets a refusal, not a hang, and a prompt never goes to a stderr
    /// redirected to a file, where nobody would see it while the process waits.
    pub(crate) fn can_prompt(&self, mode: Mode, no_input: bool) -> bool {
        let terminals = self.stdin_tty && self.stdout_tty && self.stderr_tty;
        matches!(mode, Mode::Text) && terminals && !self.automation && !no_input
    }

    /// Whether a person may read stdout: text mode, or stdout is a terminal in any mode.
    /// What a person may read is escaped and redacted (tenet 15): `AI_AGENT` or `--json`
    /// can put a person's terminal in JSON mode, and only a pipe keeps raw bytes and query
    /// values, for the program reading it.
    pub(crate) fn person_reads_stdout(&self, mode: Mode) -> bool {
        matches!(mode, Mode::Text) || self.stdout_tty
    }

    /// Whether `hipc` takes its reader to be a program: stdout is not a terminal, or a CI or
    /// agent variable is set (tenet 3). This one rule picks JSON for `--output auto` and
    /// the launch answer without a wait for `hipc compute vms create`.
    pub(crate) fn reader_is_program(&self) -> bool {
        self.automation || !self.stdout_tty
    }

    /// The mode for the parsed global flags.
    pub(crate) fn resolve(&self, global: &Global) -> Mode {
        self.mode(if global.json {
            OutputArg::Json
        } else {
            global.output
        })
    }

    fn mode(&self, output: OutputArg) -> Mode {
        let json = match output {
            OutputArg::Json => true,
            OutputArg::Text => false,
            OutputArg::Auto => self.reader_is_program(),
        };
        if json { Mode::Json } else { Mode::Text }
    }

    /// The mode read from raw argv, for errors raised before clap has parsed it. It
    /// follows clap's reading of `--json`, `--output <v>` and `--output=<v>` up to a `--`,
    /// so a command line clap accepts gets the same mode either way (a test checks this).
    /// For a command line clap rejects, it picks the mode its error is printed in.
    pub(crate) fn resolve_from_args(&self, args: &[OsString]) -> Mode {
        let mut output = OutputArg::Auto;
        let mut json = false;
        let mut words = args.iter().skip(1).map(|a| a.to_str()).peekable();
        while let Some(word) = words.next() {
            let value = match word {
                Some("--") => break,
                Some("--json") => {
                    json = true;
                    continue;
                }
                // clap does not take a word that starts with `-` as a value, so neither
                // does this scan. A word that is not UTF-8 is taken, and matches nothing.
                Some("--output") => match words.peek() {
                    Some(Some(next)) if next.starts_with('-') => None,
                    Some(_) => words.next().flatten(),
                    None => None,
                },
                Some(word) => word.strip_prefix("--output="),
                None => None,
            };
            output = match value {
                Some("json") => OutputArg::Json,
                Some("text") => OutputArg::Text,
                Some("auto") => OutputArg::Auto,
                _ => output,
            };
        }
        self.mode(if json { OutputArg::Json } else { output })
    }
}

/// Whether the environment, `CI` and `AI_AGENT`, says a program runs `hipc` (tenets 3
/// and 15).
///
/// `CI` counts when it is set to any value, empty and `false` included. `AI_AGENT` counts
/// unless its value, trimmed and in any case, is empty, `0`, `false`, `no`, or `off`: the
/// consumer rule of <https://github.com/agentsmd/agents.md/issues/136>. A value that is not
/// UTF-8 cannot be one of those words, so it counts.
///
/// The variable is an unauthenticated hint, so it may only make `hipc` stricter: it picks
/// JSON, refuses prompts, and launches a VM without the wait, and never confirms
/// or reveals anything. `CLAUDECODE` is not read: an editor extension sets it in its
/// integrated terminal, where a person types. The generic `AGENT` is not read either:
/// the agents.md issue finds it read four incompatible ways, and set by unrelated tools
/// (`AGENT=ssh-agent`).
fn is_automation(ci: Option<&OsStr>, ai_agent: Option<&OsStr>) -> bool {
    let ai_agent_set = ai_agent.is_some_and(|value| {
        let value = value.to_string_lossy();
        let value = value.trim();
        !value.is_empty()
            && !["0", "false", "no", "off"]
                .iter()
                .any(|off| value.eq_ignore_ascii_case(off))
    });
    ci.is_some() || ai_agent_set
}

/// How text mode lays a success document out. The command decides, never the document's
/// shape: a server's answer to a `get` may hold `items` and `next_page_token`, or `dry_run`
/// and `requests`, and is still printed as one object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    /// One object as a key-value block, a list of objects as a table.
    Document,
    /// The document of a run given `--dry-run`: one with `requests` is printed as a dry run.
    DryRun,
    /// A list command's list envelope: a table of `items`, the fields beside it, and the
    /// `--page-token` hint for the next page ([`crate::commands::Printed::List`]).
    List,
}

/// Prints a success document and returns exit 0. `done` is what the command did to the
/// server, which a failure to write the document must still report. `layout` is how text
/// mode lays it out.
pub(crate) fn document(
    mode: Mode,
    document: &Value,
    done: SideEffects,
    layout: Layout,
) -> ExitCode {
    stdout(mode, render(mode, document, layout).as_bytes(), done)
}

/// Prints bytes that are the output itself in every mode, such as a completion script, and
/// returns exit 0.
pub(crate) fn raw(mode: Mode, bytes: &[u8], done: SideEffects) -> ExitCode {
    stdout(mode, bytes, done)
}

/// Prints a server body for `hipc api`: the bytes on a pipe in JSON mode, and otherwise
/// (text mode, or any mode when stdout is a terminal) the text with every control
/// character but line breaks and tabs, and every character `text::changes_how_text_reads`
/// names, escaped. Invalid UTF-8 prints as U+FFFD there.
pub(crate) fn body(terminal: &Terminal, mode: Mode, bytes: &[u8], done: SideEffects) -> ExitCode {
    if !terminal.person_reads_stdout(mode) {
        return stdout(mode, bytes, done);
    }
    stdout(mode, escape_body(mode, bytes).as_bytes(), done)
}

/// A server body as a person may read it, ending in a line break unless it is empty. Text
/// mode writes Rust escapes (`\u{1b}`), as every text-mode value does. JSON mode writes
/// `\uXXXX`, the escape a JSON string accepts: inside a string it reads back as the same
/// character.
/// Outside one, the only such character valid JSON can hold is a carriage return between
/// tokens, which shows as `\u000d` rather than move the cursor back over the line.
fn escape_body(mode: Mode, bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut escaped = String::with_capacity(text.len() + 1);
    for c in text.chars() {
        let hidden = (c.is_control() && c != '\n' && c != '\t') || text::changes_how_text_reads(c);
        match (hidden, mode) {
            (false, _) => escaped.push(c),
            (true, Mode::Json) => push_json_escape(&mut escaped, c),
            (true, Mode::Text) => escaped.extend(c.escape_default()),
        }
    }
    // A terminal's prompt would start on the body's last line otherwise.
    if !escaped.is_empty() && !escaped.ends_with('\n') {
        escaped.push('\n');
    }
    escaped
}

/// Prints one diagnostic line on stderr: a `--debug` request line, a prompt, or a
/// text-mode note from `Context::note`. Everything but one trailing line break is
/// escaped, so server text inside a note (a checkout URL, a `next` link) cannot start a
/// line of its own.
pub(crate) fn note(text: &str) {
    let (body, end) = text.strip_suffix('\n').map_or((text, ""), |b| (b, "\n"));
    stderr(&format!("{}{end}", text::escape(body)));
}

/// Prints a failure and returns its exit code.
pub(crate) fn failure(mode: Mode, error: &Error) -> ExitCode {
    failure_warned(mode, error, &[])
}

/// [`failure`], with the run's `warnings`, which JSON mode adds to the error object as a
/// `warnings` array of their texts ([`add_warnings`]); text mode has printed them already.
///
/// A failure with a record of what was done prints it on stdout first, so no id or row is
/// lost (tenet 3). When that write fails, as [`stdout`] treats a document it cannot write,
/// the failure printed is `internal` (tenet 15), naming the failure it replaces, so the
/// caller knows the record is lost; a closed pipe is the reader's choice and changes nothing.
pub(crate) fn failure_warned(mode: Mode, error: &Error, warnings: &[String]) -> ExitCode {
    let lost = done_on_failure(error).and_then(|done| {
        let layout = if error.stdout_is_list() {
            Layout::List
        } else {
            Layout::Document
        };
        write_stdout(render(mode, &done, layout).as_bytes())
            .err()
            .filter(|lost| lost.kind() != io::ErrorKind::BrokenPipe)
    });
    let replacement = lost.map(|lost| {
        Error::internal(format!(
            "cannot write to stdout the record of what was done: {}; the command had failed: {}",
            phrase::io_reason(&lost),
            error.message()
        ))
        .with_side_effects(error.side_effects())
    });
    let printed = replacement.as_ref().unwrap_or(error);
    stderr(&render_failure(mode, printed, warnings));
    ExitCode::from(printed.code().exit_code())
}

/// Adds the run's `warnings` to a JSON-mode document as a top-level `warnings` array of their
/// texts, the `warning:` lines text mode writes on stderr. A document only gains fields
/// (tenet 15), and the array is there only when there is a warning. A document that is not
/// an object, such as the array `--all` prints, has no place for a field and keeps its shape
/// (tenet 3), so its warnings go to stderr ([`warnings_aside`]).
pub(crate) fn add_warnings(mode: Mode, document: &mut Value, warnings: &[String]) {
    match document {
        Value::Object(fields) if matches!(mode, Mode::Json) && !warnings.is_empty() => {
            fields.insert("warnings".to_owned(), warnings.into());
        }
        _ => warnings_aside(mode, warnings),
    }
}

/// Prints the run's `warnings` for a JSON-mode result that has no place for a field (an
/// array, or a body printed as the server sent it) on stderr, as one object, `{"warnings":
/// [...]}`, the form they take beside `error` on a failure. A config warning applies to every
/// command, so any result can carry one. Text mode has printed them already.
pub(crate) fn warnings_aside(mode: Mode, warnings: &[String]) {
    if matches!(mode, Mode::Json) && !warnings.is_empty() {
        stderr(&format!(
            "{}\n",
            json_text(&serde_json::json!({ "warnings": warnings }))
        ));
    }
}

/// What a failure prints on stdout: the document of what was done, which can hold a server
/// answer, such as the VM a launch made before its wait failed, without a `next_step` the
/// server put there ([`crate::commands::drop_next_step`]).
fn done_on_failure(error: &Error) -> Option<Value> {
    let mut done = error.stdout()?.clone();
    crate::commands::drop_next_step(&mut done);
    Some(done)
}

/// The length, in characters, from which a word a usage error would quote is not repeated:
/// a console token is 40 characters and no command name is 20, so the cut sits between them.
const TOKEN_SHAPED_CHARS: usize = 20;

/// Prints a clap parse result that is not a successful parse: help and version go to
/// stdout with exit 0, a real argv error is a `usage` failure, printed like every other
/// failure (`error:` and `hint:` in text mode, one object in JSON mode), with a hint that
/// names the help of the command the caller was typing.
///
/// A command line that stops before a subcommand (`hipc`, `hipc --json`, `hipc compute
/// --debug`) is `a subcommand is required`, whichever of clap's two kinds says so. In text
/// mode it gets the long help of that command after its `error:` and `hint:` lines and a
/// blank line: the person who typed it wants the list of commands (tenet 4).
pub(crate) fn clap_error(mode: Mode, error: &clap::Error, args: &[OsString]) -> ExitCode {
    if !error.use_stderr() {
        let rendered = error.render().to_string();
        return raw(mode, rendered.as_bytes(), SideEffects::None);
    }
    let (path, mut command) = typed_command(args);
    // A word the caller gave that is too long to be a command name or a value from a fixed
    // list may be a token pasted in the wrong place, so a refusal of it as unknown never
    // repeats it: clap's message, and [`usage_message`], would quote it. A word shorter than that, such as a mistyped command
    // name, still gets the usual message and its nearest command. clap's own name for an
    // argument (`--name <NAME>`, `<VM_ID>`) is not the caller's word.
    // Only the errors that refuse a word as unknown; a validator's refusal is its own message,
    // which says what was wrong without the value where the value may be a secret.
    let refuses_a_word = matches!(
        error.kind(),
        ErrorKind::UnknownArgument | ErrorKind::InvalidValue | ErrorKind::InvalidSubcommand
    );
    let token_shaped =
        |word: &str| !word.starts_with(['-', '<']) && word.chars().count() >= TOKEN_SHAPED_CHARS;
    let quotes_a_token = [
        ContextKind::InvalidArg,
        ContextKind::InvalidValue,
        ContextKind::InvalidSubcommand,
    ]
    .into_iter()
    .any(|kind| matches!(error.get(kind), Some(ContextValue::String(word)) if token_shaped(word)));
    if refuses_a_word && quotes_a_token {
        // No `hipc auth` command takes a word, so the token goes to a login there; elsewhere
        // the word may be a value the command takes, whose form its help gives.
        let auth = path == "hipc auth" || path.starts_with("hipc auth ");
        let hint = if auth {
            "pass the token with `hipc auth login --token-file <path>` or at its prompt, never \
             on the command line"
                .to_owned()
        } else {
            format!("run `{path} --help`")
        };
        return failure(
            mode,
            &Error::usage(
                format!(
                    "`{path}` takes no such argument or value, so the word given is not \
                     repeated here; if it is a token, treat it as exposed to anyone who can \
                     read this machine's process list or shell history"
                ),
                hint,
            ),
        );
    }
    let missing_subcommand = is_missing_subcommand(error);
    let unknown = unknown_subcommand(error);
    let message = if missing_subcommand {
        format!(
            "a subcommand is required; `{path}` holds {}",
            crate::spec::held(&command)
        )
    } else if let Some(word) = unknown.filter(|_| command.has_subcommands()) {
        let what = if word.is_empty() {
            "with an empty name".to_owned()
        } else {
            hippius::phrase::quote(word)
        };
        format!(
            "`{path}` has no command {what}; it holds {}",
            crate::spec::held(&command)
        )
    } else {
        usage_message(error)
    };
    let hint = unknown
        .and_then(|word| nearest_hint(&path, &command, word))
        .unwrap_or_else(|| format!("run `{path} --help`"));
    let code = failure(mode, &Error::usage(message, hint));
    if missing_subcommand && matches!(mode, Mode::Text) {
        let help = command.render_long_help().to_string();
        stderr(&format!("\n{}", escape_lines(&help)));
    }
    code
}

/// The word of an unknown subcommand, as typed, when that is what clap refused.
fn unknown_subcommand(error: &clap::Error) -> Option<&str> {
    match error.get(ContextKind::InvalidSubcommand) {
        Some(ContextValue::String(word)) if error.kind() == ErrorKind::InvalidSubcommand => {
            Some(word.as_str())
        }
        _ => None,
    }
}

/// The hint for an unknown subcommand `word` under `path` (`hipc compute`), `group` being
/// that command: the help of the nearest command, there or deeper (`hipc vm` gets `hipc
/// compute vms`), or of the two or three as near; `None` when no name is near.
fn nearest_hint(path: &str, group: &clap::Command, word: &str) -> Option<String> {
    let words: Vec<String> = path.split(' ').skip(1).map(str::to_owned).collect();
    let near = crate::spec::near_commands(group, &words, word)?;
    let near: Vec<String> = near
        .iter()
        .map(|path| format!("`hipc {} --help`", path.join(" ")))
        .collect();
    Some(format!("run {}", hippius::phrase::join(&near, "or")))
}

/// clap says a subcommand is missing in two ways: `hipc` and `hipc compute` alone show help
/// (`arg_required_else_help`), and a command line with only flags (`hipc --json`) is
/// `MissingSubcommand`.
fn is_missing_subcommand(error: &clap::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand | ErrorKind::MissingSubcommand
    )
}

/// The message of a usage error from clap: its first paragraph on one line, without the
/// `error: ` prefix, and with the literals it quotes in `'` put in backticks (tenet 15).
/// clap's own line breaks become spaces; what follows the first blank line is its tips
/// and usage, which the hint replaces.
///
/// The words clap quotes from the error's context (the refused value, the argument, the
/// subcommand: the caller's own text, or clap's name for a flag) are kept whole: each is
/// set aside before the cut and put back through `phrase::quote`, so a blank line, a line
/// break, or a `'` inside one cannot end the message or change it. `quote` writes its line
/// breaks as `\n`, the form `render_failure` gives every other control character, so they
/// cannot forge a line of their own.
/// Elsewhere an apostrophe inside a word stays, and a numeric range clap names bare
/// outside a literal (`0 is not in 1..=500`) is a literal too, so it gains backticks.
fn usage_message(error: &clap::Error) -> String {
    let quoted: Vec<&str> = [
        ContextKind::InvalidValue,
        ContextKind::InvalidArg,
        ContextKind::InvalidSubcommand,
    ]
    .into_iter()
    .filter_map(|kind| match error.get(kind) {
        Some(ContextValue::String(text)) => Some(text.as_str()),
        _ => None,
    })
    .collect();
    // Each quoted word becomes a marker of NUL, its index, and NUL. A NUL cannot occur in
    // argv on Unix, nor in clap's own text, so a marker cannot collide with either.
    let mut rendered = error.render().to_string();
    for (i, text) in quoted.iter().enumerate() {
        rendered = rendered.replace(&format!("'{text}'"), &format!("\0{i}\0"));
    }
    // The required arguments clap lists, one per indented line, and the possible values it
    // lists in brackets, are literals too, so each gets its own backticks.
    let listed = |kind| match error.get(kind) {
        Some(ContextValue::Strings(items)) => Some(items.as_slice()),
        _ => None,
    };
    let missing = match error.kind() {
        ErrorKind::MissingRequiredArgument => listed(ContextKind::InvalidArg),
        _ => None,
    };
    if missing.is_some() {
        // The first line is clap's sentence; the arguments follow it, one per line.
        let at = rendered.find('\n').unwrap_or(rendered.len());
        rendered.truncate(at);
    }
    let mut message = clap_sentence(&rendered);
    for (i, text) in quoted.iter().enumerate() {
        message = message.replace(&format!("\0{i}\0"), &phrase::quote(text));
    }
    if let Some(missing) = missing {
        let missing: Vec<String> = missing.iter().map(phrase::quote).collect();
        message = format!("{message} {}", phrase::join(&missing, "and"));
    }
    if let Some(values) = listed(ContextKind::ValidValue) {
        let quoted: Vec<String> = values.iter().map(phrase::quote).collect();
        message = message.replace(
            &format!("[possible values: {}]", values.join(", ")),
            &format!("[possible values: {}]", quoted.join(", ")),
        );
    }
    message
}

/// The first paragraph of clap's rendered error on one line, its `'` quotes as backticks.
fn clap_sentence(rendered: &str) -> String {
    let paragraph: Vec<&str> = rendered
        .lines()
        .map(str::trim)
        .take_while(|line| !line.is_empty())
        .collect();
    let line = paragraph.join(" ");
    let chars: Vec<char> = line.trim_start_matches("error: ").chars().collect();
    let mut quoted = String::with_capacity(line.len());
    for (i, &c) in chars.iter().enumerate() {
        let inside_word = i > 0
            && chars[i - 1].is_alphanumeric()
            && chars.get(i + 1).is_some_and(|next| next.is_alphanumeric());
        quoted.push(if c == '\'' && !inside_word { '`' } else { c });
    }
    let mut backticks = 0;
    let words: Vec<String> = quoted
        .split(' ')
        .map(|word| {
            let outside = backticks % 2 == 0;
            backticks += word.matches('`').count();
            if outside && is_range(word) {
                phrase::quote(word)
            } else {
                word.to_owned()
            }
        })
        .collect();
    words.join(" ")
}

/// `1..=500`, `1..10`, or `1..`: the ranges clap's integer parsers name.
fn is_range(word: &str) -> bool {
    word.split_once("..").is_some_and(|(low, high)| {
        let high = high.strip_prefix('=').unwrap_or(high);
        !low.is_empty() && low.chars().chain(high.chars()).all(|c| c.is_ascii_digit())
    })
}

/// `hipc` and the subcommands of `args` that clap knows, as far as they go: the command
/// whose `--help` covers what the caller was typing, as its path and as the command. A flag
/// is skipped, with its value when it takes one as the next word (`--profile s3`), and so is
/// a leaf's argument. In a group, a word that names none of its subcommands ends the walk:
/// it is the unknown subcommand clap refused, so a later word that names a command, such as
/// `api` in `hipc vm create --name api`, does not move the path.
fn typed_command(args: &[OsString]) -> (String, clap::Command) {
    // Built, so the help of a subcommand names its full path: `Usage: hipc compute ...`, and
    // every subcommand holds the global flags, so their values are skipped at any depth.
    let root = crate::cli::parse_command();
    let mut command = &root;
    let mut path = vec!["hipc".to_owned()];
    let mut value_next = false;
    for word in args.iter().skip(1).filter_map(|word| word.to_str()) {
        if word == "--" {
            break;
        }
        if std::mem::take(&mut value_next) {
            continue;
        }
        if let Some(sub) = command.find_subcommand(word) {
            path.push(sub.get_name().to_owned());
            command = sub;
        } else if word.starts_with('-') {
            value_next = takes_next_word(command, word);
        } else if command.has_subcommands() {
            break;
        }
    }
    (path.join(" "), command.clone())
}

/// Whether the flag `word`, as typed under `command`, takes the next word as its value:
/// `--profile` and `-x` do when the flag takes a value, `--profile=work` and `-xvalue`
/// carry theirs.
fn takes_next_word(command: &clap::Command, word: &str) -> bool {
    let arg = if let Some(long) = word.strip_prefix("--") {
        if long.contains('=') {
            return false;
        }
        command
            .get_arguments()
            .find(|arg| arg.get_long() == Some(long))
    } else {
        let mut chars = word.chars().skip(1);
        match (chars.next(), chars.next()) {
            (Some(short), None) => command
                .get_arguments()
                .find(|arg| arg.get_short() == Some(short)),
            _ => None,
        }
    };
    arg.is_some_and(|arg| arg.get_action().takes_values())
}

/// The stderr text of a failure: one JSON line, or a short paragraph for a person.
fn render_failure(mode: Mode, error: &Error, warnings: &[String]) -> String {
    match mode {
        Mode::Json => {
            // The top-level `message` and `hint` are the text that follows `error: ` and
            // `hint: ` in text mode (tenet 15), escaped the same way: an agent that prints the
            // decoded message cannot write a control character the server sent to a terminal.
            // `details`, the parts of a partial failure included, keeps its text as built.
            let mut object = error.to_json();
            object["error"]["message"] = text::escape(error.message()).into();
            if let Some(hint) = error.hint() {
                object["error"]["hint"] = text::escape(hint).into();
            }
            add_warnings(mode, &mut object, warnings);
            format!("{}\n", json_text(&object))
        }
        Mode::Text => {
            // The message and the hint can quote the server, so their line breaks are
            // escaped too: a body of "x\nhint: run ..." cannot forge a hint line.
            let mut text = format!("error: {}\n", text::escape(&error.to_string()));
            // A partial failure names each part that failed on an `error:` line of its own,
            // as `details` lists them in JSON.
            if error.code() == Code::PartialFailure
                && let Some(Value::Array(parts)) = error.details()
            {
                for part in parts {
                    let name = part.get("service").or_else(|| part.get("step"));
                    if let (Some(Value::String(name)), Some(Value::String(message))) =
                        (name, part.get("message"))
                    {
                        text.push_str(&format!(
                            "error: {}: {}\n",
                            text::escape(name),
                            text::escape(message)
                        ));
                    }
                }
            }
            // A `validation` refusal may list every problem the server found as `issues` (a
            // refused compose does), which JSON mode prints in `details`; a person gets each
            // one on an `error:` line of its own, so the input can be fixed in one pass.
            if error.code() == Code::Validation
                && let Some(Value::Array(issues)) = error.details().and_then(|d| d.get("issues"))
            {
                for line in issues.iter().filter_map(issue_line) {
                    text.push_str(&format!("error: {}\n", text::escape(&line)));
                }
            }
            if let Some(hint) = error.hint() {
                text.push_str(&format!("hint: {}\n", text::escape(hint)));
            }
            text
        }
    }
}

/// One issue of a refused compose, for an `error:` line: where it is, the server's rule, and
/// its message and hint, quoted, or `None` for an issue without a string `message`.
fn issue_line(issue: &Value) -> Option<String> {
    let message = issue.get("message")?.as_str()?;
    let number = |key: &str| issue.get(key).and_then(Value::as_u64);
    let mut line = match (number("line"), number("column")) {
        (Some(line), Some(column)) => format!("line {line}, column {column}: "),
        (Some(line), None) => format!("line {line}: "),
        _ => String::new(),
    };
    if let Some(rule) = issue.get("rule").and_then(Value::as_str) {
        line.push_str(&format!("{}: ", phrase::quote(rule)));
    }
    line.push_str(&format!("the API says {}", phrase::quote(message)));
    if let Some(hint) = issue.get("hint").and_then(Value::as_str) {
        line.push_str(&format!(", and suggests {}", phrase::quote(hint)));
    }
    Some(line)
}

/// `value` as JSON, with the characters text mode escapes (bidi controls, zero-width
/// characters, the Unicode separators, C1 controls) written as `\uXXXX`. That is the same
/// JSON value, and JSON on a terminal (`CI` or `AI_AGENT` set) cannot reorder a line either. Those
/// characters occur only inside strings, so replacing them never touches the structure.
fn json_text(value: &Value) -> String {
    let json = value.to_string();
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        if text::changes_how_text_reads(c) || ('\u{7f}'..='\u{9f}').contains(&c) {
            push_json_escape(&mut out, c);
        } else {
            out.push(c);
        }
    }
    out
}

/// Pushes `c` as JSON's `\uXXXX`. A character above U+FFFF is a surrogate pair in JSON:
/// one `\u` per unit.
fn push_json_escape(out: &mut String, c: char) {
    for unit in c.encode_utf16(&mut [0; 2]) {
        out.push_str(&format!("\\u{unit:04x}"));
    }
}

/// `text` with every control character but `\n`, and every character
/// `text::changes_how_text_reads` names, escaped: for clap's own multi-line text.
fn escape_lines(text: &str) -> String {
    text.split('\n')
        .map(text::escape)
        .collect::<Vec<_>>()
        .join("\n")
}

fn render(mode: Mode, document: &Value, layout: Layout) -> String {
    match mode {
        Mode::Json => format!("{}\n", json_text(document)),
        Mode::Text => text::render(document, layout),
    }
}

/// Writes a success document to stdout.
///
/// A reader that closed its end of the pipe (`hipc ... | head -1`) has chosen to stop
/// reading. That is not this command's failure, so a broken pipe is still exit 0. Any
/// other failure means the document did not arrive: it becomes an `internal` error on
/// stderr, which is a different stream and may still work. Its `side_effects` is `done`:
/// a write the server already did is not undone by a full disk.
fn stdout(mode: Mode, bytes: &[u8], done: SideEffects) -> ExitCode {
    match write_stdout(bytes) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(error) => failure(
            mode,
            &Error::internal(format!(
                "cannot write to stdout: {}",
                phrase::io_reason(&error)
            ))
            .with_side_effects(done),
        ),
    }
}

/// A stdout that was closed when `hipc` started is not an error here: the Rust runtime
/// reopens a closed fd 0, 1 or 2 on `/dev/null` before `main`, so the document is
/// discarded as `hipc > /dev/null` would discard it, and nothing can tell the two apart.
fn write_stdout(bytes: &[u8]) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(bytes)?;
    stdout.flush()
}

/// Writes to stderr. A failure here cannot be reported anywhere: stderr is the stream a
/// report would use. The exit code still carries the failure.
fn stderr(text: &str) {
    let mut stderr = io::stderr().lock();
    let written = stderr
        .write_all(text.as_bytes())
        .and_then(|()| stderr.flush());
    drop(written);
}

/// The note text mode prints on stderr for a success document that holds nothing to show,
/// so a person can tell an empty answer from a hang: `no items` for a list with no
/// items and no later page, and `no fields` for an empty object. `None` for any other
/// document. stdout is left as [`document`] writes it, so it stays empty, or holds only
/// the fields beside the list. Only a list command's envelope ([`Layout::List`]) is a list
/// with no items; another document with an empty `items` prints that field.
pub(crate) fn empty_note(document: &Value, layout: Layout) -> Option<&'static str> {
    match document {
        Value::Array(items) if items.is_empty() => Some("no items"),
        Value::Object(map) if map.is_empty() => Some("no fields"),
        Value::Object(map) if layout == Layout::List => {
            match (map.get("items"), map.get("next_page_token")) {
                (Some(Value::Array(items)), Some(Value::Null)) if items.is_empty() => {
                    Some("no items")
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Text mode: a key-value block for one object, an aligned table for a list of objects.
/// For a person; agents read JSON.
///
/// Strings are printed with control characters escaped, so a value from the server cannot
/// move the cursor, recolor the terminal, or break a row. An empty list or object prints
/// nothing on stdout, as `ls` does in an empty directory; [`empty_note`] says so on stderr.
mod text {
    use serde_json::{Map, Value};

    use super::Layout;

    /// `document` as text, laid out as `layout` says: under [`Layout::DryRun`] a document
    /// that lists `requests` is a dry run's ([`dry_run`]), and under [`Layout::List`] a
    /// document with `items` and `next_page_token` is a list envelope.
    pub(super) fn render(document: &Value, layout: Layout) -> String {
        match document {
            Value::Array(items) => list(items),
            Value::Object(map)
                if layout == Layout::DryRun
                    && map.contains_key("dry_run")
                    && map.get("requests").is_some_and(Value::is_array) =>
            {
                dry_run(map)
            }
            Value::Object(map) => match map.get("items") {
                Some(Value::Array(items))
                    if layout == Layout::List && map.contains_key("next_page_token") =>
                {
                    let mut out = list(items);
                    // The server's other fields beside the list (a total, `partial_failure`,
                    // `sharing_enabled`) are printed too: text mode drops nothing.
                    for (key, value) in map {
                        if key != "items" && key != "next_page_token" {
                            out.push_str(&format!("{}: {}\n", escape(key), cell(value)));
                        }
                    }
                    if let Some(Value::String(token)) = map.get("next_page_token") {
                        out.push_str(&format!("next page: --page-token {}\n", escape(token)));
                    }
                    out
                }
                _ => block(map),
            },
            other => format!("{}\n", cell(other)),
        }
    }

    /// A dry run: its other fields as a block, then each request as a block of its own, after
    /// a blank line, so a person reads the method, the URL, and the body on their own lines.
    fn dry_run(map: &Map<String, Value>) -> String {
        let rest: Map<String, Value> = map
            .iter()
            .filter(|(key, _)| *key != "requests")
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let mut out = block(&rest);
        let requests = map.get("requests").and_then(Value::as_array);
        for request in requests.into_iter().flatten().filter_map(Value::as_object) {
            out.push('\n');
            out.push_str(&block(request));
        }
        out
    }

    fn block(map: &Map<String, Value>) -> String {
        let width = map.keys().map(|k| width(k)).max().unwrap_or(0);
        let mut out = String::new();
        for (key, value) in map {
            out.push_str(&format!("{}  {}\n", pad(&escape(key), width), cell(value)));
        }
        out
    }

    /// A table when every item is an object. Otherwise one item per line, so no item is
    /// dropped for lacking columns.
    fn list(items: &[Value]) -> String {
        let objects: Option<Vec<&Map<String, Value>>> =
            items.iter().map(Value::as_object).collect();
        match objects {
            Some(objects) if !objects.is_empty() => table(&objects),
            _ => {
                let mut out = String::new();
                for item in items {
                    out.push_str(&format!("{}\n", cell(item)));
                }
                out
            }
        }
    }

    fn table(objects: &[&Map<String, Value>]) -> String {
        let mut columns: Vec<&str> = Vec::new();
        for object in objects {
            for key in object.keys() {
                if !columns.contains(&key.as_str()) {
                    columns.push(key);
                }
            }
        }
        let header: Vec<String> = columns.iter().map(|c| escape(c)).collect();
        let rows: Vec<Vec<String>> = objects
            .iter()
            .map(|object| {
                columns
                    .iter()
                    .map(|c| object.get(*c).map(cell).unwrap_or_default())
                    .collect()
            })
            .collect();
        let widths: Vec<usize> = header
            .iter()
            .enumerate()
            .map(|(i, h)| {
                rows.iter()
                    .map(|r| width(&r[i]))
                    .chain([width(h)])
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        let line = |cells: &[String]| {
            let padded: Vec<String> = cells.iter().zip(&widths).map(|(c, w)| pad(c, *w)).collect();
            format!("{}\n", padded.join("  ").trim_end())
        };
        let mut out = line(&header);
        for row in &rows {
            out.push_str(&line(row));
        }
        out
    }

    fn cell(value: &Value) -> String {
        match value {
            Value::Null => "-".to_owned(),
            Value::String(s) => escape(s),
            // serde_json escapes only control characters below U+0020 inside nested strings,
            // so a bidi override in an array or object cell would print raw.
            other => escape(&other.to_string()),
        }
    }

    /// Control characters, and the characters [`changes_how_text_reads`] names, as Rust
    /// escapes (`\n`, `\u{1b}`, `\u{202e}`); everything else as is.
    pub(super) fn escape(text: &str) -> String {
        text.chars()
            .map(|c| {
                if c.is_control() || changes_how_text_reads(c) {
                    c.escape_default().to_string()
                } else {
                    c.to_string()
                }
            })
            .collect()
    }

    /// Characters that are not control characters (category Cc) but still change how a
    /// line reads: the format characters (category Cf: bidi overrides and isolates, which
    /// can reverse an id or URL a person copies, zero-width and tag characters), the
    /// Unicode line and paragraph separators, and the invisible fillers and variation
    /// selectors that make `prod` and `prod` plus a filler look the same.
    ///
    /// The Cf rows come from `DerivedGeneralCategory.txt` of Unicode 15; the rest are the
    /// assigned `Default_Ignorable_Code_Point` characters of `DerivedCoreProperties.txt` that
    /// are not Cf, and the Braille blank. A Unicode release that adds a Cf or default-ignorable
    /// character needs a row here.
    pub(crate) fn changes_how_text_reads(c: char) -> bool {
        matches!(
            c,
            // Category Cf, from the Unicode 15 tables.
            '\u{00AD}'
                | '\u{0600}'..='\u{0605}'
                | '\u{061C}'
                | '\u{06DD}'
                | '\u{070F}'
                | '\u{0890}'..='\u{0891}'
                | '\u{08E2}'
                | '\u{180E}'
                | '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{206F}'
                | '\u{FEFF}'
                | '\u{FFF9}'..='\u{FFFB}'
                | '\u{110BD}'
                | '\u{110CD}'
                | '\u{13430}'..='\u{1343F}'
                | '\u{1BCA0}'..='\u{1BCA3}'
                | '\u{1D173}'..='\u{1D17A}'
                | '\u{E0001}'
                | '\u{E0020}'..='\u{E007F}'
                // Line and paragraph separators (Zl, Zp).
                | '\u{2028}'..='\u{2029}'
                // Invisible fillers and the combining grapheme joiner.
                | '\u{034F}'
                | '\u{115F}'..='\u{1160}'
                | '\u{17B4}'..='\u{17B5}'
                | '\u{3164}'
                | '\u{FFA0}'
                // Variation selectors, the Mongolian free ones included (Mn, Unicode 14).
                | '\u{180B}'..='\u{180D}'
                | '\u{180F}'
                | '\u{FE00}'..='\u{FE0F}'
                | '\u{E0100}'..='\u{E01EF}'
                // The Braille blank (So), a common invisible filler.
                | '\u{2800}'
        )
    }

    /// Display width as a count of chars. Wide CJK and emoji take two columns on most
    /// terminals, so rows holding them misalign. That is accepted to avoid a Unicode
    /// width table.
    fn width(text: &str) -> usize {
        text.chars().count()
    }

    fn pad(text: &str, to: usize) -> String {
        let mut out = text.to_owned();
        out.extend(std::iter::repeat_n(' ', to.saturating_sub(width(text))));
        out
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;
    use hippius::Failure;
    use serde_json::json;

    use super::*;
    use crate::cli::Cli;

    /// The answer a failure prints of what was done loses a server `next_step`, as every
    /// printed document does; a failure with nothing done prints nothing on stdout.
    #[test]
    fn a_failure_prints_what_was_done_without_a_server_next_step() {
        let done = json!({"vm_id": "v1", "next_step": "hipc compute vms delete v1 --yes"});
        let error = Error::internal("the wait failed").after_write(done);
        assert_eq!(done_on_failure(&error), Some(json!({"vm_id": "v1"})));
        assert_eq!(done_on_failure(&Error::internal("x")), None);
    }

    fn args(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    const TTY: Terminal = Terminal {
        automation: false,
        stdin_tty: true,
        stdout_tty: true,
        stderr_tty: true,
    };
    const PIPE: Terminal = Terminal {
        automation: false,
        stdin_tty: false,
        stdout_tty: false,
        stderr_tty: false,
    };
    /// Every stream a terminal, with a CI or agent variable set.
    const AUTOMATED_TTY: Terminal = Terminal {
        automation: true,
        ..TTY
    };

    fn is_json(mode: Mode) -> bool {
        matches!(mode, Mode::Json)
    }

    fn ai_agent(value: &str) -> bool {
        is_automation(None, Some(OsStr::new(value)))
    }

    #[test]
    fn ci_with_any_value_counts() {
        for value in ["true", "1", "false", "0", ""] {
            assert!(is_automation(Some(OsStr::new(value)), None), "CI={value:?}");
        }
        assert!(!is_automation(None, None));
    }

    #[test]
    fn ai_agent_counts_unless_it_is_empty_or_an_off_word() {
        for value in ["", "   ", "0", "false", "No", " off ", "FALSE", "\tno\n"] {
            assert!(!ai_agent(value), "AI_AGENT={value:?}");
        }
        for value in [
            "1",
            "true",
            "yes",
            "claude-code_2-1-310_agent",
            "offline",
            "00",
        ] {
            assert!(ai_agent(value), "AI_AGENT={value:?}");
        }
    }

    #[test]
    fn ai_agent_that_is_not_utf8_counts() {
        use std::os::unix::ffi::OsStrExt as _;
        assert!(is_automation(None, Some(OsStr::from_bytes(&[0xff]))));
    }

    /// An off word in `AI_AGENT` never cancels `CI`: detection only adds strictness.
    #[test]
    fn ci_counts_whatever_ai_agent_says() {
        assert!(is_automation(Some(OsStr::new("")), Some(OsStr::new("0"))));
    }

    #[test]
    fn automation_is_json_on_a_terminal() {
        assert!(is_json(AUTOMATED_TTY.mode(OutputArg::Auto)));
        assert!(!is_json(TTY.mode(OutputArg::Auto)));
        assert!(is_json(PIPE.mode(OutputArg::Auto)));
    }

    #[test]
    fn prompting_needs_text_mode_three_terminals_no_automation_and_no_no_input() {
        let with = |stdin_tty, stdout_tty, stderr_tty| Terminal {
            automation: false,
            stdin_tty,
            stdout_tty,
            stderr_tty,
        };
        assert!(TTY.can_prompt(Mode::Text, false));
        assert!(!TTY.can_prompt(Mode::Json, false));
        assert!(!TTY.can_prompt(Mode::Text, true));
        assert!(!PIPE.can_prompt(Mode::Text, false));
        assert!(!AUTOMATED_TTY.can_prompt(Mode::Text, false));
        assert!(!with(false, true, true).can_prompt(Mode::Text, false));
        assert!(!with(true, false, false).can_prompt(Mode::Text, false));
        // A prompt on a stderr redirected to a file would wait for an answer nobody sees.
        assert!(!with(true, true, false).can_prompt(Mode::Text, false));
    }

    #[test]
    fn explicit_output_wins_over_the_terminal_and_automation() {
        let automated_pipe = Terminal {
            automation: true,
            ..PIPE
        };
        assert!(!is_json(automated_pipe.mode(OutputArg::Text)));
        assert!(!is_json(AUTOMATED_TTY.mode(OutputArg::Text)));
        assert!(!is_json(PIPE.mode(OutputArg::Text)));
        assert!(is_json(TTY.mode(OutputArg::Json)));
    }

    #[test]
    fn json_flag_is_folded_into_the_mode() {
        let cli = Cli::try_parse_from(["hipc", "--json", "version"]).unwrap();
        assert!(is_json(TTY.resolve(&cli.global)));
        let cli = Cli::try_parse_from(["hipc", "--output", "text", "version"]).unwrap();
        assert!(!is_json(PIPE.resolve(&cli.global)));
    }

    #[test]
    fn raw_argv_scan_finds_the_mode_of_a_rejected_command_line() {
        assert!(is_json(
            TTY.resolve_from_args(&args(&["hipc", "--bogus", "--json"]))
        ));
        assert!(is_json(
            TTY.resolve_from_args(&args(&["hipc", "--output=json"]))
        ));
        assert!(!is_json(
            PIPE.resolve_from_args(&args(&["hipc", "--output", "text"]))
        ));
        assert!(!is_json(
            TTY.resolve_from_args(&args(&["hipc", "--", "--json"]))
        ));
        let unknown = TTY.resolve_from_args(&args(&["hipc", "--output", "bogus"]));
        assert!(!is_json(unknown));
        assert!(is_json(
            PIPE.resolve_from_args(&args(&["hipc", "--output"]))
        ));
        let flag_after = TTY.resolve_from_args(&args(&["hipc", "--output", "--json"]));
        assert!(is_json(flag_after));
    }

    #[test]
    fn raw_argv_scan_takes_a_non_utf8_word_as_the_output_value() {
        use std::os::unix::ffi::OsStringExt as _;
        let words = vec![
            OsString::from("hipc"),
            OsString::from("--output"),
            OsString::from_vec(vec![0xff]),
            OsString::from("text"),
        ];
        assert!(is_json(PIPE.resolve_from_args(&words)));
        let words = vec![OsString::from("hipc"), OsString::from_vec(vec![0xff])];
        assert!(is_json(PIPE.resolve_from_args(&words)));
    }

    /// Every command line of up to four words from this vocabulary that clap accepts gets
    /// the same mode from the raw scan as from the parse.
    ///
    /// The parser is built once and reused: `Cli::try_parse_from` builds the whole command
    /// tree afresh for each of the 10,000 lines, which is most of a parse's cost and took
    /// about 12 minutes in a debug build.
    #[test]
    fn raw_argv_scan_agrees_with_clap_on_every_accepted_command_line() {
        use clap::{CommandFactory as _, FromArgMatches as _};

        const WORDS: [&str; 9] = [
            "version",
            "--json",
            "--output",
            "json",
            "text",
            "auto",
            "--output=text",
            "--output=json",
            "--",
        ];
        let mut lines: Vec<Vec<&str>> = vec![vec![]];
        for _ in 0..4 {
            let longer: Vec<Vec<&str>> = lines
                .iter()
                .flat_map(|line| {
                    WORDS.iter().map(move |w| {
                        let mut next = line.clone();
                        next.push(*w);
                        next
                    })
                })
                .collect();
            lines.extend(longer);
        }
        let mut parser = Cli::command();
        let mut accepted = 0;
        for line in &lines {
            let mut argv = vec!["hipc"];
            argv.extend(line);
            let parsed = parser
                .try_get_matches_from_mut(&argv)
                .and_then(|mut matches| Cli::from_arg_matches_mut(&mut matches));
            let Ok(cli) = parsed else {
                continue;
            };
            accepted += 1;
            for terminal in [&TTY, &PIPE] {
                assert_eq!(
                    is_json(terminal.resolve(&cli.global)),
                    is_json(terminal.resolve_from_args(&args(&argv))),
                    "{argv:?}"
                );
            }
        }
        assert!(accepted > 10, "only {accepted} accepted lines");
    }

    fn clap_error(argv: &[&str]) -> clap::Error {
        Cli::try_parse_from(argv).unwrap_err()
    }

    #[test]
    fn a_range_is_digits_around_two_dots() {
        for range in ["1..=500", "1..10", "1.."] {
            assert!(is_range(range), "{range}");
        }
        for other in ["..5", "..", "a..b", "1...", "x", "1..=a"] {
            assert!(!is_range(other), "{other}");
        }
    }

    #[test]
    fn usage_message_states_what_clap_rejected() {
        let cases: [(&[&str], &str); 16] = [
            // A range inside a literal is not quoted a second time.
            (
                &["hipc", "version", "a 1..2 b"],
                "unexpected argument `a 1..2 b` found",
            ),
            (
                &["hipc", "billing", "transactions", "--limit", "0"],
                "invalid value `0` for `--limit <LIMIT>`: 0 is not in `1..=500`",
            ),
            // An apostrophe inside a refused word stays; the quotes around it change.
            (
                &["hipc", "version", "don't"],
                "unexpected argument `don't` found",
            ),
            (
                &["hipc", "completions"],
                "the following required arguments were not provided: `<SHELL>`",
            ),
            (
                &["hipc", "compute", "vms", "create"],
                "the following required arguments were not provided: `--flavor <FLAVOR>` and \
                 `<--ssh-key-id <ID>|--ssh-public-key-file <PATH>>`",
            ),
            (
                &["hipc", "completions", "tcsh"],
                "invalid value `tcsh` for `<SHELL>` \
                 [possible values: `bash`, `elvish`, `fish`, `powershell`, `zsh`]",
            ),
            (
                &["hipc", "compute", "vms", "create", "--os-disk-tier"],
                "a value is required for `--os-disk-tier <TIER>` but none was supplied \
                 [possible values: `nvme`, `ssd`, `hdd`]",
            ),
            (&["hipc", "--bogus"], "unexpected argument `--bogus` found"),
            (
                &["hipc", "version", "extra"],
                "unexpected argument `extra` found",
            ),
            (
                &["hipc", "--json", "--output", "text", "version"],
                "the argument `--json` cannot be used with `--output <OUTPUT>`",
            ),
            // What the caller typed is kept whole: a blank line, or clap's own `Usage:` or
            // `tip:` words, inside it do not end the message, and a `'` at its edge stays.
            // The line breaks read `\n` here because `quote` and `text::escape` share one
            // escape dialect: the message holds what the stderr line shows.
            (
                &["hipc", "version", "a\n\nb"],
                "unexpected argument `a\\n\\nb` found",
            ),
            (
                &[
                    "hipc",
                    "billing",
                    "transactions",
                    "--limit",
                    "1\n\nUsage: x",
                ],
                "invalid value `1\\n\\nUsage: x` for `--limit <LIMIT>`: \
                 invalid digit found in string",
            ),
            (
                &["hipc", "version", "'q' x\n\n  tip: y"],
                "unexpected argument `'q' x\\n\\n  tip: y` found",
            ),
            (&["hipc", "a\n\nb"], "unrecognized subcommand `a\\n\\nb`"),
            (
                &["hipc", "compute", "vms", "get", "a\n\n/b"],
                "invalid value `a\\n\\n/b` for `<VM_ID>`: a VM id is 1 to 64 letters, digits, \
                 `-`, and `_`",
            ),
            // clap's own tip after a blank line is left out; the hint replaces it.
            (
                &["hipc", "--yse", "version"],
                "unexpected argument `--yse` found",
            ),
        ];
        for (argv, message) in cases {
            assert_eq!(usage_message(&clap_error(argv)), message, "{argv:?}");
        }
    }

    /// Both of clap's ways of saying a subcommand is missing are one message, with or
    /// without flags before the stop (`hipc --json` once got clap's own text).
    #[test]
    fn a_command_line_that_stops_before_its_subcommand_is_missing_one() {
        for argv in [
            &["hipc"][..],
            &["hipc", "compute"],
            &["hipc", "--json"],
            &["hipc", "--profile", "work"],
            &["hipc", "compute", "--debug"],
        ] {
            assert!(is_missing_subcommand(&clap_error(argv)), "{argv:?}");
        }
        for argv in [&["hipc", "--bogus"][..], &["hipc", "completions"]] {
            assert!(!is_missing_subcommand(&clap_error(argv)), "{argv:?}");
        }
    }

    #[test]
    fn the_help_hint_names_the_command_the_caller_was_typing() {
        let cases: [(&[&str], &str); 14] = [
            (&["hipc", "--bogus"], "hipc"),
            (
                &["hipc", "compute", "vms", "get", "--bogus"],
                "hipc compute vms get",
            ),
            (&["hipc", "--profile", "work", "s3", "nope"], "hipc s3"),
            (
                &["hipc", "cdn", "zones", "get", "list"],
                "hipc cdn zones get",
            ),
            (&["hipc", "api", "--", "compute"], "hipc api"),
            (&["hipc", "drive", "plans", "x"], "hipc drive plans"),
            // A flag's value is not a subcommand, however it is spelled.
            (
                &["hipc", "--profile", "s3", "compute", "nope"],
                "hipc compute",
            ),
            (&["hipc", "--profile=s3", "compute"], "hipc compute"),
            (&["hipc", "compute", "--profile", "vms"], "hipc compute"),
            // A word a group does not hold ends the path, whatever follows it.
            (&["hipc", "vm", "create", "--name", "api"], "hipc"),
            (&["hipc", "nope", "compute"], "hipc"),
            (&["hipc", "compute", "vm", "list"], "hipc compute"),
            // A short flag takes no value here.
            (&["hipc", "-h", "compute"], "hipc compute"),
            (&["hipc", "-hq", "compute"], "hipc compute"),
        ];
        for (argv, command) in cases {
            assert_eq!(typed_command(&args(argv)).0, command, "{argv:?}");
        }
    }

    #[test]
    fn failure_is_one_json_line_or_a_paragraph_with_the_hint() {
        let error = Error::usage("bad flag", "run `hipc --help`");
        let json = render_failure(Mode::Json, &error, &[]);
        assert_eq!(json.lines().count(), 1);
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["error"]["code"], "usage");
        assert_eq!(
            render_failure(Mode::Text, &error, &[]),
            "error: bad flag\nhint: run `hipc --help`\n"
        );
        assert_eq!(
            render_failure(
                Mode::Text,
                &Error::new(Failure::NotFound, "no such thing"),
                &[]
            ),
            "error: no such thing\n"
        );
    }

    #[test]
    fn render_json_is_one_compact_line() {
        let doc = json!({"a": [1, 2], "b": {"c": null}});
        assert_eq!(
            render(Mode::Json, &doc, Layout::Document),
            "{\"a\":[1,2],\"b\":{\"c\":null}}\n"
        );
    }

    #[test]
    fn text_table_aligns_columns_and_names_the_next_page() {
        let doc = json!({
            "items": [{"id": "a", "name": "first"}, {"id": "bbb", "name": null}],
            "next_page_token": "2"
        });
        assert_eq!(
            text::render(&doc, Layout::List),
            "id   name\na    first\nbbb  -\nnext page: --page-token 2\n"
        );
        // A `get` whose answer has the same fields is one object: no table and no page hint,
        // which its command does not take.
        assert_eq!(
            text::render(&doc, Layout::Document),
            "items            [{\"id\":\"a\",\"name\":\"first\"},{\"id\":\"bbb\",\"name\":null}]\nnext_page_token  2\n"
        );
        assert_eq!(
            text::render(&doc, Layout::DryRun),
            text::render(&doc, Layout::Document)
        );
    }

    /// A dry run prints its own fields, then each request as a block after a blank line.
    #[test]
    fn text_dry_run_prints_each_request_as_a_block() {
        let doc = json!({
            "dry_run": true,
            "requests": [
                {"method": "GET", "url": "https://x/a"},
                {"method": "PUT", "body": {"k": 1}}
            ]
        });
        assert_eq!(
            text::render(&doc, Layout::DryRun),
            "dry_run  true\n\nmethod  GET\nurl     https://x/a\n\nmethod  PUT\nbody    {\"k\":1}\n"
        );
        // The same fields in a real run's answer, such as a server's, are a plain block.
        assert!(text::render(&doc, Layout::Document).starts_with("dry_run   true\nrequests  "));
    }

    #[test]
    fn text_block_for_one_object() {
        let doc = json!({"version": "0.1.0", "target": "aarch64-apple-darwin"});
        assert_eq!(
            text::render(&doc, Layout::Document),
            "version  0.1.0\ntarget   aarch64-apple-darwin\n"
        );
    }

    #[test]
    fn text_scalars_arrays_and_nested_values() {
        assert_eq!(text::render(&json!("plain"), Layout::Document), "plain\n");
        assert_eq!(text::render(&json!(null), Layout::Document), "-\n");
        assert_eq!(text::render(&json!(42), Layout::Document), "42\n");
        assert_eq!(
            text::render(&json!(["a", 1, null]), Layout::Document),
            "a\n1\n-\n"
        );
        assert_eq!(text::render(&json!([]), Layout::Document), "");
        assert_eq!(text::render(&json!({}), Layout::Document), "");
        assert_eq!(
            text::render(&json!({"path": ["a", "b"], "ok": true}), Layout::Document),
            "path  [\"a\",\"b\"]\nok    true\n"
        );
    }

    #[test]
    fn text_table_takes_the_union_of_columns_in_first_seen_order() {
        let doc = json!([{"a": 1}, {"b": 2, "a": 3}]);
        assert_eq!(text::render(&doc, Layout::Document), "a  b\n1\n3  2\n");
    }

    #[test]
    fn a_list_mixing_objects_and_scalars_prints_every_item() {
        let doc = json!([{"a": 1}, "loose", 7]);
        assert_eq!(
            text::render(&doc, Layout::Document),
            "{\"a\":1}\nloose\n7\n"
        );
    }

    #[test]
    fn items_without_a_page_token_field_is_a_plain_object() {
        assert_eq!(
            text::render(&json!({"items": [1]}), Layout::List),
            "items  [1]\n"
        );
        let last_page = json!({"items": [{"id": "a"}], "next_page_token": null});
        assert_eq!(text::render(&last_page, Layout::List), "id\na\n");
    }

    /// Only a list with nothing on it and no later page, or an empty object, gets
    /// the note.
    #[test]
    fn an_empty_list_or_object_is_noted_and_nothing_else() {
        let empty_page = json!({"count": 0, "items": [], "next_page_token": null});
        for layout in [Layout::List, Layout::Document] {
            assert_eq!(empty_note(&json!([]), layout), Some("no items"));
            assert_eq!(empty_note(&json!({}), layout), Some("no fields"));
        }
        assert_eq!(empty_note(&empty_page, Layout::List), Some("no items"));
        // A `get` whose answer has an empty `items` prints that field: not an empty list.
        assert_eq!(empty_note(&empty_page, Layout::Document), None);
        for shown in [
            json!([1]),
            json!({"items": [], "next_page_token": "2"}),
            json!({"items": [1], "next_page_token": null}),
            json!({"items": []}),
            json!({"items": "x", "next_page_token": null}),
            json!({"id": 1}),
            json!(""),
            json!(null),
        ] {
            assert_eq!(empty_note(&shown, Layout::List), None, "{shown}");
        }
    }

    /// The fields the server keeps beside the list are printed after the table, so text
    /// mode drops nothing a JSON caller sees.
    #[test]
    fn text_prints_the_fields_beside_the_items_after_the_table() {
        let doc = json!({
            "sharing_enabled": true,
            "items": [{"id": "a"}],
            "next_page_token": "2",
            "partial_failure": true
        });
        assert_eq!(
            text::render(&doc, Layout::List),
            "id\na\nsharing_enabled: true\npartial_failure: true\nnext page: --page-token 2\n"
        );
    }

    /// A partial failure names each failed part on an `error:` line of its own.
    #[test]
    fn a_partial_failure_lists_its_failed_parts_in_text_mode() {
        let error = Error::partial_failure(
            "cannot read the plans of `s3` and `hub`: the list holds the other 2 services",
            json!({"items": []}),
            json!([
                {"service": "s3", "message": "cannot get the `s3` plans: the API answered 503"},
                {"step": "subscribe", "message": "cannot subscribe"},
                {"service": "odd"}
            ]),
            SideEffects::None,
        );
        assert_eq!(
            render_failure(Mode::Text, &error, &[]),
            "error: cannot read the plans of `s3` and `hub`: the list holds the other 2 \
             services\nerror: s3: cannot get the `s3` plans: the API answered 503\nerror: \
             subscribe: cannot subscribe\n"
        );
    }

    #[test]
    fn text_escapes_control_characters_everywhere() {
        let doc = json!({"na\tme": "line\nbreak \u{1b}[31mred"});
        assert_eq!(
            text::render(&doc, Layout::Document),
            "na\\tme  line\\nbreak \\u{1b}[31mred\n"
        );
        let doc = json!({"items": [{"k\n": "v"}], "next_page_token": "x\ry"});
        assert_eq!(
            text::render(&doc, Layout::List),
            "k\\n\nv\nnext page: --page-token x\\ry\n"
        );
    }

    #[test]
    fn text_escapes_characters_that_reorder_or_hide_text() {
        // U+202E reverses what follows, U+200B is invisible, U+009B is a C1 control.
        let doc = json!({"name": "a\u{202E}gpj.exe\u{202C} b\u{200B}c \u{9b}2J \u{2028}"});
        let rendered = text::render(&doc, Layout::Document);
        for c in ['\u{202E}', '\u{202C}', '\u{200B}', '\u{9b}', '\u{2028}'] {
            assert!(!rendered.contains(c), "{c:?} in {rendered:?}");
        }
        assert!(rendered.contains("\\u{202e}gpj.exe"), "{rendered}");
        assert!(
            text::render(&json!({"name": "ünïcødé ✓"}), Layout::Document).contains("ünïcødé ✓")
        );
        // Invisible fillers and tag characters make one name look like another.
        let filler = text::render(
            &json!({"name": "prod\u{3164}", "tag": "a\u{E0041}b\u{E0100}"}),
            Layout::Document,
        );
        for c in ['\u{3164}', '\u{E0041}', '\u{E0100}'] {
            assert!(!filler.contains(c), "{c:?} in {filler:?}");
        }
        // So do the Mongolian free variation selectors, the Khmer inherent vowels, and the
        // Braille blank, which are not format characters.
        for c in ['\u{180B}', '\u{180D}', '\u{180F}', '\u{17B4}', '\u{2800}'] {
            let rendered = text::render(&json!({ "name": format!("prod{c}") }), Layout::Document);
            assert!(!rendered.contains(c), "{c:?} in {rendered:?}");
        }
        // JSON mode writes the same characters, and DEL, as \u escapes: still the same value.
        let json = json_text(&json!({"name": "a\u{202E}b\u{7f}c"}));
        assert_eq!(json, r#"{"name":"a\u202eb\u007fc"}"#);
        assert_eq!(
            serde_json::from_str::<Value>(&json).unwrap(),
            json!({"name": "a\u{202E}b\u{7f}c"})
        );
        // Above U+FFFF a character is a surrogate pair, and reads back as itself.
        let astral = json!({"t": "a\u{E0041}b\u{E0100}"});
        let json = json_text(&astral);
        assert_eq!(json, r#"{"t":"a\udb40\udc41b\udb40\udd00"}"#);
        assert_eq!(serde_json::from_str::<Value>(&json).unwrap(), astral);
        // Inside an array or object cell too, which serde_json does not escape.
        let nested = text::render(
            &json!({"items": [{"ports": ["a\u{202E}b"]}], "next_page_token": null}),
            Layout::List,
        );
        assert!(!nested.contains('\u{202E}'), "{nested:?}");
    }

    /// A body a person may read keeps its line breaks and tabs, and escapes the rest in the
    /// form its mode reads: Rust escapes in text, and in JSON the `\uXXXX` that leaves a
    /// string the same value (a Rust escape is not valid inside a JSON string).
    #[test]
    fn a_body_is_escaped_in_the_form_its_mode_reads() {
        let body = "{\"a\":\"\u{1b}]0;x\u{7}\u{202e}\u{E0041}\"}\n\t";
        assert_eq!(
            escape_body(Mode::Text, body.as_bytes()),
            "{\"a\":\"\\u{1b}]0;x\\u{7}\\u{202e}\\u{e0041}\"}\n\t\n"
        );
        let json = escape_body(Mode::Json, body.as_bytes());
        assert_eq!(
            json,
            "{\"a\":\"\\u001b]0;x\\u0007\\u202e\\udb40\\udc41\"}\n\t\n"
        );
        assert_eq!(
            serde_json::from_str::<Value>(&json).unwrap(),
            json!({"a": "\u{1b}]0;x\u{7}\u{202e}\u{E0041}"})
        );
        assert_eq!(escape_body(Mode::Json, b""), "");
        assert_eq!(escape_body(Mode::Text, b"a\rb\xff"), "a\\rb\u{fffd}\n");
        assert_eq!(escape_body(Mode::Json, b"a\rb"), "a\\u000db\n");
    }

    #[test]
    fn text_pads_by_characters_not_bytes() {
        let doc = json!([{"name": "é", "n": 1}, {"name": "ab", "n": 2}]);
        assert_eq!(
            text::render(&doc, Layout::Document),
            "name  n\né     1\nab    2\n"
        );
        let doc = json!({"é": 1, "ab": 2});
        assert_eq!(text::render(&doc, Layout::Document), "é   1\nab  2\n");
    }
}
