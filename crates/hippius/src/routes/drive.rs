//! Hippius Drive plans and the subscription.

use serde::Serialize;

use crate::routes::{Paging, Route};

routes! {
    /// The Drive plans.
    PLANS: 0 = Route::get("/api/drive/plans/").personal();

    /// The Drive subscription.
    SUBSCRIPTION: 0 = Route::get("/api/drive/subscription/").own_account();

    /// Subscribes to a Drive plan.
    SUBSCRIBE: 0 = Route::post("/api/drive/subscription/").own_account().undone_on_503();

    /// Changes the Drive subscription.
    SUBSCRIPTION_CHANGE: 0 = Route::patch("/api/drive/subscription/").own_account();

    /// Cancels the Drive subscription.
    SUBSCRIPTION_CANCEL: 0 = Route::delete("/api/drive/subscription/").own_account();

    /// A card checkout of a Drive plan.
    CHECKOUT: 1 = Route::get("/api/drive/subscription/checkout/{intent_id}/").own_account();

    /// The Drive subscription's history.
    HISTORY: 0 = Route::get("/api/drive/subscription/history/").own_account().paged(Paging::Numbered);
}

/// The body of [`SUBSCRIBE`].
#[derive(Debug, Serialize)]
pub struct SubscribeBody<'a> {
    /// The plan's code.
    pub plan: &'a str,
    /// How it is paid, such as `credits`.
    pub payment: &'a str,
    /// The billing period, when one is chosen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<&'a str>,
}

/// The body of [`SUBSCRIPTION_CHANGE`] that switches the plan. It has no `payment`: plan
/// changes are paid from the balance.
#[derive(Debug, Serialize)]
pub struct PlanChangeBody<'a> {
    /// The new plan's code.
    pub plan: &'a str,
    /// The billing period, when one is chosen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<&'a str>,
}
