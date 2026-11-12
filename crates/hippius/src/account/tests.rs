use super::*;
use crate::error::Code;
use crate::routes::RouteFlag;
use crate::source::Source;

const ID: &str = "acct_0123456789abcdef0123";

fn member() -> Selection {
    Selection::parse(ID, &Source::Argument).unwrap()
}

#[test]
fn parses_self_and_ids_and_rejects_the_rest() {
    assert!(matches!(
        Selection::parse("self", &Source::Argument).unwrap(),
        Selection::Own
    ));
    assert_eq!(member().as_str(), ID);
    for bad in [
        "",
        "acct_",
        "acct_0123456789ABCDEF0123",
        "acct_0123456789abcdef012",
        "acct_0123456789abcdef01234",
        "user_0123456789abcdef0123",
        "acct_0123456789abcdef012g",
    ] {
        let error = Selection::parse(bad, &Source::Argument).unwrap_err();
        assert_eq!(error.code().as_str(), "usage", "{bad}");
    }
}

/// Every route family `hipc` sends to, with the class the console's header rules give
/// it, written out by hand: the e2e harness takes the header it expects from
/// [`header`] itself, so this table is where a wrong classification fails.
const PLAN_ROUTES: &[(&str, RouteClass)] = {
    use RouteClass::{OwnAccount, Personal, Scoped};
    &[
        ("/api/accounts/", Personal),
        ("/api/accounts/members/", Scoped),
        ("/api/accounts/members/7/", Scoped),
        ("/api/accounts/invites/accept/", Personal),
        ("/api/accounts/invites/preview/", Personal),
        ("/api/accounts/invites/", Scoped),
        ("/api/accounts/invites/3/", Scoped),
        ("/api/accounts/project-transfers/1/", Personal),
        ("/api/compute/vms/", Scoped),
        ("/api/compute/vms/x/attestation/", Scoped),
        ("/api/compute/placement-groups/", Scoped),
        ("/api/compute/vms/v1/resize/", Scoped),
        ("/api/compute/vms/v1/resize/7/", Scoped),
        ("/api/compute/public-ip/availability/", Scoped),
        ("/api/compute/vms/v1/public-ip/", Scoped),
        ("/api/compute/vms/v1/public-ip/usage/", Scoped),
        ("/api/compute/vms/v1/firewall/", Scoped),
        ("/api/compute/vms/v1/expose/", Scoped),
        ("/api/compute/vms/v1/expose/7/", Scoped),
        ("/api/compute/backup-options/", Scoped),
        ("/api/compute/vms/v1/backup/", Scoped),
        ("/api/compute/vms/v1/backups/", Scoped),
        ("/api/compute/vms/v1/backups/restore/", Scoped),
        ("/api/compute/charge-cap/", Scoped),
        ("/api/compute/edge/x/", Personal),
        ("/api/databases/", Scoped),
        ("/api/databases/metrics/", Personal),
        ("/api/databases/12/backup-token/", Personal),
        ("/api/databases/12/members/", Personal),
        ("/api/databases/12/restart/", Scoped),
        ("/api/databases/12/", Scoped),
        ("/api/databases/12/events/", Scoped),
        ("/api/databases/12/expose/", Scoped),
        ("/api/databases/12/restore/", Scoped),
        ("/api/databases/refusals/", Scoped),
        ("/api/databases/x/members/", Scoped),
        ("/api/kubernetes/agent/a/", Personal),
        ("/api/kubernetes/clusters/", Scoped),
        ("/api/runners/agent/a/", Personal),
        ("/api/runners/", Scoped),
        ("/api/runners/settings/", Scoped),
        ("/api/runners/installations/", Scoped),
        ("/api/runners/installations/7/", Scoped),
        ("/api/runners/jobs/", Scoped),
        ("/api/runners/jobs/?before=812", Scoped),
        ("/api/runners/github/connect/", Scoped),
        ("/api/cdn/node/a/", Personal),
        ("/api/cdn/zones/", Scoped),
        ("/api/network/infra/a/", Personal),
        ("/api/network/", Scoped),
        ("/api/objectstore/buckets/", Scoped),
        ("/api/objectstore/master-tokens/", Scoped),
        ("/api/objectstore/tokens/x/", Personal),
        ("/api/s3/subscription/", Scoped),
        ("/api/s3/plans/", Scoped),
        ("/api/s3/plans/accounts/", Personal),
        ("/api/drive/plans/", Personal),
        ("/api/registry/plans/", Personal),
        ("/api/billing/credits/balance/", Scoped),
        ("/api/billing/invoices/", Scoped),
        ("/api/billing/profile/", Scoped),
        ("/api/billing/stripe/create-checkout-session/", Scoped),
        ("/api/billing/btcpay/create-invoice/", Scoped),
        ("/api/billing/auto-reload/history/", Scoped),
        ("/api/ssh-keys/", Personal),
        ("/api/ssh-keys/3/", Personal),
        // The customer ticket routes, and the two under them left to `hipc api`.
        ("/api/support/tickets/", Personal),
        ("/api/support/tickets/?status=open&page_size=100", Personal),
        ("/api/support/tickets/7/", Personal),
        ("/api/support/tickets/7/messages/", Personal),
        ("/api/support/tickets/7/handoff/", Personal),
        ("/api/support/tickets/7/messages/3/attachments/", Personal),
        // The referral codes and analytics are the signed-in user's.
        ("/api/referral-codes/", Personal),
        ("/api/referral-codes/4/", Personal),
        ("/api/referral/analytics/", Personal),
        ("/api/referral/analytics/?refresh=true", Personal),
        ("/api/referral/", OwnAccount),
        // Only the ticket routes are personal: the chat routes, and the routes the customer
        // console does not call, stay refused.
        ("/api/support/chat/inbox/", OwnAccount),
        ("/api/support/", OwnAccount),
        ("/api/drive/subscription/", OwnAccount),
        ("/api/registry/subscribe/", OwnAccount),
        ("/api/billing/transactions/", OwnAccount),
        ("/api/billing/stripe/active-subscription/", OwnAccount),
        ("/api/services/status/", OwnAccount),
        // The rest of the route families, one by one.
        ("/api/accounts/audit/", Scoped),
        ("/api/compute/account-ssh-keys/", Scoped),
        ("/api/compute/flavors/", Scoped),
        ("/api/compute/images/", Scoped),
        ("/api/compute/prices/", Scoped),
        ("/api/compute/quota/", Scoped),
        ("/api/compute/regions/", Scoped),
        ("/api/compute/vms/v1/start/", Scoped),
        ("/api/compute/vms/v1/failovers/", Scoped),
        ("/api/compute/apps/", Scoped),
        ("/api/compute/feasibility/?flavor=small&region=FR", Scoped),
        ("/api/databases/12/metrics/", Scoped),
        ("/api/objectstore/buckets/b/acl/", Scoped),
        ("/api/cdn/prices/", Scoped),
        ("/api/cdn/regions/", Scoped),
        ("/api/cdn/estimate/", Scoped),
        ("/api/cdn/zones/z1/", Scoped),
        ("/api/cdn/zones/z1/cap/", Scoped),
        ("/api/cdn/zones/z1/hostnames/", Scoped),
        ("/api/cdn/zones/z1/purge/", Scoped),
        ("/api/cdn/zones/z1/purges/", Scoped),
        ("/api/cdn/zones/z1/rules/", Scoped),
        ("/api/cdn/zones/z1/stats/", Scoped),
        ("/api/objectstore/buckets/b/", Scoped),
        ("/api/objectstore/buckets/b/objects/", Scoped),
        ("/api/objectstore/sub-tokens/", Scoped),
        ("/api/objectstore/sub-tokens/7/rotate/", Scoped),
        ("/api/objectstore/master-tokens/7/revoke/", Scoped),
        ("/api/s3/subscription/checkout/pi_1/", Scoped),
        ("/api/runners/github/link/", Scoped),
        // The billing reads. The console's header rules scope all but the Drive
        // history.
        ("/api/compute/billing/summary/", Scoped),
        ("/api/compute/usage/", Scoped),
        ("/api/compute/usage/?month=2026-09", Scoped),
        ("/api/billing/invoices/?page_size=100", Scoped),
        ("/api/billing/invoices/12/pdf/", Scoped),
        ("/api/billing/auto-reload/", Scoped),
        ("/api/s3/subscription/history/", Scoped),
        ("/api/drive/subscription/history/", OwnAccount),
        ("/api/drive/", OwnAccount),
        ("/api/drive/subscription/checkout/pi_1/", OwnAccount),
        ("/api/registry/", OwnAccount),
        ("/api/registry/me/", OwnAccount),
        ("/api/registry/namespaces/check/?name=n", OwnAccount),
        ("/api/registry/provision/", OwnAccount),
        ("/api/registry/provision/status/", OwnAccount),
        ("/api/registry/robot/rotate/", OwnAccount),
        ("/api/registry/keys/", OwnAccount),
        ("/api/registry/keys/7/rotate/", OwnAccount),
        ("/api/registry/subscriptions/", OwnAccount),
        ("/api/registry/subscriptions/7/", OwnAccount),
        ("/api/billing/stripe/", OwnAccount),
        ("/api/infrastructure/vm/", OwnAccount),
        ("/api/v1/operator/", OwnAccount),
        ("/api/accounts/?x=1", Personal),
        // The signed-in user, their notification settings, and their marketing consent.
        ("/api/users/me/", Personal),
        ("/api/notifications/settings/", Personal),
        ("/api/notifications/marketing-consent/", Personal),
        ("/api/users/", OwnAccount),
        ("/api/users/me/x/", OwnAccount),
        ("/api/notifications/", OwnAccount),
        ("/api/notifications/transactional/", OwnAccount),
    ]
};

/// The personal paths a scoped prefix holds. With a format suffix the written form
/// matches the prefix and no longer the personal path, so the stricter class is scoped:
/// the header goes out, which a personal route ignores.
const PERSONAL_UNDER_A_SCOPED_PREFIX: &[&str] = &[
    "/api/accounts/invites/accept/",
    "/api/accounts/invites/preview/",
    "/api/databases/metrics/",
    "/api/databases/12/backup-token/",
    "/api/databases/12/members/",
    "/api/s3/plans/accounts/",
];

/// Each row of [`PLAN_ROUTES`] with a format suffix, with and without its trailing `/`,
/// is refused with a membership exactly when the plain path is, for every method, and
/// gets the plain path's class apart from [`PERSONAL_UNDER_A_SCOPED_PREFIX`].
#[test]
fn a_format_suffix_keeps_the_class_of_every_plan_route() {
    let member = member();
    for &(path, _) in PLAN_ROUTES {
        let slashless = slashless(path);
        for plain in [path, slashless.as_str()] {
            for suffix in SUFFIXES {
                let form = suffixed(plain, suffix);
                for method in METHODS {
                    let class = route_class(method, plain);
                    let expected = if PERSONAL_UNDER_A_SCOPED_PREFIX.contains(&path) {
                        RouteClass::Scoped
                    } else {
                        class
                    };
                    assert_eq!(route_class(method, &form), expected, "{method} {form}");
                    assert_eq!(
                        header(&member, method, &form, None).is_err(),
                        class == RouteClass::OwnAccount,
                        "{method} {form}"
                    );
                }
            }
        }
    }
}

/// `classify` keeps the greater class, so failing closed rests on this order.
#[test]
fn the_route_classes_are_ordered_from_least_to_most_strict() {
    assert!(RouteClass::Personal < RouteClass::Scoped);
    assert!(RouteClass::Scoped < RouteClass::OwnAccount);
}

/// A suffix is a `.` and one or more ASCII letters or digits at the end of the last
/// segment; the rest of the path, its trailing `/` included, is kept.
#[test]
fn a_format_suffix_is_the_last_dot_part_of_the_last_segment() {
    let cases = [
        ("/api/cdn/zones.json", Some("/api/cdn/zones")),
        ("/api/cdn/zones.json/", Some("/api/cdn/zones/")),
        ("/api/cdn/zones.JSON", Some("/api/cdn/zones")),
        ("/api/cdn/zones.api/", Some("/api/cdn/zones/")),
        ("/api/x/v1.2/", Some("/api/x/v1/")),
        ("/api/x/a.b.json", Some("/api/x/a.b")),
        ("/api/x/.json", Some("/api/x/")),
        ("/api/x/.json/", Some("/api/x//")),
        ("/api/cdn/zones/", None),
        ("/api/cdn/zones", None),
        ("/api/x/a./", None),
        ("/api/x/a.j-s/", None),
        ("/api/x/a.j_s", None),
        ("/api/a.b/c/", None),
        ("/", None),
        ("", None),
    ];
    for (path, plain) in cases {
        assert_eq!(without_format_suffix(path).as_deref(), plain, "{path}");
    }
}

/// The suffixed routes the route table guards: each suffixed write and read meets the guard of
/// its plain route, and a PUT of one ticket stays refused with a suffix.
#[test]
fn a_format_suffix_meets_the_guard_of_its_plain_route() {
    assert!(returns_secret(
        Method::Post,
        "/api/objectstore/master-tokens.json"
    ));
    assert!(returns_secret(
        Method::Post,
        "/api/objectstore/master-tokens/1/rotate.json"
    ));
    assert!(returns_secret(Method::Get, "/api/auth/token.json"));
    assert!(returns_secret(Method::Get, "/api/auth/token.api/"));
    let rule = |method, path| rule_name(method, path).unwrap_or_default();
    assert_eq!(rule(Method::Post, "/api/account/deletion.json"), "refused");
    for method in METHODS {
        assert_eq!(rule(method, "/api/auth/token.json"), "refused", "{method}");
        for path in [
            "/api/Auth/token/",
            "/api/auth/TOKEN",
            "/API/AUTH/TOKEN.JSON",
        ] {
            assert_eq!(rule(method, path), "refused", "{method} {path}");
        }
    }
    assert_eq!(rule(Method::Post, "/api/Account/Deletion/"), "refused");
    assert_eq!(
        rule(Method::Post, "/api/Compute/app-engine/apps/"),
        "refused if secrets"
    );
    assert!(returns_secret(Method::Post, "/api/Registry/provision/"));
    // A flag rule follows the folded case too, as a refusal does.
    assert_eq!(rule(Method::Post, "/api/CDN/zones/"), "--public");
    for method in [Method::Get, Method::Head] {
        assert_eq!(
            rule(method, "/api/kubernetes/clusters/7/secrets.json"),
            "refused"
        );
    }
    assert_eq!(
        rule(Method::Put, "/api/compute/vms/abc/firewall.json"),
        "--remove-others"
    );
    assert_eq!(rule(Method::Post, "/api/cdn/zones.json"), "--public");
    // Each suffix, with and without the trailing `/`, on each route `e2e::api` sends one of
    // them to: the guard is the plain route's, and the plain route is guarded.
    let guard = |method, path: &str| (returns_secret(method, path), rule_name(method, path));
    for (method, plain) in [
        (Method::Post, "/api/objectstore/master-tokens"),
        (Method::Post, "/api/objectstore/master-tokens/1/rotate"),
        (Method::Get, "/api/auth/token"),
        (Method::Post, "/api/account/deletion"),
        (Method::Put, "/api/compute/vms/abc/firewall"),
        (Method::Post, "/api/cdn/zones"),
    ] {
        let guarded = guard(method, &format!("{plain}/"));
        assert_ne!(guarded, (false, None), "{method} {plain}");
        for suffix in [".json", ".api", ".json/", ".api/"] {
            let path = format!("{plain}{suffix}");
            assert_eq!(guard(method, &path), guarded, "{method} {path}");
        }
    }
    assert_eq!(
        route_class(Method::Put, "/api/support/tickets/7.json/"),
        RouteClass::OwnAccount
    );
    // The customer's close keeps the class of its plain route, as every ticket route does.
    assert_eq!(
        route_class(Method::Patch, "/api/support/tickets/7.json/"),
        RouteClass::Personal
    );
    // A `.` segment of its own is the written form's `{id}`, and both forms count.
    assert_eq!(
        route_class(Method::Put, "/api/support/tickets/.json/"),
        RouteClass::OwnAccount
    );
}

/// The accepted cost: a dot in a real last segment is read as a suffix too. A dotted
/// bucket or host name sits in an `{id}` position or under a prefix, where both forms
/// match alike, so these routes get the same class and rules as an undotted name; a
/// bucket ACL set needs its two flags, as it does for any bucket.
#[test]
fn a_dotted_bucket_or_host_name_is_classified_as_an_undotted_one() {
    for path in [
        "/api/objectstore/buckets/my.bucket/",
        "/api/objectstore/buckets/my.bucket",
        "/api/objectstore/buckets/my.bucket/objects/",
        "/api/cdn/zones/z1/hostnames/www.example.com/",
    ] {
        for method in METHODS {
            assert_eq!(route_class(method, path), RouteClass::Scoped, "{path}");
            assert_eq!(rule_name(method, path), None, "{method} {path}");
            assert!(!returns_secret(method, path), "{method} {path}");
        }
    }
    assert_eq!(
        rule_name(Method::Put, "/api/objectstore/buckets/my.bucket/acl/set/").as_deref(),
        Some("--public --remove-others")
    );
    // A Hub repository delete needs no flag beyond `--yes`, dotted or not.
    for method in METHODS {
        assert_eq!(
            rule_name(method, "/api/registry/repositories/team/app.v2/"),
            None,
            "{method}"
        );
    }
}

/// Each row of [`PLAN_ROUTES`], through [`header`], for a membership and for the
/// caller's own account.
#[test]
fn every_plan_route_family_gets_the_header_its_class_gives() {
    let member = member();
    for &(path, class) in PLAN_ROUTES {
        assert_eq!(
            header(&Selection::Own, Method::Get, path, None).unwrap(),
            None,
            "{path}"
        );
        let sent = header(&member, Method::Get, path, None);
        match class {
            RouteClass::Scoped => assert_eq!(sent.unwrap(), Some(ID), "{path}"),
            RouteClass::Personal => assert_eq!(sent.unwrap(), None, "{path}"),
            RouteClass::OwnAccount => assert_eq!(
                sent.unwrap_err().code().as_str(),
                "account_scope_unsupported",
                "{path}"
            ),
        }
    }
}

/// The ticket routes by method: a `PUT` of one ticket (with or without
/// the trailing `/` or a query) is refused with a membership, and every other method
/// and ticket route stays personal, the customer's close (`PATCH`) included.
#[test]
fn a_put_of_one_ticket_is_unsupported_and_the_rest_of_the_tickets_personal() {
    use RouteClass::{OwnAccount, Personal};
    let rows = [
        (Method::Put, "/api/support/tickets/7/", OwnAccount),
        (Method::Put, "/api/support/tickets/7", OwnAccount),
        (Method::Put, "/api/support/tickets/7/?x=1", OwnAccount),
        (Method::Patch, "/api/support/tickets/7/", Personal),
        (Method::Patch, "/api/support/tickets/7", Personal),
        (Method::Patch, "/api/support/tickets/7/?x=1", Personal),
        (Method::Get, "/api/support/tickets/7/", Personal),
        (Method::Delete, "/api/support/tickets/7/", Personal),
        (Method::Post, "/api/support/tickets/", Personal),
        (Method::Patch, "/api/support/tickets/", Personal),
        (Method::Post, "/api/support/tickets/7/messages/", Personal),
        (Method::Patch, "/api/support/tickets/7/messages/", Personal),
        (Method::Post, "/api/support/tickets/7/handoff/", Personal),
        (
            Method::Post,
            "/api/support/tickets/7/messages/3/attachments/",
            Personal,
        ),
    ];
    for (method, path, class) in rows {
        assert_eq!(route_class(method, path), class, "{method} {path}");
    }
    let error = header(&member(), Method::Put, "/api/support/tickets/7/", None).unwrap_err();
    assert_eq!(error.code(), Code::AccountScopeUnsupported);
    assert_eq!(
        header(&member(), Method::Patch, "/api/support/tickets/7/", None).unwrap(),
        None
    );
    // The server decides which fields of a ticket a customer may change, so the `PATCH`
    // carries no rule on its body.
    assert_eq!(rule_name(Method::Patch, "/api/support/tickets/7/"), None);
    assert_eq!(
        header(
            &Selection::Own,
            Method::Put,
            "/api/support/tickets/7/",
            None
        )
        .unwrap(),
        None
    );
}

#[test]
fn a_secret_route_issues_a_secret_with_or_without_its_trailing_slash() {
    for path in [
        "/api/objectstore/master-tokens/",
        "/api/objectstore/master-tokens",
        "/api/objectstore/sub-tokens",
        "/api/registry/robot/rotate",
        "/api/registry/provision",
        "/api/registry/keys",
        "/api/registry/keys/7/rotate",
        "/api/objectstore/master-tokens/k1/rotate",
        "/api/databases",
        "/api/databases/7/restore",
        "/api/network/peers/enroll",
        "/api/compute/vms/vm-1/terminal-ticket",
    ] {
        assert!(returns_secret(Method::Post, path), "{path}");
    }
    assert!(returns_secret(Method::Get, "/api/auth/token"));
    for path in [
        "/api/registry/keys/7",
        "/api/registry/robot",
        "/api/ssh-keys",
    ] {
        assert!(!returns_secret(Method::Post, path), "{path}");
    }
}

/// Each route issues its secret to one method only: the other methods on the same path
/// read, change, or delete without an answer that holds the secret.
#[test]
fn a_secret_route_matches_its_own_method_and_no_other() {
    let routes = [
        (Method::Post, "/api/objectstore/master-tokens/"),
        (Method::Post, "/api/objectstore/master-tokens/k1/rotate/"),
        (Method::Post, "/api/objectstore/sub-tokens/"),
        (Method::Post, "/api/objectstore/sub-tokens/k1/rotate/"),
        (Method::Post, "/api/registry/keys/"),
        (Method::Post, "/api/registry/keys/7/rotate/"),
        (Method::Post, "/api/registry/robot/rotate/"),
        (Method::Post, "/api/registry/provision/"),
        (Method::Post, "/api/databases/"),
        (Method::Post, "/api/databases/7/restore/"),
        (Method::Get, "/api/databases/7/backup-token/"),
        (Method::Post, "/api/databases/7/backup-token/"),
        (Method::Post, "/api/network/peers/enroll/"),
        (Method::Post, "/api/compute/vms/vm-1/terminal-ticket/"),
        (Method::Post, "/api/infrastructure/compute-miners/prepare/"),
        (Method::Post, "/api/chat/visitors/"),
        (Method::Post, "/api/chat/transfer/"),
        (Method::Post, "/api/chat/transfer/redeem/"),
        (Method::Post, "/api/billing/transactions/"),
        (Method::Post, "/api/objectstore/tokens/auth/"),
        (Method::Get, "/api/auth/token/"),
        (Method::Post, "/api/auth/exchange/"),
        (Method::Post, "/api/auth/verify/"),
        (Method::Get, "/api/kubernetes/clusters/7/backups/"),
        (
            Method::Get,
            "/api/objectstore/buckets/photos/presigned-url/",
        ),
        (Method::Get, "/api/chat/conversations/7/attachments/3/"),
        (Method::Get, "/api/support/chat/tickets/7/attachments/3/"),
        (Method::Post, "/api/auth/sso/ticket/"),
        (Method::Post, "/api/auth/session/"),
        (Method::Post, "/api/auth/sign-in-methods/step-up/verify/"),
        (Method::Post, "/api/network/infra/enroll/"),
        (Method::Post, "/api/chat/workspaces/w1/invites/"),
    ];
    assert_eq!(
        routes.len(),
        secret_routes().count(),
        "one case per row that reveals a secret and per table entry"
    );
    for (_, path) in routes {
        for method in METHODS {
            let expected = routes.iter().any(|&(m, p)| p == path && m == method);
            assert_eq!(returns_secret(method, path), expected, "{method} {path}");
            // A format suffix reaches the same route if the backend adds suffix routes.
            for path in suffixed_forms(path) {
                assert_eq!(returns_secret(method, &path), expected, "{method} {path}");
            }
        }
    }
    // The neighbours of a route with a pattern segment.
    for path in [
        "/api/compute/vms/terminal-ticket/",
        "/api/compute/vms//terminal-ticket/",
        "/api/compute/vms/a/b/terminal-ticket/",
        "/api/compute/vms/vm-1/",
        "/api/compute/vms/vm-1/terminal-ticket/x/",
        "/api/auth/token/x/",
        "/api/auth/",
        "/api/network/peers/",
        "/api/network/enroll-keys/",
        "/api/kubernetes/clusters/7/",
        "/api/kubernetes/clusters/7/events/",
    ] {
        for method in METHODS {
            assert!(!returns_secret(method, path), "{method} {path}");
        }
    }
}

const METHODS: [Method; 6] = [
    Method::Get,
    Method::Head,
    Method::Post,
    Method::Put,
    Method::Patch,
    Method::Delete,
];

/// The network summary provisions on a GET and a HEAD, with or without its trailing
/// slash, with a format suffix, and with a query; the writes on the path and its
/// neighbours do not.
#[test]
fn the_network_summary_provisions_on_its_reads_and_nowhere_else() {
    assert_eq!(
        routes::all().filter(|row| row.provisions()).count(),
        1,
        "a new row needs cases here"
    );
    let path = "/api/network/me/";
    let mut forms = vec![path.to_owned(), slashless(path), format!("{path}?x=1")];
    forms.extend(suffixed_forms(path));
    for form in &forms {
        for method in METHODS {
            let expected = matches!(method, Method::Get | Method::Head);
            assert_eq!(provisions(method, form), expected, "{method} {form}");
        }
    }
    for path in [
        "/api/network/",
        "/api/network/me/x/",
        "/api/network/me//",
        "/api/network/peers/",
        "/api/networks/me/",
        "/network/me/",
        "/api/network/me.tar.gz/",
    ] {
        for method in METHODS {
            assert!(!provisions(method, path), "{method} {path}");
        }
    }
}

/// The rules on `method` to `path` as text: `refused` for a route never sent,
/// otherwise the flag it needs, followed by `if <key>` when the body decides; one rule
/// that both forms of a suffixed path match is named once, and two are joined by `; `.
fn rule_name(method: Method, path: &str) -> Option<String> {
    let names = |flags: &[RouteFlag]| {
        flags
            .iter()
            .map(|flag| flag.flag())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut rules: Vec<String> = route_rules(method, path)
        .into_iter()
        .filter_map(|rule| match rule.guard {
            Guard::None | Guard::Distinct => None,
            Guard::Refused { .. } => Some("refused".to_owned()),
            Guard::RefusedWhenSet { key, .. } => Some(format!("refused if {key}")),
            Guard::Needs(flags) => Some(names(flags)),
            Guard::NeedsWhenSet(flags, key) => Some(format!("{} if {key}", names(flags))),
            Guard::NeedsUnlessFalse(flags, key) => {
                Some(format!("{} unless {key} is false", names(flags)))
            }
        })
        .collect();
    rules.dedup();
    (!rules.is_empty()).then(|| rules.join("; "))
}

/// The format suffixes the tests append: Django REST framework's `json`, and `api`, the
/// name of its browsable API renderer's format.
pub(crate) const SUFFIXES: [&str; 2] = [".json", ".api"];

/// `path` with `suffix` on its last segment, before its trailing `/` and its query:
/// `/api/cdn/zones.json/?x=1` for `/api/cdn/zones/?x=1`.
pub(crate) fn suffixed(path: &str, suffix: &str) -> String {
    let (path, query) = path
        .split_once('?')
        .map_or((path, None), |(p, q)| (p, Some(q)));
    let (body, slash) = path
        .strip_suffix('/')
        .map_or((path, ""), |body| (body, "/"));
    let query = query.map(|q| format!("?{q}")).unwrap_or_default();
    format!("{body}{suffix}{slash}{query}")
}

/// `path` without its trailing `/`, its query kept.
pub(crate) fn slashless(path: &str) -> String {
    let (route, query) = path
        .split_once('?')
        .map_or((path, None), |(r, q)| (r, Some(q)));
    let route = route.strip_suffix('/').unwrap_or(route);
    query.map_or_else(|| route.to_owned(), |query| format!("{route}?{query}"))
}

/// The forms of `path` a guard must treat alike: with each suffix, with and without its
/// trailing `/`.
fn suffixed_forms(path: &str) -> Vec<String> {
    let slashless = path.strip_suffix('/').unwrap_or(path);
    SUFFIXES
        .iter()
        .flat_map(|suffix| [suffixed(path, suffix), suffixed(slashless, suffix)])
        .collect()
}

/// Each route rule holds for its own method, with or without its trailing slash; a rule
/// that asks for a flag on a write holds for every other write method of its route too,
/// and a refusal or a read's rule for no other method.
#[test]
fn a_route_rule_matches_its_own_method_and_a_flag_every_write() {
    let routes = [
        (Method::Delete, "/api/account/deletion/", ""),
        (Method::Delete, "/api/account/deletion", ""),
        (Method::Post, "/api/account/deletion/", "refused"),
        (Method::Post, "/api/account/deletion", "refused"),
        (Method::Get, "/api/auth/token/", "refused"),
        (Method::Get, "/api/auth/token", "refused"),
        (Method::Head, "/api/auth/token/", "refused"),
        (Method::Head, "/api/auth/token", "refused"),
        (Method::Post, "/api/auth/token/", "refused"),
        (Method::Post, "/api/auth/token", "refused"),
        (Method::Put, "/api/auth/token/", "refused"),
        (Method::Put, "/api/auth/token", "refused"),
        (Method::Patch, "/api/auth/token/", "refused"),
        (Method::Patch, "/api/auth/token", "refused"),
        (Method::Delete, "/api/auth/token/", "refused"),
        (Method::Delete, "/api/auth/token", "refused"),
        (
            Method::Get,
            "/api/kubernetes/clusters/7/secrets/",
            "refused",
        ),
        (Method::Get, "/api/kubernetes/clusters/7/secrets", "refused"),
        (
            Method::Head,
            "/api/kubernetes/clusters/7/secrets",
            "refused",
        ),
        (
            Method::Head,
            "/api/kubernetes/clusters/7/secrets/",
            "refused",
        ),
        (
            Method::Post,
            "/api/cdn/zones/z1/hostnames/cdn.example.com/dns-setup/",
            "refused",
        ),
        (
            Method::Post,
            "/api/cdn/zones/z1/hostnames/cdn.example.com/dns-setup",
            "refused",
        ),
        (
            Method::Put,
            "/api/cdn/zones/z1/hostnames/cdn.example.com/dns-setup/",
            "refused",
        ),
        (
            Method::Patch,
            "/api/cdn/zones/z1/hostnames/cdn.example.com/dns-setup/",
            "refused",
        ),
        (
            Method::Put,
            "/api/cdn/zones/z1/hostnames/cdn.example.com/dns-setup",
            "refused",
        ),
        (
            Method::Patch,
            "/api/cdn/zones/z1/hostnames/cdn.example.com/dns-setup",
            "refused",
        ),
        (
            Method::Post,
            "/api/network/isolation/",
            "--change-isolation",
        ),
        (Method::Post, "/api/network/isolation", "--change-isolation"),
        (Method::Post, "/api/registry/subscribe/", "refused"),
        (Method::Post, "/api/registry/subscribe", "refused"),
        (Method::Post, "/api/cdn/zones/", "--public"),
        (Method::Post, "/api/cdn/zones", "--public"),
        (Method::Patch, "/api/cdn/zones/z1/", "--public if origin"),
        (Method::Patch, "/api/cdn/zones/z1", "--public if origin"),
        (
            Method::Put,
            "/api/compute/vms/vm-1/firewall/",
            "--remove-others",
        ),
        (
            Method::Put,
            "/api/compute/vms/vm-1/firewall",
            "--remove-others",
        ),
        (Method::Put, "/api/cdn/zones/z1/rules/", "--remove-others"),
        (Method::Put, "/api/cdn/zones/z1/rules", "--remove-others"),
        (
            Method::Patch,
            "/api/accounts/members/7/",
            "--remove-others if grants",
        ),
        (
            Method::Patch,
            "/api/accounts/members/7",
            "--remove-others if grants",
        ),
    ];
    assert_rules_hold(&routes);
}

/// The three App Engine routes whose body may carry secret values are refused only when
/// it sets `secrets`, each for its own method.
#[test]
fn an_app_engine_secrets_rule_matches_its_own_method_and_no_other() {
    let routes = [
        (
            Method::Post,
            "/api/compute/app-engine/apps/",
            "refused if secrets",
        ),
        (
            Method::Post,
            "/api/compute/app-engine/apps",
            "refused if secrets",
        ),
        (
            Method::Put,
            "/api/compute/app-engine/apps/vm-1/compose/",
            "refused if secrets",
        ),
        (
            Method::Patch,
            "/api/compute/app-engine/apps/vm-1/compose/",
            "refused if secrets",
        ),
        (
            Method::Post,
            "/api/compute/app-engine/apps/vm-1/compose/",
            "refused if secrets",
        ),
        (
            Method::Delete,
            "/api/compute/app-engine/apps/vm-1/compose/",
            "refused if secrets",
        ),
        (
            Method::Put,
            "/api/compute/app-engine/apps/vm-1/compose",
            "refused if secrets",
        ),
        (
            Method::Patch,
            "/api/compute/app-engine/apps/vm-1/compose",
            "refused if secrets",
        ),
        (
            Method::Post,
            "/api/compute/app-engine/apps/vm-1/compose",
            "refused if secrets",
        ),
        (
            Method::Delete,
            "/api/compute/app-engine/apps/vm-1/compose",
            "refused if secrets",
        ),
        (
            Method::Post,
            "/api/compute/app-engine/validate/",
            "refused if secrets",
        ),
        (
            Method::Post,
            "/api/compute/app-engine/validate",
            "refused if secrets",
        ),
    ];
    assert_rules_hold(&routes);
}

/// The whole-list replaces of the WAF rules, rate limits and list items, and of an
/// operator node's storage: each holds for its own method and every other write method,
/// but a list items `POST` or `DELETE`, which needs none (`""`), and the two rule tests
/// together cover every row.
#[test]
fn an_uncurated_replace_rule_matches_its_own_method_and_every_write() {
    let routes = [
        (
            Method::Put,
            "/api/cdn/zones/z1/waf/rules/",
            "--remove-others",
        ),
        (
            Method::Put,
            "/api/cdn/zones/z1/waf/rules",
            "--remove-others",
        ),
        (
            Method::Put,
            "/api/cdn/zones/z1/waf/rate-limits/",
            "--remove-others",
        ),
        (
            Method::Put,
            "/api/cdn/lists/blocked/items/",
            "--remove-others",
        ),
        (Method::Post, "/api/cdn/lists/blocked/items/", ""),
        (Method::Delete, "/api/cdn/lists/blocked/items/", ""),
        (
            Method::Patch,
            "/api/cdn/lists/blocked/",
            "--remove-others if items",
        ),
        (
            Method::Put,
            "/api/v1/operator/nodes/n1/storage/",
            "--remove-others",
        ),
        (
            Method::Put,
            "/api/v1/operator/nodes/n1/storage",
            "--remove-others",
        ),
        (
            Method::Put,
            "/api/objectstore/buckets/b/acl/set/",
            "--public --remove-others",
        ),
        (
            Method::Patch,
            "/api/registry/me/publicity/",
            "--public unless public is false",
        ),
        (
            Method::Patch,
            "/api/registry/me/publicity",
            "--public unless public is false",
        ),
    ];
    assert_rules_hold(&routes);
    let guarded_rows = routes::all()
        .filter(|row| row.guard() != Guard::None)
        .count();
    assert_eq!(
        ROUTE_RULES.len() + guarded_rows,
        29,
        "a new row or entry needs cases in a rule test"
    );
}

/// Each `(method, path, rule)` holds for that method, and no method that no entry pairs
/// with the path has a rule. The same holds for the path with a format suffix.
fn assert_rules_hold(routes: &[(Method, &str, &str)]) {
    for &(_, path, _) in routes {
        for method in METHODS {
            // A write's rule holds for the route's other write methods too, but for a row
            // with no flag (`""`), which lends nothing.
            let from_another_write = || {
                routes.iter().find(|&&(m, p, rule)| {
                    p == path && !m.is_read() && !method.is_read() && !rule.is_empty()
                })
            };
            let expected = routes
                .iter()
                .find(|&&(m, p, _)| m == method && p == path)
                .or_else(from_another_write)
                .map(|&(m, _, rule)| {
                    // A `PUT` borrows a body-conditional flag rule as unconditional; a
                    // refusal stays conditional, since a body without the key sends no
                    // secret.
                    if m != method && method == Method::Put && !rule.starts_with("refused") {
                        rule.split(" if ").next().unwrap_or(rule).to_owned()
                    } else {
                        rule.to_owned()
                    }
                })
                .filter(|rule| !rule.is_empty());
            assert_eq!(rule_name(method, path), expected, "{method} {path}");
            for path in suffixed_forms(path) {
                assert_eq!(rule_name(method, &path), expected, "{method} {path}");
            }
        }
    }
}

/// The rules the API documents for one write method hold for every write method of their
/// route, so a `PUT` where the API lists `PATCH` needs the same flag, and in any letter
/// case; a read needs none. A `POST` or `DELETE` of a list's items adds or removes the
/// items it names, so it needs no flag, while a `PUT` or `PATCH` of them does. A refusal
/// keeps its own method.
#[test]
fn a_flag_rule_holds_for_every_write_method_and_letter_case() {
    let rules = [
        ("/api/cdn/zones/z1/", "--public if origin"),
        ("/api/accounts/members/7/", "--remove-others if grants"),
        ("/api/cdn/lists/blocked/", "--remove-others if items"),
        (
            "/api/registry/me/publicity/",
            "--public unless public is false",
        ),
        ("/api/network/isolation/", "--change-isolation"),
        ("/api/compute/vms/vm-1/firewall/", "--remove-others"),
    ];
    for (path, rule) in rules {
        let upper = path.to_ascii_uppercase().replace("/API/", "/api/");
        for form in [path.to_owned(), upper, slashless(path)] {
            for method in [Method::Post, Method::Put, Method::Patch, Method::Delete] {
                // A `PUT` replaces the whole resource, so a rule conditional on a key the
                // body sets holds for it whatever the body sets.
                let expected = if method == Method::Put {
                    rule.split(" if ").next().unwrap_or(rule)
                } else {
                    rule
                };
                assert_eq!(
                    rule_name(method, &form).as_deref(),
                    Some(expected),
                    "{method} {form}"
                );
            }
            for method in [Method::Get, Method::Head] {
                assert_eq!(rule_name(method, &form), None, "{method} {form}");
            }
        }
    }
    let items = "/api/cdn/lists/blocked/items/";
    for (method, expected) in [
        (Method::Post, None),
        (Method::Delete, None),
        (Method::Put, Some("--remove-others")),
        (Method::Patch, Some("--remove-others")),
    ] {
        assert_eq!(rule_name(method, items).as_deref(), expected, "{method}");
    }
    // A refusal holds for every write method too; a deletion request's `DELETE`, which
    // withdraws it, is sent.
    for (method, path) in [
        (Method::Put, "/api/account/deletion/"),
        (Method::Patch, "/api/account/deletion/"),
        (Method::Put, "/api/registry/subscribe/"),
        (Method::Delete, "/api/cdn/zones/z1/hostnames/h/dns-setup/"),
    ] {
        assert_eq!(
            rule_name(method, path).as_deref(),
            Some("refused"),
            "{method} {path}"
        );
    }
    assert_eq!(rule_name(Method::Delete, "/api/account/deletion/"), None);
    assert_eq!(
        rule_name(Method::Put, "/api/compute/app-engine/validate/").as_deref(),
        Some("refused if secrets")
    );
    // The route a refusal names is refused in any letter case too.
    assert_eq!(
        rule_name(Method::Post, "/api/Account/Deletion/").as_deref(),
        Some("refused")
    );
}

/// Every route table matches a path in any letter case: a front end that folds case
/// delivers `/api/Network/me/` to the network summary, whose first read creates the
/// network, and `/api/Registry/...` to a permanent delete.
#[test]
fn every_route_table_matches_in_any_letter_case() {
    for path in ["/api/Network/me/", "/API/NETWORK/ME", "/api/network/ME/"] {
        let path = path.replacen("/API/", "/api/", 1);
        assert!(provisions(Method::Get, &path), "{path}");
        assert!(provisions(Method::Head, &path), "{path}");
    }
    assert!(permanent(
        Method::Delete,
        "/api/Registry/Repositories/ns/app/"
    ));
    assert!(returns_secret(Method::Get, "/api/Auth/Token/"));
    assert_eq!(
        first_listed(
            &[(Method::Get, "/api/x/{id}/", 7)],
            Method::Get,
            "/api/X/1/"
        ),
        Some(&7)
    );
}

/// `pattern` with each placeholder filled by a value every placeholder kind matches.
fn filled(pattern: &str) -> String {
    pattern
        .split('/')
        .map(|segment| {
            if segment.starts_with('{') {
                "7"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// `api_guard`, which the help and the schema say the rules from, is the guard
/// `route_rules` applies on a path that fills the template, on every method.
#[test]
fn the_guard_of_a_template_is_the_rule_of_its_path() {
    let methods = [
        Method::Get,
        Method::Head,
        Method::Post,
        Method::Put,
        Method::Patch,
        Method::Delete,
    ];
    for (_, template, _) in api_routes() {
        let path = filled(template);
        for method in methods {
            let applied = route_rules(method, &path).first().map(|rule| rule.guard);
            assert_eq!(
                applied.unwrap_or(Guard::None),
                api_guard(method, template),
                "{method} {template}"
            );
        }
    }
}

/// The tables hold only routes no row decides: a row decides its own method's secret, and
/// a path a row's template matches takes its guards from the rows, so an entry there
/// would be a second copy that never decides. Their remaining entries are routes `hipc`
/// reaches only through `hipc api`.
#[test]
fn no_table_entry_is_a_route_a_row_decides() {
    for &(method, pattern) in SECRET_ROUTES {
        let path = filled(pattern);
        assert_eq!(routes::row(method, &path), None, "{method} {pattern}");
    }
    for &(method, pattern, _) in ROUTE_RULES {
        let path = filled(pattern);
        assert_eq!(routes::matching(&path).next(), None, "{method} {pattern}");
    }
}

/// The row a request reaches is found in any form of its path, the written one first,
/// and a route no row matches is one `hipc` does not know.
#[test]
fn a_request_reaches_its_row_in_any_form() {
    let vm = routes::compute::VM.endpoint();
    for path in [
        "/api/compute/vms/v1/",
        "/api/compute/vms/v1",
        "/api/compute/vms/v1.json/",
        "/api/Compute/VMs/v1/",
        "/api/compute/vms/v1/?x=1",
    ] {
        assert_eq!(row_of(Method::Get, path), Some(vm), "{path}");
    }
    assert_eq!(row_of(Method::Head, "/api/compute/vms/v1/"), Some(vm));
    for path in ["/api/v2/compute/vms/v1/", "/api/compute/vms/v1/x/"] {
        assert_eq!(row_of(Method::Get, path), None, "{path}");
    }
    assert_eq!(row_of(Method::Patch, "/api/compute/vms/v1/"), None);
}

/// The curated commands' flags keep their codes in `hipc api`, and each code names the
/// flag that gets past it, `--change-isolation` included.
#[test]
fn a_route_flag_refuses_with_the_code_of_its_flag() {
    for (place, flag) in RouteFlag::ALL.into_iter().enumerate() {
        let listed_at = match flag {
            RouteFlag::ChangeIsolation => 0,
            RouteFlag::Public => 1,
            RouteFlag::RemoveOthers => 2,
        };
        assert_eq!(listed_at, place, "{}", flag.flag());
        assert_eq!(flag.code().flag(), Some(flag.flag()), "{}", flag.flag());
    }
    let codes = RouteFlag::ALL.map(|flag| (flag.flag(), flag.code().as_str()));
    assert_eq!(
        codes,
        [
            ("--change-isolation", "change_isolation_required"),
            ("--public", "public_required"),
            ("--remove-others", "remove_others_required"),
        ]
    );
}

/// A Hub repository or artifact DELETE is permanent in every form a route table matches:
/// slashless, with a format suffix, and with a dot in the last segment; no other method
/// on it is, and no neighbour is.
#[test]
fn a_hub_delete_is_permanent_and_its_neighbours_are_not() {
    for path in [
        "/api/registry/repositories/app/",
        "/api/registry/repositories/team/app/",
        "/api/registry/repositories/team/app",
        "/api/registry/repositories/team/app.v2/",
        "/api/registry/repositories/team/app/artifacts/v1/",
        "/api/registry/repositories/team/app/artifacts/sha256:ab12",
    ] {
        for method in METHODS {
            let expected = method == Method::Delete;
            assert_eq!(permanent(method, path), expected, "{method} {path}");
            for path in suffixed_forms(path) {
                assert_eq!(permanent(method, &path), expected, "{method} {path}");
            }
        }
    }
    for path in [
        "/api/registry/repositories/",
        "/api/registry/repositories",
        "/api/registry/repositories//",
        "/api/registry/repositories/app//",
        "/api/registry/repositories//artifacts/v1/",
        "/api/registry/repository/app/",
        "/api/registry/keys/7/",
        "/registry/repositories/app/",
    ] {
        for method in METHODS {
            assert!(!permanent(method, path), "{method} {path}");
        }
    }
    assert_eq!(
        routes::all().filter(|row| row.permanent()).count(),
        2,
        "a new row needs cases here"
    );
}

/// The neighbours of each route rule, which no method matches: the cancel and preview
/// routes of a deletion, and paths a segment off.
#[test]
fn the_neighbours_of_a_route_rule_match_no_rule() {
    for path in [
        "/api/account/",
        "/api/account/deletion/preview/",
        "/api/account/deletion/cancel/tok/",
        "/api/account/deletion/x/",
        "/api/accounts/deletion/",
        "/api/auth/",
        "/api/auth/token/x/",
        "/api/auth/tokens/",
        "/api/auth/exchange/",
        "/api/kubernetes/clusters/7/",
        "/api/kubernetes/clusters//secrets/",
        "/api/kubernetes/clusters/7/secrets/x/",
        "/api/kubernetes/clusters/7/backups/",
        "/api/kubernetes/secrets/",
        "/api/network/",
        "/api/network/isolation/x/",
        "/api/network/isolations/",
        "/registry/repositories/app/",
        "/api/cdn/",
        "/api/cdn/zones//",
        "/api/cdn/zones/z1/hostnames/",
        "/api/cdn/zones/z1/rules/x/",
        "/api/cdn/zones/z1/waf/",
        "/api/cdn/zones/z1/waf/rules/x/",
        "/api/cdn/zones/z1/waf/versions/",
        "/api/cdn/lists/",
        "/api/cdn/lists//items/",
        "/api/cdn/lists/blocked/items/x/",
        "/api/v1/operator/nodes/n1/",
        "/api/v1/operator/nodes/n1/storage/pools/",
        "/api/v1/operator/nodes//storage/",
        "/api/objectstore/buckets/b/acl/",
        "/api/objectstore/buckets//acl/set/",
        "/api/objectstore/acl/",
        "/api/compute/vms/",
        "/api/compute/vms/vm-1/firewall/x/",
        "/api/compute/vms//firewall/",
        "/api/accounts/members/",
        "/api/accounts/members//",
        "/api/accounts/members/7/x/",
    ] {
        for method in METHODS {
            assert_eq!(rule_name(method, path), None, "{method} {path}");
        }
    }
}

#[test]
fn the_database_create_and_restore_issue_a_secret_and_its_other_routes_do_not() {
    for path in ["/api/databases/", "/api/databases/7/restore/"] {
        assert!(returns_secret(Method::Post, path), "{path}");
    }
    for path in [
        "/api/databases/7/",
        "/api/databases/7/restart/",
        "/api/databases/7/expose/",
        "/api/databases/x/restore/",
        "/api/databases//restore/",
        "/api/databases/7/restore/x/",
        "/api/databases/refusals/",
    ] {
        assert!(!returns_secret(Method::Post, path), "{path}");
    }
}

#[test]
fn own_account_never_sends_the_header() {
    for path in ["/api/compute/vms/", "/api/drive/subscription/"] {
        assert_eq!(
            header(&Selection::Own, Method::Get, path, None).unwrap(),
            None
        );
    }
}

/// A path without its trailing `/` gets the class of the path with it: a personal route
/// is not refused, and a scoped one sends the header rather than being refused. With a
/// format suffix too, except that a personal path under a scoped prefix becomes scoped,
/// the stricter class ([`PERSONAL_UNDER_A_SCOPED_PREFIX`]).
#[test]
fn a_slashless_path_gets_the_class_of_the_slashed_one() {
    use RouteClass::{Personal, Scoped};
    let member = member();
    let rows = [
        ("/api/objectstore/tokens/", Personal, Scoped),
        ("/api/ssh-keys/", Personal, Personal),
        ("/api/databases/12/members/", Personal, Scoped),
        ("/api/accounts/", Personal, Personal),
        ("/api/users/me/", Personal, Personal),
        ("/api/support/tickets/", Personal, Personal),
        ("/api/drive/plans/", Personal, Personal),
        ("/api/registry/plans/", Personal, Personal),
        ("/api/notifications/settings/", Personal, Personal),
        ("/api/notifications/marketing-consent/", Personal, Personal),
        ("/api/billing/credits/balance/", Scoped, Scoped),
        ("/api/billing/invoices/", Scoped, Scoped),
        ("/api/billing/profile/", Scoped, Scoped),
        (
            "/api/billing/stripe/create-checkout-session/",
            Scoped,
            Scoped,
        ),
        ("/api/billing/btcpay/create-invoice/", Scoped, Scoped),
        ("/api/billing/auto-reload/", Scoped, Scoped),
    ];
    for (path, class, suffixed_class) in rows {
        let slashless = slashless(path);
        let suffixed = SUFFIXES
            .iter()
            .flat_map(|suffix| [suffixed(path, suffix), suffixed(&slashless, suffix)]);
        let forms = [(path.to_owned(), class), (slashless.clone(), class)]
            .into_iter()
            .chain(suffixed.map(|form| (form, suffixed_class)));
        for (form, class) in forms {
            for method in [Method::Get, Method::Post] {
                assert_eq!(route_class(method, &form), class, "{method} {form}");
                let expected = (class == Scoped).then_some(ID);
                assert_eq!(
                    header(&member, method, &form, None).unwrap(),
                    expected,
                    "{method} {form}"
                );
            }
        }
    }
}

/// The subscribe 503 is retryable on its route with or without the trailing `/`, and a
/// format suffix keeps it `partial`, the stricter answer.
#[test]
fn a_subscribe_503_did_nothing_with_or_without_the_slash() {
    for path in [
        "/api/drive/subscription/",
        "/api/drive/subscription",
        "/api/s3/subscription/",
        "/api/s3/subscription",
    ] {
        assert!(not_done_on_503(Method::Post, path), "{path}");
        for method in [Method::Get, Method::Patch, Method::Delete] {
            assert!(!not_done_on_503(method, path), "{method} {path}");
        }
    }
    for path in [
        "/api/drive/subscription.json",
        "/api/s3/subscription.json/",
        "/API/drive/subscription/",
        "/api/drive/subscription/x/",
        "/api/drive/",
    ] {
        assert!(!not_done_on_503(Method::Post, path), "{path}");
    }
}

#[test]
fn membership_sends_skips_or_refuses_by_class() {
    let member = member();
    assert_eq!(
        header(&member, Method::Get, "/api/s3/subscription/", None).unwrap(),
        Some(ID)
    );
    assert_eq!(
        header(&member, Method::Get, "/api/ssh-keys/", None).unwrap(),
        None
    );
    assert_eq!(
        header(&member, Method::Get, "/api/accounts/", None).unwrap(),
        None
    );
    let error = header(&member, Method::Get, "/api/drive/subscription/", None).unwrap_err();
    assert_eq!(error.code().as_str(), "account_scope_unsupported");
}

/// The recurring card routes act in a membership only beside a billing account; without
/// one they stay refused, and no other refused route is opened by one.
#[test]
fn the_recurring_card_routes_take_a_membership_only_beside_a_billing_account() {
    let member = member();
    let billing = Some("bill_9f8e7d6c5b4a39281706");
    for path in [
        "/api/billing/stripe/active-subscription/",
        "/api/billing/stripe/create-subscription/",
        "/api/billing/stripe/customer-portal/",
    ] {
        assert_eq!(
            header(&member, Method::Get, path, billing).unwrap(),
            Some(ID)
        );
        let error = header(&member, Method::Get, path, None).unwrap_err();
        assert_eq!(error.code(), Code::AccountScopeUnsupported, "{path}");
        assert_eq!(
            header(&Selection::Own, Method::Get, path, billing).unwrap(),
            None
        );
    }
    let error = header(&member, Method::Get, "/api/drive/subscription/", billing).unwrap_err();
    assert_eq!(error.code(), Code::AccountScopeUnsupported);
}

/// The refusal quotes the path with every query value and every bare query part
/// redacted: `hipc` cannot tell which parameter of a `hipc api` path is a secret (tenet 9).
#[test]
fn the_account_refusal_quotes_no_query_value() {
    let error = header(
        &member(),
        Method::Get,
        "/api/registry/me/?token=SEKRIT&page",
        None,
    )
    .unwrap_err();
    assert_eq!(
        error.message(),
        format!(
            "`/api/registry/me/?token=<redacted>&<redacted>` cannot act in another account, \
             and the selected account is `{ID}`"
        )
    );
    // A backtick in the path cannot close the literal early.
    let error = header(&member(), Method::Get, "/api/x`y/", None).unwrap_err();
    assert!(
        error.message().starts_with(r"`/api/x\u{60}y/` cannot"),
        "{error}"
    );
}

/// The refusal names where the account came from (tenet 15); for the profile it also
/// names the command that clears it, while the hint stays the flag that fixes this run.
#[test]
fn the_account_refusal_names_where_the_account_came_from() {
    let stored = Source::Profile {
        field: "account",
        profile: "default".to_owned(),
        path: "/h/config.toml".into(),
    };
    let drive = "/api/drive/subscription/";
    for (source, from) in [
        (Source::Flag("--account"), " from `--account`".to_owned()),
        (
            Source::Variable("HIPPIUS_ACCOUNT"),
            " from `HIPPIUS_ACCOUNT`".to_owned(),
        ),
        (
            stored,
            " from the `account` of the profile `default` in `/h/config.toml`; \
             `hipc account use self` clears it"
                .to_owned(),
        ),
    ] {
        let selection = Selection::parse(ID, &source).unwrap();
        let error = header(&selection, Method::Get, drive, None).unwrap_err();
        assert_eq!(
            error.message(),
            format!(
                "`{drive}` cannot act in another account, and the selected account is \
                 `{ID}`{from}"
            )
        );
        assert_eq!(
            error.hint(),
            Some("pass `--account self` to act in the token's own account")
        );
    }
}

/// A malformed account id is quoted as every other value is, so a backtick in it cannot
/// close the literal early; the message names the source, and the hint its fix.
#[test]
fn a_bad_account_id_is_quoted_and_names_its_source() {
    let error = Selection::parse("a`b", &Source::Flag("--account")).unwrap_err();
    assert_eq!(
        error.message(),
        concat!(
            r"`a\u{60}b` from `--account` is not an account id: an account is `self`, or ",
            "`acct_` and 20 lowercase hex characters"
        )
    );
    assert_eq!(
        error.hint(),
        Some("pass `--account self`, or an id from `hipc account list`")
    );
    let error = Selection::parse("x", &Source::Variable("HIPPIUS_ACCOUNT")).unwrap_err();
    assert_eq!(
        error.hint(),
        Some("unset `HIPPIUS_ACCOUNT`, or set it to `self` or an id from `hipc account list`")
    );
    let stored = Source::Profile {
        field: "account",
        profile: "default".to_owned(),
        path: "/h/config.toml".into(),
    };
    let error = Selection::parse("x", &stored).unwrap_err();
    assert_eq!(
        error.hint(),
        Some("run `hipc account use self` to clear the stored account")
    );
    let error = Selection::parse("x", &Source::Argument).unwrap_err();
    assert_eq!(
        error.hint(),
        Some("run `hipc account list` to see the account ids the token can act in")
    );
}

/// A route's secret, provisioning, permanent-delete, and guard rules hold for its path under
/// another API version, with or without a version segment, in any letter case, so a version
/// the API adds before a row names it fails closed. The row lookup reads the path as written,
/// so `hipc api` still treats such a path as a route it does not know.
#[test]
fn a_rule_holds_for_its_route_under_another_api_version() {
    for path in [
        "/api/v2/auth/token/",
        "/api/v1/auth/token/",
        "/api/V9/Auth/Token",
    ] {
        for method in [Method::Get, Method::Post, Method::Put] {
            assert_eq!(
                rule_name(method, path).as_deref(),
                Some("refused"),
                "{method} {path}"
            );
        }
    }
    assert!(returns_secret(
        Method::Post,
        "/api/v2/objectstore/master-tokens/"
    ));
    assert!(returns_secret(
        Method::Get,
        "/api/v3/databases/7/backup-token/"
    ));
    assert!(provisions(Method::Get, "/api/v2/network/me/"));
    assert!(permanent(
        Method::Delete,
        "/api/v2/registry/repositories/acme/app/"
    ));
    for path in [
        "/api/v2/operator/nodes/7/storage/",
        "/api/operator/nodes/7/storage/",
    ] {
        assert_eq!(
            rule_name(Method::Put, path).as_deref(),
            Some("--remove-others"),
            "{path}"
        );
    }
    assert_eq!(
        rule_name(Method::Post, "/api/v2/network/isolation/").as_deref(),
        Some("--change-isolation")
    );
    // A segment that is not a version is a path of its own.
    for path in [
        "/api/v/auth/token/",
        "/api/vx/auth/token/",
        "/api/x2/auth/token/",
    ] {
        assert_eq!(rule_name(Method::Post, path), None, "{path}");
    }
    assert!(!returns_secret(
        Method::Post,
        "/api/version/objectstore/master-tokens/"
    ));
    assert_eq!(row_of(Method::Get, "/api/v2/compute/vms/"), None);
    assert_eq!(rule_name(Method::Get, "/api/v2/compute/vms/"), None);
    assert!(!returns_secret(Method::Get, "/api/v2/compute/vms/"));
}
