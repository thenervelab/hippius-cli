//! `hipc billing balance`, `subscriptions`, `transactions`, and `stripe`.

use hippius::routes::{billing, drive, hub, s3};
use serde_json::json;

use crate::common::{Hipc, Home, MEMBER, Reply, Server, draft, route, route_of};

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
    let error: serde_json::Value = serde_json::from_str(run.stderr()).unwrap();
    assert_eq!(error["error"]["code"], "partial_failure");
    assert_eq!(
        error["error"]["details"],
        json!([{
            "service": "s3",
            "http_status": 503,
            "code": "unavailable",
            "message": "cannot get the `s3` plans: the API says `Chain unreachable`",
            "hint": "wait a few minutes, then run the command again"
        }])
    );
    assert_eq!(error["error"]["side_effects"], "none");
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
        let error: serde_json::Value = serde_json::from_str(run.stderr()).unwrap();
        assert_eq!(error["error"]["code"], "partial_failure");
        assert_eq!(error["error"]["details"][0]["service"], "drive");
        assert_eq!(error["error"]["details"][0]["code"], "internal");
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
        .run(&["billing", "transactions"])
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
        .run(&["billing", "transactions"])
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
        .run(&["billing", "transactions", "--all"])
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
        .run(&["billing", "transactions", "--all"])
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
        .run(&["billing", "transactions"])
        .fails(1, "internal");
}

#[test]
fn a_bad_page_token_is_usage_even_without_a_token() {
    let server = Server::start(vec![]);
    Hipc::tokenless(&Home::new(), &server)
        .run(&["billing", "transactions", "--page-token", "abc"])
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
        .run(&["billing", "transactions", "--all"])
        .fails(1, "internal");
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn bad_page_arguments_are_usage_without_a_request() {
    let server = Server::start(vec![]);
    let home = Home::new();
    for args in [
        &["billing", "transactions", "--page-token", "abc"][..],
        &["billing", "transactions", "--page-token", ""],
        &["billing", "transactions", "--page-token", "-1"],
        &["billing", "transactions", "--limit", "0"],
        &["billing", "transactions", "--limit", "501"],
        &["billing", "transactions", "--all", "--page-token", "2"],
    ] {
        Hipc::against(&home, &server).run(args).fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn transactions_and_stripe_refuse_a_membership_locally() {
    let server = Server::start(vec![]);
    for args in [&["billing", "transactions"][..], &["billing", "stripe"]] {
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
        .run(&["billing", "transactions", "--all", "--dry-run"])
        .ok();
    assert_eq!(doc["dry_run"], true);
    assert!(server.requests().is_empty());
}
