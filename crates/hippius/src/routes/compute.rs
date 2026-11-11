//! Virtual machines and their catalogs, public IPs, firewalls, exposures, backups, App
//! Engine, SSH keys, and the compute bills.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::routes::{Guard, Route, RouteFlag};
use crate::view::{Slot, each, field, nested};

/// Why `hipc api` refuses an App Engine launch or compose update whose body sets `secrets`.
pub(crate) const APP_SECRETS: &str = "the body holds the app's secret values";

routes! {
    /// The compute prices for the caller.
    PRICES: 0 = Route::get("/api/compute/prices/").scoped().project();

    /// The public compute prices.
    PUBLIC_PRICES: 0 = Route::get("/api/compute/public-prices/").scoped().project();

    /// The compute quota.
    QUOTA: 0 = Route::get("/api/compute/quota/").scoped().project();

    /// The compute charge cap.
    CHARGE_CAP: 0 = Route::get("/api/compute/charge-cap/").scoped().project();

    /// The VM flavors.
    FLAVORS: 0 = Route::get("/api/compute/flavors/").scoped().project();

    /// The VM images.
    IMAGES: 0 = Route::get("/api/compute/images/").scoped().project();

    /// The one-click apps.
    APPS: 0 = Route::get("/api/compute/apps/").scoped().project();

    /// The compute regions.
    REGIONS: 0 = Route::get("/api/compute/regions/").scoped().project();

    /// The placement groups.
    PLACEMENT_GROUPS: 0 = Route::get("/api/compute/placement-groups/").scoped().project();

    /// Whether a launch would fit.
    FEASIBILITY: 0 = Route::get("/api/compute/feasibility/").scoped().project();

    /// The SSH keys a VM of the account can take.
    ACCOUNT_SSH_KEYS: 0 = Route::get("/api/compute/account-ssh-keys/").scoped().project();

    /// The backup intervals on offer.
    BACKUP_OPTIONS: 0 = Route::get("/api/compute/backup-options/").scoped().project();

    /// Whether a public IP is free to attach.
    PUBLIC_IP_AVAILABILITY: 0 = Route::get("/api/compute/public-ip/availability/")
        .scoped()
        .project();

    /// The compute usage of a month.
    USAGE: 0 = Route::get("/api/compute/usage/").scoped().billing();

    /// The compute burn rate.
    BILLING_SUMMARY: 0 = Route::get("/api/compute/billing/summary/").scoped().billing();

    /// The VMs.
    VMS: 0 = Route::get("/api/compute/vms/").scoped().project();

    /// Launches a VM.
    VM_CREATE: 0 = Route::post("/api/compute/vms/").scoped().project();

    /// One VM.
    VM: 1 = Route::get("/api/compute/vms/{vm_id}/").scoped().project();

    /// Deletes a VM.
    VM_DELETE: 1 = Route::delete("/api/compute/vms/{vm_id}/").scoped().project();

    /// Starts a VM.
    VM_START: 1 = Route::post("/api/compute/vms/{vm_id}/start/").scoped().project();

    /// Stops a VM.
    VM_STOP: 1 = Route::post("/api/compute/vms/{vm_id}/stop/").scoped().project();

    /// Reboots a VM.
    VM_REBOOT: 1 = Route::post("/api/compute/vms/{vm_id}/reboot/").scoped().project();

    /// Resizes a VM.
    VM_RESIZE: 1 = Route::post("/api/compute/vms/{vm_id}/resize/").scoped().project();

    /// The flavors a VM can resize to.
    VM_RESIZE_OPTIONS: 1 = Route::get("/api/compute/vms/{vm_id}/resize/").scoped().project();

    /// One resize of a VM.
    VM_RESIZE_STATUS: 2 = Route::get("/api/compute/vms/{vm_id}/resize/{resize_id}/")
        .scoped()
        .project();

    /// A VM's attestation.
    VM_ATTESTATION: 1 = Route::get("/api/compute/vms/{vm_id}/attestation/").scoped().project();

    /// A VM's failovers.
    VM_FAILOVERS: 1 = Route::get("/api/compute/vms/{vm_id}/failovers/").scoped().project();

    /// Sets what a guest power-off does.
    VM_POWER_POLICY_UPDATE: 1 = Route::put("/api/compute/vms/{vm_id}/power-policy/")
        .scoped()
        .project();

    /// A VM's public IP.
    PUBLIC_IP: 1 = Route::get("/api/compute/vms/{vm_id}/public-ip/").scoped().project();

    /// Attaches a public IP.
    PUBLIC_IP_CREATE: 1 = Route::post("/api/compute/vms/{vm_id}/public-ip/").scoped().project();

    /// Detaches a public IP.
    PUBLIC_IP_DELETE: 1 = Route::delete("/api/compute/vms/{vm_id}/public-ip/").scoped().project();

    /// A public IP's traffic.
    PUBLIC_IP_USAGE: 1 = Route::get("/api/compute/vms/{vm_id}/public-ip/usage/").scoped().project();

    /// A VM's firewall rules.
    FIREWALL: 1 = Route::get("/api/compute/vms/{vm_id}/firewall/").scoped().project();

    /// Replaces a VM's firewall rules.
    FIREWALL_UPDATE: 1 = Route::put("/api/compute/vms/{vm_id}/firewall/")
        .scoped()
        .project()
        .guard(Guard::Needs(&[RouteFlag::RemoveOthers]));

    /// A VM's exposed ports.
    EXPOSURES: 1 = Route::get("/api/compute/vms/{vm_id}/expose/").scoped().project();

    /// Exposes a port.
    EXPOSURE_CREATE: 1 = Route::post("/api/compute/vms/{vm_id}/expose/").scoped().project();

    /// Removes an exposure.
    EXPOSURE_DELETE: 2 = Route::delete("/api/compute/vms/{vm_id}/expose/{exposure_id}/")
        .scoped()
        .project();

    /// A VM's backup policy.
    BACKUP_POLICY: 1 = Route::get("/api/compute/vms/{vm_id}/backup/").scoped().project();

    /// Sets a VM's backup policy.
    BACKUP_POLICY_UPDATE: 1 = Route::put("/api/compute/vms/{vm_id}/backup/").scoped().project();

    /// Turns a VM's backups off.
    BACKUP_POLICY_DELETE: 1 = Route::delete("/api/compute/vms/{vm_id}/backup/").scoped().project();

    /// A VM's backups.
    BACKUPS: 1 = Route::get("/api/compute/vms/{vm_id}/backups/").scoped().project();

    /// Restores a VM from a backup.
    BACKUP_RESTORE: 1 = Route::post("/api/compute/vms/{vm_id}/backups/restore/").scoped().project();

    /// A VM's restore.
    BACKUP_RESTORE_STATUS: 1 = Route::get("/api/compute/vms/{vm_id}/backups/restore/")
        .scoped()
        .project();

    /// The caller's SSH keys.
    SSH_KEYS: 0 = Route::get("/api/ssh-keys/").personal().paged();

    /// One SSH key.
    SSH_KEY: 1 = Route::get("/api/ssh-keys/{key_id}/").personal();

    /// Adds an SSH key.
    SSH_KEY_CREATE: 0 = Route::post("/api/ssh-keys/").personal();

    /// Removes an SSH key.
    SSH_KEY_DELETE: 1 = Route::delete("/api/ssh-keys/{key_id}/").personal();

    /// The App Engine limits.
    APP_ENGINE_INFO: 0 = Route::get("/api/compute/app-engine/info/").scoped().project();

    /// The App Engine apps.
    APP_ENGINE_APPS: 0 = Route::get("/api/compute/app-engine/apps/").scoped().project();

    /// Launches an App Engine app.
    APP_ENGINE_APP_CREATE: 0 = Route::post("/api/compute/app-engine/apps/")
        .scoped()
        .project()
        .guard(Guard::RefusedWhenSet {
            key: "secrets",
            why: APP_SECRETS,
            hint: "run `hipc compute app-engine apps create --secrets-file`, which reads the \
                   values from a private file and keeps them out of every output",
        });

    /// One App Engine app.
    APP_ENGINE_APP: 1 = Route::get("/api/compute/app-engine/apps/{vm_id}/").scoped().project();

    /// An app's runtime state.
    APP_ENGINE_RUNTIME: 1 = Route::get("/api/compute/app-engine/apps/{vm_id}/runtime/")
        .scoped()
        .project();

    /// An app's guest components.
    APP_ENGINE_GUEST_COMPONENTS: 1 = Route::get("/api/compute/app-engine/apps/{vm_id}/guest-components/")
        .scoped()
        .project();

    /// An app's guest upgrade status.
    APP_ENGINE_GUEST_UPGRADE: 1 = Route::get("/api/compute/app-engine/apps/{vm_id}/guest-upgrade/")
        .scoped()
        .project();

    /// Checks a compose file; changes nothing.
    APP_ENGINE_VALIDATE: 0 = Route::post("/api/compute/app-engine/validate/")
        .scoped()
        .project()
        .guard(Guard::RefusedWhenSet {
            key: "secrets",
            why: "the body holds secret values, and a compose check needs none",
            hint: "run `hipc compute app-engine validate`, which sends the compose alone",
        });

    /// Rewrites a compose file to fit; changes nothing.
    APP_ENGINE_FIX: 0 = Route::post("/api/compute/app-engine/fix/").scoped().project();

    /// Replaces an app's compose file. No command sends it yet; with secrets in the body it is
    /// refused, since only a private file may carry them.
    APP_ENGINE_COMPOSE_UPDATE: 1 = Route::put("/api/compute/app-engine/apps/{vm_id}/compose/")
        .scoped()
        .project()
        .guard(Guard::RefusedWhenSet {
            key: "secrets",
            why: APP_SECRETS,
            hint: "a compose update with secrets is not in this version of `hipc`: send the \
                   body without `secrets`, or change the app's secrets in the console",
        });
}

/// The body of [`VM_CREATE`]: exactly the flags given, nothing defaulted.
#[derive(Debug, Serialize)]
pub struct VmCreateBody<'a> {
    /// The flavor's name.
    pub flavor: &'a str,
    /// The VM's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<&'a str>,
    /// The image's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<&'a str>,
    /// The region's code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<&'a str>,
    /// The placement group the VM joins.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placement_group: Option<&'a str>,
    /// The stored SSH keys installed on the VM, at most 20; left out when there are none.
    #[serde(skip_serializing_if = "<[u64]>::is_empty")]
    pub ssh_key_ids: &'a [u64],
    /// A public key installed beside `ssh_key_ids`, one line in the OpenSSH format.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_public_key: Option<&'a str>,
    /// Whether a public IPv4 is attached; left out when it is not.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub public_ip: bool,
    /// The slowest tier the OS disk may be on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_disk_tier: Option<&'a str>,
    /// The dedicated disks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dedicated_disks: Option<DedicatedDisks<'a>>,
    /// The acknowledgement that dedicated disks pin the VM to its server, which swagger
    /// requires with `dedicated_disks` (422 `dedicated-ack-required`); left out without them.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pinned_ack: bool,
    /// How often the VM is backed up, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_interval_s: Option<u32>,
    /// What the backups do when the VM's server dies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failover_mode: Option<&'a str>,
    /// What a power-off from inside the VM does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_guest_poweroff: Option<&'a str>,
}

/// The `dedicated_disks` of a [`VmCreateBody`]: `count` disks of one size and tier.
#[derive(Debug, Serialize)]
pub struct DedicatedDisks<'a> {
    /// The slowest tier the disks may be on.
    pub tier: &'a str,
    /// How many disks, 1 to 8.
    pub count: u64,
    /// The least size of each disk, in GiB.
    pub min_size_gb: u64,
}

/// The body of [`VM_RESIZE`].
#[derive(Debug, Serialize)]
pub struct VmResizeBody<'a> {
    /// The flavor the VM resizes to.
    pub flavor: &'a str,
}

/// The body of [`VM_POWER_POLICY_UPDATE`].
#[derive(Debug, Serialize)]
pub struct PowerPolicyBody<'a> {
    /// What a power-off from inside the VM does: `restart` or `stop`.
    pub on_guest_poweroff: &'a str,
}

/// The body of [`FIREWALL_UPDATE`]: the whole rule list, which replaces the VM's.
#[derive(Debug, Serialize)]
pub struct FirewallUpdateBody<'a> {
    /// The rules, each sent as the rules file holds it.
    pub rules: &'a [Value],
    /// The `revision` of the rules this list replaces, so a list another client saved since
    /// is refused (409 `firewall-changed`) rather than written over.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_revision: Option<&'a str>,
}

/// The body of [`EXPOSURE_CREATE`]: the port, and `mode` and `protocol` only when given.
#[derive(Debug, Serialize)]
pub struct ExposureCreateBody<'a> {
    /// The VM's port.
    pub port: u16,
    /// How the edge forwards the traffic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<&'a str>,
    /// `tcp` or `udp`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<&'a str>,
}

/// The answer of [`BACKUP_OPTIONS`], as far as `hipc` decides on it: the intervals a backup
/// policy may take.
#[derive(Debug, Default, Deserialize)]
pub struct BackupOptions {
    /// The intervals offered; `None` when the answer has no list of them.
    #[serde(default, deserialize_with = "each")]
    pub intervals: Option<Vec<BackupInterval>>,
}

/// One interval of [`BackupOptions`].
#[derive(Debug, Default, Deserialize)]
pub struct BackupInterval {
    /// The seconds between backups, which a policy sends as `interval_s`.
    #[serde(default, deserialize_with = "field")]
    pub seconds: Option<u64>,
}

/// The body of [`BACKUP_POLICY_UPDATE`]: `failover_mode` only when given, since swagger says
/// one left out keeps the mode in force.
#[derive(Debug, Serialize)]
pub struct BackupPolicyBody<'a> {
    /// How often the VM is backed up, in seconds.
    pub interval_s: u32,
    /// What the backups do when the VM's server dies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failover_mode: Option<&'a str>,
}

/// The body of [`BACKUP_RESTORE`].
#[derive(Debug, Serialize)]
pub struct BackupRestoreBody<'a> {
    /// The backup restored.
    pub point_id: &'a str,
    /// How the backup is restored.
    pub mode: RestoreMode,
    /// The VM's `name`, exactly: the server's own check of what is overwritten.
    pub confirm: &'a str,
    /// Whether a point from before the VM's last reboot may be restored; left out when not.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub accept_rollback: bool,
}

/// How [`BACKUP_RESTORE`] restores a backup. Swagger: "Only `mode: in_place` is available".
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreMode {
    /// Over the VM's own disk.
    InPlace,
}

/// The body of [`SSH_KEY_CREATE`].
#[derive(Debug, Serialize)]
pub struct SshKeyCreateBody<'a> {
    /// The key's name.
    pub name: &'a str,
    /// The public key, one line in the OpenSSH format.
    pub public_key: &'a str,
}

/// The body of [`APP_ENGINE_VALIDATE`] and [`APP_ENGINE_FIX`]: the compose, byte for byte.
#[derive(Debug, Serialize)]
pub struct ComposeBody<'a> {
    /// The compose file's text, unchanged.
    pub compose: &'a str,
}

/// The body of [`APP_ENGINE_APP_CREATE`]: the flavor, the compose byte for byte, `secrets`
/// when there are any, and exactly the flags given. `public_ip` is never sent: the server's
/// default, true, is the only value it takes. Its `Debug` leaves `secrets` out.
#[derive(Serialize)]
pub struct AppCreateBody<'a> {
    /// The flavor's name.
    pub flavor: &'a str,
    /// The compose file's text, unchanged.
    pub compose: &'a str,
    /// The secret values by name, or their placeholders in a dry run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secrets: Option<&'a Value>,
    /// The app's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<&'a str>,
    /// The region's code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<&'a str>,
    /// The slowest tier the OS disk may be on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_disk_tier: Option<&'a str>,
    /// The placement group the app's VM joins.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placement_group: Option<&'a str>,
    /// What a power-off from inside the VM does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_guest_poweroff: Option<&'a str>,
    /// What the backups do when the VM's server dies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failover_mode: Option<&'a str>,
    /// How often the VM is backed up, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_interval_s: Option<u32>,
}

/// Leaves `secrets` out: it holds the values sent.
impl std::fmt::Debug for AppCreateBody<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppCreateBody")
            .field("flavor", &self.flavor)
            .field("compose_bytes", &self.compose.len())
            .field("secrets", &self.secrets.map(|_| "<withheld>"))
            .finish_non_exhaustive()
    }
}

/// A VM or an App Engine app, as far as a launch, a delete, a restore, and the power policy
/// decide on it.
#[derive(Debug, Default, Deserialize)]
#[expect(
    clippy::struct_field_names,
    reason = "each field is named as the API names it"
)]
pub struct Vm {
    /// The VM's id.
    #[serde(default, deserialize_with = "field")]
    pub vm_id: Option<String>,
    /// The VM's name.
    #[serde(default, deserialize_with = "field")]
    pub name: Option<String>,
    /// Where the VM stands.
    #[serde(default, deserialize_with = "field")]
    pub status: Option<VmStatus>,
    /// Swagger: a `failed` VM whose boot stalled "returns to `running` by itself once the
    /// guest signals".
    #[serde(default, deserialize_with = "field")]
    pub boot_stalled: Option<bool>,
    /// Whether the internet path to the public IPv4 is open.
    #[serde(default, deserialize_with = "field")]
    pub network_ready: Option<bool>,
    /// Why the public IPv4 asked for could not be attached.
    #[serde(default, deserialize_with = "field")]
    pub public_ip_error: Option<String>,
    /// Why the backups asked for at launch were not turned on (yet): a decision tells a VM
    /// that leaves the field out from one that holds `null`.
    #[serde(default)]
    pub backup_error: Slot<String>,
    /// What a power-off from inside the VM does.
    #[serde(default, deserialize_with = "field")]
    pub on_guest_poweroff: Option<String>,
    /// Whether the VM's server has not yet applied `on_guest_poweroff`.
    #[serde(default, deserialize_with = "field")]
    pub on_guest_poweroff_pending: Option<bool>,
    /// Why `on_guest_poweroff` is pending.
    #[serde(default, deserialize_with = "field")]
    pub on_guest_poweroff_reason: Option<String>,
    /// The boot phase of an App Engine app.
    #[serde(default, deserialize_with = "field")]
    pub boot_phase: Option<BootPhase>,
    /// The SHA-256 of the compose an App Engine app launched with: a decision tells an answer
    /// that names none from one that names another.
    #[serde(default)]
    pub compose_sha256: Slot<String>,
}

vocabulary! {
    /// The `status` of a VM, of the values a launch wait acts on.
    VmStatus {
        /// Up.
        Running = "running",
        /// Failed: terminal unless `boot_stalled`.
        Failed = "failed",
        /// Being torn down.
        Decommissioning = "decommissioning",
        /// Torn down.
        Decommissioned = "decommissioned",
    }
}

vocabulary! {
    /// The `boot_phase` of an App Engine app, of the values a launch wait acts on.
    BootPhase {
        /// The compose runs.
        Running = "running",
        /// The boot failed.
        Failed = "failed",
    }
}

/// The answer of [`VM_RESIZE`], as far as the note that follows it decides on it.
#[derive(Debug, Default, Deserialize)]
pub struct VmResize {
    /// The resize started.
    #[serde(default, deserialize_with = "nested")]
    pub resize: Option<Resize>,
}

/// One resize of a VM.
#[derive(Debug, Default, Deserialize)]
pub struct Resize {
    /// The resize's id, which [`VM_RESIZE_STATUS`] takes.
    #[serde(default, deserialize_with = "field")]
    pub id: Option<u64>,
}

/// An exposure of [`EXPOSURE_CREATE`], as far as its warning decides on it.
#[derive(Debug, Default, Deserialize)]
pub struct Exposure {
    /// Whether the edge can read the traffic.
    #[serde(default, deserialize_with = "field")]
    pub edge_sees_traffic: Option<bool>,
    /// The hostname and port the exposure serves on.
    #[serde(default, deserialize_with = "field")]
    pub endpoint: Option<String>,
}

/// A compute error body, as far as a hint decides on it. Swagger names the error slugs in
/// prose but not the field that carries them, and types compute error bodies as `{error}`, so
/// both `error` and `code` are read.
#[derive(Debug, Default, Deserialize)]
pub struct Refusal {
    /// The slug, such as `rollback-not-accepted`.
    #[serde(default, deserialize_with = "field")]
    pub error: Option<String>,
    /// The slug, as some routes name it.
    #[serde(default, deserialize_with = "field")]
    pub code: Option<String>,
}

impl Refusal {
    /// Whether either field is `slug`.
    pub fn is(&self, slug: &str) -> bool {
        [&self.code, &self.error]
            .into_iter()
            .any(|said| said.as_deref() == Some(slug))
    }
}

/// The App Engine limits of [`APP_ENGINE_INFO`], as far as its warning decides on it.
#[derive(Debug, Default, Deserialize)]
pub struct AppEngineInfo {
    /// `false` when the account may not use App Engine.
    #[serde(default, deserialize_with = "field")]
    pub enabled: Option<bool>,
    /// Why it may not.
    #[serde(default, deserialize_with = "field")]
    pub reason: Option<String>,
}

/// An app of [`APP_ENGINE_APP`], as far as its warning decides on it.
#[derive(Debug, Default, Deserialize)]
pub struct App {
    /// What proves what the app runs.
    #[serde(default, deserialize_with = "nested")]
    pub evidence: Option<Evidence>,
}

/// The `evidence` of an [`App`].
#[derive(Debug, Default, Deserialize)]
pub struct Evidence {
    /// A shell command the server wrote, which checks the evidence.
    #[serde(default)]
    pub verify_command: Slot<Value>,
}

/// The guest components of [`APP_ENGINE_GUEST_COMPONENTS`], as far as its warning decides on
/// it.
#[derive(Debug, Default, Deserialize)]
pub struct GuestComponents {
    /// Whether an earlier upgrade failed, and Hippius must recover the VM.
    #[serde(default, deserialize_with = "field")]
    pub needs_operator: Option<bool>,
}

/// The guest upgrade of [`APP_ENGINE_GUEST_UPGRADE`], as far as its note decides on it.
#[derive(Debug, Default, Deserialize)]
pub struct GuestUpgrade {
    /// The last upgrade job; `null` when the app was never upgraded.
    #[serde(default)]
    pub job: Slot<Value>,
}

/// The answer of [`APP_ENGINE_VALIDATE`], as far as a launch decides on it.
#[derive(Debug, Default, Deserialize)]
pub struct Validated {
    /// The secrets the compose declares, by name.
    #[serde(default)]
    pub secret_names: Slot<Vec<String>>,
}

/// The answer of [`APP_ENGINE_FIX`], as far as its file and its warnings decide on it.
#[derive(Debug, Default, Deserialize)]
#[expect(
    clippy::struct_field_names,
    reason = "each field is named as the API names it"
)]
pub struct Fixed {
    /// The rewritten compose.
    #[serde(default, deserialize_with = "field")]
    pub fixed_compose: Option<String>,
    /// Whether `fixed_compose` is valid.
    #[serde(default, deserialize_with = "field")]
    pub valid: Option<bool>,
    /// What the fix changed.
    #[serde(default, deserialize_with = "each")]
    pub changes: Option<Vec<Change>>,
}

/// One change of a [`Fixed`] compose.
#[derive(Debug, Default, Deserialize)]
pub struct Change {
    /// Whether the change alters what runs.
    #[serde(default, deserialize_with = "field")]
    pub semantic: Option<bool>,
    /// The line it is at.
    #[serde(default, deserialize_with = "field")]
    pub line: Option<u64>,
    /// What it does, in the server's words.
    #[serde(default, deserialize_with = "field")]
    pub description: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::view::view_of;

    /// Each status and boot phase the wait acts on reads as its variant and back; another is
    /// `Other`, not an error.
    #[test]
    fn a_status_the_server_adds_is_other() {
        for status in [
            "running",
            "failed",
            "decommissioning",
            "decommissioned",
            "pending",
        ] {
            let read: Vm = view_of(&json!({ "status": status, "boot_phase": status }));
            let (read, phase) = (read.status.unwrap(), read.boot_phase.unwrap());
            assert_eq!((read.as_str(), phase.as_str()), (status, status));
            assert_eq!(matches!(read, VmStatus::Other(_)), status == "pending");
            let known = matches!(status, "running" | "failed");
            assert_eq!(matches!(phase, BootPhase::Other(_)), !known);
        }
    }

    /// A launch body sends exactly the fields given, in the order the wire carries them, and
    /// `pinned_ack` and `public_ip` only when true.
    #[test]
    fn a_launch_body_sends_only_what_is_given() {
        let bare = VmCreateBody {
            flavor: "s",
            name: None,
            image: None,
            region: None,
            placement_group: None,
            ssh_key_ids: &[],
            ssh_public_key: None,
            public_ip: false,
            os_disk_tier: None,
            dedicated_disks: None,
            pinned_ack: false,
            backup_interval_s: None,
            failover_mode: None,
            on_guest_poweroff: None,
        };
        assert_eq!(serde_json::to_string(&bare).unwrap(), r#"{"flavor":"s"}"#);
        let full = VmCreateBody {
            ssh_key_ids: &[3],
            public_ip: true,
            dedicated_disks: Some(DedicatedDisks {
                tier: "nvme",
                count: 2,
                min_size_gb: 1000,
            }),
            pinned_ack: true,
            backup_interval_s: Some(900),
            ..bare
        };
        assert_eq!(
            serde_json::to_string(&full).unwrap(),
            r#"{"flavor":"s","ssh_key_ids":[3],"public_ip":true,"dedicated_disks":{"tier":"nvme","count":2,"min_size_gb":1000},"pinned_ack":true,"backup_interval_s":900}"#
        );
        let restore = BackupRestoreBody {
            point_id: "p",
            mode: RestoreMode::InPlace,
            confirm: "web",
            accept_rollback: false,
        };
        assert_eq!(
            serde_json::to_string(&restore).unwrap(),
            r#"{"point_id":"p","mode":"in_place","confirm":"web"}"#
        );
    }

    /// The `Debug` of an app launch body never shows a secret value.
    #[test]
    fn an_app_body_debug_shows_no_secret() {
        let secrets = json!({ "TOKEN": "hunter2" });
        let body = AppCreateBody {
            flavor: "s",
            compose: "services: {}",
            secrets: Some(&secrets),
            name: None,
            region: None,
            os_disk_tier: None,
            placement_group: None,
            on_guest_poweroff: None,
            failover_mode: None,
            backup_interval_s: None,
        };
        let shown = format!("{body:?}");
        assert!(!shown.contains("hunter2"), "{shown}");
        assert!(shown.contains("<withheld>"), "{shown}");
    }

    /// A refusal names its slug in `error` or in `code`.
    #[test]
    fn a_refusal_slug_is_read_from_either_field() {
        for body in [json!({"error": "x-y"}), json!({"code": "x-y"})] {
            assert!(view_of::<Refusal>(&body).is("x-y"), "{body}");
        }
        assert!(!view_of::<Refusal>(&json!({"detail": "x-y"})).is("x-y"));
    }
}
