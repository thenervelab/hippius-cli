//! The account's private network: its peers, groups, policies, VPCs, isolation, and topology.
//!
//! Swagger documents most bodies here; where it does not, the console's network types are
//! the source, and the body's doc says so.

use serde::{Deserialize, Serialize};

use crate::routes::{Guard, Route, RouteFlag};
use crate::view::{each, field, nested, text_or_number};

routes! {
    /// The private network; the first read for an account creates it.
    ME: 0 = Route::get("/api/network/me/").scoped().provisions();

    /// Creates the private network.
    PROVISION: 0 = Route::post("/api/network/provision/").scoped();

    /// The network groups.
    GROUPS: 0 = Route::get("/api/network/groups/").scoped();

    /// Creates a group.
    GROUP_CREATE: 0 = Route::post("/api/network/groups/").scoped();

    /// Deletes a group.
    GROUP_DELETE: 1 = Route::delete("/api/network/groups/{group_id}/").scoped();

    /// Adds one peer to a group, or removes one: the body says which. An explicit remove is
    /// not a list replace, so `hipc api` needs nothing beyond `--yes` for it.
    GROUP_MEMBERS_UPDATE: 1 = Route::post("/api/network/groups/{group_id}/members/").scoped();

    /// The network policies.
    POLICIES: 0 = Route::get("/api/network/policies/").scoped();

    /// Creates a policy.
    POLICY_CREATE: 0 = Route::post("/api/network/policies/").scoped();

    /// Changes a policy.
    POLICY_UPDATE: 1 = Route::patch("/api/network/policies/{policy_id}/").scoped();

    /// Deletes a policy.
    POLICY_DELETE: 1 = Route::delete("/api/network/policies/{policy_id}/").scoped();

    /// The network peers.
    PEERS: 0 = Route::get("/api/network/peers/").scoped();

    /// One peer.
    PEER: 1 = Route::get("/api/network/peers/{peer_id}/").scoped();

    /// Changes a peer.
    PEER_UPDATE: 1 = Route::patch("/api/network/peers/{peer_id}/").scoped();

    /// Removes a peer.
    PEER_DELETE: 1 = Route::delete("/api/network/peers/{peer_id}/").scoped();

    /// The VPC networks.
    VPCS: 0 = Route::get("/api/network/networks/").scoped();

    /// Creates a VPC network.
    VPC_CREATE: 0 = Route::post("/api/network/networks/").scoped();

    /// Deletes a VPC network.
    VPC_DELETE: 1 = Route::delete("/api/network/networks/{vpc_id}/").scoped();

    /// A VPC's resources.
    VPC_RESOURCES: 1 = Route::get("/api/network/networks/{vpc_id}/resources/").scoped();

    /// Adds a resource to a VPC.
    VPC_RESOURCE_CREATE: 1 = Route::post("/api/network/networks/{vpc_id}/resources/").scoped();

    /// Changes a resource of a VPC.
    VPC_RESOURCE_UPDATE: 2 =
        Route::patch("/api/network/networks/{vpc_id}/resources/{resource_id}/").scoped();

    /// Removes a resource from a VPC.
    VPC_RESOURCE_DELETE: 2 =
        Route::delete("/api/network/networks/{vpc_id}/resources/{resource_id}/").scoped();

    /// A VPC's routers.
    VPC_ROUTERS: 1 = Route::get("/api/network/networks/{vpc_id}/routers/").scoped();

    /// Makes a peer a router of a VPC.
    VPC_ROUTER_CREATE: 1 = Route::post("/api/network/networks/{vpc_id}/routers/").scoped();

    /// Removes a router from a VPC.
    VPC_ROUTER_DELETE: 2 =
        Route::delete("/api/network/networks/{vpc_id}/routers/{router_id}/").scoped();

    /// Which peers of the network reach each other without a policy that says so.
    ISOLATION: 0 = Route::get("/api/network/isolation/").scoped();

    /// Switches which peers of the network reach each other; either mode changes that.
    ISOLATION_UPDATE: 0 = Route::post("/api/network/isolation/")
        .scoped()
        .guard(Guard::Needs(&[RouteFlag::ChangeIsolation]));

    /// The whole network as a graph of nodes and edges. The console's network types say
    /// that its first read for an account creates the network, as [`ME`]'s does.
    TOPOLOGY: 0 = Route::get("/api/network/topology/").scoped().provisions();
}

/// The body of [`PEER_UPDATE`]. Swagger documents none; `name` is `NetBird`'s field for a peer's
/// name, unverified against the live API.
#[derive(Debug, Serialize)]
pub struct PeerUpdateBody<'a> {
    /// The peer's new name.
    pub name: &'a str,
}

/// A peer of [`PEER_UPDATE`]'s answer, as far as a rename decides on it.
#[derive(Debug, Default, Deserialize)]
pub struct Peer {
    /// The peer's id: swagger types it as a string, and the list may carry a number.
    #[serde(default, deserialize_with = "text_or_number")]
    pub id: Option<String>,
}

/// The body of [`GROUP_CREATE`]. Swagger: `label` is lowercase letters, digits, and `-`, 1 to
/// 50 characters.
#[derive(Debug, Serialize)]
pub struct GroupCreateBody<'a> {
    /// The group's name.
    pub label: &'a str,
}

/// The body of [`GROUP_MEMBERS_UPDATE`]. Swagger documents none; the console's network types
/// give this one, and say that membership moves one peer at a time, with no bulk form.
#[derive(Debug, Serialize)]
pub struct GroupMemberBody {
    /// `add` or `remove`.
    pub action: MemberAction,
    /// The peer, by its id in the peer list.
    pub peer_id: u64,
}

/// What a [`GroupMemberBody`] does with its peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MemberAction {
    /// Puts the peer in the group.
    Add,
    /// Takes the peer out of the group.
    Remove,
}

/// The body of [`POLICY_CREATE`] and [`POLICY_UPDATE`], each field only when given: swagger
/// documents one body for both, and the console sends a partial one to the update.
#[derive(Debug, Default, Serialize)]
pub struct PolicyBody<'a> {
    /// The policy's name; the server's default is `<source>-to-<dest>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<&'a str>,
    /// The group the traffic comes from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_group: Option<u64>,
    /// The group the traffic goes to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dest_group: Option<u64>,
    /// `all`, `tcp`, `udp`, or `icmp`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<&'a str>,
    /// Ports and `low-high` ranges, for `tcp` and `udp`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<&'a [String]>,
    /// Whether the traffic may also start from the destination group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bidirectional: Option<bool>,
    /// `accept` or `drop`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<&'a str>,
    /// Whether the policy applies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// The body of [`VPC_RESOURCE_CREATE`] and [`VPC_RESOURCE_UPDATE`], each field only when
/// given. Swagger documents the create's; the update's is undocumented, and is taken to be
/// the same fields, as Django REST framework's partial update of one serializer takes them.
#[derive(Debug, Default, Serialize)]
pub struct ResourceBody<'a> {
    /// The resource's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<&'a str>,
    /// A CIDR, an IP address, or a domain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<&'a str>,
    /// `host`, `subnet`, or `domain`; the server infers it from `address` when left out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<&'a str>,
    /// The ids of the account's groups the resource is in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groups: Option<&'a [u64]>,
}

/// The body of [`VPC_ROUTER_CREATE`].
#[derive(Debug, Serialize)]
pub struct RouterCreateBody {
    /// The peer that routes the VPC's traffic, by its id in the peer list.
    pub peer: u64,
    /// Whether the router masks the source address of the traffic it routes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub masquerade: Option<bool>,
    /// The router's priority; the lower number wins.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metric: Option<u64>,
}

/// The body of [`ISOLATION_UPDATE`].
#[derive(Debug, Serialize)]
pub struct IsolationBody<'a> {
    /// `allow_intra` or `deny_intra`.
    pub isolation_mode: &'a str,
}

/// One group of [`GROUPS`]' answer, as far as a members update decides on it.
#[derive(Debug, Default, Deserialize)]
pub struct Group {
    /// The group's id, a number in the console's types; read as text, so either form
    /// compares.
    #[serde(default, deserialize_with = "text_or_number")]
    pub id: Option<String>,
    /// The group's name.
    #[serde(default, deserialize_with = "field")]
    pub label: Option<String>,
}

/// [`TOPOLOGY`]'s answer, as far as a members update decides on it: its nodes.
#[derive(Debug, Default, Deserialize)]
pub struct Topology {
    /// The nodes; `None` when the answer has no list of them.
    #[serde(default, deserialize_with = "each")]
    pub nodes: Option<Vec<TopologyNode>>,
}

/// One node of a [`Topology`]: a group, a peer, a resource, or a router.
#[derive(Debug, Default, Deserialize)]
pub struct TopologyNode {
    /// `group`, `peer`, `resource`, or `router`.
    #[serde(default, rename = "type", deserialize_with = "field")]
    pub kind: Option<String>,
    /// What the node is; for a peer, its id and its groups.
    #[serde(default, deserialize_with = "nested")]
    pub meta: Option<PeerMeta>,
}

/// The `meta` of a peer node of a [`Topology`]: the console's network types document these
/// two fields.
#[derive(Debug, Default, Deserialize)]
pub struct PeerMeta {
    /// The peer's id in the peer list.
    #[serde(default, deserialize_with = "field")]
    pub peer_id: Option<u64>,
    /// The ids of the groups the peer is in.
    #[serde(default, deserialize_with = "field")]
    pub groups: Option<Vec<u64>>,
}
