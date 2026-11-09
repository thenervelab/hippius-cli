//! The grammar of `hipc cdn`, apart from its WAF and lists.

use std::cmp::Ordering;
use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

use crate::cli::{
    CdnLists, CdnWaf, PURGE_ID, TierZones, credits, examples, normal_credits, number, waf, zone_id,
};

#[derive(Debug, Subcommand)]
pub(crate) enum Cdn {
    /// The CDN price list in force: per GB served by region, per request, per custom hostname, and
    /// the WAF tiers; needs no token
    #[command(after_help = examples(&[
        "hipc cdn prices",
    ]))]
    Prices,
    /// The serving regions, their health, and the CDN's own domain
    #[command(after_help = examples(&[
        "hipc cdn regions",
    ]))]
    Regions,
    /// What a month of traffic would cost at today's prices; needs a token
    ///
    /// Changes nothing. Unlike hipc cdn prices, it needs a token.
    #[command(after_help = examples(&[
        "hipc cdn estimate --traffic FR=100",
        "hipc cdn estimate --traffic FR=100 --traffic US=50 --requests 1000000 --custom-hostnames 1",
    ]))]
    Estimate(CdnEstimate),
    /// List, create, update, and delete zones
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc cdn zones list",
        "hipc cdn zones create --name site --bucket photos --public --yes",
    ]))]
    Zones(CdnZones),
    /// A zone's monthly spend cap, in credits (1 credit = $1)
    ///
    /// The cap counts the zone's traffic, requests, custom hostnames, and WAF add-on. At the cap
    /// the zone is paused: every request, on its default hostname and its custom ones, is
    /// answered 503 and nothing more is billed, until the cap is raised above the month's spend
    /// or the next month starts.
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc cdn cap get zab12cd34ef56",
        "hipc cdn cap update zab12cd34ef56 --monthly-cap 50 --yes",
    ]))]
    Cap(CdnCap),
    /// A zone's custom hostnames, and the DNS records each one needs
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc cdn hostnames create zab12cd34ef56 cdn.example.com --yes",
        "hipc cdn hostnames verify zab12cd34ef56 cdn.example.com",
    ]))]
    Hostnames(CdnHostnames),
    /// Purge cached copies of a zone, and follow each purge's progress across the nodes
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc cdn purges create zab12cd34ef56 --everything",
        "hipc cdn purges get zab12cd34ef56 9",
    ]))]
    Purges(CdnPurges),
    /// A zone's cache rules: the ordered list whose first match sets the cache behavior
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc cdn rules get zab12cd34ef56",
        "hipc cdn rules update zab12cd34ef56 --rules-file cdn-rules.json --yes",
    ]))]
    Rules(CdnRules),
    /// A zone's traffic over a window, as a series and totals
    #[command(after_help = examples(&[
        "hipc cdn stats zab12cd34ef56",
        "hipc cdn stats zab12cd34ef56 --step 1h --region FR",
    ]))]
    Stats(CdnStats),
    /// A zone's web application firewall: its settings, custom rules, rate limits, and
    /// versions; billed while its mode is not off
    #[command(subcommand, after_help = examples(&[
        "hipc cdn waf get zab12cd34ef56",
        "hipc cdn waf update zab12cd34ef56 --mode log --yes",
        "hipc cdn waf rules get zab12cd34ef56",
    ]))]
    Waf(CdnWaf),
    /// The account's WAF lists of IP addresses and networks, AS numbers, or countries, which
    /// WAF rules name
    #[command(subcommand, after_help = examples(&[
        "hipc cdn lists list",
        "hipc cdn lists create blocked_ips --kind ip --items-file items.txt",
        "hipc cdn lists items create blocked_ips --items-file items.txt --yes",
    ]))]
    Lists(CdnLists),
}

#[derive(Debug, Args)]
pub(crate) struct CdnEstimate {
    /// GB served a month in one client region, such as FR=100; repeat for more regions
    #[arg(long, value_name = "REGION=GB", value_parser = region_gb)]
    pub(crate) traffic: Vec<RegionGb>,
    /// Requests a month, at most 10^15
    #[arg(
        long,
        value_name = "N",
        value_parser = clap::value_parser!(u64).range(..=MAX_ESTIMATE_COUNT)
    )]
    pub(crate) requests: Option<u64>,
    /// Custom hostnames, at most 10^15
    #[arg(
        long,
        value_name = "N",
        value_parser = clap::value_parser!(u64).range(..=MAX_ESTIMATE_COUNT)
    )]
    pub(crate) custom_hostnames: Option<u64>,
    /// Zones with the WAF for the whole month by tier, such as standard=2; repeat for the other
    /// tier
    #[arg(long = "waf", value_name = "TIER=ZONES", value_parser = waf::tier_zones)]
    pub(crate) waf: Vec<TierZones>,
}

/// The largest `requests` and `custom_domains` an estimate takes (swagger, `POST
/// /api/cdn/estimate/`: `maximum` 10^15).
const MAX_ESTIMATE_COUNT: u64 = 1_000_000_000_000_000;

/// The most GB one region of an estimate takes (swagger, `POST /api/cdn/estimate/`: "0 to
/// 10^9 each").
const MAX_ESTIMATE_GB: f64 = 1_000_000_000.0;

/// One `--traffic REGION=GB`: a region code and a number of GB, kept as the JSON number the
/// caller wrote so it is sent without rounding.
#[derive(Debug, Clone)]
pub(crate) struct RegionGb {
    pub(crate) region: String,
    pub(crate) gb: serde_json::Number,
}

/// `REGION=GB`: a two-letter region code (the API's codes are ISO country codes, such as
/// `FR` and `AU`) and a number from 0 to [`MAX_ESTIMATE_GB`]. The range is compared as an
/// `f64`, so a value a few billionths over the top can pass and is left to the server.
fn region_gb(value: &str) -> Result<RegionGb, String> {
    let refused = || {
        "a `--traffic` value is `REGION=GB` with a number from 0 to 1000000000, such as \
         `FR=100`"
    };
    let (region, text) = value.split_once('=').ok_or_else(refused)?;
    let gb: serde_json::Number = text.parse().map_err(|_| refused())?;
    let valid = region.len() == 2
        && region.bytes().all(|b| b.is_ascii_alphabetic())
        && !text.starts_with('-')
        && gb.as_f64().is_some_and(|gb| gb <= MAX_ESTIMATE_GB);
    if valid {
        Ok(RegionGb {
            region: region.to_owned(),
            gb,
        })
    } else {
        Err(refused().to_owned())
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum CdnZones {
    /// The zones of the account, and the server's beta flag (beta)
    #[command(after_help = examples(&[
        "hipc cdn zones list",
    ]))]
    List,
    /// One zone, with its hostnames, cap, and origin
    #[command(after_help = examples(&[
        "hipc cdn zones get zab12cd34ef56",
    ]))]
    Get {
        /// The zone id from hipc cdn zones list
        #[arg(value_parser = zone_id)]
        zone_id: String,
    },
    /// Create a zone serving a bucket or a prefix of it; spends money: needs --yes and --public
    ///
    /// A zone makes a private bucket public: every object under the bucket and prefix becomes
    /// readable by anyone through the zone's hostnames, though it stays private on the S3 endpoint.
    /// Use a prefix that holds only what should be published. A new zone answers 421 for a few
    /// seconds before it serves.
    #[command(after_help = examples(&[
        "hipc cdn zones create --name site --bucket photos --public --yes",
        "hipc cdn zones create --name site --bucket photos --prefix site/ --public --yes",
    ]))]
    Create(ZoneCreate),
    /// Change a zone: only the flags given are sent; needs --yes, and --public with --bucket or
    /// --prefix
    ///
    /// A new --bucket or --prefix purges the whole zone, and publishes that prefix if the bucket is
    /// private, so it needs --public too. --cors-file replaces the whole CORS policy.
    #[command(after_help = examples(&[
        "hipc cdn zones update zab12cd34ef56 --name shop --yes",
        "hipc cdn zones update zab12cd34ef56 --prefix public/ --public --yes",
        "hipc cdn zones update zab12cd34ef56 --cors-file cors.json --yes",
    ]))]
    Update(ZoneUpdate),
    /// Delete a zone; needs --yes
    ///
    /// Its hostnames stop serving and billing stops.
    #[command(after_help = examples(&[
        "hipc cdn zones delete zab12cd34ef56 --yes",
    ]))]
    Delete {
        /// The zone id from hipc cdn zones list
        #[arg(value_parser = zone_id)]
        zone_id: String,
    },
}

#[derive(Debug, Args)]
pub(crate) struct ZoneCreate {
    /// A label for the zone, at most 64 characters
    #[arg(long)]
    pub(crate) name: String,
    /// The account's S3 bucket the zone serves
    #[arg(long)]
    pub(crate) bucket: String,
    #[command(flatten)]
    pub(crate) settings: ZoneSettings,
    /// The monthly spend cap in credits (1 credit = $1), 1 to 100000, with at most 18 digits
    /// after the point
    ///
    /// The server's default is 20.
    #[arg(long, value_name = "CREDITS", value_parser = cdn_cap)]
    pub(crate) monthly_cap: Option<String>,
    /// Publish the bucket and prefix through the zone, even if the bucket is private
    ///
    /// Required: whether the bucket is private is known only after the zone exists. A local
    /// acknowledgement, like --yes: it is not sent, so a dry run's body does not show it.
    #[arg(long)]
    pub(crate) public: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ZoneUpdate {
    /// The zone id from hipc cdn zones list
    #[arg(value_parser = zone_id)]
    pub(crate) zone_id: String,
    /// A new label, at most 64 characters
    #[arg(long)]
    pub(crate) name: Option<String>,
    /// Move the zone to another of the account's buckets
    ///
    /// Purges the whole zone.
    #[arg(long)]
    pub(crate) bucket: Option<String>,
    /// Answer plain HTTP with a redirect to HTTPS
    #[arg(long, value_name = "BOOL")]
    pub(crate) redirect_https: Option<bool>,
    #[command(flatten)]
    pub(crate) settings: ZoneSettings,
    /// Publish the new bucket or prefix through the zone, even if the bucket is private
    ///
    /// Required with --bucket or --prefix: whether the bucket is private is known only after the
    /// update. A local acknowledgement, like --yes: it is not sent, so a dry run's body does not
    /// show it.
    #[arg(long)]
    pub(crate) public: bool,
}

/// The zone fields `create` and `update` both take, each sent only when given.
#[derive(Debug, Args)]
pub(crate) struct ZoneSettings {
    /// The key prefix served at the zone's root: empty for the whole bucket, or a folder ending in
    /// /, such as site/
    #[arg(long, value_parser = zone_prefix)]
    pub(crate) prefix: Option<String>,
    /// The region whose nodes fetch from the bucket, a code from hipc cdn regions
    #[arg(long, value_name = "REGION")]
    pub(crate) shield_region: Option<String>,
    /// The file served for / and every path ending in /, such as index.html
    ///
    /// Empty for none.
    #[arg(long, value_name = "KEY")]
    pub(crate) index_document: Option<String>,
    /// Serve the index document with 200 for an object that does not exist, for a single-page app;
    /// needs an index document
    #[arg(long, value_name = "BOOL")]
    pub(crate) spa_fallback: Option<bool>,
    /// A path prefix, starting and ending with /, that keeps its 404 under the SPA fallback; repeat
    /// for more, up to 16; the list replaces the zone's
    #[arg(
        long = "spa-fallback-exclude",
        value_name = "PREFIX",
        value_parser = spa_exclude
    )]
    pub(crate) spa_fallback_exclude: Vec<String>,
    /// A file holding the zone's whole CORS policy as a JSON object, or null to remove it; -
    /// for stdin
    ///
    /// It replaces the whole policy: a key the file leaves out takes its default, and an origin
    /// it leaves out loses access. The keys are allow_origins (required), allow_methods,
    /// allow_headers, expose_headers, max_age, and allow_credentials; the server judges them.
    ///
    /// It is read only as a regular file, never through a symlink; /dev/stdin and <(...) are
    /// refused, so pass - for stdin.
    #[arg(long, value_name = "PATH")]
    pub(crate) cors_file: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum CdnCap {
    /// The cap, and what the zone spent this month, in base units and in credits
    #[command(after_help = examples(&[
        "hipc cdn cap get zab12cd34ef56",
    ]))]
    Get {
        /// The zone id from hipc cdn zones list
        #[arg(value_parser = zone_id)]
        zone_id: String,
    },
    /// Set the monthly spend cap; needs --yes when it raises the cap, or when the cap in force
    /// is unknown
    ///
    /// It reads the cap first and compares with its monthly_cap, in base units, or with its
    /// monthly_cap_credits when that is missing: raising the cap lets the zone spend more, so it
    /// is confirmed, and so is any change it cannot compare. Lowering the cap, or keeping it, is
    /// not.
    #[command(after_help = examples(&[
        "hipc cdn cap update zab12cd34ef56 --monthly-cap 10",
        "hipc cdn cap update zab12cd34ef56 --monthly-cap 50 --yes",
    ]))]
    Update {
        /// The zone id from hipc cdn zones list
        #[arg(value_parser = zone_id)]
        zone_id: String,
        /// The monthly spend cap in credits (1 credit = $1), 1 to 100000, with at most 18 digits
        /// after the point
        #[arg(long, value_name = "CREDITS", value_parser = cdn_cap)]
        monthly_cap: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum CdnHostnames {
    /// The zone's hostnames: its default one and the custom ones
    #[command(after_help = examples(&[
        "hipc cdn hostnames list zab12cd34ef56",
    ]))]
    List {
        /// The zone id from hipc cdn zones list
        #[arg(value_parser = zone_id)]
        zone_id: String,
    },
    // The three records, and the DNS only setting at Cloudflare, are those of the docs'
    // custom domain page (https://docs.hippius.com/use/cdn).
    /// Add a custom hostname; spends money: needs --yes
    ///
    /// It is billed monthly, and serves once the DNS records in the answer are published. The
    /// answer holds three: the hostname's CNAME, a _hippius-cdn TXT record, and an
    /// _acme-challenge CNAME for the certificate. hipc cdn hostnames dns-setup writes them at
    /// Cloudflare or Route 53; at Cloudflare both CNAME records must be DNS only, not proxied.
    #[command(after_help = examples(&[
        "hipc cdn hostnames create zab12cd34ef56 cdn.example.com --yes",
    ]))]
    Create {
        /// The zone id from hipc cdn zones list
        #[arg(value_parser = zone_id)]
        zone_id: String,
        /// A domain the account controls, such as cdn.example.com
        #[arg(value_parser = hostname)]
        hostname: String,
    },
    /// Check the hostname's DNS records now, instead of waiting for the next check
    #[command(after_help = examples(&[
        "hipc cdn hostnames verify zab12cd34ef56 cdn.example.com",
    ]))]
    Verify {
        /// The zone id from hipc cdn zones list
        #[arg(value_parser = zone_id)]
        zone_id: String,
        /// The custom hostname
        #[arg(value_parser = hostname)]
        hostname: String,
    },
    /// Write the hostname's DNS records at Cloudflare or Route 53; needs --yes
    ///
    /// The provider credential is read from --credentials-file, a mode-0600 JSON file, and sent
    /// for this request only; it never goes in argv or in any output. Cloudflare takes
    /// {"api_token": "...", "zone_id": "..."}, a token with Zone DNS Edit on the zone (and Zone
    /// Read without zone_id). Route 53 takes {"access_key_id": "...", "secret_access_key":
    /// "...", "session_token": "...", "hosted_zone_id": "..."}, allowed
    /// route53:ListResourceRecordSets and ChangeResourceRecordSets on the hosted zone (and
    /// ListHostedZones without hosted_zone_id). zone_id, session_token, and hosted_zone_id are
    /// optional. A record that conflicts with one the zone has is refused by the server; hipc
    /// does not overwrite it.
    #[command(after_help = examples(&[DNS_SETUP_EXAMPLE]))]
    DnsSetup(DnsSetup),
    /// A Domain Connect link that publishes the hostname's DNS records at its DNS provider
    ///
    /// Open the URL in a browser and approve the records there; hipc does not wait. The
    /// hostname serves once its records are published: hipc cdn hostnames list shows its
    /// status. The server answers 404 when the provider has no Domain Connect template.
    #[command(after_help = examples(&[
        "hipc cdn hostnames domain-connect zab12cd34ef56 cdn.example.com",
    ]))]
    DomainConnect {
        /// The zone id from hipc cdn zones list
        #[arg(value_parser = zone_id)]
        zone_id: String,
        /// The custom hostname
        #[arg(value_parser = hostname)]
        hostname: String,
    },
    /// Remove a custom hostname; needs --yes
    ///
    /// It stops serving and billing.
    #[command(after_help = examples(&[
        "hipc cdn hostnames delete zab12cd34ef56 cdn.example.com --yes",
    ]))]
    Delete {
        /// The zone id from hipc cdn zones list
        #[arg(value_parser = zone_id)]
        zone_id: String,
        /// The custom hostname
        #[arg(value_parser = hostname)]
        hostname: String,
    },
}

const DNS_SETUP_EXAMPLE: &str = concat!(
    "hipc cdn hostnames dns-setup zab12cd34ef56 cdn.example.com --provider cloudflare ",
    "--credentials-file cloudflare.json --yes"
);

#[derive(Debug, Args)]
pub(crate) struct DnsSetup {
    /// The zone id from hipc cdn zones list
    #[arg(value_parser = zone_id)]
    pub(crate) zone_id: String,
    /// The custom hostname
    #[arg(value_parser = hostname)]
    pub(crate) hostname: String,
    /// The DNS provider that serves the hostname's domain
    #[arg(long, value_enum)]
    pub(crate) provider: DnsProviderArg,
    /// The mode-0600 JSON file holding the provider credential
    ///
    /// It is held to the config file's checks: a regular file of the current user with no bits
    /// for the group or others, not a symlink, at most 1 MiB. There is no stdin form.
    #[arg(long, value_name = "PATH")]
    pub(crate) credentials_file: PathBuf,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum DnsProviderArg {
    Cloudflare,
    Route53,
}

impl DnsProviderArg {
    pub(crate) fn provider(self) -> hippius::DnsProvider {
        match self {
            Self::Cloudflare => hippius::DnsProvider::Cloudflare,
            Self::Route53 => hippius::DnsProvider::Route53,
        }
    }
}

#[derive(Debug, Args)]
#[command(group(
    clap::ArgGroup::new("target").required(true).args(["everything", "paths", "prefixes"])
))]
pub(crate) struct CdnPurge {
    /// The zone id from hipc cdn zones list
    #[arg(value_parser = zone_id)]
    pub(crate) zone_id: String,
    /// Purge everything the zone has cached
    ///
    /// Not --all, which means "follow every page" on every list command.
    #[arg(long)]
    pub(crate) everything: bool,
    /// An exact path to purge, starting with / and percent-encoded; repeat for more, up to 1000
    #[arg(long = "path", value_name = "PATH", value_parser = purge_path)]
    pub(crate) paths: Vec<String>,
    /// A prefix to purge every path under, starting and ending with / and percent-encoded;
    /// repeat for more, up to 1000
    #[arg(long = "prefix", value_name = "PREFIX", value_parser = purge_prefix)]
    pub(crate) prefixes: Vec<String>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum CdnPurges {
    /// Drop cached copies so the next request fetches from the bucket again
    ///
    /// Give exactly one of --everything, --path, or --prefix.
    #[command(after_help = examples(&[
        "hipc cdn purges create zab12cd34ef56 --everything",
        "hipc cdn purges create zab12cd34ef56 --path /index.html --path /app.js",
        "hipc cdn purges create zab12cd34ef56 --prefix /img/",
    ]))]
    Create(CdnPurge),
    /// The zone's purges
    #[command(after_help = examples(&[
        "hipc cdn purges list zab12cd34ef56",
    ]))]
    List {
        /// The zone id from hipc cdn zones list
        #[arg(value_parser = zone_id)]
        zone_id: String,
    },
    /// One purge: pending until every node has applied it, then done
    #[command(after_help = examples(&[
        "hipc cdn purges get zab12cd34ef56 9",
    ]))]
    Get {
        /// The zone id from hipc cdn zones list
        #[arg(value_parser = zone_id)]
        zone_id: String,
        /// The purge id that hipc cdn purges create printed
        #[arg(value_parser = number(PURGE_ID))]
        purge_id: u64,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum CdnRules {
    /// The zone's cache rules
    #[command(after_help = examples(&[
        "hipc cdn rules get zab12cd34ef56",
    ]))]
    Get {
        /// The zone id from hipc cdn zones list
        #[arg(value_parser = zone_id)]
        zone_id: String,
    },
    /// Replace the whole rule list with a file's; needs --yes, and --remove-others to drop or
    /// move a rule
    ///
    /// A rule the zone has now that the file leaves out or moves is refused unless
    /// --remove-others is passed.
    #[command(after_help = examples(&[
        "hipc cdn rules update zab12cd34ef56 --rules-file cdn-rules.json --yes",
        "hipc cdn rules update zab12cd34ef56 --rules-file cdn-rules.json --remove-others --yes",
    ]))]
    Update {
        /// The zone id from hipc cdn zones list
        #[arg(value_parser = zone_id)]
        zone_id: String,
        /// A JSON file such as {"rules": [...]}, or - for stdin, sent as it is
        ///
        /// {"rules": []} removes every rule. A key the API does not list for a rule, its match,
        /// or its actions is refused.
        ///
        /// It is read only as a regular file, never through a symlink; /dev/stdin and <(...) are
        /// refused, so pass - for stdin.
        #[arg(long, value_name = "PATH")]
        rules_file: PathBuf,
        /// Remove the zone's rules the file leaves out
        ///
        /// Without it the current rules are read first, and the update is refused when the
        /// file leaves out a rule the zone has now or moves it: the first matching rule wins, so a
        /// rule is its place and its match. Its actions may change in place.
        #[arg(long)]
        remove_others: bool,
    },
}

#[derive(Debug, Args)]
pub(crate) struct CdnStats {
    /// The zone id from hipc cdn zones list
    #[arg(value_parser = zone_id)]
    pub(crate) zone_id: String,
    /// The start of the window, as an ISO 8601 date-time
    #[arg(long, value_name = "DATETIME")]
    pub(crate) from: Option<String>,
    /// The end of the window, as an ISO 8601 date-time
    #[arg(long, value_name = "DATETIME")]
    pub(crate) to: Option<String>,
    /// The length of one point of the series
    #[arg(long, value_enum)]
    pub(crate) step: Option<StatsStep>,
    /// One serving region, a code from hipc cdn regions
    ///
    /// Without it, every region.
    #[arg(long, value_name = "REGION")]
    pub(crate) region: Option<String>,
}

/// The step of a stats series, as the console sends it.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum StatsStep {
    #[value(name = "5m")]
    FiveMinutes,
    #[value(name = "1h")]
    Hour,
    #[value(name = "1d")]
    Day,
}

impl StatsStep {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::FiveMinutes => "5m",
            Self::Hour => "1h",
            Self::Day => "1d",
        }
    }
}

/// A zone's origin prefix: empty for the whole bucket, or a folder ending in `/` (swagger,
/// `origin.prefix` of `POST /api/cdn/zones/`). An S3 key does not start with `/`, so a
/// prefix that does serves nothing. Without the trailing `/`, `site` would also publish
/// `sitemap.xml` and `site-private/`, if the server read it as a key prefix.
fn zone_prefix(value: &str) -> Result<String, String> {
    if value.is_empty() || (value.ends_with('/') && !value.starts_with('/')) {
        Ok(value.to_owned())
    } else {
        Err(concat!(
            "a zone prefix is empty, for the whole bucket, or a folder that ends with `/` and ",
            "does not start with `/`, such as `site/`"
        )
        .to_owned())
    }
}

/// A SPA fallback exclusion: a path prefix that starts and ends with `/` (swagger,
/// `spa_fallback_exclude` of `POST /api/cdn/zones/`).
fn spa_exclude(value: &str) -> Result<String, String> {
    if value.starts_with('/') && value.ends_with('/') {
        Ok(value.to_owned())
    } else {
        Err("a SPA fallback exclusion starts and ends with `/`, such as `/assets/`".to_owned())
    }
}

/// The characters a purge path holds as they are, before [`is_percent_encoded`] sees a `%`:
/// RFC 3986 `pchar` and `/`, which is the unreserved set, the sub-delims, `:`, and `@`.
/// The refusals name the common bytes that need `%XX` instead of this list: clap's message
/// would turn its `'` into a backtick.
const PATH_CHARACTERS: &str = "-._~!$&'()*+,;=:@/";

/// Whether `value` is a percent-encoded path: every byte is a letter, a digit, one of
/// [`PATH_CHARACTERS`], or a `%` followed by two hex digits. Swagger says a purge list
/// holds "percent-encoded paths". `hipc` refuses a byte that needs encoding rather than
/// encode it, because whether `%20` in the input is already encoded cannot be told.
fn is_percent_encoded(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        if byte == b'%' {
            let hex = bytes.get(i + 1..i + 3);
            if !hex.is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit)) {
                return false;
            }
            i += 3;
        } else if byte.is_ascii_alphanumeric() || PATH_CHARACTERS.as_bytes().contains(&byte) {
            i += 1;
        } else {
            return false;
        }
    }
    true
}

/// The end of a purge path or prefix refusal.
const PERCENT_ENCODED: &str = "is percent-encoded, such as `/img/a%20b.jpg`: a space, `?`, \
                               `#`, `%` itself, and every byte that is not ASCII are written \
                               as `%XX`";

/// An exact path to purge: it starts with `/` and is percent-encoded (swagger, `POST
/// /api/cdn/zones/{zone_id}/purge/`).
fn purge_path(value: &str) -> Result<String, String> {
    if value.starts_with('/') && is_percent_encoded(value) {
        Ok(value.to_owned())
    } else {
        Err(format!(
            "a purge path starts with `/` and {PERCENT_ENCODED}"
        ))
    }
}

/// A prefix to purge: a path that also ends with `/` (swagger: "a prefix ends in `/`").
fn purge_prefix(value: &str) -> Result<String, String> {
    if value.starts_with('/') && value.ends_with('/') && is_percent_encoded(value) {
        Ok(value.to_owned())
    } else {
        Err(format!(
            "a purge prefix starts and ends with `/` and {PERCENT_ENCODED}"
        ))
    }
}

/// A hostname path segment: a DNS name of at most 253 characters, in labels of 1 to 63
/// letters, digits, and `-` that neither start nor end with `-`. So no `/`, `?`, or `..`
/// can change the route.
fn hostname(value: &str) -> Result<String, String> {
    let label_ok = |label: &str| {
        (1..=63).contains(&label.len())
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    };
    if value.len() <= 253 && value.split('.').all(label_ok) {
        Ok(value.to_owned())
    } else {
        Err(concat!(
            "a hostname is labels of 1 to 63 letters, digits, and `-`, joined by `.`, ",
            "at most 253 characters"
        )
        .to_owned())
    }
}

/// The CDN zone's monthly spend cap: a credit amount from `1` to `100000`, as the flag's help
/// says, sent in its normal form ([`normal_credits`]), as the runner limit's is. Refused here,
/// so a cap of `0` never reaches the question or the server. A credit is `10^18` base units,
/// so a finer cap cannot be held, and `cdn cap update` could not compare it with the cap in
/// force. Trailing zeros are stripped before the decimals are counted, so the server never
/// gets more than 18 written ones.
fn cdn_cap(value: &str) -> Result<String, String> {
    use crate::commands::credits::DECIMALS;

    let credits = normal_credits(&credits(value)?);
    if credits
        .split_once('.')
        .is_some_and(|(_, fraction)| fraction.len() > DECIMALS as usize)
    {
        return Err(format!(
            "a monthly spend cap has at most {DECIMALS} digits after the point, the \
             smallest part of a credit"
        ));
    }
    let at_least = |bound| {
        matches!(
            crate::commands::credits::compare(&credits, bound),
            Some(Ordering::Greater | Ordering::Equal)
        )
    };
    let at_most = |bound| {
        matches!(
            crate::commands::credits::compare(&credits, bound),
            Some(Ordering::Less | Ordering::Equal)
        )
    };
    let in_range = at_least("1") && at_most("100000");
    if in_range {
        Ok(credits)
    } else {
        Err("a monthly spend cap is from 1 to 100000 credits, such as `20` or `20.5`".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use crate::cli::Cli;

    /// The CDN spend cap takes 1 to 100000 credits, as its help says, on both flags that set
    /// it, and refuses the rest at parse time.
    #[test]
    fn the_cdn_cap_is_one_to_one_hundred_thousand_credits() {
        // Each is sent in its normal form: no leading zeros, and no trailing zeros after the
        // point, which are stripped before the decimals are counted, as the runner limit does.
        for (good, sent) in [
            ("1", "1"),
            ("1.0", "1"),
            ("001", "1"),
            ("20.5", "20.5"),
            ("020.50", "20.5"),
            ("99999.999", "99999.999"),
            ("100000", "100000"),
            ("100000.000", "100000"),
            ("1.000000000000000001", "1.000000000000000001"),
            ("99999.999999999999999999", "99999.999999999999999999"),
            ("20.5000000000000000000", "20.5"),
            ("100000.0000000000000000000", "100000"),
            ("1.0000000000000000010", "1.000000000000000001"),
        ] {
            assert_eq!(super::cdn_cap(good), Ok(sent.to_owned()), "{good}");
        }
        let range = "a monthly spend cap is from 1 to 100000 credits, such as `20` or `20.5`";
        for bad in [
            "0", "000", "0.0", "0.5", "0.999999", "100000.1", "100001", "1000000",
        ] {
            assert_eq!(super::cdn_cap(bad), Err(range.to_owned()), "{bad}");
        }
        let decimals = "a monthly spend cap has at most 18 digits after the point, the smallest \
                        part of a credit";
        for bad in [
            "1.0000000000000000001",
            "20.50000000000000000001",
            "0.0000000000000000001",
        ] {
            assert_eq!(super::cdn_cap(bad), Err(decimals.to_owned()), "{bad}");
        }
        let grammar = "a credit amount is a decimal number, such as `20` or `20.5`";
        for bad in ["", "-1", "1.", ".5", "2e3", "twenty"] {
            assert_eq!(super::cdn_cap(bad), Err(grammar.to_owned()), "{bad}");
        }
        for command in [
            &["cdn", "cap", "update", "zab12cd34ef56", "--monthly-cap"][..],
            &[
                "cdn",
                "zones",
                "create",
                "--name",
                "a",
                "--bucket",
                "b",
                "--public",
                "--monthly-cap",
            ],
        ] {
            let parse = |value| Cli::try_parse_from(["hipc"].iter().chain(command).chain([&value]));
            assert!(parse("100000").is_ok(), "{command:?}");
            let error = parse("0").expect_err("a cap of 0").to_string();
            assert!(error.contains(range), "{command:?}: {error}");
        }
    }
}
