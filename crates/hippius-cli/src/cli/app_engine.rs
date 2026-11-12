//! The grammar of `hipc compute app-engine`: App Engine, which runs one docker compose file
//! on a VM booted from Hippius's sealed app image. The reads, the compose check and fix, and
//! the launch.
//!
//! The group is `app-engine`, not `apps`: `hipc compute apps` reads the one-click app catalog.
//! An app is a VM, so its id is the `vm_id` the VM, backup, and firewall commands take.

use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::cli::{AppTarget, LaunchFlags, examples, flavor, new_file, region};

#[derive(Debug, Subcommand)]
pub(crate) enum AppEngine {
    /// Whether the account may use App Engine, and its limits: the largest compose, the
    /// published ports, the services, and the smallest flavor
    ///
    /// GET /api/compute/app-engine/info/ answers every member. enabled false, with reason,
    /// means the account may not use App Engine, and every other app-engine command answers
    /// permission; it still exits 0.
    #[command(after_help = examples(&["hipc compute app-engine get"]))]
    Get,
    /// Check a compose file against the App Engine grammar, launching nothing
    ///
    /// The file is sent byte for byte, and the answer holds what the server read from it: its
    /// compose_sha256, the services, the published ports, and the secret names it declares.
    /// hipc checks that compose_sha256 is the SHA-256 of the file it read, and an answer without
    /// it is internal. A refused compose exits 1 validation, with every issue in
    /// error.details.issues; text mode prints each one on an error line with its line and
    /// column. No secret value is sent.
    #[command(after_help = examples(&[
        "hipc compute app-engine validate --compose-file compose.yaml",
        "hipc compute app-engine validate --compose-file -",
    ]))]
    Validate {
        #[command(flatten)]
        compose: ComposeFile,
    },
    /// Rewrite a compose file into the App Engine grammar; saves nothing on the server
    ///
    /// stdout is the server's answer: fixed_compose, every change it made, what it could not
    /// fix (unresolved), and the image digests it looked up. valid false still exits 0. A change
    /// with semantic true alters what runs, and text mode warns about each one. hipc checks that
    /// compose_sha256 is the SHA-256 of fixed_compose; an answer that fails it is internal, and
    /// nothing is written. With --output-file, fixed_compose is also written byte for byte to a
    /// new file of mode 0600; hipc never overwrites a file, the compose file included.
    #[command(after_help = examples(&[
        "hipc compute app-engine fix --compose-file compose.yaml",
        "hipc compute app-engine fix --compose-file compose.yaml --output-file fixed.yaml",
    ]))]
    Fix {
        #[command(flatten)]
        compose: ComposeFile,
        /// The file to create for fixed_compose; it must not exist yet
        #[arg(long, value_name = "PATH", value_parser = new_compose_file)]
        output_file: Option<PathBuf>,
    },
    /// The account's apps: each is a VM, so delete, power, resize, and backups go through hipc
    /// compute vms and hipc compute backups with its vm_id
    ///
    /// App Engine proves which compose a VM runs, not that its data is hidden: in this beta,
    /// Hippius operators can read an app's secrets and volumes.
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute app-engine apps list",
        "hipc compute app-engine apps get vm-1a2b",
    ]))]
    Apps(AppEngineApps),
    /// The guest components release an app's VM runs, the newest one offered, and the latest
    /// upgrade onto one
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute app-engine guest-components get vm-1a2b",
        "hipc compute app-engine guest-components update-status vm-1a2b",
    ]))]
    GuestComponents(GuestComponents),
}

#[derive(Debug, Subcommand)]
pub(crate) enum AppEngineApps {
    /// Every app in the account
    #[command(after_help = examples(&["hipc compute app-engine apps list"]))]
    List,
    /// One app, with its exact compose and its last 20 revisions, newest first
    ///
    /// evidence.verify_command is a command to check the VM's attestation yourself; hipc prints
    /// it and never runs it. It comes from the API: read it before running it, and never pipe
    /// it to a shell. The answer lists secret_names only: no secret value is ever shown.
    /// The compose is printed whole, inside the JSON document.
    #[command(after_help = examples(&["hipc compute app-engine apps get vm-1a2b"]))]
    Get(AppTarget),
    /// Launch an app: a VM of the flavor with a public IPv4, running the compose file; spends
    /// money: needs --yes
    ///
    /// It is billed per hour from the balance as a VM of its flavor, at its OS disk tier's
    /// price, plus its public IPv4 and any backups; the app is a VM, deleted with hipc compute
    /// vms delete. hipc first checks the compose with POST /api/compute/app-engine/validate/,
    /// which sends no secret, then asks; each secret the compose declares must have a value in
    /// --secrets-file, and the file may give no other. Secret values are write-only: they are
    /// sent once in the launch's body, stored encrypted, given to the VM as
    /// /run/secrets/<NAME>, and never shown again. In this beta, Hippius operators can read
    /// them. A dry run shows a placeholder for each value, --debug prints no body, and every
    /// value is redacted from what the server answers. No public_ip is sent: every app gets one.
    #[command(after_help = examples(&[
        "hipc compute app-engine apps create --flavor small --compose-file compose.yaml --yes",
        "hipc compute app-engine apps create --flavor small --compose-file compose.yaml \
         --secrets-file secrets.json --region FR --yes",
    ]))]
    Create(AppCreate),
    /// What the app's runner reports about itself, relayed from its VM and cached for 15
    /// seconds; not verified
    ///
    /// The VM wrote every value, so verified is always false: the answer is never proof of what
    /// runs. compose_sha256_reported is the hash the runner says it runs. The answer is printed
    /// whole, raw_evidence included; an answer larger than 32 MiB is refused as internal. To
    /// fetch evidence for a nonce you pick, use hipc api GET
    /// /compute/app-engine/apps/<vm_id>/evidence/?nonce=<64 hex>.
    #[command(after_help = examples(&["hipc compute app-engine apps runtime vm-1a2b"]))]
    Runtime(AppTarget),
}

/// The flags of `apps create`. Those it shares with `hipc compute vms create` take the same
/// values; an app takes no image, SSH key, or dedicated disk (the server refuses them with
/// `app-field-not-allowed`: the sealed image has no SSH), and no `--public-ip`, since every
/// app gets one and the server refuses a launch without (`app-public-ip-required`).
/// The largest compose file `hipc` reads, in bytes. The server's limit is lower
/// (`max_compose_bytes` in `hipc compute app-engine get`) and it refuses a larger one itself;
/// this cap, the config file's, only stops a wrong path (a log, an image) from being read whole.
pub(crate) const MAX_COMPOSE_BYTES: u64 = 1024 * 1024;

/// `--compose-file`, which `validate`, `fix`, and `apps create` read the same way, so its help
/// is written once, with the limit [`MAX_COMPOSE_BYTES`] says.
#[derive(Debug, Args)]
pub(crate) struct ComposeFile {
    #[arg(
        id = "compose_file",
        long = "compose-file",
        value_name = "PATH",
        help = compose_help(),
        long_help = format!(
            "{}\n\nThe server takes a smaller file: at most max_compose_bytes from hipc compute \
             app-engine get. /dev/stdin and <(...) are refused, as they are not regular files, \
             so pass - for stdin.",
            compose_help()
        )
    )]
    pub(crate) path: PathBuf,
}

/// The short help of `--compose-file`.
fn compose_help() -> String {
    format!(
        "The compose file, a regular file (not a symlink) of UTF-8 text of at most {} MiB, sent \
         byte for byte; - reads stdin",
        MAX_COMPOSE_BYTES / (1024 * 1024)
    )
}

#[derive(Debug, Args)]
pub(crate) struct AppCreate {
    /// The flavor code from hipc compute flavors; hipc compute app-engine get names the
    /// smallest an app takes (min_flavor)
    #[arg(long, value_parser = flavor)]
    pub(crate) flavor: String,
    #[command(flatten)]
    pub(crate) compose: ComposeFile,
    /// A JSON object of each secret's name to its value, which the compose declares in
    /// x-hippius.secrets
    ///
    /// The file must be the current user's, of mode 0600, not a symlink, at most 1 MiB, in a
    /// directory no one else can write; there is no stdin form. Each value is sent exactly as
    /// the file holds it: nothing is trimmed. Values are write-only, and in this beta Hippius
    /// operators can read them.
    #[arg(long, value_name = "PATH", value_parser = secrets_file)]
    pub(crate) secrets_file: Option<PathBuf>,
    /// A label
    ///
    /// The server defaults it to the VM id.
    #[arg(long)]
    pub(crate) name: Option<String>,
    /// The region: a two-letter country code from hipc compute regions, such as FR
    ///
    /// Without it the scheduler chooses.
    #[arg(long, value_name = "CC", value_parser = region)]
    pub(crate) region: Option<String>,
    #[command(flatten)]
    pub(crate) launch: LaunchFlags,
}

/// The file `--secrets-file` names. `-` is refused with its own reason: stdin cannot be held
/// to the checks of a private file.
fn secrets_file(text: &str) -> Result<PathBuf, String> {
    if text == "-" {
        return Err(
            "the secrets are read only from a private file, never from stdin: name a file of \
             mode 0600"
                .to_owned(),
        );
    }
    Ok(PathBuf::from(text))
}

#[derive(Debug, Subcommand)]
pub(crate) enum GuestComponents {
    /// The guest components release the app's VM runs, the newest one, and whether an upgrade
    /// is available
    ///
    /// release is null on the bare base. needs_operator true means an earlier upgrade failed
    /// and Hippius must recover the VM.
    #[command(after_help = examples(&["hipc compute app-engine guest-components get vm-1a2b"]))]
    Get(AppTarget),
    /// The app's latest guest components upgrade: its release, state, and outcome
    ///
    /// job is null when the app was never upgraded.
    #[command(after_help = examples(&[
        "hipc compute app-engine guest-components update-status vm-1a2b",
    ]))]
    UpdateStatus(AppTarget),
}

/// The file `fix --output-file` writes `fixed_compose` to ([`new_file`]). `-` is refused
/// with its own reason: stdout already holds the whole answer, `fixed_compose` included.
fn new_compose_file(text: &str) -> Result<PathBuf, String> {
    if text == "-" {
        return Err(
            "stdout already holds the answer, `fixed_compose` included: leave the flag out, or \
             name a file"
                .to_owned(),
        );
    }
    new_file(text, "the fixed compose")
}
