//! The grammar of `hipc compute app-engine`: App Engine, which runs one docker compose file
//! on a VM booted from Hippius's sealed app image. The reads, the compose check and fix, the
//! launch, the compose and secrets update, and the guest components upgrade.
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
        "hipc compute app-engine apps update vm-1a2b --compose-file compose.yaml --yes",
    ]))]
    Apps(AppEngineApps),
    /// The guest components release an app's VM runs, the newest one offered, and the latest
    /// upgrade onto one
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute app-engine guest-components get vm-1a2b",
        "hipc compute app-engine guest-components update vm-1a2b",
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
    /// Replace an app's compose, its secrets, or both; the VM stops and starts again; needs
    /// --yes
    ///
    /// Sends PUT /api/compute/app-engine/apps/<vm_id>/compose/. Pass --compose-file,
    /// --secrets-file, or both; neither is a usage error, and an empty secrets file sends
    /// no secrets. hipc always reads GET /api/compute/app-engine/apps/<vm_id>/ for the
    /// secret names the app stores. With --compose-file it also sends POST
    /// /api/compute/app-engine/validate/ with the compose alone, and checks that
    /// compose_sha256 is the file's SHA-256. A name the new compose does not declare, or,
    /// with no new compose, a name the app does not declare, is a usage error. So is a
    /// declared name that is neither stored nor in the file. The values given replace the
    /// stored ones; the rest are kept. Then it asks. The VM stops and starts again and its
    /// volumes are kept. When a compose is sent, the launch measurement then holds that
    /// file's SHA-256, so attestation proves this file. With only --secrets-file, the
    /// launch measurement changes, but the compose hash in it stays the one the app runs
    /// now. The new compose must list the current compose's SHA-256 in
    /// x-hippius.upgrade_from, or the server answers 409 app-lineage-missing, and hipc does
    /// not insert that line. Secret values are write-only: they are sent once and never
    /// shown again. In this beta, Hippius operators can read them and the app's volumes. A
    /// dry run shows a placeholder for each value.
    #[command(after_help = examples(&[
        "hipc compute app-engine apps update vm-1a2b --compose-file compose.yaml --yes",
        "hipc compute app-engine apps update vm-1a2b --secrets-file secrets.json --yes",
        "hipc compute app-engine apps update vm-1a2b --compose-file compose.yaml \
         --secrets-file secrets.json --yes",
    ]))]
    Update(AppUpdate),
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

/// `--compose-file`, which `validate`, `fix`, `apps create`, and `apps update` read the same
/// way, so its help is written once, with the limit [`MAX_COMPOSE_BYTES`] says.
#[derive(Debug, Args)]
pub(crate) struct ComposeFile {
    #[arg(
        id = "compose_file",
        long = "compose-file",
        value_name = "PATH",
        help = compose_help(),
        long_help = compose_long_help()
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

/// The long help of `--compose-file`.
fn compose_long_help() -> String {
    format!(
        "{}\n\nThe server takes a smaller file: at most max_compose_bytes from hipc compute \
         app-engine get. /dev/stdin and <(...) are refused, as they are not regular files, \
         so pass - for stdin.",
        compose_help()
    )
}

/// The flags of `apps update`. At least one of the files is required; hipc says so, rather
/// than clap, so the message can name both.
#[derive(Debug, Args)]
pub(crate) struct AppUpdate {
    #[command(flatten)]
    pub(crate) target: AppTarget,
    #[arg(
        long = "compose-file",
        value_name = "PATH",
        help = compose_help(),
        long_help = compose_long_help()
    )]
    pub(crate) compose_file: Option<PathBuf>,
    /// A JSON object of each secret's name to its value; the values given
    /// replace the stored ones; the rest are kept
    ///
    /// A name left out keeps the value the app stores. The file must be the current user's,
    /// of mode 0600, not a symlink, at most 1 MiB, in a directory no one else can write;
    /// there is no stdin form. Each value is sent exactly as the file holds it: nothing is
    /// trimmed. Values are write-only, and in this beta Hippius operators can read them. An
    /// empty object sends no secrets.
    #[arg(long, value_name = "PATH", value_parser = secrets_file)]
    pub(crate) secrets_file: Option<PathBuf>,
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
    /// Upgrade an app's guest components to the newest release; needs --yes when one is
    /// offered
    ///
    /// hipc first reads GET /api/compute/app-engine/apps/<vm_id>/guest-components/. When
    /// release equals newest_release, it exits 0, notes that the app is already on the
    /// newest release, and posts nothing. When no newer release is offered, it exits 0,
    /// notes that, and posts nothing. --yes is not required for either. Otherwise it asks,
    /// then sends POST /api/compute/app-engine/apps/<vm_id>/guest-upgrade/ with confirm
    /// true, release set to newest_release, and expected_current_release set to the release
    /// the read showed, or null for the bare base. The VM restarts and its attestation
    /// measurement changes. Volumes and its network identity are kept. A 409
    /// guest-already-current reads the guest components again. When that read still offers
    /// an update, hipc exits 1 conflict and prints the read. Otherwise it exits 0 with the
    /// note that read supports. An upgrade already in flight, upgrade.in_flight true, is
    /// refused before the question as conflict, and the hint names hipc compute app-engine
    /// guest-components update-status. Poll a scheduled upgrade with that command.
    ///
    /// With --not-before YYYY-MM-DDTHH:MM:SSZ (or an offset such as +02:00 in place of Z),
    /// the post also sends not_before, and the upgrade waits until that time. A time in the
    /// past, or more than 30 days ahead by the local clock, is refused as usage before
    /// anything is sent. The question names the time, and the dry run shows not_before.
    #[command(after_help = examples(&[
        "hipc compute app-engine guest-components update vm-1a2b",
        "hipc compute app-engine guest-components update vm-1a2b --yes",
    ]))]
    Update(GuestUpdate),
    /// The app's latest guest components upgrade: its release, state, and outcome
    ///
    /// job is null when the app was never upgraded.
    #[command(after_help = examples(&[
        "hipc compute app-engine guest-components update-status vm-1a2b",
    ]))]
    UpdateStatus(AppTarget),
}

#[derive(Debug, Args)]
pub(crate) struct GuestUpdate {
    #[command(flatten)]
    pub(crate) app: AppTarget,
    /// The earliest time the upgrade may start, in RFC 3339: YYYY-MM-DDTHH:MM:SSZ
    ///
    /// Z or an offset such as +02:00, from now to at most 30 days ahead. Without it the
    /// upgrade starts now.
    #[arg(long, value_name = "TIME", value_parser = not_before)]
    pub(crate) not_before: Option<NotBefore>,
}

/// A `--not-before` time: the text sent as `not_before`, and the same instant in seconds
/// since 1970, which the command checks against the clock before anything is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotBefore {
    pub(crate) text: String,
    pub(crate) seconds: u64,
}

/// A `--not-before`, read here so that a typo is refused before anything is sent: an RFC 3339
/// time with `Z` or an offset, and a date that exists. A leap second (`:60`) is refused too:
/// the reader in `hippius` takes one, as the API may write one, but the API's parser refuses
/// it in a request, and only after the question was answered. A lowercase `t` or `z` is sent
/// uppercase, as the API writes them. Whether the time is in the window the API takes is a
/// matter of the clock, so the command checks that when it runs.
fn not_before(value: &str) -> Result<NotBefore, String> {
    let text = value.replace('t', "T").replace('z', "Z");
    // A time that parsed is `YYYY-MM-DDTHH:MM:SS...`, so the seconds are 6..8 after the `T`.
    let leap = text
        .split_once('T')
        .is_some_and(|(_, clock)| clock.get(6..8) == Some("60"));
    hippius::rfc3339_seconds(&text)
        .filter(|_| !leap)
        .map(|seconds| NotBefore { text, seconds })
        .ok_or_else(|| "a time is RFC 3339, such as `YYYY-MM-DDTHH:MM:SSZ`".to_owned())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A `--not-before` is an RFC 3339 time with a zone, read as the instant it names, and
    /// sent as given but for a lowercase `t` or `z`.
    #[test]
    fn not_before_reads_an_rfc_3339_time() {
        for (given, sent, seconds) in [
            (
                "2026-12-01T02:00:00Z",
                "2026-12-01T02:00:00Z",
                1_796_090_400,
            ),
            (
                "2026-12-01t02:00:00z",
                "2026-12-01T02:00:00Z",
                1_796_090_400,
            ),
            (
                "2026-12-01T04:00:00+02:00",
                "2026-12-01T04:00:00+02:00",
                1_796_090_400,
            ),
            (
                "2026-11-30T21:00:00.5-05:00",
                "2026-11-30T21:00:00.5-05:00",
                1_796_090_400,
            ),
        ] {
            assert_eq!(
                not_before(given),
                Ok(NotBefore {
                    text: sent.to_owned(),
                    seconds,
                }),
                "{given}"
            );
        }
        for bad in [
            "tomorrow",
            "2026-12-01",
            "2026-12-01 02:00:00Z",
            "2026-12-01T02:00:00",
            "2026-02-30T02:00:00Z",
            "2026-12-01T02:00:00+24:00",
            "2026-12-31T23:59:60Z",
        ] {
            assert_eq!(
                not_before(bad),
                Err("a time is RFC 3339, such as `YYYY-MM-DDTHH:MM:SSZ`".to_owned()),
                "{bad}"
            );
        }
    }

    /// The long help of `--compose-file` is the short help, then where the server's smaller
    /// limit is read and why `/dev/stdin` is refused.
    #[test]
    fn the_compose_long_help_adds_the_server_limit_and_the_stdin_rule() {
        let long = compose_long_help();
        assert!(
            long.starts_with(&format!("{}\n\n", compose_help())),
            "{long}"
        );
        assert!(
            long.contains("at most max_compose_bytes from hipc compute app-engine get"),
            "{long}"
        );
        assert!(long.contains("so pass - for stdin"), "{long}");
    }
}
