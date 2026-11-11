//! The Hub: plans, the subscription, the namespace and its robot, keys, repositories and
//! artifacts, events, usage, and the model index.

use crate::routes::{Guard, Route, RouteFlag};

routes! {
    /// The Hub plans.
    PLANS: 0 = Route::get("/api/registry/plans/").personal();

    /// The Hub subscriptions.
    SUBSCRIPTIONS: 0 = Route::get("/api/registry/subscriptions/").own_account();

    /// Subscribes to a Hub plan.
    SUBSCRIBE: 0 = Route::post("/api/registry/subscribe/")
        .own_account()
        .guard(Guard::Refused {
            why: "it reads no current plan first, so it could start a second paid Hub subscription",
            hint: "run `hipc hub subscription subscribe`, which refuses a second plan and \
                   replaces one only with `--replace`",
        });

    /// Cancels a Hub subscription.
    SUBSCRIPTION_CANCEL: 1 = Route::delete("/api/registry/subscriptions/{subscription_id}/")
        .own_account();

    /// The Hub usage.
    USAGE: 0 = Route::get("/api/registry/usage/").own_account();

    /// The Hub usage by repository.
    USAGE_BY_REPOSITORY: 0 = Route::get("/api/registry/usage/repositories/").own_account();

    /// Whether a namespace name is free.
    NAMESPACE_CHECK: 0 = Route::get("/api/registry/namespaces/check/").own_account();

    /// Creates the namespace and its robot login.
    NAMESPACE_CREATE: 0 = Route::post("/api/registry/provision/").own_account().reveals();

    /// Whether the namespace is set up.
    NAMESPACE_STATUS: 0 = Route::get("/api/registry/provision/status/").own_account();

    /// The namespace.
    NAMESPACE: 0 = Route::get("/api/registry/me/").own_account();

    /// Makes the namespace public or private. Without `public` set to `false` the server may
    /// read the body as a toggle, so `hipc api` needs `--public`, and its refusal names who
    /// can pull rather than a bucket.
    PUBLICITY_UPDATE: 0 = Route::patch("/api/registry/me/publicity/")
        .own_account()
        .guard(Guard::NeedsUnlessFalse(&[RouteFlag::Public], "public"));

    /// The robot login.
    ROBOT: 0 = Route::get("/api/registry/me/robot/").own_account();

    /// Rotates the robot login's secret.
    ROBOT_ROTATE: 0 = Route::post("/api/registry/robot/rotate/").own_account().reveals();

    /// The Hub keys.
    KEYS: 0 = Route::get("/api/registry/keys/").own_account();

    /// One Hub key.
    KEY: 1 = Route::get("/api/registry/keys/{key_id}/").own_account();

    /// Creates a Hub key.
    KEY_CREATE: 0 = Route::post("/api/registry/keys/").own_account().reveals();

    /// Rotates a Hub key.
    KEY_ROTATE: 1 = Route::post("/api/registry/keys/{key_id}/rotate/").own_account().reveals();

    /// Deletes a Hub key.
    KEY_DELETE: 1 = Route::delete("/api/registry/keys/{key_id}/").own_account();

    /// The namespace's audit events.
    EVENTS: 0 = Route::get("/api/registry/events/").own_account();

    /// The repositories.
    REPOSITORIES: 0 = Route::get("/api/registry/repositories/").own_account();

    /// Deletes a repository.
    REPOSITORY_DELETE: 1 = Route::delete("/api/registry/repositories/{repository..}/")
        .own_account()
        .permanent();

    /// A repository's artifacts.
    ARTIFACTS: 1 = Route::get("/api/registry/repositories/{repository..}/artifacts/").own_account();

    /// One artifact.
    ARTIFACT: 2 = Route::get("/api/registry/repositories/{repository..}/artifacts/{reference}/")
        .own_account();

    /// Deletes an artifact.
    ARTIFACT_DELETE: 2 = Route::delete("/api/registry/repositories/{repository..}/artifacts/{reference}/")
        .own_account()
        .permanent();

    /// The model index.
    MODELS: 0 = Route::get("/api/models/").own_account().paged();

    /// The model index's filters.
    MODEL_FORMATS: 0 = Route::get("/api/models/formats/").own_account();

    /// One model, with its versions.
    MODEL: 1 = Route::get("/api/models/{model..}/").own_account().paged();

    /// One version of a model.
    MODEL_VERSION: 2 = Route::get("/api/models/{model..}/{reference}/").own_account();
}
