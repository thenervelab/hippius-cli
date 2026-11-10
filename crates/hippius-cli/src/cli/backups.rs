//! The grammar of `hipc compute backups`.

use clap::{Args, Subcommand, ValueEnum};

use crate::cli::values::{Charset, Dash, word};
use crate::cli::{examples, vm_id};

/// A backup `run_id`. Swagger types it as a string with no pattern. It goes into the
/// request body, not the path, but is held to the same rule as a `vm_id`, wide enough for
/// the UUIDs and slugs seen, so a pasted value with spaces or quotes is refused before the
/// confirmation rather than by the server after it.
fn point_id(value: &str) -> Result<String, String> {
    word(
        value,
        1..=64,
        Charset::Slug,
        Dash::Taken,
        "a backup point is 1 to 64 letters, digits, `-`, and `_`",
    )
}

#[derive(Debug, Subcommand)]
pub(crate) enum Backups {
    /// Whether backups are offered, the intervals, how long old backups are kept, the largest disk
    /// backed up, and the price per GB-month stored
    #[command(after_help = examples(&[
        "hipc compute backups options",
    ]))]
    Options,
    /// Read, set, or turn off a VM's backup policy
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute backups policy get vm-1a2b",
        "hipc compute backups policy update vm-1a2b --interval 3600 --yes",
    ]))]
    Policy(BackupPolicy),
    /// Every backup a VM still holds, newest first, with its size, whether a restore can use it
    /// (restorable), and its class
    ///
    /// A reboot makes the earlier backups unrestorable, or class rollback: a restore to a rollback
    /// point needs --accept-rollback.
    #[command(after_help = examples(&[
        "hipc compute backups list vm-1a2b",
    ]))]
    List {
        /// The vm_id from hipc compute vms list
        #[arg(value_parser = vm_id)]
        vm_id: String,
    },
    /// Overwrite a VM's disk with one of its backups; needs --yes
    ///
    /// Everything written after that backup is lost. The backup is downloaded while the VM runs,
    /// then the VM stops for a few minutes while its disk is swapped, and returns to the power
    /// state it had. Before the confirmation, hipc reads the VM's name, which the question names,
    /// and sends it as the server's confirm check, so --yes is the only confirmation to give;
    /// --dry-run reads nothing, shows a placeholder there, and lists the read in requests. The
    /// command returns once the restore has started; restore-status shows its progress.
    #[command(after_help = examples(&[
        "hipc compute backups restore vm-1a2b --run-id run-20261027 --yes",
    ]))]
    Restore(BackupRestore),
    /// The latest restore of a VM and its progress: pending, staging, stopping, activating,
    /// verifying, or settling, then done, failed, or reverted (the VM is back as it was)
    #[command(after_help = examples(&[
        "hipc compute backups restore-status vm-1a2b",
    ]))]
    RestoreStatus {
        /// The vm_id from hipc compute vms list
        #[arg(value_parser = vm_id)]
        vm_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum BackupPolicy {
    /// A VM's backup policy: enabled, interval_s, state, the latest restore point, the bytes
    /// stored, and the failover mode
    #[command(after_help = examples(&[
        "hipc compute backups policy get vm-1a2b",
    ]))]
    Get {
        /// The vm_id from hipc compute vms list
        #[arg(value_parser = vm_id)]
        vm_id: String,
    },
    /// Turn a VM's backups on, or change their interval; spends money: needs --yes
    ///
    /// Stored bytes are billed per GB-hour from the balance, at the price hipc compute backups
    /// options shows. The first backup is a full copy of the disk; later ones copy what changed. A
    /// reboot makes the earlier backups unrestorable, so a full backup follows each boot. Backups
    /// need a VM launched from a golden image.
    #[command(after_help = examples(&[
        "hipc compute backups policy update vm-1a2b --interval 3600 --yes",
        "hipc compute backups policy update vm-1a2b --interval 900 --failover auto --yes",
    ]))]
    Update(BackupPolicyUpdate),
    /// Turn a VM's backups off; needs --yes
    ///
    /// No new backup is taken. The backups already taken are kept 7 days, billed while kept, then
    /// deleted.
    #[command(after_help = examples(&[
        "hipc compute backups policy delete vm-1a2b --yes",
    ]))]
    Delete {
        /// The vm_id from hipc compute vms list
        #[arg(value_parser = vm_id)]
        vm_id: String,
    },
}

#[derive(Debug, Args)]
pub(crate) struct BackupPolicyUpdate {
    /// The vm_id from hipc compute vms list
    #[arg(value_parser = vm_id)]
    pub(crate) vm_id: String,
    /// Seconds between backups: the most data a restore could lose
    #[arg(long, value_name = "SECONDS", value_enum)]
    pub(crate) interval: BackupInterval,
    /// The failover: auto restarts the VM on another server from its latest backup if its server
    /// dies, losing what was written after that backup; manual leaves that to Hippius support
    ///
    /// Sent only when passed; without it the VM keeps its mode.
    #[arg(long, value_enum)]
    pub(crate) failover: Option<FailoverMode>,
}

/// The `interval_s` values `PUT .../backup/` accepts (swagger enum).
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum BackupInterval {
    #[value(name = "900")]
    FifteenMinutes,
    #[value(name = "3600")]
    Hour,
    #[value(name = "21600")]
    SixHours,
    #[value(name = "86400")]
    Day,
}

impl BackupInterval {
    pub(crate) fn seconds(self) -> u32 {
        match self {
            Self::FifteenMinutes => 900,
            Self::Hour => 3600,
            Self::SixHours => 21600,
            Self::Day => 86400,
        }
    }
}

/// The `failover_mode` values `PUT .../backup/` accepts (swagger enum).
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum FailoverMode {
    Auto,
    Manual,
}

impl FailoverMode {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct BackupRestore {
    /// The vm_id from hipc compute vms list
    #[arg(value_parser = vm_id)]
    pub(crate) vm_id: String,
    /// The backup's run_id from hipc compute backups list, one marked restorable
    #[arg(long, value_name = "RUN_ID", value_parser = point_id)]
    pub(crate) run_id: String,
    /// Allow a point from before the VM's last reboot (class rollback), which the server refuses
    /// without it
    #[arg(long)]
    pub(crate) accept_rollback: bool,
}
