//! The grammar of `hipc compute` and its VM, SSH key, and public IPv4 commands.

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

use crate::cli::values::{Charset, Dash, word};
use crate::cli::{
    AppEngine, BackupInterval, Backups, Databases, Exposures, FailoverMode, Firewall, Kubernetes,
    ListArgs, RESIZE_ID, Runners, SSH_KEY_ID, examples, number, region, vm_id,
};

#[derive(Debug, Subcommand)]
pub(crate) enum Compute {
    /// The account's price list for every VM component, in base units; needs a token
    ///
    /// GET /api/compute/prices/: amounts in base units (10^18 = 1 credit, and 1 credit = $1),
    /// with billed saying whether the account's compute is charged. hipc compute public-prices
    /// reads the public list, in credits, with no token.
    #[command(after_help = examples(&[
        "hipc compute prices",
    ]))]
    Prices,
    // A VM's price is the sum of its vCPUs, memory, and disk at its OS disk tier
    // (https://docs.hippius.com/use/compute/billing).
    /// Everything compute sells, at the public prices in credits; needs no token
    ///
    /// GET /api/compute/public-prices/: decimal strings in credits (1 credit = $1), per OS disk
    /// tier, with the dedicated disks free per region, public IPv4, backups, the managed
    /// database and Kubernetes fees, and the runner sizes. Nothing in it is per account; hipc
    /// compute prices reads the account's own list. A flavor's hourly and monthly at a tier
    /// price the whole VM, its OS disk of disk_gb GB included.
    #[command(after_help = examples(&[
        "hipc compute public-prices",
    ]))]
    PublicPrices,
    /// What the account may launch, what it uses, and the balance runway a launch needs
    #[command(after_help = examples(&[
        "hipc compute quota",
    ]))]
    Quota,
    /// The most the chain charges the account per hour, and the latest request to raise it
    ///
    /// A per-hour safety limit of the chain, not a budget. A monthly limit is a project's budget,
    /// hipc account projects budget update: it mails alerts, and with --block-launches true it
    /// refuses new launches that add cost once the limit is reached, but it does not stop a VM
    /// that runs. The Default project has no budget: create a project with hipc account
    /// projects create, launch in it, and set its budget.
    ///
    /// charge_cap is in base units per hour, with overridden when it was raised for the account,
    /// and is null while the chain has no per-account caps or the cap is not read yet, so a null
    /// does not mean there is no cap. request is the latest raise request, null when there is
    /// none. An hour that would cost more than the cap waits until the cap is raised. Asking for
    /// a raise is a confirmed hipc api POST /compute/charge-cap/.
    #[command(after_help = examples(&[
        "hipc compute charge-cap",
    ]))]
    ChargeCap,
    /// The VM sizes; needs a token
    ///
    /// hipc compute public-prices lists the sizes with their prices and needs no token.
    #[command(after_help = examples(&[
        "hipc compute flavors",
    ]))]
    Flavors,
    /// The OS images, with the SSH login (default_user) each one creates
    #[command(after_help = examples(&[
        "hipc compute images",
    ]))]
    Images,
    /// The ready-made apps: each pins its image and ports, and lists the fields a launch collects
    /// (config_schema)
    ///
    /// Read-only: hipc compute vms create does not launch an app. To run a docker compose file
    /// of one's own, see hipc compute app-engine.
    #[command(after_help = examples(&[
        "hipc compute apps",
    ]))]
    Apps,
    /// App Engine: one docker compose file on a VM booted from Hippius's sealed app image under
    /// AMD SEV-SNP, whose launch measurement holds the compose's SHA-256
    ///
    /// It proves which compose a VM runs, not that its data is hidden: in this beta, Hippius
    /// operators can read an app's secrets and volumes. An app is a VM, billed as one of its
    /// flavor with a public IPv4.
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute app-engine get",
        "hipc compute app-engine validate --compose-file compose.yaml",
        "hipc compute app-engine apps list",
    ]))]
    AppEngine(AppEngine),
    /// The regions a VM can launch in
    #[command(after_help = examples(&[
        "hipc compute regions",
    ]))]
    Regions,
    /// Whether a launch can name a placement group, and the groups the account's live VMs are in
    ///
    /// A group exists while a live VM names it; there is no command to create or delete one. vms
    /// create --placement-group puts a VM in a group.
    #[command(after_help = examples(&[
        "hipc compute placement-groups",
    ]))]
    PlacementGroups,
    /// Whether a launch of a flavor could land in a region now: verdict yes, not-now, never, or
    /// unknown
    ///
    /// It launches nothing. feasible is true only for yes, and every verdict exits 0.
    #[command(after_help = examples(&[
        "hipc compute feasibility --flavor small",
        "hipc compute feasibility --flavor small --region FR",
    ]))]
    Feasibility {
        /// The flavor code from hipc compute flavors
        #[arg(long, value_parser = flavor)]
        flavor: String,
        /// The region to check: an ISO 3166-1 alpha-2 country code, such as FR
        ///
        /// Without it the answer is for the whole fleet.
        #[arg(long, value_name = "CC", value_parser = region)]
        region: Option<String>,
        /// The answer for a VM whose OS disk is on this tier, as vms create --os-disk-tier
        /// asks
        ///
        /// Without it the answer is for nvme. A tier not sold is verdict never.
        #[arg(long, value_name = "TIER", value_enum)]
        os_disk_tier: Option<DiskTier>,
    },
    /// List, launch, and control virtual machines
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute vms list",
        "hipc compute vms create --flavor small --ssh-key-id 1 --yes",
        "hipc compute vms get vm-1a2b",
    ]))]
    Vms(Vms),
    /// Attach, read, and detach a VM's dedicated public IPv4, and see what is free
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute public-ip availability",
        "hipc compute public-ip create vm-1a2b --yes",
    ]))]
    PublicIp(PublicIp),
    /// The inbound rules on a VM's public IPv4: every port is closed until a rule opens it
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute firewall get vm-1a2b",
        "hipc compute firewall update vm-1a2b --rules-file rules.json --yes",
    ]))]
    Firewall(Firewall),
    /// Publish a VM's ports on the public edge at a generated hostname, list them, and take them
    /// down
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute exposures list vm-1a2b",
        "hipc compute exposures create vm-1a2b --port 8080 --yes",
    ]))]
    Exposures(Exposures),
    /// A VM's backup policy, its backups, and restoring its disk from one of them
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute backups options",
        "hipc compute backups policy update vm-1a2b --interval 3600 --yes",
        "hipc compute backups list vm-1a2b",
    ]))]
    Backups(Backups),
    /// GitHub Actions runners: a job with runs-on: hippius-small, hippius-medium, or hippius-large,
    /// with an optional -ssd and country suffix, runs on a single-use VM of the account, billed to
    /// the balance
    ///
    /// The -ssd suffix picks the cheaper SSD tier, and a two-letter country code at the end pins
    /// the region, such as hippius-medium-ssd-fr. The Hippius Runners GitHub App must be installed
    /// on the GitHub organization or user and linked to the account: connect prints the install
    /// URL, and the console finishes the link.
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute runners connect",
        "hipc compute runners settings update --enabled true --yes",
        "hipc compute runners jobs",
    ]))]
    Runners(Runners),
    /// The SSH keys stored on the signed-in user, which vms create --ssh-key-id installs
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute ssh-keys list",
        "hipc compute ssh-keys create --name laptop --public-key-file id_ed25519.pub --yes",
    ]))]
    SshKeys(SshKeys),
    /// The SSH keys of the account's owner, which a member launching a VM in the account is
    /// offered
    ///
    /// Read-only. A member's own keys are hipc compute ssh-keys list. With no account selected
    /// (--account, HIPPIUS_ACCOUNT, or hipc account use), these are the caller's own keys.
    #[command(after_help = examples(&[
        "hipc compute account-ssh-keys",
    ]))]
    AccountSshKeys,
    /// Managed PostgreSQL 16, 17, and 18 on confidential VMs, billed by usage from the balance
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute databases list",
        "hipc compute databases create --tier starter --size s --engine-version 17 --reveal --yes",
        "hipc compute databases get 7",
    ]))]
    Databases(Databases),
    /// Managed Kubernetes on three confidential masters, billed by usage from the balance: read
    /// clusters, delete them, and manage node pools, operations, and nodes
    ///
    /// Each cluster runs RKE2, a Kubernetes distribution. Opening a cluster's kubeconfig,
    /// scaling a node pool up, and replacing a node take the cluster key file the console saved
    /// when it created the cluster, passed as --cluster-key-file. Creating a cluster, which
    /// generates that key, stays in the console.
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute kubernetes clusters list",
        "hipc compute kubernetes operations list 7",
    ]))]
    Kubernetes(Kubernetes),
}

#[derive(Debug, Subcommand)]
pub(crate) enum Vms {
    /// Every VM in the account
    #[command(after_help = examples(&[
        "hipc compute vms list",
    ]))]
    List,
    /// One VM
    #[command(after_help = examples(&[
        "hipc compute vms get vm-1a2b",
    ]))]
    Get {
        /// The vm_id from hipc compute vms list
        #[arg(value_parser = vm_id)]
        vm_id: String,
    },
    /// Whether the VM booted as a confidential (AMD SEV-SNP) guest, as the key broker recorded it
    ///
    /// verdict unknown means no evidence was recorded, so confidentiality is not proven either way;
    /// it does not mean the VM is not confidential. Text mode prints it as "not proven". attested
    /// is true or null, never false: read verdict. A positive answer is cached for 60 seconds per
    /// VM.
    #[command(after_help = examples(&[
        "hipc compute vms attestation vm-1a2b",
    ]))]
    Attestation {
        /// The vm_id from hipc compute vms list
        #[arg(value_parser = vm_id)]
        vm_id: String,
    },
    /// Up to the VM's last 10 failovers, newest first: each time it was restarted on another
    /// server because its own stopped responding
    ///
    /// automatic is true for failover_mode auto, and false when Hippius support started it.
    /// Everything written after point_taken_at is lost. usable says whether the VM can be used
    /// again; committed_at is when it ran on the other server.
    #[command(after_help = examples(&[
        "hipc compute vms failovers vm-1a2b",
    ]))]
    Failovers {
        /// The vm_id from hipc compute vms list
        #[arg(value_parser = vm_id)]
        vm_id: String,
    },
    /// Launch a VM, and at a terminal wait until it is running; spends money: needs --yes
    ///
    /// The VM is billed from its launch until it is deleted, running or stopped. The launch
    /// needs a balance that covers the runway hipc compute quota shows; the server answers 402
    /// otherwise. The SSH user is the image's default_user from hipc compute images.
    /// When stdout is not a terminal, or CI is set, or AI_AGENT is set and not off, the command
    /// prints the launch answer and exits, as --no-wait does; hipc compute vms get follows the
    /// VM, and --wait or --wait-timeout waits.
    /// With --public-ip the wait also lasts until network_ready is true, when the address can
    /// be reached. A boot that stalls shows failed with boot_stalled true and recovers by
    /// itself, so the wait goes on through it.
    /// If the wait ends without running, the VM document is still printed, so its vm_id is not
    /// lost. With --open-port or --open-ssh, the firewall rule is set right after the launch
    /// answer, before any wait, and a dry run lists that request after the launch.
    #[command(after_help = examples(&[
        "hipc compute vms create --flavor small --ssh-key-id 1 --yes",
        "hipc compute vms create --flavor small --ssh-public-key-file id_ed25519.pub --yes",
        "hipc compute vms create --flavor small --ssh-key-id 1 --public-ip --no-wait --yes",
        "hipc compute vms create --flavor small --ssh-key-id 1 --public-ip --open-ssh --open-port 443 --yes",
    ]))]
    Create(VmCreate),
    /// Power a VM on
    ///
    /// A VM that is already running answers conflict.
    #[command(after_help = examples(&[
        "hipc compute vms start vm-1a2b",
    ]))]
    Start {
        /// The vm_id from hipc compute vms list
        #[arg(value_parser = vm_id)]
        vm_id: String,
    },
    /// Power a VM off; needs --yes
    ///
    /// Everything the VM serves goes down until it is started again. A stopped VM is still
    /// billed: delete it to stop paying.
    #[command(after_help = examples(&[
        "hipc compute vms stop vm-1a2b --yes",
    ]))]
    Stop {
        /// The vm_id from hipc compute vms list
        #[arg(value_parser = vm_id)]
        vm_id: String,
    },
    /// Restart a VM; needs --yes
    #[command(after_help = examples(&[
        "hipc compute vms reboot vm-1a2b --yes",
    ]))]
    Reboot {
        /// The vm_id from hipc compute vms list
        #[arg(value_parser = vm_id)]
        vm_id: String,
    },
    /// The sizes a VM can be resized to, with prices, and why a resize cannot start now (blocked,
    /// cooldown_until, in_flight)
    #[command(after_help = examples(&[
        "hipc compute vms resize-options vm-1a2b",
    ]))]
    ResizeOptions {
        /// The vm_id from hipc compute vms list
        #[arg(value_parser = vm_id)]
        vm_id: String,
    },
    /// Change a VM's vCPU and memory; its disk and data are kept; spends money, as the bill
    /// switches to the new size: needs --yes
    ///
    /// A running VM reboots, down for about 1 to 3 minutes, longer when it moves to another server.
    /// A stopped VM stays stopped and starts at the new size. The bill switches to the new size
    /// when the VM restarts at it, not at the request. The command returns once the resize has
    /// started; resize-status shows its progress.
    #[command(after_help = examples(&[
        "hipc compute vms resize vm-1a2b --flavor large --yes",
    ]))]
    Resize {
        /// The vm_id from hipc compute vms list
        #[arg(value_parser = vm_id)]
        vm_id: String,
        /// The flavor code from hipc compute vms resize-options
        #[arg(long, value_parser = flavor)]
        flavor: String,
    },
    /// One resize of a VM: pending, stopping, migrating, relaunching, or rolling_back, then done,
    /// or failed (with rolled_back)
    #[command(after_help = examples(&[
        "hipc compute vms resize-status vm-1a2b 3",
    ]))]
    ResizeStatus {
        /// The vm_id from hipc compute vms list
        #[arg(value_parser = vm_id)]
        vm_id: String,
        /// The resize id that hipc compute vms resize printed
        #[arg(value_parser = number(RESIZE_ID))]
        resize_id: u64,
    },
    /// What happens when the VM's own operating system powers it off: restart it, or leave
    /// it stopped
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute vms power-policy get vm-1a2b",
        "hipc compute vms power-policy update vm-1a2b --on-guest-poweroff stop",
    ]))]
    PowerPolicy(PowerPolicy),
    /// Delete a VM and its disks; needs --yes
    ///
    /// The VM's time stops being billed. An attached public IPv4 is released with the VM, so
    /// there is no need to detach it first, and is billed until then; stored backups are billed
    /// until they are removed. Before the question, hipc reads the VM, whose name the question
    /// gives; --dry-run reads nothing and lists the read in requests.
    #[command(after_help = examples(&[
        "hipc compute vms delete vm-1a2b --yes",
    ]))]
    Delete {
        /// The vm_id from hipc compute vms list
        #[arg(value_parser = vm_id)]
        vm_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum PowerPolicy {
    /// The VM's power policy: on_guest_poweroff, and whether its server applied it yet
    ///
    /// on_guest_poweroff_pending is true until the VM's server acknowledges the policy, and
    /// on_guest_poweroff_effective is what it acknowledged. stop_reason is guest-poweroff
    /// while the VM is stopped because its own operating system powered it off.
    #[command(after_help = examples(&[
        "hipc compute vms power-policy get vm-1a2b",
    ]))]
    Get {
        /// The vm_id from hipc compute vms list
        #[arg(value_parser = vm_id)]
        vm_id: String,
    },
    /// Set what happens when the VM's own operating system powers it off
    ///
    /// restart, the default, starts the VM again, so a VM shut down from inside keeps running
    /// and is billed as a running VM. stop leaves it stopped, exactly like hipc compute vms
    /// stop: it keeps its reservation, is still billed as a stopped VM, and hipc compute vms
    /// start brings it back. A crash of its server restarts it either way. The policy applies
    /// to a running VM without a reboot.
    #[command(after_help = examples(&[
        "hipc compute vms power-policy update vm-1a2b --on-guest-poweroff stop",
    ]))]
    Update {
        /// The vm_id from hipc compute vms list
        #[arg(value_parser = vm_id)]
        vm_id: String,
        /// What a power-off from inside the VM does: restart or stop
        #[arg(long, value_name = "POLICY", value_enum)]
        on_guest_poweroff: GuestPoweroff,
    },
}

/// The values of `on_guest_poweroff` (swagger `compute_vms_power-policy_update`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum GuestPoweroff {
    Restart,
    Stop,
}

impl GuestPoweroff {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Restart => "restart",
            Self::Stop => "stop",
        }
    }
}

// The server refuses a launch with no key. A group names both flags in clap's refusal,
// where `required_unless_present` on one flag named only that one.
#[derive(Debug, Args)]
#[command(group(
    clap::ArgGroup::new("key")
        .required(true)
        .multiple(true)
        .args(["ssh_key_ids", "ssh_public_key_file"])
))]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is one independent command-line flag, which clap parses into a field"
)]
pub(crate) struct VmCreate {
    /// The flavor code from hipc compute flavors
    #[arg(long, value_parser = flavor)]
    pub(crate) flavor: String,
    /// A label
    ///
    /// The server defaults it to the VM id.
    #[arg(long)]
    pub(crate) name: Option<String>,
    /// The image name from hipc compute images
    ///
    /// Without it the server picks the flavor's default image. Pass one to know which image,
    /// and so which SSH user (its default_user in hipc compute images), the VM gets.
    #[arg(long)]
    pub(crate) image: Option<String>,
    /// The region: a two-letter country code from hipc compute regions, such as FR
    ///
    /// Without it the scheduler chooses.
    #[arg(long, value_name = "CC", value_parser = region)]
    pub(crate) region: Option<String>,
    /// A key id from hipc compute ssh-keys list; repeat it to install several, up to 20
    ///
    /// The server refuses a launch with no key: pass this, --ssh-public-key-file, or both.
    #[arg(
        long = "ssh-key-id",
        value_name = "ID",
        value_parser = number(SSH_KEY_ID)
    )]
    pub(crate) ssh_key_ids: Vec<u64>,
    /// A public key file to install, such as ~/.ssh/id_ed25519.pub, or - for stdin
    ///
    /// It is installed beside the --ssh-key-id keys and is not stored; hipc compute ssh-keys
    /// create stores one. The file holds one OpenSSH public key line, such as ssh-ed25519
    /// AAAA... user@host; anything else, a private key included, is refused before anything is
    /// sent.
    #[arg(long, value_name = "PATH")]
    pub(crate) ssh_public_key_file: Option<PathBuf>,
    /// Attach a public IPv4 once the VM runs, billed until it is detached or the VM is
    /// deleted
    ///
    /// It is billed per second at the price hipc compute public-ip availability shows:
    /// price_hourly, or free until free_until while it is set, then price_hourly_after_free.
    /// The server refuses the launch before anything is bought when no address is free.
    /// Deleting the VM releases the address.
    ///
    /// A new VM has no firewall rules, so the address accepts nothing: open the ports it
    /// serves, such as 22 for SSH, with --open-port or --open-ssh, or later with hipc compute
    /// firewall update.
    #[arg(long)]
    pub(crate) public_ip: bool,
    /// Open this TCP port, 1 to 65535, to every IPv4 address once the launch is accepted;
    /// repeat it for more
    ///
    /// A new VM has no firewall rules, so every port of its public IPv4 is closed. Right after
    /// the launch answer, hipc sets the VM's rules to one tcp rule for these ports with no
    /// source_cidrs, which accepts every address, as hipc compute firewall update would. A rule
    /// written before an address is attached applies once one is. If the launch succeeds but
    /// the rules cannot be set, the command exits 1 partial_failure, prints the VM on stdout,
    /// and the hint names the firewall update to run; the VM keeps running with every port
    /// closed. Otherwise the VM printed carries firewall, the rules sent. To accept fewer
    /// addresses, leave this out and run hipc compute firewall update.
    #[arg(
        long = "open-port",
        value_name = "PORT",
        value_parser = clap::value_parser!(u16).range(1..)
    )]
    pub(crate) open_ports: Vec<u16>,
    /// Same as --open-port 22, for SSH
    #[arg(long)]
    pub(crate) open_ssh: bool,
    /// The anti-affinity group: two VMs of the account in one group never share a server
    ///
    /// A name is 1 to 64 of a-z, 0-9, and -, not starting with -. Fixed for the VM's life. hipc
    /// compute placement-groups says whether the server offers groups.
    #[arg(long, value_name = "NAME", value_parser = placement_group)]
    pub(crate) placement_group: Option<String>,
    /// The OS disk's tier, a minimum: the disk may land on a faster one; the server defaults it
    /// to nvme
    ///
    /// Fixed for the VM's life, and billed at that tier's price, vm_by_os_disk_tier in hipc
    /// compute prices. hipc compute feasibility --os-disk-tier says whether a launch on it could
    /// land now.
    #[arg(long, value_name = "TIER", value_enum)]
    pub(crate) os_disk_tier: Option<DiskTier>,
    /// A whole physical disk for the VM, SIZE:TIER, such as 2000:hdd; repeat it once per disk,
    /// up to 8
    ///
    /// SIZE is the least GiB the disk has, 1000 or more; TIER is nvme, ssd, or hdd, a minimum.
    /// Every disk of a launch has the same size and tier. Each is billed per GiB-hour at its
    /// size class (floored to 100 GiB), while the VM is stopped too, at the price dedicated_disks
    /// in hipc compute prices. The disks pin the VM to its server: no backup, migration, or
    /// failover, so --backup-interval cannot go with it.
    #[arg(
        long,
        value_name = "SIZE:TIER",
        value_parser = dedicated_disk,
        conflicts_with = "backup_interval"
    )]
    pub(crate) dedicated_disk: Vec<DedicatedDisk>,
    /// Turn on scheduled backups once the VM runs, at this many seconds between backups
    ///
    /// Stored backups are billed per GB-hour at the price hipc compute backups options shows.
    /// The server checks the interval before anything is bought; a failure to turn backups on
    /// later is the VM's backup_error.
    #[arg(long, value_name = "SECONDS", value_enum)]
    pub(crate) backup_interval: Option<BackupInterval>,
    /// The failover of those backups: auto restarts the VM on another server from its latest
    /// backup if its server dies, losing what was written after it; manual, the server's
    /// default, leaves that to Hippius support
    ///
    /// auto places the VM only on a server with a failover peer, so it is chosen at launch.
    #[arg(
        long,
        value_name = "MODE",
        value_enum,
        requires = "backup_interval",
        conflicts_with = "dedicated_disk"
    )]
    pub(crate) failover: Option<FailoverMode>,
    /// What a power-off from inside the VM does: restart (the server's default) or stop
    ///
    /// stop places the VM only on a server that supports it, so it is chosen at launch; hipc
    /// compute vms power-policy update changes it later.
    #[arg(long, value_name = "POLICY", value_enum)]
    pub(crate) on_guest_poweroff: Option<GuestPoweroff>,
    /// Wait for running even when stdout is not a terminal, CI is set, or AI_AGENT is set
    ///
    /// A run killed during the wait prints nothing on stdout; a caller that may be killed
    /// before running omits --wait and follows the VM with hipc compute vms get.
    #[arg(long, conflicts_with = "no_wait")]
    pub(crate) wait: bool,
    /// Print the launch answer and exit without waiting for running
    ///
    /// The default when stdout is not a terminal, or CI is set, or AI_AGENT is set and not off:
    /// the rule that picks JSON for --output auto.
    #[arg(long, conflicts_with = "wait_timeout")]
    pub(crate) no_wait: bool,
    /// Wait for running for this many seconds, 1 to 3600, default 300
    ///
    /// It waits as --wait does, even when stdout is not a terminal, or CI or AI_AGENT is set.
    /// A poll still in flight at the deadline can add up to --request-timeout.
    #[arg(
        long,
        value_name = "SECONDS",
        value_parser = clap::value_parser!(u64).range(1..=3600)
    )]
    pub(crate) wait_timeout: Option<u64>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum SshKeys {
    /// The stored keys
    #[command(after_help = examples(&[
        "hipc compute ssh-keys list",
    ]))]
    List(ListArgs),
    /// One stored key
    #[command(after_help = examples(&[
        "hipc compute ssh-keys get 1",
    ]))]
    Get {
        /// The key id from hipc compute ssh-keys list
        #[arg(value_parser = number(SSH_KEY_ID))]
        key_id: u64,
    },
    /// Store a public key; needs --yes
    #[command(after_help = examples(&[
        "hipc compute ssh-keys create --name laptop --public-key-file id_ed25519.pub --yes",
    ]))]
    Create {
        /// A label, 1 to 100 characters
        #[arg(long, value_parser = ssh_key_name)]
        name: String,
        /// The public key file, such as ~/.ssh/id_ed25519.pub, or - for stdin
        ///
        /// It holds one OpenSSH public key line, such as ssh-ed25519 AAAA... user@host; anything
        /// else, a private key included, is refused before anything is sent.
        #[arg(long, value_name = "PATH")]
        public_key_file: PathBuf,
    },
    /// Delete a stored key; needs --yes
    ///
    /// VMs that already have it keep it.
    #[command(after_help = examples(&[
        "hipc compute ssh-keys delete 1 --yes",
    ]))]
    Delete {
        /// The key id from hipc compute ssh-keys list
        #[arg(value_parser = number(SSH_KEY_ID))]
        key_id: u64,
    },
}

/// A placement group name, checked here because it is the caller's own label, and a bad
/// one would otherwise fail only at launch. The pattern is swagger's (`POST
/// /api/compute/vms/`, `placement_group`): 1 to 64 of `a-z`, `0-9`, and `-`. A leading `-`
/// is refused too, as for every name the caller picks: a command line reads it as a flag.
pub(crate) fn placement_group(value: &str) -> Result<String, String> {
    word(
        value,
        1..=64,
        Charset::LowerDash,
        Dash::Refused,
        "a placement group is 1 to 64 of `a-z`, `0-9`, and `-`, and does not start with `-`",
    )
}

/// A `--flavor`: a code from `hipc compute flavors`, so not empty, in lowercase, and with no
/// space or control character. The server judges whether the code exists; the catalogue's
/// codes are lowercase, and `SMALL` is refused here rather than read as some other code.
pub(crate) fn flavor(value: &str) -> Result<String, String> {
    if value.is_empty()
        || value
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c.is_uppercase())
    {
        Err(
            "a flavor is a code from `hipc compute flavors`, in lowercase, such as `small`"
                .to_owned(),
        )
    } else {
        Ok(value.to_owned())
    }
}

/// An `ssh-keys create --name`: 1 to 100 characters, as its help says.
fn ssh_key_name(value: &str) -> Result<String, String> {
    if (1..=100).contains(&value.chars().count()) {
        Ok(value.to_owned())
    } else {
        Err("an SSH key name is 1 to 100 characters".to_owned())
    }
}

/// The disk tiers of `POST /api/compute/vms/` (`os_disk_tier`, and `tier` in
/// `dedicated_disks`) and of `GET /api/compute/feasibility/` (swagger enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum DiskTier {
    /// The fastest tier
    Nvme,
    /// Slower than nvme, and cheaper
    Ssd,
    /// The slowest and cheapest: as an OS disk, slow shared storage for development and test,
    /// not for production or databases; max_flavor in hipc compute public-prices is the
    /// largest flavor it is offered on
    Hdd,
}

impl DiskTier {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Nvme => "nvme",
            Self::Ssd => "ssd",
            Self::Hdd => "hdd",
        }
    }
}

/// One `--dedicated-disk SIZE:TIER`. Swagger's `dedicated_disks` is one object, `{tier,
/// count, min_size_gb}`, so the disks of a launch share a tier and a minimum size, and the
/// repeats of the flag are its `count`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DedicatedDisk {
    /// The least size the disk has, in GiB (swagger: `min_size_gb`, ">= 1000").
    pub(crate) min_size_gb: u64,
    pub(crate) tier: DiskTier,
}

/// The least `min_size_gb` swagger accepts in `dedicated_disks`.
const MIN_DEDICATED_GB: u64 = 1000;

fn dedicated_disk(value: &str) -> Result<DedicatedDisk, String> {
    let refused = || {
        format!(
            "a dedicated disk is `SIZE:TIER`, such as `2000:hdd`: SIZE a whole number of GiB, \
             {MIN_DEDICATED_GB} or more, and TIER one of `nvme`, `ssd`, and `hdd`"
        )
    };
    let (size, tier) = value.split_once(':').ok_or_else(refused)?;
    // `u64::from_str` takes a leading `+`, which the message's "whole number" does not.
    let min_size_gb = Some(size)
        .filter(|size| size.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|size| size.parse().ok())
        .filter(|gb| *gb >= MIN_DEDICATED_GB)
        .ok_or_else(refused)?;
    let tier = DiskTier::from_str(tier, false).map_err(|_| refused())?;
    Ok(DedicatedDisk { min_size_gb, tier })
}

#[derive(Debug, Subcommand)]
pub(crate) enum PublicIp {
    /// How many public IPv4 addresses are free, overall and per region, and their price
    #[command(after_help = examples(&[
        "hipc compute public-ip availability",
        "hipc compute public-ip availability --region FR",
    ]))]
    Availability {
        /// Only this region: an ISO 3166-1 alpha-2 country code, such as FR
        #[arg(long, value_name = "CC", value_parser = region)]
        region: Option<String>,
    },
    /// The public IPv4 attached to a VM (public_ip, null when none), and its price
    #[command(after_help = examples(&[
        "hipc compute public-ip get vm-1a2b",
    ]))]
    Get {
        /// The vm_id from hipc compute vms list
        #[arg(value_parser = vm_id)]
        vm_id: String,
    },
    /// Attach a dedicated public IPv4 to a VM; spends money: needs --yes
    ///
    /// The address is billed per second at the price hipc compute public-ip availability shows:
    /// price_hourly, or free until free_until while it is set, then price_hourly_after_free. It is
    /// billed until it is detached or the VM is deleted, which releases it. All inbound traffic
    /// to it is blocked until hipc compute firewall update adds rules. Attaching again returns
    /// the address already attached.
    #[command(after_help = examples(&[
        "hipc compute public-ip create vm-1a2b --yes",
    ]))]
    Create {
        /// The vm_id from hipc compute vms list
        #[arg(value_parser = vm_id)]
        vm_id: String,
    },
    /// Detach a VM's public IPv4; needs --yes
    ///
    /// Billing for it stops. Deleting the VM releases the address too, so a VM about to be
    /// deleted needs no detach first.
    ///
    /// The address is not handed to anyone else for 24 hours, but a later attach may get a
    /// different one.
    #[command(after_help = examples(&[
        "hipc compute public-ip delete vm-1a2b --yes",
    ]))]
    Delete {
        /// The vm_id from hipc compute vms list
        #[arg(value_parser = vm_id)]
        vm_id: String,
    },
    /// Bytes in and out of a VM's public IPv4, per step, over the periods it held one
    #[command(after_help = examples(&[
        "hipc compute public-ip usage vm-1a2b",
        "hipc compute public-ip usage vm-1a2b --range 7d",
    ]))]
    Usage {
        /// The vm_id from hipc compute vms list
        #[arg(value_parser = vm_id)]
        vm_id: String,
        /// The window
        ///
        /// Without it the server uses 24h.
        #[arg(long, value_enum)]
        range: Option<UsageRange>,
    },
}

/// The windows `GET .../public-ip/usage/` accepts (swagger enum).
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum UsageRange {
    #[value(name = "1h")]
    Hour,
    #[value(name = "24h")]
    Day,
    #[value(name = "7d")]
    Week,
    #[value(name = "30d")]
    Month,
}

impl UsageRange {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Hour => "1h",
            Self::Day => "24h",
            Self::Week => "7d",
            Self::Month => "30d",
        }
    }
}
