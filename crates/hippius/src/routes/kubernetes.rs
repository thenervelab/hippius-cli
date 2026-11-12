//! Kubernetes clusters, their node pools, nodes, operations, and backups.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::routes::{Guard, Route};
use crate::view::{Slot, each, field, nested};

routes! {
    /// The clusters.
    CLUSTERS: 0 = Route::get("/api/kubernetes/clusters/").scoped().project();

    /// One cluster.
    CLUSTER: 1 = Route::get("/api/kubernetes/clusters/{cluster_id}/").scoped().project();

    /// Deletes a cluster.
    CLUSTER_DELETE: 1 = Route::delete("/api/kubernetes/clusters/{cluster_id}/").scoped().project();

    /// A cluster's events.
    CLUSTER_EVENTS: 1 = Route::get("/api/kubernetes/clusters/{cluster_id}/events/")
        .scoped()
        .project();

    /// A cluster's sealed kubeconfig.
    CLUSTER_SECRETS: 1 = Route::get("/api/kubernetes/clusters/{cluster_id}/secrets/")
        .scoped()
        .project()
        .guard(Guard::Refused {
            why: "its answer holds the cluster's sealed kubeconfig",
            hint: "run `hipc compute kubernetes clusters kubeconfig`, which opens the kubeconfig \
                   with the cluster key file and writes it to a new file",
        });

    /// The refused cluster requests.
    CLUSTER_REFUSALS: 0 = Route::get("/api/kubernetes/clusters/refusals/").scoped().project();

    /// A cluster's backups.
    BACKUPS: 1 = Route::get("/api/kubernetes/clusters/{cluster_id}/backups/")
        .scoped()
        .project()
        .reveals();

    /// A cluster's node pools.
    NODE_POOLS: 1 = Route::get("/api/kubernetes/clusters/{cluster_id}/node-pools/")
        .scoped()
        .project();

    /// Adds a node pool.
    NODE_POOL_CREATE: 1 = Route::post("/api/kubernetes/clusters/{cluster_id}/node-pools/")
        .scoped()
        .project();

    /// Scales a node pool.
    NODE_POOL_SCALE: 2 = Route::post("/api/kubernetes/clusters/{cluster_id}/node-pools/{pool_id}/scale/")
        .scoped()
        .project();

    /// Removes a node.
    NODE_REMOVE: 2 = Route::post("/api/kubernetes/clusters/{cluster_id}/nodes/{node_name}/remove/")
        .scoped()
        .project();

    /// Replaces a node.
    NODE_REPLACE: 2 = Route::post("/api/kubernetes/clusters/{cluster_id}/nodes/{node_name}/replace/")
        .scoped()
        .project();

    /// A cluster's operations.
    OPERATIONS: 1 = Route::get("/api/kubernetes/clusters/{cluster_id}/operations/")
        .scoped()
        .project();

    /// Cancels an operation.
    OPERATION_CANCEL: 2 = Route::post("/api/kubernetes/clusters/{cluster_id}/operations/{operation_id}/cancel/")
        .scoped()
        .project();

    /// Resumes a held operation.
    OPERATION_RESUME: 2 = Route::post("/api/kubernetes/clusters/{cluster_id}/operations/{operation_id}/resume/")
        .scoped()
        .project();
}

/// The body of [`NODE_POOL_CREATE`]: the fields the console's form sends; the API documents
/// no body. Both collections go out empty when nothing fills them.
#[derive(Debug, Serialize)]
pub struct NodePoolCreateBody<'a> {
    /// The pool's name.
    pub name: &'a str,
    /// The flavor of its nodes.
    pub flavor: &'a str,
    /// The Kubernetes labels of its nodes, in the order given.
    pub labels: Map<String, Value>,
    /// The Kubernetes taints of its nodes, `key[=value]:effect`.
    pub taints: &'a [String],
}

/// The body of [`NODE_POOL_SCALE`]: the size the pool goes to, and one signed `add_node` per
/// node added. `T` is the signed operations, or what a dry run shows in their place.
#[derive(Debug, Serialize)]
pub struct NodePoolScaleBody<T> {
    /// The number of nodes the pool ends with.
    pub count: u64,
    /// The signed `add_node`s; left out when the pool shrinks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operations: Option<T>,
}

/// The body of [`NODE_REPLACE`]: one `add_node` signed over the node's pool's
/// `signing_params` with `replaces` added. `T` is the signed operation, or what a dry run
/// shows in its place.
#[derive(Debug, Serialize)]
pub struct NodeReplaceBody<T> {
    /// The signed `add_node`.
    pub operation: T,
}

/// A cluster of [`CLUSTER`], as far as its key file is checked against it.
#[derive(Debug, Default, Deserialize)]
#[expect(
    clippy::struct_field_names,
    reason = "each field is named as the API names it"
)]
pub struct ClusterKeys {
    /// The client signing key the cluster was created with, base64.
    #[serde(default, deserialize_with = "field")]
    pub client_sign_pubkey: Option<String>,
    /// The client box key the cluster was created with, base64.
    #[serde(default, deserialize_with = "field")]
    pub client_box_pubkey: Option<String>,
    /// The key the cluster's guest signs its drops with, base64; none until it publishes one.
    #[serde(default, deserialize_with = "field")]
    pub guest_sign_pubkey: Option<String>,
}

/// The sealed secrets of [`CLUSTER_SECRETS`].
#[derive(Debug, Default, Deserialize)]
pub struct Sealed {
    /// The key the drops are signed with, base64.
    #[serde(default, deserialize_with = "field")]
    pub guest_sign_pubkey: Option<String>,
    /// The drops, newest or not.
    #[serde(default, deserialize_with = "each")]
    pub drops: Option<Vec<SealedDrop>>,
}

/// One drop of [`Sealed`]: a secret the guest sealed to the cluster's box key and signed. Its
/// `Debug` shows its kind and time, not its ciphertext.
#[derive(Default, Deserialize)]
pub struct SealedDrop {
    /// What it holds, such as `kubeconfig`.
    #[serde(default, deserialize_with = "field")]
    pub kind: Option<String>,
    /// The operation it was made for; a drop made at launch has none, or `null`.
    #[serde(default)]
    pub operation: Slot<Value>,
    /// The sealed box, base64.
    #[serde(default, deserialize_with = "field")]
    pub ciphertext: Option<String>,
    /// The guest's Ed25519 signature of the drop's message, base64.
    #[serde(default, deserialize_with = "field")]
    pub signature: Option<String>,
    /// When it was made, RFC 3339.
    #[serde(default, deserialize_with = "field")]
    pub created_at: Option<String>,
}

impl std::fmt::Debug for SealedDrop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealedDrop")
            .field("kind", &self.kind)
            .field("created_at", &self.created_at)
            .finish_non_exhaustive()
    }
}

/// A node pool of [`NODE_POOLS`], as far as a scale and a replacement decide on it. Its
/// `signing_params` are read from the pool's `Value`, as the API gave them, since that map is
/// what is signed.
#[derive(Debug, Default, Deserialize)]
pub struct Pool {
    /// The pool's id.
    #[serde(default, deserialize_with = "field")]
    pub id: Option<u64>,
    /// The pool's name, which a signed `add_node` names.
    #[serde(default, deserialize_with = "field")]
    pub name: Option<String>,
    /// The pool's nodes, a removed one among them until it is gone.
    #[serde(default, deserialize_with = "each")]
    pub nodes: Option<Vec<Node>>,
}

/// A node of a [`Pool`].
#[derive(Debug, Default, Deserialize)]
pub struct Node {
    /// The node's name.
    #[serde(default, deserialize_with = "field")]
    pub name: Option<String>,
    /// Where it stands; `removed` once its removal is done.
    #[serde(default, deserialize_with = "field")]
    pub state: Option<String>,
}

impl Node {
    /// Whether its removal is done: it stays in the list, with no VM left.
    pub fn is_removed(&self) -> bool {
        self.state.as_deref() == Some("removed")
    }
}

/// An operation of [`OPERATIONS`], as far as a cancel, a resume, a scale, and a replacement
/// decide on it.
#[derive(Debug, Default, Deserialize)]
pub struct Operation {
    /// The operation's id.
    #[serde(default, deserialize_with = "field")]
    pub id: Option<u64>,
    /// What it does.
    #[serde(default, deserialize_with = "field")]
    pub kind: Option<OperationKind>,
    /// Where it stands.
    #[serde(default, deserialize_with = "field")]
    pub state: Option<OperationState>,
    /// The step it is at, such as `check_local_data`.
    #[serde(default, deserialize_with = "field")]
    pub step: Option<String>,
    /// Why it is blocked, in the server's words.
    #[serde(default, deserialize_with = "field")]
    pub reason: Option<String>,
    /// The node it acts on: an `add_node` has none until its node exists, so a decision
    /// tells a missing or `null` node from one there.
    #[serde(default)]
    pub node: Slot<String>,
    /// What it was asked to do.
    #[serde(default, deserialize_with = "nested")]
    pub params: Option<OperationParams>,
}

impl Operation {
    /// Whether it is in flight ([`OperationState::is_active`]).
    pub fn is_active(&self) -> bool {
        self.state.as_ref().is_some_and(OperationState::is_active)
    }

    /// The node an `add_node` replaces, when it replaces one: a name, not empty.
    pub fn replaces(&self) -> Option<&str> {
        self.params
            .as_ref()
            .and_then(|params| params.replaces.as_deref())
            .filter(|node| !node.is_empty())
    }
}

/// The `params` of an [`Operation`].
#[derive(Debug, Default, Deserialize)]
pub struct OperationParams {
    /// The node an `add_node` replaces.
    #[serde(default, deserialize_with = "field")]
    pub replaces: Option<String>,
    /// The pool an `add_node` adds a node to, by name.
    #[serde(default, deserialize_with = "field")]
    pub pool: Option<String>,
}

vocabulary! {
    /// The `state` of an [`Operation`], as the API emits it: lowercase and untrimmed, so
    /// `Queued` or `blocked ` is [`OperationState::Other`], not a misspelling to fix.
    OperationState {
        /// Waiting to start.
        Queued = "queued",
        /// Under way.
        Running = "running",
        /// Held until it is resumed or cancelled.
        Blocked = "blocked",
        /// Finished.
        Done = "done",
        /// Ended in a failure.
        Failed = "failed",
        /// Cancelled.
        Cancelled = "cancelled",
    }
}

impl OperationState {
    /// Whether an operation in this state is in flight: `queued`, `running`, or `blocked`.
    pub fn is_active(&self) -> bool {
        match self {
            Self::Queued | Self::Running | Self::Blocked => true,
            Self::Done | Self::Failed | Self::Cancelled | Self::Other(_) => false,
        }
    }

    /// Whether the operation is over: `done`, `failed`, or `cancelled`.
    pub fn is_over(&self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

vocabulary! {
    /// The `kind` of an [`Operation`].
    OperationKind {
        /// Adds a node, or replaces one (`params.replaces`).
        AddNode = "add_node",
        /// Drains a node and decommissions its VM.
        RemoveNode = "remove_node",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::view::view_of;

    /// Each state and kind reads as its variant and back; another is `Other`. In flight are
    /// `queued`, `running` and `blocked`; over are `done`, `failed` and `cancelled`; an unknown
    /// state is neither.
    #[test]
    fn an_operation_state_or_kind_the_server_adds_is_other() {
        let states = [
            ("queued", true, false),
            ("running", true, false),
            ("blocked", true, false),
            ("done", false, true),
            ("failed", false, true),
            ("cancelled", false, true),
            ("paused", false, false),
        ];
        for (state, active, over) in states {
            let read: Operation = view_of(&json!({ "state": state }));
            assert_eq!(read.is_active(), active, "{state}");
            let read = read.state.unwrap();
            assert_eq!((read.as_str(), read.is_over()), (state, over));
        }
        assert!(!view_of::<Operation>(&json!({})).is_active());
        for kind in ["add_node", "remove_node", "revoke_join_token"] {
            let read: Operation = view_of(&json!({ "kind": kind }));
            let read = read.kind.unwrap();
            assert_eq!(read.as_str(), kind);
            assert_eq!(
                matches!(read, OperationKind::Other(_)),
                kind == "revoke_join_token"
            );
        }
    }

    /// An operation replaces a node only when `params.replaces` names one; a node is removed
    /// only in the state `removed`.
    #[test]
    fn a_replacement_names_its_node_and_a_removed_node_says_so() {
        let replaces = |op: Value| view_of::<Operation>(&op).replaces().map(str::to_owned);
        assert_eq!(
            replaces(json!({"params": {"replaces": "w-1"}})).as_deref(),
            Some("w-1")
        );
        for op in [
            json!({"params": {"replaces": ""}}),
            json!({"params": {}}),
            json!({"params": ["w-1"]}),
            json!({}),
        ] {
            assert_eq!(replaces(op.clone()), None, "{op}");
        }
        let pool: Pool = view_of(&json!({"nodes": [{"state": "removed"}, {"state": "ready"}, 1]}));
        let removed: Vec<bool> = pool.nodes.unwrap().iter().map(Node::is_removed).collect();
        assert_eq!(removed, [true, false, false]);
    }

    /// A drop's `Debug` names its kind and time, never its ciphertext or signature.
    #[test]
    fn a_drop_debug_shows_no_ciphertext() {
        let sealed: Sealed = view_of(&json!({
            "drops": [{"kind": "kubeconfig", "ciphertext": "c1pher", "signature": "s1g"}],
        }));
        let shown = format!("{sealed:?}");
        assert!(shown.contains("kubeconfig"), "{shown}");
        assert!(
            !shown.contains("c1pher") && !shown.contains("s1g"),
            "{shown}"
        );
    }
}
