//! The grammar of `hipc compute runners`.

use clap::{Args, Subcommand, ValueEnum};

use crate::cli::{
    CursorArgs, INSTALLATION_ID, country_code, credits, examples, normal_credits, number,
};

#[derive(Debug, Subcommand)]
pub(crate) enum Runners {
    /// Whether runners are on, how many run at once, the sizes and region they use, and the flavor
    /// behind each size
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute runners settings get",
        "hipc compute runners settings update --enabled true --yes",
    ]))]
    Settings(RunnerSettings),
    /// The GitHub App installations linked to the account
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute runners installations list",
        "hipc compute runners installations delete 4 --yes",
    ]))]
    Installations(RunnerInstallations),
    /// The runner jobs, newest first, one page at a time
    ///
    /// A job the server did not launch carries its refusal (such as compute_runway, concurrency, or
    /// runners_disabled) on the row; that is not an error of this command. The server's page size
    /// is fixed, so there is no --limit.
    #[command(after_help = examples(&[
        "hipc compute runners jobs",
        "hipc compute runners jobs --all",
    ]))]
    Jobs(CursorArgs),
    /// Start linking a GitHub organization or user: prints the URL that installs the Hippius
    /// Runners GitHub App
    ///
    /// Open url in a browser before expires_in seconds pass. Nothing is linked until the App is
    /// installed and GitHub returns to the console, which finishes the link; hipc cannot finish it.
    /// installations list then shows the installation.
    #[command(after_help = examples(&[
        "hipc compute runners connect",
    ]))]
    Connect,
    /// What the account's runners cost over a period, from the compute ledger
    #[command(after_help = examples(&[
        "hipc compute runners usage",
    ]))]
    Usage,
    /// The account's monthly spend limit on runners, not yet enabled on api.hippius.com, which
    /// answers 404
    ///
    /// The server answers 404 while the limit is not enabled on it. A dry run sends nothing, so
    /// it exits 0 whether the limit is enabled or not. Until it is, hipc compute runners
    /// settings update --max-concurrent and --allowed-size bound how many runners run at once
    /// and how large they are.
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute runners limit get",
        "hipc compute runners limit update --monthly-cap 50",
    ]))]
    Limit(RunnerLimit),
}

#[derive(Debug, Subcommand)]
pub(crate) enum RunnerLimit {
    /// The monthly runner spend limit
    #[command(after_help = examples(&[
        "hipc compute runners limit get",
    ]))]
    Get,
    /// Set or remove the monthly runner spend limit; needs --yes when it raises or removes the
    /// cap, or when the cap in force is unknown
    ///
    /// It reads the limit first and compares with its monthly_cap_credits: raising the cap, or
    /// removing it, lets runners spend more, so it is confirmed; so is any change while the
    /// limit has no amount there, and setting a cap while it shows null, which may be a server
    /// default. Lowering the cap, or keeping it, is not. The amount is sent in monthly_cap,
    /// without leading or trailing zeros and with at most 18 digits after the point, and an
    /// answer whose monthly_cap_credits is not that amount is an error. While the limit is not
    /// enabled on the server, the read answers 404 and nothing is written.
    #[command(after_help = examples(&[
        "hipc compute runners limit update --monthly-cap 50",
        "hipc compute runners limit update --monthly-cap none --yes",
    ]))]
    Update {
        /// The monthly spend cap in credits (1 credit = $1), or none to remove the cap
        #[arg(long, value_name = "CREDITS", value_parser = monthly_cap)]
        monthly_cap: MonthlyCap,
    },
}

/// `--monthly-cap` of the runner limit: an amount of credits, or no cap, which is sent as
/// `null`.
#[derive(Debug, Clone)]
pub(crate) enum MonthlyCap {
    Credits(String),
    None,
}

fn monthly_cap(value: &str) -> Result<MonthlyCap, String> {
    if value == "none" {
        return Ok(MonthlyCap::None);
    }
    let credits = normal_credits(&credits(value).map_err(|_| {
        "a cap is a decimal number of credits, such as `50` or `20.5`, or `none`".to_owned()
    })?);
    // A credit is 10^18 base units, so a finer amount cannot be stored as sent, and the
    // answer, which the update compares exactly with the amount sent, could never show it.
    if credits
        .split_once('.')
        .is_some_and(|(_, fraction)| fraction.len() > MAX_CAP_FRACTION_DIGITS)
    {
        return Err(format!(
            "a cap has at most {MAX_CAP_FRACTION_DIGITS} digits after the point, the smallest \
             part of a credit"
        ));
    }
    Ok(MonthlyCap::Credits(credits))
}

/// The decimals of a credit: 1 credit is 10^18 base units.
const MAX_CAP_FRACTION_DIGITS: usize = 18;

#[derive(Debug, Subcommand)]
pub(crate) enum RunnerSettings {
    /// The runner settings, with effective_max_concurrent (the cap after the quota), active
    /// (runners launching or running now), and size_flavors
    #[command(after_help = examples(&[
        "hipc compute runners settings get",
    ]))]
    Get,
    /// Change the settings given; the others are kept; jobs spend money while runners are
    /// enabled: needs --yes when the change may spend more
    ///
    /// While runners are enabled, every job a linked repository starts runs on a VM billed to the
    /// balance. It reads the settings first: turning runners on, a higher --max-concurrent,
    /// --max-concurrent 0, an --allowed-size the list lacks, and opening public repositories may
    /// spend more, so they are confirmed, and so is a setting it cannot compare. A pure cut, such
    /// as --enabled false, a lower --max-concurrent, or fewer sizes, needs no --yes, and neither
    /// does --default-region, which moves where jobs run, not how many. The read and the write
    /// are separate requests, since the route takes no version, so a change someone else makes
    /// between them is not seen.
    #[command(after_help = examples(&[
        "hipc compute runners settings update --enabled true --yes",
        "hipc compute runners settings update --max-concurrent 4 --allowed-size small --yes",
        "hipc compute runners settings update --enabled false",
    ]))]
    Update(RunnerSettingsUpdate),
}

/// `hipc compute runners settings update`: the settings to change. One at least is required,
/// so `hipc schema` lists the group, as it does for the other updates of a few fields.
#[derive(Debug, Args)]
#[command(group(
    clap::ArgGroup::new("change")
        .required(true)
        .multiple(true)
        .args([
            "enabled",
            "max_concurrent",
            "allowed_sizes",
            "default_region",
            "allow_public_repos",
        ])
))]
pub(crate) struct RunnerSettingsUpdate {
    /// Turn runners on or off: true or false
    #[arg(long, value_name = "BOOL")]
    pub(crate) enabled: Option<bool>,
    /// The most runners at once
    ///
    /// The server caps it at the account's runner quota. What 0 means is not documented by the
    /// server, and it may mean no cap, so 0 needs --yes, as a raise does, unless the setting is
    /// already 0.
    #[arg(long, value_name = "N")]
    pub(crate) max_concurrent: Option<u32>,
    /// A size jobs may use: small, medium, or large; repeat it for several; the list given replaces
    /// the current one
    #[arg(long = "allowed-size", value_name = "SIZE", value_enum)]
    pub(crate) allowed_sizes: Vec<RunnerSize>,
    /// Where runners launch when a job's label names no region: a two-letter country code, such as
    /// FR, or anywhere
    #[arg(long, value_name = "CC", value_parser = runner_region)]
    pub(crate) default_region: Option<String>,
    /// Whether jobs from public repositories may run: true or false
    #[arg(long, value_name = "BOOL")]
    pub(crate) allow_public_repos: Option<bool>,
}

/// The runner sizes, the `hippius-<size>` labels the console offers.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum RunnerSize {
    Small,
    Medium,
    Large,
}

impl RunnerSize {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }
}

/// A runner `default_region`: `anywhere`, sent as `""`, which the console reads as any
/// region, or a compute region ([`region`](crate::cli::region)).
fn runner_region(value: &str) -> Result<String, String> {
    if value == "anywhere" {
        return Ok(String::new());
    }
    country_code(value)
        .ok_or_else(|| "a region is a two-letter country code, such as `FR`, or `anywhere`".into())
}

#[derive(Debug, Subcommand)]
pub(crate) enum RunnerInstallations {
    /// The linked installations, each with its GitHub account_login, status, and settings_url
    #[command(after_help = examples(&[
        "hipc compute runners installations list",
    ]))]
    List,
    /// Unlink an installation from the account; needs --yes
    ///
    /// Its repositories stop getting Hippius runners. To uninstall the App from GitHub as well, use
    /// the installation's settings_url.
    #[command(after_help = examples(&[
        "hipc compute runners installations delete 4 --yes",
    ]))]
    Delete {
        /// The installation id from hipc compute runners installations list
        #[arg(value_parser = number(INSTALLATION_ID))]
        installation_id: u64,
    },
}
