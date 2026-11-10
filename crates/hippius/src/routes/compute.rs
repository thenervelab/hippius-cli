//! Virtual machines and their catalogs, public IPs, firewalls, exposures, backups, App
//! Engine, SSH keys, and the compute bills.

use crate::routes::{Guard, Route, RouteFlag};

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
        .guard(Guard::RefusedWhenSet("secrets"));

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
        .guard(Guard::RefusedWhenSet("secrets"));

    /// Rewrites a compose file to fit; changes nothing.
    APP_ENGINE_FIX: 0 = Route::post("/api/compute/app-engine/fix/").scoped().project();
}
