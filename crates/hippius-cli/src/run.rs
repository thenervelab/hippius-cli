//! Parses argv, runs one command, and hands its result to the output module.

use std::ffi::OsString;
use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches as _};
use hippius::Result;
use serde_json::json;

use crate::cli::{
    Account, Auth, AutoReload, Billing, Cdn, CdnHostnames, Cli, Command, Drive, DriveSubscription,
    S3, S3Subscription,
};
use crate::commands::{self, Printed};
use crate::context::Context;
use crate::output::{self, Terminal};
use crate::spec;

/// Runs `hipc` with these arguments and returns the process exit code.
pub(crate) fn run(args: &[OsString]) -> ExitCode {
    disable_core_dumps();
    let terminal = Terminal::detect();
    let parsed = crate::cli::parse_command()
        .try_get_matches_from(args)
        .and_then(|matches| {
            Cli::from_arg_matches(&matches).map(|cli| (cli, command_path(&matches)))
        });
    let (cli, path) = match parsed {
        Ok(parsed) => parsed,
        Err(error) => {
            let mode = terminal.resolve_from_args(args);
            // An access key typed on the command line is refused without the words: clap's
            // message would quote them.
            let line = args
                .iter()
                .map(|arg| arg.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            if hippius::holds_phrase(&line) {
                return output::failure(mode, &hippius::phrase_in_argv());
            }
            return output::clap_error(mode, &error, args);
        }
    };
    let mode = terminal.resolve(&cli.global);
    if let Some(name) = crate::cli::repeated_global(args) {
        let flag = if name == "output" {
            "`--output` or `--json`".to_owned()
        } else {
            format!("`--{name}`")
        };
        let error = hippius::Error::usage(
            format!("{flag} is given more than once"),
            format!("pass {flag} once"),
        );
        return output::failure(mode, &error);
    }
    let words: Vec<String> = args
        .iter()
        .skip(1)
        .map(|word| word.to_string_lossy().into_owned())
        .collect();
    let ctx = Context::new(&cli.global, &terminal, mode, &words);
    // A wait for another run's lock of the config or an AWS file is a note, so a run that
    // waits for another one's question does not seem to hang.
    if matches!(mode, output::Mode::Text) && !cli.global.quiet {
        hippius::on_lock_wait(|waiting| output::note(&format!("note: {waiting}\n")));
    }
    let ran = guarded(&ctx, || execute(&ctx, &cli.command));
    // Also after a failure: a command can write the config and then fail at a later step.
    // Text mode warns on stderr; JSON mode, which writes no warning lines, adds the text to a
    // `warnings` array beside the document or the error, so an agent sees it too.
    let mut warnings = Vec::new();
    if let Some(path) = hippius::take_split_link() {
        let warning = split_link_warning(&path);
        ctx.warning(&warning);
        warnings.push(warning);
    }
    match ran {
        Ok(Printed::Document(mut document)) => {
            if !sets_next_step(&cli.command) {
                commands::drop_next_step(&mut document);
            }
            if let Some(note) = output::empty_note(&document) {
                ctx.note(note);
            }
            spec::add_confirmation(&path, &mut document);
            output::add_warnings(mode, &mut document, &warnings);
            output::document(mode, &document, ctx.side_effects())
        }
        Ok(Printed::Raw(bytes)) => output::raw(mode, &bytes, ctx.side_effects()),
        Ok(Printed::Body(bytes)) => output::body(&terminal, mode, &bytes, ctx.side_effects()),
        Err(error) => output::failure_warned(mode, &error, &warnings),
    }
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

/// Runs `command`, turning a panic into an `internal` error with what the run has done to
/// the server so far, so a bug after a write still says `side_effects: partial` (tenet 4)
/// and the output stays one error object, without the default panic message beside it. The panic's own text is not shown:
/// it can quote a value from the server or a file.
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

/// Whether `command` is one of the three flows that add their own `next_step` (tenet 3),
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
        Command::Auth(Auth::Login {
            token_file,
            access_key_file,
            access_key,
            new_account,
        }) => commands::auth::login(
            ctx,
            token_file.as_deref(),
            access_key_file.as_deref(),
            *access_key,
            *new_account,
        ),
        Command::Auth(Auth::Status) => commands::auth::status(ctx),
        Command::Auth(Auth::Logout) => commands::auth::logout(ctx),
        Command::Account(Account::List) => commands::account::list(ctx),
        Command::Account(Account::Get) => commands::account::get(ctx),
        Command::Account(Account::Notifications(command)) => {
            commands::account::notifications(ctx, command)
        }
        Command::Account(Account::MarketingConsent(command)) => {
            commands::account::marketing_consent(ctx, command)
        }
        Command::Account(Account::Use { account_id }) => commands::account::select(ctx, account_id),
        Command::Account(Account::Members(command)) => commands::account::members(ctx, command),
        Command::Account(Account::Invites(command)) => commands::account::invites(ctx, command),
        Command::Account(Account::Projects(command)) => commands::projects::projects(ctx, command),
        Command::Account(Account::ProjectTransfers(command)) => {
            commands::projects::project_transfers(ctx, command)
        }
        Command::Account(Account::BillingAccounts(command)) => {
            commands::projects::billing_accounts(ctx, command)
        }
        Command::Account(Account::Referrals(command)) => commands::referrals::run(ctx, command),
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
        let ctx = Context::new(&cli.global, &terminal, mode, &[]);
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
