//! Kubernetes clusters, their node pools, nodes, operations, and backups.

use crate::routes::{Guard, Route};

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
