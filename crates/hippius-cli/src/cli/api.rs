//! The grammar of `hipc api`.

pub(crate) mod rules;

use std::path::PathBuf;

use clap::Args;
use hippius::RouteFlag;

use crate::cli::api::rules::Voice;
use crate::cli::reveal_long_help;

#[derive(Debug, Args)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is one independent command-line flag, which clap parses into a field"
)]
pub(crate) struct Api {
    /// GET, HEAD, POST, PUT, PATCH, or DELETE
    pub(crate) method: String,
    /// A path such as /accounts/ (sent under /api), or a full URL on the configured API origin
    pub(crate) path: String,
    /// A JSON request body, from this file or - for stdin; POST, PUT, PATCH, and DELETE only
    ///
    /// It is read only as a regular file, never through a symlink; /dev/stdin and <(...) are
    /// refused, so pass - for stdin.
    #[arg(long, value_name = "PATH")]
    pub(crate) body_file: Option<PathBuf>,
    /// Follow every page: one JSON array, or one table in text mode; GET only
    ///
    /// The same flag, and the same output, as --all on every list command.
    #[arg(long)]
    pub(crate) all: bool,
    /// Print a secret; required for the requests that return one, or nothing is sent
    #[arg(long, long_help = reveal_long_help())]
    pub(crate) reveal: bool,
    // The help of the three route flags is built from the route rows (`rules`), so it names
    // every route each flag gets past.
    #[arg(
        long,
        help = flag_help(RouteFlag::ChangeIsolation),
        long_help = flag_long_help(RouteFlag::ChangeIsolation)
    )]
    pub(crate) change_isolation: bool,
    #[arg(
        long,
        help = flag_help(RouteFlag::Public),
        long_help = flag_long_help(RouteFlag::Public)
    )]
    pub(crate) public: bool,
    #[arg(
        long,
        help = flag_help(RouteFlag::RemoveOthers),
        long_help = flag_long_help(RouteFlag::RemoveOthers)
    )]
    pub(crate) remove_others: bool,
}

/// What `flag` does, as the start of its short help.
fn purpose(flag: RouteFlag) -> &'static str {
    match flag {
        RouteFlag::ChangeIsolation => "Change which peers of the network can reach each other",
        RouteFlag::Public => {
            "Let a write publish a bucket that may be private, or open the Hub namespace"
        }
        RouteFlag::RemoveOthers => {
            "Let a write that replaces a whole list or document remove the entries it leaves out"
        }
    }
}

/// Why `--yes` alone does not send what `flag` guards, after the short help in its long help.
fn why(flag: RouteFlag) -> &'static str {
    match flag {
        RouteFlag::ChangeIsolation => {
            "The deny_intra mode deletes the catch-all policy, so --yes alone sends nothing."
        }
        RouteFlag::Public => {
            "A zone makes a private bucket readable by anyone, which is known only from the \
             answer, and a bucket policy can name any principal, so --yes alone sends nothing, \
             as with hipc cdn zones create. A public namespace lets anyone pull its images and \
             models without logging in, as with hipc hub namespace update --visibility public."
        }
        RouteFlag::RemoveOthers => {
            "hipc api does not read the list first, so without the flag it sends nothing. hipc \
             compute firewall update, hipc cdn rules update, and hipc account members update \
             read the list, and refuse only a write that removes an entry."
        }
    }
}

/// The short help of `flag`: what it does and every route it is required for.
fn flag_help(flag: RouteFlag) -> String {
    let mut routes = rules::needs(Voice::Help, flag);
    if rules::leaves_out(flag) {
        routes.push("an operator route hipc guards the same way".to_owned());
    }
    format!(
        "{}; required for {}",
        purpose(flag),
        hippius::phrase::join(&routes, "and")
    )
}

/// The long help of `flag`: the short help, why `--yes` is not enough, and the rules every
/// route of it follows.
fn flag_long_help(flag: RouteFlag) -> String {
    let mut rules = rules::flag_rules(Voice::Help, flag);
    rules[..1].make_ascii_uppercase();
    format!("{}\n\n{} {rules}.", flag_help(flag), why(flag))
}
