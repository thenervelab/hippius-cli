//! `hipc compute`: the VM catalog, VMs, and stored SSH keys.
//!
//! The compute routes are account-scoped, so a membership sends `X-Hippius-Account`.
//! `/api/ssh-keys/` is personal: the header is never sent there, and a membership still
//! sends the request. The owner's keys, `/api/compute/account-ssh-keys/`, are under compute
//! and scoped like the rest of it. `hipc` opens no SSH session, and a launch exposes no port:
//! `exposures create` does, as its own confirmed command. A launch attaches a public IPv4
//! only with `--public-ip`; `public-ip create` attaches one to a VM that exists.
//!
//! Managed databases live in `compute/databases.rs`: the console and the grants group them
//! with compute, though their routes are under `/api/databases/`.
//!
//! Placement groups are read here and named at launch. There is no route to create or
//! delete one: a group exists while a live VM names it.
//!
//! GitHub Actions runners are in [`runners`], managed Kubernetes in [`kubernetes`], and App
//! Engine in [`app_engine`].

mod app_engine;

pub(crate) mod runners;

mod databases;

mod firewall;

mod kubernetes;

use std::path::Path;
use std::time::{Duration, Instant};

use hippius::{Client, Code, Error, Method, Request, Response, Result, phrase};
use serde_json::{Map, Value, json};

use crate::cli::{
    BackupInterval, BackupPolicy, BackupPolicyUpdate, BackupRestore, Backups, Compute,
    DedicatedDisk, ExposeMode, ExposeProtocol, ExposureCreate, Exposures, FailoverMode, Firewall,
    GuestPoweroff, PowerPolicy, PublicIp, SshKeys, VmCreate, Vms,
};
use crate::commands::poll::{Next, Schedule, Wait, poll_until};
use crate::commands::{
    Input, Printed, failed_part, keyed_list, list, print_body, print_body_or, print_envelope,
    print_keyed_list, print_list_with, print_with,
};
use crate::context::{self, Context, Gate, Step, Write};
use crate::output::Mode;

/// How often a launch is polled, 2 seconds: a launch takes on the order of a
/// minute, so a shorter interval only adds requests.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) fn run(ctx: &Context<'_>, command: &Compute) -> Result<Printed> {
    let get = |path: &str, action: &str| {
        let client = ctx.load_authed_client()?;
        print_body(ctx, &client, &Request::get(path).about(action), Gate::Open)
    };
    match command {
        // Prices, quota, the charge cap, images, and placement groups are objects, printed as
        // the server sent them.
        Compute::Prices => get("/compute/prices/", "get the compute prices"),
        // Swagger documents the public list with no security, so it sends a token only when
        // one is configured, as `cdn prices` does. It is a command of its own, not what
        // `prices` reads when no token is set: the two answers differ in shape and unit,
        // and a document must not change with the configuration (tenet 15).
        Compute::PublicPrices => print_body(
            ctx,
            &ctx.client(&context::config()?)?,
            &Request::get("/compute/public-prices/").about("get the public compute prices"),
            Gate::Open,
        ),
        Compute::Quota => get("/compute/quota/", "get the compute quota"),
        Compute::ChargeCap => get("/compute/charge-cap/", "get the compute charge cap"),
        Compute::Images => get("/compute/images/", "list the VM images"),
        Compute::PlacementGroups => get("/compute/placement-groups/", "list the placement groups"),
        Compute::Flavors => print_envelope(
            ctx,
            &ctx.load_authed_client()?,
            &Request::get("/compute/flavors/").about("list the VM flavors"),
        ),
        Compute::Regions => print_envelope(
            ctx,
            &ctx.load_authed_client()?,
            &Request::get("/compute/regions/").about("list the VM regions"),
        ),
        // The console reads the apps catalog and the owner's keys as bare arrays;
        // swagger documents no body.
        Compute::Apps => print_envelope(
            ctx,
            &ctx.load_authed_client()?,
            &Request::get("/compute/apps/").about("list the ready-made apps"),
        ),
        Compute::AppEngine(command) => app_engine::run(ctx, command),
        Compute::AccountSshKeys => print_envelope(
            ctx,
            &ctx.load_authed_client()?,
            &Request::get("/compute/account-ssh-keys/")
                .about("list the SSH keys of the account's owner"),
        ),
        Compute::Feasibility {
            flavor,
            region,
            os_disk_tier,
        } => {
            let request = Request::get("/compute/feasibility/")
                .query("flavor", flavor)
                .about(format!("check whether the flavor `{flavor}` can launch"))
                .listed_by("hipc compute flavors");
            let request = match region {
                Some(region) => request.query("region", region),
                None => request,
            };
            let request = match os_disk_tier {
                Some(tier) => request.query("os_disk_tier", tier.code()),
                None => request,
            };
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        Compute::Vms(command) => vms(ctx, &ctx.load_authed_client()?, command),
        Compute::PublicIp(command) => public_ip(ctx, &ctx.load_authed_client()?, command),
        Compute::Firewall(Firewall::Get { vm_id }) => print_body(
            ctx,
            &ctx.load_authed_client()?,
            &firewall_get(vm_id),
            Gate::Open,
        ),
        Compute::Firewall(Firewall::Update(args)) => firewall::update(ctx, args),
        Compute::Exposures(command) => exposures(ctx, command),
        Compute::Backups(command) => backups(ctx, &ctx.load_authed_client()?, command),
        Compute::Runners(command) => runners::run(ctx, command),
        Compute::SshKeys(command) => ssh_keys(ctx, command),
        Compute::Databases(command) => databases::run(ctx, command),
        Compute::Kubernetes(command) => kubernetes::run(ctx, command),
    }
}

/// The command that lists the VM ids, for the hints that send the caller to the list: a
/// 404 on a VM route, and a launch that may or may not have gone through.
const VMS: &str = "hipc compute vms list";

fn vm_path(vm_id: &str) -> String {
    format!("/compute/vms/{vm_id}/")
}

fn resize_path(vm_id: &str) -> String {
    format!("{}resize/", vm_path(vm_id))
}

/// `POST .../stop/`, confirmed: a stop takes down what the VM serves. Text mode warns that
/// the stopped VM is still billed.
fn stop(ctx: &Context<'_>, client: &Client, request: &Request, vm_id: &str) -> Result<Printed> {
    let gate = Gate::Confirm {
        consequence: Some(
            "what it serves goes down, and it is still billed while stopped, until it is deleted",
        ),
    };
    print_with(ctx, client, request, gate, |response| {
        let vm = response.document()?;
        ctx.warning(&format!(
            "the VM `{vm_id}` is still billed while it is stopped, until it is deleted"
        ));
        Ok(vm)
    })
}

fn vms(ctx: &Context<'_>, client: &Client, command: &Vms) -> Result<Printed> {
    let power = |vm_id: &str, action: &str| {
        Request::new(Method::Post, format!("{}{action}/", vm_path(vm_id)))
            .listed_by(VMS)
            .about(format!("{action} the VM `{vm_id}`"))
    };
    match command {
        // Swagger types the VM list as a bare array with no page parameters, so it is read
        // whole; a `next` link fails `internal` rather than printing part of the list.
        Vms::List => print_envelope(
            ctx,
            client,
            &Request::get("/compute/vms/").about("list the VMs"),
        ),
        Vms::Get { vm_id } => {
            let request = Request::get(vm_path(vm_id))
                .about(format!("get the VM `{vm_id}`"))
                .listed_by(VMS);
            print_body(ctx, client, &request, Gate::Open)
        }
        Vms::Attestation { vm_id } => {
            let request = Request::get(format!("{}attestation/", vm_path(vm_id)))
                .about(format!("get the attestation of the VM `{vm_id}`"))
                .listed_by(VMS);
            print_with(ctx, client, &request, Gate::Open, |response| {
                let attestation = response.json()?;
                Ok(match ctx.mode() {
                    Mode::Text => for_a_person(attestation),
                    Mode::Json => attestation,
                })
            })
        }
        // Swagger: `{failovers}`, at most 10, newest first.
        Vms::Failovers { vm_id } => {
            let request = Request::get(format!("{}failovers/", vm_path(vm_id)))
                .about(format!("list the failovers of the VM `{vm_id}`"))
                .listed_by(VMS);
            print_keyed_list(ctx, client, &request, "failovers")
        }
        Vms::Create(args) => create(ctx, client, args),
        // A start spends nothing new and deletes nothing (tenet 5): the VM is billed until it
        // is deleted, stopped or not. A stop takes down what the VM serves, so it is confirmed.
        Vms::Start { vm_id } => print_body(ctx, client, &power(vm_id, "start"), Gate::Open),
        Vms::Stop { vm_id } => stop(ctx, client, &power(vm_id, "stop"), vm_id),
        Vms::Reboot { vm_id } => {
            let gate = Gate::Confirm { consequence: None };
            print_body(ctx, client, &power(vm_id, "reboot"), gate)
        }
        Vms::ResizeOptions { vm_id } => {
            let request = Request::get(resize_path(vm_id))
                .about(format!("get the resize options of the VM `{vm_id}`"))
                .listed_by(VMS);
            print_body(ctx, client, &request, Gate::Open)
        }
        // The command does not wait for `done`; `resize-status` follows it. Operations that
        // finish later get a status command and no new wait.
        Vms::Resize { vm_id, flavor } => {
            let request = Request::new(Method::Post, resize_path(vm_id))
                .json(&json!({ "flavor": flavor }))
                .listed_by(VMS);
            let request = request.about(format!("resize the VM `{vm_id}` to `{flavor}`"));
            let consequence = "a running VM reboots, and the bill changes to the new size";
            let gate = Gate::Confirm {
                consequence: Some(consequence),
            };
            print_with(ctx, client, &request, gate, |response| {
                let body = response.document()?;
                // clap accepted the typed vm_id; one that starts with `-` came after `--`,
                // and a printed command would read it as a flag, so it gets no note.
                let vm = crate::cli::as_argument(vm_id, crate::cli::vm_id);
                if let (Some(vm), Some(id)) =
                    (vm, body.pointer("/resize/id").and_then(Value::as_u64))
                    && let Some(command) =
                        ctx.next_command(&format!("compute vms resize-status {vm} {id}"))
                {
                    ctx.note(&format!("`{command}` shows its progress"));
                }
                Ok(body)
            })
        }
        Vms::ResizeStatus { vm_id, resize_id } => print_body(
            ctx,
            client,
            &Request::get(format!("{}{resize_id}/", resize_path(vm_id)))
                .about(format!("get the resize `{resize_id}` of the VM `{vm_id}`"))
                .listed_by(VMS),
            Gate::Open,
        ),
        Vms::PowerPolicy(command) => power_policy_command(ctx, client, command),
        Vms::Delete { vm_id } => vm_delete(ctx, client, vm_id),
    }
}

/// `DELETE .../<vm_id>/`, after a read of the VM whose `name` the question gives, so the
/// person checks the name and not only the id. A dry run sends neither, and lists both.
fn vm_delete(ctx: &Context<'_>, client: &Client, vm_id: &str) -> Result<Printed> {
    let delete = |named: &str| {
        Request::new(Method::Delete, vm_path(vm_id))
            .listed_by(VMS)
            .about(format!("delete the VM {named} and its disks"))
    };
    let read = Request::get(vm_path(vm_id))
        .about(format!("get the VM `{vm_id}`"))
        .listed_by(VMS);
    let draft = delete(&format!("`{vm_id}`"));
    let checked = match ctx.check_write(client, &draft)? {
        Write::DryRun(mut doc) => {
            doc["requests"] = ctx.describe_each(client, &[&read, &draft])?;
            return Ok(Printed::Document(doc));
        }
        Write::Checked(checked) => checked,
    };
    let vm = ctx.send(client, &read)?.document()?;
    let named = match vm.get("name").and_then(Value::as_str) {
        Some(name) => format!("{} (`{vm_id}`)", phrase::quote(name)),
        None => format!("`{vm_id}`"),
    };
    let request = delete(&named);
    ctx.confirm_write(checked, &request.action(), Some(VM_LEFT_BILLED))?;
    ctx.send(client, &request)?
        .document()
        .map(Printed::Document)
}

/// The launch clause of `--on-guest-poweroff stop`, for the VM and the app questions: the VM
/// is then billed while it sits stopped.
pub(crate) const STAYS_STOPPED: &str = "a power-off from inside the VM leaving it stopped, and \
                                        billed as a stopped VM until it is started or deleted";

/// The launch clause of `--backup-interval`, for the VM and the app questions: how often, the
/// failover, and that stored backups are billed, so neither question can leave the bill out.
pub(crate) fn backups_clause(interval: BackupInterval, failover: Option<FailoverMode>) -> String {
    let failover = failover.map_or(String::new(), |mode| {
        format!(" and {}", failover_clause(mode))
    });
    format!(
        "backups every {}{failover}, stored backups billed per GB-hour at the price that `hipc \
         compute backups options` shows",
        phrase::count(u64::from(interval.seconds()), "second")
    )
}

/// The words that name a backup failover, for the launch questions and the backup policy
/// update: `auto` says what it loses.
const fn failover_clause(mode: FailoverMode) -> &'static str {
    match mode {
        FailoverMode::Auto => {
            "failover `auto`, which restarts it on another server from its latest backup if its \
             server dies, losing what was written after that backup"
        }
        FailoverMode::Manual => "failover `manual`",
    }
}

/// What a VM delete leaves billed (swagger, `DELETE /api/compute/vms/{vm_id}/`), for its
/// question: the VM's time stops; an attached address goes with the VM (a deleted VM's
/// public-ip read answers `null` once it is gone), and the backups stay.
pub(crate) const VM_LEFT_BILLED: &str = "an attached public IPv4 is released with the VM and billed \
                                         until then; stored backups are billed until they are \
                                         removed";

/// The fields of a VM that make up its power policy (swagger `GET /api/compute/vms/{vm_id}/`):
/// the policy asked for, what the VM's server acknowledged, whether it has not yet and why, and
/// why the VM is stopped when its own operating system powered it off.
const POWER_POLICY: [&str; 5] = [
    "on_guest_poweroff",
    "on_guest_poweroff_effective",
    "on_guest_poweroff_pending",
    "on_guest_poweroff_reason",
    "stop_reason",
];

/// The power policy of the VM `response` answers with: its `vm_id` and the [`POWER_POLICY`]
/// fields, each `null` when the answer leaves it out. A VM read and the policy `PUT` both answer
/// with the whole VM. An answer without `on_guest_poweroff` as text, or, after an update, with
/// a policy other than the one sent (`sent`), is `internal`, `partial` after the write: a server
/// that dropped the field or ignored the body would otherwise print a policy nobody set.
///
/// The read takes any text as the policy, not only swagger's `restart` and `stop`: it changes
/// nothing, and a policy the server adds later is better printed than refused. An answer
/// without its `vm_id` gets the one asked for (`vm_id`), so the document always names its VM.
fn power_policy(response: &Response, vm_id: &str, sent: Option<GuestPoweroff>) -> Result<Value> {
    let vm = response.document()?;
    match (vm.get("on_guest_poweroff"), sent) {
        (Some(Value::String(policy)), Some(sent)) if policy != sent.code() => {
            return Err(response.refused(&format!(
                "the API answered with `on_guest_poweroff` {}",
                phrase::quote(policy)
            )));
        }
        (Some(Value::String(_)), _) => {}
        _ => return Err(response.refused("the API answered without `on_guest_poweroff`")),
    }
    let mut policy = Map::new();
    for field in std::iter::once("vm_id").chain(POWER_POLICY) {
        let value = vm.get(field).cloned().unwrap_or(Value::Null);
        policy.insert(field.to_owned(), value);
    }
    if policy["vm_id"].is_null() {
        policy.insert("vm_id".to_owned(), Value::from(vm_id));
    }
    Ok(Value::Object(policy))
}

/// `hipc compute vms power-policy`: the read is the VM's, since swagger has no read of its own.
fn power_policy_command(
    ctx: &Context<'_>,
    client: &Client,
    command: &PowerPolicy,
) -> Result<Printed> {
    match command {
        PowerPolicy::Get { vm_id } => {
            let request = Request::get(vm_path(vm_id))
                .about(format!("get the power policy of the VM `{vm_id}`"))
                .listed_by(VMS);
            print_with(ctx, client, &request, Gate::Open, |response| {
                let answer = power_policy(&response, vm_id, None)?;
                note_pending(ctx, &answer);
                Ok(answer)
            })
        }
        PowerPolicy::Update {
            vm_id,
            on_guest_poweroff,
        } => update_power_policy(ctx, client, vm_id, *on_guest_poweroff),
    }
}

/// `PUT /api/compute/vms/{vm_id}/power-policy/`, then the policy the answer holds.
///
/// Not confirmed: it spends nothing, setting the other value undoes it, and it only decides
/// what happens when the guest powers itself off; `hipc compute vms start` changes more
/// unasked (tenet 5). Swagger says `stop` is "exactly like a
/// stop", so a VM shut down from inside is still billed as a stopped VM; only a delete ends the
/// bill, which a text-mode note says, as the `stop` command's help does. Swagger: the server
/// applies it without a reboot and reads `on_guest_poweroff_pending` until it acknowledges, so
/// a pending answer gets a note ([`note_pending`]).
fn update_power_policy(
    ctx: &Context<'_>,
    client: &Client,
    vm_id: &str,
    policy: GuestPoweroff,
) -> Result<Printed> {
    let does = match policy {
        GuestPoweroff::Restart => "restart",
        GuestPoweroff::Stop => "stay stopped",
    };
    let request = Request::new(Method::Put, format!("{}power-policy/", vm_path(vm_id)))
        .json(&json!({ "on_guest_poweroff": policy.code() }))
        .listed_by(VMS)
        .about(format!(
            "set the VM `{vm_id}` to {does} when its own operating system powers it off"
        ));
    print_with(ctx, client, &request, Gate::Open, |response| {
        let answer = power_policy(&response, vm_id, Some(policy))?;
        // A server without `stop` restarts the VM meanwhile, which the pending note says; the
        // bill of a stopped VM would contradict it.
        if policy == GuestPoweroff::Stop && answer["on_guest_poweroff_reason"] != "host-unsupported"
        {
            ctx.note(
                "once applied, a VM shut down from inside stays stopped, and is still billed as a \
                 stopped VM until it is deleted",
            );
        }
        note_pending(ctx, &answer);
        Ok(answer)
    })
}

/// A text-mode note while the VM's server has not acknowledged the policy, by the reason
/// swagger gives: `host-unsupported` is a server that does not support `stop` and restarts the
/// VM meanwhile, which a person who set `stop` must know; `awaiting-ack`, or no reason, is a
/// policy on its way.
fn note_pending(ctx: &Context<'_>, answer: &Value) {
    if answer["on_guest_poweroff_pending"] != true {
        return;
    }
    if answer["on_guest_poweroff_reason"] == "host-unsupported" {
        ctx.note(
            "the VM's server does not support `stop` yet, so it restarts the VM meanwhile when \
             its own operating system powers it off",
        );
    } else {
        ctx.note(
            "the VM's server has not applied the policy yet: `on_guest_poweroff_pending` stays \
             true until it does",
        );
    }
}

/// The attestation as text mode shows it: `verdict` `unknown` reads "not proven". Swagger
/// (`GET /api/compute/vms/{vm_id}/attestation/`) says `unknown` means no evidence was
/// recorded, which proves nothing either way, and must never read as a negative. Every
/// other field is left as sent: `attested` is `true` or null, and text mode prints null as
/// `-`, not as "false".
fn for_a_person(mut attestation: Value) -> Value {
    if let Some(verdict) = attestation.get_mut("verdict")
        && verdict == "unknown"
    {
        *verdict = Value::from("not proven");
    }
    attestation
}

/// `POST /api/compute/vms/`, then a wait for `running` (and `network_ready` with
/// `--public-ip`, see [`Goal`]) when [`launch_wait`] says so.
///
/// With `--public-ip` the action names the address and where its price is shown, so the
/// refusal an agent reads says the launch spends on both. The price is not read before
/// the question: the one read tenet 5 allows there names the target, and `hipc compute
/// public-ip availability` already shows the price. So a dry run and a refusal send
/// nothing, and `--yes`, where nobody reads the question, costs no extra request.
fn create(ctx: &Context<'_>, client: &Client, args: &VmCreate) -> Result<Printed> {
    // Swagger, `ssh_key_ids` of `POST /api/compute/vms/`: "at most 20". Refused here, so a
    // launch the server would reject is not sent after the confirmation.
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
    let timeout = launch_wait(ctx, args.wait, args.no_wait, args.wait_timeout);
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
    let request = Request::new(Method::Post, "/compute/vms/")
        .json(&create_body(args, public_key.as_deref(), dedicated))
        .about(launch_action(args, dedicated));
    let gate = Gate::Confirm {
        consequence: Some(consequence),
    };
    // The launch's 503 `placement-groups-unavailable` (swagger, the `placement_group` field
    // of `POST /api/compute/vms/`) stays `partial` and not retryable, as every 5xx to a
    // write: whether the backend can record the VM before it refuses is not documented, and
    // a second launch is billed. Its hint names the list to check first.
    let step = ctx.call(client, &request, gate).map_err(|error| {
        if refused_as(&error, 503, "placement-groups-unavailable") {
            error.with_hint(format!(
                "run `{VMS}` to check that no VM was launched before running the command again"
            ))
        } else {
            balance_hint(error)
        }
    })?;
    let ports = open_ports(args);
    let launched = match step {
        Step::Sent(response) => response.document()?,
        Step::DryRun(mut doc) => {
            if !ports.is_empty() {
                let rules = open_ports_request(VM_ID_STAND_IN, &ports);
                doc["requests"] = ctx.describe_each(client, &[&request, &rules])?;
                // The URL is built by the HTTP library, which takes no `<`, so the id is a
                // stand-in there, written as the placeholder once the URL is built.
                if let Some(Value::String(url)) = doc.pointer_mut("/requests/1/url") {
                    *url = url.replace(
                        &format!("/{VM_ID_STAND_IN}/"),
                        &format!("/{VM_ID_PLACEHOLDER}/"),
                    );
                }
            }
            return Ok(Printed::Document(doc));
        }
    };
    // The rules the PUT set, added to the VM printed on success and after a wait that ends
    // early alike, so a JSON caller sees what was opened without another read.
    let firewall = (!ports.is_empty()).then(|| open_ports_rules(&ports));
    if firewall.is_some() {
        open_launched_ports(ctx, client, &launched, &ports, args.public_ip)?;
    }
    // Kept before the wait takes the 202: a polled document that omits the field must not
    // turn a failed attach into a warning about an address that does not exist.
    let launch_error = launched
        .get("public_ip_error")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let backup_error = launched.get("backup_error").cloned();
    // A failed attach leaves no address that could turn `network_ready`, so the wait then
    // ends at `running`.
    let goal = if args.public_ip && launch_error.is_none() {
        Goal::Reachable
    } else {
        Goal::Running
    };
    let printed = match timeout {
        Some(timeout) => wait(ctx, client, launched, goal, timeout),
        None => Ok(Printed::Document(launched)),
    };
    let printed = with_launch_firewall(printed, firewall.as_ref())?;
    if args.public_ip
        && let Printed::Document(vm) = &printed
    {
        address_warning(ctx, vm, launch_error.as_deref(), &ports);
    }
    if args.backup_interval.is_some()
        && let Printed::Document(vm) = &printed
    {
        backup_warning(ctx, vm, backup_error.as_ref());
    }
    Ok(printed)
}

/// The text-mode warning a launch with `--backup-interval` ends with when its backups are
/// not on; the document is printed unchanged. Swagger's
/// `backup_error`, on the 202 and on the VM detail, is "Why the backups asked for at launch
/// were not turned on (yet)". The printed VM decides: the 202's value (`launched`) counts
/// only when the printed VM omits the field, unlike `public_ip_error`. A null there means
/// no failure is reported yet, which does not prove the backups are on: they turn on after
/// `running`, and a later failure shows only in `hipc compute vms get`. Unverified: whether a
/// pending 202 carries a "not yet" reason, which would make every `--no-wait` launch with
/// `--backup-interval` warn.
fn backup_warning(ctx: &Context<'_>, vm: &Value, launched: Option<&Value>) {
    let error = vm.get("backup_error").or(launched).and_then(Value::as_str);
    if let Some(error) = error {
        ctx.warning(&format!(
            "the VM launched without its scheduled backups: the API says {}",
            phrase::quote(error)
        ));
    }
}

/// The text-mode warning a launch with `--public-ip` ends with; the document is printed
/// unchanged, so JSON callers see the server's answer as sent. When the printed VM, or else the 202
/// launch answer (`launch_error`), carries `public_ip_error` (swagger, on both: why the address
/// could not be attached, and nothing is billed for it), the VM runs without the address the caller
/// asked for, so that is the warning. Otherwise the address blocks inbound traffic, as after
/// `public-ip create`.
fn address_warning(ctx: &Context<'_>, vm: &Value, launch_error: Option<&str>, ports: &[u16]) {
    let error = vm
        .get("public_ip_error")
        .and_then(Value::as_str)
        .or(launch_error);
    if let Some(error) = error {
        ctx.warning(&format!(
            "the VM launched without its public IPv4: the API says {}",
            phrase::quote(error)
        ));
        return;
    }
    // `--open-port` set a rule, which [`open_launched_ports`] noted; the rest stays closed.
    if !ports.is_empty() {
        return;
    }
    // The id comes from the server, and the warning names a command to run with it.
    let vm_id = vm
        .get("vm_id")
        .and_then(Value::as_str)
        .and_then(|id| crate::cli::as_argument(id, crate::cli::vm_id))
        .unwrap_or("<vm_id>");
    ctx.warning(&inbound_blocked(vm_id));
}

/// The hint of a launch the API refused with 402, which it answers when the balance does not
/// cover the runway `hipc compute quota` shows: the two reads that tell how much is needed,
/// the hourly burn of what runs and the hours it must cover. Any other error is left as it is.
pub(crate) fn balance_hint(error: Error) -> Error {
    if error.code() == Code::PaymentRequired {
        error.with_hint(LAUNCH_BALANCE_HINT)
    } else {
        error
    }
}

/// [`balance_hint`]'s text.
const LAUNCH_BALANCE_HINT: &str = "run `hipc billing summary` and `hipc compute quota`, then \
     top up until the balance covers the runway hours at the hourly burn, this launch included";

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

/// The rules `--open-port` sets on a new VM: one `tcp` rule for `ports` with no
/// `source_cidrs`, which swagger reads as every source. A new VM has no rules, so the PUT
/// replaces none, and it carries no `base_revision`.
fn open_ports_rules(ports: &[u16]) -> Value {
    let ports: Vec<String> = ports.iter().map(u16::to_string).collect();
    json!({
        "rules": [{
            "protocol": "tcp",
            "ports": ports,
            "description": "opened at launch by hipc",
        }],
    })
}

/// `PUT .../firewall/` of the VM `vm_id` with [`open_ports_rules`].
fn open_ports_request(vm_id: &str, ports: &[u16]) -> Request {
    Request::new(Method::Put, firewall_path(vm_id))
        .listed_by(VMS)
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
    launched: &Value,
    ports: &[u16],
    public_ip: bool,
) -> Result<()> {
    // The id goes into the PUT's path, so it is held to the rule of the `vm_id` argument; a
    // leading `-` is fine there, and only matters to a printed command (the hint below).
    let vm_id = launched
        .get("vm_id")
        .and_then(Value::as_str)
        .filter(|id| crate::cli::vm_id(id).is_ok());
    let Some(vm_id) = vm_id else {
        return Err(Error::internal(
            "the launch answer has no usable `vm_id`, so the firewall rule was not set",
        )
        .with_hint(format!(
            "run `{VMS}` to find the new VM, then open its ports with `hipc compute firewall \
             update`"
        ))
        .after_write(launched.clone()));
    };
    let request = open_ports_request(vm_id, ports);
    match ctx.send(client, &request) {
        Ok(_) => {
            // Without `--public-ip` the rule waits for an address attached later.
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

/// The warning after a public IPv4 is attached, by `public-ip create` or by a launch with
/// `--public-ip`. Swagger: "All inbound traffic is blocked until you add firewall rules".
fn inbound_blocked(vm_id: &str) -> String {
    format!(
        "all inbound traffic to the address is blocked until a firewall rule accepts it, which \
         `{}` adds",
        crate::cli::with_id("compute firewall update", vm_id, "--rules-file <path>")
    )
}

/// The most stored keys one launch installs (swagger, `ssh_key_ids` of `POST
/// /api/compute/vms/`).
const MAX_SSH_KEY_IDS: usize = 20;

/// How long the launch waits for `running` when `--wait-timeout` is not given: 300
/// seconds. Arbitrary. The `--wait-timeout` help in
/// `cli/compute.rs` and `cli/app_engine.rs` repeats this value.
const WAIT_TIMEOUT: Duration = Duration::from_secs(300);

/// How long the launch waits for `running`, or `None` when it does not wait. `--wait`,
/// `--wait-timeout`, and `--no-wait` decide; without any of them, a person at a terminal
/// waits and a program does not, by the rule that picks JSON for `--output auto`. A
/// program that waits and is killed first loses the launch answer, and an
/// agent's tool timeout is often shorter than the wait.
///
/// `--wait-timeout` asks for a wait, so it waits wherever it runs: a flag wins over
/// detection (tenet 15), and the same command line behaves the same at a terminal and on a
/// pipe. Beside `--no-wait` it means nothing, and clap refuses that pair.
///
/// `hipc compute app-engine apps create` takes the same three flags and waits by this rule.
pub(crate) fn launch_wait(
    ctx: &Context<'_>,
    wait: bool,
    no_wait: bool,
    wait_timeout: Option<u64>,
) -> Option<Duration> {
    let waits = wait || wait_timeout.is_some() || !(no_wait || ctx.reader_is_program());
    waits.then(|| wait_timeout.map_or(WAIT_TIMEOUT, Duration::from_secs))
}

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
fn launch_action(args: &VmCreate, dedicated: Option<Dedicated>) -> String {
    let mut clauses = Vec::new();
    if args.public_ip {
        clauses.push(
            "a public IPv4, billed per second at the price that `hipc compute public-ip \
             availability` shows (`price_hourly`, or free until `free_until` while it is set, \
             then `price_hourly_after_free`)"
                .to_owned(),
        );
    }
    if let Some(tier) = args.os_disk_tier {
        let tier = tier.code();
        clauses.push(format!(
            "an OS disk on tier `{tier}` or faster, fixed for the VM's life and billed at the \
             `{tier}` price that `hipc compute prices` shows (`vm_by_os_disk_tier`)"
        ));
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
    if let Some(interval) = args.backup_interval {
        clauses.push(backups_clause(interval, args.failover));
    }
    if args.on_guest_poweroff == Some(GuestPoweroff::Stop) {
        clauses.push(STAYS_STOPPED.to_owned());
    }
    let ports = open_ports(args);
    if !ports.is_empty() {
        clauses.push(format!(
            "{} open to every IPv4 address",
            ports_named(&ports)
        ));
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
fn create_body(args: &VmCreate, public_key: Option<&str>, dedicated: Option<Dedicated>) -> Value {
    let mut body = Map::new();
    body.insert("flavor".to_owned(), args.flavor.clone().into());
    for (key, value) in [
        ("name", &args.name),
        ("image", &args.image),
        ("region", &args.region),
        ("placement_group", &args.placement_group),
    ] {
        if let Some(value) = value {
            body.insert(key.to_owned(), value.clone().into());
        }
    }
    if !args.ssh_key_ids.is_empty() {
        body.insert("ssh_key_ids".to_owned(), args.ssh_key_ids.clone().into());
    }
    if let Some(key) = public_key {
        body.insert("ssh_public_key".to_owned(), key.into());
    }
    if args.public_ip {
        body.insert("public_ip".to_owned(), true.into());
    }
    if let Some(tier) = args.os_disk_tier {
        body.insert("os_disk_tier".to_owned(), tier.code().into());
    }
    if let Some(Dedicated { disk, count }) = dedicated {
        let disks = json!({
            "tier": disk.tier.code(),
            "count": count,
            "min_size_gb": disk.min_size_gb,
        });
        body.insert("dedicated_disks".to_owned(), disks);
        // Swagger: `dedicated_disks` needs `pinned_ack: true` (422 `dedicated-ack-required`),
        // the customer's acknowledgement that the VM is pinned to its server. The question,
        // or `--yes`, is that acknowledgement, and the action names the pinning.
        // `pinned_ack_version` is not sent: only a read of `GET /api/compute/prices/` gives
        // it, and swagger refuses only a stale one (422 `dedicated-ack-stale`).
        body.insert("pinned_ack".to_owned(), true.into());
    }
    if let Some(interval) = args.backup_interval {
        body.insert("backup_interval_s".to_owned(), interval.seconds().into());
    }
    if let Some(mode) = args.failover {
        body.insert("failover_mode".to_owned(), mode.code().into());
    }
    if let Some(policy) = args.on_guest_poweroff {
        body.insert("on_guest_poweroff".to_owned(), policy.code().into());
    }
    Value::Object(body)
}

/// What ends a launch wait.
#[derive(Debug, Clone, Copy)]
enum Goal {
    /// `status` is `running`.
    Running,
    /// `status` is `running` and `network_ready` is true: a launch with `--public-ip`.
    /// Swagger: `network_ready` says the internet path to the address is open; it can lag the
    /// opened path by up to about 30 seconds, never lead it. A `public_ip_error` on the VM also
    /// ends it, since the attach failed and no address will turn ready.
    Reachable,
}

/// Where a launch stands, from the VM's `status`, `boot_stalled`, and `network_ready`.
#[derive(Debug)]
enum Launch<'a> {
    /// The goal is met.
    Done,
    /// A terminal status: `failed` without `boot_stalled`, `decommissioning`, or
    /// `decommissioned`.
    Ended(&'a str),
    /// Not there yet, so the wait goes on.
    Waiting(Pending),
}

/// Why a launch wait goes on, which a timeout names.
#[derive(Debug)]
enum Pending {
    /// `pending`, `provisioning`, a missing status, or a status this version does not
    /// know, which may be new and transient.
    Booting,
    /// `failed` with `boot_stalled` true. Swagger: such a VM "returns to `running` by itself
    /// once the guest signals", so it is not terminal.
    Stalled,
    /// `running`, but `network_ready` is not true yet.
    Unreachable,
}

fn launch_state(vm: &Value, goal: Goal) -> Launch<'_> {
    let field = |key| vm.get(key);
    match field("status").and_then(Value::as_str) {
        Some("running") => match goal {
            Goal::Running => Launch::Done,
            Goal::Reachable
                if field("network_ready").and_then(Value::as_bool) == Some(true)
                    || field("public_ip_error").is_some_and(Value::is_string) =>
            {
                Launch::Done
            }
            Goal::Reachable => Launch::Waiting(Pending::Unreachable),
        },
        Some("failed") if field("boot_stalled").and_then(Value::as_bool) == Some(true) => {
            Launch::Waiting(Pending::Stalled)
        }
        Some(status @ ("failed" | "decommissioning" | "decommissioned")) => Launch::Ended(status),
        _ => Launch::Waiting(Pending::Booting),
    }
}

/// Polls the launched VM until `goal` is met, its status is terminal, or `timeout` has
/// passed. Every failure prints the last VM document seen, so `vm_id` is not lost, and is
/// never retryable: a retry of the create would launch, and bill, a second VM
/// (`TENETS.md` tenet 4).
///
/// A poll can end up to `--request-timeout` past the deadline, because the last request
/// is not cut short. A run killed during the wait prints nothing on stdout; text mode
/// names the VM on stderr before the wait, unless `--quiet`, and JSON mode stays silent
/// (tenet 3).
fn wait(
    ctx: &Context<'_>,
    client: &Client,
    launched: Value,
    goal: Goal,
    timeout: Duration,
) -> Result<Printed> {
    let Some(vm_id) = launched
        .get("vm_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return Err(
            Error::internal("the launch answer has no `vm_id`, so it cannot be waited on")
                .with_hint(format!("run `{VMS}` to find the new VM"))
                .after_write(launched),
        );
    };
    // The id comes from the server, so every message quotes it through `phrase::quote`.
    let vm_quoted = phrase::quote(&vm_id);
    let hint = match crate::cli::as_argument(&vm_id, crate::cli::vm_id) {
        Some(id) => format!(
            "run {} to see where the launch stands",
            phrase::quote(format!("hipc compute vms get {id}"))
        ),
        None => format!("run `{VMS}` to find the new VM"),
    };
    if let Launch::Waiting(_) = launch_state(&launched, goal) {
        // The id goes into the poll URL, so it is held to the rule of the `vm_id` argument
        // before the first poll: `a?b=1` or `a/b` would poll another route. A launch answer
        // that already meets the goal or is terminal needs no poll, so it is not refused.
        if let Err(rule) = crate::cli::vm_id(&vm_id) {
            let message = format!(
                "the launch answer names the VM {vm_quoted}, which `hipc` cannot poll: {rule}"
            );
            return Err(Error::internal(message)
                .with_hint(hint)
                .after_write(launched));
        }
        let until = match goal {
            Goal::Running => "it is running",
            Goal::Reachable => "it is running and its public IPv4 is reachable",
        };
        ctx.note(&format!(
            "launched the VM {vm_quoted}; waiting until {until}"
        ));
    }
    let request = Request::get(vm_path(&vm_id)).about(format!("poll the VM {vm_quoted}"));
    let schedule = Schedule {
        first: POLL_INTERVAL,
        interval: POLL_INTERVAL,
        deadline: Instant::now() + timeout,
    };
    let launch = VmLaunch {
        vm: launched,
        goal,
        pending: Pending::Booting,
        vm_id: &vm_id,
        vm_quoted: &vm_quoted,
        hint: &hint,
        timeout,
    };
    poll_until(ctx, client, &request, schedule, launch)
}

/// The wait of a VM launch: the last VM document seen, and what the wait says of it.
struct VmLaunch<'a> {
    vm: Value,
    goal: Goal,
    /// What the VM was waiting for when it was last looked at.
    pending: Pending,
    vm_id: &'a str,
    vm_quoted: &'a str,
    hint: &'a str,
    timeout: Duration,
}

impl Wait for VmLaunch<'_> {
    type Output = Printed;

    fn next(&mut self) -> Next<Printed> {
        match launch_state(&self.vm, self.goal) {
            Launch::Done => Next::Stop(Ok(Printed::Document(std::mem::take(&mut self.vm)))),
            Launch::Ended(status) => {
                let message = format!(
                    "the VM {} reached {} instead of `running`",
                    self.vm_quoted,
                    phrase::quote(status)
                );
                Next::Stop(Err(Error::new(Code::PartialFailure, message)
                    .with_hint(self.hint)
                    .after_write(std::mem::take(&mut self.vm))))
            }
            Launch::Waiting(pending) => {
                self.pending = pending;
                Next::Poll
            }
        }
    }

    fn take(&mut self, polled: Value) -> Result<()> {
        // A document for another VM, or for none, is not this launch: printing it would
        // report another VM's status as this one's.
        match polled.get("vm_id").and_then(Value::as_str) {
            Some(id) if id == self.vm_id => {
                self.vm = polled;
                Ok(())
            }
            other => {
                let named = match other {
                    Some(other) => format!("the VM {}", phrase::quote(other)),
                    None => "no VM".to_owned(),
                };
                let message = format!(
                    "the poll of the VM {} answered with the document of {named}",
                    self.vm_quoted
                );
                Err(Error::internal(message)
                    .with_hint(self.hint)
                    .after_write(std::mem::take(&mut self.vm)))
            }
        }
    }

    fn timed_out(self) -> Result<Printed> {
        let (vm, after) = (
            self.vm_quoted,
            phrase::count(self.timeout.as_secs(), "second"),
        );
        let message = match self.pending {
            Pending::Booting => format!("the VM {vm} was not running after {after}"),
            Pending::Stalled => format!(
                "the VM {vm} was not running after {after}: its boot stalled (`boot_stalled`) \
                 and has not recovered"
            ),
            Pending::Unreachable => format!(
                "the VM {vm} was running, but its public IPv4 was not reachable \
                 (`network_ready`) after {after}"
            ),
        };
        Err(Error::new(Code::Timeout, message)
            .with_hint(self.hint)
            .after_write(self.vm))
    }

    // A poll is a read, so polling on after a passing outage cannot launch twice: the last
    // good document is kept, and only a run of failures ends the wait.
    fn failed(self, error: Error) -> Error {
        error.with_hint(self.hint).after_write(self.vm)
    }
}

fn public_ip_path(vm_id: &str) -> String {
    format!("{}public-ip/", vm_path(vm_id))
}

fn firewall_path(vm_id: &str) -> String {
    format!("{}firewall/", vm_path(vm_id))
}

fn firewall_get(vm_id: &str) -> Request {
    Request::get(firewall_path(vm_id))
        .about(format!("get the firewall of the VM `{vm_id}`"))
        .listed_by(VMS)
}

fn public_ip(ctx: &Context<'_>, client: &Client, command: &PublicIp) -> Result<Printed> {
    match command {
        PublicIp::Availability { region } => {
            let request = Request::get("/compute/public-ip/availability/")
                .about("get the public IPv4 availability");
            let request = match region {
                Some(region) => request.query("region", region),
                None => request,
            };
            print_body(ctx, client, &request, Gate::Open)
        }
        PublicIp::Get { vm_id } => print_body(
            ctx,
            client,
            &Request::get(public_ip_path(vm_id))
                .about(format!("get the public IPv4 of the VM `{vm_id}`"))
                .listed_by(VMS),
            Gate::Open,
        ),
        // Swagger documents the attach as idempotent: a repeat returns the address already
        // attached. It still needs `--yes`, because the first call starts a bill.
        PublicIp::Create { vm_id } => {
            let request = Request::new(Method::Post, public_ip_path(vm_id))
                .listed_by(VMS)
                .about(format!("attach a public IPv4 to the VM `{vm_id}`"));
            let gate = Gate::Confirm {
                consequence: Some("it is billed until it is detached or the VM is deleted"),
            };
            print_with(ctx, client, &request, gate, |response| {
                let address = response.document()?;
                ctx.warning(&inbound_blocked(vm_id));
                Ok(address)
            })
        }
        PublicIp::Delete { vm_id } => {
            let request = Request::new(Method::Delete, public_ip_path(vm_id))
                .listed_by(VMS)
                .about(format!("detach the public IPv4 from the VM `{vm_id}`"));
            let gate = Gate::Confirm {
                consequence: Some(IP_DETACHED),
            };
            let done = json!({ "vm_id": vm_id, "public_ip": null });
            print_body_or(ctx, client, &request, gate, done)
        }
        PublicIp::Usage { vm_id, range } => {
            let request = Request::get(format!("{}usage/", public_ip_path(vm_id)))
                .about(format!("get the public IPv4 usage of the VM `{vm_id}`"))
                .listed_by(VMS);
            let request = match range {
                Some(range) => request.query("range", range.code()),
                None => request,
            };
            print_body(ctx, client, &request, Gate::Open)
        }
    }
}

fn expose_path(vm_id: &str) -> String {
    format!("{}expose/", vm_path(vm_id))
}

fn exposures(ctx: &Context<'_>, command: &Exposures) -> Result<Printed> {
    match command {
        // Swagger types the list as a bare array with no page parameters.
        Exposures::List { vm_id } => {
            let request = Request::get(expose_path(vm_id))
                .about(format!("list the exposures of the VM `{vm_id}`"))
                .listed_by(VMS);
            print_envelope(ctx, &ctx.load_authed_client()?, &request)
        }
        Exposures::Create(args) => exposure_create(ctx, args),
        Exposures::Delete { vm_id, exposure_id } => {
            let request = Request::new(
                Method::Delete,
                format!("{}{exposure_id}/", expose_path(vm_id)),
            )
            .listed_by(crate::cli::with_id("compute exposures list", vm_id, ""))
            .about(format!(
                "take down the exposure `{exposure_id}` of the VM `{vm_id}`"
            ));
            let gate = Gate::Confirm {
                consequence: Some(EXPOSURE_DELETED),
            };
            let done = json!({ "vm_id": vm_id, "id": exposure_id, "deleted": true });
            print_body_or(ctx, &ctx.load_authed_client()?, &request, gate, done)
        }
    }
}

/// `POST .../expose/` with `port`, and `mode` and `protocol` only when passed. `hipc`
/// sends only the flags given rather than defaulting to the end-to-end `sni`, so
/// with no `--mode` the server picks `http`, and the confirmation says the edge can read
/// it.
fn exposure_create(ctx: &Context<'_>, args: &ExposureCreate) -> Result<Printed> {
    // Swagger: "`udp` needs `mode: raw`". Refused here, so the question is never asked for
    // an exposure the server would refuse.
    if matches!(args.protocol, Some(ExposeProtocol::Udp))
        && !matches!(args.mode, Some(ExposeMode::Raw))
    {
        return Err(Error::usage(
            "`--protocol udp` needs `--mode raw`, the one mode that forwards UDP",
            "pass `--mode raw`",
        ));
    }
    let mut body = Map::new();
    body.insert("port".to_owned(), args.port.into());
    if let Some(mode) = args.mode {
        body.insert("mode".to_owned(), mode.code().into());
    }
    if let Some(protocol) = args.protocol {
        body.insert("protocol".to_owned(), protocol.code().into());
    }
    let request = Request::new(Method::Post, expose_path(&args.vm_id))
        .json(&Value::Object(body))
        .listed_by(VMS);
    let port = match args.protocol {
        Some(protocol) => format!("`{}` port `{}`", protocol.code(), args.port),
        None => format!("port `{}`", args.port),
    };
    let request = request.about(format!(
        "expose {port} of the VM `{}` to the internet {}",
        args.vm_id,
        exposure_reach(args.mode)
    ));
    let gate = Gate::Confirm { consequence: None };
    let client = ctx.load_authed_client()?;
    print_with(ctx, &client, &request, gate, |response| {
        let exposure = response.document()?;
        // The server's `edge_sees_traffic` decides the note, not the mode that was asked
        // for. Swagger sets it in `raw` mode too, where the edge reads the traffic only when
        // the protocol inside does not encrypt it, so the note repeats the flag and claims
        // no more.
        if exposure.get("edge_sees_traffic") == Some(&Value::Bool(true)) {
            let endpoint = exposure
                .get("endpoint")
                .and_then(Value::as_str)
                .map_or_else(|| "this exposure".to_owned(), phrase::quote);
            ctx.warning(&format!(
                "the API reports that the edge sees the traffic to {endpoint}: it is not end \
                 to end"
            ));
        }
        Ok(exposure)
    })
}

/// What the edge can see in `mode`, as the end of the confirmation (swagger, `POST
/// .../expose/`).
fn exposure_reach(mode: Option<ExposeMode>) -> &'static str {
    match mode {
        None => {
            "in `http` mode, the server's default, where the edge terminates TLS and can read \
             the traffic"
        }
        Some(ExposeMode::Http) => {
            "in `http` mode, where the edge terminates TLS and can read the traffic"
        }
        Some(ExposeMode::Sni) => {
            "in `sni` mode, end to end: the edge forwards ciphertext it cannot open"
        }
        Some(ExposeMode::Raw) => {
            "in `raw` mode, where the edge forwards the bytes as sent and can read the traffic \
             unless the protocol inside encrypts it"
        }
    }
}

fn backup_path(vm_id: &str) -> String {
    format!("{}backup/", vm_path(vm_id))
}

fn restore_path(vm_id: &str) -> String {
    format!("{}backups/restore/", vm_path(vm_id))
}

fn backups(ctx: &Context<'_>, client: &Client, command: &Backups) -> Result<Printed> {
    let get = |request: Request| print_body(ctx, client, &request, Gate::Open);
    let of_vm = |path: String, what: &str, vm_id: &str| {
        Request::get(path)
            .about(format!("get the {what} of the VM `{vm_id}`"))
            .listed_by(VMS)
    };
    match command {
        Backups::Options => {
            get(Request::get("/compute/backup-options/").about("get the backup options"))
        }
        Backups::Policy(BackupPolicy::Get { vm_id }) => {
            get(of_vm(backup_path(vm_id), "backup policy", vm_id))
        }
        Backups::Policy(BackupPolicy::Update(args)) => policy_update(ctx, client, args),
        // Swagger: "Idempotent". The 204 has no body, so the document says what is now true.
        Backups::Policy(BackupPolicy::Delete { vm_id }) => {
            let request = Request::new(Method::Delete, backup_path(vm_id))
                .about(format!("turn off the backups of the VM `{vm_id}`"))
                .listed_by(VMS);
            let gate = Gate::Confirm {
                consequence: Some(BACKUPS_KEPT),
            };
            let done = json!({ "vm_id": vm_id, "enabled": false });
            print_body_or(ctx, client, &request, gate, done)
        }
        // The answer is an object (`state`, `stored_bytes`, `restore_points`, ...): the list
        // envelope holds `restore_points` as `items`, with the other fields beside it.
        // Swagger requires no field of the answer, so one without `restore_points`, or with
        // `null`, such as a VM whose backups are off, is an empty list.
        Backups::List { vm_id } => {
            let request = of_vm(format!("{}backups/", vm_path(vm_id)), "backups", vm_id);
            print_list_with(ctx, client, &request, |response| {
                let mut answer = response.json()?;
                if let Value::Object(fields) = &mut answer
                    && fields.get("restore_points").is_none_or(Value::is_null)
                {
                    fields.insert("restore_points".to_owned(), Value::Array(Vec::new()));
                }
                keyed_list(answer, &request, "restore_points")
            })
        }
        Backups::Restore(args) => restore(ctx, client, args),
        // A restore runs for minutes. Operations that finish later get a status command,
        // not a wait, so this reads once.
        Backups::RestoreStatus { vm_id } => {
            get(of_vm(restore_path(vm_id), "restore status", vm_id))
        }
    }
}

/// `PUT .../backup/` with `interval_s`, and `failover_mode` only when passed: swagger says
/// an omitted mode keeps the current one.
fn policy_update(ctx: &Context<'_>, client: &Client, args: &BackupPolicyUpdate) -> Result<Printed> {
    let vm_id = &args.vm_id;
    let seconds = args.interval.seconds();
    let mut body = Map::new();
    body.insert("interval_s".to_owned(), seconds.into());
    let failover = match args.failover {
        Some(mode) => {
            body.insert("failover_mode".to_owned(), mode.code().into());
            format!(", with {}", failover_clause(mode))
        }
        None => String::new(),
    };
    let request = Request::new(Method::Put, backup_path(vm_id))
        .json(&Value::Object(body))
        .listed_by(VMS)
        .about(format!(
            "back up the VM `{vm_id}` every {}{failover}",
            phrase::count(u64::from(seconds), "second")
        ));
    let gate = Gate::Confirm {
        consequence: Some(
            "stored backups are billed per GB-hour from the balance, at the price `hipc \
             compute backups options` shows",
        ),
    };
    print_body(ctx, client, &request, gate)
}

/// What a dry run shows for the server's `confirm` field. A real run fills it with the
/// VM's `name`, which only a read can give, and a dry run sends nothing.
const NAME_PLACEHOLDER: &str = "<the VM's name>";

/// `POST .../backups/restore/`. Swagger: "`confirm` must be the VM's `name`, exactly". `hipc`
/// fills it from `GET /api/compute/vms/<vm_id>/` rather than taking a `--confirm-name`
/// flag, which would be a second confirmation beside `--yes`. The read comes before the
/// question, so the question names the VM whose disk is overwritten and the name the
/// server checks is one the person saw (a read after the question would let a wrong
/// id be confirmed under a name nobody read). With `--yes` nobody sees
/// the question, and the server's check is the only guard. The local refusals (an account
/// the route cannot use, the token) run before that read. A failed read is the error, and
/// nothing is written. A dry run sends neither: its document is the POST with the name as a
/// placeholder, and its `requests` lists the read, then the POST.
fn restore(ctx: &Context<'_>, client: &Client, args: &BackupRestore) -> Result<Printed> {
    let vm_id = &args.vm_id;
    let point = &args.run_id;
    let body = |name: &str| {
        let mut body = Map::new();
        body.insert("point_id".to_owned(), point.clone().into());
        // Swagger: "Only `mode: in_place` is available".
        body.insert("mode".to_owned(), "in_place".into());
        body.insert("confirm".to_owned(), name.into());
        if args.accept_rollback {
            body.insert("accept_rollback".to_owned(), true.into());
        }
        Value::Object(body)
    };
    let post = |name: &str| Request::new(Method::Post, restore_path(vm_id)).json(&body(name));
    let read = Request::get(vm_path(vm_id))
        .about(format!("get the VM `{vm_id}`"))
        .listed_by(VMS);
    let draft = post(NAME_PLACEHOLDER);
    let checked = match ctx.check_write(client, &draft)? {
        Write::DryRun(mut doc) => {
            doc["requests"] = ctx.describe_each(client, &[&read, &draft])?;
            return Ok(Printed::Document(doc));
        }
        Write::Checked(checked) => checked,
    };
    let vm = ctx.send(client, &read)?.document()?;
    let Some(name) = vm.get("name").and_then(Value::as_str) else {
        return Err(Error::internal(format!(
            "the VM `{vm_id}` has no `name`, so the restore's `confirm` cannot be filled"
        )));
    };
    let rollback = if args.accept_rollback {
        ", which may be a point from before its last reboot"
    } else {
        ""
    };
    let request = post(name).listed_by(VMS).about(format!(
        "overwrite the disk of the VM {} (`{vm_id}`) with the backup `{point}`{rollback}",
        phrase::quote(name)
    ));
    let consequence = "everything written after that backup is lost, and the VM stops for a \
                       few minutes";
    ctx.confirm_write(checked, &request.action(), Some(consequence))?;
    let restore = ctx
        .send(client, &request)
        .and_then(|response| response.document());
    restore.map(Printed::Document).map_err(|error| {
        // A 400 for a point from before the VM's last reboot sent without
        // `accept_rollback`. Any other 400 gets no hint: a wrong hint is worse than none.
        if refused_as(&error, 400, "rollback-not-accepted") {
            error.with_hint(
                "pass `--accept-rollback` to restore a point from before the VM's last reboot",
            )
        } else {
            error
        }
    })
}

/// Whether the server answered `status` with the compute error slug `slug`. Swagger names
/// such slugs in prose but not the field that carries them, and types compute error bodies
/// as `{error}`, so `error` and `code` are both read.
fn refused_as(error: &Error, status: u16, slug: &str) -> bool {
    error.http_status() == Some(status)
        && error.details().is_some_and(|body| {
            ["code", "error"]
                .iter()
                .any(|key| body.get(*key).and_then(Value::as_str) == Some(slug))
        })
}

fn ssh_keys(ctx: &Context<'_>, command: &SshKeys) -> Result<Printed> {
    match command {
        SshKeys::List(args) => list::run(
            ctx,
            &ctx.load_authed_client()?,
            &Request::get("/ssh-keys/").about("list the SSH keys"),
            args,
        ),
        SshKeys::Get { key_id } => {
            let request = Request::get(format!("/ssh-keys/{key_id}/"))
                .about(format!("get the SSH key `{key_id}`"))
                .listed_by("hipc compute ssh-keys list");
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        SshKeys::Create {
            name,
            public_key_file,
        } => {
            let public_key = read_public_key(public_key_file, "--public-key-file")?;
            let request = Request::new(Method::Post, "/ssh-keys/")
                .json(&json!({ "name": name, "public_key": public_key }));
            let request = request.about(format!("store the SSH key {}", phrase::quote(name)));
            let client = ctx.load_authed_client()?;
            print_body(ctx, &client, &request, Gate::Confirm { consequence: None })
        }
        SshKeys::Delete { key_id } => {
            let request = Request::new(Method::Delete, format!("/ssh-keys/{key_id}/"))
                .listed_by("hipc compute ssh-keys list")
                .about(format!("delete the SSH key `{key_id}`"));
            let gate = Gate::Confirm {
                consequence: Some(SSH_KEY_KEPT),
            };
            let done = json!({ "id": key_id, "deleted": true });
            print_body_or(ctx, &ctx.load_authed_client()?, &request, gate, done)
        }
    }
}

/// The public key in the file `flag` names (or stdin for `-`), for `ssh-keys create` and
/// `vms create`. A private key is refused before anything is sent: the obvious mistake is
/// passing `~/.ssh/id_ed25519` instead of `~/.ssh/id_ed25519.pub`, and that would hand the
/// private key to the server. Errors name the file and the flag, and never quote what the
/// file holds. Only a line in the OpenSSH public key format ([`is_public_key`]) is sent: any
/// other one-line file, such as a token, would otherwise reach the server, whose refusal
/// echoes it on stderr. A symlink is followed: dotfile managers link `~/.ssh/*.pub`. Its
/// target must be a regular file other than the config or an AWS file, and the format check
/// refuses whatever else a link may point to.
fn read_public_key(path: &Path, flag: &'static str) -> Result<String> {
    let input = Input {
        path,
        flag,
        what: "public key",
    };
    let text = input.read_text_following_links(MAX_PUBLIC_KEY_FILE_BYTES)?;
    let key = text.trim();
    if key.contains("PRIVATE KEY") {
        return Err(Error::usage(
            format!("{} holds a private key, which is never sent", input.name()),
            format!(
                "pass the public key file next to it to `{flag}`, such as `~/.ssh/id_ed25519.pub`"
            ),
        ));
    }
    if key.lines().count() != 1 || !is_public_key(key) {
        return Err(Error::usage(
            format!(
                "{} does not hold a public key: a public key is one line in the OpenSSH format, \
                 such as `ssh-ed25519 AAAA... user@host`",
                input.name()
            ),
            format!("pass the `.pub` file to `{flag}`, such as `~/.ssh/id_ed25519.pub`"),
        ));
    }
    Ok(key.to_owned())
}

/// The largest public key file read, in bytes: an OpenSSH RSA key of 16384 bits is under
/// 3 KiB.
const MAX_PUBLIC_KEY_FILE_BYTES: u64 = 64 * 1024;

/// The key types OpenSSH writes as the first word of a `.pub` file (`ssh-keygen -t`, and the
/// `-sk` types of a security key).
const PUBLIC_KEY_TYPES: [&str; 8] = [
    "ssh-ed25519",
    "ssh-rsa",
    "ssh-dss",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "sk-ssh-ed25519@openssh.com",
    "sk-ecdsa-sha2-nistp256@openssh.com",
];

/// Whether `line` is an OpenSSH public key: a key type of [`PUBLIC_KEY_TYPES`], then the key
/// in base64, then an optional comment.
fn is_public_key(line: &str) -> bool {
    let mut words = line.split_ascii_whitespace();
    let (Some(kind), Some(blob)) = (words.next(), words.next()) else {
        return false;
    };
    PUBLIC_KEY_TYPES.contains(&kind)
        && blob
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
}

/// What a public IPv4 detach gives up, for its question and `hipc api`'s.
pub(crate) const IP_DETACHED: &str = "a later attach may get a different address";

/// What an exposure delete gives up, for its question and `hipc api`'s.
pub(crate) const EXPOSURE_DELETED: &str = "its hostname cannot be used again";

/// What a backup policy delete leaves, for its question and `hipc api`'s.
pub(crate) const BACKUPS_KEPT: &str =
    "the backups already taken are kept 7 days, billed while kept, then deleted";

/// What an SSH key delete leaves, for its question and `hipc api`'s.
pub(crate) const SSH_KEY_KEPT: &str = "a VM that has it keeps it";
