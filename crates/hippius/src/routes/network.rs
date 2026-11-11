//! The account's private network: its peers, groups, policies, and VPCs.

use serde::{Deserialize, Serialize};

use crate::routes::Route;
use crate::view::text_or_number;

routes! {
    /// The private network; the first read for an account creates it.
    ME: 0 = Route::get("/api/network/me/").scoped().provisions();

    /// Creates the private network.
    PROVISION: 0 = Route::post("/api/network/provision/").scoped();

    /// The network groups.
    GROUPS: 0 = Route::get("/api/network/groups/").scoped();

    /// The network policies.
    POLICIES: 0 = Route::get("/api/network/policies/").scoped();

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

    /// A VPC's resources.
    VPC_RESOURCES: 1 = Route::get("/api/network/networks/{vpc_id}/resources/").scoped();

    /// A VPC's routers.
    VPC_ROUTERS: 1 = Route::get("/api/network/networks/{vpc_id}/routers/").scoped();
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
