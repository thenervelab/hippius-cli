//! `hipc network`: the account's private network (`NetBird`), under `/api/network/`.
//!
//! Every route here acts in the selected account, so a membership sends
//! `X-Hippius-Account`; the network is account-wide, so a project or a billing account is
//! refused. Both come from the route tables in the library, not from this module.
//!
//! Swagger documents no answer body for these routes and no page parameters, so a list
//! is read whole: a bare array, or a page with no `next` link. A `next` link is `internal`
//! rather than a short list printed as complete. None of the answers here is documented, or
//! shown by the console's network types, to hold a secret: the writes answer the group,
//! policy, VPC network, resource, or router they made or changed, and the isolation switch
//! the network's summary. The routes that do, or may, issue one (a peer enrollment and the
//! enroll keys) are left to the console and `hipc api`, which guards the enrollment with
//! `--reveal`.

mod groups;

use hippius::phrase::quote;
use hippius::routes::Route;
use hippius::routes::account::NameBody;
use hippius::routes::network::{self, IsolationBody, PolicyBody, ResourceBody, RouterCreateBody};
use hippius::{Error, Failure, ISSUES_URL, Request, Response, Result, view_of};
use serde_json::{Value, json};

use crate::cli::{
    Groups, Isolation, IsolationMode, Network, Peers, Policies, PolicyAction, PolicyCreate,
    PolicyFields, PolicyUpdate, Protocol, ResourceFields, ResourceKind, ResourceUpdate,
    VpcResources, VpcRouters, Vpcs,
};
use crate::commands::replace::{Entry, Found, Replace, replace_list};
use crate::commands::{
    Printed, print_body, print_body_or, print_envelope, print_with, run_hint, set_up_first,
    unreadable,
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
                .about("get the private network, creating it if the account has none");
            print_body(ctx, &ctx.load_authed_client()?, &request, creation_gate())
        }
        // Swagger: idempotent, no body, and it answers the summary `get` reads.
        Network::Create => {
            let request = Request::to(&network::PROVISION, []).about("create the private network");
            print_body(ctx, &ctx.load_authed_client()?, &request, creation_gate())
        }
        Network::Peers(command) => peers(ctx, command),
        Network::Groups(command) => groups(ctx, command),
        Network::Policies(command) => policies(ctx, command),
        Network::Vpcs(command) => vpcs(ctx, command),
        Network::Isolation(command) => isolation(ctx, command),
        Network::Topology => topology(ctx),
    }
}

fn groups(ctx: &Context<'_>, command: &Groups) -> Result<Printed> {
    match command {
        Groups::List => list(ctx, &network::GROUPS, "list the network's groups"),
        // A new group has no peers and no policy, so it changes no access: it is not gated
        // (tenet 5), as a CDN list's create is not.
        Groups::Create { name } => {
            let request = Request::to(&network::GROUP_CREATE, [])
                .json(&network::GroupCreateBody { label: name })
                .about(format!("create the network group `{name}`"));
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        Groups::Update {
            group_id,
            peers,
            remove_others,
        } => groups::set_peers(ctx, *group_id, peers, *remove_others),
        Groups::Delete { group_id } => {
            let request = Request::to(&network::GROUP_DELETE, [group_id])
                .listed_by("hipc network groups list")
                .about(format!("delete the network group `{group_id}`"));
            let gate = Gate::Confirm {
                consequence: Some(GROUP_DELETED),
            };
            let done = json!({ "id": group_id, "deleted": true });
            print_body_or(ctx, &ctx.load_authed_client()?, &request, gate, done)
        }
    }
}

fn policies(ctx: &Context<'_>, command: &Policies) -> Result<Printed> {
    match command {
        Policies::List => list(ctx, &network::POLICIES, "list the network's policies"),
        Policies::Create(PolicyCreate {
            source_group,
            dest_group,
            fields,
        }) => {
            let body = policy_body(fields, Some(*source_group), Some(*dest_group))?;
            let request = Request::to(&network::POLICY_CREATE, [])
                .json(&body)
                .about(format!(
                    "create a network policy from the group `{source_group}` to the group \
                     `{dest_group}`"
                ));
            let gate = Gate::Confirm {
                consequence: Some(ACCESS_CHANGES),
            };
            print_body(ctx, &ctx.load_authed_client()?, &request, gate)
        }
        Policies::Update(PolicyUpdate {
            policy_id,
            source_group,
            dest_group,
            fields,
            remove_others,
        }) => {
            let body = policy_body(fields, *source_group, *dest_group)?;
            let request = Request::to(&network::POLICY_UPDATE, [policy_id])
                .json(&body)
                .listed_by("hipc network policies list")
                .about(format!(
                    "change the network policy `{policy_id}`: {}",
                    hippius::phrase::ticked(sent_keys(&body), "and")
                ));
            let client = ctx.load_authed_client()?;
            let Some(ports) = body.ports else {
                let gate = Gate::Confirm {
                    consequence: Some(ACCESS_CHANGES),
                };
                return print_body(ctx, &client, &request, gate);
            };
            // `ports` replaces the policy's whole list, so a port it leaves out is a removal.
            let replace = Replace {
                write: request,
                consequence: Some(ACCESS_CHANGES),
                entries: ports
                    .iter()
                    .map(|port| Entry::new(port.as_str().into()))
                    .collect(),
                remove_others: *remove_others,
                // There is no read of one policy: the list holds every policy's ports.
                read: Request::to(&network::POLICIES, []).about("list the network's policies"),
                draft_body: None,
                draft_if_match: None,
                noun: "port",
                owner: format!("the network policy `{policy_id}`"),
            };
            replace_list(ctx, &client, &replace, |answer| {
                let policy: network::Policy =
                    listed(answer, "/network/policies/", *policy_id, "policy")?;
                let ports = policy
                    .ports
                    .ok_or_else(|| unreadable("list the network's policies"))?;
                Ok(Found::new(
                    ports
                        .into_iter()
                        .map(|port| Entry::new(port.into()))
                        .collect(),
                ))
            })
        }
        Policies::Delete { policy_id } => {
            let request = Request::to(&network::POLICY_DELETE, [policy_id])
                .listed_by("hipc network policies list")
                .about(format!("delete the network policy `{policy_id}`"));
            let gate = Gate::Confirm {
                consequence: Some(POLICY_DELETED),
            };
            let done = json!({ "id": policy_id, "deleted": true });
            print_body_or(ctx, &ctx.load_authed_client()?, &request, gate, done)
        }
    }
}

/// The body of a policy create or update: the groups given and every field of `fields` that
/// was. A port is sent only with a protocol that has ports, since `NetBird` ignores the ports
/// of the others; an update must name the protocol too, as hipc does not read the one the
/// policy has.
///
/// # Errors
///
/// `usage` when `--port` is given without `--protocol tcp` or `--protocol udp`.
fn policy_body(
    fields: &PolicyFields,
    source_group: Option<u64>,
    dest_group: Option<u64>,
) -> Result<PolicyBody<'_>> {
    let ports = (!fields.ports.is_empty()).then_some(fields.ports.as_slice());
    if ports.is_some() && !fields.protocol.is_some_and(Protocol::has_ports) {
        return Err(Error::usage(
            "`--port` needs `--protocol tcp` or `--protocol udp`: the other protocols have no \
             ports",
            "pass `--protocol tcp` or `--protocol udp` with `--port`, or remove `--port`",
        ));
    }
    Ok(PolicyBody {
        name: fields.name.as_deref(),
        source_group,
        dest_group,
        protocol: fields.protocol.map(Protocol::code),
        ports,
        bidirectional: fields.bidirectional,
        action: fields.action.map(PolicyAction::code),
        enabled: fields.enabled,
    })
}

/// The top-level keys `body` sends, in the order it sends them: an update's question names
/// what it changes, as the fields go out.
fn sent_keys(body: &impl serde::Serialize) -> Vec<String> {
    crate::commands::sent_fields(body)
        .into_iter()
        .map(|(key, _)| key)
        .collect()
}

fn vpcs(ctx: &Context<'_>, command: &Vpcs) -> Result<Printed> {
    match command {
        Vpcs::List => list(ctx, &network::VPCS, "list the VPC networks"),
        // A new VPC network has no resources and no routers, so it reaches nothing and changes
        // no access: it is not gated (tenet 5).
        Vpcs::Create { name } => {
            let request = Request::to(&network::VPC_CREATE, [])
                .json(&NameBody { name })
                .about(format!("create the VPC network {}", quote(name)));
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        Vpcs::Delete { vpc_id } => {
            let request = Request::to(&network::VPC_DELETE, [vpc_id])
                .listed_by("hipc network vpcs list")
                .about(format!("delete the VPC network `{vpc_id}`"));
            let gate = Gate::Confirm {
                consequence: Some(VPC_DELETED),
            };
            let done = json!({ "id": vpc_id, "deleted": true });
            print_body_or(ctx, &ctx.load_authed_client()?, &request, gate, done)
        }
        Vpcs::Resources(command) => resources(ctx, command),
        Vpcs::Routers(command) => routers(ctx, command),
    }
}

fn resources(ctx: &Context<'_>, command: &VpcResources) -> Result<Printed> {
    match command {
        VpcResources::List { vpc_id } => list_of_vpc(
            ctx,
            &network::VPC_RESOURCES,
            vpc_id,
            &format!("list the resources of the VPC network `{vpc_id}`"),
        ),
        VpcResources::Create {
            vpc_id,
            name,
            address,
            fields,
        } => {
            let body = resource_body(Some(name), Some(address), fields);
            let request = Request::to(&network::VPC_RESOURCE_CREATE, [vpc_id])
                .json(&body)
                .listed_by("hipc network vpcs list")
                .about(format!(
                    "add the resource {} to the VPC network `{vpc_id}`",
                    quote(address)
                ));
            let gate = Gate::Confirm {
                consequence: Some(RESOURCE_REACHED),
            };
            print_body(ctx, &ctx.load_authed_client()?, &request, gate)
        }
        VpcResources::Update(ResourceUpdate {
            vpc_id,
            resource_id,
            name,
            address,
            fields,
            remove_others,
        }) => {
            let body = resource_body(name.as_deref(), address.as_deref(), fields);
            let request = Request::to(&network::VPC_RESOURCE_UPDATE, [vpc_id, resource_id])
                .json(&body)
                .listed_by(format!("hipc network vpcs resources list {vpc_id}"))
                .about(format!(
                    "change the resource `{resource_id}` of the VPC network `{vpc_id}`: {}",
                    hippius::phrase::ticked(sent_keys(&body), "and")
                ));
            let client = ctx.load_authed_client()?;
            let Some(groups) = body.groups else {
                let gate = Gate::Confirm {
                    consequence: Some(ACCESS_CHANGES),
                };
                return print_body(ctx, &client, &request, gate);
            };
            // `groups` replaces the resource's whole list, so a group it leaves out is a
            // removal: its peers lose their route to the resource.
            let list = format!("list the resources of the VPC network `{vpc_id}`");
            let replace = Replace {
                write: request,
                consequence: Some(ACCESS_CHANGES),
                entries: groups
                    .iter()
                    .map(|&group| Entry::new(group.into()))
                    .collect(),
                remove_others: *remove_others,
                // There is no read of one resource: the list holds every resource's groups.
                read: Request::to(&network::VPC_RESOURCES, [vpc_id])
                    .listed_by("hipc network vpcs list")
                    .about(list.clone()),
                draft_body: None,
                draft_if_match: None,
                noun: "group",
                owner: format!("the resource `{resource_id}`"),
            };
            let path = format!("/network/networks/{vpc_id}/resources/");
            replace_list(ctx, &client, &replace, |answer| {
                let resource: network::Resource = listed(answer, &path, *resource_id, "resource")?;
                let groups = resource.groups.ok_or_else(|| unreadable(&list))?;
                Ok(Found::new(
                    groups
                        .into_iter()
                        .map(|group| Entry::new(group.into()))
                        .collect(),
                ))
            })
        }
        VpcResources::Delete {
            vpc_id,
            resource_id,
        } => {
            let request = Request::to(&network::VPC_RESOURCE_DELETE, [vpc_id, resource_id])
                .listed_by(format!("hipc network vpcs resources list {vpc_id}"))
                .about(format!(
                    "remove the resource `{resource_id}` from the VPC network `{vpc_id}`"
                ));
            let gate = Gate::Confirm {
                consequence: Some(RESOURCE_REMOVED),
            };
            let done = json!({ "id": resource_id, "vpc_id": vpc_id, "deleted": true });
            print_body_or(ctx, &ctx.load_authed_client()?, &request, gate, done)
        }
    }
}

/// The item whose `id` is `id` in `answer`, a list read whole from `path` (as `hipc api` takes
/// it), as a view: what a list replace compares against.
///
/// # Errors
///
/// `internal` for an answer that is not a list, or one that names a next page; `not_found`
/// when no item has the id, naming the list that shows what exists.
fn listed<T: serde::de::DeserializeOwned + Default>(
    answer: Value,
    path: &str,
    id: u64,
    what: &str,
) -> Result<T> {
    let items = hippius::page(answer)
        .map_err(|_| unreadable(&format!("read the {what} list")))?
        .into_complete(path)?;
    let id = id.to_string();
    items
        .iter()
        .find(|item| {
            let item_id = item.get("id").cloned().map(serde_json::from_value);
            matches!(item_id, Some(Ok(hippius::TextOrNumber(text))) if text == id)
        })
        .map(view_of)
        .ok_or_else(|| {
            Error::new(
                Failure::NotFound,
                format!("cannot change the {what} `{id}`: the API does not list it"),
            )
            .with_hint(format!("run `hipc api GET {path}` to see what exists"))
        })
}

/// The body of a resource create or update: every field given.
fn resource_body<'a>(
    name: Option<&'a str>,
    address: Option<&'a str>,
    fields: &'a ResourceFields,
) -> ResourceBody<'a> {
    ResourceBody {
        name,
        address,
        kind: fields.kind.map(ResourceKind::code),
        groups: (!fields.groups.is_empty()).then_some(fields.groups.as_slice()),
    }
}

fn routers(ctx: &Context<'_>, command: &VpcRouters) -> Result<Printed> {
    match command {
        VpcRouters::List { vpc_id } => list_of_vpc(
            ctx,
            &network::VPC_ROUTERS,
            vpc_id,
            &format!("list the routers of the VPC network `{vpc_id}`"),
        ),
        VpcRouters::Create {
            vpc_id,
            peer,
            masquerade,
            metric,
        } => {
            let body = RouterCreateBody {
                peer: *peer,
                masquerade: *masquerade,
                metric: *metric,
            };
            let request = Request::to(&network::VPC_ROUTER_CREATE, [vpc_id])
                .json(&body)
                .listed_by("hipc network vpcs list")
                .about(format!(
                    "make the peer `{peer}` a router of the VPC network `{vpc_id}`"
                ));
            let gate = Gate::Confirm {
                consequence: Some(ROUTED_THROUGH),
            };
            print_body(ctx, &ctx.load_authed_client()?, &request, gate)
        }
        VpcRouters::Delete { vpc_id, router_id } => {
            let request = Request::to(&network::VPC_ROUTER_DELETE, [vpc_id, router_id])
                .listed_by(format!("hipc network vpcs routers list {vpc_id}"))
                .about(format!(
                    "remove the router `{router_id}` from the VPC network `{vpc_id}`"
                ));
            let gate = Gate::Confirm {
                consequence: Some(ROUTER_REMOVED),
            };
            let done = json!({ "id": router_id, "vpc_id": vpc_id, "deleted": true });
            print_body_or(ctx, &ctx.load_authed_client()?, &request, gate, done)
        }
    }
}

fn isolation(ctx: &Context<'_>, command: &Isolation) -> Result<Printed> {
    match command {
        // Only the summary's and the topology's reads are documented to create the network;
        // this one is not known not to, so it is sent only once the group list, which answers
        // 403 for a network never set up, shows the network exists, as the topology is.
        Isolation::Get => {
            let client = ctx.load_authed_client()?;
            let (probe, _) = topology_reads();
            let read =
                Request::to(&network::ISOLATION, []).about("get the network's isolation mode");
            if ctx.dry_run() {
                return ctx.dry_run_of(&client, &[&probe, &read]).map(Printed::from);
            }
            ctx.send(&client, &probe)?;
            Ok(Printed::Document(ctx.send(&client, &read)?.document()?))
        }
        Isolation::Update {
            mode,
            change_isolation,
        } => {
            require_change_isolation(ctx, *change_isolation)?;
            let request = Request::to(&network::ISOLATION_UPDATE, [])
                .json(&IsolationBody {
                    isolation_mode: mode.code(),
                })
                .about(format!(
                    "switch the private network's isolation to `{}`",
                    mode.code()
                ));
            let gate = Gate::Confirm {
                consequence: Some(isolation_consequence(*mode)),
            };
            print_body(ctx, &ctx.load_authed_client()?, &request, gate)
        }
    }
}

/// Refuses an isolation switch without `--change-isolation`, as `hipc api` refuses the
/// route: the one flag of the route's row, so the two cannot drift. A dry run sends nothing,
/// so it needs no flag (tenet 5). It comes before the config file is read, as `--public`
/// does.
fn require_change_isolation(ctx: &Context<'_>, change_isolation: bool) -> Result<()> {
    if change_isolation || ctx.dry_run() {
        return Ok(());
    }
    Err(Error::change_isolation_required(ctx.args()))
}

/// What a switch to `mode` does, for its question.
fn isolation_consequence(mode: IsolationMode) -> &'static str {
    match mode {
        IsolationMode::AllowIntra => {
            "every peer of the network can reach every other, and policies can only widen that"
        }
        IsolationMode::DenyIntra => {
            "peers reach each other only where a policy allows it, and every other connection \
             between them closes"
        }
    }
}

/// `hipc network topology`: the group list first, which answers 403 when the network was
/// never set up and creates nothing, then the topology, whose first read for an account
/// would create the network (its row `provisions`). After the group list answered, the
/// network exists, so the topology read creates nothing either and is sent as a read: it
/// needs no confirmation, and a failure of it has no side effects.
fn topology(ctx: &Context<'_>) -> Result<Printed> {
    let client = ctx.load_authed_client()?;
    let (probe, read) = topology_reads();
    if ctx.dry_run() {
        return ctx.dry_run_of(&client, &[&probe, &read]).map(Printed::from);
    }
    ctx.send(&client, &probe)?;
    let body = ctx.send(&client, &read)?.document()?;
    Ok(Printed::Document(body))
}

/// The two reads of [`topology`], each named: the group list that shows the network exists,
/// and the topology read, sent as a read once it does.
pub(super) fn topology_reads() -> (Request, Request) {
    let probe = Request::to(&network::GROUPS, []).about("list the network's groups");
    let read = Request::to(&network::TOPOLOGY, [])
        .changes_nothing()
        .about("read the network's topology");
    (probe, read)
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
                .json(&network::PeerUpdateBody { name })
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
    let id = view_of::<network::Peer>(&answer).id;
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

/// What a group delete ends, for its question and `hipc api`'s.
pub(crate) const GROUP_DELETED: &str =
    "its peers lose the access its policies give them, and a group made again gets a new id";

/// What a policy delete ends, for its question and `hipc api`'s.
pub(crate) const POLICY_DELETED: &str =
    "the traffic it allowed is no longer allowed by it, and traffic it dropped may pass";

/// What a VPC network delete ends, for its question and `hipc api`'s.
pub(crate) const VPC_DELETED: &str = "peers lose the routes it gave them to its resources";

/// What a resource removal ends, for its question and `hipc api`'s.
pub(crate) const RESOURCE_REMOVED: &str = "peers lose their route to it";

/// What a router removal ends, for its question and `hipc api`'s.
pub(crate) const ROUTER_REMOVED: &str = "the VPC network's traffic no longer goes through that \
                                         peer, and with no router left its resources are out of \
                                         reach";

/// What a policy create or update, or a resource update, does, for its question.
const ACCESS_CHANGES: &str = "it changes which peers of the network can reach what";

/// What a resource create does, for its question.
const RESOURCE_REACHED: &str =
    "the peers of its groups can reach it through the VPC network's routers";

/// What a router create does, for its question.
const ROUTED_THROUGH: &str = "the VPC network's traffic goes through that peer";

#[cfg(test)]
mod tests {
    use hippius::routes::{Guard, RouteFlag};

    use super::*;

    /// The isolation switch asks for the flag its route's row names, so the curated command
    /// and `hipc api` refuse the same way.
    #[test]
    fn the_isolation_switch_needs_the_flag_its_row_names() {
        assert_eq!(
            network::ISOLATION_UPDATE.endpoint().guard(),
            Guard::Needs(&[RouteFlag::ChangeIsolation])
        );
    }
}
