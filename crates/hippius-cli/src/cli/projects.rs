//! The grammar of the project and billing-account writes under `hipc account`: create,
//! rename, archive, a project's budget and quota, and the project transfers.
//!
//! The API documents none of these bodies; the fields and their rules are the ones the
//! console's forms send and check.

use clap::{ArgAction, Args, Subcommand};

use crate::cli::ids::account_id;
use crate::cli::values::{Spaces, label};
use crate::cli::{BillingAccount, ProjectTarget, examples, number};
use crate::units::{Usd, usd, whole_number};

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
    #[command(flatten)]
    pub(crate) project: ProjectTarget,
    /// The new name, 1 to 64 characters on one line, none invisible, sent trimmed
    #[arg(long, value_parser = principal_name)]
    pub(crate) name: String,
}

/// `hipc account projects transfer`.
#[derive(Debug, Args)]
pub(crate) struct ProjectTransfer {
    #[command(flatten)]
    pub(crate) project: ProjectTarget,
    /// The account the project is offered to: acct_ and 20 lowercase hex characters
    #[arg(long, value_name = "ACCOUNT_ID", value_parser = account_id)]
    pub(crate) to_account: hippius::AccountId,
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
    Get(ProjectTarget),
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
    Delete(ProjectTarget),
}

/// `hipc account projects budget update`.
#[derive(Debug, Args)]
pub(crate) struct BudgetUpdate {
    #[command(flatten)]
    pub(crate) project: ProjectTarget,
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
    Get(ProjectTarget),
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
    #[command(flatten)]
    pub(crate) project: ProjectTarget,
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
        #[arg(long, value_name = "BILLING_ACCOUNT_ID", value_parser = BillingAccount::parse)]
        bill_to: Option<BillingAccount>,
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

/// A project or billing account name: a [`label`] of 1 to 64 characters once trimmed, as the
/// console checks it, sent trimmed as the console sends it.
pub(crate) fn principal_name(value: &str) -> Result<String, String> {
    label(value, 1..=MAX_NAME, Spaces::Trimmed, "a name")
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
    let limit = whole_number(value)
        .ok_or_else(|| "a limit is a whole number, such as `5`, or `none`".to_owned())?;
    if limit > MAX_QUOTA {
        return Err(format!("a limit is at most `{MAX_QUOTA}`"));
    }
    Ok(QuotaLimit::At(limit))
}
