//! The accounts API: the accounts a token can act in, the signed-in user, the team,
//! projects, billing accounts, and the user's notification settings and referral links.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::routes::{Guard, Route, RouteFlag};
use crate::view::field;

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

    /// Requests the account's deletion, which only the console sends: it locks the account
    /// and revokes every token at once.
    DELETION_REQUEST: 0 = Route::post("/api/account/deletion/")
        .own_account()
        .guard(Guard::Refused {
            why: "an account deletion locks the account and revokes every token at once",
            hint: "request the deletion in the console, under Settings, Delete Account",
        });

    /// Withdraws a pending deletion request, which locks and revokes nothing (swagger), so it
    /// lends the request's refusal to no other method.
    DELETION_WITHDRAW: 0 = Route::delete("/api/account/deletion/")
        .own_account()
        .guard(Guard::Needs(&[]));
}

/// The body of [`NOTIFICATIONS_UPDATE`]: only the settings given, so one left out keeps its
/// value.
#[derive(Debug, Default, Serialize)]
pub struct NotificationsUpdateBody {
    /// Whether any notification email is sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email_enabled: Option<bool>,
    /// The email when the balance runs low.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub low_credit_alerts: Option<bool>,
    /// The email when the balance reaches zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zero_balance_alerts: Option<bool>,
    /// The updates when files are pinned or fail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_status_updates: Option<bool>,
    /// Marketing and feature updates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marketing_emails: Option<bool>,
    /// The new-advice digest of the providers dashboard.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_advice_emails: Option<bool>,
}

/// The body of [`MARKETING_CONSENT_UPDATE`].
#[derive(Debug, Serialize)]
pub struct MarketingConsentUpdateBody<'a> {
    /// Whether the consent is given.
    pub granted: bool,
    /// Where the change was made, such as `settings`.
    pub source: &'a str,
}

/// The body of [`INVITE_PREVIEW`]. `Debug` never shows the token, a bearer secret.
#[derive(Serialize)]
pub struct InvitePreviewBody<'a> {
    /// The invite token.
    pub token: &'a str,
}

impl fmt::Debug for InvitePreviewBody<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InvitePreviewBody").finish_non_exhaustive()
    }
}

/// The body of [`INVITE_ACCEPT`]. `Debug` never shows the token, a bearer secret.
#[derive(Serialize)]
pub struct InviteAcceptBody<'a> {
    /// The invite token.
    pub token: &'a str,
    /// Whether to join as a user the invite was not emailed to.
    pub confirm_without_email: bool,
}

impl fmt::Debug for InviteAcceptBody<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InviteAcceptBody")
            .field("confirm_without_email", &self.confirm_without_email)
            .finish_non_exhaustive()
    }
}

/// One grant of a member or an invite, as the console sends it: on the whole account, or on
/// the one project or billing account it names.
#[derive(Debug, Serialize)]
pub struct GrantBody<'a> {
    /// What the grant covers, such as `compute`.
    pub scope: &'a str,
    /// How much, such as `read`.
    pub level: &'a str,
    /// The project the grant is limited to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<&'a str>,
    /// The billing account the grant is limited to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub billing: Option<&'a str>,
}

/// The body of [`MEMBER_UPDATE`]: only the keys given. `grants` replaces the member's whole
/// list.
#[derive(Debug, Serialize)]
pub struct MemberUpdateBody<'a> {
    /// Every grant the member keeps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grants: Option<Vec<GrantBody<'a>>>,
    /// Whether the member manages the team.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_manage_team: Option<bool>,
    /// Whether the member manages billing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_billing: Option<bool>,
}

/// The body of [`INVITE_CREATE`], with every key, as the console sends it.
#[derive(Debug, Serialize)]
pub struct InviteCreateBody<'a> {
    /// The address the invite is emailed to.
    pub email: &'a str,
    /// The grants the invite gives.
    pub grants: Vec<GrantBody<'a>>,
    /// Whether the new member manages the team.
    pub can_manage_team: bool,
    /// Whether the new member manages billing.
    pub can_billing: bool,
}

/// The body of [`PROJECT_CREATE`].
#[derive(Debug, Serialize)]
pub struct ProjectCreateBody<'a> {
    /// The project's name.
    pub name: &'a str,
    /// Its billing account; `null` is the Default one.
    pub billing: Option<&'a str>,
}

/// The body that names or renames something: [`PROJECT_UPDATE`], [`BILLING_ACCOUNT_CREATE`],
/// [`BILLING_ACCOUNT_UPDATE`], [`REFERRAL_CODE_CREATE`], and [`REFERRAL_CODE_UPDATE`].
#[derive(Debug, Serialize)]
pub struct NameBody<'a> {
    /// The name.
    pub name: &'a str,
}

/// The body of [`PROJECT_BUDGET_UPDATE`], whole: it replaces the budget.
#[derive(Debug, Serialize)]
pub struct ProjectBudgetUpdateBody<'a> {
    /// The monthly limit, a decimal string of credits.
    pub monthly_limit: &'a str,
    /// The percentages of the limit at which an alert is mailed, ascending.
    pub thresholds: &'a [u8],
    /// Whether a launch over the limit is refused.
    pub block_launch_over: bool,
}

/// The body of [`PROJECT_QUOTA_UPDATE`]: `{limits}`, which the server merges.
#[derive(Debug, Serialize)]
pub struct ProjectQuotaUpdateBody {
    /// The limits given.
    pub limits: QuotaLimits,
}

/// A project's own compute limits: only those given are sent, and `null` is no limit of its
/// own.
#[derive(Debug, Default, Serialize)]
#[expect(
    clippy::option_option,
    reason = "a limit left out keeps its value, and `null` removes it"
)]
pub struct QuotaLimits {
    /// Virtual machines.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vms: Option<Option<u64>>,
    /// Databases.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub databases: Option<Option<u64>>,
    /// Kubernetes clusters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clusters: Option<Option<u64>>,
    /// Virtual CPUs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vcpus: Option<Option<u64>>,
    /// Memory, in GB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_gb: Option<Option<u64>>,
    /// Disk, in GB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_gb: Option<Option<u64>>,
    /// Public IP addresses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_ips: Option<Option<u64>>,
    /// CI runners.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runners: Option<Option<u64>>,
}

/// The body of [`PROJECT_TRANSFER`].
#[derive(Debug, Serialize)]
pub struct ProjectTransferBody<'a> {
    /// The account the project is offered to.
    pub to_account: &'a str,
}

/// The body of [`PROJECT_TRANSFER_ACCEPT`].
#[derive(Debug, Serialize)]
pub struct ProjectTransferAcceptBody<'a> {
    /// The billing account the project is billed to; `null` is the Default one.
    pub billing: Option<&'a str>,
}

/// One entry of the `accounts` of [`ACCOUNTS`], as far as `hipc` decides on it.
#[derive(Debug, Default, Deserialize)]
pub struct AccountEntry {
    /// The account's id.
    #[serde(default, deserialize_with = "field")]
    pub id: Option<String>,
    /// Whether this is the token's own account rather than a membership.
    #[serde(default, deserialize_with = "field")]
    pub is_owner: Option<bool>,
}

/// What [`INVITE_PREVIEW`] says of an invite, as far as `hipc` decides on it.
#[derive(Debug, Default, Deserialize)]
pub struct InviteOffer {
    /// `false` once the invite can no longer be used.
    #[serde(default, deserialize_with = "field")]
    pub valid: Option<bool>,
    /// Why it cannot: `accepted`, `revoked`, or `expired`.
    #[serde(default, deserialize_with = "field")]
    pub reason: Option<String>,
    /// The account the invite joins.
    #[serde(default, deserialize_with = "field")]
    pub account_name: Option<String>,
    /// Who sent it.
    #[serde(default, deserialize_with = "field")]
    pub invited_by: Option<String>,
}

/// What [`INVITE_ACCEPT`] answers, `{account}`, as far as `hipc` decides on it.
#[derive(Debug, Default, Deserialize)]
pub struct InviteAccepted {
    /// The account joined.
    #[serde(default, deserialize_with = "field")]
    pub account: Option<AccountEntry>,
}

/// One project of [`PROJECTS`], as far as `hipc` decides on it.
#[derive(Debug, Default, Deserialize)]
pub struct ProjectEntry {
    /// The project's id.
    #[serde(default, deserialize_with = "field")]
    pub id: Option<String>,
    /// The project's name.
    #[serde(default, deserialize_with = "field")]
    pub name: Option<String>,
}

/// One transfer of [`PROJECT_TRANSFERS`], as far as `hipc` decides on it.
#[derive(Debug, Default, Deserialize)]
pub struct TransferEntry {
    /// The transfer's id.
    #[serde(default, deserialize_with = "field")]
    pub id: Option<u64>,
    /// The project offered.
    #[serde(default, deserialize_with = "field")]
    pub project: Option<Principal>,
    /// The account that offered it.
    #[serde(default, deserialize_with = "field")]
    pub from_account: Option<Principal>,
    /// The account it is offered to.
    #[serde(default, deserialize_with = "field")]
    pub to_account: Option<Principal>,
}

/// A project or account as a transfer names it, `{id, name}`.
#[derive(Debug, Default, Deserialize)]
pub struct Principal {
    /// Its id.
    #[serde(default, deserialize_with = "field")]
    pub id: Option<String>,
    /// Its name.
    #[serde(default, deserialize_with = "field")]
    pub name: Option<String>,
}

/// One referral link of [`REFERRAL_CODES`], as far as `hipc` decides on it.
#[derive(Debug, Default, Deserialize)]
pub struct ReferralEntry {
    /// The referral code.
    #[serde(default, deserialize_with = "field")]
    pub code: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::{json, to_value};

    use super::*;

    /// A key left out is not sent, and a limit of `None` is sent as `null`.
    #[test]
    fn a_body_sends_only_the_keys_given() {
        let grants = vec![
            GrantBody {
                scope: "compute",
                level: "read",
                project: None,
                billing: None,
            },
            GrantBody {
                scope: "billing",
                level: "admin",
                project: None,
                billing: Some("bill_1"),
            },
        ];
        let update = MemberUpdateBody {
            grants: Some(grants),
            can_manage_team: None,
            can_billing: Some(false),
        };
        assert_eq!(
            serde_json::to_string(&update).unwrap(),
            r#"{"grants":[{"scope":"compute","level":"read"},{"scope":"billing","level":"admin","billing":"bill_1"}],"can_billing":false}"#
        );
        let quota = ProjectQuotaUpdateBody {
            limits: QuotaLimits {
                vms: Some(Some(3)),
                runners: Some(None),
                ..QuotaLimits::default()
            },
        };
        assert_eq!(
            to_value(&quota).unwrap(),
            json!({"limits": {"vms": 3, "runners": null}})
        );
        assert_eq!(
            to_value(NotificationsUpdateBody::default()).unwrap(),
            json!({})
        );
        let create = ProjectCreateBody {
            name: "web",
            billing: None,
        };
        assert_eq!(
            serde_json::to_string(&create).unwrap(),
            r#"{"name":"web","billing":null}"#
        );
    }

    /// The `Debug` of a body that holds an invite token never shows it.
    #[test]
    fn an_invite_body_debugs_without_its_token() {
        let preview = format!("{:?}", InvitePreviewBody { token: "s3cret" });
        let accept = format!(
            "{:?}",
            InviteAcceptBody {
                token: "s3cret",
                confirm_without_email: false
            }
        );
        for shown in [preview, accept] {
            assert!(!shown.contains("s3cret"), "{shown}");
        }
    }
}
