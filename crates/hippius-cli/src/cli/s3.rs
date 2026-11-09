//! The grammar of `hipc s3`.

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

use crate::cli::{ListArgs, PayFrom, SecretFile, examples, history_limit, intent_id};

#[derive(Debug, Subcommand)]
pub(crate) enum S3 {
    /// The S3 plan catalog and the pay-as-you-go rate; needs no token
    #[command(after_help = examples(&[
        "hipc s3 plans",
    ]))]
    Plans,
    /// Read, buy, change, or cancel the S3 plan, and read its history
    ///
    /// Without a plan the account is on pay as you go.
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc s3 subscription get",
        "hipc s3 subscription subscribe --plan pro --yes",
    ]))]
    Subscription(S3Subscription),
    /// The account's buckets: list, create, and delete them, list and presign their objects, and
    /// read and replace their policies
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc s3 buckets list",
        "hipc s3 buckets create photos --yes",
    ]))]
    Buckets(Buckets),
    /// List, create, revoke, and rotate S3 tokens, the key pairs s3.hippius.com accepts
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc s3 tokens list",
        "hipc s3 tokens create --kind master --reveal --yes",
    ]))]
    Tokens(Tokens),
    /// Write an AWS CLI profile for s3.hippius.com; does not touch the network; needs --yes to
    /// overwrite a profile
    ///
    /// Writes endpoint_url, region = decentralized, and path-style addressing to ~/.aws/config, and
    /// the key pair to ~/.aws/credentials (mode 0600 when new; a file that exists keeps its mode).
    /// Every other profile in both files is kept as it was. Needs --yes when the profile exists.
    ///
    /// For a new token, write its secret to a private file and pass that file with the token's
    /// access_key_id: hipc s3 tokens create --kind sub --scope-type single-bucket --bucket photos
    /// --action read --action write --secret-file s3.secret --yes. Or save its whole
    /// JSON to a file only the user can read, and pass that file: (umask 077; hipc --json s3
    /// tokens create --kind sub --bucket photos --action read --action write --reveal --yes >
    /// token.json). Delete the file once the profile is written. A sub token reaches only the
    /// buckets it names; a master token reaches every bucket and bypasses their policies, so
    /// give an app a sub token.
    #[command(after_help = examples(&[
        "hipc s3 configure --from-token-file token.json",
        "hipc s3 configure --access-key-id hip_A1B2 --secret-key-file secret.txt",
        "hipc s3 configure --aws-profile work --access-key-id hip_A1B2 --secret-key-file secret.txt",
    ]))]
    Configure(Configure),
}

#[derive(Debug, Subcommand)]
pub(crate) enum S3Subscription {
    /// The S3 plan, or the pay-as-you-go usage without a plan
    #[command(after_help = examples(&[
        "hipc s3 subscription get",
    ]))]
    Get,
    /// Buy an S3 plan: a flat monthly price for a block of storage; spends money: needs --yes
    ///
    /// A plan is not a ceiling on the bill: storage above the plan's block is billed hourly at
    /// the plan's own rate, up to the cap hipc s3 subscription change --storage-cap sets. Below
    /// the block, pay as you go can cost less; hipc s3 plans lists the plans and their prices.
    /// An account that has a plan switches to another with hipc s3 subscription change --plan.
    #[command(after_help = examples(&[
        "hipc s3 subscription subscribe --plan pro --yes",
        "hipc s3 subscription subscribe --plan business --pay-from card --yes",
    ]))]
    Subscribe(S3Subscribe),
    /// Switch the S3 plan, how its renewals are paid, or its overage cap; spends money: needs
    /// --yes
    ///
    /// Takes one of --plan, --pay-from, and --storage-cap. A downgrade below current usage
    /// pauses uploads.
    #[command(after_help = examples(&[
        "hipc s3 subscription change --plan business --yes",
        "hipc s3 subscription change --pay-from card --yes",
        "hipc s3 subscription change --storage-cap 2000000000000 --yes",
    ]))]
    Change(S3Change),
    /// Cancel the S3 plan; needs --yes
    ///
    /// The plan stays active until the end of the month paid for, and nothing is refunded. Then
    /// the account is on pay as you go.
    #[command(after_help = examples(&[
        "hipc s3 subscription cancel --yes",
    ]))]
    Cancel,
    /// The progress of a card checkout: pending, paid, fulfilling, then fulfilled, or failed
    ///
    /// The plan is active only at fulfilled.
    #[command(after_help = examples(&[
        "hipc s3 subscription checkout pi_3Nabc",
    ]))]
    Checkout {
        /// The intent_id the card subscribe printed
        #[arg(value_parser = intent_id)]
        intent_id: String,
    },
    /// Every event of the S3 plan, newest first: purchases, renewals, changes, cancels, and
    /// refunds, one page at a time
    #[command(mut_arg("limit", history_limit))]
    #[command(after_help = examples(&[
        "hipc s3 subscription history",
        "hipc s3 subscription history --all",
    ]))]
    History(ListArgs),
}

#[derive(Debug, Args)]
pub(crate) struct S3Subscribe {
    /// The plan code: pro, business, or enterprise
    #[arg(long, value_enum)]
    pub(crate) plan: S3Plan,
    /// Pay from the balance (credits), or start a card checkout (card)
    #[arg(long, value_enum, default_value_t = PayFrom::Credits)]
    pub(crate) pay_from: PayFrom,
}

/// Swagger `s3_subscription_partial_update` separates a plan change from a funding switch
/// ("Either a plan change (`plan`) or a funding switch (`funding`)"); whether `funding` and
/// `storage_cap_bytes` may share one PATCH is not documented, so one kind of change is taken
/// at a time.
#[derive(Debug, Args)]
#[command(group(
    clap::ArgGroup::new("change").required(true).args(["plan", "pay_from", "storage_cap"])
))]
pub(crate) struct S3Change {
    /// The plan code to switch to
    #[arg(long, value_enum)]
    pub(crate) plan: Option<S3Plan>,
    #[command(flatten)]
    pub(crate) setting: PlanSetting,
}

/// The two changes to a Drive or S3 plan that keep the plan: how renewals are paid, and the
/// overage cap. Each is a PATCH of its own, without `plan`.
#[derive(Debug, Args)]
pub(crate) struct PlanSetting {
    /// How each renewal is paid; keeps the plan
    #[arg(long, value_enum)]
    pub(crate) pay_from: Option<Funding>,
    /// The cap on stored bytes of a plan billed past its quota, or default; keeps the plan
    ///
    /// Storage past the quota is billed, up to this cap. The server refuses a cap below the
    /// plan's storage. default is twice the plan's storage.
    #[arg(long, value_name = "BYTES", value_parser = storage_cap)]
    pub(crate) storage_cap: Option<StorageCap>,
}

/// How the renewals of a Drive or S3 plan are paid (swagger `funding`).
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum Funding {
    /// Each renewal is paid from the balance
    Credits,
    /// Each renewal is charged to the card saved at Stripe; the server refuses it without one
    Card,
}

impl Funding {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Credits => "credits",
            Self::Card => "card",
        }
    }
}

/// `--storage-cap`: a number of bytes, or the server's default, which is sent as
/// `null`.
#[derive(Debug, Clone, Copy)]
pub(crate) enum StorageCap {
    Bytes(u64),
    Default,
}

/// The largest cap swagger allows (`storage_cap_bytes`: `maximum` 9223372036854775807, the
/// largest signed 64-bit integer). Zero is left out: Drive reads it as the default, which
/// `default` already names.
const MAX_STORAGE_CAP: u64 = i64::MAX.unsigned_abs();

fn storage_cap(text: &str) -> Result<StorageCap, String> {
    if text == "default" {
        return Ok(StorageCap::Default);
    }
    match text.parse::<u64>() {
        Ok(bytes) if (1..=MAX_STORAGE_CAP).contains(&bytes) => Ok(StorageCap::Bytes(bytes)),
        _ => Err(format!(
            "a cap is a whole number of bytes from 1 to {MAX_STORAGE_CAP}, or default"
        )),
    }
}

/// The S3 plan codes. Pay as you go is the absence of a plan.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum S3Plan {
    Pro,
    Business,
    Enterprise,
}

impl S3Plan {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Pro => "pro",
            Self::Business => "business",
            Self::Enterprise => "enterprise",
        }
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum Buckets {
    /// Every bucket in the account
    #[command(after_help = examples(&[
        "hipc s3 buckets list",
    ]))]
    List,
    /// Read and replace the buckets' S3 bucket policies, and list the sub-token grants
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc s3 buckets acl list",
        "hipc s3 buckets acl get photos",
    ]))]
    Acl(BucketAcl),
    /// A bucket's objects: list them, and presign a download URL; moves no bytes
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc s3 buckets objects list photos",
    ]))]
    Objects(BucketObjects),
    /// Create a bucket; needs --yes
    #[command(after_help = examples(&[
        "hipc s3 buckets create photos --yes",
        "hipc s3 buckets create photos --region EU --yes",
    ]))]
    Create {
        /// The bucket name: 3 to 63 lowercase letters, digits, ., and -
        bucket_name: String,
        /// The S3 region, sent as given; not a compute country code
        ///
        /// Without it the server chooses. The API documents no list of codes, only the default,
        /// auto.
        #[arg(long)]
        region: Option<String>,
    },
    /// Delete an empty bucket; needs --yes
    #[command(after_help = examples(&[
        "hipc s3 buckets delete photos --yes",
    ]))]
    Delete {
        /// The bucket name
        bucket_name: String,
    },
}

/// The longest a presigned URL works: 7 days, the most an S3 SigV4 presigned URL allows.
const MAX_PRESIGN_SECONDS: i64 = 7 * 24 * 60 * 60;

#[derive(Debug, Args)]
pub(crate) struct Presign {
    /// The bucket name
    pub(crate) bucket_name: String,
    /// The object key, such as 2026/cat.jpg
    pub(crate) key: String,
    /// How long the URL works, in seconds, 1 to 604800 (7 days); the server's default is 3600
    #[arg(
        long,
        value_name = "SECONDS",
        value_parser = clap::value_parser!(u32).range(1..=MAX_PRESIGN_SECONDS)
    )]
    pub(crate) expires_in: Option<u32>,
    /// Print the URL, which grants access to the object; required: without it nothing is sent
    #[arg(long)]
    pub(crate) reveal: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum BucketObjects {
    /// A bucket's objects, with their keys, sizes, and dates, one page at a time
    ///
    /// The listing moves no bytes. Copy an object with the AWS CLI and the profile hipc s3
    /// configure writes: aws --profile hippius s3 cp s3://photos/<key> .
    #[command(after_help = examples(&[
        "hipc s3 buckets objects list photos",
        "hipc s3 buckets objects list photos --prefix 2026/ --limit 50",
        "hipc s3 buckets objects list photos --all",
    ]))]
    List(ObjectsList),
    /// A presigned URL that downloads one object; needs --reveal
    ///
    /// Anyone holding the URL can read the object until it expires, with no other credential,
    /// so it is a secret: without --reveal nothing is sent, and it goes only to stdout. hipc
    /// moves no bytes; fetch the URL with curl or a browser.
    #[command(after_help = examples(&[
        "hipc s3 buckets objects presign photos 2026/cat.jpg --reveal",
        "hipc s3 buckets objects presign photos report.pdf --expires-in 600 --reveal",
    ]))]
    Presign(Presign),
}

/// `--limit`, `--page-token`, and `--all` on a bucket's objects (tenet 6). The route pages as
/// S3 does, by `max_keys` and the server's continuation token, at most 1000 keys a page.
#[derive(Debug, Args)]
pub(crate) struct ObjectsList {
    /// The bucket name
    pub(crate) bucket_name: String,
    /// Only the keys that start with this text, such as 2026/
    #[arg(long, value_name = "TEXT")]
    pub(crate) prefix: Option<String>,
    /// Objects per page, 1 to 1000
    ///
    /// S3 answers at most 1000 keys a page.
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=1000))]
    pub(crate) limit: u32,
    /// The page to fetch, from a previous answer's next_page_token
    #[arg(long, value_name = "TOKEN", conflicts_with = "all", value_parser = continuation_token)]
    pub(crate) page_token: Option<String>,
    /// Follow every page: one JSON array, or one table in text mode
    #[arg(long)]
    pub(crate) all: bool,
}

/// The longest continuation token `--page-token` takes, in bytes. Arbitrary: an S3 token is
/// a few hundred bytes of base64, and 16 KiB leaves room for a longer one while keeping a
/// wrong value out of the URL.
pub(crate) const MAX_CONTINUATION_TOKEN_BYTES: usize = 16 * 1024;

/// An S3 continuation token is the server's opaque cursor, so only what can never be one is
/// refused before sending: an empty token, which S3 would read as the first page, a space or
/// a control character, and one longer than [`MAX_CONTINUATION_TOKEN_BYTES`]. The answer's
/// token is held to the same rule, so every `next_page_token` printed is one this takes.
pub(crate) fn continuation_token(text: &str) -> Result<String, String> {
    let valid = !text.is_empty()
        && text.len() <= MAX_CONTINUATION_TOKEN_BYTES
        && !text.chars().any(|c| c.is_whitespace() || c.is_control());
    if valid {
        Ok(text.to_owned())
    } else {
        Err("use a `next_page_token` from an earlier answer: a continuation token".to_owned())
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum BucketAcl {
    /// Every bucket, with the sub-token grants (actions and prefixes) that apply to it
    #[command(after_help = examples(&[
        "hipc s3 buckets acl list",
    ]))]
    List,
    /// A bucket's S3 bucket policy
    #[command(after_help = examples(&[
        "hipc s3 buckets acl get photos",
    ]))]
    Get {
        /// The bucket name
        bucket_name: String,
    },
    /// Replace a bucket's whole S3 bucket policy; needs --yes, --public, and --remove-others
    ///
    /// A policy can make the bucket readable by anyone (a statement whose Principal is *), and
    /// it replaces the whole policy, so every statement the file leaves out is removed. hipc does
    /// not judge the policy, so both flags are always required, as hipc api requires them on the
    /// same route.
    #[command(after_help = examples(&[
        "hipc s3 buckets acl update photos --policy-file policy.json --public --remove-others --yes",
    ]))]
    Update(AclUpdate),
}

#[derive(Debug, Args)]
pub(crate) struct AclUpdate {
    /// The bucket name
    pub(crate) bucket_name: String,
    /// The file holding the new policy as a JSON object, or - for stdin
    ///
    /// The policy alone, as the policy field of hipc s3 buckets acl get prints it.
    ///
    /// It is read only as a regular file, never through a symlink; /dev/stdin and <(...) are
    /// refused, so pass - for stdin.
    #[arg(long, value_name = "PATH")]
    pub(crate) policy_file: PathBuf,
    /// Let the policy make the bucket readable by anyone; required
    #[arg(long)]
    pub(crate) public: bool,
    /// Let the policy remove the statements the bucket has now; required
    #[arg(long)]
    pub(crate) remove_others: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Tokens {
    /// Every S3 token, master and sub
    ///
    /// The master and the sub tokens are two lists, read whole and printed as one: every page
    /// of each is followed, up to the caps --all has, so this list takes no page flags, and
    /// next_page_token is null.
    /// Secrets are never shown again; secret_preview is the last four characters.
    #[command(after_help = examples(&[
        "hipc s3 tokens list",
    ]))]
    List,
    /// Create a token; needs --yes, and --reveal or --secret-file
    ///
    /// The secret is shown once, so --reveal, which prints it on stdout, or --secret-file, which
    /// writes it only to a new private file, is required before anything is sent. The file keeps
    /// the secret out of a log or an agent's transcript. A sub token reaches only the buckets it
    /// names; give an app one rather than a master token, which reaches every bucket and
    /// bypasses their policies.
    #[command(after_help = examples(&[
        "hipc s3 tokens create --kind master --reveal --yes",
        "hipc s3 tokens create --kind sub --bucket photos --action read --reveal --yes",
        "hipc s3 tokens create --kind sub --bucket photos --action read --action write --secret-file s3.secret --yes",
    ]))]
    Create(TokenCreate),
    /// Revoke a token; needs --yes
    #[command(after_help = examples(&[
        "hipc s3 tokens revoke --kind master 0f8fad5b-d9cb-469f-a165-70867728950e --yes",
    ]))]
    Revoke(TokenTarget),
    /// Replace a token with a new one and revoke the old one; needs --yes, and --reveal or
    /// --secret-file
    ///
    /// The new secret is shown once, so --reveal, which prints it on stdout, or --secret-file,
    /// which writes it only to a new private file, is required.
    #[command(after_help = examples(&[
        "hipc s3 tokens rotate --kind master 0f8fad5b-d9cb-469f-a165-70867728950e --reveal --yes",
        "hipc s3 tokens rotate --kind sub 0f8fad5b-d9cb-469f-a165-70867728950e --secret-file s3-new.secret --yes",
    ]))]
    Rotate(TokenRotate),
}

/// Master tokens reach every bucket. Sub tokens are scoped to buckets, actions, and prefixes.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum TokenKind {
    Master,
    Sub,
}

impl TokenKind {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Master => "master",
            Self::Sub => "sub",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum ScopeType {
    SingleBucket,
    MultiBucket,
    Global,
}

impl ScopeType {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::SingleBucket => "single_bucket",
            Self::MultiBucket => "multi_bucket",
            Self::Global => "global",
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct TokenCreate {
    /// A master token or a sub token
    #[arg(long, value_enum)]
    pub(crate) kind: TokenKind,
    /// Print the one-time secret on stdout; without it or --secret-file nothing is sent
    #[arg(long)]
    pub(crate) reveal: bool,
    #[command(flatten)]
    pub(crate) secret_file: SecretFile,
    /// A label for the token
    #[arg(long)]
    pub(crate) name: Option<String>,
    /// When the token stops working, as an ISO 8601 date-time
    #[arg(long, value_name = "DATETIME")]
    pub(crate) expires_at: Option<String>,
    /// Sub tokens: which buckets the token reaches
    ///
    /// The server's default is one bucket.
    #[arg(long, value_enum)]
    pub(crate) scope_type: Option<ScopeType>,
    /// Sub tokens: a bucket the token reaches; repeat for more
    #[arg(long = "bucket", value_name = "BUCKET")]
    pub(crate) buckets: Vec<String>,
    /// Sub tokens: what the token may do, read or write; repeat for more
    ///
    /// The docs list read and write, the console's Object Read Only and Object Read & Write
    /// (https://docs.hippius.com/storage/s3/advanced#give-an-app-scoped-credentials): pass
    /// --action read --action write for both. hipc sends the value as given, and the server
    /// judges it.
    #[arg(long = "action", value_name = "ACTION")]
    pub(crate) actions: Vec<String>,
    /// Sub tokens: an object key prefix the token is limited to; repeat for more
    #[arg(long = "prefix", value_name = "PREFIX")]
    pub(crate) prefixes: Vec<String>,
    /// Sub tokens: an IP address or CIDR the token may be used from; repeat for more
    #[arg(long = "ip", value_name = "CIDR")]
    pub(crate) ips: Vec<String>,
    /// Sub tokens: when the token starts working, as an ISO 8601 date-time
    #[arg(long, value_name = "DATETIME")]
    pub(crate) not_before: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct TokenTarget {
    /// The collection the token is in
    #[arg(long, value_enum)]
    pub(crate) kind: TokenKind,
    /// The token's id (a UUID), from hipc s3 tokens list
    pub(crate) token_id: String,
}

#[derive(Debug, Args)]
pub(crate) struct TokenRotate {
    #[command(flatten)]
    pub(crate) target: TokenTarget,
    /// Print the new one-time secret on stdout; without it or --secret-file nothing is sent
    #[arg(long)]
    pub(crate) reveal: bool,
    #[command(flatten)]
    pub(crate) secret_file: SecretFile,
}

// The key pair comes from one place: the token document, or the id and the secret file
// together. clap refuses any other mix before anything is read, and `hipc schema` lists the
// group in `required_groups`.
#[derive(Debug, Args)]
#[command(group(
    clap::ArgGroup::new("key_pair")
        .required(true)
        .args(["from_token_file", "access_key_id"])
))]
pub(crate) struct Configure {
    /// The AWS profile to write
    ///
    /// Not --profile, which names the hipc profile.
    #[arg(long, default_value = "hippius", value_name = "NAME")]
    pub(crate) aws_profile: String,
    /// The file holding the JSON that hipc s3 tokens create --reveal or rotate --reveal printed
    ///
    /// Its access_key_id and secret are the key pair, so the secret goes from one hipc to the next
    /// through a file, never through argv or a shell variable. From a create or rotate with
    /// --secret-file, the document has secret_file in place of secret, and the secret is read
    /// from that file, a relative path from the current directory, as the create wrote it, held
    /// to the checks of --secret-key-file. Not with --access-key-id or
    /// --secret-key-file. Not stdin: a secret piped from a create would be lost whenever this
    /// command writes nothing (a dry run, a refusal, a bad document).
    ///
    /// It is read only as a regular file, never through a symlink.
    #[arg(
        long,
        value_name = "PATH",
        conflicts_with = "secret_key_file",
        value_parser = token_json_file
    )]
    pub(crate) from_token_file: Option<PathBuf>,
    /// The access key id, such as hip_A1B2; with --secret-key-file
    #[arg(long, value_name = "ID", requires = "secret_key_file")]
    pub(crate) access_key_id: Option<String>,
    /// The file holding the secret key, or - for stdin; never a flag value, which ps would show
    ///
    /// It is read only as a regular file, never through a symlink; /dev/stdin and <(...) are
    /// refused, so pass - for stdin.
    #[arg(long, value_name = "PATH", requires = "access_key_id")]
    pub(crate) secret_key_file: Option<PathBuf>,
}

/// `--from-token-file`: a path, never `-`. The document holds a one-time secret, and a
/// document read from a pipe is gone when `s3 configure` then writes nothing (a dry run, a
/// profile that exists without `--yes`, a key it refuses), while a file keeps it.
/// Refused here, before stdin is read.
fn token_json_file(value: &str) -> Result<PathBuf, String> {
    if value == "-" {
        Err(
            "`--from-token-file` reads a file, not stdin, so a one-time secret is never lost \
             in a pipe: save the JSON of `hipc --json s3 tokens create --reveal` to a file and \
             pass its path"
                .to_owned(),
        )
    } else {
        Ok(PathBuf::from(value))
    }
}
