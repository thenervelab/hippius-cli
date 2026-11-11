//! `hipc drive plans` and `hipc drive subscription`.

use hippius::routes::drive;
use serde_json::json;

use crate::common::{Hipc, Home, MEMBER, Reply, Server, draft, route_of};

const PLANS: &str = concat!(
    r#"[{"code":"free","name":"Free","price_credits_monthly":0},"#,
    r#"{"code":"solo","name":"Starter","price_credits_monthly":4}]"#
);

#[test]
fn plans_need_no_token_and_are_an_envelope() {
    let server = Server::start(vec![route_of(&drive::PLANS, [], Reply::json(200, PLANS))]);
    let doc = Hipc::tokenless(&Home::new(), &server)
        .run(&["drive", "plans"])
        .ok();
    assert_eq!(doc["items"][1]["code"], "solo");
    assert_eq!(doc["next_page_token"], serde_json::Value::Null);
    assert_eq!(server.only_request().header("authorization"), None);
}

#[test]
fn every_drive_route_refuses_a_membership_before_sending() {
    let server = Server::start(vec![]);
    for args in [
        &["drive", "subscription", "get"][..],
        &[
            "drive",
            "subscription",
            "subscribe",
            "--plan",
            "solo",
            "--yes",
        ],
        &["drive", "subscription", "change", "--plan", "duo", "--yes"],
        &["drive", "subscription", "cancel", "--yes"],
    ] {
        Hipc::against(&Home::new(), &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(args)
            .fails(2, "account_scope_unsupported");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn plans_with_a_membership_are_sent_without_the_header() {
    let server = Server::start(vec![route_of(&drive::PLANS, [], Reply::json(200, PLANS))]);
    Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["drive", "plans"])
        .ok();
    assert_eq!(server.only_request().header("x-hippius-account"), None);
}

#[test]
fn get_is_the_subscription_object() {
    let server = Server::start(vec![route_of(
        &drive::SUBSCRIPTION,
        [],
        Reply::json(
            200,
            concat!(
                r#"{"active":false,"#,
                r#""free_plan":{"available":false,"unavailable_reason":"wallet_only_account"}}"#
            ),
        ),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["drive", "subscription", "get"])
        .ok();
    assert_eq!(
        doc["free_plan"]["unavailable_reason"],
        "wallet_only_account"
    );
}

#[test]
fn subscribe_on_a_pipe_needs_yes_and_sends_nothing() {
    let server = Server::start(vec![]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["drive", "subscription", "subscribe", "--plan", "solo"])
        .fails(2, "confirmation_required");
    assert!(error["message"].as_str().unwrap().contains("solo"));
    assert!(server.requests().is_empty());
}

#[test]
fn subscribe_from_credits_posts_plan_and_payment() {
    let server = Server::start(vec![route_of(
        &drive::SUBSCRIBE,
        [],
        Reply::json(201, r#"{"active":true,"plan":"duo"}"#),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&[
            "drive",
            "subscription",
            "subscribe",
            "--plan",
            "duo",
            "--yes",
        ])
        .ok();
    assert_eq!(doc["plan"], "duo");
    assert_eq!(
        server.only_request().json(),
        json!({"plan": "duo", "payment": "credits"})
    );
}

#[test]
fn subscribe_sends_the_period_only_when_given() {
    let server = Server::start(vec![route_of(
        &drive::SUBSCRIBE,
        [],
        Reply::json(201, "{}"),
    )]);
    Hipc::against(&Home::new(), &server)
        .run(&[
            "drive",
            "subscription",
            "subscribe",
            "--plan",
            "scale",
            "--period",
            "annual",
            "--yes",
        ])
        .ok();
    assert_eq!(
        server.only_request().json(),
        json!({"plan": "scale", "payment": "credits", "period": "annual"})
    );
}

#[test]
fn card_checkout_prints_the_body_and_a_note_only_in_text_mode() {
    let body = concat!(
        r#"{"intent_id":"i-1","checkout_url":"https://checkout.example/x","#,
        r#""credits":"4","amount_cents":400}"#
    );
    let server = Server::start(vec![route_of(
        &drive::SUBSCRIBE,
        [],
        Reply::json(202, body),
    )]);
    let args = [
        "drive",
        "subscription",
        "subscribe",
        "--plan",
        "solo",
        "--pay-from",
        "card",
        "--yes",
    ];
    let json_run = Hipc::against(&Home::new(), &server).run(&args);
    assert_eq!(json_run.code(), Some(0));
    assert_eq!(json_run.stderr(), "", "JSON mode has no sidecar note");
    // The server's body, plus the command that goes on with the flow.
    assert_eq!(
        json_run.json(),
        json!({
            "intent_id": "i-1",
            "checkout_url": "https://checkout.example/x",
            "credits": "4",
            "amount_cents": 400,
            "next_step": format!("hipc --api-url {} drive subscription checkout i-1", server.url())
        })
    );
    assert_eq!(server.requests()[0].json()["payment"], "card");
    let mut text_args = vec!["--output", "text"];
    text_args.extend(args);
    let text = Hipc::against(&Home::new(), &server).run(&text_args);
    assert_eq!(text.code(), Some(0));
    assert_eq!(
        text.stderr(),
        format!(
            "note: nothing has been bought yet: the plan starts once the checkout at \
             `checkout_url` in the output is paid; `hipc --api-url {} drive subscription \
             checkout i-1` shows its progress\n",
            server.url()
        )
    );
    let mut quiet = text_args.clone();
    quiet.push("--quiet");
    assert_eq!(
        Hipc::against(&Home::new(), &server).run(&quiet).stderr(),
        ""
    );
}

/// `next_step` is a command that runs as printed: it is added only for an `intent_id`
/// that `subscription checkout` accepts and that cannot be read as a flag.
#[test]
fn next_step_names_the_checkout_only_when_it_can() {
    let card = [
        "drive",
        "subscription",
        "subscribe",
        "--plan",
        "solo",
        "--pay-from",
        "card",
        "--yes",
    ];
    let run = |body: &str| {
        let server = Server::start(vec![route_of(
            &drive::SUBSCRIBE,
            [],
            Reply::json(202, body),
        )]);
        let doc = Hipc::against(&Home::new(), &server).run(&card).ok();
        // The command repeats the run's origin, so it reads the checkout where it started.
        let prefix = format!("hipc --api-url {} ", server.url());
        match doc.get("next_step").and_then(|step| step.as_str()) {
            Some(step) => {
                let mut doc = doc.clone();
                doc["next_step"] = step.strip_prefix(&prefix).unwrap().into();
                doc
            }
            None => doc,
        }
    };
    assert_eq!(run("{}"), json!({}));
    assert_eq!(run(r#"{"intent_id":7}"#), json!({"intent_id": 7}));
    let long = "a".repeat(129);
    for id in ["i 1;x", "i.1", "", "-i1", long.as_str()] {
        let body = json!({ "intent_id": id }).to_string();
        assert_eq!(run(&body), json!({ "intent_id": id }), "{id}");
    }
    assert_eq!(
        run(r#"{"intent_id":"i-1","next_step":"pay"}"#)["next_step"],
        "drive subscription checkout i-1"
    );
    // The server's own `next_step` is dropped when hipc has none to put in its place.
    assert_eq!(
        run(r#"{"intent_id":"-i1","next_step":"hipc drive subscription cancel --yes"}"#),
        json!({"intent_id": "-i1"})
    );
    let longest = "a".repeat(128);
    for id in ["i_1-A9", longest.as_str()] {
        let body = json!({ "intent_id": id }).to_string();
        assert_eq!(
            run(&body)["next_step"],
            format!("drive subscription checkout {id}")
        );
    }
    // A subscribe on the balance is bought at once, so nothing waits and nothing is added.
    let server = Server::start(vec![route_of(
        &drive::SUBSCRIBE,
        [],
        Reply::json(201, r#"{"intent_id":"i-1","status":"active"}"#),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&[
            "drive",
            "subscription",
            "subscribe",
            "--plan",
            "solo",
            "--yes",
        ])
        .ok();
    assert_eq!(doc, json!({"intent_id": "i-1", "status": "active"}));
}

#[test]
fn a_202_without_a_checkout_url_still_says_nothing_was_bought() {
    let server = Server::start(vec![route_of(
        &drive::SUBSCRIBE,
        [],
        Reply::json(202, "{}"),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&[
        "--output",
        "text",
        "drive",
        "subscription",
        "subscribe",
        "--plan",
        "solo",
        "--pay-from",
        "card",
        "--yes",
    ]);
    assert!(
        run.stderr()
            .starts_with("note: nothing has been bought yet")
    );
}

#[test]
fn subscribe_errors_map_to_their_codes() {
    let server = Server::start(vec![route_of(
        &drive::SUBSCRIBE,
        [],
        Reply::json(402, r#"{"error":"Not enough credits"}"#),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&[
            "drive",
            "subscription",
            "subscribe",
            "--plan",
            "max",
            "--yes",
        ])
        .fails(1, "payment_required");
    assert_eq!(
        error["message"],
        "cannot buy the Drive plan `max`: the API says `Not enough credits`"
    );
    assert_eq!(error["side_effects"], "none");
}

#[test]
fn change_patches_the_plan_without_payment_or_rail() {
    let server = Server::start(vec![route_of(
        &drive::SUBSCRIPTION_CHANGE,
        [],
        Reply::json(200, r#"{"plan":"max"}"#),
    )]);
    Hipc::against(&Home::new(), &server)
        .run(&["drive", "subscription", "change", "--plan", "max", "--yes"])
        .ok();
    let request = server.only_request();
    assert_eq!(request.method, "PATCH");
    assert_eq!(request.json(), json!({"plan": "max"}));
    Hipc::against(&Home::new(), &server)
        .run(&[
            "drive",
            "subscription",
            "change",
            "--plan",
            "max",
            "--period",
            "monthly",
            "--yes",
        ])
        .ok();
    assert_eq!(
        server.requests()[1].json(),
        json!({"plan": "max", "period": "monthly"})
    );
}

#[test]
fn cancel_deletes_and_needs_yes() {
    let server = Server::start(vec![route_of(
        &drive::SUBSCRIPTION_CANCEL,
        [],
        Reply::json(202, r#"{"status":"cancel_submitted"}"#),
    )]);
    let home = Home::new();
    Hipc::against(&home, &server)
        .run(&["drive", "subscription", "cancel"])
        .fails(2, "confirmation_required");
    assert!(server.requests().is_empty());
    let doc = Hipc::against(&home, &server)
        .run(&["drive", "subscription", "cancel", "--yes"])
        .ok();
    assert_eq!(doc["status"], "cancel_submitted");
}

#[test]
fn cancel_on_another_rail_or_while_confirming_maps_to_its_code() {
    let server = Server::start(vec![route_of(
        &drive::SUBSCRIPTION_CANCEL,
        [],
        Reply::json(409, r#"{"error":"Managed on another rail"}"#),
    )]);
    Hipc::against(&Home::new(), &server)
        .run(&["drive", "subscription", "cancel", "--yes"])
        .fails(1, "conflict");
    let early = Server::start(vec![route_of(
        &drive::SUBSCRIPTION_CANCEL,
        [],
        Reply::json(425, r#"{"error":"Still confirming"}"#),
    )]);
    let error = Hipc::against(&Home::new(), &early)
        .run(&["drive", "subscription", "cancel", "--yes"])
        .fails(1, "unavailable");
    assert_eq!(error["retryable"], true);
}

#[test]
fn bad_plan_period_or_rail_is_usage_without_a_request() {
    let server = Server::start(vec![]);
    for args in [
        &["drive", "subscription", "subscribe", "--yes"][..],
        &[
            "drive",
            "subscription",
            "subscribe",
            "--plan",
            "free",
            "--yes",
        ],
        &[
            "drive",
            "subscription",
            "subscribe",
            "--plan",
            "starter",
            "--yes",
        ],
        &[
            "drive",
            "subscription",
            "subscribe",
            "--plan",
            "solo",
            "--period",
            "weekly",
            "--yes",
        ],
        &[
            "drive",
            "subscription",
            "subscribe",
            "--plan",
            "solo",
            "--pay-from",
            "btc",
            "--yes",
        ],
        &["drive", "subscription", "change", "--yes"],
    ] {
        Hipc::against(&Home::new(), &server)
            .run(args)
            .fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn dry_run_shows_the_exact_body_and_sends_nothing() {
    let server = Server::start(vec![]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&[
            "drive",
            "subscription",
            "subscribe",
            "--plan",
            "solo",
            "--dry-run",
        ])
        .dry_ok();
    assert_eq!(
        doc,
        crate::common::one_request_dry_run(json!({
            "method": "POST",
            "url": format!("{}/api/drive/subscription/", server.url()),
            "account_header": null,
            "project_header": null,
            "billing_header": null,
            "body": {"plan": "solo", "payment": "credits"}
        }))
    );
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["drive", "plans", "--dry-run"])
        .dry_ok();
    assert_eq!(draft(&doc)["method"], "GET");
    assert!(server.requests().is_empty());
}

#[test]
fn subscribe_409_is_conflict_with_no_side_effects() {
    let server = Server::start(vec![route_of(
        &drive::SUBSCRIBE,
        [],
        Reply::json(409, r#"{"error":"Already subscribed"}"#),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&[
            "drive",
            "subscription",
            "subscribe",
            "--plan",
            "solo",
            "--yes",
        ])
        .fails(1, "conflict");
    assert_eq!(
        error["message"],
        "cannot buy the Drive plan `solo`: the API says `Already subscribed`"
    );
    assert_eq!(error["side_effects"], "none");
}

#[test]
fn a_membership_is_refused_before_the_confirmation_question() {
    let server = Server::start(vec![]);
    Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["drive", "subscription", "subscribe", "--plan", "solo"])
        .fails(2, "account_scope_unsupported");
    assert!(server.requests().is_empty());
}

#[test]
fn a_subscribe_503_with_retry_after_did_nothing_and_may_be_retried() {
    let unreachable =
        Reply::json(503, r#"{"error":"Chain unreachable"}"#).header("Retry-After", "4");
    let server = Server::start(vec![
        route_of(&drive::SUBSCRIBE, [], unreachable.clone()),
        route_of(&drive::SUBSCRIPTION_CHANGE, [], unreachable),
    ]);
    let subscribe = [
        "drive",
        "subscription",
        "subscribe",
        "--plan",
        "solo",
        "--yes",
    ];
    let error = Hipc::against(&Home::new(), &server)
        .run(&subscribe)
        .fails(1, "unavailable");
    assert_eq!(error["side_effects"], "none");
    assert_eq!(error["retryable"], true);
    assert_eq!(error["retry_after_ms"], 4000);
    // Only the subscribe POST: a change answering the same is still partial.
    let error = Hipc::against(&Home::new(), &server)
        .run(&["drive", "subscription", "change", "--plan", "duo", "--yes"])
        .fails(1, "unavailable");
    assert_eq!(error["side_effects"], "partial");
    assert_eq!(error["retryable"], false);
    // Without Retry-After, or with another 5xx, the purchase may have been done.
    for reply in [
        Reply::json(503, r#"{"error":"x"}"#),
        Reply::json(503, r#"{"error":"x"}"#).header("Retry-After", "soon"),
        Reply::json(502, r#"{"error":"x"}"#).header("Retry-After", "4"),
    ] {
        let other = Server::start(vec![route_of(&drive::SUBSCRIBE, [], reply)]);
        let error = Hipc::against(&Home::new(), &other)
            .run(&subscribe)
            .fails(1, "unavailable");
        assert_eq!(error["side_effects"], "partial");
        assert_eq!(error["retryable"], false);
    }
}

#[test]
fn checkout_shows_a_card_checkout_and_refuses_a_membership() {
    let server = Server::start(vec![route_of(
        &drive::CHECKOUT,
        [&"pi_3Abc-9"],
        Reply::json(
            200,
            r#"{"intent_id":"pi_3Abc-9","status":"paid","minted":true}"#,
        ),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["drive", "subscription", "checkout", "pi_3Abc-9"])
        .ok();
    assert_eq!(doc["status"], "paid");
    Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["drive", "subscription", "checkout", "pi_3Abc-9"])
        .fails(2, "account_scope_unsupported");
    let missing = Server::start(vec![route_of(
        &drive::CHECKOUT,
        [&"pi_x"],
        Reply::not_found(),
    )]);
    Hipc::against(&Home::new(), &missing)
        .run(&["drive", "subscription", "checkout", "pi_x"])
        .fails(1, "not_found");
    for bad in ["", "a/b", "../x", "a?b", &"a".repeat(129)] {
        Hipc::against(&Home::new(), &server)
            .run(&["drive", "subscription", "checkout", bad])
            .fails(2, "usage");
    }
    assert_eq!(server.requests().len(), 1);
}
