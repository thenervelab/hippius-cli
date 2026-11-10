//! The accounts API: the accounts a token can act in, the signed-in user, the team,
//! projects, billing accounts, and the user's notification settings and referral links.

use crate::routes::{Guard, Route, RouteFlag};

routes! {
    /// The accounts the token can act in: the caller's own, then each membership.
    ACCOUNTS: 0 = Route::get("/api/accounts/").personal();

    /// The signed-in user.
    USER: 0 = Route::get("/api/users/me/").personal();

    /// The team members, with their grants.
    MEMBERS: 0 = Route::get("/api/accounts/members/").scoped();

    /// Changes a member's grants or permissions.
    MEMBER_UPDATE: 1 = Route::patch("/api/accounts/members/{member_id}/")
        .scoped()
        .guard(Guard::NeedsWhenSet(&[RouteFlag::RemoveOthers], "grants"));

    /// Removes a member from the team.
    MEMBER_DELETE: 1 = Route::delete("/api/accounts/members/{member_id}/")
        .scoped()
        .guard(Guard::NeedsWhenSet(&[RouteFlag::RemoveOthers], "grants"));

    /// The pending invites.
    INVITES: 0 = Route::get("/api/accounts/invites/").scoped();

    /// Emails an invite.
    INVITE_CREATE: 0 = Route::post("/api/accounts/invites/").scoped();

    /// Revokes a pending invite.
    INVITE_REVOKE: 1 = Route::delete("/api/accounts/invites/{invite_id}/").scoped();

    /// What an invite token offers; changes nothing.
    INVITE_PREVIEW: 0 = Route::post("/api/accounts/invites/preview/").personal();

    /// Joins the team an invite token names.
    INVITE_ACCEPT: 0 = Route::post("/api/accounts/invites/accept/").personal();

    /// The account's projects.
    PROJECTS: 0 = Route::get("/api/accounts/projects/").scoped();

    /// Creates a project.
    PROJECT_CREATE: 0 = Route::post("/api/accounts/projects/").scoped();

    /// Renames or archives a project.
    PROJECT_UPDATE: 1 = Route::patch("/api/accounts/projects/{project_id}/").scoped().names_one();

    /// Deletes a project.
    PROJECT_DELETE: 1 = Route::delete("/api/accounts/projects/{project_id}/").scoped().names_one();

    /// A project's budget.
    PROJECT_BUDGET: 1 = Route::get("/api/accounts/projects/{project_id}/budget/")
        .scoped()
        .names_one();

    /// Sets a project's budget.
    PROJECT_BUDGET_UPDATE: 1 = Route::put("/api/accounts/projects/{project_id}/budget/")
        .scoped()
        .names_one();

    /// Removes a project's budget.
    PROJECT_BUDGET_DELETE: 1 = Route::delete("/api/accounts/projects/{project_id}/budget/")
        .scoped()
        .names_one();

    /// A project's quota.
    PROJECT_QUOTA: 1 = Route::get("/api/accounts/projects/{project_id}/quota/")
        .scoped()
        .names_one();

    /// Changes a project's quota.
    PROJECT_QUOTA_UPDATE: 1 = Route::patch("/api/accounts/projects/{project_id}/quota/")
        .scoped()
        .names_one();

    /// Offers a project to another account.
    PROJECT_TRANSFER: 1 = Route::post("/api/accounts/projects/{project_id}/transfer/")
        .scoped()
        .names_one();

    /// The project transfers offered to and by the caller.
    PROJECT_TRANSFERS: 0 = Route::get("/api/accounts/project-transfers/").personal();

    /// Accepts a project transfer.
    PROJECT_TRANSFER_ACCEPT: 1 = Route::post("/api/accounts/project-transfers/{transfer_id}/accept/")
        .personal();

    /// Declines a project transfer.
    PROJECT_TRANSFER_DECLINE: 1 = Route::post("/api/accounts/project-transfers/{transfer_id}/decline/")
        .personal();

    /// Cancels a project transfer the caller offered.
    PROJECT_TRANSFER_CANCEL: 1 = Route::delete("/api/accounts/project-transfers/{transfer_id}/")
        .personal();

    /// The account's billing accounts.
    BILLING_ACCOUNTS: 0 = Route::get("/api/accounts/billing-accounts/").scoped();

    /// Creates a billing account.
    BILLING_ACCOUNT_CREATE: 0 = Route::post("/api/accounts/billing-accounts/").scoped();

    /// Renames a billing account.
    BILLING_ACCOUNT_UPDATE: 1 = Route::patch("/api/accounts/billing-accounts/{billing_account_id}/")
        .scoped()
        .names_one();

    /// A billing account's costs, by project.
    BILLING_ACCOUNT_COSTS: 1 = Route::get("/api/accounts/billing-accounts/{billing_account_id}/costs/")
        .scoped();

    /// The signed-in user's notification settings.
    NOTIFICATIONS: 0 = Route::get("/api/notifications/settings/").personal();

    /// Changes the notification settings given.
    NOTIFICATIONS_UPDATE: 0 = Route::patch("/api/notifications/settings/").personal();

    /// The signed-in user's marketing consent.
    MARKETING_CONSENT: 0 = Route::get("/api/notifications/marketing-consent/").personal();

    /// Records a marketing consent change.
    MARKETING_CONSENT_UPDATE: 0 = Route::post("/api/notifications/marketing-consent/").personal();

    /// The signed-in user's referral links.
    REFERRAL_CODES: 0 = Route::get("/api/referral-codes/").personal();

    /// Creates a referral link.
    REFERRAL_CODE_CREATE: 0 = Route::post("/api/referral-codes/").personal();

    /// Renames a referral link.
    REFERRAL_CODE_UPDATE: 1 = Route::patch("/api/referral-codes/{referral_id}/").personal();

    /// What the signed-in user's referral code brought.
    REFERRAL_ANALYTICS: 0 = Route::get("/api/referral/analytics/").personal();
}
