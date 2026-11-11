//! The operator API: the nodes an operator runs for the network. No command sends it; `hipc
//! api` reaches it with the caller's own account only.

use crate::routes::{Guard, Route, RouteFlag};

routes! {
    /// Replaces a node's whole storage list, so an entry the body leaves out is removed.
    NODE_STORAGE_UPDATE: 1 = Route::put("/api/v1/operator/nodes/{node_id}/storage/")
        .own_account()
        .guard(Guard::Needs(&[RouteFlag::RemoveOthers]));
}
