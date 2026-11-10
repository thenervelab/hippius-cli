//! The command-line grammar (`TENETS.md` tenet 2).
//!
//! The doc comments on the commands and flags are their help text, which a person reads in a
//! terminal: plain text, not Markdown (tenet 15), so no backticks or link brackets.
#![expect(
    clippy::doc_markdown,
    reason = "the doc comments are help text printed as plain text, where Markdown would show"
)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};

mod account;
mod api;
mod app_engine;
mod auth;
mod backups;
mod billing;
mod cdn;
mod compute;
mod databases;
mod drive;
mod exposures;
mod firewall;
mod hub;
mod hub_keys;
mod hub_models;
mod kubernetes;
mod network;
mod projects;
mod runners;
mod s3;
mod support;
mod waf;

pub(crate) use crate::cli::app_engine::{AppCreate, AppEngine, AppEngineApps, GuestComponents};
pub(crate) use crate::cli::hub_keys::{HubKeys, KeyCreate};
pub(crate) use crate::cli::hub_models::{HubEvents, HubModels};
pub(crate) use crate::cli::kubernetes::{
    ClusterKubeconfig, Clusters, Kubernetes, KubernetesBackups, NodePoolCreate, NodePoolScale,
    NodePools, NodeReplace, Nodes, OperationTarget, Operations,
};
pub(crate) use crate::cli::network::{
    Groups, Network, Peers, Policies, VpcResources, VpcRouters, Vpcs,
};
pub(crate) use crate::cli::projects::{
    Budget, BudgetUpdate, ProjectCreate, ProjectTransfer, ProjectTransfers, ProjectUpdate, Quota,
    QuotaLimit, QuotaUpdate,
};
pub(crate) use waf::{
    CdnLists, CdnWaf, GroupBy, ListItems, RuleAction, TierZones, WafAnalytics, WafEvents, WafMode,
    WafRateLimits, WafRules, WafTest, WafUpdate, WafVersions,
};

pub(crate) use crate::cli::account::{
    Access, Account, BillingAccounts, Costs, Grant, GrantArg, InviteCreate, Invites,
    MarketingConsent, MemberUpdate, Members, Notifications, Projects, Referrals,
};
pub(crate) use crate::cli::api::Api;
pub(crate) use crate::cli::auth::Auth;
pub(crate) use crate::cli::backups::{
    BackupInterval, BackupPolicy, BackupPolicyUpdate, BackupRestore, Backups, FailoverMode,
};
pub(crate) use crate::cli::billing::{
    AutoReload, AutoReloadUpdate, Billing, BillingProfile, CHAIN_MINIMUM, Gamma, GammaConversions,
    GammaHotkeys, Invoices, ONE, Planck, ProfileUpdate, ReloadCap, TopUp, TopUpMethod, Usd,
    conversion_id, usd,
};
pub(crate) use crate::cli::cdn::{
    Cdn, CdnCap, CdnEstimate, CdnHostnames, CdnPurge, CdnPurges, CdnRules, CdnStats, CdnZones,
    DnsSetup, StatsStep, ZoneCreate, ZoneSettings, ZoneUpdate,
};
pub(crate) use crate::cli::compute::{
    Compute, DedicatedDisk, DiskTier, GuestPoweroff, PowerPolicy, PublicIp, SshKeys, VmCreate, Vms,
    flavor, placement_group,
};
pub(crate) use crate::cli::databases::{
    CREDENTIALS_JSON, DatabaseCreate, DatabaseExposure, DatabaseRestore, Databases, ENV_FILE,
    TLS_FILES,
};
pub(crate) use crate::cli::drive::{
    Drive, DriveChange, DriveSubscribe, DriveSubscription, PayFrom,
};
pub(crate) use crate::cli::exposures::{ExposeMode, ExposeProtocol, ExposureCreate, Exposures};
pub(crate) use crate::cli::firewall::{Firewall, FirewallUpdate};
pub(crate) use crate::cli::hub::{
    Hub, HubArtifacts, HubNamespace, HubRepositories, HubRobot, HubSubscribe, HubSubscription,
    Visibility,
};
pub(crate) use crate::cli::runners::{
    MonthlyCap, RunnerInstallations, RunnerLimit, RunnerSettings, RunnerSettingsUpdate, RunnerSize,
    Runners,
};
pub(crate) use crate::cli::s3::{
    AclUpdate, BucketAcl, BucketObjects, Buckets, Configure, Funding, MAX_CONTINUATION_TOKEN_BYTES,
    ObjectsList, PlanSetting, Presign, S3, S3Subscription, StorageCap, TokenCreate, TokenKind,
    TokenRotate, TokenTarget, Tokens, continuation_token,
};
pub(crate) use crate::cli::support::{
    Support, TicketBody, TicketCreate, TicketList, TicketMessages, TicketUpdateStatus, Tickets,
};

/// `hipc`: the Hippius console control plane.
#[derive(Debug, Parser)]
#[command(
    name = "hipc",
    version,
    about = "The Hippius console control plane, for agents and people",
    after_help = top_level_after_help(),
    disable_help_subcommand = true
)]
pub(crate) struct Cli {
    #[command(flatten)]
    pub(crate) global: Global,
    #[command(subcommand)]
    pub(crate) command: Command,
}

/// Flags accepted anywhere after `hipc`.
///
/// They sit under their own heading in every `--help`, after the command's own options, so a
/// command's required flags are not lost among the ones every command takes.
#[derive(Debug, Args)]
#[command(next_help_heading = "Global options")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is one independent command-line flag, which clap parses into a field"
)]
pub(crate) struct Global {
    /// The output format: auto is JSON when stdout is not a terminal, CI is set, or AI_AGENT
    /// is set and not off; --output text overrides the detection
    #[arg(long, global = true, value_enum, default_value_t = OutputArg::Auto)]
    pub(crate) output: OutputArg,
    /// Same as --output json
    ///
    /// Passing both is a usage error, as a repeated --output is.
    #[arg(long, global = true, conflicts_with = "output")]
    pub(crate) json: bool,
    /// The profile in the config file; overrides HIPPIUS_PROFILE
    #[arg(long, global = true, value_name = "NAME")]
    pub(crate) profile: Option<String>,
    /// The API origin, such as https://api.hippius.com; overrides HIPPIUS_API_URL
    ///
    /// HIPPIUS_TOKEN goes only to https://api.hippius.com, or to the origin
    /// HIPPIUS_TOKEN_ORIGIN names; another origin exits 1 auth before sending.
    #[arg(long, global = true, value_name = "URL")]
    pub(crate) api_url: Option<String>,
    /// The account to act in: self, or an id from hipc account list; overrides HIPPIUS_ACCOUNT
    #[arg(long, global = true, value_name = "ID")]
    pub(crate) account: Option<String>,
    /// The project to act in: default, or an id from hipc account projects list; overrides
    /// HIPPIUS_PROJECT
    ///
    /// Only compute, databases, Kubernetes, runners, S3 buckets and sub tokens, CDN, and the S3
    /// plan reads take a project; a master S3 token does not. A personal route goes out
    /// without it, and any other request exits 2 until --project default is passed. A project
    /// stored with hipc account projects use is skipped on reads and refused on writes of the
    /// routes that do not take it, apart from the writes to a route that names a project or a
    /// billing account in its path, which skip it too.
    #[arg(long, global = true, value_name = "ID")]
    pub(crate) project: Option<String>,
    /// The billing account to act on: default, or an id from hipc account billing-accounts
    /// list; overrides HIPPIUS_BILLING
    ///
    /// Only the balance, invoices, billing profile, top-ups, auto-reload, the legacy Stripe
    /// row, and the compute and S3 billing reads take one; a personal route goes out without it,
    /// and any other request exits 2 until --billing default is passed. A billing account
    /// stored with hipc account billing-accounts use is skipped on every route that does not
    /// take it, reads and writes alike, so a write there that spends is paid by the Default
    /// billing account.
    #[arg(long, global = true, value_name = "ID")]
    pub(crate) billing: Option<String>,
    /// Confirm a command whose help says it needs --yes
    #[arg(long, global = true)]
    pub(crate) yes: bool,
    /// Never prompt
    ///
    /// A command that needs confirmation and lacks --yes exits 2.
    #[arg(long, global = true)]
    pub(crate) no_input: bool,
    /// Print the requests a real run would send, and send nothing
    ///
    /// It exits 0, or 2 when an argument is wrong, when a local file the command reads first is
    /// missing or open to the group or others, when the route is one hipc api never sends, or
    /// when the account, project, or billing account is refused: a route that does not take the
    /// one selected, or a membership with no billing account on a route that needs one.
    #[arg(long, global = true)]
    pub(crate) dry_run: bool,
    /// Print each request's method, URL (query values redacted), and status on stderr; never a
    /// header or a body
    #[arg(long, global = true)]
    pub(crate) debug: bool,
    /// Drop the text-mode notes and warnings on stderr (the note: and warning: lines), such as
    /// "nothing has been bought yet"; never drops an error: or hint: line
    #[arg(long, global = true)]
    pub(crate) quiet: bool,
    /// Seconds before a request is abandoned
    #[arg(
        long,
        global = true,
        value_name = "SECONDS",
        default_value_t = 30,
        value_parser = clap::value_parser!(u64).range(1..=3600)
    )]
    pub(crate) request_timeout: u64,
}

/// The line every page of help but the root's ends with, in place of the global options:
/// the root's help lists them, and so does `hipc schema`. Repeated on every page, about 60
/// lines of the same flags filled most of a short command's help and an agent's context.
pub(crate) const GLOBAL_OPTIONS_POINTER: &str = "Global options: see hipc --help";

/// The command `hipc` parses argv with: [`Cli`], built, so every subcommand holds the global
/// options, with those options hidden from every page of help but the root's, and
/// [`GLOBAL_OPTIONS_POINTER`] at the end of each such page. Hidden flags still parse, so a
/// global option works at any depth as before. The completion script and `hipc schema` are
/// built from [`Cli`] itself, which shows every flag.
pub(crate) fn parse_command() -> clap::Command {
    use clap::CommandFactory as _;

    let mut root = Cli::command();
    root.build();
    let names: Vec<String> = root
        .get_subcommands()
        .map(|sub| sub.get_name().to_owned())
        .collect();
    for name in names {
        root = root.mut_subcommand(name, without_global_help);
    }
    root
}

/// `command` and every subcommand under it with the global options hidden and the help
/// ending in [`GLOBAL_OPTIONS_POINTER`] ([`parse_command`]).
fn without_global_help(command: clap::Command) -> clap::Command {
    // Every page has examples in its after-help (a unit test holds that), so the pointer
    // follows them after a blank line.
    let examples = command
        .get_after_help()
        .map(ToString::to_string)
        .unwrap_or_default();
    let after = format!("{}\n\n{GLOBAL_OPTIONS_POINTER}", examples.trim_end());
    // `mut_args` keeps each flag in its place. The command is built, and its key index points
    // at the flags by position, so `mut_arg`, which moves the flag to the end, would make
    // `--output` read another flag's value.
    let mut command = command.after_help(after).mut_args(|arg| {
        if arg.is_global_set() {
            arg.hide(true)
        } else {
            arg
        }
    });
    let names: Vec<String> = command
        .get_subcommands()
        .map(|sub| sub.get_name().to_owned())
        .collect();
    for name in names {
        command = command.mut_subcommand(name, without_global_help);
    }
    command
}

/// The global options that take one value. `--json` counts as `--output`.
const GLOBAL_VALUE_OPTIONS: [&str; 7] = [
    "output",
    "profile",
    "api-url",
    "account",
    "project",
    "billing",
    "request-timeout",
];

/// The first global option given more than once, or `--json` with `--output`, anywhere up
/// to a `--` (tenet 3). clap refuses a repeat at one command level, but a global given at
/// two levels (`hipc --json version --output text`) is merged with the last one winning, so
/// this scan of raw argv catches what clap lets through. Like clap, it does not take a word
/// that starts with `-` as a value.
pub(crate) fn repeated_global(args: &[OsString]) -> Option<&'static str> {
    let mut seen = [0_u8; GLOBAL_VALUE_OPTIONS.len()];
    let mut words = args.iter().skip(1).map(|a| a.to_str()).peekable();
    while let Some(word) = words.next() {
        let Some(word) = word else { continue };
        if word == "--" {
            break;
        }
        let Some(flag) = word.strip_prefix("--") else {
            continue;
        };
        let (name, inline_value) = match flag.split_once('=') {
            Some((name, _)) => (name, true),
            None => (flag, false),
        };
        let name = if name == "json" { "output" } else { name };
        let Some(index) = GLOBAL_VALUE_OPTIONS.iter().position(|o| *o == name) else {
            continue;
        };
        seen[index] += 1;
        if seen[index] > 1 {
            return Some(GLOBAL_VALUE_OPTIONS[index]);
        }
        let takes_next = !inline_value && flag != "json";
        if takes_next
            && words
                .peek()
                .is_some_and(|next| next.is_none_or(|n| !n.starts_with('-')))
        {
            words.next();
        }
    }
    None
}

/// The values of `--output`.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum OutputArg {
    Auto,
    Text,
    Json,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Print the version; does not touch the network
    #[command(after_help = examples(&[
        "hipc version",
    ]))]
    Version,
    /// The state of each product for the signed-in user: Hub, Drive, S3, and compute
    ///
    /// GET /api/services/status/, the read behind the console's banners, printed as sent. It
    /// needs a token: the API serves it to the signed-in user. A next_charge_unix_day counts
    /// days since 1970-01-01 UTC; hipc drive subscription get adds the date beside it.
    #[command(after_help = examples(&[
        "hipc status",
    ]))]
    Status,
    /// Print every command with its effect, or only the commands under a path, as JSON in
    /// every output mode; does not touch the network
    ///
    /// With a path, such as compute app-engine, commands lists only the commands under it, and a
    /// whole command path, such as s3 buckets create, lists that command alone. schema_version,
    /// global_flags, and errors are printed as without a path: the path filters commands and
    /// changes nothing else. A name that is not a command at its place is a usage error, exit 2,
    /// that lists the commands there, and whose hint runs the nearest one.
    #[command(after_help = examples(&[
        "hipc schema",
        "hipc schema compute app-engine",
        "hipc schema s3 buckets create",
    ]))]
    Schema {
        /// The command path to list, one word per argument; without it, every command
        #[arg(value_name = "PATH")]
        path: Vec<String>,
    },
    /// Print a shell completion script
    #[command(after_help = examples(&[
        "hipc completions zsh",
        "hipc completions bash",
    ]))]
    Completions {
        /// The shell to complete for
        shell: clap_complete::Shell,
    },
    /// Store, check, or remove the console API token
    #[command(subcommand, after_help = examples(AUTH_EXAMPLES))]
    Auth(Auth),
    /// The accounts the token can act in and the one selected, its team, projects, project
    /// transfers, and billing accounts, and the signed-in user's email settings and referral links
    #[command(subcommand, after_help = examples(ACCOUNT_EXAMPLES))]
    Account(Account),
    /// The balance, compute burn rate and usage, every plan in one view, transactions, and
    /// invoices
    #[command(subcommand, after_help = examples(BILLING_EXAMPLES))]
    Billing(Billing),
    /// Drive plans and the Drive subscription
    ///
    /// Drive files stay in the Drive apps and SDK.
    #[command(subcommand, after_help = examples(DRIVE_EXAMPLES))]
    Drive(Drive),
    /// S3 plans, buckets, and tokens, and an AWS CLI profile for s3.hippius.com
    ///
    /// Object bytes go through the AWS CLI, boto3, or rclone.
    #[command(subcommand, after_help = examples(S3_EXAMPLES))]
    S3(S3),
    /// Hub plans, the Hub subscription, the registry namespace, its usage, its repositories and
    /// artifacts, and its CI keys (hipc hub keys)
    ///
    /// Model and image uploads stay in hippius-hub. Two things to know about it: hippius-hub
    /// ignores HIPPIUS_TOKEN for registry calls, and its login --hippius-token does not allow a
    /// blob upload until hippius-hub registry rotate-token has stored the docker secret.
    ///
    /// For CI, create a scoped key (hipc hub keys create --role push) and log docker in with
    /// its login and its secret on stdin, as the answer's docker_login_command does. hippius-hub
    /// registry rotate-token, like hipc hub namespace robot rotate, replaces the namespace's
    /// docker secret, which every other login uses.
    #[command(subcommand, after_help = examples(HUB_EXAMPLES))]
    Hub(Hub),
    /// Virtual machines with their catalog, quota, public IPv4, firewall, exposures, and backups,
    /// App Engine apps, managed databases and Kubernetes, GitHub Actions runners, and SSH keys
    ///
    /// hipc does not open SSH; use ssh for that. A launch attaches a public IPv4 only with
    /// --public-ip; hipc compute public-ip create attaches one to a VM that exists. A project's
    /// monthly budget, hipc account projects budget update, mails alerts and, with
    /// --block-launches true, refuses new launches that add cost at its limit; it does not stop
    /// a VM that runs.
    #[command(subcommand, after_help = examples(COMPUTE_EXAMPLES))]
    Compute(Compute),
    /// CDN zones that serve an S3 bucket, with their spend cap, custom hostnames, cache purges
    /// and rules, traffic, and WAF, plus the WAF lists, the regions, the prices, and an estimate
    ///
    /// CDN is billed by usage from the balance, plus a monthly fee per custom hostname, and the
    /// WAF add-on by the hour while its mode is not off; hipc cdn prices is the price list. There
    /// is no CDN plan.
    #[command(subcommand, after_help = examples(CDN_EXAMPLES))]
    Cdn(Cdn),
    /// Support tickets: open one, list them, read one, reply, read its messages, and close it
    ///
    /// A ticket is read by a person on the Hippius support team. Attachments and the handoff
    /// from the chat stay on hipc api.
    #[command(subcommand, after_help = examples(SUPPORT_EXAMPLES))]
    Support(Support),
    /// The account's private network: its peers, groups, access policies, and VPC networks
    ///
    /// An account's network is created by hipc network create, or by the first hipc network
    /// get; until then the other network commands answer that it is not set up. Enrolling a
    /// device, its enroll keys, and the isolation mode stay on hipc api.
    #[command(subcommand, after_help = examples(NETWORK_EXAMPLES))]
    Network(Network),
    // The help is built from the library's provisioning table ([`api_about`]), as the --reveal
    // help is from the secret table, so a doc comment here would go stale.
    #[command(
        about = api_about(),
        long_about = api_long_about(),
        after_help = examples(API_EXAMPLES)
    )]
    Api(Api),
}

/// The examples of the top level and of each command group, the most common call first
/// (clig.dev "Lead with examples"). A test parses every one, so none goes stale.
const TOP_EXAMPLES: &[&str] = &[
    "hipc auth login --token-file token.txt",
    "hipc account list",
    "hipc compute vms list",
    "hipc --dry-run s3 buckets create photos",
    "hipc api GET /accounts/",
];
const AUTH_EXAMPLES: &[&str] = &[
    "hipc auth login --token-file token.txt",
    "hipc auth login --access-key-file access-key.txt",
    "hipc auth status",
    "hipc auth logout --yes",
];
const ACCOUNT_EXAMPLES: &[&str] = &[
    "hipc account list",
    "hipc account use acct_0123456789abcdef0123",
    "hipc account get",
    "hipc account members list",
];
const BILLING_EXAMPLES: &[&str] = &[
    "hipc billing balance",
    "hipc billing summary",
    "hipc billing subscriptions",
    "hipc billing usage --month 2026-09",
    "hipc billing invoices get 42 --output-file invoice-42.pdf",
    "hipc billing native-quote",
];
const DRIVE_EXAMPLES: &[&str] = &[
    "hipc drive plans",
    "hipc drive subscription get",
    "hipc drive subscription subscribe --plan solo --yes",
];
const S3_EXAMPLES: &[&str] = &[
    "hipc s3 buckets list",
    "hipc s3 buckets create photos --yes",
    "hipc s3 tokens create --kind master --reveal --yes",
    "hipc s3 configure --access-key-id hip_A1B2 --secret-key-file secret.txt",
];
const HUB_EXAMPLES: &[&str] = &[
    "hipc hub plans",
    "hipc hub subscription subscribe --plan Pro --yes",
    "hipc hub namespace create my-team --reveal --yes",
];
const COMPUTE_EXAMPLES: &[&str] = &[
    "hipc compute flavors",
    "hipc compute ssh-keys list",
    "hipc compute vms create --flavor small --ssh-key-id 1 --yes",
    "hipc compute vms list",
];
const CDN_EXAMPLES: &[&str] = &[
    "hipc cdn zones list",
    "hipc cdn zones create --name site --bucket photos --public --yes",
    "hipc cdn purges create zab12cd34ef56 --everything",
];
const SUPPORT_EXAMPLES: &[&str] = &[
    "hipc support tickets list --status open",
    "hipc support tickets create --subject Refund --body-file message.txt --yes",
    "hipc support tickets get 42",
    "hipc support tickets reply 42 --body-file message.txt",
    "hipc support tickets messages list 42",
    "hipc support tickets update 42 --status closed --yes",
];
const NETWORK_EXAMPLES: &[&str] = &[
    "hipc network create --yes",
    "hipc network peers list",
    "hipc network get --yes",
    "hipc network peers delete 12 --yes",
];
const API_EXAMPLES: &[&str] = &[
    "hipc api GET /accounts/",
    "hipc api GET /billing/transactions/ --all",
    "hipc api POST /objectstore/buckets/ --body-file bucket.json --yes",
    "hipc api DELETE /cdn/lists/blocked/items/ --body-file items.json --yes",
];

/// An `Examples:` section of help, one command per line.
fn examples(lines: &[&str]) -> String {
    let mut text = "Examples:\n".to_owned();
    for line in lines {
        text.push_str(&format!("  {line}\n"));
    }
    text
}

/// The end of the top-level help: the examples, the exit codes (tenet 4), how an agent or
/// CI is detected and where it starts (tenets 3, 5, and 15), where a flag goes, said once
/// here rather than on each flag that looks global, and where the docs and the issue
/// tracker are.
fn top_level_after_help() -> String {
    format!(
        "{}
Exit codes:
  0  the command did what it said, --dry-run included
  1  the API or the runtime failed
  2  the invocation must change: a flag, a variable, or a local file

Agents and CI: when CI or AI_AGENT is set, hipc treats the caller as a program. --output auto
prints JSON, and a command that needs confirmation exits 2 with code confirmation_required
instead of asking, having changed nothing (a command whose question names its target may
read first); pass --yes only for the write you meant. hipc network get is one such command:
the first call for an account may create its private network. A flag always wins over the
detection: --output text prints text. AI_AGENT that is empty, or 0, false, no, or off in any
case and around spaces, counts as unset.
stdout is also JSON when it is not a terminal, or with --json, and hipc schema lists every
command, its effect, and every error code with its exit code.

Flags: the global options go anywhere after hipc. Every other flag, --reveal, --public, and
--replace included, goes after the command it belongs to.

Docs: https://docs.hippius.com
Issues: {}",
        examples(TOP_EXAMPLES),
        hippius::ISSUES_URL
    )
}

/// An artifact reference, a tag or a digest (swagger, `registry_repositories_artifacts_read`),
/// in the grammar of the OCI specs (`spec.md` "Pulling manifests" of
/// `opencontainers/distribution-spec` v1.1.0 at `0f98d91`, and `descriptor.md` "Digests" of
/// `opencontainers/image-spec` v1.1.0 at `e7f7c0c`). A tag is 1 to 128 of letters, digits,
/// `_`, `.`, and `-`, and does not start with `.` or `-`. A digest is `<algorithm>:<encoded>`,
/// with lowercase algorithm parts joined by `+`, `.`, `_`, or `-`, and an encoded part of
/// letters, digits, `=`, `_`, and `-`. Neither holds a `/`, so a reference is one path
/// segment.
fn reference(value: &str) -> Result<String, String> {
    let tag = |text: &str| {
        (1..=128).contains(&text.len())
            && !text.starts_with(['.', '-'])
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    };
    let algorithm = |text: &str| {
        text.split(['+', '.', '_', '-']).all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
    };
    let encoded = |text: &str| {
        !text.is_empty()
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"=_-".contains(&b))
    };
    let valid = match value.split_once(':') {
        Some((head, tail)) => algorithm(head) && encoded(tail),
        None => tag(value),
    };
    if valid {
        Ok(value.to_owned())
    } else {
        Err(
            "an artifact reference is a tag, such as `v1`, or a digest, such as `sha256:0a1b2c`, \
             as `hipc hub artifacts list` prints them"
                .to_owned(),
        )
    }
}

/// A `vm_id`: it goes into the URL path, so only the characters an id has are accepted.
/// The swagger types it as a string; 1 to 64 letters, digits, `-`, and `_` is a guess
/// wide enough for the UUIDs and slugs seen, and narrow enough that no `/` or `?` can
/// change the route.
pub(crate) fn vm_id(value: &str) -> Result<String, String> {
    slug_id(
        value,
        64,
        "a VM id is 1 to 64 letters, digits, `-`, and `_`",
    )
}

// The refusal of each numeric id: what it is, and which command prints it.
const SUBSCRIPTION_ID: &str =
    "a subscription id is a whole number, from `hipc hub subscription get`";
const RESIZE_ID: &str =
    "a resize id is a whole number, from the answer of `hipc compute vms resize`";
const SSH_KEY_ID: &str = "an SSH key id is a whole number, from `hipc compute ssh-keys list`";
const EXPOSURE_ID: &str = "an exposure id is a whole number, from `hipc compute exposures list`";
const DATABASE_ID: &str = "a database id is a whole number, from `hipc compute databases list`";
const INSTALLATION_ID: &str =
    "an installation id is a whole number, from `hipc compute runners installations list`";
const PURGE_ID: &str = "a purge id is a whole number, from the answer of `hipc cdn purges create`";
const INVOICE_ID: &str = "an invoice id is a whole number, from `hipc billing invoices list`";
const MEMBER_ID: &str = "a member id is a whole number, from `hipc account members list`";
const INVITE_ID: &str = "an invite id is a whole number, from `hipc account invites list`";
const TICKET_ID: &str = "a ticket id is a whole number, from `hipc support tickets list`";
const REFERRAL_ID: &str =
    "a referral link id is a whole number, from `hipc account referrals list`";

/// A parser for a numeric id, which goes into the URL path or the body as a number.
/// `refusal` says what the id is and which command prints it, in place of Rust's "invalid
/// digit found in string". The digits are read as `u64` parses them.
fn number(refusal: &'static str) -> impl Fn(&str) -> Result<u64, String> + Clone {
    move |value| value.parse().map_err(|_| refusal.to_owned())
}

/// The one rule of every slug id parser: `value` when it is 1 to `max` ASCII letters,
/// digits, `-`, and `_`, else `refusal`, which names the id and states that rule. No `/`,
/// `?`, `.`, or space gets past it, so an id cannot change a URL path it goes into.
fn slug_id(value: &str, max: usize, refusal: &str) -> Result<String, String> {
    let valid = (1..=max).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if valid {
        Ok(value.to_owned())
    } else {
        Err(refusal.to_owned())
    }
}

/// A compute region: two ASCII letters (ISO 3166-1 alpha-2), as swagger documents every
/// compute `region` (the launch, feasibility, and public IP availability) and as runners
/// store it. Sent in uppercase, the ISO spelling and the one `hipc compute regions` lists:
/// the availability query takes either case, and the other routes are not known to. Every
/// command that takes a country code parses it here, so
/// each checks and spells it the same way. The CDN and S3 regions are other
/// vocabularies, named by their own lists.
pub(crate) fn region(value: &str) -> Result<String, String> {
    country_code(value).ok_or_else(|| "a region is a two-letter country code, such as `FR`".into())
}

/// `value` as an uppercase ISO 3166-1 alpha-2 code, or `None` when it is not two ASCII
/// letters.
fn country_code(value: &str) -> Option<String> {
    (value.len() == 2 && value.bytes().all(|b| b.is_ascii_alphabetic()))
        .then(|| value.to_ascii_uppercase())
}

/// A zone id: `z` and 12 characters. It goes into the URL path. Any ASCII letter or digit
/// is accepted for the 12, because the API does not document the alphabet or its case;
/// the check exists so that no `/`, `?`, or `..` can change the route.
fn zone_id(value: &str) -> Result<String, String> {
    let valid = value.len() == 13
        && value.starts_with('z')
        && value.bytes().all(|b| b.is_ascii_alphanumeric());
    if valid {
        Ok(value.to_owned())
    } else {
        Err("a zone id is `z` and 12 letters or digits, such as `zab12cd34ef56`".to_owned())
    }
}

/// A credit amount: a decimal number such as `20` or `20.5`, sent as the string the caller
/// wrote.
fn credits(value: &str) -> Result<String, String> {
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    let valid = match value.split_once('.') {
        Some((whole, fraction)) => digits(whole) && digits(fraction),
        None => digits(value),
    };
    if valid {
        Ok(value.to_owned())
    } else {
        Err("a credit amount is a decimal number, such as `20` or `20.5`".to_owned())
    }
}

/// A credit amount [`credits`] accepted, in its normal form: no leading zeros before the
/// point and no trailing zeros after it, so the question and the body show `50` for
/// `050.00`, and the decimals counted against a credit's 18 are the ones that hold value.
fn normal_credits(credits: &str) -> String {
    let (whole, fraction) = credits.split_once('.').unwrap_or((credits, ""));
    let whole = match whole.trim_start_matches('0') {
        "" => "0",
        digits => digits,
    };
    match fraction.trim_end_matches('0') {
        "" => whole.to_owned(),
        fraction => format!("{whole}.{fraction}"),
    }
}

/// `--limit`, `--page-token`, and `--all` on a paginated list (tenet 6).
#[derive(Debug, Args)]
pub(crate) struct ListArgs {
    /// Items per page, 1 to 500
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=500))]
    pub(crate) limit: u32,
    /// The page to fetch, from a previous answer's next_page_token
    #[arg(long, value_name = "TOKEN", conflicts_with = "all", value_parser = page_token)]
    pub(crate) page_token: Option<String>,
    /// Follow every page: one JSON array, or one table in text mode
    #[arg(long)]
    pub(crate) all: bool,
}

/// `--limit` of a plan history: swagger caps its `page_size` at 200 ("default 50, max
/// 200"), below the 500 of [`ListArgs`], so a larger page is refused, not cut short.
fn history_limit(limit: clap::Arg) -> clap::Arg {
    limit
        .value_parser(clap::value_parser!(u32).range(1..=200))
        .help("Items per page, 1 to 200")
}

/// `--page-token` and `--all` on a list the server pages by cursor (tenet 6). The server's
/// page size is fixed, so there is no `--limit`, and passing one is a usage error.
#[derive(Debug, Args)]
pub(crate) struct CursorArgs {
    /// The page to fetch, from a previous answer's next_page_token
    #[arg(long, value_name = "TOKEN", conflicts_with = "all", value_parser = cursor_token)]
    pub(crate) page_token: Option<String>,
    /// Follow every page: one JSON array, or one table in text mode
    #[arg(long)]
    pub(crate) all: bool,
}

/// `value` when a printed `hipc` command can take it as written: `parse` accepts it, and clap
/// cannot read it as a flag. A hint or `next_step` built from server text names a value only
/// through this, because an agent runs those commands as written (tenet 15) and a `;`, a
/// `$(`, or a line break in the value would become part of the command.
pub(crate) fn as_argument(value: &str, parse: fn(&str) -> Result<String, String>) -> Option<&str> {
    (parse(value).is_ok() && !value.starts_with('-')).then_some(value)
}

/// The printed command `hipc <words> <id> <flags>` for an `id` its argument rule accepts: an
/// id clap would read as a flag, such as one with a leading `-`, goes last, after `--`, so
/// the command still runs as written (tenet 15).
pub(crate) fn with_id(words: &str, id: &str, flags: &str) -> String {
    let flags = if flags.is_empty() {
        String::new()
    } else {
        format!(" {flags}")
    };
    if id.starts_with('-') {
        format!("hipc {words}{flags} -- {id}")
    } else {
        format!("hipc {words} {id}{flags}")
    }
}

/// A checkout `intent_id`: it goes into the URL path, so only the characters an id has are
/// accepted. The swagger types it as a string, not a UUID, and card checkouts are Stripe
/// payment intents (`pi_...`); 1 to 128 letters, digits, `-`, and `_` fits those and keeps
/// `/` and `?` out of the route.
pub(crate) fn intent_id(value: &str) -> Result<String, String> {
    slug_id(
        value,
        128,
        "an intent id is 1 to 128 letters, digits, `-`, and `_`",
    )
}

/// A page token is the decimal page number `next_page_token` gave. Checked at parse time,
/// so a bad token is a usage error before anything else runs.
fn page_token(text: &str) -> Result<String, String> {
    // As a number, so `01` is page 1, as the server's links name it (json.rs `page_number`).
    // Pages count from 1, so `0` is no token any answer gave.
    decimal(text).filter(|page| page != "0").ok_or_else(|| {
        "use a `next_page_token` from an earlier answer: a page number from 1".to_owned()
    })
}

/// A cursor token is the decimal cursor `next_page_token` gave. Checked at parse time, as
/// a page token is, so a token that is not a decimal cursor is a usage error and never
/// reaches the server, whose answer to an unreadable cursor is not documented.
fn cursor_token(text: &str) -> Result<String, String> {
    // As a number, so `0812` and `812` are one cursor to the loop check, as json.rs
    // `cursor_page` writes it.
    decimal(text)
        .ok_or_else(|| "use a `next_page_token` from an earlier answer: a cursor".to_owned())
}

/// `text` as a `u64` in plain digits, written back without leading zeros.
fn decimal(text: &str) -> Option<String> {
    if !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<u64>().ok().map(|number| number.to_string())
}

/// The file `what` goes to, such as an invoice PDF or a kubeconfig: a path, not `-`, at which
/// nothing exists yet, not even a symlink, in a directory that exists and that no other user
/// can write (`commands::files::private_dir`). Checked at parse time,
/// so a usage error comes before anything is sent. The file is still created exclusively
/// (`create_new`), so one made after this check is not overwritten either, and a directory
/// that cannot be written is found only then.
fn new_file(text: &str, what: &str) -> Result<PathBuf, String> {
    if text == "-" {
        return Err(format!(
            "{what} is written only to a file, never to stdout: name a path"
        ));
    }
    // A path that ends in `/` or `/.` names a directory, and the system resolves it through a
    // symlink at its last name: `link/`, for a link to a file that does not exist, would pass
    // the checks below and be created at the link's target.
    if text.ends_with('/') || text.ends_with("/.") || Path::new(text).file_name().is_none() {
        return Err("that path names a directory, not a file: name the file to create".to_owned());
    }
    let path = PathBuf::from(text);
    if std::fs::symlink_metadata(&path).is_ok() {
        return Err(
            "something is already at that path, and hipc never overwrites it: name a new \
             path, or move it away"
                .to_owned(),
        );
    }
    // A bare file name has the empty path as its parent, which is the current directory.
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    match std::fs::metadata(parent) {
        Ok(metadata) if metadata.is_dir() => {
            crate::commands::files::private_dir_refusal(parent, &metadata)?;
        }
        _ => {
            return Err(
                "the directory of that path does not exist: name a file in one that does"
                    .to_owned(),
            );
        }
    }
    Ok(path)
}

/// The file `--secret-file` writes a one-time secret to ([`new_file`]), on the commands that
/// issue one: S3 token and Hub key create and rotate, the Hub namespace create, and the Hub
/// robot rotate.
fn secret_file(text: &str) -> Result<PathBuf, String> {
    new_file(text, "the secret")
}

/// `--secret-file`, on every command that issues a one-time secret but a database's, whose
/// several secrets go to `--credentials-dir`.
#[derive(Debug, Args)]
pub(crate) struct SecretFile {
    /// Write the one-time secret to a new file of mode 0600 at this path instead of stdout; in
    /// place of --reveal
    ///
    /// Nothing may be at the path yet, not even a symbolic link, and its directory must exist
    /// and must not be writable by other users; it is checked before anything is sent. The file
    /// holds the secret alone, with no newline, and the document names the file in its place:
    /// secret becomes secret_file, and robot_secret becomes robot_secret_file. If the answer
    /// holds no secret, no file is written. If the file cannot be written after the secret was
    /// issued, the whole answer is printed on stdout instead, so the secret is not lost. Delete
    /// the file once the secret is stored where it is used.
    #[arg(long = "secret-file", value_name = "PATH", value_parser = secret_file)]
    pub(crate) path: Option<PathBuf>,
}

/// The long help of `hipc api --reveal`: the short help, then every route of the library's
/// secret table except [`UNLISTED_SECRET_ROUTES`], written as `hipc api` takes the path. It is
/// built from the table, so a route added there is listed here without a second edit.
fn reveal_long_help() -> String {
    let mut text = "Print a secret; required for the requests that return one, or nothing is \
                    sent\n\nThe requests that return one, where <id> is one path segment and \
                    <n> a number:\n"
        .to_owned();
    for (method, route) in hippius::SECRET_ROUTES {
        if !UNLISTED_SECRET_ROUTES.contains(&(*method, *route)) {
            text.push_str(&format!("  {method} {}\n", typed_route(route)));
        }
    }
    text
}

/// The short help of `hipc api`: the writes need `--yes`, and so does every read of the
/// library's provisioning table, named from the table as [`reveal_long_help`] names the
/// secret table, so the help, `hipc schema`, and the guard cannot drift. The methods of one
/// route are joined on its line: `GET or HEAD /network/me/`. It is the line `hipc --help`
/// shows for the command, so it stays short; the auth and account rules go in the long help.
fn api_about() -> String {
    let mut reads: Vec<(String, Vec<String>)> = Vec::new();
    for (method, route) in hippius::PROVISIONING_ROUTES {
        let typed = typed_route(route);
        match reads.iter_mut().find(|(known, _)| *known == typed) {
            Some((_, methods)) => methods.push(method.to_string()),
            None => reads.push((typed, vec![method.to_string()])),
        }
    }
    let reads: Vec<String> = reads
        .into_iter()
        .map(|(route, methods)| format!("{} {route}", methods.join(" or ")))
        .collect();
    format!(
        "Call any console API endpoint; a method other than GET or HEAD needs --yes, as does \
         {}, which may create the network",
        reads.join(" and ")
    )
}

/// The long help of `hipc api`: the short help, as clap takes it from a doc comment, then
/// what the short help has no room for.
fn api_long_about() -> String {
    format!(
        "{}\n\nThe request carries the same auth, account, project, and billing rules as every \
         command. It goes only to the configured API origin: a full URL on any other origin, \
         plain http to a loopback port included, exits 2 usage; use curl for another host. An \
         account deletion request is never sent: request it in the console. No request to the \
         account token route (/auth/token/, any method) is sent: create a token in the console \
         settings. A read \
         of a Kubernetes cluster's secrets (GET or HEAD \
         /kubernetes/clusters/<cluster_id>/secrets/) is never sent either: hipc compute \
         kubernetes clusters kubeconfig opens the kubeconfig. A CDN hostname's DNS setup (POST \
         /cdn/zones/<zone_id>/hostnames/<hostname>/dns-setup/) is never sent: hipc cdn hostnames \
         dns-setup reads the provider credential from a private file. A Hub subscribe (POST \
         /registry/subscribe/) is never sent: hipc hub subscription subscribe reads the current \
         plans first and refuses a second paid one. An App Engine launch, \
         compose update, or compose check whose body sets secrets (POST \
         /compute/app-engine/apps/, any write of /compute/app-engine/apps/<vm_id>/compose/, POST \
         /compute/app-engine/validate/) is never sent: hipc compute app-engine apps create \
         launches an app with secrets read from a private file, and a compose update with \
         secrets is not in this version; without secrets they are sent as usual. A POST, PUT, \
         PATCH, or DELETE under /support/, such as a new ticket, a reply, or a chat handoff, \
         whose body holds HIPPIUS_TOKEN or a token a profile of the config file stores, as it \
         is, quoted, percent-encoded, or behind a JSON escape, is never sent: a person on the \
         support team may read it. Without a token, a PATCH of one support ticket (PATCH \
         /support/tickets/<ticket_id>/) is sent with the body given: the server decides which \
         fields a customer may change, and hipc support tickets update sends only the close. \
         A resume of a Kubernetes operation blocked on data loss is sent: the \
         server is the guard, and the refusal of hipc compute kubernetes operations resume is a \
         courtesy. hipc api sends no If-Match, so a PUT of a zone's WAF rules or rate limits \
         (/cdn/zones/<zone_id>/waf/rules/, /cdn/zones/<zone_id>/waf/rate-limits/) is not \
         conditional and writes over a version another client published since it was read; hipc \
         cdn waf rules update and hipc cdn waf rate-limits update send the version they read and \
         stop on a conflict. A 2xx answer larger than 32 MiB is not printed: it exits 1 internal. On an \
         error status a body that large is dropped, and the status decides the error.",
        api_about()
    )
}

/// Secret-table routes the `--reveal` help leaves out: staff and operator routes that no `hipc`
/// command sends. The guard still applies to them; customer-facing help lists only customer
/// routes.
const UNLISTED_SECRET_ROUTES: [(hippius::Method, &str); 8] = [
    (
        hippius::Method::Post,
        "/api/infrastructure/compute-miners/prepare/",
    ),
    (hippius::Method::Post, "/api/network/infra/enroll/"),
    (
        hippius::Method::Get,
        "/api/support/chat/tickets/{id}/attachments/{id}/",
    ),
    (hippius::Method::Post, "/api/chat/workspaces/{id}/invites/"),
    (hippius::Method::Post, "/api/chat/visitors/"),
    (hippius::Method::Post, "/api/chat/transfer/"),
    (hippius::Method::Post, "/api/chat/transfer/redeem/"),
    (hippius::Method::Get, "/api/auth/token/"),
];

/// A route of the secret table as a person types it to `hipc api`: without the /api prefix,
/// and with `<id>` for `{id}`, because clap's help renderer reads `{n}` as a line break.
fn typed_route(route: &str) -> String {
    route
        .strip_prefix("/api")
        .unwrap_or(route)
        .replace('{', "<")
        .replace('}', ">")
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    /// An id that reads as a flag goes last, after `--`, and the printed command parses with
    /// the id as the positional; any other id stays in place.
    #[test]
    fn a_printed_command_puts_a_dash_id_after_the_separator() {
        assert_eq!(
            with_id("compute firewall get", "vm-1", ""),
            "hipc compute firewall get vm-1"
        );
        assert_eq!(
            with_id("compute firewall update", "-vm1", "--rules-file <path>"),
            "hipc compute firewall update --rules-file <path> -- -vm1"
        );
        for command in [
            with_id("compute firewall get", "-vm1", ""),
            with_id("compute exposures list", "--help", ""),
        ] {
            let words: Vec<&str> = command.split(' ').collect();
            let parsed = Cli::try_parse_from(&words).expect("the command parses");
            assert!(
                format!("{parsed:?}").contains(words[words.len() - 1]),
                "{command}"
            );
        }
    }

    /// What is wrong with one help text, if anything: help is plain text (no Markdown
    /// backticks, no `<https://...>` link syntax), and a short help line is one sentence in
    /// sentence case, with no period inside or at the end.
    fn help_problem(text: &str, short: bool) -> Option<&'static str> {
        if text.trim().is_empty() {
            return Some("no help");
        }
        if text.contains('`') {
            return Some("Markdown backticks");
        }
        if text.contains("<http") {
            return Some("link syntax around a URL");
        }
        if short {
            if !text.starts_with(|c: char| c.is_ascii_uppercase()) {
                return Some("a short help that does not start with a capital");
            }
            if text.ends_with('.') {
                return Some("a final period");
            }
            // A period inside a name (`s3.hippius.com`, `index.html`) is not a sentence end;
            // one before a space or a capital (`One.Two`) is.
            let chars: Vec<char> = text.chars().collect();
            let ends_a_sentence = chars
                .windows(2)
                .any(|pair| pair[0] == '.' && (pair[1] == ' ' || pair[1].is_uppercase()));
            if ends_a_sentence {
                return Some("more than one sentence");
            }
        }
        None
    }

    /// Every help page, the top level, each group, and each command, has examples;
    /// every example parses, so none goes stale; and the examples of a page run the
    /// command or group it documents, past any leading flag that takes no value, such as
    /// `--dry-run`. The top-level help ends with the exit codes, where an agent starts, and
    /// the issue tracker.
    #[test]
    fn every_page_has_examples_that_parse_and_the_top_level_help_says_where_to_go() {
        let mut problems = Vec::new();
        examples_of(&Cli::command(), &[], &mut problems);
        assert!(problems.is_empty(), "{}", problems.join("\n"));

        let mut root = Cli::command();
        let help = root.render_long_help().to_string();
        for part in [
            "Examples:",
            "Exit codes:",
            "Agents and CI: when CI or AI_AGENT is set",
            "--json",
            "hipc schema",
            hippius::ISSUES_URL,
        ] {
            assert!(
                help.contains(part),
                "{part} missing from the top-level help"
            );
        }
    }

    /// The problems of the examples of `command`, whose words after `hipc` are `path`, and of
    /// every command under it.
    fn examples_of(command: &clap::Command, path: &[String], problems: &mut Vec<String>) {
        let page = format!("hipc {}", path.join(" "));
        let after = command
            .get_after_help()
            .map(ToString::to_string)
            .unwrap_or_default();
        let examples: Vec<&str> = after
            .split_once("Examples:\n")
            .map(|(_, rest)| {
                rest.lines()
                    .map_while(|line| line.strip_prefix("  "))
                    .collect()
            })
            .unwrap_or_default();
        if examples.is_empty() {
            problems.push(format!("`{page}` has no examples"));
        }
        for example in examples {
            let words: Vec<&str> = example.split(' ').collect();
            if let Err(error) = Cli::try_parse_from(&words) {
                problems.push(format!("`{example}` does not parse: {error}"));
            }
            let runs: Vec<&str> = words
                .iter()
                .skip(1)
                .copied()
                .skip_while(|word| word.starts_with("--"))
                .collect();
            if !runs.starts_with(&path.iter().map(String::as_str).collect::<Vec<_>>()) {
                problems.push(format!("`{example}` on the page of `{page}` runs another"));
            }
        }
        for sub in command.get_subcommands() {
            let mut sub_path = path.to_vec();
            sub_path.push(sub.get_name().to_owned());
            examples_of(sub, &sub_path, problems);
        }
    }

    /// The examples check finds a page without examples, an example that does not parse,
    /// and one that runs another command than its page's.
    #[test]
    fn the_examples_check_refuses_what_an_example_must_not_be() {
        for (after, problem) in [
            ("", "`hipc auth status` has no examples"),
            (
                "Examples:\n  hipc auth status --nope\n",
                "`hipc auth status --nope` does not parse",
            ),
            (
                "Examples:\n  hipc auth login --token-file t\n",
                "runs another",
            ),
        ] {
            let mut problems = Vec::new();
            examples_of(
                &clap::Command::new("status").after_help(after),
                &["auth".to_owned(), "status".to_owned()],
                &mut problems,
            );
            assert_eq!(problems.len(), 1, "{problems:#?}");
            assert!(problems[0].contains(problem), "{problems:#?}");
        }
    }

    /// The `hipc api` help names every read of the library's provisioning table, as `hipc api`
    /// takes the path, on the line `hipc --help` shows for the command and at the top of `hipc
    /// api --help`, which goes on to the long text; the help stays plain text.
    #[test]
    fn the_api_help_names_every_provisioning_read() {
        let about = api_about();
        assert_eq!(help_problem(&about, true), None, "{about}");
        assert!(
            about.ends_with(", as does GET or HEAD /network/me/, which may create the network"),
            "{about}"
        );
        // `hipc --help` wraps the line at the width of the other commands'; a longer one
        // would take a third line where every other command takes one or two.
        assert!(about.chars().count() <= 140, "{about}");
        for (_, route) in hippius::PROVISIONING_ROUTES {
            assert!(route.starts_with("/api/"), "{route}");
            assert!(
                about.contains(&typed_route(route)),
                "{route} missing from: {about}"
            );
        }
        let mut root = Cli::command();
        let root_help = root.render_help().to_string();
        assert!(
            root_help.contains("GET or HEAD /network/me/"),
            "{root_help}"
        );
        let api = root
            .find_subcommand_mut("api")
            .expect("hipc api is a command");
        let long = api.render_long_help().to_string();
        assert!(long.contains("GET or HEAD /network/me/"), "{long}");
        assert!(
            long.contains("An account deletion request is never sent"),
            "{long}"
        );
        assert!(
            long.contains(
                "GET or HEAD /kubernetes/clusters/<cluster_id>/secrets/) is never sent either: \
                 hipc compute kubernetes clusters kubeconfig opens the kubeconfig"
            ),
            "{long}"
        );
        assert!(
            long.contains(
                "(PATCH /support/tickets/<ticket_id>/) is sent with the body given: the server \
                 decides which fields a customer may change"
            ),
            "{long}"
        );
        assert!(
            long.contains("blocked on data loss is sent: the server is the guard"),
            "{long}"
        );
        assert!(api_long_about().starts_with(&format!("{about}\n\n")));
    }

    /// The reveal help lists every customer route of the library's secret table, as `hipc api`
    /// takes the path (without /api), so a person with only the binary can see them. The staff
    /// routes in `UNLISTED_SECRET_ROUTES` stay guarded but are left out of the help.
    #[test]
    fn the_reveal_help_lists_every_customer_secret_route() {
        let mut root = Cli::command();
        let api = root
            .find_subcommand_mut("api")
            .expect("hipc api is a command");
        let help = api.render_long_help().to_string();
        assert!(!help.contains("PLAN.md"), "{help}");
        // The long help repeats the short help, which clap takes from the doc comment.
        let short = api
            .get_arguments()
            .find(|a| a.get_id() == "reveal")
            .and_then(clap::Arg::get_help)
            .expect("--reveal has a short help")
            .to_string();
        assert!(
            reveal_long_help().starts_with(&format!("{short}\n\n")),
            "{short}"
        );
        assert!(help.contains("  POST /databases/<n>/restore/\n"), "{help}");
        for (method, route) in hippius::SECRET_ROUTES {
            assert!(route.starts_with("/api/"), "{route}");
            let line = format!("  {method} {}\n", typed_route(route));
            if UNLISTED_SECRET_ROUTES.contains(&(*method, *route)) {
                assert!(!help.contains(&line), "{line:?} listed in:\n{help}");
            } else {
                assert!(help.contains(&line), "{line:?} missing from:\n{help}");
            }
        }
        // An unlisted route that left the secret table would hide nothing and go stale.
        for unlisted in UNLISTED_SECRET_ROUTES {
            assert!(hippius::SECRET_ROUTES.contains(&unlisted), "{unlisted:?}");
        }
        // The --remove-others help names customer routes only; the guard still covers the
        // operator storage route.
        assert!(!help.contains("/operator/"), "{help}");
        let short_help = api.render_help().to_string();
        assert!(!short_help.contains("/operator/"), "{short_help}");
    }

    /// Every command and every flag has help, and none breaks the rules of `help_problem`:
    /// the tree is walked, so a new command or flag is checked too.
    #[test]
    fn every_help_text_is_plain_and_one_sentence_per_short_line() {
        let mut problems = Vec::new();
        walk(&Cli::command(), "hipc", &mut problems);
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    /// The checks themselves catch what the help style forbids, in an about, a long about, the help
    /// before and after, and a flag.
    #[test]
    fn the_help_checks_refuse_what_help_must_not_be() {
        let bad = clap::Command::new("x")
            .about("Two sentences. In one line.")
            .long_about("Uses `backticks` and <https://x.example>")
            .before_help("Before, with `backticks`")
            .after_help("Examples:\n  see <https://x.example>")
            .arg(clap::Arg::new("flag").long("flag").help("lowercase start"))
            .arg(
                clap::Arg::new("other")
                    .long("other")
                    .long_help("Ends here."),
            );
        let mut problems = Vec::new();
        walk(&bad, "x", &mut problems);
        assert_eq!(problems.len(), 6, "{problems:#?}");
        for (text, short, problem) in [
            ("", true, "no help"),
            ("A link <https://x>", false, "link syntax around a URL"),
            ("Ends.", true, "a final period"),
            ("Ends. Again", true, "more than one sentence"),
            ("One.Two", true, "more than one sentence"),
            ("For hosts such as s3.hippius.com and index.html", true, ""),
            ("Fine, in one sentence", true, ""),
        ] {
            assert_eq!(help_problem(text, short).unwrap_or(""), problem, "{text}");
        }
    }

    /// Every help text under `command`, checked by `help_problem`, with where it is.
    fn walk(command: &clap::Command, path: &str, problems: &mut Vec<String>) {
        let mut check = |what: &str, text: Option<String>, short: bool| {
            if let Some(problem) = help_problem(text.as_deref().unwrap_or(""), short) {
                problems.push(format!("{path} {what}: {problem}: {text:?}"));
            }
        };
        check("about", command.get_about().map(ToString::to_string), true);
        for (what, text) in [
            ("long about", command.get_long_about()),
            ("before help", command.get_before_help()),
            ("before long help", command.get_before_long_help()),
            ("after help", command.get_after_help()),
            ("after long help", command.get_after_long_help()),
        ] {
            if let Some(text) = text {
                check(what, Some(text.to_string()), false);
            }
        }
        for arg in command.get_arguments() {
            let name = format!("--{}", arg.get_id());
            check(&name, arg.get_help().map(ToString::to_string), true);
            if let Some(long) = arg.get_long_help() {
                check(&format!("{name} long"), Some(long.to_string()), false);
            }
        }
        for sub in command.get_subcommands() {
            walk(sub, &format!("{path} {}", sub.get_name()), problems);
        }
    }

    /// No help text, at any depth, names the API schema file: the help says what the API
    /// documents, in its own words.
    #[test]
    fn no_help_text_says_swagger() {
        fn walk(command: &mut clap::Command, path: &str, found: &mut Vec<String>) {
            let help = command.render_long_help().to_string();
            if help.to_lowercase().contains("swagger") {
                found.push(format!("{path}: {help}"));
            }
            for sub in command.get_subcommands_mut() {
                let path = format!("{path} {}", sub.get_name());
                walk(sub, &path, found);
            }
        }
        let mut root = Cli::command();
        root.build();
        let mut found = Vec::new();
        walk(&mut root, "hipc", &mut found);
        assert!(found.is_empty(), "{}", found.join("\n"));
        // The walk finds the word in a nested command's flag help, in any case.
        let mut bad = clap::Command::new("x").subcommand(
            clap::Command::new("y").arg(clap::Arg::new("z").long("z").help("As Swagger lists")),
        );
        bad.build();
        walk(&mut bad, "x", &mut found);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].starts_with("x y: "), "{found:?}");
    }

    /// Every `--name` a help text names is a flag of some `hipc` command, so a help sentence
    /// cannot name a flag that does not exist (`--grants` for `--grant`). A help may name a
    /// flag of another command (`vms create --ssh-key-id`), so the whole tree counts.
    /// The examples after the help are left out: they parse.
    #[test]
    fn the_help_names_only_flags_that_exist() {
        let root = Cli::command();
        // `--hippius-token` is a flag of hippius-hub, which the `hipc hub` help names.
        let mut known = vec![
            "help".to_owned(),
            "version".to_owned(),
            "hippius-token".to_owned(),
        ];
        longs(&root, &mut known);
        let mut problems = Vec::new();
        unknown_flags(&root, "hipc", &known, &mut problems);
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    /// The check of `the_help_names_only_flags_that_exist` finds a misspelled flag in an
    /// about, a flag's help, and a value's help, and accepts a known flag.
    #[test]
    fn the_flag_check_finds_a_flag_that_does_not_exist() {
        let bad = clap::Command::new("x")
            .about("Pass --grants or --yes")
            .arg(clap::Arg::new("grant").long("grant").help("See --nope-"))
            .arg(clap::Arg::new("kind").long("kind").value_parser([
                clap::builder::PossibleValue::new("a").help("Needs --nada"),
                clap::builder::PossibleValue::new("b"),
            ]))
            .subcommand(clap::Command::new("y").about("Uses --deep"));
        let mut problems = Vec::new();
        unknown_flags(
            &bad,
            "x",
            &["yes".to_owned(), "deep".to_owned()],
            &mut problems,
        );
        assert_eq!(problems, ["x: --grants", "x: --nope", "x: --nada"]);
        let mut known = Vec::new();
        longs(&bad, &mut known);
        assert_eq!(known, ["grant", "kind"]);
    }

    /// The long flags of `command` and of every command under it.
    fn longs(command: &clap::Command, out: &mut Vec<String>) {
        out.extend(
            command
                .get_arguments()
                .filter_map(|arg| arg.get_long().map(str::to_owned)),
        );
        for sub in command.get_subcommands() {
            longs(sub, out);
        }
    }

    /// Every `--name` in the help of `command` and the commands under it that is not in
    /// `known`, with where it is.
    fn unknown_flags(
        command: &clap::Command,
        path: &str,
        known: &[String],
        problems: &mut Vec<String>,
    ) {
        let values = command.get_arguments().flat_map(|arg| {
            arg.get_possible_values()
                .into_iter()
                .filter_map(|value| value.get_help().map(ToString::to_string))
        });
        let texts = [command.get_about(), command.get_long_about()]
            .into_iter()
            .flatten()
            .chain(
                command
                    .get_arguments()
                    .flat_map(|arg| [arg.get_help(), arg.get_long_help()])
                    .flatten(),
            )
            .map(ToString::to_string)
            .chain(values);
        for text in texts {
            for (start, _) in text.match_indices("--") {
                let name: String = text[start + 2..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                    .collect();
                let name = name.trim_end_matches('-');
                if !name.is_empty() && !known.iter().any(|long| long == name) {
                    problems.push(format!("{path}: --{name}"));
                }
            }
        }
        for sub in command.get_subcommands() {
            unknown_flags(sub, &format!("{path} {}", sub.get_name()), known, problems);
        }
    }

    #[test]
    fn repeated_global_reads_argv_as_clap_does() {
        use std::os::unix::ffi::OsStringExt as _;
        let argv = |words: &[&str]| -> Vec<OsString> {
            std::iter::once("hipc")
                .chain(words.iter().copied())
                .map(OsString::from)
                .collect()
        };
        assert_eq!(
            repeated_global(&argv(&["--json", "x", "--json"])),
            Some("output")
        );
        assert_eq!(
            repeated_global(&argv(&["--account", "a", "x", "--account=b"])),
            Some("account")
        );
        // The value of an option is not read as a flag, unless it starts with `-`.
        assert_eq!(repeated_global(&argv(&["--profile", "account", "x"])), None);
        assert_eq!(
            repeated_global(&argv(&["--profile", "--profile"])),
            Some("profile")
        );
        assert_eq!(
            repeated_global(&argv(&["x", "--", "--json", "--json"])),
            None
        );
        assert_eq!(
            repeated_global(&argv(&["--yes", "x", "--yes"])),
            None,
            "flags may repeat"
        );
        assert_eq!(repeated_global(&argv(&["--profile"])), None);
        // A word that is not UTF-8, such as a `--token-file` path, matches nothing.
        let mut odd = argv(&["--profile", "a"]);
        odd.push(OsString::from_vec(b"--profile=\xff".to_vec()));
        odd.push(OsString::from_vec(vec![0xff]));
        assert_eq!(repeated_global(&odd), None);
        // ...but it is taken as the value of the option before it.
        let mut odd = argv(&["--profile"]);
        odd.push(OsString::from_vec(vec![0xff]));
        odd.push(OsString::from("--profile=b"));
        assert_eq!(repeated_global(&odd), Some("profile"));
    }

    #[test]
    fn parser_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn global_flags_work_after_the_subcommand() {
        let cli = Cli::try_parse_from([
            "hipc",
            "auth",
            "status",
            "--json",
            "--profile",
            "work",
            "--yes",
            "--debug",
        ])
        .unwrap();
        assert!(cli.global.json && cli.global.yes && cli.global.debug);
        assert_eq!(cli.global.profile.as_deref(), Some("work"));
        assert_eq!(cli.global.request_timeout, 30);
        let cli = Cli::try_parse_from(["hipc", "schema", "--output", "text"]).unwrap();
        assert!(matches!(cli.global.output, OutputArg::Text));
    }

    #[test]
    fn request_timeout_is_bounded() {
        for bad in ["0", "3601", "-1", "ten"] {
            assert!(
                Cli::try_parse_from(["hipc", "--request-timeout", bad, "version"]).is_err(),
                "{bad}"
            );
        }
        let cli = Cli::try_parse_from(["hipc", "--request-timeout", "3600", "version"]).unwrap();
        assert_eq!(cli.global.request_timeout, 3600);
    }

    #[test]
    fn there_is_no_token_flag() {
        assert!(Cli::try_parse_from(["hipc", "auth", "login", "--token", "x"]).is_err());
        // Without `--token-file`, a terminal is asked for the token (`auth::login`).
        let cli = Cli::try_parse_from(["hipc", "auth", "login"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Auth(Auth::Login {
                token_file: None,
                access_key_file: None,
                access_key: false,
                new_account: false,
            })
        ));
        // Nor a flag that takes the access key's words: `--access-key` takes no value.
        assert!(Cli::try_parse_from(["hipc", "auth", "login", "--mnemonic", "x"]).is_err());
        assert!(Cli::try_parse_from(["hipc", "auth", "login", "--access-key=x"]).is_err());
        for pair in [
            ["--token-file", "t", "--access-key-file", "k"].as_slice(),
            &["--token-file", "t", "--access-key"],
            &["--access-key-file", "k", "--access-key"],
        ] {
            let args = [&["hipc", "auth", "login"][..], pair].concat();
            assert!(Cli::try_parse_from(args).is_err(), "{pair:?}");
        }
    }

    #[test]
    fn dash_v_is_not_a_flag() {
        assert!(Cli::try_parse_from(["hipc", "-v", "version"]).is_err());
        let error = Cli::try_parse_from(["hipc", "-V"]).unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::DisplayVersion);
    }

    /// Every slug id parser holds the one rule, 1 to its maximum of ASCII letters, digits,
    /// `-`, and `_`, and refuses with its own message, whichever command takes it.
    #[test]
    fn every_slug_id_parser_holds_one_rule_with_its_own_message() {
        let parsers: [(&[&str], &[&str], usize, &str); 7] = [
            (
                &["compute", "vms", "get"],
                &[],
                64,
                "a VM id is 1 to 64 letters, digits, `-`, and `_`",
            ),
            (
                &["compute", "backups", "restore", "vm-1", "--run-id"],
                &[],
                64,
                "a backup point is 1 to 64 letters, digits, `-`, and `_`",
            ),
            (
                &["network", "peers", "get"],
                &[],
                64,
                "a peer id is 1 to 64 letters, digits, `-`, and `_`, from `hipc network peers list`",
            ),
            (
                &["network", "vpcs", "routers", "list"],
                &[],
                64,
                "a VPC network id is 1 to 64 letters, digits, `-`, and `_`, from \
                 `hipc network vpcs list`",
            ),
            (
                &["compute", "kubernetes", "nodes", "delete", "7"],
                &[],
                64,
                "a node name is 1 to 64 letters, digits, `-`, and `_`, from \
                 `hipc compute kubernetes node-pools list`",
            ),
            (
                &["s3", "subscription", "checkout"],
                &[],
                128,
                "an intent id is 1 to 128 letters, digits, `-`, and `_`",
            ),
            (
                &["drive", "subscription", "checkout"],
                &[],
                128,
                "an intent id is 1 to 128 letters, digits, `-`, and `_`",
            ),
        ];
        for (command, after, max, refusal) in parsers {
            let parse = |value: &str| {
                let argv = ["hipc"]
                    .iter()
                    .chain(command)
                    .copied()
                    .chain([value])
                    .chain(after.iter().copied());
                Cli::try_parse_from(argv)
            };
            let longest = "a".repeat(max);
            for good in ["a", "A-9_z", "-", "_x", longest.as_str()] {
                assert!(parse(good).is_ok(), "{command:?} {good:?}");
            }
            let too_long = "a".repeat(max + 1);
            for bad in [
                "",
                too_long.as_str(),
                "a.b",
                "..",
                "a/b",
                "a?b",
                "a b",
                "a%2F",
                "é",
            ] {
                let error = parse(bad).expect_err(bad).to_string();
                assert!(error.contains(refusal), "{command:?} {bad:?}: {error}");
            }
        }
    }
}
