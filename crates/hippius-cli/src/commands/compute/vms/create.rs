//! `hipc compute vms create`: the launch, the ports `--open-port` opens, and the warnings
//! it ends with.

use hippius::routes::compute::{self, Vm};
use hippius::{Client, Error, Request, Result, Slot, phrase, view_of};
use serde_json::{Value, json};

use crate::cli::{DedicatedDisk, DiskTier, FailoverMode, GuestPoweroff, VmCreate};
use crate::commands::compute::backups;
use crate::commands::compute::launch::{
    Goal, Kind, STAYS_STOPPED, backups_clause, balance_hint, launch_wait, os_disk_clause, wait,
};
use crate::commands::compute::public_ip::inbound_blocked;
use crate::commands::compute::ssh_keys::read_public_key;
use crate::commands::compute::{VMS_LIST_COMMAND, refused_as};
use crate::commands::{Printed, failed_part, send_from_catalog};
use crate::context::{Confirmed, ConfirmedStep, Context};

/// `POST /api/compute/vms/`, then a wait for `running` (and `network_ready` with
/// `--public-ip`, see [`Goal`]) when [`launch_wait`] says so.
///
/// With `--public-ip` the action names the address and where its price is shown, so the
/// refusal an agent reads says the launch spends on both. The price is not read before
/// the question: the one read tenet 5 allows there names the target, and `hipc compute
/// public-ip availability` already shows the price. So a dry run and a refusal send
/// nothing, and `--yes`, where nobody reads the question, costs no extra request.
pub(super) fn create(ctx: &Context<'_>, client: &Client, args: &VmCreate) -> Result<Printed> {
    // Refused before the question, so no one confirms a launch the server would reject.
    if args.ssh_key_ids.len() > MAX_SSH_KEY_IDS {
        return Err(Error::usage(
            format!(
                "`--ssh-key-id` is given {} times, and a launch installs at most \
                 {MAX_SSH_KEY_IDS} stored keys",
                args.ssh_key_ids.len()
            ),
            format!(
                "pass at most {MAX_SSH_KEY_IDS} `--ssh-key-id`, and install more keys once the \
                 VM runs"
            ),
        ));
    }
    let dedicated = dedicated_disks(&args.dedicated_disk)?;
    let timeout = launch_wait(ctx, &args.launch);
    let public_key = args
        .ssh_public_key_file
        .as_deref()
        .map(|path| read_public_key(path, "--ssh-public-key-file"))
        .transpose()?;
    let consequence = if args.public_ip {
        "the VM is billed until it is deleted, and the address until it is detached or the VM \
         is deleted"
    } else {
        "it is billed until it is deleted"
    };
    let ports = open_ports(args);
    let request = Request::to(&compute::VM_CREATE, [])
        .json(&create_body(args, public_key.as_deref(), dedicated))
        .about(launch_action(args, dedicated, &ports));
    // A backup interval the server does not offer is refused after the question and before
    // the launch, as `backups policy update` refuses it (owner decision D19).
    let interval = args
        .launch
        .backup_interval
        .map(|seconds| seconds.to_string());
    let intervals = interval.as_deref().map(backups::intervals);
    // The launch's 503 `placement-groups-unavailable` (swagger, the `placement_group` field
    // of `POST /api/compute/vms/`) stays `partial` and not retryable, as every 5xx to a
    // write: whether the backend can record the VM before it refuses is not documented, and
    // a second launch is billed. Its hint names the list to check first.
    let step = send_from_catalog(ctx, client, intervals.as_ref(), &request, Some(consequence))
        .map_err(|error| {
            if refused_as(&error, 503, "placement-groups-unavailable") {
                error.with_hint(format!(
                    "run `{VMS_LIST_COMMAND}` to check that no VM was launched before running the \
                     command again"
                ))
            } else {
                balance_hint(error)
            }
        })?;
    let (launched, confirmed) = match step {
        ConfirmedStep::Sent(response, confirmed) => (response.document()?, confirmed),
        ConfirmedStep::DryRun(doc) if ports.is_empty() => return Ok(doc.into()),
        ConfirmedStep::DryRun(_) => {
            let rules = open_ports_request(VM_ID_STAND_IN, &ports);
            let sent: Vec<&Request> = intervals
                .iter()
                .map(|catalog| &catalog.read)
                .chain([&request, &rules])
                .collect();
            let doc = ctx.dry_run_of(client, &sent)?;
            return Ok(doc
                .with_placeholder_in_urls(VM_ID_STAND_IN, VM_ID_PLACEHOLDER)
                .into());
        }
    };
    // The rules the PUT set, added to the VM printed on success and after a wait that ends
    // early alike, so a JSON caller sees what was opened without another read.
    let firewall = (!ports.is_empty()).then(|| open_ports_rules(&ports));
    if firewall.is_some() {
        open_launched_ports(ctx, client, &confirmed, &launched, args, &ports)?;
    }
    // Kept before the wait takes the 202: a polled document that omits the field must not
    // turn a failed attach into a warning about an address that does not exist.
    let Vm {
        public_ip_error: launch_error,
        backup_error,
        ..
    } = view_of(&launched);
    // A failed attach leaves no address that could turn `network_ready`, so the wait then
    // ends at `running`.
    let goal = if args.public_ip && launch_error.is_none() {
        Goal::Reachable
    } else {
        Goal::Running
    };
    let printed = match timeout {
        Some(timeout) => wait(ctx, client, launched, Kind::Vm(goal), timeout),
        None => Ok(Printed::Document(launched)),
    };
    let printed = with_launch_firewall(printed, firewall.as_ref())?;
    if args.public_ip
        && let Printed::Document(vm) = &printed
    {
        address_warning(ctx, vm, launch_error.as_deref(), &ports);
    }
    if args.launch.backup_interval.is_some()
        && let Printed::Document(vm) = &printed
    {
        backup_warning(ctx, vm, &backup_error);
    }
    Ok(printed)
}

/// The text-mode warning a launch with `--backup-interval` ends with when its backups are not on;
/// the document is printed unchanged. Swagger's `backup_error`, on the 202 and on the VM detail, is
/// "Why the backups asked for at launch were not turned on (yet)". The printed VM decides: the
/// 202's value (`launched`) counts only when the printed VM omits the field, unlike
/// `public_ip_error`. A null there means no failure is reported yet, which does not prove the
/// backups are on: they turn on after `running`, and a later failure shows only in `hipc compute
/// vms get`. Unverified: whether a pending 202 carries a "not yet" reason, which would make every
/// `--no-wait` launch with `--backup-interval` warn.
fn backup_warning(ctx: &Context<'_>, vm: &Value, launched: &Slot<String>) {
    let printed = view_of::<Vm>(vm).backup_error;
    let error = match &printed {
        Slot::Missing => launched.value(),
        printed => printed.value(),
    };
    if let Some(error) = error {
        ctx.warning(&format!(
            "the VM launched without its scheduled backups: the API says {}",
            phrase::quote(error)
        ));
    }
}

/// The text-mode warning a launch with `--public-ip` ends with; the document is printed unchanged,
/// so JSON callers see the server's answer as sent. When the printed VM, or else the 202 launch
/// answer (`launch_error`), carries `public_ip_error` (swagger, on both: why the address could not
/// be attached, and nothing is billed for it), the VM runs without the address the caller asked
/// for, so that is the warning. Otherwise the address blocks inbound traffic, as after `public-ip
/// create`.
fn address_warning(ctx: &Context<'_>, vm: &Value, launch_error: Option<&str>, ports: &[u16]) {
    let vm: Vm = view_of(vm);
    let error = vm.public_ip_error.as_deref().or(launch_error);
    if let Some(error) = error {
        ctx.warning(&format!(
            "the VM launched without its public IPv4: the API says {}",
            phrase::quote(error)
        ));
        return;
    }
    // `--open-port` set a rule, which `open_launched_ports` noted; the rest stays closed.
    if !ports.is_empty() {
        return;
    }
    // The id comes from the server, and the warning names a command to run with it.
    let vm_id = vm
        .vm_id
        .as_deref()
        .and_then(|id| crate::cli::as_argument(id, crate::cli::VmId::parse))
        .unwrap_or("<vm_id>");
    ctx.warning(&inbound_blocked(vm_id));
}

/// What a dry run shows for the VM id in the firewall request of a launch with
/// `--open-port`: a real run takes it from the launch answer.
const VM_ID_PLACEHOLDER: &str = "<the vm_id the launch returns>";

/// The id the dry run builds the firewall URL with before it writes [`VM_ID_PLACEHOLDER`]
/// there: a word no VM id is, which the HTTP library takes.
const VM_ID_STAND_IN: &str = "vm-id-the-launch-returns";

/// The ports `--open-port` and `--open-ssh` open, each once, in ascending order.
fn open_ports(args: &VmCreate) -> Vec<u16> {
    let mut ports = args.open_ports.clone();
    if args.open_ssh {
        ports.push(22);
    }
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// The rule `--open-port` sets on a new VM: one `tcp` rule for `ports` with no
/// `source_cidrs`, which swagger reads as every source.
fn open_ports_rule(ports: &[u16]) -> Value {
    let ports: Vec<String> = ports.iter().map(u16::to_string).collect();
    json!({
        "protocol": "tcp",
        "ports": ports,
        "description": "opened at launch by hipc",
    })
}

/// The body of the PUT `--open-port` sends, as a launch prints it ([`with_firewall`]) and its
/// hint names it: the one rule of [`open_ports_rule`]. A new VM has no rules, so the PUT
/// replaces none, and it carries no `base_revision`.
fn open_ports_rules(ports: &[u16]) -> Value {
    let rule = open_ports_rule(ports);
    serde_json::to_value(compute::FirewallUpdateBody {
        rules: std::slice::from_ref(&rule),
        base_revision: None,
    })
    .expect("a firewall body is plain JSON data")
}

/// `PUT .../firewall/` of the VM `vm_id` with [`open_ports_rules`].
fn open_ports_request(vm_id: &str, ports: &[u16]) -> Request {
    Request::to(&compute::FIREWALL_UPDATE, [&vm_id])
        .listed_by(VMS_LIST_COMMAND)
        .json(&open_ports_rules(ports))
        .about(format!(
            "open {} to every IPv4 address on the VM `{vm_id}`",
            ports_named(ports)
        ))
}

/// `printed`, what a launch prints, with `firewall` added ([`with_firewall`]) to the VM on
/// success and to the VM a failed wait puts on stdout alike.
fn with_launch_firewall(printed: Result<Printed>, firewall: Option<&Value>) -> Result<Printed> {
    let Some(rules) = firewall else {
        return printed;
    };
    let mut printed = printed;
    if let Ok(Printed::Document(vm)) = &mut printed {
        with_firewall(vm, rules);
    }
    printed.map_err(|error| error.with_stdout_edited(|vm| with_firewall(vm, rules)))
}

/// Adds `firewall`, the body of the rules PUT `--open-port` sent ([`open_ports_rules`]), to the
/// VM document a launch prints. A field of that name the server already sent is kept as it
/// came.
fn with_firewall(vm: &mut Value, rules: &Value) {
    if let Some(fields) = vm.as_object_mut()
        && !fields.contains_key("firewall")
    {
        fields.insert("firewall".to_owned(), rules.clone());
    }
}

/// `the TCP port `22``, or `the TCP ports `22` and `443``.
fn ports_named(ports: &[u16]) -> String {
    format!(
        "the TCP {} {}",
        phrase::plural(ports.len() as u64, "port"),
        phrase::ticked(ports, "and")
    )
}

/// Sets the rules `--open-port` asked for on the VM the launch answer `launched` names, right
/// after the launch, under the launch's confirmation. The launch already went through, so any
/// failure here prints the VM on stdout ([`hippius::Error::after_write`]): an answer with no
/// usable `vm_id` is `internal`, and a refused or failed PUT is `partial_failure`, whose hint
/// is the firewall update that sets the same rules. The VM keeps running with every port
/// closed until then.
fn open_launched_ports(
    ctx: &Context<'_>,
    client: &Client,
    confirmed: &Confirmed,
    launched: &Value,
    args: &VmCreate,
    ports: &[u16],
) -> Result<()> {
    let public_ip = args.public_ip;
    // The id goes into the PUT's path, so it is held to the rule of the `vm_id` argument; a
    // leading `-` is fine there, and only matters to a printed command (the hint below).
    let vm_id = view_of::<Vm>(launched)
        .vm_id
        .filter(|id| crate::cli::VmId::parse(id).is_ok());
    let Some(vm_id) = vm_id.as_deref() else {
        return Err(Error::internal(
            "the launch answer has no usable `vm_id`, so the firewall rule was not set",
        )
        .with_hint(format!(
            "run `{VMS_LIST_COMMAND}` to find the new VM, then open its ports with `hipc \
             compute firewall update`"
        ))
        .after_write(launched.clone()));
    };
    let request = open_ports_request(vm_id, ports);
    match ctx.send_confirmed(client, &request, confirmed) {
        Ok(_) => {
            let later = if public_ip {
                ""
            } else {
                "; the VM has no public IPv4 yet, and the rule applies once `hipc compute \
                 public-ip create` attaches one"
            };
            ctx.note(&format!(
                "opened {} to every IPv4 address on the VM `{vm_id}`{later}",
                ports_named(ports)
            ));
            Ok(())
        }
        Err(error) => {
            let rules = open_ports_rules(ports);
            let mut part = failed_part("step", "firewall", &error);
            part["rules"] = rules;
            Err(Error::partial_failure(
                format!(
                    "launched the VM `{vm_id}`, but could not open {}: {}",
                    ports_named(ports),
                    error.message()
                ),
                launched.clone(),
                Value::Array(vec![part]),
                hippius::SideEffects::Partial,
            )
            .with_hint(format!(
                "run `{}` with `{}` on stdin",
                crate::cli::with_id("compute firewall update", vm_id, "--rules-file - --yes"),
                open_ports_rules(ports)
            )))
        }
    }
}

/// The most stored keys one launch installs (swagger, `ssh_key_ids` of `POST
/// /api/compute/vms/`).
const MAX_SSH_KEY_IDS: usize = 20;

/// The dedicated disks of one launch (swagger: `dedicated_disks` of `POST
/// /api/compute/vms/`, one object `{tier, count, min_size_gb}`).
#[derive(Debug, Clone, Copy)]
struct Dedicated {
    disk: DedicatedDisk,
    /// 1 to [`MAX_DEDICATED_DISKS`].
    count: u64,
}

/// The most dedicated disks one launch takes (swagger: `count` of `dedicated_disks`, 1-8).
const MAX_DEDICATED_DISKS: usize = 8;

/// The repeated `--dedicated-disk` as swagger's one object, or `None` when none is given.
/// The repeats are the `count`, so they must agree on size and tier; more than
/// [`MAX_DEDICATED_DISKS`] is refused here, so a launch the server would reject is not sent
/// after the confirmation.
fn dedicated_disks(disks: &[DedicatedDisk]) -> Result<Option<Dedicated>> {
    let Some(&disk) = disks.first() else {
        return Ok(None);
    };
    if disks.len() > MAX_DEDICATED_DISKS {
        return Err(Error::usage(
            format!(
                "`--dedicated-disk` is given {} times, and a launch takes at most \
                 {MAX_DEDICATED_DISKS} dedicated disks",
                disks.len()
            ),
            format!("pass at most {MAX_DEDICATED_DISKS} `--dedicated-disk`"),
        ));
    }
    if disks.iter().any(|other| *other != disk) {
        return Err(Error::usage(
            "the `--dedicated-disk` values differ, and a launch's dedicated disks share one \
             size and tier",
            "repeat `--dedicated-disk` with the same size and tier, once per disk",
        ));
    }
    Ok(Some(Dedicated {
        disk,
        count: disks.len() as u64,
    }))
}

/// What the launch does, for the question and the refusal: the flavor, then one clause per
/// option that bills more, each naming where its price is shown. `hipc` holds no price and
/// reads none before the question: a dry run and a refusal send nothing.
fn launch_action(args: &VmCreate, dedicated: Option<Dedicated>, ports: &[u16]) -> String {
    let mut clauses = Vec::new();
    if args.public_ip {
        clauses.push(
            "a public IPv4, billed per second at the price that `hipc compute public-ip \
             availability` shows (`price_hourly`, or free until `free_until` while it is set, \
             then `price_hourly_after_free`)"
                .to_owned(),
        );
    }
    if let Some(tier) = args.launch.os_disk_tier {
        clauses.push(os_disk_clause(tier));
    }
    if let Some(Dedicated { disk, count }) = dedicated {
        clauses.push(format!(
            "{} of at least {} GiB on tier `{}` or faster, each billed per GiB-hour at its size \
             class (floored to 100 GiB), while the VM is stopped too, at the price that `hipc \
             compute prices` shows (`dedicated_disks`); the disks pin the VM to its server: no \
             backup, migration, or failover",
            phrase::count(count, "dedicated disk"),
            disk.min_size_gb,
            disk.tier.code()
        ));
    }
    if let Some(interval) = args.launch.backup_interval {
        clauses.push(backups_clause(interval, args.launch.failover));
    }
    if args.launch.on_guest_poweroff == Some(GuestPoweroff::Stop) {
        clauses.push(STAYS_STOPPED.to_owned());
    }
    if !ports.is_empty() {
        clauses.push(format!("{} open to every IPv4 address", ports_named(ports)));
    }
    let launch = format!("launch a VM of flavor {}", phrase::quote(&args.flavor));
    if clauses.is_empty() {
        launch
    } else {
        format!("{launch} with {}", clauses.join("; with "))
    }
}

/// The create body: exactly the flags given, nothing defaulted. `public_key` is
/// the checked text of `--ssh-public-key-file`; swagger installs it beside the
/// `ssh_key_ids` keys when both are given.
fn create_body<'a>(
    args: &'a VmCreate,
    public_key: Option<&'a str>,
    dedicated: Option<Dedicated>,
) -> compute::VmCreateBody<'a> {
    compute::VmCreateBody {
        flavor: &args.flavor,
        name: args.name.as_deref(),
        image: args.image.as_deref(),
        region: args.region.as_deref(),
        placement_group: args.launch.placement_group.as_deref(),
        ssh_key_ids: &args.ssh_key_ids,
        ssh_public_key: public_key,
        public_ip: args.public_ip,
        os_disk_tier: args.launch.os_disk_tier.map(DiskTier::code),
        dedicated_disks: dedicated.map(|Dedicated { disk, count }| compute::DedicatedDisks {
            tier: disk.tier.code(),
            count,
            min_size_gb: disk.min_size_gb,
        }),
        // Swagger: `dedicated_disks` needs `pinned_ack: true` (422 `dedicated-ack-required`),
        // the customer's acknowledgement that the VM is pinned to its server. The question,
        // or `--yes`, is that acknowledgement, and the action names the pinning.
        // `pinned_ack_version` is not sent: only a read of `GET /api/compute/prices/` gives
        // it, and swagger refuses only a stale one (422 `dedicated-ack-stale`).
        pinned_ack: dedicated.is_some(),
        backup_interval_s: args.launch.backup_interval,
        failover_mode: args.launch.failover.map(FailoverMode::code),
        on_guest_poweroff: args.launch.on_guest_poweroff.map(GuestPoweroff::code),
    }
}
