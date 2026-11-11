//! `hipc compute vms`: one VM, its power, its resize, and its delete.

mod create;

use hippius::routes::Route;
use hippius::routes::compute::{self, Vm};
use hippius::{Client, Request, Result, phrase, view_of};
use serde_json::Value;

use crate::cli::{VmId, VmTarget, Vms};
use crate::commands::compute::power_policy::power_policy_command;
use crate::commands::compute::{VMS_LIST_COMMAND, of_vm};
use crate::commands::{
    Decided, Printed, print_body, print_envelope, print_keyed_list, print_with, read_then_write,
};
use crate::context::{Context, Gate, WritePlan};
use crate::output::Mode;

/// `POST .../stop/`, confirmed: a stop takes down what the VM serves. Text mode warns that
/// the stopped VM is still billed.
fn stop(ctx: &Context<'_>, client: &Client, request: &Request, vm_id: &VmId) -> Result<Printed> {
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
    let power = |route: &Route<1>, vm_id: &VmId, action: &str| of_vm(route, action, vm_id);
    match command {
        // Swagger types the VM list as a bare array with no page parameters, so it is read
        // whole; a `next` link fails `internal` rather than printing part of the list.
        Vms::List => print_envelope(
            ctx,
            client,
            &Request::to(&compute::VMS, []).about("list the VMs"),
        ),
        Vms::Get(VmTarget { vm_id }) => {
            print_body(ctx, client, &of_vm(&compute::VM, "get", vm_id), Gate::Open)
        }
        Vms::Attestation(VmTarget { vm_id }) => {
            let request = of_vm(&compute::VM_ATTESTATION, "get the attestation of", vm_id);
            print_with(ctx, client, &request, Gate::Open, |response| {
                let attestation = response.json()?;
                Ok(match ctx.mode() {
                    Mode::Text => for_a_person(attestation),
                    Mode::Json => attestation,
                })
            })
        }
        // Swagger: `{failovers}`, at most 10, newest first.
        Vms::Failovers(VmTarget { vm_id }) => {
            let request = of_vm(&compute::VM_FAILOVERS, "list the failovers of", vm_id);
            print_keyed_list(ctx, client, &request, "failovers")
        }
        Vms::Create(args) => create::create(ctx, client, args),
        // Not confirmed: a start spends nothing new and deletes nothing (tenet 5), since the VM
        // is billed until it is deleted, stopped or not.
        Vms::Start(VmTarget { vm_id }) => print_body(
            ctx,
            client,
            &power(&compute::VM_START, vm_id, "start"),
            Gate::Open,
        ),
        Vms::Stop(VmTarget { vm_id }) => {
            stop(ctx, client, &power(&compute::VM_STOP, vm_id, "stop"), vm_id)
        }
        Vms::Reboot(VmTarget { vm_id }) => {
            let gate = Gate::Confirm { consequence: None };
            print_body(
                ctx,
                client,
                &power(&compute::VM_REBOOT, vm_id, "reboot"),
                gate,
            )
        }
        Vms::ResizeOptions(VmTarget { vm_id }) => {
            let request = of_vm(
                &compute::VM_RESIZE_OPTIONS,
                "get the resize options of",
                vm_id,
            );
            print_body(ctx, client, &request, Gate::Open)
        }
        // The command does not wait for `done`; `resize-status` follows it. Operations that
        // finish later get a status command and no new wait.
        Vms::Resize {
            vm: VmTarget { vm_id },
            flavor,
        } => {
            let request = Request::to(&compute::VM_RESIZE, [vm_id])
                .json(&compute::VmResizeBody { flavor })
                .listed_by(VMS_LIST_COMMAND);
            let request = request.about(format!("resize the VM `{vm_id}` to `{flavor}`"));
            let consequence = "a running VM reboots, and the bill changes to the new size";
            let gate = Gate::Confirm {
                consequence: Some(consequence),
            };
            print_with(ctx, client, &request, gate, |response| {
                let body = response.document()?;
                // clap accepted the typed vm_id; one that starts with `-` came after `--`,
                // and a printed command would read it as a flag, so it gets no note.
                let vm = crate::cli::as_argument(vm_id.as_str(), crate::cli::VmId::parse);
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
        Vms::ResizeStatus {
            vm: VmTarget { vm_id },
            resize_id,
        } => print_body(
            ctx,
            client,
            &Request::to(&compute::VM_RESIZE_STATUS, [vm_id, resize_id])
                .about(format!("get the resize `{resize_id}` of the VM `{vm_id}`"))
                .listed_by(VMS_LIST_COMMAND),
            Gate::Open,
        ),
        Vms::PowerPolicy(command) => power_policy_command(ctx, client, command),
        Vms::Delete(VmTarget { vm_id }) => vm_delete(ctx, client, vm_id),
    }
}

/// `DELETE .../<vm_id>/`, after a read of the VM whose `name` the question gives, so the
/// person checks the name and not only the id. A dry run sends neither, and lists both.
fn vm_delete(ctx: &Context<'_>, client: &Client, vm_id: &VmId) -> Result<Printed> {
    let delete = |named: &str| {
        Request::to(&compute::VM_DELETE, [&vm_id])
            .listed_by(VMS_LIST_COMMAND)
            .about(format!("delete the VM {named} and its disks"))
    };
    let read = of_vm(&compute::VM, "get", vm_id);
    let draft = delete(&format!("`{vm_id}`"));
    let decide = || {
        let vm: Vm = view_of(&ctx.send(client, &read)?.document()?);
        let named = match vm.name {
            Some(name) => format!("{} (`{vm_id}`)", phrase::quote(name)),
            None => format!("`{vm_id}`"),
        };
        Ok(Decided::asked(delete(&named), Some(VM_LEFT_BILLED)))
    };
    let plan = WritePlan::new(&[&read], &draft);
    read_then_write(ctx, client, &plan, decide, |sent| sent?.document())
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
