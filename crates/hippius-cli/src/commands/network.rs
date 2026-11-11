//! `hipc network`: the account's private network (`NetBird`), under `/api/network/`.
//!
//! Every route here acts in the selected account, so a membership sends
//! `X-Hippius-Account`; the network is account-wide, so a project or a billing account is
//! refused. Both come from the route tables in the library, not from this module.
//!
//! Swagger documents no answer body for these routes and no page parameters, so a list
//! is read whole: a bare array, or a page with no `next` link. A `next` link is `internal`
//! rather than a short list printed as complete. None of these answers is documented to
//! hold a secret. The routes that do, or may, issue one (a peer enrollment and the enroll
//! keys) are left to `hipc api`, which guards the enrollment with `--reveal`.

use hippius::phrase::quote;
use hippius::routes::{Route, network};
use hippius::{Error, ISSUES_URL, Request, Response, Result};
use serde_json::{Value, json};

use crate::cli::{Groups, Network, Peers, Policies, VpcResources, VpcRouters, Vpcs};
use crate::commands::{
    Printed, print_body, print_body_or, print_envelope, print_with, run_hint, set_up_first,
};
use crate::context::{Context, Gate};

/// What the first `GET /api/network/me/` for an account creates, as the confirmation of
/// `hipc network get` and of `hipc api` on that route names it: one text, so the two cannot
/// drift.
pub(crate) const NETWORK_CREATED: &str = concat!(
    "a new network has a default group, a VPC network, ",
    "and a policy that lets its peers reach each other"
);

/// The gate of a request that may create the network: `hipc network get` and `hipc api` on
/// the route take the same one, so they ask the same question.
pub(crate) fn creation_gate() -> Gate<'static> {
    Gate::Confirm {
        consequence: Some(NETWORK_CREATED),
    }
}

pub(crate) fn run(ctx: &Context<'_>, command: &Network) -> Result<Printed> {
    match command {
        // The create is the fix the mapping names, so its own refusal keeps the server's
        // words rather than send the caller back to it.
        Network::Create => dispatch(ctx, command),
        // One mapping around every other command, so a new one cannot miss it.
        _ => dispatch(ctx, command).map_err(|error| not_set_up(ctx, error)),
    }
}

/// `error`, said plainly when it is the API's answer to an account whose network was never
/// created ([`set_up_first`]). It keeps `permission`, the code of a 403 without an account
/// code (tenet 4), with its status, `details`, and side effects; the server's words in the
/// message give way to what they mean, and the hint names the command that fixes it, with
/// this run's profile, origin, and account, so it creates the network this run read.
fn not_set_up(ctx: &Context<'_>, error: Error) -> Error {
    // The API's 403 names the route `hipc network create` sends, with its base:
    // `You must call /api/network/provision/ first.`
    let provision = network::PROVISION.endpoint().template();
    if !set_up_first(&error, provision) {
        return error;
    }
    // The message is `cannot <action>: the API says <the sentence>`; the sentence is known
    // whole, so it is cut off exactly, whatever the action holds.
    let says = format!(
        ": the API says {}",
        quote(format!("You must call {provision} first."))
    );
    let message = error.message();
    let action = message.strip_suffix(says.as_str()).unwrap_or(message);
    let message = format!("{action}: the private network is not set up for this account");
    let hint = run_hint(
        ctx,
        "network create --yes",
        "to create the account's private network",
    );
    error.with_message(message).with_hint(hint)
}

fn dispatch(ctx: &Context<'_>, command: &Network) -> Result<Printed> {
    match command {
        // Swagger: the first `GET /api/network/me/` for an account creates its network, and
        // the same GET answers every later call. Nothing tells the two apart before the
        // request, so every call is confirmed and treated as a write.
        Network::Get => {
            let request = Request::to(&network::ME, [])
                .provisions()
                .about("get the private network, creating it if the account has none");
            print_body(ctx, &ctx.load_authed_client()?, &request, creation_gate())
        }
        // Swagger: idempotent, no body, and it answers the summary `get` reads.
        Network::Create => {
            let request = Request::to(&network::PROVISION, []).about("create the private network");
            print_body(ctx, &ctx.load_authed_client()?, &request, creation_gate())
        }
        Network::Peers(command) => peers(ctx, command),
        Network::Groups(Groups::List) => list(ctx, &network::GROUPS, "list the network's groups"),
        Network::Policies(Policies::List) => {
            list(ctx, &network::POLICIES, "list the network's policies")
        }
        Network::Vpcs(Vpcs::List) => list(ctx, &network::VPCS, "list the VPC networks"),
        Network::Vpcs(Vpcs::Resources(VpcResources::List { vpc_id })) => list_of_vpc(
            ctx,
            &network::VPC_RESOURCES,
            vpc_id,
            &format!("list the resources of the VPC network `{vpc_id}`"),
        ),
        Network::Vpcs(Vpcs::Routers(VpcRouters::List { vpc_id })) => list_of_vpc(
            ctx,
            &network::VPC_ROUTERS,
            vpc_id,
            &format!("list the routers of the VPC network `{vpc_id}`"),
        ),
    }
}

fn peers(ctx: &Context<'_>, command: &Peers) -> Result<Printed> {
    match command {
        Peers::List => list(ctx, &network::PEERS, "list the network's peers"),
        Peers::Get { peer_id } => {
            let request = Request::to(&network::PEER, [peer_id])
                .listed_by("hipc network peers list")
                .about(format!("get the network peer `{peer_id}`"));
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        // Swagger documents no body for this PATCH, and the console has no rename to copy.
        // `name` is NetBird's field for a peer's name; not yet verified against the live API,
        // which is why `renamed` trusts nothing but an answer that carries the new name.
        Peers::Update { peer_id, name } => {
            let request = Request::to(&network::PEER_UPDATE, [peer_id])
                .json(&json!({ "name": name }))
                .listed_by("hipc network peers list")
                .about(format!(
                    "rename the network peer `{peer_id}` to {}",
                    hippius::phrase::quote(name)
                ));
            let gate = Gate::Confirm { consequence: None };
            print_with(
                ctx,
                &ctx.load_authed_client()?,
                &request,
                gate,
                |response| renamed(&response, peer_id, name),
            )
        }
        Peers::Delete { peer_id } => {
            let request = Request::to(&network::PEER_DELETE, [peer_id])
                .listed_by("hipc network peers list")
                .about(format!(
                    "remove the peer `{peer_id}` from the private network"
                ));
            let gate = Gate::Confirm {
                consequence: Some(PEER_REMOVED),
            };
            let done = json!({ "id": peer_id, "deleted": true });
            print_body_or(ctx, &ctx.load_authed_client()?, &request, gate, done)
        }
    }
}

/// The answer to a rename, checked: the field the rename sent, `name`, has not been verified
/// against the live API, and a server that ignores an unknown field would answer 200 with
/// the old name, or with a body that has no name at all. Only an answer whose `name` is the
/// requested one, and whose `id`, when it has one, is the peer's, is a rename. Any other
/// 2xx is `internal` with `partial` side effects, and
/// nothing goes to stdout: the PATCH was sent, and may or may not have renamed the peer, so
/// printing the answer would record a rename `hipc` cannot confirm. The answer travels in
/// the error's `details`, with the status and the request id, as a server error's would.
/// The hint reads the peer rather than running the rename again: the PATCH's effect is
/// unknown, and the read prints the name the server holds (tenet 15).
fn renamed(response: &Response, peer_id: &str, name: &str) -> Result<Value> {
    let answer = response.document()?;
    // The message names the requested name through the action; the why quotes the server's
    // (tenet 15), as the JSON literal when it is not a string. Swagger types the id as a
    // string, and the list may carry it as a number, so either is compared as text; an
    // answer without an `id`, or with one of another type, is judged by its `name`.
    let id = match answer.get("id") {
        Some(Value::String(id)) => Some(id.clone()),
        Some(Value::Number(id)) => Some(id.to_string()),
        _ => None,
    };
    let why = match (id, answer.get("name")) {
        (Some(id), _) if id != peer_id => {
            format!("the API answered about another peer, {}", quote(id))
        }
        (_, Some(sent)) if sent.as_str() == Some(name) => return Ok(answer),
        (_, Some(sent)) => format!(
            "the API says {}",
            sent.as_str().map_or_else(|| quote(sent), quote)
        ),
        (_, None) => "the API answered without `name`".to_owned(),
    };
    Err(response.refused(&why).with_hint(format!(
        "run `hipc network peers get {peer_id}` to see the peer's name, and report \
         what it prints at `{ISSUES_URL}`"
    )))
}

/// `GET` of `route`, a list read whole and printed as the list envelope.
fn list(ctx: &Context<'_>, route: &Route<0>, action: &str) -> Result<Printed> {
    let request = Request::to(route, []).about(action);
    print_envelope(ctx, &ctx.load_authed_client()?, &request)
}

/// `GET` of `route`, a list of the VPC network `vpc_id`, read as [`list`] reads one. A 404
/// names the list of VPC networks.
fn list_of_vpc(ctx: &Context<'_>, route: &Route<1>, vpc_id: &str, action: &str) -> Result<Printed> {
    let request = Request::to(route, [&vpc_id])
        .listed_by("hipc network vpcs list")
        .about(action);
    print_envelope(ctx, &ctx.load_authed_client()?, &request)
}

/// What a peer removal ends, for its question and `hipc api`'s.
pub(crate) const PEER_REMOVED: &str = "it loses its access to the private network";
