//! Parses argv, runs one command, and hands its result to the output module.

use std::ffi::OsString;
use std::process::ExitCode;

use clap::{CommandFactory as _, FromArgMatches as _};
use hippius::Result;
use serde_json::{Value, json};

use crate::cli::{
    AutoReload, Billing, Cdn, CdnHostnames, Cli, Command, Drive, DriveSubscription, S3,
    S3Subscription,
};
use crate::commands::{self, Printed};
use crate::context::Context;
use crate::output::{self, Layout, Terminal};
use crate::spec;

/// Runs `hipc` with these arguments and returns the process exit code.
pub(crate) fn run(args: &[OsString]) -> ExitCode {
    disable_core_dumps();
    let terminal = Terminal::detect();
    // Before anything parses or checks a value, so no message can quote a secret given as one,
    // whether clap takes the value or a later check refuses it (tenet 9).
    if let Some(refusal) = secret_in_argv(args) {
        return output::failure(terminal.resolve_from_args(args), &refusal);
    }
    let parsed = crate::cli::parse_command()
        .try_get_matches_from(args)
        .and_then(|matches| {
            Cli::from_arg_matches(&matches).map(|cli| (cli, command_path(&matches)))
        });
    let (cli, path) = match parsed {
        Ok(parsed) => parsed,
        Err(error) => return output::clap_error(terminal.resolve_from_args(args), &error, args),
    };
    let mode = terminal.resolve(&cli.global);
    // A command the parser ran with no schema row is a bug that `spec`'s walking test keeps
    // from shipping; should one ship, it runs nothing and ends in the `internal` error object
    // of `spec::row`, never a panic.
    let row = spec::row(&path);
    if let Some(refusal) = repeated_global(args) {
        return output::failure(mode, &refusal);
    }
    let words: Vec<String> = args
        .iter()
        .skip(1)
        .map(|word| word.to_string_lossy().into_owned())
        .collect();
    let asks = row
        .as_ref()
        .map_or(spec::Asks::Always, spec::Spec::confirmation);
    let ctx = Context::new(&cli.global, &terminal, mode, &words, asks);
    // A wait for another run's lock of the config or an AWS file is a note, so a run that
    // waits for another one's question does not seem to hang.
    if matches!(mode, output::Mode::Text) && !cli.global.quiet {
        hippius::on_lock_wait(|waiting| output::note(&format!("note: {waiting}\n")));
    }
    let ran = guarded(&ctx, || {
        spec::row(&path).and_then(|_| execute(&ctx, &cli.command))
    });
    let warnings = run_warnings(&ctx);
    let print = |mut document: Value, layout: Layout| {
        if !sets_next_step(&cli.command) {
            commands::drop_next_step(&mut document);
        }
        if let Some(note) = output::empty_note(&document, layout) {
            ctx.note(note);
        }
        if let (Layout::DryRun, Ok(row)) = (layout, &row) {
            spec::add_confirmation(row, &mut document);
        }
        output::add_warnings(mode, &mut document, &warnings);
        output::document(mode, &document, ctx.side_effects(), layout)
    };
    match ran {
        // Only a run given `--dry-run` printed a dry run; a server's answer with the same
        // fields is printed as it came. A list command's dry run is a document too.
        Ok(Printed::Document(document)) if cli.global.dry_run => print(document, Layout::DryRun),
        Ok(Printed::Document(document)) => print(document, Layout::Document),
        // Only a command that prints a list envelope prints a list: a `get` whose answer holds
        // `items` and `next_page_token` is one object, with no page hint.
        Ok(Printed::List(document)) => print(document, Layout::List),
        Ok(Printed::Raw(bytes)) => {
            output::warnings_aside(mode, &warnings);
            output::raw(mode, &bytes, ctx.side_effects())
        }
        Ok(Printed::Body(bytes)) => {
            output::warnings_aside(mode, &warnings);
            output::body(&terminal, mode, &bytes, ctx.side_effects())
        }
        Err(error) => output::failure_warned(mode, &error, &warnings),
    }
}

/// The refusal of a global flag given more than once, which would leave unsaid which value
/// counts.
fn repeated_global(args: &[OsString]) -> Option<hippius::Error> {
    let name = crate::cli::repeated_global(args)?;
    let flag = if name == "output" {
        "`--output` or `--json`".to_owned()
    } else {
        format!("`--{name}`")
    };
    Some(hippius::Error::usage(
        format!("{flag} is given more than once"),
        format!("pass {flag} once"),
    ))
}

/// The warnings of the whole run, for JSON's `warnings`: the command's own run warnings and
/// those of answers that marked their route deprecated, which were printed as they came, then
/// those of the config file, printed here in text mode. Also after a failure: a command can
/// write the config and then fail at a later step, and an answer that failed can say its route
/// is deprecated. JSON mode writes no
/// warning lines, so the `warnings` array beside the document or the error is where an agent
/// sees them.
fn run_warnings(ctx: &Context<'_>) -> Vec<String> {
    let mut warnings = ctx.take_warnings();
    let split_link = hippius::take_split_link().map(|path| split_link_warning(&path));
    for warning in split_link
        .into_iter()
        .chain(hippius::take_unknown_keys_warning())
    {
        ctx.warning(&warning);
        warnings.push(warning);
    }
    warnings
}

/// The refusal of a command line that holds a seed phrase or an API token the run knows, which
/// an error that quoted a word of it would copy into a log or a transcript.
fn secret_in_argv(args: &[OsString]) -> Option<hippius::Error> {
    let line = args
        .iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    if hippius::holds_phrase(&line) {
        Some(hippius::phrase_in_argv())
    } else if holds_a_known_token(&line) {
        Some(token_in_argv())
    } else {
        None
    }
}

/// Whether `line`, the command line, holds `HIPPIUS_TOKEN` or a token a profile of the config
/// file stores, in any spelling an error would redact it in. Best effort: a config file that
/// cannot be read holds no token here, and the error that follows names that file, not the
/// command line.
fn holds_a_known_token(line: &str) -> bool {
    let variable = crate::context::env_os("HIPPIUS_TOKEN")
        .and_then(|value| value.into_string().ok())
        .and_then(|value| {
            hippius::Token::new(&value, &hippius::Source::Variable("HIPPIUS_TOKEN")).ok()
        });
    variable.is_some_and(|token| token.is_in(line))
        || crate::context::config().is_ok_and(|config| config.holds_a_stored_token(line))
}

/// The refusal of a command line that holds an API token ([`holds_a_known_token`]): `usage`,
/// with nothing of the command line in it, as [`hippius::phrase_in_argv`] is for a seed phrase.
fn token_in_argv() -> hippius::Error {
    hippius::Error::usage(
        "the command line holds an API token, so it is not repeated here; treat the token as \
         exposed to anyone who can read this machine's process list or shell history",
        "pass a token with `hipc auth login --token-file <path>` or at its prompt, never on the \
         command line, and replace an exposed one in the console settings",
    )
}

/// The words of the command `matches` ran, after `hipc`, such as `["compute", "vms",
/// "create"]`: the path `hipc schema` lists it under.
fn command_path(matches: &clap::ArgMatches) -> Vec<String> {
    let mut path = Vec::new();
    let mut at = matches;
    while let Some((name, sub)) = at.subcommand() {
        path.push(name.to_owned());
        at = sub;
    }
    path
}

/// Sets the soft limit on core dump size to zero, so a crash leaves no dump of memory that
/// may hold the token (tenet 8), and on Linux marks the process not dumpable: a core handler
/// that `core_pattern` pipes to (systemd-coredump, apport) is not bound by the limit, and
/// another process of the same user cannot attach to it or read its memory. Best effort: a
/// system that refuses either runs `hipc` as before. The hard limit is left alone.
fn disable_core_dumps() {
    use rustix::process::{Resource, getrlimit, setrlimit};
    let mut limit = getrlimit(Resource::Core);
    limit.current = Some(0);
    let _best_effort = setrlimit(Resource::Core, limit);
    #[cfg(target_os = "linux")]
    {
        use rustix::process::{DumpableBehavior, set_dumpable_behavior};
        let _best_effort = set_dumpable_behavior(DumpableBehavior::NotDumpable);
    }
}

/// The warning after a write to a config file that had another hard link (tenet 8): the
/// write renamed a new file over this name, so the other link still holds the old contents.
fn split_link_warning(path: &std::path::Path) -> String {
    format!(
        "the config file {} had another hard link, which still holds its old contents, tokens \
         included; only this name was changed",
        hippius::phrase::quote(path.display())
    )
}

#[cfg(not(panic = "unwind"))]
compile_error!(
    "hipc must be built with `panic = \"unwind\"`: `run::guarded` turns a panic into one error \
     object (tenet 4), and an abort would print nothing"
);

/// Runs `command`, turning a panic into an `internal` error with what the run has done to
/// the server so far, so a bug after a write still says `side_effects: partial` (tenet 4)
/// and the output stays one error object, without the default panic message beside it. The
/// panic's own text is not shown: it can quote a value from the server or a file.
fn guarded(ctx: &Context<'_>, command: impl FnOnce() -> Result<Printed>) -> Result<Printed> {
    // The default message is silenced for the command alone: a panic outside it, such as
    // in printing the result, still says what it was.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(command));
    std::panic::set_hook(previous);
    ran.unwrap_or_else(|_| {
        let error = hippius::Error::internal("the command stopped on a bug in `hipc`")
            .with_side_effects(ctx.side_effects());
        // After a write that may have been done, the library's hint says to check whether it
        // was before running the command again (tenet 15); a report can wait for that.
        Err(match ctx.side_effects() {
            hippius::SideEffects::Partial => error,
            hippius::SideEffects::None => error.with_hint(format!(
                "report it at `{}` with the command line, without any secret",
                hippius::ISSUES_URL
            )),
        })
    })
}

/// Whether `command` is one of the flows tenet 3 lets add their own `next_step`,
/// each after dropping the server's ([`commands::drop_next_step`]). Every other document
/// loses a top-level `next_step` before it is printed.
fn sets_next_step(command: &Command) -> bool {
    matches!(
        command,
        Command::Drive(Drive::Subscription(DriveSubscription::Subscribe(_)))
            | Command::S3(S3::Subscription(S3Subscription::Subscribe(_)))
            | Command::Billing(Billing::AutoReload(AutoReload::CardSetup))
            | Command::Cdn(Cdn::Hostnames(CdnHostnames::DomainConnect { .. }))
    )
}

fn execute(ctx: &Context<'_>, command: &Command) -> Result<Printed> {
    match command {
        Command::Version => Ok(Printed::Document(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "target": env!("HIPC_TARGET"),
        }))),
        Command::Status => commands::status(ctx),
        Command::Schema { path } => Ok(Printed::Raw(
            format!("{}\n", spec::schema(path)?).into_bytes(),
        )),
        Command::Completions { shell } => {
            let mut script = Vec::new();
            clap_complete::generate(*shell, &mut Cli::command(), "hipc", &mut script);
            Ok(Printed::Raw(script))
        }
        Command::Auth(auth) => commands::auth::run(ctx, auth),
        Command::Account(account) => commands::account::run(ctx, account),
        Command::Billing(billing) => commands::billing::run(ctx, billing),
        Command::Drive(drive) => commands::drive::run(ctx, drive),
        Command::S3(s3) => commands::s3::run(ctx, s3),
        Command::Hub(hub) => commands::hub::run(ctx, hub),
        Command::Compute(compute) => commands::compute::run(ctx, compute),
        Command::Cdn(cdn) => commands::cdn::run(ctx, cdn),
        Command::Support(support) => commands::support::run(ctx, support),
        Command::Network(network) => commands::network::run(ctx, network),
        Command::Api(api) => commands::api::run(ctx, api),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;
    use hippius::Code;

    use super::*;

    /// The process cannot dump core once `hipc` starts: the soft limit is zero.
    #[test]
    fn core_dumps_are_disabled() {
        use rustix::process::{Resource, getrlimit, setrlimit};
        // Raised first, where the hard limit allows, so the check does not pass on a host
        // whose soft limit is zero already.
        let mut limit = getrlimit(Resource::Core);
        limit.current = limit.maximum;
        setrlimit(Resource::Core, limit).expect("raise the soft limit to the hard one");
        disable_core_dumps();
        let limit = rustix::process::getrlimit(rustix::process::Resource::Core);
        assert_eq!(limit.current, Some(0));
        #[cfg(target_os = "linux")]
        assert_eq!(
            rustix::process::dumpable_behavior().expect("read the dumpable flag"),
            rustix::process::DumpableBehavior::NotDumpable
        );
    }

    #[test]
    fn the_split_link_warning_names_the_file() {
        assert_eq!(
            split_link_warning(std::path::Path::new("/h/.hippius/config.toml")),
            "the config file `/h/.hippius/config.toml` had another hard link, which still \
             holds its old contents, tokens included; only this name was changed"
        );
    }

    /// A panic becomes one `internal` error, with what the run had done to the server,
    /// rather than Rust text and exit 101.
    #[test]
    fn a_panic_is_an_internal_error() {
        let cli = Cli::try_parse_from(["hipc", "version"]).unwrap();
        let terminal = Terminal::detect();
        let mode = terminal.resolve(&cli.global);
        let ctx = Context::new(&cli.global, &terminal, mode, &[], spec::Asks::Never);
        let error = guarded(&ctx, || panic!("a bug")).unwrap_err();
        assert_eq!(error.code(), Code::Internal);
        assert_eq!(error.message(), "the command stopped on a bug in `hipc`");
        assert!(matches!(error.side_effects(), hippius::SideEffects::None));
        assert!(matches!(
            guarded(&ctx, || Ok(Printed::Raw(Vec::new()))),
            Ok(Printed::Raw(_))
        ));
        // After a write, the hint says to check whether it was done, not to report first.
        ctx.mark_written();
        let error = guarded(&ctx, || panic!("a bug")).unwrap_err();
        assert!(matches!(
            error.side_effects(),
            hippius::SideEffects::Partial
        ));
        assert_eq!(error.hint(), Some(hippius::CHECK_FIRST_HINT));
    }
}
