//! The grammar of the project and billing-account writes under `hipc account`: create,
//! rename, archive, a project's budget and quota, and the project transfers.
//!
//! The API documents none of these bodies; the fields and their rules are the ones the
//! console's forms send and check.

use clap::{ArgAction, Args, Subcommand};

use crate::cli::{Usd, examples, number, usd};

/// `hipc account projects create`.
#[derive(Debug, Args)]
pub(crate) struct ProjectCreate {
    /// The project's name, 1 to 64 characters on one line, none invisible, sent trimmed
    #[arg(long, value_parser = principal_name)]
    pub(crate) name: String,
}

/// `hipc account projects update`.
#[derive(Debug, Args)]
pub(crate) struct ProjectUpdate {
    /// The project id, proj_ and 20 lowercase hex characters, from hipc account projects list
    #[arg(value_parser = project_id)]
    pub(crate) project_id: String,
    /// The new name, 1 to 64 characters on one line, none invisible, sent trimmed
    #[arg(long, value_parser = principal_name)]
    pub(crate) name: String,
}

/// `hipc account projects transfer`.
#[derive(Debug, Args)]
pub(crate) struct ProjectTransfer {
    /// The project id, proj_ and 20 lowercase hex characters, from hipc account projects list
    #[arg(value_parser = project_id)]
    pub(crate) project_id: String,
    /// The account the project is offered to: acct_ and 20 lowercase hex characters
    #[arg(long, value_name = "ACCOUNT_ID", value_parser = account_id)]
    pub(crate) to_account: String,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Budget {
    /// A project's monthly budget: its limit, alert thresholds, and this month's spend
    ///
    /// monthly_limit is null when the project has no budget. Amounts are credits as decimal
    /// strings (1 credit = $1). The Default project has no id here, so it has no budget.
    #[command(after_help = examples(&[
        "hipc account projects budget get proj_0a1b2c3d4e5f60718293",
    ]))]
    Get {
        /// The project id, proj_ and 20 lowercase hex characters, from hipc account projects
        /// list
        #[arg(value_parser = project_id)]
        project_id: String,
    },
    /// Set a project's monthly budget; needs --yes
    ///
    /// The PUT replaces the whole budget, so every field is given. The owner, the billing
    /// admins of the project's billing account, and its billing email get a mail when the
    /// month's spend crosses each --alert-at. With --block-launches true, nothing new that
    /// adds cost starts once the limit is reached; what runs keeps running and is billed.
    ///
    /// A budget needs a project other than Default, which has no id here. To cap what launches
    /// spend in a month, create a project with hipc account projects create, launch in it with
    /// --project or after hipc account projects use, then set its budget.
    #[command(after_help = examples(&[
        "hipc account projects budget update proj_0a1b2c3d4e5f60718293 --monthly-limit 100 \
         --alert-at 50 --alert-at 90 --alert-at 100 --block-launches false --yes",
    ]))]
    Update(BudgetUpdate),
    /// Remove a project's monthly budget, so only the account's limits hold; needs --yes
    #[command(after_help = examples(&[
        "hipc account projects budget delete proj_0a1b2c3d4e5f60718293 --yes",
    ]))]
    Delete {
        /// The project id, proj_ and 20 lowercase hex characters, from hipc account projects
        /// list
        #[arg(value_parser = project_id)]
        project_id: String,
    },
}

/// `hipc account projects budget update`.
#[derive(Debug, Args)]
pub(crate) struct BudgetUpdate {
    /// The project id, proj_ and 20 lowercase hex characters, from hipc account projects list
    #[arg(value_parser = project_id)]
    pub(crate) project_id: String,
    /// The month's spend limit in US dollars to the cent, more than 0
    ///
    /// budget get reads amounts back in credits (1 credit = $1).
    #[arg(long, value_name = "USD", value_parser = budget_limit)]
    pub(crate) monthly_limit: Usd,
    /// A share of the limit, 1 to 100, at which a mail is sent; repeat for more, up to 5
    #[arg(
        long = "alert-at",
        value_name = "PERCENT",
        required = true,
        value_parser = clap::value_parser!(u8).range(1..=100)
    )]
    pub(crate) alert_at: Vec<u8>,
    /// Refuse new launches that add cost once the limit is reached: true or false
    #[arg(long, value_name = "BOOL", action = ArgAction::Set, required = true)]
    pub(crate) block_launches: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Quota {
    /// A project's own compute limits, what it uses, and the account's limits
    ///
    /// A limit of null means the project has none of its own: only the account's, which holds
    /// for all its projects together, applies.
    #[command(after_help = examples(&[
        "hipc account projects quota get proj_0a1b2c3d4e5f60718293",
    ]))]
    Get {
        /// The project id, proj_ and 20 lowercase hex characters, from hipc account projects
        /// list
        #[arg(value_parser = project_id)]
        project_id: String,
    },
    /// Change the project's own compute limits given; the others are kept; needs --yes
    ///
    /// Each limit is a whole number up to 2147483647, or none to leave only the account's
    /// limit. The server refuses a limit above the account's.
    #[command(after_help = examples(&[
        "hipc account projects quota update proj_0a1b2c3d4e5f60718293 --vms 5 --memory 64 --yes",
        "hipc account projects quota update proj_0a1b2c3d4e5f60718293 --vcpus none --yes",
    ]))]
    Update(QuotaUpdate),
}

/// `hipc account projects quota update`. The flags are the dimensions the console names,
/// in its order, which is the order they are sent in.
#[derive(Debug, Args)]
#[command(group(
    clap::ArgGroup::new("limits").required(true).multiple(true).args([
        "vms",
        "databases",
        "clusters",
        "vcpus",
        "memory",
        "disk",
        "public_ips",
        "runners",
    ])
))]
pub(crate) struct QuotaUpdate {
    /// The project id, proj_ and 20 lowercase hex characters, from hipc account projects list
    #[arg(value_parser = project_id)]
    pub(crate) project_id: String,
    /// VMs, or none
    #[arg(long, value_name = "COUNT", value_parser = quota_limit)]
    pub(crate) vms: Option<QuotaLimit>,
    /// Managed databases, or none
    #[arg(long, value_name = "COUNT", value_parser = quota_limit)]
    pub(crate) databases: Option<QuotaLimit>,
    /// Kubernetes clusters, or none
    #[arg(long, value_name = "COUNT", value_parser = quota_limit)]
    pub(crate) clusters: Option<QuotaLimit>,
    /// Virtual CPUs, or none
    #[arg(long, value_name = "COUNT", value_parser = quota_limit)]
    pub(crate) vcpus: Option<QuotaLimit>,
    /// Memory, or none
    #[arg(long, value_name = "GB", value_parser = quota_limit)]
    pub(crate) memory: Option<QuotaLimit>,
    /// Disk, or none
    #[arg(long, value_name = "GB", value_parser = quota_limit)]
    pub(crate) disk: Option<QuotaLimit>,
    /// Public IPs, or none
    #[arg(long, value_name = "COUNT", value_parser = quota_limit)]
    pub(crate) public_ips: Option<QuotaLimit>,
    /// Runners, or none
    #[arg(long, value_name = "COUNT", value_parser = quota_limit)]
    pub(crate) runners: Option<QuotaLimit>,
}

impl QuotaUpdate {
    /// The limits given, each with the key the API names it by, in the console's order.
    pub(crate) fn limits(&self) -> Vec<(&'static str, QuotaLimit)> {
        [
            ("vms", self.vms),
            ("databases", self.databases),
            ("clusters", self.clusters),
            ("vcpus", self.vcpus),
            ("memory_gb", self.memory),
            ("disk_gb", self.disk),
            ("public_ips", self.public_ips),
            ("runners", self.runners),
        ]
        .into_iter()
        .filter_map(|(key, limit)| limit.map(|limit| (key, limit)))
        .collect()
    }
}

/// One of a project's own compute limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuotaLimit {
    /// At most this many.
    At(u64),
    /// No limit of its own (`null`): only the account's applies.
    None,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ProjectTransfers {
    /// The project transfers of the token's own account: offered to it, then offered by it
    ///
    /// A personal route: it reads the token's own account's transfers whichever account is
    /// selected. hipc adds direction to each, incoming or outgoing, which the API does not
    /// send; from_account and to_account name the accounts. Text mode shows the project and
    /// the accounts by name and id.
    #[command(after_help = examples(&[
        "hipc account project-transfers list",
    ]))]
    List,
    /// Accept a project offered to the token's own account; needs --yes
    ///
    /// The project moves into the token's own account, whichever account is selected, and
    /// is billed to --bill-to, or to the Default billing account. The transfers are read
    /// first, so the question names the project and the account offering it, and an id not
    /// offered to the token's own account is refused.
    #[command(after_help = examples(&[
        "hipc account project-transfers accept 5 --yes",
        "hipc account project-transfers accept 5 --bill-to bill_9f8e7d6c5b4a39281706 --yes",
    ]))]
    Accept {
        /// The transfer id from hipc account project-transfers list
        #[arg(value_parser = number(TRANSFER_ID))]
        transfer_id: u64,
        /// The billing account of the token's own account that pays for the project:
        /// default, or bill_ and 20 lowercase hex characters
        ///
        /// Not --billing, which is the global flag that picks a billing account.
        #[arg(long, value_name = "BILLING_ACCOUNT_ID", value_parser = bill_to)]
        bill_to: Option<String>,
    },
    /// Decline a project offered to the token's own account; needs --yes
    ///
    /// The transfers are read first, so the question names the project and the account
    /// offering it, and an id not offered to the token's own account is refused.
    #[command(after_help = examples(&[
        "hipc account project-transfers decline 5 --yes",
    ]))]
    Decline {
        /// The transfer id from hipc account project-transfers list
        #[arg(value_parser = number(TRANSFER_ID))]
        transfer_id: u64,
    },
    /// Cancel a pending transfer the token's own account offered; needs --yes
    ///
    /// The transfers are read first, so the question names the project and the account it
    /// was offered to, and an id the token's own account did not offer is refused.
    #[command(after_help = examples(&[
        "hipc account project-transfers cancel 5 --yes",
    ]))]
    Cancel {
        /// The transfer id from hipc account project-transfers list
        #[arg(value_parser = number(TRANSFER_ID))]
        transfer_id: u64,
    },
}

const TRANSFER_ID: &str =
    "a transfer id is a whole number, from `hipc account project-transfers list`";

/// The most characters a project or billing account name may have: the console's limit.
const MAX_NAME: usize = 64;

/// A project or billing account name: 1 to 64 characters once trimmed, as the console
/// checks it, sent trimmed as the console sends it. A control character, a tab or a newline
/// included, is refused: the console shows a name on one line, where a tab or a newline
/// would misalign or split it. So is a character that changes how text reads without being
/// seen (a bidi override, a zero-width space), with which two names that look the same
/// would be two projects, and a person would pick the wrong one.
pub(crate) fn principal_name(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > MAX_NAME {
        return Err(format!("a name is 1 to {MAX_NAME} characters"));
    }
    // The line and paragraph separators break a line as a newline does, so they get this
    // message rather than the one about invisible characters.
    if value
        .chars()
        .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
    {
        return Err(
            "a name holds no control character or line break, a tab or a newline \
                    included, since it is shown on one line"
                .to_owned(),
        );
    }
    if value.chars().any(crate::output::changes_how_text_reads) {
        return Err(
            "a name holds no invisible character, such as a bidi override or a \
                    zero-width space, since two names would look the same"
                .to_owned(),
        );
    }
    Ok(value.to_owned())
}

/// A project id, which goes into the path. `default` is refused: the Default project is
/// the account itself and has no id these routes take.
pub(crate) fn project_id(value: &str) -> Result<String, String> {
    match hippius::ProjectId::parse(value, &hippius::Source::Argument) {
        Ok(Some(_)) => Ok(value.to_owned()),
        _ => Err(
            "a project id is `proj_` and 20 lowercase hex characters, from \
                  `hipc account projects list`; the Default project has none"
                .to_owned(),
        ),
    }
}

/// A billing account id, which goes into the path; `default` is refused as for
/// [`project_id`].
pub(crate) fn billing_account_id(value: &str) -> Result<String, String> {
    match hippius::BillingId::parse(value, &hippius::Source::Argument) {
        Ok(Some(_)) => Ok(value.to_owned()),
        _ => Err(
            "a billing account id is `bill_` and 20 lowercase hex characters, from \
                  `hipc account billing-accounts list`; the Default billing account has none"
                .to_owned(),
        ),
    }
}

/// The billing account an accepted project goes to: `default` or an id, as the costs read
/// takes it.
fn bill_to(value: &str) -> Result<String, String> {
    match hippius::BillingId::parse(value, &hippius::Source::Argument) {
        Ok(_) => Ok(value.to_owned()),
        Err(_) => Err(
            "a billing account is `default`, or `bill_` and 20 lowercase hex \
                       characters, from `hipc account billing-accounts list`"
                .to_owned(),
        ),
    }
}

/// The account a project is offered to: an account id, never `self`, which names no
/// account the server can find.
fn account_id(value: &str) -> Result<String, String> {
    match hippius::Selection::parse(value, &hippius::Source::Argument) {
        Ok(hippius::Selection::Member(_)) => Ok(value.to_owned()),
        _ => Err("an account id is `acct_` and 20 lowercase hex characters".to_owned()),
    }
}

/// A budget's limit: US dollars to the cent, more than 0, as the console requires.
fn budget_limit(value: &str) -> Result<Usd, String> {
    let limit = usd(value)?;
    if limit == Usd::from_cents(0) {
        return Err("a monthly limit is more than `0`".to_owned());
    }
    Ok(limit)
}

/// The largest quota limit sent: the largest signed 32-bit integer, the widest column a
/// server is likely to store a count in, so a larger one is refused here rather than
/// overflow or be cut there.
const MAX_QUOTA: u64 = i32::MAX as u64;

/// A quota limit: a whole number up to [`MAX_QUOTA`], or `none`.
fn quota_limit(value: &str) -> Result<QuotaLimit, String> {
    if value == "none" {
        return Ok(QuotaLimit::None);
    }
    let limit: u64 = value
        .parse()
        .map_err(|_| "a limit is a whole number, such as `5`, or `none`".to_owned())?;
    if limit > MAX_QUOTA {
        return Err(format!("a limit is at most `{MAX_QUOTA}`"));
    }
    Ok(QuotaLimit::At(limit))
}
