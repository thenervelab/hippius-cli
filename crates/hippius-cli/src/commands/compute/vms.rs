//! `hipc compute vms`: one VM, its power, its resize, and its delete.

mod create;

use hippius::routes::Route;
use hippius::routes::compute::{self, Vm};
use hippius::{Client, Request, Result, phrase, view_of};
use serde_json::Value;

use crate::cli::Vms;
use crate::commands::compute::VMS;
use crate::commands::compute::power_policy::power_policy_command;
use crate::commands::{Printed, print_body, print_envelope, print_keyed_list, print_with};
use crate::context::{Context, Gate, Write};
use crate::output::Mode;

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

pub(super) fn vms(ctx: &Context<'_>, client: &Client, command: &Vms) -> Result<Printed> {
    let power = |route: &Route<1>, vm_id: &str, action: &str| {
        Request::to(route, [&vm_id])
            .listed_by(VMS)
            .about(format!("{action} the VM `{vm_id}`"))
    };
    match command {
        // Swagger types the VM list as a bare array with no page parameters, so it is read
        // whole; a `next` link fails `internal` rather than printing part of the list.
        Vms::List => print_envelope(
            ctx,
            client,
            &Request::to(&compute::VMS, []).about("list the VMs"),
        ),
        Vms::Get { vm_id } => {
            let request = Request::to(&compute::VM, [vm_id])
                .about(format!("get the VM `{vm_id}`"))
                .listed_by(VMS);
            print_body(ctx, client, &request, Gate::Open)
        }
        Vms::Attestation { vm_id } => {
            let request = Request::to(&compute::VM_ATTESTATION, [vm_id])
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
            let request = Request::to(&compute::VM_FAILOVERS, [vm_id])
                .about(format!("list the failovers of the VM `{vm_id}`"))
                .listed_by(VMS);
            print_keyed_list(ctx, client, &request, "failovers")
        }
        Vms::Create(args) => create::create(ctx, client, args),
        // A start spends nothing new and deletes nothing (tenet 5): the VM is billed until it
        // is deleted, stopped or not. A stop takes down what the VM serves, so it is confirmed.
        Vms::Start { vm_id } => print_body(
            ctx,
            client,
            &power(&compute::VM_START, vm_id, "start"),
            Gate::Open,
        ),
        Vms::Stop { vm_id } => stop(ctx, client, &power(&compute::VM_STOP, vm_id, "stop"), vm_id),
        Vms::Reboot { vm_id } => {
            let gate = Gate::Confirm { consequence: None };
            print_body(
                ctx,
                client,
                &power(&compute::VM_REBOOT, vm_id, "reboot"),
                gate,
            )
        }
        Vms::ResizeOptions { vm_id } => {
            let request = Request::to(&compute::VM_RESIZE_OPTIONS, [vm_id])
                .about(format!("get the resize options of the VM `{vm_id}`"))
                .listed_by(VMS);
            print_body(ctx, client, &request, Gate::Open)
        }
        // The command does not wait for `done`; `resize-status` follows it. Operations that
        // finish later get a status command and no new wait.
        Vms::Resize { vm_id, flavor } => {
            let request = Request::to(&compute::VM_RESIZE, [vm_id])
                .json(&compute::VmResizeBody { flavor })
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
                let resize = view_of::<compute::VmResize>(&body).resize;
                if let (Some(vm), Some(id)) = (vm, resize.and_then(|resize| resize.id))
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
            &Request::to(&compute::VM_RESIZE_STATUS, [vm_id, resize_id])
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
        Request::to(&compute::VM_DELETE, [&vm_id])
            .listed_by(VMS)
            .about(format!("delete the VM {named} and its disks"))
    };
    let read = Request::to(&compute::VM, [&vm_id])
        .about(format!("get the VM `{vm_id}`"))
        .listed_by(VMS);
    let draft = delete(&format!("`{vm_id}`"));
    let checked = match ctx.check_write(client, &draft)? {
        Write::DryRun(_) => {
            return ctx
                .dry_run_of(client, &[&read, &draft])
                .map(Printed::Document);
        }
        Write::Checked(checked) => checked,
    };
    let vm: Vm = view_of(&ctx.send(client, &read)?.document()?);
    let named = match vm.name {
        Some(name) => format!("{} (`{vm_id}`)", phrase::quote(name)),
        None => format!("`{vm_id}`"),
    };
    let request = delete(&named);
    let confirmed = ctx.confirm_write(checked, &request.action(), Some(VM_LEFT_BILLED))?;
    ctx.send_confirmed(client, &request, &confirmed)?
        .document()
        .map(Printed::Document)
}

/// What a VM delete leaves billed (swagger, `DELETE /api/compute/vms/{vm_id}/`), for its
/// question: the VM's time stops; an attached address goes with the VM (a deleted VM's
/// public-ip read answers `null` once it is gone), and the backups stay.
pub(crate) const VM_LEFT_BILLED: &str = "an attached public IPv4 is released with the VM and billed \
                                         until then; stored backups are billed until they are \
                                         removed";

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
