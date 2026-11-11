//! `hipc compute vms power-policy`: what a power-off from inside the VM does.

use hippius::routes::compute::{self, Vm};
use hippius::{Client, Request, Response, Result, phrase, view_of};
use serde_json::{Map, Value};

use crate::cli::{GuestPoweroff, PowerPolicy};
use crate::commands::compute::VMS;
use crate::commands::{Printed, print_with};
use crate::context::{Context, Gate};

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
    match (view_of::<Vm>(&vm).on_guest_poweroff, sent) {
        (Some(policy), Some(sent)) if policy != sent.code() => {
            return Err(response.refused(&format!(
                "the API answered with `on_guest_poweroff` {}",
                phrase::quote(policy)
            )));
        }
        (Some(_), _) => {}
        (None, _) => return Err(response.refused("the API answered without `on_guest_poweroff`")),
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
pub(super) fn power_policy_command(
    ctx: &Context<'_>,
    client: &Client,
    command: &PowerPolicy,
) -> Result<Printed> {
    match command {
        PowerPolicy::Get { vm_id } => {
            let request = Request::to(&compute::VM, [vm_id])
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
    let request = Request::to(&compute::VM_POWER_POLICY_UPDATE, [&vm_id])
        .json(&compute::PowerPolicyBody {
            on_guest_poweroff: policy.code(),
        })
        .listed_by(VMS)
        .about(format!(
            "set the VM `{vm_id}` to {does} when its own operating system powers it off"
        ));
    print_with(ctx, client, &request, Gate::Open, |response| {
        let answer = power_policy(&response, vm_id, Some(policy))?;
        // A server without `stop` restarts the VM meanwhile, which the pending note says; the
        // bill of a stopped VM would contradict it.
        if policy == GuestPoweroff::Stop && !host_unsupported(&view_of(&answer)) {
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
    let policy: Vm = view_of(answer);
    if policy.on_guest_poweroff_pending != Some(true) {
        return;
    }
    if host_unsupported(&policy) {
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

/// Whether the VM's server does not support `stop` (`on_guest_poweroff_reason`
/// `host-unsupported`), and restarts the VM meanwhile.
fn host_unsupported(policy: &Vm) -> bool {
    policy.on_guest_poweroff_reason.as_deref() == Some("host-unsupported")
}
