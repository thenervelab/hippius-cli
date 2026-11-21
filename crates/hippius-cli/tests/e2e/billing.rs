//! `hipc billing balance`, `subscriptions`, `transactions`, `stripe`, and `top-up-status`.

use hippius::routes::{billing, drive, hub, s3};
use serde_json::json;

use crate::common::{BILLING, Hipc, Home, MEMBER, PROJECT, Reply, Server, draft, route, route_of};

const DRIVE: &str = r#"{"active":true,"plan":"solo","plan_name":"Starter"}"#;
const S3: &str = r#"{"active":false,"mode":"pay_as_you_go","used_bytes":0}"#;
const HUB: &str =
    r#"[{"subscription_id":7,"plan_id":2,"active":true,"paid_per_month":50000000000000000000}]"#;
const STRIPE: &str = r#"{"subscription":null,"has_subscription":false}"#;

fn all_legs() -> Vec<crate::common::Route> {
    vec![
        route_of(&drive::SUBSCRIPTION, [], Reply::json(200, DRIVE)),
        route_of(&s3::SUBSCRIPTION, [], Reply::json(200, S3)),
        route_of(&hub::SUBSCRIPTIONS, [], Reply::json(200, HUB)),
        route_of(&billing::STRIPE_SUBSCRIPTION, [], Reply::json(200, STRIPE)),
    ]
}

#[test]
fn balance_is_the_server_body_and_takes_the_account_header() {
    let server = Server::start(vec![route_of(
        &billing::BALANCE,
        [],
        Reply::json(
            200,
            r#"{"balance":"12.500000000000000001","last_updated":"t"}"#,
        ),
    )]);
    let home = Home::new();
    let run = Hipc::against(&home, &server).run(&["billing", "balance"]);
    assert_eq!(
        run.stdout(),
        "{\"balance\":\"12.500000000000000001\",\"last_updated\":\"t\"}\n"
    );
    assert_eq!(server.requests()[0].header("x-hippius-account"), None);
    Hipc::against(&home, &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["billing", "balance"])
        .ok();
    assert_eq!(
        server.requests()[1].header("x-hippius-account"),
        Some(MEMBER)
    );
}

#[test]
fn subscriptions_lists_every_plan_source_in_order() {
    let server = Server::start(all_legs());
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["billing", "subscriptions"])
        .ok();
    let services: Vec<&str> = doc["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["service"].as_str().unwrap())
        .collect();
    assert_eq!(services, ["drive", "s3", "hub", "stripe"]);
    assert_eq!(doc["items"][1]["subscription"]["mode"], "pay_as_you_go");
    assert_eq!(doc["next_page_token"], serde_json::Value::Null);
    // The raw chain number keeps every digit, though it is above `u64::MAX`.
    let raw = Hipc::against(&Home::new(), &server).run(&["billing", "subscriptions"]);
    assert!(raw.stdout().contains("50000000000000000000"));
}

#[test]
fn one_failed_leg_still_prints_the_others_and_exits_partial_failure() {
    let server = Server::start(vec![
        route_of(&drive::SUBSCRIPTION, [], Reply::json(200, DRIVE)),
        route_of(
            &s3::SUBSCRIPTION,
            [],
            Reply::json(503, r#"{"error":"Chain unreachable"}"#),
        ),
        route_of(&hub::SUBSCRIPTIONS, [], Reply::json(200, HUB)),
        route_of(&billing::STRIPE_SUBSCRIPTION, [], Reply::json(200, STRIPE)),
    ]);
    let run = Hipc::against(&Home::new(), &server).run(&["billing", "subscriptions"]);
    assert_eq!(run.code(), Some(1));
    let done = run.json();
    assert_eq!(done["partial_failure"], true);
    assert_eq!(done["items"].as_array().unwrap().len(), 3);
    let error = run.error();
    assert_eq!(error["code"], "partial_failure");
    assert_eq!(
        error["details"],
        json!([{
            "service": "s3",
            "http_status": 503,
            "code": "unavailable",
            "message": "cannot get the `s3` plans: the API says `Chain unreachable`",
            "hint": "wait a few minutes, then run the command again"
        }])
    );
    assert_eq!(error["side_effects"], "none");
}

#[test]
fn every_leg_failing_is_the_first_legs_error_not_a_partial_failure() {
    let server = Server::start(vec![
        route_of(
            &drive::SUBSCRIPTION,
            [],
            Reply::json(402, r#"{"error":"first"}"#),
        ),
        route_of(&s3::SUBSCRIPTION, [], Reply::json(503, "{}")),
        route_of(&hub::SUBSCRIPTIONS, [], Reply::json(409, "{}")),
        route_of(&billing::STRIPE_SUBSCRIPTION, [], Reply::json(500, "{}")),
    ]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["billing", "subscriptions"])
        .fails(1, "payment_required");
    assert_eq!(
        error["message"],
        "cannot get the `drive` plans: the API says `first`"
    );
    assert_eq!(server.requests().len(), 4);
}

#[test]
fn an_unreadable_or_empty_leg_is_one_failure_among_the_rest() {
    for body in ["<html>", "", "null"] {
        let server = Server::start(vec![
            route_of(&drive::SUBSCRIPTION, [], Reply::json(200, body)),
            route_of(&s3::SUBSCRIPTION, [], Reply::json(200, S3)),
            route_of(&hub::SUBSCRIPTIONS, [], Reply::json(200, HUB)),
            route_of(&billing::STRIPE_SUBSCRIPTION, [], Reply::json(200, STRIPE)),
        ]);
        let run = Hipc::against(&Home::new(), &server).run(&["billing", "subscriptions"]);
        assert_eq!(run.code(), Some(1), "{body:?}");
        assert_eq!(run.json()["items"].as_array().unwrap().len(), 3);
        let error = run.error();
        assert_eq!(error["code"], "partial_failure");
        assert_eq!(error["details"][0]["service"], "drive");
        assert_eq!(error["details"][0]["code"], "internal");
    }
}

#[test]
fn subscriptions_refuses_a_membership_before_sending() {
    let server = Server::start(all_legs());
    let error = Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["billing", "subscriptions"])
        .fails(2, "account_scope_unsupported");
    assert!(error["hint"].as_str().unwrap().contains("--account self"));
    // The route class of the first leg that cannot act in another account decides, as
    // every other command's does (tenet 12), under `--dry-run` too.
    assert_eq!(
        error["message"],
        format!(
            "`/api/drive/subscription/` cannot act in another account, and the selected \
             account is `{MEMBER}` from `HIPPIUS_ACCOUNT`"
        )
    );
    let dry_run = Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["billing", "subscriptions", "--dry-run"])
        .fails(2, "account_scope_unsupported");
    assert_eq!(dry_run, error);
    assert!(server.requests().is_empty());
}

#[test]
fn subscriptions_dry_run_lists_all_four_requests() {
    let server = Server::start(vec![]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["billing", "subscriptions", "--dry-run"])
        .ok();
    let urls: Vec<String> = doc["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["url"].as_str().unwrap().replace(server.url(), ""))
        .collect();
    assert_eq!(
        urls,
        [
            "/api/drive/subscription/",
            "/api/s3/subscription/",
            "/api/registry/subscriptions/",
            "/api/billing/stripe/active-subscription/"
        ]
    );
    assert!(server.requests().is_empty());
}

#[test]
fn transactions_fetch_one_page_with_the_limit_and_the_token() {
    let server = Server::start(vec![route_of(
        &billing::TRANSACTIONS,
        [],
        Reply::json(
            200,
            concat!(
                r#"{"count":5,"next":"{base}/api/billing/transactions/?page=3&page_size=2","#,
                r#""results":[{"id":"a"}]}"#
            ),
        ),
    )]);
    let home = Home::new();
    let doc = Hipc::against(&home, &server)
        .run(&[
            "billing",
            "transactions",
            "list",
            "--limit",
            "2",
            "--page-token",
            "2",
        ])
        .ok();
    assert_eq!(
        doc,
        json!({"count": 5, "items": [{"id": "a"}], "next_page_token": "3"})
    );
    assert_eq!(
        server.only_request().target,
        "/api/billing/transactions/?page_size=2&page=2"
    );
}

#[test]
fn transactions_default_to_one_page_of_100_and_never_follow_on_their_own() {
    let server = Server::start(vec![route_of(
        &billing::TRANSACTIONS,
        [],
        Reply::json(
            200,
            r#"{"next":"{base}/api/billing/transactions/?page=2","results":[1]}"#,
        ),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["billing", "transactions", "list"])
        .ok();
    assert_eq!(doc["next_page_token"], "2");
    assert_eq!(
        server.only_request().target,
        "/api/billing/transactions/?page_size=100"
    );
}

#[test]
fn transactions_as_a_bare_array_have_no_next_page() {
    let server = Server::start(vec![route_of(
        &billing::TRANSACTIONS,
        [],
        Reply::json(200, r#"[{"id":"a"},{"id":"b"}]"#),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["billing", "transactions", "list"])
        .ok();
    assert_eq!(doc["items"].as_array().unwrap().len(), 2);
    assert_eq!(doc["next_page_token"], serde_json::Value::Null);
}

#[test]
fn all_follows_every_page_into_one_array() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/billing/transactions/?page_size=100",
            Reply::json(
                200,
                concat!(
                    r#"{"next":"{base}/api/billing/transactions/?page=2&page_size=100","#,
                    r#""results":[1,2]}"#
                ),
            ),
        ),
        route(
            "GET",
            "/api/billing/transactions/?page_size=100&page=2",
            Reply::json(200, r#"{"next":null,"results":[3]}"#),
        ),
    ]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["billing", "transactions", "list", "--all"])
        .ok();
    assert_eq!(doc, json!([1, 2, 3]));
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn all_refuses_a_page_loop_and_a_next_link_without_a_page_number() {
    let looping = Server::start(vec![
        route(
            "GET",
            "/api/billing/transactions/?page_size=100",
            Reply::json(
                200,
                r#"{"next":"{base}/api/billing/transactions/?page=2","results":[1]}"#,
            ),
        ),
        route(
            "GET",
            "/api/billing/transactions/?page_size=100&page=2",
            Reply::json(
                200,
                r#"{"next":"{base}/api/billing/transactions/?page=2","results":[2]}"#,
            ),
        ),
    ]);
    let error = Hipc::against(&Home::new(), &looping)
        .run(&["billing", "transactions", "list", "--all"])
        .fails(1, "internal");
    assert!(error["message"].as_str().unwrap().contains("leads back to"));
    let cursor = Server::start(vec![route_of(
        &billing::TRANSACTIONS,
        [],
        Reply::json(
            200,
            r#"{"next":"{base}/api/billing/transactions/?cursor=x","results":[1]}"#,
        ),
    )]);
    Hipc::against(&Home::new(), &cursor)
        .run(&["billing", "transactions", "list"])
        .fails(1, "internal");
}

#[test]
fn a_bad_page_token_is_usage_even_without_a_token() {
    let server = Server::start(vec![]);
    Hipc::tokenless(&Home::new(), &server)
        .run(&["billing", "transactions", "list", "--page-token", "abc"])
        .fails(2, "usage");
}

#[test]
fn all_catches_a_next_link_back_to_the_first_page_before_refetching_it() {
    let server = Server::start(vec![route_of(
        &billing::TRANSACTIONS,
        [],
        Reply::json(
            200,
            r#"{"next":"{base}/api/billing/transactions/?page=1","results":[1]}"#,
        ),
    )]);
    Hipc::against(&Home::new(), &server)
        .run(&["billing", "transactions", "list", "--all"])
        .fails(1, "internal");
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn bad_page_arguments_are_usage_without_a_request() {
    let server = Server::start(vec![]);
    let home = Home::new();
    for args in [
        &["billing", "transactions", "list", "--page-token", "abc"][..],
        &["billing", "transactions", "list", "--page-token", ""],
        &["billing", "transactions", "list", "--page-token", "-1"],
        &["billing", "transactions", "list", "--limit", "0"],
        &["billing", "transactions", "list", "--limit", "501"],
        &[
            "billing",
            "transactions",
            "list",
            "--all",
            "--page-token",
            "2",
        ],
    ] {
        Hipc::against(&home, &server).run(args).fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

/// The query of the URL a dry run of `hipc billing transactions list` with `flags` would
/// read; nothing is sent.
fn transactions_query(flags: &[&str]) -> String {
    let server = Server::start(vec![]);
    let args = [&["--dry-run", "billing", "transactions", "list"], flags].concat();
    let doc = Hipc::against(&Home::new(), &server).run(&args).ok();
    assert_eq!(doc["dry_run"], true);
    assert!(server.requests().is_empty());
    let url = draft(&doc)["url"]
        .as_str()
        .expect("a dry run shows the URL");
    let (_, query) = url
        .split_once("/api/billing/transactions/")
        .expect("the list route");
    query.to_owned()
}

/// `search` goes in the query only when `--search` is given, under the name swagger's
/// `billing_transactions_list` spells, percent-encoded, before the page size and the page.
#[test]
fn transactions_send_search_only_when_given() {
    assert_eq!(transactions_query(&[]), "?page_size=100");
    let cases: &[(&[&str], &str)] = &[
        (&["--search", "stripe"], "search=stripe"),
        (
            &["--search", "a b&c#d=e+f/g?h%i é"],
            "search=a%20b%26c%23d%3De%2Bf%2Fg%3Fh%25i%20%C3%A9",
        ),
        (&["--search", " padded "], "search=%20padded%20"),
        (&["--search=-amount"], "search=-amount"),
    ];
    for (flags, query) in cases {
        assert_eq!(
            transactions_query(flags),
            format!("?{query}&page_size=100"),
            "{flags:?}"
        );
    }
    assert_eq!(
        transactions_query(&["--search", "x", "--limit", "5", "--page-token", "3"]),
        "?search=x&page_size=5&page=3"
    );
    assert_eq!(
        transactions_query(&["--search", "x", "--all"]),
        "?search=x&page_size=100"
    );
}

/// The search is kept on each page `--all` reads and on the page `--page-token` names.
#[test]
fn transactions_keep_the_search_on_every_page() {
    let filters = "search=top%20up";
    let first = format!("/api/billing/transactions/?{filters}&page_size=1");
    let second = format!("/api/billing/transactions/?{filters}&page_size=1&page=2");
    let next = format!("{{base}}/api/billing/transactions/?{filters}&page=2");
    let server = Server::start(vec![
        route(
            "GET",
            &first,
            Reply::json(
                200,
                &format!(r#"{{"next":"{next}","results":[{{"id":"a"}}]}}"#),
            ),
        ),
        route(
            "GET",
            &second,
            Reply::json(200, r#"{"next":null,"results":[{"id":"b"}]}"#),
        ),
    ]);
    let home = Home::new();
    let flags = [
        "billing",
        "transactions",
        "list",
        "--search",
        "top up",
        "--limit",
        "1",
    ];
    let doc = Hipc::against(&home, &server)
        .run(&[&flags[..], &["--all"]].concat())
        .ok();
    assert_eq!(doc, json!([{"id": "a"}, {"id": "b"}]));
    let doc = Hipc::against(&home, &server)
        .run(&[&flags[..], &["--page-token", "2"]].concat())
        .ok();
    assert_eq!(doc["items"], json!([{"id": "b"}]));
    let targets: Vec<String> = server.requests().into_iter().map(|r| r.target).collect();
    assert_eq!(targets, [first, second.clone(), second]);
}

/// A blank search, or one holding a control or invisible character, is usage, and nothing is
/// sent: a blank one would list every transaction.
#[test]
fn transactions_refuse_a_blank_or_unprintable_search_and_send_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let refused: &[(&[&str], &str)] = &[
        (
            &["--search", ""],
            "a search text is not blank; leave out --search to list every transaction",
        ),
        (&["--search", " \t "], "a search text is not blank"),
        (
            &["--search", "two\nlines"],
            "a search text holds no control character",
        ),
        (
            &["--search", "a\u{202e}b"],
            "a search text holds no invisible character",
        ),
    ];
    for (flags, says) in refused {
        let args = [&["billing", "transactions", "list"], *flags].concat();
        let error = Hipc::against(&home, &server).run(&args).fails(2, "usage");
        let message = error["message"].as_str().unwrap();
        assert!(message.contains(says), "{flags:?}: {message}");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn transactions_and_stripe_refuse_a_membership_locally() {
    let server = Server::start(vec![]);
    for args in [
        &["billing", "transactions", "list"][..],
        &["billing", "stripe"],
    ] {
        Hipc::against(&Home::new(), &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(args)
            .fails(2, "account_scope_unsupported");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn stripe_is_the_server_body() {
    let server = Server::start(vec![route_of(
        &billing::STRIPE_SUBSCRIPTION,
        [],
        Reply::json(200, STRIPE),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["billing", "stripe"])
        .ok();
    assert_eq!(
        doc,
        json!({"subscription": null, "has_subscription": false})
    );
}

#[test]
fn billing_needs_a_token() {
    let server = Server::start(vec![]);
    Hipc::tokenless(&Home::new(), &server)
        .run(&["billing", "balance"])
        .fails(1, "auth");
    assert!(server.requests().is_empty());
}

#[test]
fn balance_dry_run_sends_nothing() {
    let server = Server::start(vec![]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["billing", "balance", "--dry-run"])
        .ok();
    assert_eq!(
        draft(&doc)["url"],
        format!("{}/api/billing/credits/balance/", server.url())
    );
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["billing", "transactions", "list", "--all", "--dry-run"])
        .ok();
    assert_eq!(doc["dry_run"], true);
    assert!(server.requests().is_empty());
}

/// A transaction id and the card top-up status route as it is asked about that one.
const TRANSACTION_ID: &str = "3f1c2b9e-8a1d-4c55-9a51-2f0d6c1e7b42";
const STATUS_TARGET: &str =
    "/api/billing/stripe/checkout-status/?transaction_id=3f1c2b9e-8a1d-4c55-9a51-2f0d6c1e7b42";
const TRANSACTION: &str = concat!(
    r#"{"id":"3f1c2b9e-8a1d-4c55-9a51-2f0d6c1e7b42","payment_type":"stripe","amount":50.0,"#,
    r#""currency":"USD","credits":50,"credit_amount":null,"status":"completed","#,
    r#""created_at":"2026-11-01T10:00:00Z","completed_at":"2026-11-01T10:01:00Z","#,
    r#""substrate_block_number":123,"substrate_success":true}"#
);
const TOP_UP_STATUS: &str = concat!(
    r#"{"transaction_id":"3f1c2b9e-8a1d-4c55-9a51-2f0d6c1e7b42","item_id":"credits","#,
    r#""payment_rail":"card","status":"paid","minted":true,"value":"50.00","#,
    r#""currency":"USD","first_purchase":false}"#
);

#[test]
fn transactions_get_prints_the_one_transaction_as_sent() {
    let server = Server::start(vec![route_of(
        &billing::TRANSACTION,
        [&TRANSACTION_ID],
        Reply::json(200, TRANSACTION),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["billing", "transactions", "get", TRANSACTION_ID])
        .ok();
    assert_eq!(
        doc,
        serde_json::from_str::<serde_json::Value>(TRANSACTION).unwrap()
    );
    let request = server.only_request();
    assert_eq!(request.method, "GET");
    assert_eq!(
        request.target,
        "/api/billing/transactions/3f1c2b9e-8a1d-4c55-9a51-2f0d6c1e7b42/"
    );
    assert_eq!(request.header("x-hippius-account"), None);
    let shown = Hipc::against(&Home::new(), &server).run(&[
        "--output",
        "text",
        "billing",
        "transactions",
        "get",
        TRANSACTION_ID,
    ]);
    assert_eq!(shown.code(), Some(0), "{}", shown.stderr());
    assert!(shown.stdout().contains("completed"), "{}", shown.stdout());
}

#[test]
fn transactions_get_maps_the_errors_and_names_the_list_on_a_404() {
    for (reply, code) in [
        (Reply::not_found(), "not_found"),
        (Reply::json(401, r#"{"detail":"Invalid token."}"#), "auth"),
        (Reply::json(403, r#"{"detail":"no"}"#), "permission"),
        (Reply::json(500, "{}"), "internal"),
    ] {
        let server = Server::start(vec![route_of(
            &billing::TRANSACTION,
            [&TRANSACTION_ID],
            reply,
        )]);
        let error = Hipc::against(&Home::new(), &server)
            .run(&["billing", "transactions", "get", TRANSACTION_ID])
            .fails(1, code);
        assert_eq!(error["side_effects"], "none", "{code}");
        if code == "not_found" {
            assert_eq!(
                error["hint"],
                "run `hipc billing transactions list` to see what exists"
            );
        }
    }
}

#[test]
fn transactions_get_refuses_an_id_that_is_not_a_uuid_without_a_request() {
    let server = Server::start(vec![]);
    for bad in ["42", "../x", "", "3f1c2b9e-8a1d-4c55-9a51-2f0d6c1e7b4"] {
        let error = Hipc::tokenless(&Home::new(), &server)
            .run(&["billing", "transactions", "get", bad])
            .fails(2, "usage");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("a transaction id is a UUID"),
            "{bad:?}: {error}"
        );
    }
    assert!(server.requests().is_empty());
}

#[test]
fn transactions_get_refuses_a_membership_and_a_billing_account_locally() {
    let server = Server::start(vec![]);
    let args = ["billing", "transactions", "get", TRANSACTION_ID];
    for dry in [&[][..], &["--dry-run"]] {
        let argv = [dry, &args[..]].concat();
        Hipc::against(&Home::new(), &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&argv)
            .fails(2, "account_scope_unsupported");
        Hipc::against(&Home::new(), &server)
            .env("HIPPIUS_BILLING", BILLING)
            .run(&argv)
            .fails(2, "billing_scope_unsupported");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn top_up_status_reads_the_card_top_up_by_its_transaction_id() {
    let server = Server::start(vec![route(
        "GET",
        STATUS_TARGET,
        Reply::json(200, TOP_UP_STATUS),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["billing", "top-up-status", TRANSACTION_ID])
        .ok();
    assert_eq!(
        doc,
        serde_json::from_str::<serde_json::Value>(TOP_UP_STATUS).unwrap()
    );
    let request = server.only_request();
    assert_eq!(request.method, "GET");
    assert_eq!(request.target, STATUS_TARGET);
    assert_eq!(request.header("x-hippius-account"), None);
    assert_eq!(request.header("x-hippius-billing"), None);
    let shown = Hipc::against(&Home::new(), &server).run(&[
        "--output",
        "text",
        "billing",
        "top-up-status",
        TRANSACTION_ID,
    ]);
    assert_eq!(shown.code(), Some(0), "{}", shown.stderr());
    assert!(shown.stdout().contains("paid"), "{}", shown.stdout());
}

/// Swagger: the route finds only a top-up of the billing account the request acts on, so the
/// billing account goes with it, and a member, who tops up only a billing account, reads one
/// only beside it.
#[test]
fn top_up_status_takes_the_billing_account_and_a_member_needs_one() {
    let server = Server::start(vec![route(
        "GET",
        STATUS_TARGET,
        Reply::json(200, TOP_UP_STATUS),
    )]);
    Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_BILLING", BILLING)
        .run(&["billing", "top-up-status", TRANSACTION_ID])
        .ok();
    Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .env("HIPPIUS_BILLING", BILLING)
        .run(&["billing", "top-up-status", TRANSACTION_ID])
        .ok();
    let headers: Vec<(Option<String>, Option<String>)> = server
        .requests()
        .iter()
        .map(|r| {
            (
                r.header("x-hippius-account").map(str::to_owned),
                r.header("x-hippius-billing").map(str::to_owned),
            )
        })
        .collect();
    assert_eq!(
        headers,
        [
            (None, Some(BILLING.to_owned())),
            (Some(MEMBER.to_owned()), Some(BILLING.to_owned())),
        ]
    );
    for dry in [&[][..], &["--dry-run"]] {
        let argv = [dry, &["billing", "top-up-status", TRANSACTION_ID][..]].concat();
        let error = Hipc::against(&Home::new(), &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&argv)
            .fails(2, "account_scope_unsupported");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .starts_with("a top-up status read in another account needs a billing account"),
            "{error}"
        );
        Hipc::against(&Home::new(), &server)
            .env("HIPPIUS_PROJECT", PROJECT)
            .run(&argv)
            .fails(2, "project_scope_unsupported");
    }
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn top_up_status_maps_the_documented_errors() {
    for (status, body, code) in [
        (404, r#"{"detail":"Not found."}"#, "not_found"),
        (
            400,
            r#"{"error":"transaction_id or session_id is required"}"#,
            "validation",
        ),
        (
            403,
            r#"{"code":"billing_required","error":"no"}"#,
            "billing_required",
        ),
        (503, "{}", "unavailable"),
    ] {
        let server = Server::start(vec![route("GET", STATUS_TARGET, Reply::json(status, body))]);
        let error = Hipc::against(&Home::new(), &server)
            .run(&["billing", "top-up-status", TRANSACTION_ID])
            .fails(1, code);
        assert_eq!(error["http_status"], status);
        assert_eq!(error["side_effects"], "none", "{code}");
    }
}

#[test]
fn top_up_status_refuses_an_id_that_is_not_a_uuid_and_needs_a_token() {
    let server = Server::start(vec![]);
    for bad in ["cs_test_a1b2c3", "", "x&session_id=y"] {
        Hipc::against(&Home::new(), &server)
            .run(&["billing", "top-up-status", bad])
            .fails(2, "usage");
    }
    Hipc::tokenless(&Home::new(), &server)
        .run(&["billing", "top-up-status", TRANSACTION_ID])
        .fails(1, "auth");
    assert!(server.requests().is_empty());
}

#[test]
fn the_new_reads_dry_run_their_one_get_without_a_token() {
    let server = Server::start(vec![]);
    for (args, path) in [
        (
            &["billing", "transactions", "get", TRANSACTION_ID][..],
            "/api/billing/transactions/3f1c2b9e-8a1d-4c55-9a51-2f0d6c1e7b42/",
        ),
        (&["billing", "top-up-status", TRANSACTION_ID], STATUS_TARGET),
    ] {
        let argv = [&["--dry-run"][..], args].concat();
        let doc = Hipc::tokenless(&Home::new(), &server).run(&argv).ok();
        assert_eq!(doc["dry_run"], true);
        assert_eq!(doc["requests"].as_array().unwrap().len(), 1, "{args:?}");
        assert_eq!(draft(&doc)["method"], "GET");
        assert_eq!(draft(&doc)["url"], format!("{}{path}", server.url()));
        assert_eq!(doc["confirmation"], false);
    }
    assert!(server.requests().is_empty());
}
