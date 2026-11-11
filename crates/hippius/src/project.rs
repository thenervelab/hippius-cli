//! Which project and billing account a request acts in, and whether `X-Hippius-Project` and
//! `X-Hippius-Billing` go on it (tenet 12).
//!
//! A project holds resources (VMs, databases, clusters, runners, S3 buckets and keys, CDN
//! zones), and a billing account is a wallet that pays for projects. Without a header, a
//! request acts in the Default project and the Default billing account, which are the
//! account itself (<https://docs.hippius.com/use/console/projects>, read 2026-10-30).
//!
//! Each header has its own allowlist, narrower than the account header's, matching the routes
//! the console sends that header on.
//!
//! The server refuses a selector on a route that does not take it, with 403
//! `project_scope_unsupported` or `billing_scope_unsupported`, the owner included. So with one
//! from a flag or a variable, `hipc` refuses such a route locally and sends nothing, as it
//! refuses a membership on a route that cannot act in another account. One stored in the
//! profile is left off a read of such a route instead, and a stored project is refused on a
//! write (`header` says why), while a stored billing account is left off a write too. A
//! stored project is left off every method of the routes whose path names the one they act
//! on: the four routes of one project (the project, its budget, quota, and transfer) and the
//! route of one billing account.
//! A personal route (the account header's personal class) acts as the signed-in user in
//! every project and billing account, so it goes out without the header, as it goes out
//! without the account header.
//!
//! A route a command sends has a row in `routes`, which says how it takes each selector; the
//! tables below decide only a path no row matches. They compare a path with its trailing `/`
//! added, so the exclusions match a slashless
//! path too: `/api/compute/usage` is refused a project, as `/api/compute/usage/` is.

use crate::account::{BILLING_NAMED_ONLY_PATHS, classify, listed_class, matches_pattern};
use crate::error::Result;
use crate::method::{Access, Method};
use crate::phrase::quote;
use crate::routes::{self, Endpoint, RouteClass, Take};
use crate::selector::{BillingId, ProjectId, Selected, Selector, is_id};
use crate::source::Source;
use crate::url::{with_trailing_slash, without_query_values};

/// The prefixes of the routes that take a project: compute, managed databases, Kubernetes,
/// runners, S3 buckets and keys, and the CDN. The S3 plan (`/api/s3/`) stays the account's,
/// apart from its two reads below, and so does the accounts API.
const PROJECT_PREFIXES: &[&str] = &[
    "/api/compute/",
    "/api/databases/",
    "/api/kubernetes/",
    "/api/runners/",
    "/api/objectstore/",
    "/api/cdn/",
];

/// Inside those prefixes, the compute billing reads, which take a billing account instead.
pub(crate) const PROJECT_EXCLUDED_PATHS: &[&str] =
    &["/api/compute/usage/", "/api/compute/billing/summary/"];

/// Inside those prefixes, the S3 master keys, which only the Default project has: a
/// project's master key is held by the backend.
const PROJECT_EXCLUDED_PREFIX: &str = "/api/objectstore/master-tokens/";

/// The accounts API routes of one project, whose path names the project they act on, so
/// the header has nothing to add: the project itself (rename, archive), and its budget,
/// quota, and transfer. `{id}` is a project id ([`names_in_path`]).
const PROJECT_IN_PATH: &[&str] = &[
    "/api/accounts/projects/{id}/",
    "/api/accounts/projects/{id}/budget/",
    "/api/accounts/projects/{id}/quota/",
    "/api/accounts/projects/{id}/transfer/",
];

/// The accounts API route of one billing account, whose path names the billing account it
/// acts on (rename), as [`PROJECT_IN_PATH`] does a project. Its costs are a read, which a
/// stored billing account is left off already. `{id}` is a billing account id.
const BILLING_IN_PATH: &[&str] = &["/api/accounts/billing-accounts/{id}/"];

/// Whether `form` (with its trailing `/`) is the accounts route of one project or of one
/// billing account. Either path decides what the write acts on, and neither route takes a
/// project or a billing account header, so a stored selection of either kind is left off
/// both ([`Take::Named`]): a stored billing account says nothing about a project's rename,
/// and a stored project nothing about a billing account's.
fn names_one(form: &str) -> bool {
    names_in_path(Selector::Project, PROJECT_IN_PATH, form)
        || names_in_path(Selector::Billing, BILLING_IN_PATH, form)
}

/// The other routes that take a billing account, whole paths in the pattern language of
/// [`matches_pattern`].
pub(crate) const BILLING_PATHS: &[&str] = &[
    "/api/billing/credits/balance/",
    "/api/billing/invoices/",
    "/api/billing/invoices/{n}/pdf/",
    "/api/billing/profile/",
    "/api/billing/stripe/create-checkout-session/",
    "/api/billing/stripe/checkout-status/",
    "/api/billing/btcpay/create-invoice/",
    "/api/compute/usage/",
    "/api/compute/billing/summary/",
    "/api/s3/subscription/history/",
    "/api/billing/auto-reload/",
    "/api/billing/auto-reload/history/",
    "/api/billing/auto-reload/card-setup/",
];

/// What `method` to `path` (a request path, its query ignored) does with a project: its
/// row's answer, else the tables'. A path with a format suffix takes the stricter answer of
/// its two forms ([`classify`]); a route neither lists refuses.
pub(crate) fn project_take(method: Method, path: &str) -> Take {
    classify(path, Take::Refuse, |form| {
        routes::row(method, form)
            .map(Endpoint::project)
            .or_else(|| listed_project(method, form))
    })
}

/// What the tables say `method` to `form` (one form, no query) does with a project, or
/// `None` when they do not list it. Only a path no row matches reaches them.
pub(crate) fn listed_project(method: Method, form: &str) -> Option<Take> {
    if listed_class(method, form) == Some(RouteClass::Personal) {
        return Some(Take::Skip);
    }
    let form = with_trailing_slash(form);
    let excluded =
        PROJECT_EXCLUDED_PATHS.contains(&&*form) || form.starts_with(PROJECT_EXCLUDED_PREFIX);
    if excluded {
        Some(Take::Refuse)
    } else if names_one(&form) {
        Some(Take::Named)
    } else if PROJECT_PREFIXES.iter().any(|p| form.starts_with(p)) {
        Some(Take::Send)
    } else {
        None
    }
}

/// Whether `form` (with its trailing `/`) is one of `patterns` with an id of `selector` in
/// its `{id}` segment, the fourth. A written form whose id carries a format suffix
/// (`proj_….json`) is not one, so [`classify`] gives it the answer of its plain form, which is.
fn names_in_path(selector: Selector, patterns: &[&str], form: &str) -> bool {
    let id = form.split('/').nth(4).unwrap_or_default();
    is_id(selector, id) && patterns.iter().any(|p| matches_pattern(p, form))
}

/// What `method` to `path` (a request path, its query ignored) does with a billing account,
/// by the same rules as [`project_take`].
pub(crate) fn billing_take(method: Method, path: &str) -> Take {
    classify(path, Take::Refuse, |form| {
        routes::row(method, form)
            .map(Endpoint::billing)
            .or_else(|| listed_billing(method, form))
    })
}

/// What the tables say `method` to `form` does with a billing account, as
/// [`listed_project`] says it of a project.
pub(crate) fn listed_billing(method: Method, form: &str) -> Option<Take> {
    if listed_class(method, form) == Some(RouteClass::Personal) {
        return Some(Take::Skip);
    }
    let form = with_trailing_slash(form);
    if names_one(&form) {
        return Some(Take::Named);
    }
    BILLING_NAMED_ONLY_PATHS
        .iter()
        .chain(BILLING_PATHS)
        .any(|pattern| matches_pattern(pattern, &form))
        .then_some(Take::Send)
}

/// The `X-Hippius-Project` value for a `method` request to `path`, or `None` when no
/// header goes on it. `project` is the selected project, `None` for the Default one. `access`
/// says whether the request may change state.
///
/// # Errors
///
/// `project_scope_unsupported` when a project from a flag or a variable is selected and the
/// route does not take one. No request may be sent then.
pub(crate) fn project_header<'a>(
    project: Option<&'a ProjectId>,
    method: Method,
    access: Access,
    path: &str,
) -> Result<Option<&'a str>> {
    let Some(ProjectId(project)) = project else {
        return Ok(None);
    };
    header(project, project_take(method, path), method, access, path)
}

/// The `X-Hippius-Billing` value for a `method` request to `path`, or `None` when no header
/// goes on it. `billing` is the selected billing account, `None` for the Default one. `access`
/// says whether the request may change state.
///
/// # Errors
///
/// `billing_scope_unsupported` when a billing account from a flag or a variable is selected
/// and the route does not take one. No request may be sent then.
pub(crate) fn billing_header<'a>(
    billing: Option<&'a BillingId>,
    method: Method,
    access: Access,
    path: &str,
) -> Result<Option<&'a str>> {
    let Some(BillingId(billing)) = billing else {
        return Ok(None);
    };
    header(billing, billing_take(method, path), method, access, path)
}

/// The billing account a `method` request to `path` leaves off although one is selected: a
/// stored one on a route that takes none, which [`billing_header`] skips rather than refuses.
/// A charge such a write makes goes to the Default billing account (tenet 12), so the
/// question of a write names it. A personal route acts as the caller, and a route that names
/// one project or billing account in its path acts on that one, so neither is reported. Only
/// meaningful once [`billing_header`] accepted the request.
pub(crate) fn skipped_billing<'a>(
    billing: Option<&'a BillingId>,
    method: Method,
    path: &str,
) -> Option<&'a str> {
    let BillingId(selected) = billing?;
    (billing_take(method, path) == Take::Refuse).then_some(selected.id.as_str())
}

/// The header value of `selected` on a route that `take`s it so: sent, left off, or refused.
/// A stored selection is left off a read of a route that refuses it, and off any request to a
/// route that names the project or billing account in its path; a stored billing account is
/// left off a write to a route that refuses it too. A stored project's write elsewhere, or
/// one from a flag or a variable, is refused.
fn header<'a>(
    selected: &'a Selected,
    take: Take,
    method: Method,
    access: Access,
    path: &str,
) -> Result<Option<&'a str>> {
    match take {
        Take::Send => Ok(Some(&selected.id)),
        Take::Skip => Ok(None),
        // A project or billing account stored with
        // `hipc account projects use` or `billing-accounts use` is the profile's standing
        // choice for the routes that take it, so reads of the accounts API, Drive, and the
        // balance keep working beside it. A stored billing account is left off a write there
        // too: a route that takes no billing account bills no one by the header, so leaving
        // it off moves nothing. A write is still refused a stored project: sent without the
        // header it would act in the Default project the caller did not pick. One named for
        // this run, by a flag or a variable, is refused on any method: the caller asked for
        // it on this run, and a flag wins over the profile (tenet 8). The refusal names
        // which one (tenet 15).
        Take::Refuse
            if matches!(selected.source, Source::Profile { .. })
                && (access == Access::Read || matches!(selected.selector, Selector::Billing)) =>
        {
            Ok(None)
        }
        // A write to one project's or billing account's accounts route acts on the one its
        // path names, so leaving the stored one off cannot move it into the Default one.
        Take::Named if matches!(selected.source, Source::Profile { .. }) => Ok(None),
        Take::Named | Take::Refuse => {
            let noun = selected.selector.noun();
            // A `hipc api` path can carry a secret in its query (tenet 9).
            let route = quote(format!("{method} {}", without_query_values(path)));
            Err(selected.refusal(&format!("{route} does not take a {noun}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::names_billing_only;
    use crate::account::tests::{SUFFIXES, slashless, suffixed};
    use crate::error::Code;

    const PROJECT: &str = "proj_0a1b2c3d4e5f60718293";
    const BILLING: &str = "bill_9f8e7d6c5b4a39281706";

    fn project() -> ProjectId {
        ProjectId::parse(PROJECT, &Source::Argument)
            .unwrap()
            .unwrap()
    }

    fn billing() -> BillingId {
        BillingId::parse(BILLING, &Source::Argument)
            .unwrap()
            .unwrap()
    }

    /// The ids of the docs' own examples parse, `default` is the Default one, and anything
    /// else is `usage`.
    #[test]
    fn parses_default_and_ids_and_rejects_the_rest() {
        assert_eq!(project().as_str(), PROJECT);
        assert_eq!(billing().as_str(), BILLING);
        assert!(
            ProjectId::parse("default", &Source::Argument)
                .unwrap()
                .is_none()
        );
        assert!(
            BillingId::parse("default", &Source::Argument)
                .unwrap()
                .is_none()
        );
        for bad in [
            "",
            "proj_",
            "Default",
            "self",
            "proj_0A1B2C3D4E5F60718293",
            "proj_0a1b2c3d4e5f6071829",
            "proj_0a1b2c3d4e5f607182930",
            "proj_0a1b2c3d4e5f6071829g",
            BILLING,
            "acct_0123456789abcdef0123",
        ] {
            let error = ProjectId::parse(bad, &Source::Argument).unwrap_err();
            assert_eq!(error.code(), Code::Usage, "{bad}");
        }
        for bad in ["", "bill_", PROJECT, "bill_9F8E7D6C5B4A39281706"] {
            let error = BillingId::parse(bad, &Source::Argument).unwrap_err();
            assert_eq!(error.code(), Code::Usage, "{bad}");
        }
    }

    /// The refusal of a bad id names where it came from, and the hint the fix for that
    /// source.
    #[test]
    fn a_bad_id_names_its_source_and_the_fix_for_it() {
        let error = ProjectId::parse("nope", &Source::Flag("--project")).unwrap_err();
        assert_eq!(
            error.message(),
            "`nope` from `--project` is not a project id: a project is `default`, or `proj_` \
             and 20 lowercase hex characters"
        );
        assert_eq!(
            error.hint(),
            Some("pass `--project default`, or an id from `hipc account projects list`")
        );
        let error = BillingId::parse("nope", &Source::Variable("HIPPIUS_BILLING")).unwrap_err();
        assert_eq!(
            error.message(),
            "`nope` from `HIPPIUS_BILLING` is not a billing account id: a billing account is \
             `default`, or `bill_` and 20 lowercase hex characters"
        );
        assert_eq!(
            error.hint(),
            Some(
                "unset `HIPPIUS_BILLING`, or set it to `default` or an id from `hipc account \
                 billing-accounts list`"
            )
        );
        let stored = Source::Profile {
            field: "billing",
            profile: "default".to_owned(),
            path: "/h/config.toml".into(),
        };
        let error = BillingId::parse("nope", &stored).unwrap_err();
        assert_eq!(
            error.message(),
            "`nope` from the `billing` of the profile `default` in `/h/config.toml` is not a \
             billing account id: a billing account is `default`, or `bill_` and 20 lowercase \
             hex characters"
        );
        assert_eq!(
            error.hint(),
            Some(
                "run `hipc account billing-accounts use default` to clear the stored billing \
                 account"
            )
        );
        let error = ProjectId::parse("nope", &Source::Argument).unwrap_err();
        assert_eq!(
            error.message(),
            "`nope` is not a project id: a project is `default`, or `proj_` and 20 lowercase \
             hex characters"
        );
        assert_eq!(
            error.hint(),
            Some("run `hipc account projects list` to see the project ids the token can act in")
        );
    }

    /// Every route family `hipc` sends to, with what it does with each selector,
    /// written out by hand from the console's tables, so a wrong classification fails here.
    /// Each row is the path, then the project's take on `GET` and on `POST`, then the billing
    /// account's on either.
    const ROUTES: &[(&str, Take, Take, Take)] = {
        use Take::{Named, Refuse, Send, Skip};
        &[
            // Compute, databases, Kubernetes, runners, object store, and the CDN take a
            // project and no billing account.
            ("/api/compute/vms/", Send, Send, Refuse),
            ("/api/compute/vms/v1/firewall/", Send, Send, Refuse),
            ("/api/compute/prices/", Send, Send, Refuse),
            ("/api/compute/charge-cap/", Send, Send, Refuse),
            ("/api/compute/feasibility/?flavor=small", Send, Send, Refuse),
            ("/api/databases/", Send, Send, Refuse),
            ("/api/databases/12/restore/", Send, Send, Refuse),
            ("/api/kubernetes/clusters/", Send, Send, Refuse),
            ("/api/runners/jobs/?before=812", Send, Send, Refuse),
            ("/api/objectstore/buckets/", Send, Send, Refuse),
            ("/api/objectstore/sub-tokens/7/rotate/", Send, Send, Refuse),
            ("/api/cdn/zones/z1/rules/", Send, Send, Refuse),
            ("/api/cdn/lists/", Send, Send, Refuse),
            // The WAF and its lists sit under the CDN prefix, so they take a project.
            ("/api/cdn/zones/z1/waf/", Send, Send, Refuse),
            ("/api/cdn/zones/z1/waf/rules/", Send, Send, Refuse),
            ("/api/cdn/zones/z1/waf/rate-limits/", Send, Send, Refuse),
            (
                "/api/cdn/zones/z1/waf/versions/3/rollback/",
                Send,
                Send,
                Refuse,
            ),
            ("/api/cdn/lists/blocked/items/", Send, Send, Refuse),
            ("/api/cdn/estimate/", Send, Send, Refuse),
            // The master keys belong to the Default project only.
            ("/api/objectstore/master-tokens/", Refuse, Refuse, Refuse),
            (
                "/api/objectstore/master-tokens/7/rotate/",
                Refuse,
                Refuse,
                Refuse,
            ),
            ("/api/objectstore/master-tokens", Refuse, Refuse, Refuse),
            // The compute billing reads take a billing account, not a project, with or
            // without their trailing slash.
            ("/api/compute/usage/", Refuse, Refuse, Send),
            ("/api/compute/usage/?month=2026-09", Refuse, Refuse, Send),
            ("/api/compute/usage", Refuse, Refuse, Send),
            ("/api/compute/billing/summary/", Refuse, Refuse, Send),
            // The S3 plan reads take a project on GET only; the history takes a billing
            // account.
            ("/api/s3/plans/", Send, Refuse, Refuse),
            ("/api/s3/subscription/", Send, Refuse, Refuse),
            ("/api/s3/subscription", Send, Refuse, Refuse),
            ("/api/s3/subscription/history/", Refuse, Refuse, Send),
            (
                "/api/s3/subscription/checkout/pi_1/",
                Refuse,
                Refuse,
                Refuse,
            ),
            // Billing.
            ("/api/billing/credits/balance/", Refuse, Refuse, Send),
            ("/api/billing/invoices/", Refuse, Refuse, Send),
            ("/api/billing/invoices/?page_size=100", Refuse, Refuse, Send),
            ("/api/billing/invoices/12/pdf/", Refuse, Refuse, Send),
            ("/api/billing/invoices/x/pdf/", Refuse, Refuse, Refuse),
            ("/api/billing/invoices/12/", Refuse, Refuse, Refuse),
            ("/api/billing/profile/", Refuse, Refuse, Send),
            // Swagger: "Only a top-up of the wallet the request acts on (X-Hippius-Billing)".
            (
                "/api/billing/stripe/checkout-status/?transaction_id=t1",
                Refuse,
                Refuse,
                Send,
            ),
            (
                "/api/billing/stripe/create-checkout-session/",
                Refuse,
                Refuse,
                Send,
            ),
            ("/api/billing/btcpay/create-invoice/", Refuse, Refuse, Send),
            (
                "/api/billing/stripe/active-subscription/",
                Refuse,
                Refuse,
                Send,
            ),
            (
                "/api/billing/stripe/create-subscription/",
                Refuse,
                Refuse,
                Send,
            ),
            ("/api/billing/stripe/customer-portal/", Refuse, Refuse, Send),
            ("/api/billing/auto-reload/", Refuse, Refuse, Send),
            ("/api/billing/auto-reload/history/", Refuse, Refuse, Send),
            ("/api/billing/auto-reload/card-setup/", Refuse, Refuse, Send),
            ("/api/billing/transactions/", Refuse, Refuse, Refuse),
            ("/api/billing/stripe/", Refuse, Refuse, Refuse),
            // The accounts API is the account's own, and the network is account-wide.
            ("/api/accounts/projects/", Refuse, Refuse, Refuse),
            // The routes of one project name it in the path, so they take neither selector
            // and a stored one of either kind is left off; a path with no project id in it,
            // or another route under one, is not one of them.
            (
                "/api/accounts/projects/proj_0a1b2c3d4e5f60718293/",
                Named,
                Named,
                Named,
            ),
            (
                "/api/accounts/projects/proj_0a1b2c3d4e5f60718293/budget/",
                Named,
                Named,
                Named,
            ),
            (
                "/api/accounts/projects/proj_0a1b2c3d4e5f60718293/quota/",
                Named,
                Named,
                Named,
            ),
            (
                "/api/accounts/projects/proj_0a1b2c3d4e5f60718293/transfer/",
                Named,
                Named,
                Named,
            ),
            ("/api/accounts/projects/proj_x/", Refuse, Refuse, Refuse),
            ("/api/accounts/projects/bulk/", Refuse, Refuse, Refuse),
            (
                "/api/accounts/projects/bulk-archive/",
                Refuse,
                Refuse,
                Refuse,
            ),
            (
                "/api/accounts/projects/proj_0A1B2C3D4E5F60718293/",
                Refuse,
                Refuse,
                Refuse,
            ),
            (
                "/api/accounts/projects/proj_0a1b2c3d4e5f60718293/members/",
                Refuse,
                Refuse,
                Refuse,
            ),
            (
                "/api/accounts/projects/proj_0a1b2c3d4e5f60718293/budget/x/",
                Refuse,
                Refuse,
                Refuse,
            ),
            ("/api/accounts/projectsx/proj_x/", Refuse, Refuse, Refuse),
            ("/api/accounts/billing-accounts/", Refuse, Refuse, Refuse),
            (
                "/api/accounts/billing-accounts/default/costs/",
                Refuse,
                Refuse,
                Refuse,
            ),
            // The route of one billing account names it in the path, as the routes of one
            // project do; a path with no billing account id in it, or another route under
            // one, is not it.
            (
                "/api/accounts/billing-accounts/bill_9f8e7d6c5b4a39281706/",
                Named,
                Named,
                Named,
            ),
            (
                "/api/accounts/billing-accounts/bill_x/",
                Refuse,
                Refuse,
                Refuse,
            ),
            (
                "/api/accounts/billing-accounts/bill_9F8E7D6C5B4A39281706/",
                Refuse,
                Refuse,
                Refuse,
            ),
            (
                "/api/accounts/billing-accounts/bill_9f8e7d6c5b4a3928170/",
                Refuse,
                Refuse,
                Refuse,
            ),
            (
                "/api/accounts/billing-accounts/proj_0a1b2c3d4e5f60718293/",
                Refuse,
                Refuse,
                Refuse,
            ),
            (
                "/api/accounts/billing-accounts/default/",
                Refuse,
                Refuse,
                Refuse,
            ),
            (
                "/api/accounts/billing-accounts/bill_9f8e7d6c5b4a39281706/costs/",
                Refuse,
                Refuse,
                Refuse,
            ),
            (
                "/api/accounts/billing-accounts/bill_9f8e7d6c5b4a39281706/x/",
                Refuse,
                Refuse,
                Refuse,
            ),
            (
                "/api/accounts/billing-accountsx/bill_9f8e7d6c5b4a39281706/",
                Refuse,
                Refuse,
                Refuse,
            ),
            (
                "/api/accounts/projects/bill_9f8e7d6c5b4a39281706/",
                Refuse,
                Refuse,
                Refuse,
            ),
            ("/api/accounts/members/", Refuse, Refuse, Refuse),
            ("/api/network/", Refuse, Refuse, Refuse),
            // Drive, Hub, and the support chat take neither.
            ("/api/drive/subscription/", Refuse, Refuse, Refuse),
            ("/api/registry/subscriptions/", Refuse, Refuse, Refuse),
            ("/api/support/chat/inbox/", Refuse, Refuse, Refuse),
            // Personal routes act as the signed-in user: neither header goes on them.
            ("/api/accounts/", Skip, Skip, Skip),
            ("/api/accounts/project-transfers/1/", Skip, Skip, Skip),
            ("/api/accounts/invites/accept/", Skip, Skip, Skip),
            ("/api/ssh-keys/", Skip, Skip, Skip),
            ("/api/support/tickets/7/", Skip, Skip, Skip),
            ("/api/referral-codes/", Skip, Skip, Skip),
            ("/api/referral-codes/4/", Skip, Skip, Skip),
            ("/api/referral/analytics/", Skip, Skip, Skip),
            ("/api/users/me/", Skip, Skip, Skip),
            ("/api/compute/edge/x/", Skip, Skip, Skip),
            ("/api/databases/12/members/", Skip, Skip, Skip),
            ("/api/objectstore/tokens/x/", Skip, Skip, Skip),
            ("/api/s3/plans/accounts/", Skip, Skip, Skip),
            ("/api/drive/plans/", Skip, Skip, Skip),
            // The personal prefixes inside a project prefix, with and without their trailing
            // `/`: the slashless form is personal too, not the wider project prefix.
            ("/api/compute/edge/", Skip, Skip, Skip),
            ("/api/compute/edge", Skip, Skip, Skip),
            ("/api/kubernetes/agent/", Skip, Skip, Skip),
            ("/api/kubernetes/agent", Skip, Skip, Skip),
            ("/api/runners/agent/", Skip, Skip, Skip),
            ("/api/runners/agent", Skip, Skip, Skip),
            ("/api/cdn/node/", Skip, Skip, Skip),
            ("/api/cdn/node", Skip, Skip, Skip),
            ("/api/objectstore/tokens/", Skip, Skip, Skip),
            ("/api/objectstore/tokens", Skip, Skip, Skip),
            ("/api/databases/metrics/", Skip, Skip, Skip),
            ("/api/databases/metrics", Skip, Skip, Skip),
            ("/api/databases/12/members", Skip, Skip, Skip),
            ("/api/databases/12/backup-token/", Skip, Skip, Skip),
            ("/api/databases/12/backup-token", Skip, Skip, Skip),
        ]
    };

    #[test]
    fn every_route_family_takes_the_selectors_its_class_gives() {
        for &(path, on_get, on_post, billing) in ROUTES {
            assert_eq!(project_take(Method::Get, path), on_get, "GET {path}");
            assert_eq!(project_take(Method::Post, path), on_post, "POST {path}");
            for method in [Method::Get, Method::Post] {
                assert_eq!(
                    billing_take(method, path),
                    billing,
                    "billing {method} {path}"
                );
            }
        }
    }

    /// A route whose row names the project in its path names it whatever the path's value,
    /// `default` included, as the budget row does; a method the route has no row for falls
    /// to the tables, which name a project only by its id.
    #[test]
    fn a_row_that_names_its_project_names_any_value() {
        let path = "/api/accounts/projects/default/budget/";
        for method in [Method::Get, Method::Put, Method::Delete] {
            assert_eq!(project_take(method, path), Take::Named, "{method}");
            assert_eq!(billing_take(method, path), Take::Named, "{method}");
        }
        assert_eq!(project_take(Method::Post, path), Take::Refuse);
        assert_eq!(billing_take(Method::Post, path), Take::Refuse);
    }

    /// `classify` keeps the greater take, so failing closed rests on this order.
    #[test]
    fn the_takes_are_ordered_from_least_to_most_strict() {
        assert!(Take::Skip < Take::Send);
        assert!(Take::Send < Take::Named);
        assert!(Take::Named < Take::Refuse);
    }

    /// Each row of [`ROUTES`] without its trailing `/` takes what it takes with it, so a
    /// personal route such as `/api/databases/12/members` goes out without either header.
    #[test]
    fn a_slashless_route_takes_the_selectors_of_the_slashed_one() {
        for &(path, on_get, on_post, billing) in ROUTES {
            let path = slashless(path);
            assert_eq!(project_take(Method::Get, &path), on_get, "GET {path}");
            assert_eq!(project_take(Method::Post, &path), on_post, "POST {path}");
            assert_eq!(billing_take(Method::Get, &path), billing, "billing {path}");
        }
    }

    /// Each row of [`ROUTES`] with a format suffix, with and without its trailing `/`, is
    /// refused a selector exactly when the plain path is, and takes the plain path's answer,
    /// apart from the personal routes under a project prefix whose suffix sits on the personal
    /// segment itself: their written form matches only the prefix and so sends the project,
    /// which a personal route ignores.
    #[test]
    fn a_format_suffix_keeps_the_take_of_every_route_family() {
        let personal_under_the_prefix = [
            "/api/databases/12/members/",
            "/api/databases/12/members",
            "/api/databases/12/backup-token/",
            "/api/databases/12/backup-token",
            "/api/databases/metrics/",
            "/api/databases/metrics",
            "/api/compute/edge/",
            "/api/compute/edge",
            "/api/kubernetes/agent/",
            "/api/kubernetes/agent",
            "/api/runners/agent/",
            "/api/runners/agent",
            "/api/cdn/node/",
            "/api/cdn/node",
            "/api/objectstore/tokens/",
            "/api/objectstore/tokens",
        ];
        for &(path, ..) in ROUTES {
            for plain in [path.to_owned(), slashless(path)] {
                for suffix in SUFFIXES {
                    let form = suffixed(&plain, suffix);
                    for method in [Method::Get, Method::Post, Method::Put, Method::Delete] {
                        // The invite revoke's `{invite_id}` takes `accept.json` as an id, so
                        // a `DELETE` there is the revoke's, a scoped route: the stricter
                        // answer of the two forms.
                        let revoke =
                            method == Method::Delete && path == "/api/accounts/invites/accept/";
                        let (project, billing) = if revoke {
                            (Take::Refuse, Take::Refuse)
                        } else {
                            (project_take(method, &plain), billing_take(method, &plain))
                        };
                        let expected = if personal_under_the_prefix.contains(&path) {
                            Take::Send
                        } else {
                            project
                        };
                        assert_eq!(project_take(method, &form), expected, "{method} {form}");
                        assert_eq!(
                            billing_take(method, &form),
                            billing,
                            "billing {method} {form}"
                        );
                    }
                }
            }
        }
    }

    /// The S3 plan read takes the project with a suffix on `GET` only, and the recurring
    /// card routes name a billing account with a suffix as without one.
    #[test]
    fn a_format_suffix_keeps_the_selector_rules_of_the_plain_route() {
        let (project, billing) = (project(), billing());
        let read = "/api/s3/subscription.json/";
        assert_eq!(
            project_header(Some(&project), Method::Get, Access::Read, read).unwrap(),
            Some(PROJECT)
        );
        let error = project_header(Some(&project), Method::Post, Access::Write, read).unwrap_err();
        assert_eq!(error.code(), Code::ProjectScopeUnsupported);
        let error = project_header(
            Some(&project),
            Method::Get,
            Access::Read,
            "/api/objectstore/master-tokens.json",
        )
        .unwrap_err();
        assert_eq!(error.code(), Code::ProjectScopeUnsupported);
        assert_eq!(
            billing_header(
                Some(&billing),
                Method::Get,
                Access::Read,
                "/api/compute/usage.json"
            )
            .unwrap(),
            Some(BILLING)
        );
        for path in BILLING_NAMED_ONLY_PATHS {
            for suffix in SUFFIXES {
                assert!(
                    names_billing_only(Method::Get, &suffixed(path, suffix)),
                    "{path}"
                );
                assert!(
                    names_billing_only(Method::Get, &suffixed(&slashless(path), suffix)),
                    "{path}"
                );
            }
        }
        assert!(!names_billing_only(
            Method::Get,
            "/api/billing/stripe.json/"
        ));
        assert!(!names_billing_only(
            Method::Get,
            "/api/billing/stripe/active-subscription/x.json/"
        ));
    }

    /// Only a read names the project on the S3 plan reads: a `GET`, and a `HEAD`, which
    /// reaches the `GET` row, while every write is refused.
    #[test]
    fn the_s3_plan_reads_take_a_project_on_reads_only() {
        for path in ["/api/s3/plans/", "/api/s3/subscription/"] {
            for method in [Method::Get, Method::Head] {
                assert_eq!(project_take(method, path), Take::Send, "{method} {path}");
            }
            for method in [Method::Post, Method::Put, Method::Patch, Method::Delete] {
                assert_eq!(project_take(method, path), Take::Refuse, "{method} {path}");
            }
        }
    }

    #[test]
    fn the_recurring_card_routes_name_a_billing_account_only() {
        for path in BILLING_NAMED_ONLY_PATHS {
            assert!(names_billing_only(Method::Get, path), "{path}");
        }
        assert!(names_billing_only(
            Method::Get,
            "/api/billing/stripe/active-subscription?x=1"
        ));
        for path in [
            "/api/billing/stripe/",
            "/api/billing/credits/balance/",
            "/api/billing/stripe/active-subscription/x/",
        ] {
            assert!(!names_billing_only(Method::Get, path), "{path}");
        }
    }

    #[test]
    fn the_default_selection_sends_no_header_and_refuses_nothing() {
        for path in ["/api/compute/vms/", "/api/drive/subscription/"] {
            assert_eq!(
                project_header(None, Method::Get, Access::Read, path).unwrap(),
                None
            );
            assert_eq!(
                billing_header(None, Method::Get, Access::Read, path).unwrap(),
                None
            );
        }
    }

    #[test]
    fn a_selection_sends_skips_or_refuses_by_route() {
        let (project, billing) = (project(), billing());
        let vms = "/api/compute/vms/";
        assert_eq!(
            project_header(Some(&project), Method::Get, Access::Read, vms).unwrap(),
            Some(PROJECT)
        );
        assert_eq!(
            project_header(Some(&project), Method::Get, Access::Read, "/api/ssh-keys/").unwrap(),
            None
        );
        assert_eq!(
            billing_header(Some(&billing), Method::Get, Access::Read, "/api/ssh-keys/").unwrap(),
            None
        );
        let balance = "/api/billing/credits/balance/";
        assert_eq!(
            billing_header(Some(&billing), Method::Get, Access::Read, balance).unwrap(),
            Some(BILLING)
        );
        let error = project_header(Some(&project), Method::Get, Access::Read, balance).unwrap_err();
        assert_eq!(error.code(), Code::ProjectScopeUnsupported);
        assert_eq!(
            error.message(),
            format!(
                "`GET /api/billing/credits/balance/` does not take a project, and the selected \
                 project is `{PROJECT}`"
            )
        );
        assert_eq!(
            error.hint(),
            Some("pass `--project default` to act in the Default project")
        );
        let error = billing_header(Some(&billing), Method::Post, Access::Write, vms).unwrap_err();
        assert_eq!(error.code(), Code::BillingScopeUnsupported);
        assert_eq!(
            error.message(),
            format!(
                "`POST /api/compute/vms/` does not take a billing account, and the selected \
                 billing account is `{BILLING}`"
            )
        );
        assert_eq!(
            error.hint(),
            Some("pass `--billing default` to use the Default billing account")
        );
    }

    /// A selection from a flag or a variable is refused on a route that does not take it,
    /// and the message names where it came from (tenet 15).
    #[test]
    fn a_refusal_names_the_flag_or_variable_of_the_selection() {
        let (balance, vms) = ("/api/billing/credits/balance/", "/api/compute/vms/");
        for (source, from) in [
            (Source::Flag("--project"), "`--project`"),
            (Source::Variable("HIPPIUS_PROJECT"), "`HIPPIUS_PROJECT`"),
        ] {
            let project = ProjectId::parse(PROJECT, &source).unwrap().unwrap();
            let error =
                project_header(Some(&project), Method::Get, Access::Read, balance).unwrap_err();
            assert_eq!(
                error.message(),
                format!(
                    "`GET {balance}` does not take a project, and the selected project is \
                     `{PROJECT}` from {from}"
                )
            );
            assert_eq!(
                project_header(Some(&project), Method::Get, Access::Read, vms).unwrap(),
                Some(PROJECT)
            );
        }
        for (source, from) in [
            (Source::Flag("--billing"), "`--billing`"),
            (Source::Variable("HIPPIUS_BILLING"), "`HIPPIUS_BILLING`"),
        ] {
            let billing = BillingId::parse(BILLING, &source).unwrap().unwrap();
            let error = billing_header(Some(&billing), Method::Get, Access::Read, vms).unwrap_err();
            assert_eq!(
                error.message(),
                format!(
                    "`GET {vms}` does not take a billing account, and the selected billing \
                     account is `{BILLING}` from {from}"
                )
            );
            assert_eq!(
                billing_header(Some(&billing), Method::Get, Access::Read, balance).unwrap(),
                Some(BILLING)
            );
        }
    }

    /// Two ids are equal when they name the same project or billing account, wherever each
    /// came from: a grant compares them so (`hipc account members update`).
    #[test]
    fn ids_compare_by_value_not_by_source() {
        let flag = Source::Flag("--project");
        assert_eq!(ProjectId::parse(PROJECT, &flag).unwrap(), Some(project()));
        assert_ne!(
            ProjectId::parse("proj_ffffffffffffffffffff", &flag).unwrap(),
            Some(project())
        );
        assert_eq!(BillingId::parse(BILLING, &flag).unwrap(), Some(billing()));
        assert_ne!(
            BillingId::parse("bill_ffffffffffffffffffff", &flag).unwrap(),
            Some(billing())
        );
    }

    /// A stored selection goes on the routes that take it, and is left off a read of every
    /// other route instead of refusing it, so those reads keep working beside it; a stored
    /// project's write there is refused, naming the profile, a stored billing account is left
    /// off the write, and a personal write goes out without either.
    #[test]
    fn a_stored_selection_is_left_off_reads_and_refused_on_writes() {
        let stored = |field| Source::Profile {
            field,
            profile: "default".to_owned(),
            path: "/h/config.toml".into(),
        };
        let project = ProjectId::parse(PROJECT, &stored("project"))
            .unwrap()
            .unwrap();
        let billing = BillingId::parse(BILLING, &stored("billing"))
            .unwrap()
            .unwrap();
        let (balance, vms, keys) = (
            "/api/billing/credits/balance/",
            "/api/compute/vms/",
            "/api/ssh-keys/",
        );
        let project_on =
            |path| project_header(Some(&project), Method::Get, Access::Read, path).unwrap();
        let billing_on =
            |path| billing_header(Some(&billing), Method::Get, Access::Read, path).unwrap();
        assert_eq!(project_on(vms), Some(PROJECT));
        assert_eq!(project_on(balance), None);
        assert_eq!(project_on(keys), None);
        assert_eq!(billing_on(balance), Some(BILLING));
        assert_eq!(billing_on(vms), None);
        assert_eq!(billing_on(keys), None);
        assert_eq!(
            project_header(Some(&project), Method::Head, Access::Read, balance).unwrap(),
            None
        );
        let error =
            project_header(Some(&project), Method::Post, Access::Write, balance).unwrap_err();
        assert_eq!(
            error.message(),
            format!(
                "`POST {balance}` does not take a project, and the selected project is \
                 `{PROJECT}` from the `project` of the profile `default` in `/h/config.toml`; \
                 `hipc account projects use default` clears it"
            )
        );
        assert_eq!(
            error.hint(),
            Some("pass `--project default` to act in the Default project")
        );
        // A stored billing account is left off a write to a route that takes none, as on a
        // read; only a stored project is refused there.
        for method in [Method::Post, Method::Put, Method::Patch, Method::Delete] {
            assert_eq!(
                billing_header(Some(&billing), method, Access::Write, vms).unwrap(),
                None,
                "{method}"
            );
        }
        let error =
            project_header(Some(&project), Method::Delete, Access::Write, balance).unwrap_err();
        assert_eq!(error.code(), Code::ProjectScopeUnsupported);
        assert_eq!(
            project_header(Some(&project), Method::Post, Access::Write, keys).unwrap(),
            None
        );
        assert_eq!(
            billing_header(Some(&billing), Method::Post, Access::Write, keys).unwrap(),
            None
        );
    }

    /// A route that names one project in its path, such as its budget, acts on that project
    /// and not in the header's, so a stored project is left off every method there, writes
    /// included, and so is a stored billing account, which the route takes no more than a
    /// project; one from a flag or a variable still refuses it, as does the stored project on
    /// the project list and create, which name none.
    #[test]
    fn a_stored_project_is_left_off_a_route_that_names_the_project() {
        let stored = |field| Source::Profile {
            field,
            profile: "default".to_owned(),
            path: "/h/config.toml".into(),
        };
        let project = ProjectId::parse(PROJECT, &stored("project"))
            .unwrap()
            .unwrap();
        let billing = BillingId::parse(BILLING, &stored("billing"))
            .unwrap()
            .unwrap();
        let flag = ProjectId::parse(PROJECT, &Source::Flag("--project"))
            .unwrap()
            .unwrap();
        let billing_flag = BillingId::parse(BILLING, &Source::Flag("--billing"))
            .unwrap()
            .unwrap();
        let named = [
            format!("/api/accounts/projects/{PROJECT}/"),
            format!("/api/accounts/projects/{PROJECT}/budget/"),
            format!("/api/accounts/projects/{PROJECT}/quota"),
            format!("/api/accounts/projects/{PROJECT}/transfer/"),
            format!("/api/accounts/projects/{PROJECT}/budget.json/"),
            format!("/api/accounts/projects/{PROJECT}.json/"),
            format!("/api/accounts/projects/{PROJECT}"),
            format!("/api/accounts/projects/{PROJECT}/transfer.json"),
            // Another project than the stored one: the path still says which.
            "/api/accounts/projects/proj_ffffffffffffffffffff/quota/".to_owned(),
        ];
        for path in &named {
            for method in [Method::Put, Method::Patch, Method::Delete, Method::Post] {
                let stored = project_header(Some(&project), method, Access::Write, path);
                assert_eq!(stored.unwrap(), None, "{method} {path}");
                let error = project_header(Some(&flag), method, Access::Write, path);
                let code = error.unwrap_err().code();
                assert_eq!(code, Code::ProjectScopeUnsupported, "{method} {path}");
                let stored = billing_header(Some(&billing), method, Access::Write, path);
                assert_eq!(stored.unwrap(), None, "{method} {path}");
                let error = billing_header(Some(&billing_flag), method, Access::Write, path);
                let code = error.unwrap_err().code();
                assert_eq!(code, Code::BillingScopeUnsupported, "{method} {path}");
            }
            let read = project_header(Some(&flag), Method::Get, Access::Read, path);
            assert_eq!(read.unwrap_err().code(), Code::ProjectScopeUnsupported);
        }
        let refused = [
            "/api/accounts/projects/",
            "/api/accounts/projects//budget/",
            "/api/accounts/projects/bulk/",
            "/api/accounts/projects/bulk-archive",
            "/api/accounts/projects/proj_x/quota/",
            "/api/accounts/projects/proj_0a1b2c3d4e5f60718293/members/",
        ];
        for path in refused {
            let error = project_header(Some(&project), Method::Post, Access::Write, path);
            let code = error.unwrap_err().code();
            assert_eq!(code, Code::ProjectScopeUnsupported, "{path}");
        }
    }

    /// The route of one billing account acts on the billing account its path names, so a
    /// stored billing account is left off every method there, writes included, and so is a
    /// stored project; one from a flag or a variable still refuses it, and on the list and
    /// create, which name none, and on the costs and any other route under one.
    #[test]
    fn a_stored_billing_account_is_left_off_the_route_that_names_it() {
        let stored = |field| Source::Profile {
            field,
            profile: "default".to_owned(),
            path: "/h/config.toml".into(),
        };
        let billing = BillingId::parse(BILLING, &stored("billing"))
            .unwrap()
            .unwrap();
        let project = ProjectId::parse(PROJECT, &stored("project"))
            .unwrap()
            .unwrap();
        let flag = BillingId::parse(BILLING, &Source::Variable("HIPPIUS_BILLING"))
            .unwrap()
            .unwrap();
        let project_flag = ProjectId::parse(PROJECT, &Source::Variable("HIPPIUS_PROJECT"))
            .unwrap()
            .unwrap();
        let named = [
            format!("/api/accounts/billing-accounts/{BILLING}/"),
            format!("/api/accounts/billing-accounts/{BILLING}"),
            format!("/api/accounts/billing-accounts/{BILLING}.json/"),
            format!("/api/accounts/billing-accounts/{BILLING}.json"),
            format!("/api/accounts/billing-accounts/{BILLING}/?x=1"),
            // Another billing account than the stored one: the path still says which.
            "/api/accounts/billing-accounts/bill_ffffffffffffffffffff/".to_owned(),
        ];
        for path in &named {
            for method in [Method::Put, Method::Patch, Method::Delete, Method::Post] {
                let stored = billing_header(Some(&billing), method, Access::Write, path);
                assert_eq!(stored.unwrap(), None, "{method} {path}");
                let error = billing_header(Some(&flag), method, Access::Write, path);
                let code = error.unwrap_err().code();
                assert_eq!(code, Code::BillingScopeUnsupported, "{method} {path}");
                let stored = project_header(Some(&project), method, Access::Write, path);
                assert_eq!(stored.unwrap(), None, "{method} {path}");
                let error = project_header(Some(&project_flag), method, Access::Write, path);
                let code = error.unwrap_err().code();
                assert_eq!(code, Code::ProjectScopeUnsupported, "{method} {path}");
            }
            let read = billing_header(Some(&flag), Method::Get, Access::Read, path);
            assert_eq!(read.unwrap_err().code(), Code::BillingScopeUnsupported);
        }
        // The list and create name no billing account, and the costs and the other routes
        // under one take none: a stored one is left off them, as off any route that takes
        // none, and one from a flag or a variable is refused.
        let refused = [
            "/api/accounts/billing-accounts/",
            "/api/accounts/billing-accounts",
            "/api/accounts/billing-accounts//",
            "/api/accounts/billing-accounts/default/",
            "/api/accounts/billing-accounts/bill_x/",
            "/api/accounts/billing-accounts/bill_x.json/",
            "/api/accounts/billing-accounts/bill_9f8e7d6c5b4a39281706/costs/",
            "/api/accounts/billing-accounts/bill_9f8e7d6c5b4a39281706/x.json",
            "/api/accounts/projects/bill_9f8e7d6c5b4a39281706/",
        ];
        for path in refused {
            for method in [Method::Post, Method::Patch] {
                let stored = billing_header(Some(&billing), method, Access::Write, path);
                assert_eq!(stored.unwrap(), None, "{method} {path}");
                let error = billing_header(Some(&flag), method, Access::Write, path);
                let code = error.unwrap_err().code();
                assert_eq!(code, Code::BillingScopeUnsupported, "{method} {path}");
            }
        }
    }

    /// The refusal quotes the path with every query value redacted (tenet 9), and a backtick
    /// in it cannot close the literal early.
    #[test]
    fn the_refusal_quotes_no_query_value() {
        let error = project_header(
            Some(&project()),
            Method::Get,
            Access::Read,
            "/api/registry/me/?token=SEKRIT",
        )
        .unwrap_err();
        assert!(
            error
                .message()
                .starts_with("`GET /api/registry/me/?token=<redacted>` does not"),
            "{error}"
        );
        let error =
            billing_header(Some(&billing()), Method::Get, Access::Read, "/api/x`y/").unwrap_err();
        assert!(
            error.message().starts_with(r"`GET /api/x\u{60}y/` does"),
            "{error}"
        );
    }
}
