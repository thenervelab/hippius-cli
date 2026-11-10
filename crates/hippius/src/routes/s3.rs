//! S3 storage: plans, the subscription, buckets and their objects and policies, and access
//! keys.

use crate::routes::{Guard, Route, RouteFlag};

routes! {
    /// The S3 plans.
    PLANS: 0 = Route::get("/api/s3/plans/").scoped().project();

    /// The S3 subscription.
    SUBSCRIPTION: 0 = Route::get("/api/s3/subscription/").scoped().project();

    /// Subscribes to an S3 plan.
    SUBSCRIBE: 0 = Route::post("/api/s3/subscription/").scoped().undone_on_503();

    /// Changes the S3 subscription.
    SUBSCRIPTION_CHANGE: 0 = Route::patch("/api/s3/subscription/").scoped();

    /// Cancels the S3 subscription.
    SUBSCRIPTION_CANCEL: 0 = Route::delete("/api/s3/subscription/").scoped();

    /// A card checkout of an S3 plan.
    CHECKOUT: 1 = Route::get("/api/s3/subscription/checkout/{intent_id}/").scoped();

    /// The S3 subscription's history.
    HISTORY: 0 = Route::get("/api/s3/subscription/history/").scoped().billing().paged();

    /// The buckets.
    BUCKETS: 0 = Route::get("/api/objectstore/buckets/").scoped().project();

    /// Creates a bucket.
    BUCKET_CREATE: 0 = Route::post("/api/objectstore/buckets/").scoped().project();

    /// Deletes a bucket.
    BUCKET_DELETE: 1 = Route::delete("/api/objectstore/buckets/{bucket_name}/").scoped().project();

    /// A bucket's objects.
    OBJECTS: 1 = Route::get("/api/objectstore/buckets/{bucket_name}/objects/")
        .scoped()
        .project()
        .paged();

    /// A signed URL for one object.
    PRESIGNED_URL: 1 = Route::get("/api/objectstore/buckets/{bucket_name}/presigned-url/")
        .scoped()
        .project()
        .reveals();

    /// Every bucket's policy.
    ACLS: 0 = Route::get("/api/objectstore/acl/").scoped().project();

    /// A bucket's policy.
    BUCKET_ACL: 1 = Route::get("/api/objectstore/buckets/{bucket_name}/acl/").scoped().project();

    /// Replaces a bucket's policy.
    BUCKET_ACL_SET: 1 = Route::put("/api/objectstore/buckets/{bucket_name}/acl/set/")
        .scoped()
        .project()
        .guard(Guard::Needs(&[RouteFlag::Public, RouteFlag::RemoveOthers]));

    /// The master access keys.
    MASTER_TOKENS: 0 = Route::get("/api/objectstore/master-tokens/").scoped();

    /// Creates a master access key.
    MASTER_TOKEN_CREATE: 0 = Route::post("/api/objectstore/master-tokens/").scoped().reveals();

    /// Rotates a master access key.
    MASTER_TOKEN_ROTATE: 1 = Route::post("/api/objectstore/master-tokens/{token_id}/rotate/")
        .scoped()
        .reveals();

    /// Revokes a master access key.
    MASTER_TOKEN_REVOKE: 1 = Route::post("/api/objectstore/master-tokens/{token_id}/revoke/")
        .scoped();

    /// The sub access keys.
    SUB_TOKENS: 0 = Route::get("/api/objectstore/sub-tokens/").scoped().project();

    /// Creates a sub access key.
    SUB_TOKEN_CREATE: 0 = Route::post("/api/objectstore/sub-tokens/").scoped().project().reveals();

    /// Rotates a sub access key.
    SUB_TOKEN_ROTATE: 1 = Route::post("/api/objectstore/sub-tokens/{token_id}/rotate/")
        .scoped()
        .project()
        .reveals();

    /// Revokes a sub access key.
    SUB_TOKEN_REVOKE: 1 = Route::post("/api/objectstore/sub-tokens/{token_id}/revoke/")
        .scoped()
        .project();
}
