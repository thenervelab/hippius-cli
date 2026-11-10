//! The account's private network: its peers, groups, policies, and VPCs.

use crate::routes::Route;

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
