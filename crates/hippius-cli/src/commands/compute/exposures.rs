//! `hipc compute exposures`: a VM port served on a hostname through the edge.

use hippius::routes::compute;
use hippius::{Error, Request, Result, phrase, view_of};
use serde_json::json;

use crate::cli::{ExposeMode, ExposeProtocol, ExposureCreate, Exposures};
use crate::commands::compute::VMS;
use crate::commands::{Printed, print_body_or, print_envelope, print_with};
use crate::context::{Context, Gate};

pub(super) fn exposures(ctx: &Context<'_>, command: &Exposures) -> Result<Printed> {
    match command {
        // Swagger types the list as a bare array with no page parameters.
        Exposures::List { vm_id } => {
            let request = Request::to(&compute::EXPOSURES, [vm_id])
                .about(format!("list the exposures of the VM `{vm_id}`"))
                .listed_by(VMS);
            print_envelope(ctx, &ctx.load_authed_client()?, &request)
        }
        Exposures::Create(args) => exposure_create(ctx, args),
        Exposures::Delete { vm_id, exposure_id } => {
            let request = Request::to(&compute::EXPOSURE_DELETE, [vm_id, exposure_id])
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
    let request = Request::to(&compute::EXPOSURE_CREATE, [&args.vm_id])
        .json(&compute::ExposureCreateBody {
            port: args.port,
            mode: args.mode.map(ExposeMode::code),
            protocol: args.protocol.map(ExposeProtocol::code),
        })
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
        let seen: compute::Exposure = view_of(&exposure);
        // The server's `edge_sees_traffic` decides the note, not the mode that was asked
        // for. Swagger sets it in `raw` mode too, where the edge reads the traffic only when
        // the protocol inside does not encrypt it, so the note repeats the flag and claims
        // no more.
        if seen.edge_sees_traffic == Some(true) {
            let endpoint = seen
                .endpoint
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

/// What an exposure delete gives up, for its question and `hipc api`'s.
pub(crate) const EXPOSURE_DELETED: &str = "its hostname cannot be used again";
