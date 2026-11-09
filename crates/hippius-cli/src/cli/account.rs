//! The grammar of `hipc account`, apart from the project and billing-account writes.

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};
use hippius::phrase;

use crate::cli::{
    Budget, INVITE_ID, MEMBER_ID, ProjectCreate, ProjectTransfer, ProjectTransfers, ProjectUpdate,
    Quota, REFERRAL_ID, examples, number, projects,
};

#[derive(Debug, Subcommand)]
pub(crate) enum Account {
    /// The accounts the token can act in: the token's own first, then memberships
    #[command(after_help = examples(&[
        "hipc account list",
    ]))]
    List,
    /// Select the account later commands act in; does not touch the network
    ///
    /// self goes back to the token's own account. Changing the account also clears the stored
    /// project and billing account, since each belongs to the account left.
    #[command(after_help = examples(&[
        "hipc account use acct_0123456789abcdef0123",
        "hipc account use self",
    ]))]
    Use {
        /// The account: self, or an id of acct_ and 20 lowercase hex characters
        account_id: String,
    },
    /// The selected account
    #[command(after_help = examples(&[
        "hipc account get",
    ]))]
    Get,
    /// The signed-in user's email notification settings: low and zero balance alerts,
    /// marketing, and the rest
    ///
    /// They are the user's own, whichever account is selected.
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc account notifications get",
        "hipc account notifications update --low-credit-alerts true --marketing-emails false",
    ]))]
    Notifications(Notifications),
    /// The signed-in user's consent to marketing email: read it, or withdraw it
    ///
    /// It is the user's own, whichever account is selected.
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc account marketing-consent get",
        "hipc account marketing-consent update --granted false",
    ]))]
    MarketingConsent(MarketingConsent),
    /// The members of the selected account's team, their grants, and their permissions
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc account members list",
        "hipc account members update 12 --grant vms=operator --yes",
    ]))]
    Members(Members),
    /// Invites to the selected account's team
    ///
    /// An invite is an email, lasts 7 days, and is used once; the team holds at most 50 members and
    /// pending invites.
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc account invites list",
        "hipc account invites create --email ana@example.com --grant vms=viewer --yes",
        "hipc account invites accept --token-file invite.txt --yes",
    ]))]
    Invites(Invites),
    /// The account's projects, and the one later commands act in
    ///
    /// A project holds VMs, databases, clusters, runners, S3 buckets and sub tokens, and CDN
    /// zones; a master S3 token stays with the account.
    /// The account itself is the Default project.
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc account projects list",
        "hipc account projects create --name web",
        "hipc account projects use proj_0a1b2c3d4e5f60718293",
    ]))]
    Projects(Projects),
    /// Projects offered to the token's own account, and those it offered to others
    ///
    /// The transfers are the token's own account's, whichever account is selected.
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc account project-transfers list",
        "hipc account project-transfers accept 5 --yes",
    ]))]
    ProjectTransfers(ProjectTransfers),
    /// The account's billing accounts, what each paid, and the one later commands act on
    ///
    /// A billing account is a wallet that pays for projects. The account itself is the Default
    /// billing account.
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc account billing-accounts list",
        "hipc account billing-accounts create --name ops",
        "hipc account billing-accounts costs default",
        "hipc account billing-accounts use bill_9f8e7d6c5b4a39281706",
    ]))]
    BillingAccounts(BillingAccounts),
    /// The signed-in user's referral links, and the signups and top-ups they brought
    ///
    /// Every link shares the user's one referral code; each has its own name and the slug
    /// that tells which link a signup came through. They are the user's own, whichever account
    /// is selected.
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc account referrals list",
        "hipc account referrals create --name telegram",
        "hipc account referrals analytics",
    ]))]
    Referrals(Referrals),
}

#[derive(Debug, Subcommand)]
pub(crate) enum Referrals {
    /// The referral links, each with its name, slug, and code
    #[command(after_help = examples(&[
        "hipc account referrals list",
    ]))]
    List,
    /// Add a named referral link; the first one mints the user's referral code
    ///
    /// It costs nothing. Later links reuse the same code with a new slug.
    #[command(after_help = examples(&[
        "hipc account referrals create --name telegram",
    ]))]
    Create {
        /// A label for the link, 1 to 64 characters, unique among yours; it stays on Hippius
        #[arg(long, value_parser = referral_name)]
        name: String,
    },
    /// Rename a referral link; the code and slug stay the same
    #[command(after_help = examples(&[
        "hipc account referrals update 4 --name discord",
    ]))]
    Update {
        /// The referral link id, from hipc account referrals list
        #[arg(value_parser = number(REFERRAL_ID))]
        referral_id: u64,
        /// The new label, 1 to 64 characters, unique among yours
        #[arg(long, value_parser = referral_name)]
        name: String,
    },
    /// Signups, conversion, and top-up volume the referral code brought, by link
    ///
    /// The referred users' emails come masked. The server keeps the figures for 5 minutes;
    /// --refresh counts them again.
    #[command(after_help = examples(&[
        "hipc account referrals analytics",
        "hipc account referrals analytics --refresh",
    ]))]
    Analytics {
        /// Count the figures again instead of reading the server's 5-minute cache
        #[arg(long)]
        refresh: bool,
    },
}

/// A referral link's name. Swagger's `name` is required, at most 64 characters, and a
/// label, so a blank one or one with a control character is refused here rather than by a
/// 400 from the server.
fn referral_name(value: &str) -> Result<String, String> {
    let valid = !value.trim().is_empty()
        && value.chars().count() <= 64
        && !value.chars().any(char::is_control);
    if valid {
        Ok(value.to_owned())
    } else {
        Err("a referral name is one line of 1 to 64 characters".to_owned())
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum Notifications {
    /// The signed-in user's email notification settings
    #[command(after_help = examples(&[
        "hipc account notifications get",
    ]))]
    Get,
    /// Turn the notification settings given on or off; the others are kept; needs --yes to
    /// turn an alert off
    ///
    /// Only the flags given are sent. --email-enabled false turns every notification email off,
    /// whatever the other settings say. Turning off an alert, --email-enabled,
    /// --low-credit-alerts, --zero-balance-alerts, or --file-status-updates, needs --yes. The
    /// marketing consent is kept apart: hipc account marketing-consent get reads it, and its
    /// receives_marketing says whether marketing email goes out.
    #[command(after_help = examples(&[
        "hipc account notifications update --low-credit-alerts true --marketing-emails false",
        "hipc account notifications update --email-enabled false --yes",
    ]))]
    Update(NotificationsUpdate),
}

/// `hipc account notifications update`: the settings to change, named as the route names them.
#[derive(Debug, Args)]
#[command(group(
    clap::ArgGroup::new("change")
        .required(true)
        .multiple(true)
        .args([
            "email_enabled",
            "low_credit_alerts",
            "zero_balance_alerts",
            "file_status_updates",
            "marketing_emails",
            "provider_advice_emails",
        ])
))]
pub(crate) struct NotificationsUpdate {
    /// Every notification email: true or false
    #[arg(long, value_name = "BOOL")]
    pub(crate) email_enabled: Option<bool>,
    /// The warning that the balance runs low: true or false
    #[arg(long, value_name = "BOOL")]
    pub(crate) low_credit_alerts: Option<bool>,
    /// The alert that the balance reached zero and services are suspended: true or false
    #[arg(long, value_name = "BOOL")]
    pub(crate) zero_balance_alerts: Option<bool>,
    /// The updates when files are pinned or fail: true or false
    #[arg(long, value_name = "BOOL")]
    pub(crate) file_status_updates: Option<bool>,
    /// Marketing and feature updates: true or false
    #[arg(long, value_name = "BOOL")]
    pub(crate) marketing_emails: Option<bool>,
    /// The "New advice for your node(s)" digest of the providers dashboard: true or false
    #[arg(long, value_name = "BOOL")]
    pub(crate) provider_advice_emails: Option<bool>,
}

impl NotificationsUpdate {
    /// Each setting with its value when given, in the order swagger lists them.
    pub(crate) fn fields(&self) -> [(&'static str, Option<bool>); 6] {
        [
            ("email_enabled", self.email_enabled),
            ("low_credit_alerts", self.low_credit_alerts),
            ("zero_balance_alerts", self.zero_balance_alerts),
            ("file_status_updates", self.file_status_updates),
            ("marketing_emails", self.marketing_emails),
            ("provider_advice_emails", self.provider_advice_emails),
        ]
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum MarketingConsent {
    /// The signed-in user's marketing consent: state is yes, no, or never_asked, and
    /// receives_marketing says whether marketing email goes out
    #[command(after_help = examples(&[
        "hipc account marketing-consent get",
    ]))]
    Get,
    /// Withdraw the signed-in user's consent to marketing email
    ///
    /// Only false is taken: consent is given in the console, which shows the wording it is
    /// to. Each withdrawal is added to the consent history, from the settings.
    #[command(after_help = examples(&[
        "hipc account marketing-consent update --granted false",
    ]))]
    Update {
        /// Withdraw the consent with false; true is refused, since consent is given in the console
        #[arg(long, value_name = "BOOL", action = clap::ArgAction::Set)]
        granted: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum Projects {
    /// The account's projects, each with the billing account that pays for it
    #[command(after_help = examples(&[
        "hipc account projects list",
        "hipc account projects list --account acct_0123456789abcdef0123",
    ]))]
    List,
    /// Create a project in the selected account, billed to the Default billing account
    ///
    /// A project is free, so the create is not confirmed. The answer is the project, with
    /// the id --project takes.
    #[command(after_help = examples(&[
        "hipc account projects create --name web",
    ]))]
    Create(ProjectCreate),
    /// Rename a project
    #[command(after_help = examples(&[
        "hipc account projects update proj_0a1b2c3d4e5f60718293 --name web-prod",
    ]))]
    Update(ProjectUpdate),
    /// Archive an empty project; needs --yes
    ///
    /// The API archives the project rather than remove it, and refuses one that still owns
    /// a VM, database, cluster, public IP, CDN zone, bucket, S3 key, or runner.
    #[command(after_help = examples(&[
        "hipc account projects delete proj_0a1b2c3d4e5f60718293 --yes",
    ]))]
    Delete {
        /// The project id, proj_ and 20 lowercase hex characters, from hipc account projects
        /// list
        #[arg(value_parser = projects::project_id)]
        project_id: String,
    },
    /// Offer a project to another account, which accepts or declines it; needs --yes
    ///
    /// The project leaves the selected account once the other account accepts. Only the
    /// account owner can offer one. hipc account project-transfers list shows the offer, and
    /// hipc account project-transfers cancel withdraws it while it is pending. The projects are
    /// read first, and from the token's own account the accounts too, so the question names
    /// the project and both accounts; a project the account does not list, or an offer to the
    /// account the project is in, is refused.
    #[command(after_help = examples(&[
        "hipc account projects transfer proj_0a1b2c3d4e5f60718293 \
         --to-account acct_0123456789abcdef0123 --yes",
    ]))]
    Transfer(ProjectTransfer),
    /// A project's monthly budget
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc account projects budget get proj_0a1b2c3d4e5f60718293",
        "hipc account projects budget delete proj_0a1b2c3d4e5f60718293 --yes",
    ]))]
    Budget(Budget),
    /// A project's own compute limits, inside the account's
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc account projects quota get proj_0a1b2c3d4e5f60718293",
        "hipc account projects quota update proj_0a1b2c3d4e5f60718293 --vms 5 --yes",
    ]))]
    Quota(Quota),
    /// Select the project later commands act in; does not touch the network
    ///
    /// default clears the selection, so commands act in the Default project. Only compute,
    /// databases, Kubernetes, runners, S3 buckets and sub tokens (not a master S3 token), CDN,
    /// and the S3 plan reads take a project; the stored one goes on those, is skipped on reads of
    /// every other route, and refuses a write there until --project default is passed, apart
    /// from the writes that name a project (hipc account projects update, delete, transfer,
    /// budget update and delete, and quota update) or a billing account (hipc account
    /// billing-accounts update), which skip it too. Archiving it clears it.
    /// hipc account use clears it, since a project belongs to one account, and under an
    /// --account that names another account it is not read and cannot be stored.
    #[command(after_help = examples(&[
        "hipc account projects use proj_0a1b2c3d4e5f60718293",
        "hipc account projects use default",
    ]))]
    Use {
        /// The project: default, or an id of proj_ and 20 lowercase hex characters
        project_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum BillingAccounts {
    /// The account's billing accounts, each with the projects it pays for
    #[command(after_help = examples(&[
        "hipc account billing-accounts list",
    ]))]
    List,
    /// Create a billing account in the selected account
    ///
    /// A billing account is free, so the create is not confirmed. The answer is the billing
    /// account, with the id --billing takes.
    #[command(after_help = examples(&[
        "hipc account billing-accounts create --name ops",
    ]))]
    Create {
        /// The billing account's name, 1 to 64 characters on one line, none invisible, sent trimmed
        #[arg(long, value_parser = projects::principal_name)]
        name: String,
    },
    /// Rename a billing account
    #[command(after_help = examples(&[
        "hipc account billing-accounts update bill_9f8e7d6c5b4a39281706 --name ops-eu",
    ]))]
    Update {
        /// The billing account id, bill_ and 20 lowercase hex characters, from hipc account
        /// billing-accounts list
        #[arg(value_parser = projects::billing_account_id)]
        billing_account_id: String,
        /// The new name, 1 to 64 characters on one line, none invisible, sent trimmed
        #[arg(long, value_parser = projects::principal_name)]
        name: String,
    },
    /// What a billing account paid, by project, kind, and day
    ///
    /// Amounts are decimal strings in base units, with decimals places (18 for credits). The
    /// server's default range is the current UTC month up to today; a range is at most a
    /// year. The CSV export stays on hipc api.
    #[command(after_help = examples(&[
        "hipc account billing-accounts costs default",
        "hipc account billing-accounts costs default --from 2026-09-01 --to 2026-09-30 \
         --group-by project",
    ]))]
    Costs(Costs),
    /// Select the billing account later commands act on; does not touch the network
    ///
    /// default clears the selection, so commands use the Default billing account. Only the
    /// balance, invoices, billing profile, top-ups, auto-reload, the legacy Stripe row, and
    /// the compute and S3 billing reads take one; the stored one goes on those and is
    /// skipped on every other route, reads and writes alike, so a write there that spends is
    /// paid by the Default billing account. hipc account use clears
    /// it, since a billing account belongs to one account, and under an --account that names
    /// another account it is not read and cannot be stored.
    #[command(after_help = examples(&[
        "hipc account billing-accounts use bill_9f8e7d6c5b4a39281706",
        "hipc account billing-accounts use default",
    ]))]
    Use {
        /// The billing account: default, or an id of bill_ and 20 lowercase hex characters
        billing_account_id: String,
    },
}

/// `hipc account billing-accounts costs`.
#[derive(Debug, Args)]
pub(crate) struct Costs {
    /// The billing account: default, or an id from hipc account billing-accounts list
    #[arg(value_parser = costs_account)]
    pub(crate) billing_account_id: String,
    /// The first day, as YYYY-MM-DD in UTC
    #[arg(long, value_name = "DAY", value_parser = day)]
    pub(crate) from: Option<String>,
    /// The last day, as YYYY-MM-DD in UTC
    #[arg(long, value_name = "DAY", value_parser = day)]
    pub(crate) to: Option<String>,
    /// What the rows are split by; repeat for more
    ///
    /// Without it the server picks.
    #[arg(long, value_enum)]
    pub(crate) group_by: Vec<CostGroup>,
}

/// What a billing account's costs are split by, in the console's order, which is the order
/// they are sent in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub(crate) enum CostGroup {
    Project,
    Kind,
    Day,
}

impl CostGroup {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Kind => "kind",
            Self::Day => "day",
        }
    }
}

/// The billing account of a costs read: `default` or a billing account id, which goes into
/// the path. Checked at parse time, so nothing else can reach the path.
fn costs_account(text: &str) -> Result<String, String> {
    match hippius::BillingId::parse(text, &hippius::Source::Argument) {
        Ok(_) => Ok(text.to_owned()),
        Err(_) => Err(
            "a billing account is `default`, or `bill_` and 20 lowercase hex \
                       characters, from `hipc account billing-accounts list`"
                .to_owned(),
        ),
    }
}

/// A day as `YYYY-MM-DD`, months 01 to 12 and days 01 to 31, the shape the console checks.
/// Whether the day exists in its month, and the range, are the server's to check
/// (`bad_range`).
fn day(text: &str) -> Result<String, String> {
    let valid = match text.as_bytes() {
        [y1, y2, y3, y4, b'-', m1, m2, b'-', d1, d2] => {
            [y1, y2, y3, y4, m1, m2, d1, d2]
                .iter()
                .all(|b| b.is_ascii_digit())
                && matches!((m1, m2), (b'0', b'1'..=b'9') | (b'1', b'0'..=b'2'))
                && matches!(
                    (d1, d2),
                    (b'0', b'1'..=b'9') | (b'1' | b'2', _) | (b'3', b'0' | b'1')
                )
        }
        _ => false,
    };
    if valid {
        Ok(text.to_owned())
    } else {
        Err("a day is YYYY-MM-DD, such as `2026-09-30`".to_owned())
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum Members {
    /// The members, with the account owner beside them (owner)
    #[command(after_help = examples(&[
        "hipc account members list",
    ]))]
    List,
    /// Replace a member's whole grant list with the --grant flags given; needs --yes, and
    /// --remove-others to drop a grant
    ///
    /// Nothing is merged with the current list: a grant the member has now that the --grant
    /// flags leave out is refused, unless --remove-others is passed to remove it. To add one
    /// grant, read the member's grants with hipc account members list, then pass all of them.
    /// Without --grant the grants are kept, and only --manage-team and --billing-permission change.
    ///
    /// The route takes no version or If-Match, so the grants are written whole, after a read
    /// unless --remove-others is passed: a change another admin makes before the write is
    /// lost, and a grant revoked meanwhile is restored if a --grant flag names it.
    #[command(after_help = examples(&[
        "hipc account members update 12 --grant vms=operator --grant s3=viewer --yes",
        "hipc account members update 12 --grant vms=admin --remove-others --yes",
        "hipc account members update 12 --billing-permission true --yes",
    ]))]
    Update(MemberUpdate),
    /// Remove a member from the team; needs --yes
    #[command(after_help = examples(&[
        "hipc account members delete 12 --yes",
    ]))]
    Delete {
        /// The member id from hipc account members list
        #[arg(value_parser = number(MEMBER_ID))]
        member_id: u64,
    },
}

#[derive(Debug, Args)]
pub(crate) struct MemberUpdate {
    /// The member id from hipc account members list
    #[arg(value_parser = number(MEMBER_ID))]
    pub(crate) member_id: u64,
    #[command(flatten)]
    pub(crate) access: Access,
    /// Remove the member's grants the --grant flags leave out
    ///
    /// Without it the member's grants are read first, and the update is refused when the
    /// --grant flags leave out a grant the member has now. A grant is its scope and its level,
    /// so a new level for a scope removes the old one.
    #[arg(long, requires = "grants")]
    pub(crate) remove_others: bool,
}

/// The grants and permissions `members update` and `invites create` both take.
#[derive(Debug, Args)]
pub(crate) struct Access {
    // What each level allows is the docs' team page (https://docs.hippius.com/use/console/team).
    /// A grant, SCOPE=LEVEL, such as vms=operator, or SCOPE=LEVEL@ID for one project or
    /// billing account; repeat for more
    ///
    /// SCOPE is vms, databases, kubernetes, runners, s3, cdn, network, compute:*, storage:*, or
    /// billing; LEVEL is viewer, operator, or admin, and viewer or admin for billing. A grant
    /// without @ID covers the whole account. @ID is a project id (proj_...) for every scope but
    /// network and billing, which is account-wide, and a billing account id (bill_...) for
    /// billing. none alone means no grants.
    ///
    /// Each level holds the ones below it. viewer reads. operator acts on what exists: starts,
    /// stops, and reboots VMs, uploads and deletes S3 objects, and purges the CDN cache. admin
    /// creates, deletes, and resizes: buckets, access keys, firewall rules, public IPv4, zones,
    /// and spend caps.
    #[arg(long = "grant", value_name = "SCOPE=LEVEL", value_parser = grant)]
    pub(crate) grants: Vec<GrantArg>,
    /// The Manage the team permission, true or false
    #[arg(long, value_name = "BOOL")]
    pub(crate) manage_team: Option<bool>,
    /// The Billing permission, true or false
    ///
    /// A member with it can read the balance. The flag is not --billing, which is the global
    /// flag that picks a billing account.
    #[arg(long, value_name = "BOOL")]
    pub(crate) billing_permission: Option<bool>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Invites {
    /// The pending invites the selected account sent
    ///
    /// An invite to the signed-in user is not listed: it is only in the email it came in.
    #[command(after_help = examples(&[
        "hipc account invites list",
    ]))]
    List,
    /// Email an invite to join the team, with the grants and permissions given; needs --yes
    ///
    /// Without --grant the invite grants nothing; without --manage-team or --billing-permission
    /// that permission is not given.
    #[command(after_help = examples(&[
        "hipc account invites create --email ana@example.com --yes",
        "hipc account invites create --email ana@example.com --grant vms=operator --yes",
    ]))]
    Create(InviteCreate),
    /// Revoke a pending invite, so its email link stops working; needs --yes
    #[command(after_help = examples(&[
        "hipc account invites revoke 5 --yes",
    ]))]
    Revoke {
        /// The invite id from hipc account invites list
        #[arg(value_parser = number(INVITE_ID))]
        invite_id: u64,
    },
    /// Show what an emailed invite offers: the account, who sent it, and the access it grants;
    /// needs --yes at another API origin
    ///
    /// The invite token is the last part of the invite link. It is read from a file, or from
    /// stdin with --token-file -, never from argv, and it is never printed. The preview changes
    /// nothing and needs no stored token. Needs --yes when the API origin is not
    /// https://api.hippius.com, because the invite token is sent there.
    #[command(after_help = examples(&[
        "hipc account invites preview --token-file invite.txt",
    ]))]
    Preview {
        /// The file holding the invite token, or - for stdin
        ///
        /// It is read only as a regular file, never through a symlink; /dev/stdin and <(...) are
        /// refused, so pass - for stdin.
        #[arg(long, value_name = "PATH")]
        token_file: PathBuf,
    },
    /// Join the account an emailed invite is for, as the signed-in user; needs --yes
    ///
    /// The invite is previewed first, so the confirmation names the account to join. At an
    /// origin other than https://api.hippius.com it first asks whether the invite token may go
    /// there. The invite token is the last part of the invite link, read from a file or stdin
    /// as for preview, and is never printed. An invite sent to
    /// an address the signed-in user has not verified is refused here; accept it in the
    /// console instead.
    #[command(after_help = examples(&[
        "hipc account invites accept --token-file invite.txt --yes",
    ]))]
    Accept {
        /// The file holding the invite token, or - for stdin
        ///
        /// It is read only as a regular file, never through a symlink; /dev/stdin and <(...) are
        /// refused, so pass - for stdin.
        #[arg(long, value_name = "PATH")]
        token_file: PathBuf,
    },
}

#[derive(Debug, Args)]
pub(crate) struct InviteCreate {
    /// The address the invite is emailed to
    #[arg(long, value_parser = email)]
    pub(crate) email: String,
    #[command(flatten)]
    pub(crate) access: Access,
}

/// The grant scopes: the products, the console's two groups, and the billing scope,
/// as the console's account-sharing types list them.
const GRANT_SCOPES: [&str; 10] = [
    "vms",
    "databases",
    "kubernetes",
    "runners",
    "s3",
    "cdn",
    "network",
    "compute:*",
    "storage:*",
    BILLING_SCOPE,
];

/// The scope of a grant on billing, which may name one billing account.
const BILLING_SCOPE: &str = "billing";

/// The scope whose grant is always account-wide: the network is the account's, and a grant
/// naming a project for it is refused, as the console refuses it.
const ACCOUNT_WIDE_SCOPE: &str = "network";

/// The grant levels, from least to most.
const GRANT_LEVELS: [&str; 3] = ["viewer", "operator", "admin"];

/// The levels of the billing scope, as the console offers them.
const BILLING_LEVELS: [&str; 2] = ["viewer", "admin"];

/// One grant: a scope from [`GRANT_SCOPES`] at a level from [`GRANT_LEVELS`], on what
/// `target` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Grant {
    pub(crate) scope: &'static str,
    pub(crate) level: &'static str,
    pub(crate) target: GrantTarget,
}

/// What a grant covers: the whole account, every project and billing account included,
/// or one project or one billing account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GrantTarget {
    Account,
    Project(hippius::ProjectId),
    Billing(hippius::BillingId),
}

impl GrantTarget {
    /// The body key and the id, as the grant's JSON names them; `None` for the account.
    pub(crate) fn key_and_id(&self) -> Option<(&'static str, &str)> {
        match self {
            Self::Account => None,
            Self::Project(id) => Some(("project", id.as_str())),
            Self::Billing(id) => Some(("billing", id.as_str())),
        }
    }
}

/// One `--grant` value.
#[derive(Debug, Clone)]
pub(crate) enum GrantArg {
    /// none: the grant list is empty
    NoGrants,
    Grant(Grant),
}

/// `SCOPE=LEVEL`, `SCOPE=LEVEL@ID`, or `none`, each from a closed set.
fn grant(value: &str) -> Result<GrantArg, String> {
    if value == "none" {
        return Ok(GrantArg::NoGrants);
    }
    let Some((scope, rest)) = value.split_once('=') else {
        return Err(
            "a grant is `SCOPE=LEVEL`, such as `vms=operator`, `SCOPE=LEVEL@ID`, or `none`"
                .to_owned(),
        );
    };
    let (level, id) = match rest.split_once('@') {
        Some((level, id)) => (level, Some(id)),
        None => (rest, None),
    };
    let scope = one_of(scope, &GRANT_SCOPES, "a grant scope")?;
    let level = if scope == BILLING_SCOPE {
        one_of(level, &BILLING_LEVELS, "a billing level")?
    } else {
        one_of(level, &GRANT_LEVELS, "a grant level")?
    };
    let target = match id {
        None => GrantTarget::Account,
        Some(id) => grant_target(scope, id)?,
    };
    Ok(GrantArg::Grant(Grant {
        scope,
        level,
        target,
    }))
}

/// `text` as the member of `values` it equals, or a refusal that lists them.
fn one_of(text: &str, values: &[&'static str], what: &str) -> Result<&'static str, String> {
    values.iter().copied().find(|v| *v == text).ok_or_else(|| {
        let quoted: Vec<String> = values.iter().map(phrase::quote).collect();
        format!(
            "{} is not {what}: use {}",
            phrase::quote(text),
            phrase::join(&quoted, "or")
        )
    })
}

/// The one project or billing account `id` names for a grant on `scope`: a billing account
/// for the billing scope, none for the network, and a project for every other scope.
fn grant_target(scope: &str, id: &str) -> Result<GrantTarget, String> {
    let source = hippius::Source::Argument;
    if scope == ACCOUNT_WIDE_SCOPE {
        return Err(format!(
            "a grant on `{scope}` is account-wide, so it names no project or billing account"
        ));
    }
    // `default` parses as the Default one, which a grant names by leaving the id out.
    let target = if scope == BILLING_SCOPE {
        hippius::BillingId::parse(id, &source)
            .ok()
            .flatten()
            .map(GrantTarget::Billing)
    } else {
        hippius::ProjectId::parse(id, &source)
            .ok()
            .flatten()
            .map(GrantTarget::Project)
    };
    target.ok_or_else(|| {
        let (noun, prefix, list) = if scope == BILLING_SCOPE {
            ("billing account", "bill_", "billing-accounts")
        } else {
            ("project", "proj_", "projects")
        };
        format!(
            "{} is not a {noun} id: a grant on `{scope}` names a {noun}, `{prefix}` and 20 \
             lowercase hex characters, from `hipc account {list} list`",
            phrase::quote(id)
        )
    })
}

/// An email address for an invite: it goes into the body, so only a plausible address is
/// sent. One `@` with text on both sides, a `.` in the domain, no space or control
/// character, and at most 254 bytes (RFC 5321's path limit in octets, less the brackets). The
/// server's check is the real one.
fn email(value: &str) -> Result<String, String> {
    let valid = value.len() <= 254
        && !value.chars().any(|c| c.is_whitespace() || c.is_control())
        && value.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && !domain.contains('@')
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        });
    if valid {
        Ok(value.to_owned())
    } else {
        Err("an email address is `name@domain`, such as `ana@example.com`".to_owned())
    }
}
