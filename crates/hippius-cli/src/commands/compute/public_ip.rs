//! `hipc compute public-ip`: the public IPv4 of a VM.

use hippius::routes::compute;
use hippius::{Client, Request, Result};
use serde_json::json;

use crate::cli::{PublicIp, VmTarget};
use crate::commands::compute::VMS;
use crate::commands::{Printed, print_body, print_body_or, print_with};
use crate::context::{Context, Gate};

pub(super) fn public_ip(ctx: &Context<'_>, client: &Client, command: &PublicIp) -> Result<Printed> {
    match command {
        PublicIp::Availability { region } => {
            let request = Request::to(&compute::PUBLIC_IP_AVAILABILITY, [])
                .about("get the public IPv4 availability");
            let request = match region {
                Some(region) => request.query("region", region),
                None => request,
            };
            print_body(ctx, client, &request, Gate::Open)
        }
        PublicIp::Get(VmTarget { vm_id }) => print_body(
            ctx,
            client,
            &Request::to(&compute::PUBLIC_IP, [vm_id])
                .about(format!("get the public IPv4 of the VM `{vm_id}`"))
                .listed_by(VMS),
            Gate::Open,
        ),
        // Swagger documents the attach as idempotent: a repeat returns the address already
        // attached. It still needs `--yes`, because the first call starts a bill.
        PublicIp::Create(VmTarget { vm_id }) => {
            let request = Request::to(&compute::PUBLIC_IP_CREATE, [vm_id])
                .listed_by(VMS)
                .about(format!("attach a public IPv4 to the VM `{vm_id}`"));
            let gate = Gate::Confirm {
                consequence: Some("it is billed until it is detached or the VM is deleted"),
            };
            print_with(ctx, client, &request, gate, |response| {
                let address = response.document()?;
                ctx.warning(&inbound_blocked(vm_id.as_str()));
                Ok(address)
            })
        }
        PublicIp::Delete(VmTarget { vm_id }) => {
            let request = Request::to(&compute::PUBLIC_IP_DELETE, [vm_id])
                .listed_by(VMS)
                .about(format!("detach the public IPv4 from the VM `{vm_id}`"));
            let gate = Gate::Confirm {
                consequence: Some(IP_DETACHED),
            };
            let done = json!({ "vm_id": vm_id, "public_ip": null });
            print_body_or(ctx, client, &request, gate, done)
        }
        PublicIp::Usage {
            vm: VmTarget { vm_id },
            range,
        } => {
            let request = Request::to(&compute::PUBLIC_IP_USAGE, [vm_id])
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

/// The warning after a public IPv4 is attached, by `public-ip create` or by a launch with
/// `--public-ip`. Swagger: "All inbound traffic is blocked until you add firewall rules".
pub(super) fn inbound_blocked(vm_id: &str) -> String {
    format!(
        "all inbound traffic to the address is blocked until a firewall rule accepts it, which \
         `{}` adds",
        crate::cli::with_id("compute firewall update", vm_id, "--rules-file <path>")
    )
}

/// What a public IPv4 detach gives up, for its question and `hipc api`'s.
pub(crate) const IP_DETACHED: &str = "a later attach may get a different address";
