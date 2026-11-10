//! The grammar of `hipc compute app-engine`: App Engine, which runs one docker compose file
//! on a VM booted from Hippius's sealed app image. The reads, the compose check and fix, and
//! the launch.
//!
//! The group is `app-engine`, not `apps`: `hipc compute apps` reads the one-click app catalog.
//! An app is a VM, so its id is the `vm_id` the VM, backup, and firewall commands take.

use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::cli::{
    BackupInterval, DiskTier, FailoverMode, GuestPoweroff, examples, flavor, new_file,
    placement_group, region, vm_id,
};

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
        /// The compose file, a regular file (not a symlink) of UTF-8 text of at most 1 MiB, sent
        /// byte for byte; - reads stdin
        ///
        /// The server takes a smaller file: at most max_compose_bytes from hipc compute app-engine
        /// get. /dev/stdin and <(...) are refused, as they are not regular files, so pass - for
        /// stdin.
        #[arg(long, value_name = "PATH")]
        compose_file: PathBuf,
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
        /// The compose file, a regular file (not a symlink) of UTF-8 text of at most 1 MiB, sent
        /// byte for byte; - reads stdin
        ///
        /// The server takes a smaller file: at most max_compose_bytes from hipc compute app-engine
        /// get. /dev/stdin and <(...) are refused, as they are not regular files, so pass - for
        /// stdin.
        #[arg(long, value_name = "PATH")]
        compose_file: PathBuf,
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
    Get {
        /// The vm_id from hipc compute app-engine apps list
        #[arg(value_parser = vm_id)]
        vm_id: String,
    },
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
    Runtime {
        /// The vm_id from hipc compute app-engine apps list
        #[arg(value_parser = vm_id)]
        vm_id: String,
    },
}

/// The flags of `apps create`. Those it shares with `hipc compute vms create` take the same
/// values; an app takes no image, SSH key, or dedicated disk (the server refuses them with
/// `app-field-not-allowed`: the sealed image has no SSH), and no `--public-ip`, since every
/// app gets one and the server refuses a launch without (`app-public-ip-required`).
#[derive(Debug, Args)]
pub(crate) struct AppCreate {
    /// The flavor code from hipc compute flavors; hipc compute app-engine get names the
    /// smallest an app takes (min_flavor)
    #[arg(long, value_parser = flavor)]
    pub(crate) flavor: String,
    /// The compose file, a regular file (not a symlink) of UTF-8 text of at most 1 MiB, sent
    /// byte for byte; - reads stdin
    ///
    /// The server takes a smaller file: at most max_compose_bytes from hipc compute app-engine
    /// get. /dev/stdin and <(...) are refused, as they are not regular files, so pass - for
    /// stdin.
    #[arg(long, value_name = "PATH")]
    pub(crate) compose_file: PathBuf,
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
    /// The OS disk's tier, a minimum: the disk may land on a faster one; the server defaults it
    /// to nvme
    ///
    /// Fixed for the VM's life, and billed at that tier's price, vm_by_os_disk_tier in hipc
    /// compute prices.
    #[arg(long, value_name = "TIER", value_enum)]
    pub(crate) os_disk_tier: Option<DiskTier>,
    /// The anti-affinity group: two VMs of the account in one group never share a server
    ///
    /// A name is 1 to 64 of a-z, 0-9, and -, not starting with -. Fixed for the VM's life.
    #[arg(long, value_name = "NAME", value_parser = placement_group)]
    pub(crate) placement_group: Option<String>,
    /// What a power-off from inside the VM does: restart (the server's default) or stop
    #[arg(long, value_name = "POLICY", value_enum)]
    pub(crate) on_guest_poweroff: Option<GuestPoweroff>,
    /// Turn on scheduled backups once the app runs, at this many seconds between backups
    ///
    /// Stored backups are billed per GB-hour at the price hipc compute backups options shows.
    #[arg(long, value_name = "SECONDS", value_enum)]
    pub(crate) backup_interval: Option<BackupInterval>,
    /// The failover of those backups: auto restarts the app on another server from its latest
    /// backup if its server dies, losing what was written after it; manual, the server's
    /// default, leaves that to Hippius support
    #[arg(long, value_name = "MODE", value_enum, requires = "backup_interval")]
    pub(crate) failover: Option<FailoverMode>,
    /// Wait for running even when stdout is not a terminal, CI is set, or AI_AGENT is set
    ///
    /// The app is running when its status and boot_phase both are. A run killed during the
    /// wait prints nothing on stdout; a caller that may be killed first omits --wait and
    /// follows the app with hipc compute app-engine apps get.
    #[arg(long, conflicts_with = "no_wait")]
    pub(crate) wait: bool,
    /// Print the launch answer and exit without waiting for running
    ///
    /// The default when stdout is not a terminal, or CI is set, or AI_AGENT is set and not off.
    #[arg(long, conflicts_with = "wait_timeout")]
    pub(crate) no_wait: bool,
    /// Wait for running for this many seconds, 1 to 3600, default 300
    ///
    /// It waits as --wait does. A wait that ends before running prints the app on stdout, so
    /// its vm_id is not lost.
    #[arg(
        long,
        value_name = "SECONDS",
        value_parser = clap::value_parser!(u64).range(1..=3600)
    )]
    pub(crate) wait_timeout: Option<u64>,
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
    Get {
        /// The vm_id from hipc compute app-engine apps list
        #[arg(value_parser = vm_id)]
        vm_id: String,
    },
    /// The app's latest guest components upgrade: its release, state, and outcome
    ///
    /// job is null when the app was never upgraded.
    #[command(after_help = examples(&[
        "hipc compute app-engine guest-components update-status vm-1a2b",
    ]))]
    UpdateStatus {
        /// The vm_id from hipc compute app-engine apps list
        #[arg(value_parser = vm_id)]
        vm_id: String,
    },
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
