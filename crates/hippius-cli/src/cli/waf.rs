//! The grammar of `hipc cdn waf` and `hipc cdn lists`, and the
//! `--waf` of `hipc cdn estimate`.

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

use crate::cli::values::{Charset, Dash, word};
use crate::cli::{StatsStep, ZoneTarget, examples, number};

#[derive(Debug, Subcommand)]
pub(crate) enum CdnWaf {
    /// The zone's WAF settings: its mode, tier, block response, active version, and the tier's
    /// limits
    #[command(after_help = examples(&[
        "hipc cdn waf get zab12cd34ef56",
    ]))]
    Get(ZoneTarget),
    /// Change the zone's WAF settings: only the flags given are sent; spends money while the mode
    /// is not off: needs --yes
    ///
    /// The WAF is billed from the balance while its mode is not off, log included, at the price
    /// of its tier, which hipc cdn prices lists under waf. Each change publishes a new version.
    #[command(after_help = examples(&[
        "hipc cdn waf update zab12cd34ef56 --mode on --tier standard --yes",
        "hipc cdn waf update zab12cd34ef56 --block-status 403 --block-content-type html \
         --block-body-file block.html --yes",
    ]))]
    Update(WafUpdate),
    /// The zone's live cache nodes, and how many of them run the active version
    #[command(after_help = examples(&[
        "hipc cdn waf status zab12cd34ef56",
    ]))]
    Status(ZoneTarget),
    /// The zone's WAF matches over a window, as counters
    #[command(after_help = examples(&[
        "hipc cdn waf analytics zab12cd34ef56 --step 1h --group-by rule",
    ]))]
    Analytics(WafAnalytics),
    /// Sampled WAF matches, with each visitor's IP address
    ///
    /// Needs CDN operator or admin access. Requests that match no rule are never recorded.
    #[command(after_help = examples(&[
        "hipc cdn waf events zab12cd34ef56 --action block",
    ]))]
    Events(WafEvents),
    /// Whether a request matches an expression, or which published rules it matches
    ///
    /// Changes nothing and publishes nothing. Needs CDN operator access.
    #[command(after_help = examples(&[
        "hipc cdn waf test zab12cd34ef56 --request-file waf-request.json",
    ]))]
    Test(WafTest),
    /// The zone's custom rules: the ordered list that blocks, allows, or logs a request
    #[command(subcommand, after_help = examples(&[
        "hipc cdn waf rules get zab12cd34ef56",
        "hipc cdn waf rules update zab12cd34ef56 --rules-file waf-rules.json --yes",
    ]))]
    Rules(WafRules),
    /// The zone's rate limits, each answered 429 once a client passes it
    #[command(subcommand, after_help = examples(&[
        "hipc cdn waf rate-limits get zab12cd34ef56",
        "hipc cdn waf rate-limits update zab12cd34ef56 --rules-file rate-limits.json --yes",
    ]))]
    RateLimits(WafRateLimits),
    /// The versions each WAF change publishes, and a rollback to one of them
    #[command(subcommand, after_help = examples(&[
        "hipc cdn waf versions list zab12cd34ef56",
        "hipc cdn waf versions rollback zab12cd34ef56 3 --yes",
    ]))]
    Versions(WafVersions),
}

#[derive(Debug, Args)]
pub(crate) struct WafUpdate {
    #[command(flatten)]
    pub(crate) zone: ZoneTarget,
    /// What the WAF does: on blocks, log only counts, and off stops it and its billing
    #[arg(long, value_enum)]
    pub(crate) mode: Option<WafMode>,
    /// The tier, which sets the price and the limits
    #[arg(long, value_enum)]
    pub(crate) tier: Option<WafTier>,
    /// Keep a sample of the matching requests, which hipc cdn waf events reads
    #[arg(long, value_name = "BOOL")]
    pub(crate) event_sampling: Option<bool>,
    /// The HTTP status of a blocked request
    #[arg(long, value_enum, value_name = "STATUS")]
    pub(crate) block_status: Option<BlockStatus>,
    /// The content type of the block page
    #[arg(long, value_enum, value_name = "CONTENT_TYPE")]
    pub(crate) block_content_type: Option<BlockContentType>,
    /// A file holding the block page, at most 4096 bytes of UTF-8 text, or - for stdin
    ///
    /// {ref} in it is replaced by the request's reference.
    ///
    /// It is read only as a regular file, never through a symlink; /dev/stdin and <(...) are
    /// refused, so pass - for stdin.
    #[arg(long, value_name = "PATH")]
    pub(crate) block_body_file: Option<PathBuf>,
}

/// The WAF modes (swagger, `mode` of `PATCH .../waf/`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum WafMode {
    Off,
    Log,
    On,
}

impl WafMode {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Log => "log",
            Self::On => "on",
        }
    }
}

/// The WAF tiers (swagger, `tier` of `PATCH .../waf/`).
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum WafTier {
    Standard,
    Advanced,
}

impl WafTier {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Advanced => "advanced",
        }
    }
}

/// The statuses of a blocked request (swagger, `block_response.status`).
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum BlockStatus {
    #[value(name = "403")]
    Forbidden,
    #[value(name = "429")]
    TooManyRequests,
}

impl BlockStatus {
    pub(crate) fn code(self) -> u16 {
        match self {
            Self::Forbidden => 403,
            Self::TooManyRequests => 429,
        }
    }
}

/// The content types of the block page (swagger, `block_response.content_type`).
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum BlockContentType {
    Html,
    Json,
    Text,
}

impl BlockContentType {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Json => "json",
            Self::Text => "text",
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct WafAnalytics {
    #[command(flatten)]
    pub(crate) zone: ZoneTarget,
    /// The start of the window, as an ISO 8601 date-time
    #[arg(long, value_name = "DATETIME")]
    pub(crate) from: Option<String>,
    /// The end of the window, as an ISO 8601 date-time
    #[arg(long, value_name = "DATETIME")]
    pub(crate) to: Option<String>,
    /// The length of one point of the series
    #[arg(long, value_enum)]
    pub(crate) step: Option<StatsStep>,
    /// Split the counters by rule, action, or visitor country
    #[arg(long, value_enum)]
    pub(crate) group_by: Option<GroupBy>,
}

/// What the analytics counters are split by (the docs' `group_by`).
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum GroupBy {
    Rule,
    Action,
    Country,
}

impl GroupBy {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Rule => "rule",
            Self::Action => "action",
            Self::Country => "country",
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct WafEvents {
    #[command(flatten)]
    pub(crate) zone: ZoneTarget,
    /// The start of the window, as an ISO 8601 date-time
    #[arg(long, value_name = "DATETIME")]
    pub(crate) from: Option<String>,
    /// The end of the window, at most 7 days after the start
    #[arg(long, value_name = "DATETIME")]
    pub(crate) to: Option<String>,
    /// Only the matches of this rule id
    #[arg(long, value_name = "RULE_ID")]
    pub(crate) rule_id: Option<String>,
    /// Only the matches with this action
    #[arg(long, value_enum)]
    pub(crate) action: Option<RuleAction>,
    /// Only the matches from this visitor IP address
    #[arg(long)]
    pub(crate) ip: Option<String>,
}

/// The actions of a custom rule (swagger, `action` of `PUT .../waf/rules/`).
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum RuleAction {
    Block,
    Allow,
    Log,
}

impl RuleAction {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Allow => "allow",
            Self::Log => "log",
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct WafTest {
    #[command(flatten)]
    pub(crate) zone: ZoneTarget,
    /// A JSON file holding the request, or - for stdin, with exactly ten keys
    ///
    /// The keys are ip, country, asn (a number), host, method, path, query, headers (an object
    /// of lowercase names), version, and ssl (true or false). hipc refuses another key, and
    /// the API refuses a request that leaves one out. For example: {"ip": "203.0.113.7",
    /// "country": "FR", "asn": 64496, "host": "example.com", "method": "GET", "path":
    /// "/admin", "query": "", "headers": {"user-agent": "curl/8"}, "version": "HTTP/1.1",
    /// "ssl": true}
    ///
    /// It is read only as a regular file, never through a symlink; /dev/stdin and <(...) are
    /// refused, so pass - for stdin.
    #[arg(long, value_name = "PATH")]
    pub(crate) request_file: PathBuf,
    /// The expression to test the request against; without it, the published rules are tested
    #[arg(long)]
    pub(crate) expression: Option<String>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum WafRules {
    /// The zone's custom rules, in order, and the version they are in
    #[command(after_help = examples(&[
        "hipc cdn waf rules get zab12cd34ef56",
    ]))]
    Get(ZoneTarget),
    /// Replace the whole custom rule list with a file's, publishing a new version; needs --yes,
    /// and --remove-others to drop, move, or change a rule; --check only checks the file
    ///
    /// A rule the zone has now that the file leaves out or moves, or whose expression, action,
    /// or enabled flag it changes, is refused unless --remove-others is passed. There is no
    /// create: a new rule is added by sending the whole list with it.
    ///
    /// A file with one rule: {"rules": [{"description": "admin from the office", "enabled":
    /// true, "action": "block", "expression": "http.request.uri.path starts_with \"/admin\"
    /// and not ip.src in {203.0.113.0/24}"}]}. The action is block, allow, or log.
    ///
    /// With --check the server compiles the list and answers whether it is valid, its size, and
    /// the tier's limit, publishing nothing; that needs no --yes. An invalid list exits 1
    /// validation, with the server's answer in error.details.
    #[expect(
        clippy::doc_link_with_quotes,
        reason = "the help shows an example file, whose JSON arrays of strings look like links"
    )]
    #[command(after_help = examples(&[
        "hipc cdn waf rules update zab12cd34ef56 --rules-file waf-rules.json --check",
        "hipc cdn waf rules update zab12cd34ef56 --rules-file waf-rules.json --yes",
        "hipc cdn waf rules update zab12cd34ef56 --rules-file waf-rules.json --remove-others --yes",
    ]))]
    Update {
        #[command(flatten)]
        zone: ZoneTarget,
        /// A JSON file such as {"rules": [...]}, or - for stdin, sent as it is
        ///
        /// {"rules": []} removes every rule. A rule key the API does not list is refused.
        ///
        /// It is read only as a regular file, never through a symlink; /dev/stdin and <(...) are
        /// refused, so pass - for stdin.
        #[arg(long, value_name = "PATH")]
        rules_file: PathBuf,
        /// Remove the zone's rules the file leaves out
        ///
        /// Without it the current rules are read first, and the update is refused when the file
        /// leaves out a rule the zone has now, moves it, or changes its expression, action, or
        /// enabled flag: the rules run in order, and a block or an allow stops the rest.
        /// With it the rules are still read, after the question, for the version the update is
        /// conditional on.
        #[arg(long)]
        remove_others: bool,
        /// Have the server check the list, publishing nothing; needs no --yes
        ///
        /// It is the update's PUT with dry_run=1, sent as the update would be: after the local
        /// checks of the file, the read of the current list, and the --remove-others guard,
        /// with the version read as If-Match. The answer is the server's: valid, size, and
        /// limit.
        #[arg(long)]
        check: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum WafRateLimits {
    /// The zone's rate limits, and the version they are in
    #[command(after_help = examples(&[
        "hipc cdn waf rate-limits get zab12cd34ef56",
    ]))]
    Get(ZoneTarget),
    // The keys, periods, and actions are the docs' (https://docs.hippius.com/use/cdn/firewall).
    /// Replace the whole rate limit list with a file's, publishing a new version; needs --yes,
    /// and --remove-others to drop or change a rate limit; --check only checks the file
    ///
    /// A rate limit the zone has now that the file leaves out, or whose expression, action, key,
    /// period, requests, timeout, or enabled flag it changes, is refused unless --remove-others
    /// is passed. There is no create: a new rate limit is added by sending the whole list with
    /// it.
    ///
    /// A file with one rate limit, 300 requests a minute per address, then a 10-minute ban:
    /// {"rules": [{"description": "scrapers", "enabled": true, "action": "block", "expression":
    /// "http.request.uri.path starts_with \"/api\"", "key": "ip", "period": 60, "requests": 300,
    /// "timeout": 600}]}. The key is ip, ip+host, or ip+path_prefix; the period is 10, 60, or
    /// 600 seconds; the timeout, the ban, is in seconds; the action is block or log.
    ///
    /// With --check the server compiles the list and answers whether it is valid, its size, and
    /// the tier's limit, publishing nothing; that needs no --yes. An invalid list exits 1
    /// validation, with the server's answer in error.details.
    #[command(after_help = examples(&[
        "hipc cdn waf rate-limits update zab12cd34ef56 --rules-file rate-limits.json --check",
        "hipc cdn waf rate-limits update zab12cd34ef56 --rules-file rate-limits.json --yes",
        "hipc cdn waf rate-limits update zab12cd34ef56 --rules-file rate-limits.json \
         --remove-others --yes",
    ]))]
    Update {
        #[command(flatten)]
        zone: ZoneTarget,
        /// A JSON file such as {"rules": [...]}, or - for stdin, sent as it is
        ///
        /// {"rules": []} removes every rate limit. A key the API does not list is refused.
        ///
        /// It is read only as a regular file, never through a symlink; /dev/stdin and <(...) are
        /// refused, so pass - for stdin.
        #[arg(long, value_name = "PATH")]
        rules_file: PathBuf,
        /// Remove the zone's rate limits the file leaves out
        ///
        /// Without it the current rate limits are read first, and the update is refused when
        /// the file leaves out one the zone has now, or changes what it counts or allows: its
        /// expression, action, key, period, requests, timeout, or enabled flag.
        /// With it the rate limits are still read, after the question, for the version the
        /// update is conditional on.
        #[arg(long)]
        remove_others: bool,
        /// Have the server check the list, publishing nothing; needs no --yes
        ///
        /// It is the update's PUT with dry_run=1, sent as the update would be: after the local
        /// checks of the file, the read of the current list, and the --remove-others guard,
        /// with the version read as If-Match. The answer is the server's: valid, size, and
        /// limit.
        #[arg(long)]
        check: bool,
    },
}

const VERSION: &str = "a WAF version is a whole number, from `hipc cdn waf versions list`";

#[derive(Debug, Subcommand)]
pub(crate) enum WafVersions {
    /// The zone's last 50 WAF versions, newest first
    #[command(after_help = examples(&[
        "hipc cdn waf versions list zab12cd34ef56",
    ]))]
    List(ZoneTarget),
    /// One version and its source
    #[command(after_help = examples(&[
        "hipc cdn waf versions get zab12cd34ef56 3",
    ]))]
    Get {
        #[command(flatten)]
        zone: ZoneTarget,
        /// The version number from hipc cdn waf versions list
        #[arg(value_parser = number(VERSION))]
        version_number: u64,
    },
    /// Publish a copy of an older version as the newest one; spends money while the mode it
    /// then has is not off: needs --yes
    ///
    /// The history is kept. The WAF is billed while the mode it then has is not off.
    #[command(after_help = examples(&[
        "hipc cdn waf versions rollback zab12cd34ef56 3 --yes",
    ]))]
    Rollback {
        #[command(flatten)]
        zone: ZoneTarget,
        /// The version number from hipc cdn waf versions list
        #[arg(value_parser = number(VERSION))]
        version_number: u64,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum CdnLists {
    /// The account's WAF lists
    #[command(after_help = examples(&[
        "hipc cdn lists list",
    ]))]
    List,
    /// One list and its items
    #[command(after_help = examples(&[
        "hipc cdn lists get blocked_ips",
    ]))]
    Get {
        /// The list name from hipc cdn lists list
        #[arg(value_parser = list_name)]
        list_name: String,
    },
    /// Create a list of IP addresses and networks, AS numbers, or country codes
    #[command(after_help = examples(&[
        "hipc cdn lists create blocked_ips --kind ip --items-file items.txt",
        "hipc cdn lists create blocked_countries --kind country",
    ]))]
    Create {
        /// The new list name: 1 to 32 lowercase letters, digits, and _, such as blocked_ips
        #[arg(value_parser = list_name)]
        list_name: String,
        /// What the list holds
        #[arg(long, value_enum)]
        kind: ListKind,
        /// A note for people, at most 256 characters
        #[arg(long, value_parser = list_description)]
        description: Option<String>,
        /// A text file of items, one per line, or - for stdin; # starts a comment
        ///
        /// It is read only as a regular file, never through a symlink; /dev/stdin and <(...) are
        /// refused, so pass - for stdin.
        #[arg(long, value_name = "PATH")]
        items_file: Option<PathBuf>,
    },
    /// Change a list's description
    #[command(after_help = examples(&[
        "hipc cdn lists update blocked_ips --description Abusers",
    ]))]
    Update {
        /// The list name from hipc cdn lists list
        #[arg(value_parser = list_name)]
        list_name: String,
        /// A note for people, at most 256 characters
        #[arg(long, value_parser = list_description)]
        description: String,
    },
    /// Delete a list; needs --yes
    ///
    /// A list a rule uses cannot be deleted.
    #[command(after_help = examples(&[
        "hipc cdn lists delete blocked_ips --yes",
    ]))]
    Delete {
        /// The list name from hipc cdn lists list
        #[arg(value_parser = list_name)]
        list_name: String,
    },
    /// Add, replace, or remove a list's items; each change republishes every zone that uses it
    #[command(subcommand, after_help = examples(&[
        "hipc cdn lists items create blocked_ips --items-file items.txt --yes",
        "hipc cdn lists items delete blocked_ips --items-file items.txt --yes",
    ]))]
    Items(ListItems),
}

/// What a WAF list holds (swagger, `kind` of `POST /api/cdn/lists/`).
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum ListKind {
    Ip,
    Asn,
    Country,
}

impl ListKind {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Ip => "ip",
            Self::Asn => "asn",
            Self::Country => "country",
        }
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum ListItems {
    /// Add the file's items to the list; needs --yes
    ///
    /// Every zone whose WAF uses the list publishes it again, and an item changes what those
    /// zones let through or block.
    #[command(after_help = examples(&[
        "hipc cdn lists items create blocked_ips --items-file items.txt --yes",
    ]))]
    Create {
        /// The list name from hipc cdn lists list
        #[arg(value_parser = list_name)]
        list_name: String,
        /// A text file of items, one per line, or - for stdin; # starts a comment
        ///
        /// It is read only as a regular file, never through a symlink; /dev/stdin and <(...) are
        /// refused, so pass - for stdin.
        #[arg(long, value_name = "PATH")]
        items_file: PathBuf,
    },
    /// Replace every item of the list with the file's; needs --yes, and --remove-others to
    /// drop an item
    ///
    /// An item the list has now that the file leaves out is refused unless --remove-others is
    /// passed. The route takes no version or If-Match, so the items are written whole, after a
    /// read unless --remove-others is passed: an item someone else adds before the write is
    /// lost, and one removed meanwhile comes back if the file names it.
    #[command(after_help = examples(&[
        "hipc cdn lists items update blocked_ips --items-file items.txt --yes",
        "hipc cdn lists items update blocked_ips --items-file items.txt --remove-others --yes",
    ]))]
    Update {
        /// The list name from hipc cdn lists list
        #[arg(value_parser = list_name)]
        list_name: String,
        /// A text file of items, one per line, or - for stdin; # starts a comment
        ///
        /// An empty file removes every item.
        ///
        /// It is read only as a regular file, never through a symlink; /dev/stdin and <(...) are
        /// refused, so pass - for stdin.
        #[arg(long, value_name = "PATH")]
        items_file: PathBuf,
        /// Remove the list's items the file leaves out
        ///
        /// Without it the current items are read first, and the update is refused when the file
        /// leaves out one of them. The list stores items in canonical form, merged and sorted, so
        /// an item is compared with the form hipc cdn lists get prints.
        #[arg(long)]
        remove_others: bool,
    },
    /// Remove the file's items from the list; needs --yes
    #[command(after_help = examples(&[
        "hipc cdn lists items delete blocked_ips --items-file items.txt --yes",
    ]))]
    Delete {
        /// The list name from hipc cdn lists list
        #[arg(value_parser = list_name)]
        list_name: String,
        /// A text file of items, one per line, or - for stdin; # starts a comment
        ///
        /// It is read only as a regular file, never through a symlink; /dev/stdin and <(...) are
        /// refused, so pass - for stdin.
        #[arg(long, value_name = "PATH")]
        items_file: PathBuf,
    },
}

/// A WAF list name: swagger's `^[a-z0-9_]{1,32}$`, which also keeps a `/`, `?`, or `..` out
/// of the URL path it goes into.
fn list_name(value: &str) -> Result<String, String> {
    word(
        value,
        1..=32,
        Charset::LowerUnderscore,
        Dash::Refused,
        "a WAF list name is 1 to 32 lowercase letters, digits, and `_`, such as `blocked_ips`",
    )
}

/// The longest list description, in characters (swagger: `maxLength` 256).
const MAX_LIST_DESCRIPTION: usize = 256;

fn list_description(value: &str) -> Result<String, String> {
    if value.chars().count() <= MAX_LIST_DESCRIPTION {
        Ok(value.to_owned())
    } else {
        Err(format!(
            "a WAF list description is at most {MAX_LIST_DESCRIPTION} characters"
        ))
    }
}

/// The most zones one tier of an estimate takes (swagger, `waf` of `POST
/// /api/cdn/estimate/`: `maximum` 10^6).
const MAX_ESTIMATE_WAF_ZONES: u32 = 1_000_000;

/// One `--waf TIER=ZONES` of `hipc cdn estimate`.
#[derive(Debug, Clone)]
pub(crate) struct TierZones {
    pub(crate) tier: WafTier,
    pub(crate) zones: u32,
}

/// `TIER=ZONES`: a WAF tier and a whole number of zones from 0 to
/// [`MAX_ESTIMATE_WAF_ZONES`].
pub(crate) fn tier_zones(value: &str) -> Result<TierZones, String> {
    let refused = || {
        format!(
            "a `--waf` value is `TIER=ZONES`, with the tier `standard` or `advanced` and 0 to \
             {MAX_ESTIMATE_WAF_ZONES} zones, such as `standard=2`"
        )
    };
    let (tier, zones) = value.split_once('=').ok_or_else(refused)?;
    let tier = WafTier::from_str(tier, false).map_err(|_| refused())?;
    let zones = crate::units::whole_number(zones)
        .and_then(|zones| u32::try_from(zones).ok())
        .ok_or_else(refused)?;
    if zones > MAX_ESTIMATE_WAF_ZONES {
        return Err(refused());
    }
    Ok(TierZones { tier, zones })
}
