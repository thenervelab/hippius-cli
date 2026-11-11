//! `hipc compute kubernetes`: managed Kubernetes.
//!
//! Every route here is under `/api/kubernetes/`, which is account-scoped and takes a
//! project, so a membership sends `X-Hippius-Account` and a selected project
//! `X-Hippius-Project`. `/api/kubernetes/agent/` is personal and not wrapped.
//!
//! Swagger types none of the answers. The shapes are the console's: the cluster, pool, operation,
//! event, and refusal lists are bare arrays with no page parameters, so each is read whole into the
//! list envelope, whose `next_page_token` is null (tenet 3). The backups answer is an object,
//! printed as the envelope of its `snapshots`, with its other fields beside them. The POSTs send
//! `{}` where they carry nothing, as the console does.
//!
//! The backups answer holds a presigned S3 download link and the sealed key of each etcd
//! snapshot. A link fetches the snapshot for minutes without a token, so the command needs
//! `--reveal`, as `hipc api` does on that route (tenet 9).
//!
//! A cluster delete, a cancel, and a resume read first, so the question names what they act
//! on: the cluster's name, and the operation's state, its kind, and what going on with it
//! does. That read changes nothing (tenet 5). The read also refuses, as `conflict`, what the
//! operation's state rules out: a resume of an operation that is neither `blocked` nor
//! `queued`, a cancel of one that is over, and a resume of an operation blocked because data
//! on a node would be lost, since only the console can sign the `accept_data_loss` it needs.
//! Each is the server's state blocking the command, not a wrong invocation, so none is
//! `usage`.
//!
//! The commands that take the cluster key file, the kubeconfig, a scale, and a node
//! replacement, are in [`keyed`].
//!
//! Nothing here waits: a delete, a node removal, a resume, a scale, and a replacement each
//! start work the masters finish later, and `clusters get` and `operations list` read its
//! progress.

mod keyed;

use hippius::routes::kubernetes::{self, Operation, OperationKind, OperationState};
use hippius::routes::{EmptyBody, Route};
use hippius::{Client, Error, Failure, Request, Result, phrase, view_of};
use serde_json::{Map, Value};

use crate::cli::{
    ClusterTarget, Clusters, Kubernetes, KubernetesBackups, NodePoolCreate, NodePools, Nodes,
    OperationTarget, Operations,
};
use crate::commands::{
    Decided, Printed, print_body, print_envelope, print_keyed_list, read_then_write, require_reveal,
};
use crate::context::{Context, Gate, WritePlan};

const CLUSTERS_LIST_COMMAND: &str = "hipc compute kubernetes clusters list";

pub(super) fn run(ctx: &Context<'_>, command: &Kubernetes) -> Result<Printed> {
    match command {
        Kubernetes::Clusters(command) => clusters(ctx, command),
        Kubernetes::Backups(KubernetesBackups::List {
            cluster: ClusterTarget { cluster_id },
            reveal,
        }) => {
            require_reveal(ctx, *reveal)?;
            let request = Request::to(&kubernetes::BACKUPS, [cluster_id])
                .about(format!(
                    "get the backups of the Kubernetes cluster `{cluster_id}`"
                ))
                .listed_by(CLUSTERS_LIST_COMMAND);
            print_keyed_list(ctx, &ctx.load_authed_client()?, &request, "snapshots")
        }
        Kubernetes::NodePools(NodePools::List(ClusterTarget { cluster_id })) => {
            list(ctx, *cluster_id, &kubernetes::NODE_POOLS, "node pools")
        }
        Kubernetes::NodePools(NodePools::Create(args)) => pool_create(ctx, args),
        Kubernetes::NodePools(NodePools::Scale(args)) => keyed::scale(ctx, args),
        Kubernetes::Nodes(Nodes::Replace(args)) => keyed::replace(ctx, args),
        Kubernetes::Operations(Operations::List(ClusterTarget { cluster_id })) => {
            list(ctx, *cluster_id, &kubernetes::OPERATIONS, "operations")
        }
        Kubernetes::Operations(Operations::Cancel(target)) => operation(ctx, target, Verb::Cancel),
        Kubernetes::Operations(Operations::Resume(target)) => operation(ctx, target, Verb::Resume),
        Kubernetes::Nodes(Nodes::Delete {
            cluster: ClusterTarget { cluster_id },
            node_name,
        }) => {
            let request = Request::to(&kubernetes::NODE_REMOVE, [cluster_id, node_name])
                .json(&EmptyBody {})
                .about(format!(
                    "remove the node `{node_name}` from the Kubernetes cluster `{cluster_id}`, \
                     draining its pods to other nodes and decommissioning its VM; if data on the \
                     node would be lost, the operation blocks until the loss is accepted in the \
                     console"
                ))
                .listed_by(format!(
                    "hipc compute kubernetes node-pools list {cluster_id}"
                ));
            let gate = Gate::Confirm { consequence: None };
            print_body(ctx, &ctx.load_authed_client()?, &request, gate)
        }
    }
}

fn clusters(ctx: &Context<'_>, command: &Clusters) -> Result<Printed> {
    let read = |request: Request| print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open);
    match command {
        Clusters::List => print_envelope(
            ctx,
            &ctx.load_authed_client()?,
            &Request::to(&kubernetes::CLUSTERS, []).about("list the Kubernetes clusters"),
        ),
        Clusters::Get(ClusterTarget { cluster_id }) => read(cluster_get(*cluster_id)),
        Clusters::Delete(ClusterTarget { cluster_id }) => {
            cluster_delete(ctx, &ctx.load_authed_client()?, *cluster_id)
        }
        Clusters::Events(ClusterTarget { cluster_id }) => {
            list(ctx, *cluster_id, &kubernetes::CLUSTER_EVENTS, "events")
        }
        Clusters::Refusals => print_envelope(
            ctx,
            &ctx.load_authed_client()?,
            &Request::to(&kubernetes::CLUSTER_REFUSALS, [])
                .about("list the refused Kubernetes cluster launches"),
        ),
        Clusters::Kubeconfig(args) => keyed::kubeconfig(ctx, args),
    }
}

fn cluster_get(id: u64) -> Request {
    Request::to(&kubernetes::CLUSTER, [&id])
        .about(format!("get the Kubernetes cluster `{id}`"))
        .listed_by(CLUSTERS_LIST_COMMAND)
}

/// A list under a cluster, at `route`, read whole as the envelope (module docs).
fn list(ctx: &Context<'_>, id: u64, route: &Route<1>, what: &str) -> Result<Printed> {
    print_envelope(
        ctx,
        &ctx.load_authed_client()?,
        &list_request(id, route, what),
    )
}

/// The read of the list under the cluster `id` at `route`, which holds `what`.
fn list_request(id: u64, route: &Route<1>, what: &str) -> Request {
    Request::to(route, [&id])
        .about(format!("list the {what} of the Kubernetes cluster `{id}`"))
        .listed_by(CLUSTERS_LIST_COMMAND)
}

/// Sends `request`, the read of a list under a cluster, and returns its items, refusing a page
/// that is not the last as the list commands do.
fn read_list(ctx: &Context<'_>, client: &Client, request: &Request) -> Result<Vec<Value>> {
    hippius::page(ctx.send(client, request)?.json()?)?.into_complete(request.target())
}

/// `DELETE .../<id>/`, after a read of the cluster whose `name` the question gives. The
/// console reads the answer as the cluster, now `decommissioning`. A dry run sends neither,
/// and lists both.
fn cluster_delete(ctx: &Context<'_>, client: &Client, id: u64) -> Result<Printed> {
    let delete = |named: &str| {
        Request::to(&kubernetes::CLUSTER_DELETE, [&id])
            .about(format!(
                "delete the Kubernetes cluster {named}: every VM in it stops and is \
                 decommissioned, with the workloads and the data on it, and this cannot be \
                 undone"
            ))
            .listed_by(CLUSTERS_LIST_COMMAND)
    };
    let read = cluster_get(id);
    let draft = delete(&format!("`{id}`"));
    let decide = || {
        let cluster = ctx.send(client, &read)?.document()?;
        Ok(Decided::asked(delete(&phrase::named(id, &cluster)), None))
    };
    let plan = WritePlan::new(&[&read], &draft);
    read_then_write(ctx, client, &plan, decide, |sent| sent?.document())
}

/// What `operations cancel` and `operations resume` do to an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    Cancel,
    Resume,
}

impl Verb {
    /// The verb as the question says it.
    fn segment(self) -> &'static str {
        match self {
            Self::Cancel => "cancel",
            Self::Resume => "resume",
        }
    }

    /// The route the verb posts to.
    fn route(self) -> &'static Route<2> {
        match self {
            Self::Cancel => &kubernetes::OPERATION_CANCEL,
            Self::Resume => &kubernetes::OPERATION_RESUME,
        }
    }

    /// Whether the verb can act on an operation in `state`. Only a `blocked` or `queued`
    /// operation can be resumed, so `hipc` sends no `{}` to one in any other state, an
    /// unknown one, or none. A cancel stops one that is not over: a `done`, `failed`, or
    /// `cancelled` operation is refused, and an unknown state, or none, is left to the
    /// server, which refuses what it cannot cancel.
    fn allows(self, state: Option<&OperationState>) -> bool {
        match self {
            Self::Resume => matches!(
                state,
                Some(OperationState::Blocked | OperationState::Queued)
            ),
            Self::Cancel => !state.is_some_and(OperationState::is_over),
        }
    }

    /// Why the verb cannot act on an operation in `state`, for the refusal: the message
    /// names the state the server reached, so the caller knows it is not the invocation.
    fn refused_because(self, state: Option<&OperationState>) -> String {
        let is = match state {
            Some(state) => format!("its state is {}", phrase::quote(state.as_str())),
            // Only a resume gets here without a state: a cancel of such an operation goes
            // to the server, which can tell what `hipc` cannot.
            None => "the API gives it no state".to_owned(),
        };
        match self {
            Self::Resume => {
                format!("{is}, and only a `blocked` or `queued` operation can be resumed")
            }
            Self::Cancel => format!("{is}, so there is nothing left to cancel"),
        }
    }
}

/// `POST .../operations/<op_id>/<verb>/` with `{}`, answered with the operation, after a read
/// of the cluster's operations that names the operation's state and kind and what going on
/// with it does (module docs). There is no route that reads one operation, so the list is
/// read. What the read finds the verb cannot act on is refused as `conflict` before the
/// question, with nothing sent after the read.
fn operation(ctx: &Context<'_>, target: &OperationTarget, verb: Verb) -> Result<Printed> {
    let (id, op_id) = (target.cluster.cluster_id, target.operation_id);
    let operations = format!("hipc compute kubernetes operations list {id}");
    let post = |action: String| {
        Request::to(verb.route(), [&id, &op_id])
            .json(&EmptyBody {})
            .about(action)
            .listed_by(operations.clone())
    };
    let read = list_request(id, &kubernetes::OPERATIONS, "operations");
    let client = ctx.load_authed_client()?;
    let draft = post(format!(
        "{} the operation `{op_id}` of the Kubernetes cluster `{id}`",
        verb.segment()
    ));
    let decide = || {
        let op = actionable(target, verb, &read_list(ctx, &client, &read)?)?;
        Ok(Decided::asked(
            post(operation_action(verb, &op, id, op_id)),
            None,
        ))
    };
    let plan = WritePlan::new(&[&read], &draft);
    read_then_write(ctx, &client, &plan, decide, |sent| sent?.document())
}

/// The operation `target` names in `listed`, the cluster's operations, when `verb` may act
/// on it; refused otherwise, so nothing is sent after the read.
fn actionable(target: &OperationTarget, verb: Verb, listed: &[Value]) -> Result<Operation> {
    let (id, op_id) = (target.cluster.cluster_id, target.operation_id);
    let operations = format!("hipc compute kubernetes operations list {id}");
    let found = listed
        .iter()
        .map(view_of::<Operation>)
        .find(|op| op.id == Some(op_id));
    let Some(op) = found else {
        return Err(Error::new(
            Failure::NotFound,
            format!("the Kubernetes cluster `{id}` has no operation `{op_id}`"),
        )
        .with_hint(format!("run `{operations}` to see what exists")));
    };
    // An empty state would print as an empty literal, so it reads as no state at all.
    let state = op.state.as_ref().filter(|state| !state.as_str().is_empty());
    // No hint: nothing the caller runs changes a state the server reached (tenet 15).
    if !verb.allows(state) {
        return Err(Error::new(
            Failure::Conflict,
            format!(
                "cannot {} the operation `{op_id}` of the Kubernetes cluster `{id}`: {}",
                verb.segment(),
                verb.refused_because(state)
            ),
        ));
    }
    // Only a blocked operation can be blocked on data: a queued one has not reached the step
    // that finds it, whatever its `step` or `reason` still hold.
    //
    // This refusal is a courtesy, not the guard: the server is the real guard on a resume
    // without the signed acceptance, and `hipc api POST .../resume/ --yes` sends one unchecked.
    // Mirroring this read in `hipc api` would mean a second copy of this parser over a raw
    // route, so it is left to the server. Whether the server enforces the signed acceptance
    // is unverified; if it does not, this read belongs in `hipc api` too.
    if verb == Verb::Resume && state == Some(&OperationState::Blocked) && blocked_on_local_data(&op)
    {
        return Err(Error::new(
            Failure::Conflict,
            format!(
                "the operation `{op_id}` of the Kubernetes cluster `{id}` is blocked because \
                 data on the node would be lost, and only the console can sign the acceptance \
                 of that loss"
            ),
        )
        .with_hint(format!(
            "accept the data loss in the console, or run `hipc compute kubernetes operations \
             cancel {id} {op_id}`"
        )));
    }
    Ok(op)
}

/// Whether `op` is blocked on data a node would lose, which a resume can only get past with a
/// signed `accept_data_loss`, which only the console can make. Such a block is told from the step
/// and the reason: the step `check_local_data`, or a drain whose reason says data "would be lost".
/// The reason is matched at any step, so a block reported another way is refused here rather than
/// resumed with `{}`.
fn blocked_on_local_data(op: &Operation) -> bool {
    op.step.as_deref() == Some("check_local_data")
        || op
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("would be lost"))
}

/// The question of a cancel or resume of `op`: its state, kind, and node, and what going on
/// with it does, in the action itself, since an agent's refusal keeps only the action. The
/// state is named only when it is in flight ([`OperationState::is_active`]): a cancel of an
/// unknown state goes to the server, and the question does not pass on a word `hipc` cannot
/// explain.
fn operation_action(verb: Verb, op: &Operation, id: u64, op_id: u64) -> String {
    let kind = op.kind.as_ref().filter(|kind| !kind.as_str().is_empty());
    let node = op
        .node
        .value()
        .filter(|node| !node.is_empty())
        .map(phrase::quote);
    let replaces = op.replaces().map(phrase::quote);
    let effect = match (verb, kind, replaces, node) {
        (Verb::Resume, Some(OperationKind::AddNode), Some(old), _) => format!(
            ", which launches a new VM billed to the balance to replace the node {old}, then \
             decommissions {old}"
        ),
        (Verb::Cancel, Some(OperationKind::AddNode), Some(old), _) => {
            format!(", so the node {old} is not replaced")
        }
        (Verb::Resume, Some(OperationKind::AddNode), None, _) => {
            ", which launches a new node's VM, billed to the balance".to_owned()
        }
        (Verb::Cancel, Some(OperationKind::AddNode), None, _) => {
            ", so its node is not added".to_owned()
        }
        (Verb::Resume, Some(OperationKind::RemoveNode), _, Some(node)) => {
            format!(", which drains the node {node} and decommissions its VM")
        }
        (Verb::Cancel, Some(OperationKind::RemoveNode), _, Some(node)) => {
            format!(", so the node {node} stays")
        }
        _ => String::new(),
    };
    let kind = kind.map_or("unknown", OperationKind::as_str);
    let state = op
        .state
        .as_ref()
        .filter(|state| state.is_active())
        .map_or(String::new(), |state| {
            format!("{} ", phrase::quote(state.as_str()))
        });
    format!(
        "{} the {state}{} operation `{op_id}` of the Kubernetes cluster `{id}`{effect}",
        verb.segment(),
        phrase::quote(kind)
    )
}

/// `POST .../node-pools/` with exactly `name`, `flavor`, `labels`, and `taints`, the fields
/// the console's form sends; the API documents no body. Both collections go out empty when no
/// flag fills them.
fn pool_create(ctx: &Context<'_>, args: &NodePoolCreate) -> Result<Printed> {
    let mut labels = Map::new();
    for label in &args.labels {
        // A JSON object holds a key once, so a repeat would drop a value without a word.
        if labels
            .insert(label.key.clone(), label.value.as_str().into())
            .is_some()
        {
            return Err(Error::usage(
                format!("`--label` names the key `{}` more than once", label.key),
                "pass each label key once",
            ));
        }
    }
    let id = args.cluster.cluster_id;
    let request = Request::to(&kubernetes::NODE_POOL_CREATE, [&id])
        .json(&kubernetes::NodePoolCreateBody {
            name: &args.name,
            flavor: &args.flavor,
            labels,
            taints: &args.taints,
        })
        .about(format!(
            "create the empty worker pool `{}` of flavor {} in the Kubernetes cluster `{id}`, \
             which costs nothing until it is scaled",
            args.name,
            phrase::quote(&args.flavor)
        ))
        .listed_by(CLUSTERS_LIST_COMMAND);
    // Not confirmed: an empty pool spends nothing until a scale, which is confirmed (tenet 5).
    print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_action_names_the_state_the_kind_and_what_going_on_does() {
        let add = json!({
            "kind": "add_node", "state": "queued", "node": null, "params": { "pool": "w" }
        });
        let replace = json!({
            "kind": "add_node", "state": "blocked", "params": { "replaces": "work-1" }
        });
        let remove = json!({
            "kind": "remove_node", "state": "blocked", "node": "work-2", "params": {}
        });
        let running = json!({ "kind": "remove_node", "state": "running", "node": "work-2" });
        let other = json!({ "kind": "revoke_join_token", "state": "queued", "params": {} });
        let tail = "operation `12` of the Kubernetes cluster `7`";
        let cases = [
            (
                Verb::Resume,
                &add,
                format!(
                    "resume the `queued` `add_node` {tail}, which launches a new node's VM, \
                     billed to the balance"
                ),
            ),
            (
                Verb::Cancel,
                &add,
                format!("cancel the `queued` `add_node` {tail}, so its node is not added"),
            ),
            (
                Verb::Resume,
                &replace,
                format!(
                    "resume the `blocked` `add_node` {tail}, which launches a new VM billed to the \
                     balance to replace the node `work-1`, then decommissions `work-1`"
                ),
            ),
            (
                Verb::Cancel,
                &replace,
                format!(
                    "cancel the `blocked` `add_node` {tail}, so the node `work-1` is not replaced"
                ),
            ),
            (
                Verb::Resume,
                &remove,
                format!(
                    "resume the `blocked` `remove_node` {tail}, which drains the node `work-2` and \
                     decommissions its VM"
                ),
            ),
            (
                Verb::Cancel,
                &remove,
                format!("cancel the `blocked` `remove_node` {tail}, so the node `work-2` stays"),
            ),
            (
                Verb::Cancel,
                &running,
                format!("cancel the `running` `remove_node` {tail}, so the node `work-2` stays"),
            ),
            (
                Verb::Resume,
                &other,
                format!("resume the `queued` `revoke_join_token` {tail}"),
            ),
            // An unknown state is not put in the question, nor is none.
            (
                Verb::Cancel,
                &json!({ "kind": "a`b", "state": "odd`state" }),
                format!("cancel the `a\\u{{60}}b` {tail}"),
            ),
            (
                Verb::Cancel,
                &json!({ "kind": "add_node", "state": "" }),
                format!("cancel the `add_node` {tail}, so its node is not added"),
            ),
            (
                Verb::Cancel,
                &json!({}),
                format!("cancel the `unknown` {tail}"),
            ),
        ];
        for (verb, op, expected) in cases {
            assert_eq!(
                operation_action(verb, &view_of(op), 7, 12),
                expected,
                "{op}"
            );
        }
    }

    #[test]
    fn a_resume_needs_blocked_or_queued_and_a_cancel_an_operation_not_over() {
        let states = [
            "queued",
            "running",
            "blocked",
            "done",
            "failed",
            "cancelled",
            "paused",
            "",
        ];
        let state = |text: &str| OperationState::from(text.to_owned());
        let resumes: Vec<&str> = states
            .iter()
            .copied()
            .filter(|text| Verb::Resume.allows(Some(&state(text))))
            .collect();
        assert_eq!(resumes, ["queued", "blocked"]);
        let cancels: Vec<&str> = states
            .iter()
            .copied()
            .filter(|text| Verb::Cancel.allows(Some(&state(text))))
            .collect();
        assert_eq!(cancels, ["queued", "running", "blocked", "paused", ""]);
        assert!(!Verb::Resume.allows(None));
        assert!(Verb::Cancel.allows(None));
        assert_eq!(
            Verb::Resume.refused_because(Some(&state("run`ning"))),
            "its state is `run\\u{60}ning`, and only a `blocked` or `queued` operation can be \
             resumed"
        );
        assert_eq!(
            Verb::Resume.refused_because(None),
            "the API gives it no state, and only a `blocked` or `queued` operation can be \
             resumed"
        );
        // The caller reads an empty state as none; the wording never shows an empty literal.
        assert_eq!(
            Verb::Resume.refused_because(Some(&state(""))),
            "its state is ``, and only a `blocked` or `queued` operation can be resumed"
        );
        assert_eq!(
            Verb::Cancel.refused_because(Some(&state("done"))),
            "its state is `done`, so there is nothing left to cancel"
        );
    }

    #[test]
    fn a_local_data_block_is_told_from_the_step_or_the_reason() {
        for op in [
            json!({ "step": "check_local_data", "reason": null }),
            json!({ "step": "cordon_drain", "reason": "2 volumes would be lost" }),
        ] {
            assert!(blocked_on_local_data(&view_of(&op)), "{op}");
        }
        for op in [
            json!({ "step": "cordon_drain", "reason": "a disruption budget blocks the drain" }),
            json!({ "step": "cordon_drain" }),
            json!({}),
        ] {
            assert!(!blocked_on_local_data(&view_of(&op)), "{op}");
        }
    }
}
