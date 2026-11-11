use super::*;
use crate::account::{BILLING_NAMED_ONLY_PATHS, PERSONAL_PATHS, listed_class};
use crate::project::{BILLING_PATHS, PROJECT_EXCLUDED_PATHS, listed_billing, listed_project};

/// A path that fills each placeholder of `endpoint` with a value the policy tables read as
/// its kind: a project or billing account id where the template names one, else digits,
/// which every `{name}` and `{n}` of the tables matches.
fn sample(endpoint: &Endpoint) -> String {
    endpoint.sample(|name| {
        match name {
            "project_id" => "proj_0a1b2c3d4e5f60718293",
            "billing_account_id" => "bill_9f8e7d6c5b4a39281706",
            "model" => "acme/llama",
            _ => "7",
        }
        .to_owned()
    })
}

/// What the header tables say of `path`, a path of `endpoint`, where they list it, side by
/// side with what the row says, as `(what, the row, the tables)`. A path the tables do not
/// list is the row's alone to decide, such as a new API version's.
fn listed_decisions(
    endpoint: &Endpoint,
    path: &str,
) -> Vec<(&'static str, String, Option<String>)> {
    let method = endpoint.method;
    vec![
        (
            "account class",
            format!("{:?}", endpoint.account()),
            listed_class(method, path).map(|class| format!("{class:?}")),
        ),
        (
            "account beside a billing account",
            endpoint.account_beside_billing.to_string(),
            BILLING_NAMED_ONLY_PATHS
                .contains(&path)
                .then(|| true.to_string()),
        ),
        (
            "project",
            format!("{:?}", endpoint.project),
            listed_project(method, path).map(|take| format!("{take:?}")),
        ),
        (
            "billing account",
            format!("{:?}", endpoint.billing),
            listed_billing(method, path).map(|take| format!("{take:?}")),
        ),
    ]
}

/// The paths the header tables name one by one, with no placeholder. A row whose
/// placeholder takes one of them decides it, so it must decide it as the tables do: were a
/// database row's `{database_id}` to take any segment, not digits alone, it would take
/// `/api/databases/metrics/`, a personal route.
fn literal_table_paths() -> impl Iterator<Item = &'static str> {
    PERSONAL_PATHS
        .iter()
        .chain(BILLING_NAMED_ONLY_PATHS)
        .chain(PROJECT_EXCLUDED_PATHS)
        .chain(BILLING_PATHS)
        .copied()
        .filter(|path| !path.contains('{'))
}

/// The header tables decide a request only when its path has no row, and they also decide
/// the other methods of a row's path, so where they list a path a row matches, its own or a
/// literal path a placeholder takes, they say what the row says: the account class, the
/// recurring card rule, and the project and billing handling. Each disagreement is listed,
/// so one run shows them all.
#[test]
fn the_header_tables_never_contradict_a_row() {
    let mut disagreements = Vec::new();
    for endpoint in all() {
        let own = sample(endpoint);
        let literal =
            literal_table_paths().filter(|path| row(endpoint.method, path) == Some(endpoint));
        let paths: Vec<String> = std::iter::once(own)
            .chain(literal.map(str::to_owned))
            .collect();
        for path in &paths {
            for (what, row, tables) in listed_decisions(endpoint, path) {
                if tables.as_ref().is_some_and(|tables| *tables != row) {
                    disagreements.push(format!(
                        "{} {path} ({}): {what} is {row} in the row and {} in the tables",
                        endpoint.method,
                        endpoint.template,
                        tables.unwrap_or_default()
                    ));
                }
            }
        }
    }
    assert!(disagreements.is_empty(), "{disagreements:#?}");
}

/// Each row is the row its own path reaches, so no two templates tie on a path and every
/// row's policy and paging apply to the requests it describes.
#[test]
fn every_row_is_reached_by_its_own_path() {
    for endpoint in all() {
        let path = sample(endpoint);
        let found = row(endpoint.method, &path).expect("a row matches its own path");
        assert!(
            found == endpoint,
            "{} {path} reaches {} instead of {}",
            endpoint.method,
            found.template,
            endpoint.template
        );
        // Slashless, as a server or a proxy that adds the slash delivers it.
        let slashless = path.strip_suffix('/').expect("a template ends with `/`");
        assert_eq!(row(endpoint.method, slashless), Some(found), "{slashless}");
    }
}

/// A literal segment beats a placeholder, as the server's router tries a literal path first,
/// a `HEAD` reaches the `GET` row, and a path or a method no row has reaches none.
#[test]
fn the_most_literal_row_wins_and_a_head_reaches_the_get() {
    let refusals = row(Method::Get, "/api/databases/refusals/").expect("a row");
    assert_eq!(refusals, databases::DATABASE_REFUSALS.endpoint());
    let one = row(Method::Get, "/api/databases/7/").expect("a row");
    assert_eq!(one, databases::DATABASE.endpoint());
    // A placeholder holds the shape its name gives: a project or billing account id, and a
    // model of two segments, so a version path is the version's.
    assert_eq!(row(Method::Patch, "/api/accounts/projects/bulk/"), None);
    assert_eq!(
        row(Method::Patch, "/api/accounts/projects/default/"),
        Some(account::PROJECT_UPDATE.endpoint())
    );
    assert_eq!(
        row(Method::Patch, "/api/accounts/billing-accounts/bill_x/"),
        None
    );
    assert_eq!(
        row(Method::Get, "/api/models/acme/llama/"),
        Some(hub::MODEL.endpoint())
    );
    assert_eq!(
        row(Method::Get, "/api/models/acme/llama/v1/"),
        Some(hub::MODEL_VERSION.endpoint())
    );
    assert_eq!(row(Method::Get, "/api/models/acme/"), None);
    assert_eq!(row(Method::Get, "/api/models/a/b/c/d/"), None);
    assert_eq!(
        row(Method::Head, "/api/network/me/"),
        Some(network::ME.endpoint())
    );
    assert_eq!(row(Method::Get, "/api/v2/compute/vms/"), None);
    assert_eq!(row(Method::Put, "/api/compute/vms/"), None);
    assert_eq!(
        matching("/api/compute/vms/")
            .map(|endpoint| endpoint.method)
            .collect::<Vec<_>>(),
        [Method::Get, Method::Post]
    );
}

/// A method and template appear in one row only, so a command and a test that name a route
/// cannot reach two policies.
#[test]
fn no_route_has_two_rows() {
    let mut seen = std::collections::BTreeSet::new();
    for endpoint in all() {
        assert!(
            seen.insert((endpoint.method.as_str(), endpoint.template)),
            "{} {} has two rows",
            endpoint.method,
            endpoint.template
        );
    }
}

#[test]
fn placeholders_fill_one_segment_each_percent_encoded() {
    const ONE: Route<1> = Route::get("/api/a/{id}/b/").own_account();
    const TWO: Route<2> = Route::delete("/api/r/{repository..}/t/{tag}/").own_account();
    assert_eq!(ONE.path([&7]), "/api/a/7/b/");
    assert_eq!(ONE.path([&"x/../y z"]), "/api/a/x%2F..%2Fy%20z/b/");
    assert_eq!(ONE.path([&"sha256:0a+b=c@d"]), "/api/a/sha256:0a+b=c@d/b/");
    assert_eq!(ONE.path([&"a;b?c#d%e"]), "/api/a/a%3Bb%3Fc%23d%25e/b/");
    assert_eq!(TWO.path([&"team/app", &"v1"]), "/api/r/team/app/t/v1/");
    assert_eq!(TWO.path([&"a b/c", &"x/y"]), "/api/r/a%20b/c/t/x%2Fy/");
    assert_eq!(TWO.path([&"a:b/c", &"x"]), "/api/r/a:b/c/t/x/");
    assert_eq!(ONE.method(), Method::Get);
    assert_eq!(ONE.endpoint().template(), "/api/a/{id}/b/");
}

#[test]
fn a_template_matches_paths_with_a_value_in_each_placeholder() {
    const TWO: Route<2> = Route::delete("/api/r/{repository..}/t/{tag}/").own_account();
    let endpoint = TWO.endpoint();
    for (path, matches) in [
        ("/api/r/app/t/v1/", true),
        ("/api/r/team/app/t/v1/", true),
        ("/api/r/t/v1/", false),
        ("/api/r//t/v1/", false),
        ("/api/r/app/t//", false),
        ("/api/r/app/t/v1/x/", false),
        ("/api/r/app/u/v1/", false),
        ("/api/r/app/t/v1", false),
    ] {
        assert_eq!(endpoint.matches(path), matches, "{path}");
    }
}

/// A path that ends before a one-segment placeholder is not the route.
#[test]
fn a_path_that_stops_before_a_placeholder_is_not_the_route() {
    const ONE: Route<1> = Route::get("/api/a/{id}/").own_account();
    let endpoint = ONE.endpoint();
    assert!(endpoint.matches("/api/a/7/"));
    assert!(!endpoint.matches("/api/a"));
}

#[test]
fn a_template_counts_only_whole_segment_placeholders() {
    assert_eq!(placeholders("/api/a/"), 0);
    assert_eq!(placeholders("/api/a/{id}/b/{n..}/"), 2);
    assert_eq!(placeholders("{id}/"), usize::MAX);
    assert_eq!(placeholders("/api/a{id}/"), usize::MAX);
    assert_eq!(placeholders("/api/{id}x/"), usize::MAX);
    assert_eq!(placeholders("/api/{id}"), usize::MAX);
    assert!(starts_with_base("/api/a/"));
    for template in ["/api/", "/apix/", "/api/a", "/ap/a/", "api/a/", "/xpi/a/"] {
        assert!(!starts_with_base(template), "{template}");
    }
}

/// Each builder method sets its one part of the policy, and a route starts refusing every
/// selector and needing nothing beyond `--yes`. Run, not only evaluated as constants, so each
/// method's body runs under the coverage gate.
#[test]
fn each_builder_method_sets_its_part_of_the_policy() {
    let plain = Route::<0>::get("/api/a/").own_account().declared();
    let endpoint = plain.endpoint();
    assert_eq!(endpoint.account, Some(RouteClass::OwnAccount));
    assert_eq!(
        (endpoint.project, endpoint.billing),
        (Take::Refuse, Take::Refuse)
    );
    assert!(!endpoint.reveals && !endpoint.provisions && !endpoint.permanent);
    assert!(
        !endpoint.undone_on_503 && endpoint.paging.is_none() && !endpoint.account_beside_billing
    );
    assert_eq!(endpoint.guard, Guard::None);

    let personal = Route::<0>::put("/api/a/").personal();
    assert_eq!(personal.endpoint().account, Some(RouteClass::Personal));
    assert_eq!(
        (personal.endpoint().project, personal.endpoint().billing),
        (Take::Skip, Take::Skip)
    );
    let named = Route::<1>::patch("/api/a/{id}/").scoped().names_one();
    assert_eq!(named.endpoint().account, Some(RouteClass::Scoped));
    assert_eq!(
        (named.endpoint().project, named.endpoint().billing),
        (Take::Named, Take::Named)
    );
    let flagged = Route::<0>::post("/api/a/")
        .own_account()
        .beside_billing()
        .project()
        .billing()
        .reveals()
        .provisions()
        .permanent()
        .undone_on_503()
        .paged(Paging::Cursor)
        .guard(Guard::Refused {
            why: "why",
            hint: "hint",
        });
    let endpoint = flagged.endpoint();
    assert_eq!(
        (endpoint.project, endpoint.billing),
        (Take::Send, Take::Send)
    );
    assert!(endpoint.reveals && endpoint.provisions && endpoint.permanent);
    assert!(
        endpoint.undone_on_503 && endpoint.paging().is_some() && endpoint.account_beside_billing
    );
    assert_eq!(endpoint.paging(), Some(Paging::Cursor));
    assert_eq!(
        endpoint.guard(),
        Guard::Refused {
            why: "why",
            hint: "hint"
        }
    );
    assert!(endpoint.reveals() && endpoint.provisions());
    assert!(endpoint.permanent() && endpoint.undone_on_503());
    assert!(!Guard::None.lends() && !Guard::Distinct.lends());
    assert!(Guard::Needs(&[RouteFlag::Public]).lends());
    assert_eq!(Route::<0>::delete("/api/a/").method(), Method::Delete);
}

/// A route that never says its class is refused, which stops the build of a constant.
#[test]
#[should_panic(expected = "a route says whether it is scoped")]
fn a_route_without_a_class_is_refused() {
    let _unclassed = Route::<0>::get("/api/a/").declared();
}

/// A template whose placeholders the type does not count, or that is outside the base, is
/// refused.
#[test]
#[should_panic(expected = "a route's type counts the placeholders of its template")]
fn a_route_with_the_wrong_count_is_refused() {
    let _miscounted = Route::<0>::get("/api/a/{id}/");
}

#[test]
#[should_panic(expected = "a route starts with `/api/`")]
fn a_route_outside_the_base_is_refused() {
    let _outside = Route::<0>::get("/v2/a/");
}

/// A write that needs no flag says so with `Guard::Distinct` or `Guard::None`, so an empty
/// `Guard::Needs` stops the build of a constant.
#[test]
#[should_panic(expected = "a `Guard::Needs` names a flag")]
fn a_guard_that_needs_no_flag_is_refused() {
    let _needs_nothing = Route::<0>::post("/api/a/").guard(Guard::Needs(&[]));
}

/// Each paging style names the queries its pages are asked for with, and says whether its
/// pages are numbered.
#[test]
fn each_paging_style_names_its_queries() {
    let styles = [
        (Paging::Numbered, true, "page", Some("page_size")),
        (
            Paging::Totalled { items: "artifacts" },
            true,
            "page",
            Some("page_size"),
        ),
        (Paging::Cursor, false, "before", None),
        (
            Paging::Continuation,
            false,
            "continuation_token",
            Some("max_keys"),
        ),
    ];
    for (paging, numbered, token, size) in styles {
        assert_eq!(
            (paging.numbered(), paging.token_query(), paging.size_query()),
            (numbered, token, size),
            "{paging:?}"
        );
    }
}

/// Every row's method and path, written out as text rather than read from the constants, in
/// the order of [`all`]: a route whose path or method changes by mistake, a typo or a
/// template edited during a version move, fails here as well as in the `hipc schema`
/// snapshot, so a changed route is an edit in two places, one of them this list.
const WRITTEN_OUT: &[(&str, &str)] = &[
    // account
    ("GET", "/api/accounts/"),
    ("GET", "/api/users/me/"),
    ("GET", "/api/accounts/members/"),
    ("PATCH", "/api/accounts/members/{member_id}/"),
    ("DELETE", "/api/accounts/members/{member_id}/"),
    ("GET", "/api/accounts/invites/"),
    ("POST", "/api/accounts/invites/"),
    ("DELETE", "/api/accounts/invites/{invite_id}/"),
    ("POST", "/api/accounts/invites/preview/"),
    ("POST", "/api/accounts/invites/accept/"),
    ("GET", "/api/accounts/projects/"),
    ("POST", "/api/accounts/projects/"),
    ("PATCH", "/api/accounts/projects/{project_id}/"),
    ("DELETE", "/api/accounts/projects/{project_id}/"),
    ("GET", "/api/accounts/projects/{project_id}/budget/"),
    ("PUT", "/api/accounts/projects/{project_id}/budget/"),
    ("DELETE", "/api/accounts/projects/{project_id}/budget/"),
    ("GET", "/api/accounts/projects/{project_id}/quota/"),
    ("PATCH", "/api/accounts/projects/{project_id}/quota/"),
    ("POST", "/api/accounts/projects/{project_id}/transfer/"),
    ("GET", "/api/accounts/project-transfers/"),
    (
        "POST",
        "/api/accounts/project-transfers/{transfer_id}/accept/",
    ),
    (
        "POST",
        "/api/accounts/project-transfers/{transfer_id}/decline/",
    ),
    ("DELETE", "/api/accounts/project-transfers/{transfer_id}/"),
    ("GET", "/api/accounts/billing-accounts/"),
    ("POST", "/api/accounts/billing-accounts/"),
    (
        "PATCH",
        "/api/accounts/billing-accounts/{billing_account_id}/",
    ),
    (
        "GET",
        "/api/accounts/billing-accounts/{billing_account_id}/costs/",
    ),
    ("GET", "/api/notifications/settings/"),
    ("PATCH", "/api/notifications/settings/"),
    ("GET", "/api/notifications/marketing-consent/"),
    ("POST", "/api/notifications/marketing-consent/"),
    ("GET", "/api/referral-codes/"),
    ("POST", "/api/referral-codes/"),
    ("PATCH", "/api/referral-codes/{referral_id}/"),
    ("GET", "/api/referral/analytics/"),
    ("POST", "/api/account/deletion/"),
    ("DELETE", "/api/account/deletion/"),
    // billing
    ("GET", "/api/billing/credits/balance/"),
    ("GET", "/api/billing/transactions/"),
    ("GET", "/api/billing/invoices/"),
    ("GET", "/api/billing/invoices/{invoice_id}/pdf/"),
    ("GET", "/api/billing/profile/"),
    ("PUT", "/api/billing/profile/"),
    ("GET", "/api/billing/auto-reload/"),
    ("PUT", "/api/billing/auto-reload/"),
    ("GET", "/api/billing/auto-reload/history/"),
    ("POST", "/api/billing/auto-reload/card-setup/"),
    ("GET", "/api/billing/stripe/active-subscription/"),
    ("POST", "/api/billing/stripe/create-checkout-session/"),
    ("POST", "/api/billing/btcpay/create-invoice/"),
    ("GET", "/api/billing/native/quote/"),
    ("POST", "/api/billing/native/buy/"),
    ("GET", "/api/billing/native/purchases/"),
    ("GET", "/api/billing/gamma/"),
    ("GET", "/api/billing/gamma/conversions/"),
    ("GET", "/api/billing/gamma/conversions/{conversion_id}/"),
    ("POST", "/api/billing/gamma/conversions/"),
    ("POST", "/api/billing/gamma/hotkeys/challenge/"),
    ("POST", "/api/billing/gamma/hotkeys/"),
    ("DELETE", "/api/billing/gamma/hotkeys/{hotkey}/"),
    // cdn
    ("GET", "/api/cdn/prices/"),
    ("GET", "/api/cdn/regions/"),
    ("POST", "/api/cdn/estimate/"),
    ("GET", "/api/cdn/zones/"),
    ("POST", "/api/cdn/zones/"),
    ("GET", "/api/cdn/zones/{zone_id}/"),
    ("PATCH", "/api/cdn/zones/{zone_id}/"),
    ("DELETE", "/api/cdn/zones/{zone_id}/"),
    ("GET", "/api/cdn/zones/{zone_id}/cap/"),
    ("PATCH", "/api/cdn/zones/{zone_id}/cap/"),
    ("GET", "/api/cdn/zones/{zone_id}/purges/"),
    ("GET", "/api/cdn/zones/{zone_id}/purges/{purge_id}/"),
    ("POST", "/api/cdn/zones/{zone_id}/purge/"),
    ("GET", "/api/cdn/zones/{zone_id}/rules/"),
    ("PUT", "/api/cdn/zones/{zone_id}/rules/"),
    ("GET", "/api/cdn/zones/{zone_id}/stats/"),
    ("GET", "/api/cdn/zones/{zone_id}/hostnames/"),
    ("POST", "/api/cdn/zones/{zone_id}/hostnames/"),
    ("DELETE", "/api/cdn/zones/{zone_id}/hostnames/{hostname}/"),
    (
        "POST",
        "/api/cdn/zones/{zone_id}/hostnames/{hostname}/verify/",
    ),
    (
        "GET",
        "/api/cdn/zones/{zone_id}/hostnames/{hostname}/domain-connect/",
    ),
    (
        "POST",
        "/api/cdn/zones/{zone_id}/hostnames/{hostname}/dns-setup/",
    ),
    ("GET", "/api/cdn/zones/{zone_id}/waf/"),
    ("PATCH", "/api/cdn/zones/{zone_id}/waf/"),
    ("GET", "/api/cdn/zones/{zone_id}/waf/status/"),
    ("GET", "/api/cdn/zones/{zone_id}/waf/analytics/"),
    ("GET", "/api/cdn/zones/{zone_id}/waf/events/"),
    ("GET", "/api/cdn/zones/{zone_id}/waf/rules/"),
    ("PUT", "/api/cdn/zones/{zone_id}/waf/rules/"),
    ("GET", "/api/cdn/zones/{zone_id}/waf/rate-limits/"),
    ("PUT", "/api/cdn/zones/{zone_id}/waf/rate-limits/"),
    ("POST", "/api/cdn/zones/{zone_id}/waf/test/"),
    ("GET", "/api/cdn/zones/{zone_id}/waf/versions/"),
    ("GET", "/api/cdn/zones/{zone_id}/waf/versions/{version}/"),
    (
        "POST",
        "/api/cdn/zones/{zone_id}/waf/versions/{version}/rollback/",
    ),
    ("GET", "/api/cdn/lists/"),
    ("POST", "/api/cdn/lists/"),
    ("GET", "/api/cdn/lists/{list_name}/"),
    ("PATCH", "/api/cdn/lists/{list_name}/"),
    ("DELETE", "/api/cdn/lists/{list_name}/"),
    ("POST", "/api/cdn/lists/{list_name}/items/"),
    ("DELETE", "/api/cdn/lists/{list_name}/items/"),
    ("PUT", "/api/cdn/lists/{list_name}/items/"),
    // compute
    ("GET", "/api/compute/prices/"),
    ("GET", "/api/compute/public-prices/"),
    ("GET", "/api/compute/quota/"),
    ("GET", "/api/compute/charge-cap/"),
    ("GET", "/api/compute/flavors/"),
    ("GET", "/api/compute/images/"),
    ("GET", "/api/compute/apps/"),
    ("GET", "/api/compute/regions/"),
    ("GET", "/api/compute/placement-groups/"),
    ("GET", "/api/compute/feasibility/"),
    ("GET", "/api/compute/account-ssh-keys/"),
    ("GET", "/api/compute/backup-options/"),
    ("GET", "/api/compute/public-ip/availability/"),
    ("GET", "/api/compute/usage/"),
    ("GET", "/api/compute/billing/summary/"),
    ("GET", "/api/compute/vms/"),
    ("POST", "/api/compute/vms/"),
    ("GET", "/api/compute/vms/{vm_id}/"),
    ("DELETE", "/api/compute/vms/{vm_id}/"),
    ("POST", "/api/compute/vms/{vm_id}/start/"),
    ("POST", "/api/compute/vms/{vm_id}/stop/"),
    ("POST", "/api/compute/vms/{vm_id}/reboot/"),
    ("POST", "/api/compute/vms/{vm_id}/resize/"),
    ("GET", "/api/compute/vms/{vm_id}/resize/"),
    ("GET", "/api/compute/vms/{vm_id}/resize/{resize_id}/"),
    ("GET", "/api/compute/vms/{vm_id}/attestation/"),
    ("GET", "/api/compute/vms/{vm_id}/failovers/"),
    ("PUT", "/api/compute/vms/{vm_id}/power-policy/"),
    ("GET", "/api/compute/vms/{vm_id}/public-ip/"),
    ("POST", "/api/compute/vms/{vm_id}/public-ip/"),
    ("DELETE", "/api/compute/vms/{vm_id}/public-ip/"),
    ("GET", "/api/compute/vms/{vm_id}/public-ip/usage/"),
    ("GET", "/api/compute/vms/{vm_id}/firewall/"),
    ("PUT", "/api/compute/vms/{vm_id}/firewall/"),
    ("GET", "/api/compute/vms/{vm_id}/expose/"),
    ("POST", "/api/compute/vms/{vm_id}/expose/"),
    ("DELETE", "/api/compute/vms/{vm_id}/expose/{exposure_id}/"),
    ("GET", "/api/compute/vms/{vm_id}/backup/"),
    ("PUT", "/api/compute/vms/{vm_id}/backup/"),
    ("DELETE", "/api/compute/vms/{vm_id}/backup/"),
    ("GET", "/api/compute/vms/{vm_id}/backups/"),
    ("POST", "/api/compute/vms/{vm_id}/backups/restore/"),
    ("GET", "/api/compute/vms/{vm_id}/backups/restore/"),
    ("GET", "/api/ssh-keys/"),
    ("GET", "/api/ssh-keys/{key_id}/"),
    ("POST", "/api/ssh-keys/"),
    ("DELETE", "/api/ssh-keys/{key_id}/"),
    ("GET", "/api/compute/app-engine/info/"),
    ("GET", "/api/compute/app-engine/apps/"),
    ("POST", "/api/compute/app-engine/apps/"),
    ("GET", "/api/compute/app-engine/apps/{vm_id}/"),
    ("GET", "/api/compute/app-engine/apps/{vm_id}/runtime/"),
    (
        "GET",
        "/api/compute/app-engine/apps/{vm_id}/guest-components/",
    ),
    ("GET", "/api/compute/app-engine/apps/{vm_id}/guest-upgrade/"),
    ("POST", "/api/compute/app-engine/validate/"),
    ("POST", "/api/compute/app-engine/fix/"),
    ("PUT", "/api/compute/app-engine/apps/{vm_id}/compose/"),
    // databases
    ("GET", "/api/databases/"),
    ("POST", "/api/databases/"),
    ("GET", "/api/databases/{database_id}/"),
    ("DELETE", "/api/databases/{database_id}/"),
    ("GET", "/api/databases/{database_id}/events/"),
    ("GET", "/api/databases/{database_id}/metrics/"),
    ("POST", "/api/databases/{database_id}/restart/"),
    ("POST", "/api/databases/{database_id}/restore/"),
    ("POST", "/api/databases/{database_id}/expose/"),
    ("DELETE", "/api/databases/{database_id}/expose/"),
    ("GET", "/api/databases/refusals/"),
    // drive
    ("GET", "/api/drive/plans/"),
    ("GET", "/api/drive/subscription/"),
    ("POST", "/api/drive/subscription/"),
    ("PATCH", "/api/drive/subscription/"),
    ("DELETE", "/api/drive/subscription/"),
    ("GET", "/api/drive/subscription/checkout/{intent_id}/"),
    ("GET", "/api/drive/subscription/history/"),
    // hub
    ("GET", "/api/registry/plans/"),
    ("GET", "/api/registry/subscriptions/"),
    ("POST", "/api/registry/subscribe/"),
    ("DELETE", "/api/registry/subscriptions/{subscription_id}/"),
    ("GET", "/api/registry/usage/"),
    ("GET", "/api/registry/usage/repositories/"),
    ("GET", "/api/registry/namespaces/check/"),
    ("POST", "/api/registry/provision/"),
    ("GET", "/api/registry/provision/status/"),
    ("GET", "/api/registry/me/"),
    ("PATCH", "/api/registry/me/publicity/"),
    ("GET", "/api/registry/me/robot/"),
    ("POST", "/api/registry/robot/rotate/"),
    ("GET", "/api/registry/keys/"),
    ("GET", "/api/registry/keys/{key_id}/"),
    ("POST", "/api/registry/keys/"),
    ("POST", "/api/registry/keys/{key_id}/rotate/"),
    ("DELETE", "/api/registry/keys/{key_id}/"),
    ("GET", "/api/registry/events/"),
    ("GET", "/api/registry/repositories/"),
    ("DELETE", "/api/registry/repositories/{repository..}/"),
    (
        "GET",
        "/api/registry/repositories/{repository..}/artifacts/",
    ),
    (
        "GET",
        "/api/registry/repositories/{repository..}/artifacts/{reference}/",
    ),
    (
        "DELETE",
        "/api/registry/repositories/{repository..}/artifacts/{reference}/",
    ),
    ("GET", "/api/models/"),
    ("GET", "/api/models/formats/"),
    ("GET", "/api/models/{model..}/"),
    ("GET", "/api/models/{model..}/{reference}/"),
    // kubernetes
    ("GET", "/api/kubernetes/clusters/"),
    ("GET", "/api/kubernetes/clusters/{cluster_id}/"),
    ("DELETE", "/api/kubernetes/clusters/{cluster_id}/"),
    ("GET", "/api/kubernetes/clusters/{cluster_id}/events/"),
    ("GET", "/api/kubernetes/clusters/{cluster_id}/secrets/"),
    ("GET", "/api/kubernetes/clusters/refusals/"),
    ("GET", "/api/kubernetes/clusters/{cluster_id}/backups/"),
    ("GET", "/api/kubernetes/clusters/{cluster_id}/node-pools/"),
    ("POST", "/api/kubernetes/clusters/{cluster_id}/node-pools/"),
    (
        "POST",
        "/api/kubernetes/clusters/{cluster_id}/node-pools/{pool_id}/scale/",
    ),
    (
        "POST",
        "/api/kubernetes/clusters/{cluster_id}/nodes/{node_name}/remove/",
    ),
    (
        "POST",
        "/api/kubernetes/clusters/{cluster_id}/nodes/{node_name}/replace/",
    ),
    ("GET", "/api/kubernetes/clusters/{cluster_id}/operations/"),
    (
        "POST",
        "/api/kubernetes/clusters/{cluster_id}/operations/{operation_id}/cancel/",
    ),
    (
        "POST",
        "/api/kubernetes/clusters/{cluster_id}/operations/{operation_id}/resume/",
    ),
    // network
    ("GET", "/api/network/me/"),
    ("POST", "/api/network/provision/"),
    ("GET", "/api/network/groups/"),
    ("GET", "/api/network/policies/"),
    ("GET", "/api/network/peers/"),
    ("GET", "/api/network/peers/{peer_id}/"),
    ("PATCH", "/api/network/peers/{peer_id}/"),
    ("DELETE", "/api/network/peers/{peer_id}/"),
    ("GET", "/api/network/networks/"),
    ("GET", "/api/network/networks/{vpc_id}/resources/"),
    ("GET", "/api/network/networks/{vpc_id}/routers/"),
    ("POST", "/api/network/isolation/"),
    // operator
    ("PUT", "/api/v1/operator/nodes/{node_id}/storage/"),
    // runners
    ("POST", "/api/runners/github/connect/"),
    ("GET", "/api/runners/installations/"),
    ("DELETE", "/api/runners/installations/{installation_id}/"),
    ("GET", "/api/runners/jobs/"),
    ("GET", "/api/runners/limit/"),
    ("PUT", "/api/runners/limit/"),
    ("GET", "/api/runners/settings/"),
    ("PUT", "/api/runners/settings/"),
    ("GET", "/api/runners/usage/"),
    // s3
    ("GET", "/api/s3/plans/"),
    ("GET", "/api/s3/subscription/"),
    ("POST", "/api/s3/subscription/"),
    ("PATCH", "/api/s3/subscription/"),
    ("DELETE", "/api/s3/subscription/"),
    ("GET", "/api/s3/subscription/checkout/{intent_id}/"),
    ("GET", "/api/s3/subscription/history/"),
    ("GET", "/api/objectstore/buckets/"),
    ("POST", "/api/objectstore/buckets/"),
    ("DELETE", "/api/objectstore/buckets/{bucket_name}/"),
    ("GET", "/api/objectstore/buckets/{bucket_name}/objects/"),
    (
        "GET",
        "/api/objectstore/buckets/{bucket_name}/presigned-url/",
    ),
    ("GET", "/api/objectstore/acl/"),
    ("GET", "/api/objectstore/buckets/{bucket_name}/acl/"),
    ("PUT", "/api/objectstore/buckets/{bucket_name}/acl/set/"),
    ("GET", "/api/objectstore/master-tokens/"),
    ("POST", "/api/objectstore/master-tokens/"),
    ("POST", "/api/objectstore/master-tokens/{token_id}/rotate/"),
    ("POST", "/api/objectstore/master-tokens/{token_id}/revoke/"),
    ("GET", "/api/objectstore/sub-tokens/"),
    ("POST", "/api/objectstore/sub-tokens/"),
    ("POST", "/api/objectstore/sub-tokens/{token_id}/rotate/"),
    ("POST", "/api/objectstore/sub-tokens/{token_id}/revoke/"),
    // support
    ("GET", "/api/support/tickets/"),
    ("POST", "/api/support/tickets/"),
    ("GET", "/api/support/tickets/{ticket_id}/"),
    ("PATCH", "/api/support/tickets/{ticket_id}/"),
    ("GET", "/api/support/tickets/{ticket_id}/messages/"),
    ("POST", "/api/support/tickets/{ticket_id}/messages/"),
    ("GET", "/api/services/status/"),
];

#[test]
fn every_row_is_the_route_written_out_by_hand() {
    let rows: Vec<(&str, &str)> = all()
        .map(|endpoint| (endpoint.method.as_str(), endpoint.template))
        .collect();
    assert_eq!(rows, WRITTEN_OUT);
}
