//! S3 storage: plans, the subscription, buckets and their objects and policies, and access
//! keys.

use serde::Serialize;

use crate::routes::{Guard, Paging, Route, RouteFlag};

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
    HISTORY: 0 = Route::get("/api/s3/subscription/history/").scoped().billing().paged(Paging::Numbered);

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
        .paged(Paging::Continuation);

    /// A signed URL for one object.
    PRESIGNED_URL: 1 = Route::get("/api/objectstore/buckets/{bucket_name}/presigned-url/")
        .scoped()
        .project()
        .reveals();

    /// Every bucket's policy.
    ACLS: 0 = Route::get("/api/objectstore/acl/").scoped().project();

    /// A bucket's policy.
    BUCKET_ACL: 1 = Route::get("/api/objectstore/buckets/{bucket_name}/acl/").scoped().project();

    /// Replaces a bucket's policy. `hipc api` needs both `--public` and `--remove-others`
    /// whatever the body: telling whether an S3 policy is public would mean judging it, and
    /// a misjudged one would publish the bucket.
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

/// The body of [`SUBSCRIBE`].
#[derive(Debug, Serialize)]
pub struct SubscribeBody<'a> {
    /// The plan's code.
    pub plan: &'a str,
    /// How it is paid, such as `credits`.
    pub rail: &'a str,
}

/// The body of [`SUBSCRIPTION_CHANGE`] that switches the plan.
#[derive(Debug, Serialize)]
pub struct PlanChangeBody<'a> {
    /// The new plan's code.
    pub plan: &'a str,
}

/// The body of [`BUCKET_CREATE`].
#[derive(Debug, Serialize)]
pub struct BucketCreateBody<'a> {
    /// The bucket's name.
    pub name: &'a str,
    /// Its region, when one is chosen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<&'a str>,
}

/// The body of [`BUCKET_ACL_SET`].
#[derive(Debug, Serialize)]
pub struct BucketAclSetBody<'a> {
    /// The bucket policy, a JSON object, which replaces the whole policy.
    pub policy: &'a serde_json::Value,
}

/// The body of [`MASTER_TOKEN_CREATE`] and [`SUB_TOKEN_CREATE`]: only the fields given. The
/// limits are a sub token's only.
#[derive(Debug, Default, Serialize)]
pub struct TokenCreateBody<'a> {
    /// The token's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<&'a str>,
    /// When it expires.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<&'a str>,
    /// What it may reach.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_type: Option<&'a str>,
    /// The buckets it may reach.
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    pub bucket_names: &'a [String],
    /// The S3 actions it may take.
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    pub actions: &'a [String],
    /// The key prefixes it may reach.
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    pub allowed_prefixes: &'a [String],
    /// The addresses it may be used from.
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    pub ip_allowlist: &'a [String],
    /// When it starts to work.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_before: Option<&'a str>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_body_sends_only_the_fields_given() {
        let buckets = ["logs".to_owned()];
        let body = TokenCreateBody {
            name: Some("ci"),
            bucket_names: &buckets,
            ..TokenCreateBody::default()
        };
        assert_eq!(
            serde_json::to_string(&body).unwrap(),
            r#"{"name":"ci","bucket_names":["logs"]}"#
        );
    }
}
