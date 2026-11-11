//! `hipc hub plans`, `hipc hub subscription`, `hipc hub namespace`, and `hipc hub usage`.

use hippius::routes::hub;
use serde_json::{Value, json};

use crate::common::{
    self, Hipc, Home, MEMBER, Reply, Route, Run, Server, docker_logins, draft, hub_provision_hint,
    route, route_of,
};

const PLANS: &str = concat!(
    r#"[{"id":1,"name":"Free","price_credits":0},"#,
    r#"{"id":2,"name":"Builder","price_credits":5},"#,
    r#"{"id":3,"name":"Pro","price_credits":20}]"#
);

const SUBMITTED: &str = r#"{"submitted":true,"extrinsic_hash":"0xab"}"#;

fn plans() -> Route {
    route_of(&hub::PLANS, [], Reply::json(200, PLANS))
}

fn subscriptions(body: &str) -> Route {
    route_of(&hub::SUBSCRIPTIONS, [], Reply::json(200, body))
}

fn subscribe(server: &Server, extra: &[&str]) -> Run {
    let mut args = vec!["hub", "subscription", "subscribe"];
    args.extend(extra);
    Hipc::against(&Home::new(), server).run(&args)
}

#[test]
fn plans_need_no_token_and_are_an_envelope() {
    let server = Server::start(vec![plans()]);
    let doc = Hipc::tokenless(&Home::new(), &server)
        .run(&["hub", "plans"])
        .ok();
    assert_eq!(doc["items"][1]["name"], "Builder");
    assert_eq!(doc["next_page_token"], Value::Null);
    assert_eq!(server.only_request().header("authorization"), None);
}

#[test]
fn a_failing_catalog_is_its_error() {
    let server = Server::start(vec![route_of(
        &hub::PLANS,
        [],
        Reply::json(503, r#"{"error":"down"}"#),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["hub", "plans"])
        .fails(1, "unavailable");
    assert_eq!(
        error["message"],
        "cannot list the Hub plans: the API says `down`"
    );
}

#[test]
fn plans_with_a_membership_are_sent_without_the_header() {
    let server = Server::start(vec![plans()]);
    Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["hub", "plans"])
        .ok();
    assert_eq!(server.only_request().header("x-hippius-account"), None);
}

#[test]
fn every_registry_route_refuses_a_membership_before_sending() {
    let server = Server::start(vec![]);
    for args in [
        &["hub", "subscription", "get"][..],
        &["hub", "subscription", "subscribe", "--plan", "2", "--yes"],
        // A name is resolved by a catalog read, which a membership must not send.
        &[
            "hub",
            "subscription",
            "subscribe",
            "--plan",
            "builder",
            "--yes",
        ],
        &[
            "hub",
            "subscription",
            "subscribe",
            "--plan",
            "2",
            "--replace",
            "--yes",
        ],
        &[
            "hub",
            "subscription",
            "subscribe",
            "--plan",
            "2",
            "--dry-run",
        ],
        &[
            "hub",
            "subscription",
            "subscribe",
            "--plan",
            "builder",
            "--dry-run",
        ],
        &["hub", "subscription", "cancel", "7", "--yes"],
        &["hub", "namespace", "availability", "acme"],
        &["hub", "namespace", "create", "acme", "--reveal", "--yes"],
        &["hub", "namespace", "status"],
        &["hub", "namespace", "get"],
        &["hub", "namespace", "robot", "get"],
        &["hub", "namespace", "robot", "get", "--dry-run"],
        &["hub", "usage"],
        &["hub", "usage", "--repositories"],
        &["hub", "usage", "--dry-run"],
    ] {
        Hipc::against(&Home::new(), &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(args)
            .fails(2, "account_scope_unsupported");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn get_keeps_inactive_rows_and_large_amounts_unchanged() {
    let body = concat!(
        r#"[{"subscription_id":7,"plan_id":2,"active":false,"#,
        r#""cancelled_at":"2026-10-01T00:00:00Z","paid_per_month":99000000000000000000}]"#
    );
    let server = Server::start(vec![subscriptions(body)]);
    let run = Hipc::against(&Home::new(), &server).run(&["hub", "subscription", "get"]);
    let doc = run.ok();
    assert_eq!(doc["items"][0]["active"], false);
    assert!(
        run.stdout().contains("99000000000000000000"),
        "{}",
        run.stdout()
    );
}

#[test]
fn subscribe_by_name_in_any_case_posts_the_plan_id() {
    let server = Server::start(vec![
        plans(),
        subscriptions("[]"),
        route_of(&hub::SUBSCRIBE, [], Reply::json(201, SUBMITTED)),
    ]);
    let doc = subscribe(&server, &["--plan", "bUiLdEr", "--yes"]).ok();
    assert_eq!(doc, serde_json::from_str::<Value>(SUBMITTED).unwrap());
    assert_eq!(
        server.calls(),
        [
            "GET /api/registry/plans/",
            "GET /api/registry/subscriptions/",
            "POST /api/registry/subscribe/"
        ]
    );
    assert_eq!(server.requests()[2].json(), json!({"plan_id": 2}));
}

#[test]
fn subscribe_by_numeric_id_and_says_submitted_in_text_mode() {
    let server = Server::start(vec![
        plans(),
        subscriptions("[]"),
        route_of(&hub::SUBSCRIBE, [], Reply::json(201, SUBMITTED)),
    ]);
    let run = Hipc::against(&Home::new(), &server).run(&[
        "--output",
        "text",
        "hub",
        "subscription",
        "subscribe",
        "--plan",
        "3",
        "--yes",
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    // The whole line, so the command it names cannot go stale after a rename.
    assert_eq!(
        run.stderr().lines().next(),
        Some(
            "note: the subscribe was submitted: the plan shows as active in \
             `hipc hub subscription get` after a few minutes"
        ),
        "{}",
        run.stderr()
    );
    assert_eq!(server.requests()[2].json(), json!({"plan_id": 3}));
}

#[test]
fn an_active_free_or_inactive_plan_does_not_block_a_subscribe() {
    let rows = concat!(
        r#"[{"subscription_id":5,"plan_id":1,"active":true},"#,
        r#"{"subscription_id":6,"plan_id":3,"active":false}]"#
    );
    let server = Server::start(vec![
        plans(),
        subscriptions(rows),
        route_of(&hub::SUBSCRIBE, [], Reply::json(201, SUBMITTED)),
    ]);
    subscribe(&server, &["--plan", "pro", "--yes"]).ok();
    assert!(!server.calls().iter().any(|m| m.starts_with("DELETE")));
}

#[test]
fn an_active_paid_plan_without_replace_is_refused_before_any_write() {
    for plan_id in ["3", "99"] {
        // Plan 99 is not in the catalog, so it counts as paid.
        let rows = format!(r#"[{{"subscription_id":8,"plan_id":{plan_id},"active":true}}]"#);
        let server = Server::start(vec![plans(), subscriptions(&rows)]);
        let error =
            subscribe(&server, &["--plan", "builder", "--yes"]).fails(2, "replace_required");
        assert!(error["message"].as_str().unwrap().contains('8'));
        assert!(error["hint"].as_str().unwrap().contains("--replace"));
        assert_eq!(
            server.calls(),
            [
                "GET /api/registry/plans/",
                "GET /api/registry/subscriptions/"
            ]
        );
    }
}

#[test]
fn replace_cancels_every_active_paid_plan_then_subscribes() {
    let rows = concat!(
        r#"[{"subscription_id":8,"plan_id":3,"active":true},"#,
        r#"{"subscription_id":9,"plan_id":2,"active":true},"#,
        r#"{"subscription_id":10,"plan_id":1,"active":true}]"#
    );
    let server = Server::start(vec![
        plans(),
        subscriptions(rows),
        route_of(&hub::SUBSCRIPTION_CANCEL, [&8], Reply::json(204, "")),
        route_of(&hub::SUBSCRIPTION_CANCEL, [&9], Reply::json(204, "")),
        route_of(&hub::SUBSCRIBE, [], Reply::json(201, SUBMITTED)),
    ]);
    subscribe(&server, &["--plan", "builder", "--replace", "--yes"]).ok();
    assert_eq!(
        server.calls(),
        [
            "GET /api/registry/plans/",
            "GET /api/registry/subscriptions/",
            "DELETE /api/registry/subscriptions/8/",
            "DELETE /api/registry/subscriptions/9/",
            "POST /api/registry/subscribe/"
        ]
    );
}

#[test]
fn a_post_failing_after_a_cancel_is_a_partial_failure_naming_the_cancel() {
    let server = Server::start(vec![
        plans(),
        subscriptions(r#"[{"subscription_id":8,"plan_id":3,"active":true}]"#),
        route_of(&hub::SUBSCRIPTION_CANCEL, [&8], Reply::json(204, "")),
        route_of(
            &hub::SUBSCRIBE,
            [],
            Reply::json(402, r#"{"error":"Not enough credits"}"#),
        ),
    ]);
    let run = subscribe(&server, &["--plan", "builder", "--replace", "--yes"]);
    assert_eq!(run.code(), Some(1), "{}", run.stderr());
    assert_eq!(run.json(), json!({"cancelled": [8]}));
    let error: Value = serde_json::from_str(run.stderr()).unwrap();
    let error = &error["error"];
    assert_eq!(error["code"], "partial_failure");
    assert_eq!(error["side_effects"], "partial");
    // The step's own words are in its part, not repeated in the top message; its hint is
    // the failure's hint.
    assert_eq!(
        error["message"],
        "cancelled the subscription `8`, then the subscribe failed"
    );
    assert_eq!(error["details"][0]["step"], "subscribe");
    assert_eq!(error["details"][0]["code"], "payment_required");
    assert_eq!(error["details"][0]["http_status"], 402);
    let part = error["details"][0]["message"].as_str().unwrap();
    assert!(part.contains("Not enough credits"), "{part}");
    // The cancel keeps this run's marker, so a rerun inside its window is refused: the hint
    // waits it out. The step keeps its own hint in its part.
    assert_eq!(
        error["hint"],
        "top up the balance in the console, wait 10 minutes, then run the command again"
    );
    assert_eq!(
        error["details"][0]["hint"],
        "top up the balance in the console, then run the command again"
    );
}

#[test]
fn a_partial_failure_in_text_names_each_part_once() {
    let server = Server::start(vec![
        plans(),
        subscriptions(r#"[{"subscription_id":8,"plan_id":3,"active":true}]"#),
        route_of(&hub::SUBSCRIPTION_CANCEL, [&8], Reply::json(204, "")),
        route_of(
            &hub::SUBSCRIBE,
            [],
            Reply::json(402, r#"{"error":"Not enough credits"}"#),
        ),
    ]);
    let run = Hipc::against(&Home::new(), &server).run(&[
        "--output",
        "text",
        "hub",
        "subscription",
        "subscribe",
        "--plan",
        "builder",
        "--replace",
        "--yes",
    ]);
    assert_eq!(run.code(), Some(1), "{}", run.stderr());
    assert_eq!(
        run.stderr().matches("Not enough credits").count(),
        1,
        "{}",
        run.stderr()
    );
    assert!(
        run.stderr()
            .starts_with("error: cancelled the subscription `8`, then the subscribe failed\n"),
        "{}",
        run.stderr()
    );
}

#[test]
fn a_second_cancel_failing_is_a_partial_failure_and_nothing_is_bought() {
    let server = Server::start(vec![
        plans(),
        subscriptions(concat!(
            r#"[{"subscription_id":8,"plan_id":3,"active":true},"#,
            r#"{"subscription_id":9,"plan_id":2,"active":true}]"#
        )),
        route_of(&hub::SUBSCRIPTION_CANCEL, [&8], Reply::json(204, "")),
        route_of(
            &hub::SUBSCRIPTION_CANCEL,
            [&9],
            Reply::json(409, r#"{"error":"Already cancelling"}"#),
        ),
    ]);
    let run = subscribe(&server, &["--plan", "builder", "--replace", "--yes"]);
    assert_eq!(run.code(), Some(1), "{}", run.stderr());
    assert_eq!(run.json(), json!({"cancelled": [8]}));
    let error = run.error();
    assert_eq!(error["code"], "partial_failure");
    assert_eq!(error["details"][0]["step"], "cancel");
    assert_eq!(error["details"][0]["code"], "conflict");
    assert!(!server.calls().iter().any(|m| m.starts_with("POST")));
}

#[test]
fn the_first_cancel_failing_is_that_error_with_no_side_effects() {
    let server = Server::start(vec![
        plans(),
        subscriptions(r#"[{"subscription_id":8,"plan_id":3,"active":true}]"#),
        route_of(
            &hub::SUBSCRIPTION_CANCEL,
            [&8],
            Reply::json(409, r#"{"error":"Already cancelling"}"#),
        ),
    ]);
    let error =
        subscribe(&server, &["--plan", "builder", "--replace", "--yes"]).fails(1, "conflict");
    assert_eq!(error["side_effects"], "none");
    assert!(!server.calls().iter().any(|m| m.starts_with("POST")));
}

#[test]
fn a_failing_read_stops_the_subscribe_before_any_write() {
    let server = Server::start(vec![
        plans(),
        route_of(
            &hub::SUBSCRIPTIONS,
            [],
            Reply::json(503, r#"{"error":"down"}"#),
        ),
    ]);
    subscribe(&server, &["--plan", "builder", "--yes"]).fails(1, "unavailable");
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn an_unknown_plan_is_usage_and_lists_the_names() {
    for wanted in ["enterprise", "42"] {
        let server = Server::start(vec![plans()]);
        let error = subscribe(&server, &["--plan", wanted, "--yes"]).fails(2, "usage");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("`Free`, `Builder`, and `Pro`"),
            "{error}"
        );
        assert_eq!(server.calls(), ["GET /api/registry/plans/"]);
    }
}

#[test]
fn subscribe_refusals_send_nothing() {
    let server = Server::start(vec![]);
    let error = subscribe(&server, &["--plan", "builder"]).fails(2, "confirmation_required");
    assert!(error["message"].as_str().unwrap().contains("builder"));
    let error = subscribe(&server, &["--plan", "2", "--replace"]).fails(2, "confirmation_required");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("cancel every active paid Hub plan")
    );
    // Hub plans are paid from the balance only, so there is no `--pay-from` to choose a card
    // that would always be refused.
    for value in ["card", "credits"] {
        let error = subscribe(
            &server,
            &["--plan", "builder", "--pay-from", value, "--yes"],
        )
        .fails(2, "usage");
        assert_eq!(
            error["message"], "unexpected argument `--pay-from` found",
            "{value}"
        );
    }
    // An argv error comes before the confirmation, so it shows without `--yes`.
    subscribe(&server, &["--plan", "builder", "--pay-from", "card"]).fails(2, "usage");
    subscribe(&server, &["--yes"]).fails(2, "usage");
    assert!(server.requests().is_empty());
}

/// A subscribe's dry run is the POST, and `requests` names every request a real run sends,
/// in order: the catalog and subscription reads, the cancel `--replace` may send, then the
/// POST. A plan name is looked up only by a real run.
#[test]
fn subscribe_dry_run_lists_every_request_and_shows_the_post() {
    let server = Server::start(vec![]);
    let request = |method: &str, path: &str, body: Value| {
        json!({
            "method": method,
            "url": format!("{}/api/registry/{path}", server.url()),
            "account_header": null,
            "project_header": null,
            "billing_header": null,
            "body": body,
        })
    };
    let plans = request("GET", "plans/", Value::Null);
    let subscriptions = request("GET", "subscriptions/", Value::Null);
    // The one placeholder not written `<...>`, kept as it shipped (tenet 15).
    let cancel = request("DELETE", "subscriptions/{subscription_id}/", Value::Null);
    let post = |plan_id: &Value| request("POST", "subscribe/", json!({ "plan_id": plan_id }));
    let expected = |plan_id: Value, replace: bool| {
        let mut doc = json!({ "dry_run": true });
        let mut requests = vec![plans.clone(), subscriptions.clone()];
        if replace {
            doc["may_cancel_first"] = cancel.clone();
            requests.push(cancel.clone());
        }
        requests.push(post(&plan_id));
        doc["requests"] = Value::Array(requests);
        doc
    };
    assert_eq!(
        subscribe(&server, &["--plan", "2", "--dry-run"]).dry_ok(),
        expected(json!(2), false)
    );
    // --replace may cancel first, which the dry run cannot list without reading.
    assert_eq!(
        subscribe(&server, &["--plan", "2", "--replace", "--dry-run"]).dry_ok(),
        expected(json!(2), true)
    );
    // A name is kept as given, in its case, inside the placeholder.
    assert_eq!(
        subscribe(&server, &["--plan", "Builder", "--dry-run"]).dry_ok(),
        expected(json!("<looked up from Builder>"), false)
    );
    assert_eq!(
        subscribe(&server, &["--plan", "builder", "--replace", "--dry-run"]).dry_ok(),
        expected(json!("<looked up from builder>"), true)
    );
    // An id too large for `u64` is a name to a real run too, so it is looked up.
    assert_eq!(
        subscribe(&server, &["--plan", "99999999999999999999", "--dry-run"]).dry_ok()["requests"]
            [2]["body"],
        json!({"plan_id": "<looked up from 99999999999999999999>"})
    );
    subscribe(
        &server,
        &["--plan", "builder", "--pay-from", "card", "--dry-run"],
    )
    .fails(2, "usage");
    assert!(server.requests().is_empty());
}

/// A dry run by name needs no token, and text mode shows the requests too.
#[test]
fn subscribe_dry_run_by_name_needs_no_token_and_prints_text() {
    let server = Server::start(vec![]);
    let run = Hipc::against(&Home::new(), &server)
        .env_remove("HIPPIUS_TOKEN")
        .run(&[
            "hub",
            "subscription",
            "subscribe",
            "--plan",
            "Builder",
            "--dry-run",
            "--output",
            "text",
        ]);
    assert_eq!(run.code(), Some(0));
    let stdout = run.stdout();
    assert!(stdout.contains("<looked up from Builder>"), "{stdout}");
    assert!(stdout.contains("/api/registry/plans/"), "{stdout}");
    assert!(server.requests().is_empty());
}

#[test]
fn cancel_deletes_once_and_names_the_subscription_on_204() {
    let server = Server::start(vec![route_of(
        &hub::SUBSCRIPTION_CANCEL,
        [&7],
        Reply::json(204, ""),
    )]);
    let home = Home::new();
    Hipc::against(&home, &server)
        .run(&["hub", "subscription", "cancel", "7"])
        .fails(2, "confirmation_required");
    assert!(server.requests().is_empty());
    let doc = Hipc::against(&home, &server)
        .run(&["hub", "subscription", "cancel", "7", "--yes"])
        .ok();
    assert_eq!(
        doc,
        json!({"subscription_id": 7, "status": "cancel_submitted"})
    );
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn cancel_prints_the_server_body_when_there_is_one() {
    let server = Server::start(vec![route_of(
        &hub::SUBSCRIPTION_CANCEL,
        [&7],
        Reply::json(200, r#"{"cancelled":true,"grace_until":"2026-11-01"}"#),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "subscription", "cancel", "7", "--yes"])
        .ok();
    assert_eq!(doc["grace_until"], "2026-11-01");
}

#[test]
fn cancel_needs_a_numeric_id() {
    let server = Server::start(vec![]);
    for id in ["x", "-1", "../7"] {
        Hipc::against(&Home::new(), &server)
            .run(&["hub", "subscription", "cancel", id, "--yes"])
            .fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn availability_sends_the_name_as_the_query() {
    let server = Server::start(vec![route(
        "GET",
        "/api/registry/namespaces/check/?name=acme-1",
        Reply::json(200, r#"{"available":false,"reason":"taken"}"#),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "namespace", "availability", "acme-1"])
        .ok();
    assert_eq!(doc, json!({"available": false, "reason": "taken"}));
}

#[test]
fn bad_namespace_names_are_usage_without_a_request() {
    let server = Server::start(vec![]);
    let long = "a".repeat(64);
    for name in ["", "Acme", "a_b", "a.b", "a/b", "é", long.as_str()] {
        for args in [
            &["hub", "namespace", "availability", name][..],
            &["hub", "namespace", "create", name, "--reveal", "--yes"],
        ] {
            Hipc::against(&Home::new(), &server)
                .run(args)
                .fails(2, "usage");
        }
    }
    // A leading `-` would read as a flag in a later command line; after `--` it reaches
    // the name check, which refuses it.
    for args in [
        &["hub", "namespace", "availability", "--", "-acme"][..],
        &[
            "hub",
            "namespace",
            "create",
            "--reveal",
            "--yes",
            "--",
            "-acme",
        ],
    ] {
        let error = Hipc::against(&Home::new(), &server)
            .run(args)
            .fails(2, "usage");
        assert_eq!(
            error["message"],
            "invalid value `-acme` for `<NAMESPACE>`: a namespace name is 1 to 63 lowercase \
             letters, digits, and `-`, and does not start with `-`, such as `my-team`"
        );
    }
    assert!(server.requests().is_empty());
    let longest = "a".repeat(63);
    let server = Server::start(vec![route_of(
        &hub::NAMESPACE_CHECK,
        [],
        Reply::json(200, r#"{"available":true}"#),
    )]);
    Hipc::against(&Home::new(), &server)
        .run(&["hub", "namespace", "availability", &longest])
        .ok();
}

#[test]
fn status_is_the_status_object() {
    let server = Server::start(vec![route_of(
        &hub::NAMESPACE_STATUS,
        [],
        Reply::json(
            200,
            r#"{"status":"provisioning","namespace":"acme","robot":{"robot_secret":"s3cr3t"}}"#,
        ),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&["hub", "namespace", "status"]);
    // Only the call that issues the docker secret prints it (tenet 9).
    assert!(!run.stdout().contains("s3cr3t"), "{}", run.stdout());
    let doc = run.ok();
    assert_eq!(doc["status"], "provisioning");
    assert_eq!(doc["robot"], json!({}));
}

#[test]
fn namespace_get_strips_the_robot_secret() {
    let server = Server::start(vec![route_of(
        &hub::NAMESPACE,
        [],
        Reply::json(
            200,
            r#"{"namespace":"acme","robot_name":"robot$acme","robot_secret":"s3cr3t-robot"}"#,
        ),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&["hub", "namespace", "get"]);
    assert!(!run.stdout().contains("s3cr3t-robot"));
    assert_eq!(
        run.ok(),
        json!({"namespace": "acme", "robot_name": "robot$acme"})
    );
}

#[test]
fn namespace_get_strips_a_nested_robot_secret_too() {
    let server = Server::start(vec![route_of(
        &hub::NAMESPACE,
        [],
        Reply::json(
            200,
            concat!(
                r#"{"project":{"namespace":"acme","robot_secret":"s3cr3t-a"},"#,
                r#""projects":[{"robot_secret":"s3cr3t-b","namespace":"b"}]}"#
            ),
        ),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&["hub", "namespace", "get"]);
    assert!(!run.stdout().contains("s3cr3t"), "{}", run.stdout());
    assert_eq!(
        run.ok(),
        json!({"project": {"namespace": "acme"}, "projects": [{"namespace": "b"}]})
    );
}

/// Both namespace reads drop every spelling of a robot secret, at any depth, as the robot read
/// does, and print nothing of an answer that holds a `docker login` command.
#[test]
fn namespace_reads_strip_every_secret_spelling() {
    let answer = concat!(
        r#"{"namespace":"acme","Password":"s3cr3t-a","robot":{"secret":"s3cr3t-b","#,
        r#""TOKEN":"s3cr3t-c","docker_password":"s3cr3t-d","docker_login_cmd":"x"}}"#
    );
    for (path, words) in [
        ("/api/registry/me/", "get"),
        ("/api/registry/provision/status/", "status"),
    ] {
        let server = Server::start(vec![route("GET", path, Reply::json(200, answer))]);
        let run = Hipc::against(&Home::new(), &server).run(&["hub", "namespace", words]);
        assert!(!run.stdout().contains("s3cr3t"), "{}", run.stdout());
        assert_eq!(run.ok(), json!({"namespace": "acme", "robot": {}}));
        let login = r#"{"namespace":"acme","note":"docker login -u robot -p s3cr3t-e"}"#;
        let server = Server::start(vec![route("GET", path, Reply::json(200, login))]);
        let run = Hipc::against(&Home::new(), &server).run(&["hub", "namespace", words]);
        assert_eq!(run.stdout(), "");
        assert!(!run.stderr().contains("s3cr3t"), "{}", run.stderr());
        run.fails(1, "internal");
    }
}

/// An account with no Hub namespace gets 403 `You must call /api/registry/provision/
/// first.`, and the hint names the command that creates one. Another
/// 403 keeps no hint.
#[test]
fn namespace_get_without_a_project_hints_at_namespace_create() {
    let server = Server::start(vec![route_of(
        &hub::NAMESPACE,
        [],
        Reply::json(
            403,
            r#"{"detail":"You must call /api/registry/provision/ first."}"#,
        ),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["hub", "namespace", "get"])
        .fails(1, "permission");
    assert_eq!(error["hint"], hub_provision_hint(&server));
    // A one-shot profile is carried into the hint, so the create runs where the read did.
    let error = Hipc::against(&Home::new(), &server)
        .run(&["--profile", "work", "hub", "namespace", "get"])
        .fails(1, "permission");
    assert_eq!(
        error["hint"],
        format!(
            "run `hipc --profile work --api-url {} hub namespace create <namespace> --reveal \
             --yes` to create the account's Hub namespace",
            server.url()
        )
    );
    // Only the live sentence, whole, and only as a plain `permission`, gets the hint.
    for (body, code) in [
        (r#"{"detail":"You do not have permission."}"#, "permission"),
        (
            r#"{"detail":"Only an owner may call /api/registry/provision/."}"#,
            "permission",
        ),
        (
            r#"{"detail":"Missing grant to call /api/registry/provision/."}"#,
            "permission",
        ),
        (
            r#"{"detail":"You must call /api/registry/provision/ first.","code":"owner_only"}"#,
            "owner_only",
        ),
    ] {
        let server = Server::start(vec![route_of(&hub::NAMESPACE, [], Reply::json(403, body))]);
        let error = Hipc::against(&Home::new(), &server)
            .run(&["hub", "namespace", "get"])
            .fails(1, code);
        let hint = error["hint"].as_str().unwrap_or_default();
        assert!(!hint.contains("namespace create"), "{body}: {hint}");
    }
}

#[test]
fn namespace_get_passes_a_non_object_through() {
    let server = Server::start(vec![route_of(&hub::NAMESPACE, [], Reply::json(200, "[1]"))]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "namespace", "get"])
        .ok();
    assert_eq!(doc, serde_json::json!([1]));
}

#[test]
fn namespace_create_needs_reveal_then_yes_and_sends_nothing_without_them() {
    let server = Server::start(vec![]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["hub", "namespace", "create", "acme", "--yes"])
        .fails(2, "reveal_required");
    assert!(error["hint"].as_str().unwrap().contains("--reveal"));
    Hipc::against(&Home::new(), &server)
        .run(&["hub", "namespace", "create", "acme"])
        .fails(2, "reveal_required");
    Hipc::against(&Home::new(), &server)
        .run(&["hub", "namespace", "create", "acme", "--reveal"])
        .fails(2, "confirmation_required");
    assert!(server.requests().is_empty());
}

#[test]
fn a_bad_namespace_is_usage_before_the_reveal_and_confirmation_checks() {
    let server = Server::start(vec![]);
    Hipc::against(&Home::new(), &server)
        .run(&["hub", "namespace", "create", "Acme"])
        .fails(2, "usage");
    assert!(server.requests().is_empty());
}

#[test]
fn namespace_create_prints_the_one_time_secret_on_stdout() {
    let provision = |answer: &'static str| {
        let server = Server::start(vec![route_of(
            &hub::NAMESPACE_CREATE,
            [],
            Reply::json(201, answer),
        )]);
        let run = Hipc::against(&Home::new(), &server).run(&[
            "--output",
            "text",
            "hub",
            "namespace",
            "create",
            "acme",
            "--reveal",
            "--yes",
        ]);
        assert_eq!(run.code(), Some(0), "{}", run.stderr());
        assert!(run.stdout().contains("s3cr3t-robot"));
        assert!(!run.stderr().contains("s3cr3t-robot"));
        assert_eq!(server.only_request().json(), json!({"namespace": "acme"}));
        run
    };
    let run = provision(
        r#"{"namespace":"acme","robot_name":"robot$acme+push","robot_secret":"s3cr3t-robot"}"#,
    );
    // The note names the registry and the login a person runs, never the secret or the
    // robot name, which holds a `$` a shell would expand.
    assert_eq!(
        run.stderr(),
        "note: log in to the registry with `docker login registry.hippius.com --username \
         <robot_name> --password-stdin`, giving it `robot_secret` on stdin\n"
    );
    // The server's login command, which passes the secret, is replaced by the one `hipc`
    // builds, which reads it from stdin.
    let run = provision(concat!(
        r#"{"namespace":"acme","robot_name":"robot$acme+push","robot_secret":"s3cr3t-robot","#,
        r#""docker_login_cmd":"docker login registry.hippius.com -u robot -p s3cr3t-robot"}"#
    ));
    assert!(
        !run.stdout().contains("-p s3cr3t-robot"),
        "{}",
        run.stdout()
    );
    assert!(
        run.stdout()
            .contains("docker login registry.hippius.com -u 'robot$acme+push' --password-stdin"),
        "{}",
        run.stdout()
    );
    // Without a `robot_name` in the output, the note would name nothing to copy.
    let run = provision(r#"{"namespace":"acme","robot_secret":"s3cr3t-robot"}"#);
    assert_eq!(run.stderr(), "");
}

/// A namespace create answer with a `docker login` command outside the keys that are
/// dropped may pass the robot secret, so none of it is printed; the namespace exists, so
/// the error is `partial` and names the rotate that issues a new secret.
#[test]
fn namespace_create_refuses_an_answer_with_another_login_command() {
    let server = Server::start(vec![route_of(
        &hub::NAMESPACE_CREATE,
        [],
        Reply::json(
            201,
            r#"{"robot_name":"robot$acme+push","robot_secret":"s3cr3t-robot","help":"docker login -p s3cr3t-robot"}"#,
        ),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&[
        "hub",
        "namespace",
        "create",
        "acme",
        "--reveal",
        "--yes",
    ]);
    assert_eq!(run.stdout(), "");
    assert!(!run.stderr().contains("s3cr3t-robot"), "{}", run.stderr());
    let error = run.fails(1, "internal");
    assert_eq!(
        error["message"],
        "the Hub namespace create answer holds a `docker login` command, which may pass the \
         secret where it could be copied into a shell history, so `hipc` printed none of it; \
         the create was sent"
    );
    assert_eq!(error["side_effects"], "partial");
    assert!(
        error["hint"]
            .as_str()
            .unwrap()
            .contains("hub namespace robot rotate --reveal --yes"),
        "{error}"
    );
}

/// A secret issued under another of its names is not followed by a note that none was.
#[test]
fn namespace_create_with_a_secret_under_another_name_notes_no_missing_secret() {
    let server = Server::start(vec![route_of(
        &hub::NAMESPACE_CREATE,
        [],
        Reply::json(201, r#"{"status":"active","Password":"s3cr3t"}"#),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&[
        "--output",
        "text",
        "hub",
        "namespace",
        "create",
        "acme",
        "--reveal",
        "--yes",
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(
        !run.stderr().contains("no `robot_secret` was issued"),
        "{}",
        run.stderr()
    );
}

#[test]
fn namespace_create_without_a_secret_succeeds_and_text_mode_names_the_rotate() {
    let server = Server::start(vec![route_of(
        &hub::NAMESPACE_CREATE,
        [],
        Reply::json(202, r#"{"status":"provisioning","idempotent":true}"#),
    )]);
    let args = ["hub", "namespace", "create", "acme", "--reveal", "--yes"];
    let doc = Hipc::against(&Home::new(), &server).run(&args).ok();
    assert_eq!(doc["status"], "provisioning");
    let mut text = vec!["--output", "text"];
    text.extend(args);
    let run = Hipc::against(&Home::new(), &server).run(&text);
    assert_eq!(run.code(), Some(0));
    // The rotate repeats the origin the run went to, here `HIPPIUS_API_URL`.
    assert_eq!(
        run.stderr(),
        format!(
            "note: no `robot_secret` was issued, because the namespace already exists \
             or is pending; `hipc --api-url {} hub namespace robot rotate --reveal --yes` issues \
             one\n",
            server.url()
        )
    );
    // A profile that is not one plain word gives no command to name.
    let home = Home::new();
    home.write_config("[profiles.\"café\"]\n");
    let run = Hipc::against(&home, &server)
        .env("HIPPIUS_PROFILE", "café")
        .run(&text);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        "note: no `robot_secret` was issued, because the namespace already exists or \
         is pending\n"
    );
}

#[test]
fn namespace_create_402_hints_at_the_balance_gate_and_409_is_conflict() {
    let server = Server::start(vec![route_of(
        &hub::NAMESPACE_CREATE,
        [],
        Reply::json(402, r#"{"error":"Insufficient balance"}"#),
    )]);
    let args = ["hub", "namespace", "create", "acme", "--reveal", "--yes"];
    let error = Hipc::against(&Home::new(), &server)
        .run(&args)
        .fails(1, "payment_required");
    // The server sets the minimum, so the hint names the balance read, not an amount.
    assert_eq!(
        error["hint"],
        "a namespace needs a balance even on the Free plan, at a minimum the server sets: run \
         `hipc billing balance` to see yours, top it up with `hipc billing top-up`, then run \
         the command again"
    );
    let limit = Server::start(vec![route_of(
        &hub::NAMESPACE_CREATE,
        [],
        Reply::json(409, r#"{"error":"Project limit reached"}"#),
    )]);
    let error = Hipc::against(&Home::new(), &limit)
        .run(&args)
        .fails(1, "conflict");
    assert_eq!(
        error["message"],
        "cannot create the Hub namespace `acme`: the API says `Project limit reached`"
    );
}

#[test]
fn namespace_create_dry_run_needs_neither_reveal_nor_yes() {
    let server = Server::start(vec![]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "namespace", "create", "acme", "--dry-run"])
        .ok();
    assert_eq!(draft(&doc)["method"], "POST");
    assert_eq!(draft(&doc)["body"], json!({"namespace": "acme"}));
    for args in [
        &["hub", "namespace", "availability", "acme", "--dry-run"][..],
        &["hub", "namespace", "status", "--dry-run"],
        &["hub", "namespace", "get", "--dry-run"],
        &["hub", "subscription", "get", "--dry-run"],
        &["hub", "subscription", "cancel", "7", "--dry-run"],
        &["hub", "plans", "--dry-run"],
    ] {
        let doc = Hipc::against(&Home::new(), &server).run(args).ok();
        assert_eq!(doc["dry_run"], true, "{args:?}");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn registry_calls_need_a_token() {
    let server = Server::start(vec![]);
    for args in [
        &["hub", "subscription", "get"][..],
        &["hub", "namespace", "get"],
        &["hub", "namespace", "robot", "get"],
        &["hub", "usage"],
        &["hub", "usage", "--repositories"],
    ] {
        Hipc::tokenless(&Home::new(), &server)
            .run(args)
            .fails(1, "auth");
    }
    assert!(server.requests().is_empty());
}

fn replace_with_post(reply: Reply) -> Server {
    Server::start(vec![
        plans(),
        subscriptions(r#"[{"subscription_id":8,"plan_id":3,"active":true}]"#),
        route_of(&hub::SUBSCRIPTION_CANCEL, [&8], Reply::json(204, "")),
        route_of(&hub::SUBSCRIBE, [], reply),
    ])
}

#[test]
fn a_post_answering_5xx_after_a_cancel_is_partial_and_keeps_the_hint() {
    let server = replace_with_post(Reply::json(503, r#"{"error":"down"}"#));
    let run = subscribe(&server, &["--plan", "builder", "--replace", "--yes"]);
    assert_eq!(run.code(), Some(1), "{}", run.stderr());
    assert_eq!(run.json(), json!({"cancelled": [8]}));
    let error = run.error();
    assert_eq!(error["side_effects"], "partial");
    assert_eq!(error["details"][0]["code"], "unavailable");
    assert!(error["details"][0].get("hint").is_some());
}

#[test]
fn a_post_with_a_body_that_is_not_json_or_empty_still_names_the_cancels() {
    for body in ["<html>ok</html>", ""] {
        let server = replace_with_post(Reply::json(201, body));
        let run = subscribe(&server, &["--plan", "builder", "--replace", "--yes"]);
        assert_eq!(run.code(), Some(1), "{}", run.stderr());
        assert_eq!(run.json(), json!({"cancelled": [8]}));
        let error = run.error();
        assert_eq!(error["code"], "partial_failure");
        assert_eq!(error["side_effects"], "partial");
        assert_eq!(error["details"][0]["step"], "subscribe");
    }
}

#[test]
fn a_paginated_subscription_list_is_refused_before_any_write() {
    let server = Server::start(vec![
        plans(),
        subscriptions(concat!(
            r#"{"count":2,"next":"{base}/api/registry/subscriptions/?page=2","#,
            r#""previous":null,"results":[]}"#
        )),
    ]);
    let error = subscribe(&server, &["--plan", "builder", "--yes"]).fails(1, "internal");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("split a list into pages"),
        "{error}"
    );
    assert_eq!(server.requests().len(), 2, "no write follows");
}

#[test]
fn an_unreadable_subscription_row_is_refused_before_any_write() {
    for row in [
        r#"[{"subscription_id":"8","plan_id":3,"active":true}]"#,
        r#"[{"plan_id":3,"active":true}]"#,
        r#"[{"subscription_id":8,"plan_id":3,"active":"true"}]"#,
    ] {
        let server = Server::start(vec![plans(), subscriptions(row)]);
        let error =
            subscribe(&server, &["--plan", "builder", "--replace", "--yes"]).fails(1, "internal");
        assert!(error["hint"].as_str().unwrap().contains("subscription get"));
        assert_eq!(server.requests().len(), 2, "{row}: no cancel and no POST");
    }
    // An inactive row needs no id: it is never cancelled.
    let server = Server::start(vec![
        plans(),
        subscriptions(r#"[{"plan_id":3,"active":false}]"#),
        route_of(&hub::SUBSCRIBE, [], Reply::json(201, SUBMITTED)),
    ]);
    subscribe(&server, &["--plan", "builder", "--yes"]).ok();
}

/// A subscribe from `home`, so a marker written by one run is seen by the next.
fn subscribe_in(home: &Home, server: &Server, extra: &[&str]) -> crate::common::Run {
    let mut args = vec!["hub", "subscription", "subscribe"];
    args.extend(extra);
    Hipc::against(home, server).run(&args)
}

/// Inside the window a second subscribe is refused, and so is
/// `--replace` when the list shows no paid plan for it to cancel.
#[test]
fn a_second_subscribe_inside_the_sync_window_is_refused_even_with_replace() {
    let server = Server::start(vec![
        plans(),
        subscriptions("[]"),
        route_of(&hub::SUBSCRIBE, [], Reply::json(201, SUBMITTED)),
    ]);
    let home = Home::new();
    subscribe_in(&home, &server, &["--plan", "builder", "--yes"]).ok();
    assert!(home.config_text().contains("hub_subscribe"));
    // The first subscription is not in the list yet, but the marker sees it.
    let error =
        subscribe_in(&home, &server, &["--plan", "builder", "--yes"]).fails(2, "replace_required");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("last 10 minutes"),
        "{error}"
    );
    assert_eq!(
        error["hint"],
        "wait a few minutes, then run `hipc hub subscription get`"
    );
    let posts = |s: &Server| s.requests().iter().filter(|r| r.method == "POST").count();
    assert_eq!(posts(&server), 1);
    // `--replace` has nothing listed to cancel, so it would only charge again: it is
    // refused the same way, with the same hint, and the marker stays the first run's.
    let marker = home.config_text();
    let replace = subscribe_in(&home, &server, &["--plan", "builder", "--replace", "--yes"])
        .fails(2, "replace_required");
    assert_eq!(replace, error);
    assert_eq!(posts(&server), 1);
    assert!(!server.calls().iter().any(|m| m.starts_with("DELETE")));
    assert_eq!(home.config_text(), marker);
    // Past the window, `--replace` with nothing listed is a plain subscribe again. The
    // profile holds one marker here, so `move_marker` moves this one.
    move_marker(&home, 1);
    subscribe_in(&home, &server, &["--plan", "builder", "--replace", "--yes"]).ok();
    assert_eq!(posts(&server), 2);
    // Another profile with the same token sees this profile's marker: a second plan
    // bought from it would be charged to the same account.
    let other = [
        "hub",
        "subscription",
        "subscribe",
        "--plan",
        "builder",
        "--profile",
        "other",
        "--yes",
    ];
    let error = Hipc::against(&home, &server)
        .run(&other)
        .fails(2, "replace_required");
    assert_eq!(
        error["message"],
        "a Hub subscribe was sent from the profile `default`, which holds the same token, in \
         the last 10 minutes, and it may not show in the list yet, so `--replace` cannot \
         cancel it; subscribing again could charge twice"
    );
    assert_eq!(posts(&server), 2);
    // Another token is another identity: its marker is its own.
    Hipc::against(&home, &server)
        .env("HIPPIUS_TOKEN", "hip_another_token_value")
        .run(&other)
        .ok();
    assert_eq!(posts(&server), 3);
}

/// A server that lists `listed`, each an active paid plan, and answers each cancel and the
/// subscribe as scripted.
fn scripted(listed: &[u64], deletes: Vec<(u64, Reply)>, post: Reply) -> Server {
    let rows: Vec<String> = listed
        .iter()
        .map(|id| format!(r#"{{"subscription_id":{id},"plan_id":3,"active":true}}"#))
        .collect();
    let mut routes = vec![plans(), subscriptions(&format!("[{}]", rows.join(",")))];
    for (id, reply) in deletes {
        let path = format!("/api/registry/subscriptions/{id}/");
        routes.push(route("DELETE", &path, reply));
    }
    routes.push(route_of(&hub::SUBSCRIBE, [], post));
    Server::start(routes)
}

/// The list shows only the rows `ids`, each an active paid plan, and accepts their cancels
/// and a subscribe.
fn listing(ids: &[u64]) -> Server {
    let deletes = ids.iter().map(|id| (*id, Reply::json(204, ""))).collect();
    scripted(ids, deletes, Reply::json(201, SUBMITTED))
}

/// Rekeys the marker a run against `from` wrote to the origin of `to`, as if both runs
/// had talked to one API whose list changed between them. The key is the origin and a
/// token fingerprint, and each test server has its own port.
fn rekey_marker(home: &Home, from: &Server, to: &Server) {
    let text = home.config_text();
    assert!(
        text.contains(from.url()),
        "a run against `from` wrote a marker"
    );
    std::fs::write(home.config(), text.replace(from.url(), to.url()))
        .expect("the config file is writable");
}

/// Inside the window, a paid plan in the list that the last run did not cancel
/// is one `--replace` can cancel, so it cancels it and subscribes, as outside the window.
/// Without `--replace` that listed plan is refused by the paid-plan guard, whose hint names
/// the flag.
#[test]
fn replace_inside_the_sync_window_cancels_a_listed_paid_plan() {
    let home = Home::new();
    let empty = listing(&[]);
    subscribe_in(&home, &empty, &["--plan", "builder", "--yes"]).ok();
    // Nothing was cancelled, so the marker has no `cancels` key, which an earlier release
    // that denies unknown fields can still read.
    assert!(
        !home.config_text().contains("cancels"),
        "{}",
        home.config_text()
    );
    let listed = listing(&[8]);
    rekey_marker(&home, &empty, &listed);
    let error =
        subscribe_in(&home, &listed, &["--plan", "builder", "--yes"]).fails(2, "replace_required");
    assert!(
        error["hint"].as_str().unwrap().contains("--replace"),
        "{error}"
    );
    subscribe_in(&home, &listed, &["--plan", "builder", "--replace", "--yes"]).ok();
    assert_eq!(
        listed.calls(),
        [
            "GET /api/registry/plans/",
            "GET /api/registry/subscriptions/",
            "GET /api/registry/plans/",
            "GET /api/registry/subscriptions/",
            "DELETE /api/registry/subscriptions/8/",
            "POST /api/registry/subscribe/"
        ]
    );
}

/// After a `--replace` cancels `8` and subscribes, the list can still
/// show `8` as active until it syncs. A second `--replace` in the window that sees only that
/// row has nothing new to cancel, so it is refused before any DELETE. Once the plan bought
/// in the window shows (`9`), `--replace` cancels both and subscribes.
#[test]
fn replace_inside_the_sync_window_is_refused_when_it_sees_only_what_it_cancelled() {
    let home = Home::new();
    let first = listing(&[8]);
    subscribe_in(&home, &first, &["--plan", "builder", "--replace", "--yes"]).ok();
    assert!(
        home.config_text().contains("cancels = [8]"),
        "{}",
        home.config_text()
    );

    let stale = listing(&[8]);
    rekey_marker(&home, &first, &stale);
    let error = subscribe_in(&home, &stale, &["--plan", "builder", "--replace", "--yes"])
        .fails(2, "replace_required");
    assert_eq!(
        error["hint"],
        "wait a few minutes, then run `hipc hub subscription get`"
    );
    assert_eq!(
        stale.calls(),
        [
            "GET /api/registry/plans/",
            "GET /api/registry/subscriptions/"
        ]
    );

    let synced = listing(&[8, 9]);
    rekey_marker(&home, &stale, &synced);
    subscribe_in(&home, &synced, &["--plan", "builder", "--replace", "--yes"]).ok();
    assert_eq!(
        synced.calls()[2..],
        [
            "DELETE /api/registry/subscriptions/8/",
            "DELETE /api/registry/subscriptions/9/",
            "POST /api/registry/subscribe/"
        ]
    );
    assert!(
        home.config_text().contains("cancels = [8, 9]"),
        "{}",
        home.config_text()
    );
}

/// A marker set inside another one's window keeps that window's
/// cancels. After `--replace` cancels `8` and buys `9`, a second one sees only `9`, cancels
/// it and buys again; a third that reads an older copy of the list, still showing `8`, is
/// refused, because `8` is still in the marker.
#[test]
fn a_marker_set_inside_a_window_keeps_that_windows_cancels() {
    let home = Home::new();
    let first = listing(&[8]);
    subscribe_in(&home, &first, &["--plan", "builder", "--replace", "--yes"]).ok();
    let second = listing(&[9]);
    rekey_marker(&home, &first, &second);
    subscribe_in(&home, &second, &["--plan", "builder", "--replace", "--yes"]).ok();
    assert!(
        home.config_text().contains("cancels = [8, 9]"),
        "{}",
        home.config_text()
    );
    let older = listing(&[8]);
    rekey_marker(&home, &second, &older);
    subscribe_in(&home, &older, &["--plan", "builder", "--replace", "--yes"])
        .fails(2, "replace_required");
    assert_eq!(
        older.calls(),
        [
            "GET /api/registry/plans/",
            "GET /api/registry/subscriptions/"
        ]
    );
    // Past the window the old cancels no longer count: the marker lists this run's only.
    move_marker(&home, 1);
    subscribe_in(&home, &older, &["--plan", "builder", "--replace", "--yes"]).ok();
    assert!(
        home.config_text().contains("cancels = [8]\n"),
        "{}",
        home.config_text()
    );
}

/// A `--replace` that passes inside another run's window and then changes nothing on the
/// server (its first cancel is refused) puts back the marker it replaced, so the earlier
/// purchase, not listed yet, keeps its protection.
#[test]
fn a_replace_that_changed_nothing_puts_back_the_marker_it_replaced() {
    let home = Home::new();
    // The first marker lists a cancel, so the failing run's marker is a union, `[7, 8]`,
    // and putting back works only if it matches what was written.
    let first = listing(&[7]);
    subscribe_in(&home, &first, &["--plan", "builder", "--replace", "--yes"]).ok();
    let refusing = scripted(
        &[8],
        vec![(8, Reply::json(404, r#"{"detail":"Not found."}"#))],
        Reply::json(201, SUBMITTED),
    );
    rekey_marker(&home, &first, &refusing);
    let before = home.config_text();
    assert!(before.contains("cancels = [7]"), "{before}");
    let run = subscribe_in(
        &home,
        &refusing,
        &["--plan", "builder", "--replace", "--yes"],
    );
    assert_eq!(run.code(), Some(1), "{}", run.stderr());
    assert_eq!(
        refusing.calls()[2..],
        ["DELETE /api/registry/subscriptions/8/"]
    );
    assert_eq!(home.config_text(), before);
}

/// A `--replace` that failed after a cancel went through, or after a cancel whose outcome is
/// unknown, keeps its own marker, which lists every row it set out to cancel. Those rows can still
/// show as active, so a third `--replace` that sees only them is refused before any DELETE.
#[test]
fn a_replace_that_may_have_cancelled_keeps_its_marker() {
    let refused_post = || Reply::json(402, r#"{"error":"Not enough credits"}"#);
    let gone = || Reply::json(204, "");
    let cases = [
        (
            "the cancel went through, and the POST was refused",
            vec![8],
            vec![(8, gone())],
            refused_post(),
        ),
        (
            "the cancel may have gone through: a 5xx says nothing about it",
            vec![8],
            vec![(8, Reply::json(500, r#"{"error":"down"}"#))],
            refused_post(),
        ),
        (
            "the first cancel went through, and the second was refused",
            vec![8, 9],
            vec![
                (8, gone()),
                (9, Reply::json(404, r#"{"detail":"Not found."}"#)),
            ],
            refused_post(),
        ),
    ];
    for (case, listed, deletes, post) in cases {
        let home = Home::new();
        let empty = listing(&[]);
        subscribe_in(&home, &empty, &["--plan", "builder", "--yes"]).ok();
        let server = scripted(&listed, deletes, post);
        rekey_marker(&home, &empty, &server);
        let run = subscribe_in(&home, &server, &["--plan", "builder", "--replace", "--yes"]);
        assert_eq!(run.code(), Some(1), "{case}: {}", run.stderr());
        let cancels: Vec<String> = listed.iter().map(u64::to_string).collect();
        let cancels = format!("cancels = [{}]", cancels.join(", "));
        assert!(
            home.config_text().contains(&cancels),
            "{case}: {}",
            home.config_text()
        );
        let requests = server.calls().len();
        subscribe_in(&home, &server, &["--plan", "builder", "--replace", "--yes"])
            .fails(2, "replace_required");
        assert_eq!(
            server.calls()[requests..],
            [
                "GET /api/registry/plans/",
                "GET /api/registry/subscriptions/"
            ],
            "{case}"
        );
    }
}

/// Rewrites the `at` of the marker a real run wrote, keeping its key.
fn move_marker(home: &Home, at: u64) {
    let text = home.config_text();
    let line = text
        .lines()
        .find(|line| line.starts_with("at = "))
        .expect("a subscribe wrote a marker");
    std::fs::write(home.config(), text.replace(line, &format!("at = {at}")))
        .expect("the config file is writable");
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_secs()
}

#[test]
fn an_old_marker_does_not_block_and_a_refused_subscribe_clears_its_own() {
    let accepting = Server::start(vec![
        plans(),
        subscriptions("[]"),
        route_of(&hub::SUBSCRIBE, [], Reply::json(201, SUBMITTED)),
    ]);
    let refusing = Server::start(vec![
        plans(),
        subscriptions("[]"),
        route_of(
            &hub::SUBSCRIBE,
            [],
            Reply::json(402, r#"{"error":"Not enough credits"}"#),
        ),
    ]);
    let home = Home::new();
    // Refused, so nothing was bought: the run takes its marker back.
    subscribe_in(&home, &refusing, &["--plan", "builder", "--yes"]).fails(1, "payment_required");
    assert!(!home.config_text().contains("hub_subscribe"), "taken back");
    // An accepted run's marker, aged past the window, does not block a repeat.
    subscribe_in(&home, &accepting, &["--plan", "builder", "--yes"]).ok();
    move_marker(&home, 1);
    subscribe_in(&home, &accepting, &["--plan", "builder", "--yes"]).ok();
    // The repeat wrote a fresh marker.
    assert!(!home.config_text().contains("at = 1\n"));
}

#[test]
fn a_marker_from_a_clock_that_stepped_back_still_blocks() {
    let server = Server::start(vec![
        plans(),
        subscriptions("[]"),
        route_of(&hub::SUBSCRIBE, [], Reply::json(201, SUBMITTED)),
    ]);
    let home = Home::new();
    subscribe_in(&home, &server, &["--plan", "builder", "--yes"]).ok();
    // Up to the window ahead, waiting it out is a few minutes more.
    move_marker(&home, unix_now() + 60);
    let error =
        subscribe_in(&home, &server, &["--plan", "builder", "--yes"]).fails(2, "replace_required");
    assert_eq!(
        error["hint"],
        "wait a few minutes, then run `hipc hub subscription get`"
    );
    // Further ahead, waiting could take hours or years: the hint names the field to remove.
    move_marker(&home, unix_now() + 3600);
    let error =
        subscribe_in(&home, &server, &["--plan", "builder", "--yes"]).fails(2, "replace_required");
    assert_eq!(
        error["message"],
        "the Hub subscribe marker of this profile is dated after the clock, so it blocks a \
         subscribe until 10 minutes after that date; subscribing again could charge twice"
    );
    assert_eq!(
        error["hint"],
        format!(
            "remove the `hub_subscribe` of the profile `default` in `{}`, once `hipc hub \
             subscription get` lists the last subscribe",
            home.config().display()
        )
    );
}

#[test]
fn a_subscribe_that_may_have_landed_keeps_its_marker() {
    let server = Server::start(vec![
        plans(),
        subscriptions("[]"),
        route_of(&hub::SUBSCRIBE, [], Reply::json(503, r#"{"error":"down"}"#)),
    ]);
    let home = Home::new();
    subscribe_in(&home, &server, &["--plan", "builder", "--yes"]).fails(1, "unavailable");
    assert!(
        home.config_text().contains("hub_subscribe"),
        "a 5xx may have bought it"
    );
    subscribe_in(&home, &server, &["--plan", "builder", "--yes"]).fails(2, "replace_required");
}

#[test]
fn another_token_on_the_same_profile_is_not_blocked() {
    let server = Server::start(vec![
        plans(),
        subscriptions("[]"),
        route_of(&hub::SUBSCRIBE, [], Reply::json(201, SUBMITTED)),
    ]);
    let home = Home::new();
    subscribe_in(&home, &server, &["--plan", "builder", "--yes"]).ok();
    Hipc::against(&home, &server)
        .env("HIPPIUS_TOKEN", "tok_another_user_0123456789abcd")
        .run(&[
            "hub",
            "subscription",
            "subscribe",
            "--plan",
            "builder",
            "--yes",
        ])
        .ok();
}

#[test]
fn a_marker_that_cannot_be_written_sends_no_subscribe() {
    if !common::permission_bits_apply() {
        return;
    }
    let server = Server::start(vec![plans(), subscriptions("[]")]);
    let home = Home::new();
    let dir = home.config().parent().unwrap().to_owned();
    std::fs::create_dir_all(&dir).unwrap();
    // The owner may read but not write: the config check passes, the marker write fails.
    std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o500)).unwrap();
    let run = subscribe_in(&home, &server, &["--plan", "builder", "--yes"]);
    std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    // A directory the owner cannot write is a mode to change.
    let error = run.fails(2, "usage");
    assert_eq!(error["hint"], format!("run `chmod 700 {}`", dir.display()));
    assert!(!server.requests().iter().any(|r| r.method == "POST"));
}

/// Swagger `registry_me_robot_list` says the answer has no secret. The console reads the
/// robot secret from any of `secret`, `password`, `robot_secret`, and `token`, so all four
/// are removed at any depth in case one appears. The server's `docker login` command is
/// withheld, and `hipc` prints its own in its place.
#[test]
fn namespace_robot_prints_the_login_without_any_secret() {
    let server = Server::start(vec![route_of(
        &hub::ROBOT,
        [],
        Reply::json(
            200,
            concat!(
                r#"{"username":"robot$acme","docker_login":"docker login registry.hippius.com"#,
                r#" -u 'robot$acme'","expires_at":null,"secret":"s3cr3t-a","#,
                r#""password":"s3cr3t-b","token":"s3cr3t-c","#,
                r#""robot":{"robot_secret":"s3cr3t-d","name":"robot$acme"}}"#
            ),
        ),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&["hub", "namespace", "robot", "get"]);
    assert!(!run.stdout().contains("s3cr3t"), "{}", run.stdout());
    assert_eq!(
        run.ok(),
        json!({
            "username": "robot$acme",
            "expires_at": null,
            "robot": {"name": "robot$acme"},
            "docker_login_command":
                "docker login registry.hippius.com -u 'robot$acme' --password-stdin",
        })
    );
    let request = server.only_request();
    assert_eq!(request.method, "GET");
    assert_eq!(request.header("x-hippius-account"), None);
}

/// Every spelling a server command could pass the secret in, and a command that passes
/// none: under a command key the field is dropped whatever it holds, and `hipc` prints its
/// own command, in JSON and in text alike.
#[test]
fn namespace_robot_drops_every_command_field_the_server_sends() {
    for command in [
        json!("docker login registry.hippius.com -u r -p S3CRET"),
        json!("echo S3CRET | docker login registry.hippius.com -u r --password-stdin"),
        json!("DOCKER_PASSWORD=S3CRET docker login registry.hippius.com -u r"),
        json!(["docker", "login", "-u", "r", "-p", "S3CRET"]),
        // Base64 of `docker login -p S3CRET`: no check can read it, so the key decides.
        json!("ZG9ja2VyIGxvZ2luIC1wIFMzQ1JFVA=="),
    ] {
        let body = json!({
            "login": "robot$acme+ci",
            "docker_login": command,
            "login_cmd": command,
            "Command": command,
            "nested": {"docker_login_cmd": command, "kept": "registry.hippius.com"},
        });
        let server = Server::start(vec![route_of(
            &hub::ROBOT,
            [],
            Reply::json(200, &body.to_string()),
        )]);
        let built = "docker login registry.hippius.com -u 'robot$acme+ci' --password-stdin";
        let run = Hipc::against(&Home::new(), &server).run(&["hub", "namespace", "robot", "get"]);
        assert!(!run.stdout().contains("S3CRET"), "{}", run.stdout());
        assert_eq!(
            run.ok(),
            json!({
                "login": "robot$acme+ci",
                "nested": {"kept": "registry.hippius.com"},
                "docker_login_command": built,
            }),
            "{command}"
        );
        let run = Hipc::against(&Home::new(), &server).run(&[
            "--output",
            "text",
            "hub",
            "namespace",
            "robot",
            "get",
        ]);
        assert!(!run.stdout().contains("S3CRET"), "{}", run.stdout());
        assert!(!run.stderr().contains("S3CRET"), "{}", run.stderr());
        assert!(
            run.stdout().contains("docker_login_command"),
            "{}",
            run.stdout()
        );
        assert!(run.stdout().contains(built), "{}", run.stdout());
    }
}

/// A key that may hold the secret is dropped in any case, and so is any key that names a
/// password, such as an environment variable.
#[test]
fn namespace_robot_drops_secret_keys_in_any_case() {
    let body = json!({
        "username": "r",
        "Secret": "S3CRET",
        "TOKEN": "S3CRET",
        "Robot_Secret": "S3CRET",
        "env": {"DOCKER_PASSWORD": "S3CRET", "HOME": "/root"},
        "description": "pass -path or -project to the client",
    });
    let server = Server::start(vec![route_of(
        &hub::ROBOT,
        [],
        Reply::json(200, &body.to_string()),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&["hub", "namespace", "robot", "get"]);
    assert!(!run.stdout().contains("S3CRET"), "{}", run.stdout());
    // A word that only looks like a flag, outside a `docker login` command, prints.
    assert_eq!(
        run.ok(),
        json!({
            "username": "r",
            "env": {"HOME": "/root"},
            "description": "pass -path or -project to the client",
            "docker_login_command": "docker login registry.hippius.com -u r --password-stdin",
        })
    );
}

/// A `docker login` command anywhere else, as one string, an argv array, a string with
/// zero-width spaces, or the whole body, may pass the secret in a way no check can list,
/// so the answer is refused whole and nothing is printed.
#[test]
fn namespace_robot_refuses_any_other_docker_login_command() {
    let bodies = docker_logins("S3CRET")
        .into_iter()
        .map(|login| json!({"help": login}))
        // The whole body.
        .chain([json!("echo S3CRET | docker login -u r --password-stdin")]);
    for body in bodies {
        let server = Server::start(vec![route_of(
            &hub::ROBOT,
            [],
            Reply::json(200, &body.to_string()),
        )]);
        let run = Hipc::against(&Home::new(), &server).run(&["hub", "namespace", "robot", "get"]);
        assert_eq!(run.stdout(), "", "{body}");
        assert!(!run.stderr().contains("S3CRET"), "{}", run.stderr());
        let error = run.fails(1, "internal");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("holds a `docker login` command"),
            "{error}"
        );
    }
    // The env and base64 bodies of the review lose their command keys and print the rest.
    for body in [
        json!({"env": {"DOCKER_PASSWORD": "S3CRET"}, "command": ["docker", "login", "-u", "r"]}),
        json!({"login_cmd": "ZG9ja2VyIGxvZ2luIC1wIFMzQ1JFVA=="}),
    ] {
        let server = Server::start(vec![route_of(
            &hub::ROBOT,
            [],
            Reply::json(200, &body.to_string()),
        )]);
        let run = Hipc::against(&Home::new(), &server).run(&["hub", "namespace", "robot", "get"]);
        assert_eq!(run.code(), Some(0), "{}", run.stderr());
        assert!(!run.stdout().contains("S3CRET"), "{}", run.stdout());
        assert!(!run.stdout().contains("ZG9ja2Vy"), "{}", run.stdout());
    }
}
/// Without a robot name in the answer, `hipc` has no login to build a command for.
#[test]
fn namespace_robot_without_a_name_has_a_null_login_command() {
    let server = Server::start(vec![route_of(
        &hub::ROBOT,
        [],
        Reply::json(200, r#"{"expires_at":null,"username":7}"#),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "namespace", "robot", "get"])
        .ok();
    assert_eq!(
        doc,
        json!({"expires_at": null, "username": 7, "docker_login_command": null})
    );
}

/// A value cannot be cut out of an argv array reliably, so an answer whose strings pass a
/// password to `docker login` without naming it in one string is refused whole, with
/// nothing on stdout.
#[test]
fn namespace_robot_refuses_a_login_command_that_passes_a_password() {
    for body in [
        r#"{"argv":["docker","login","-u","r","-p","s3cr3t"]}"#,
        r#"{"argv":["docker","login","-u","r","-ps3cr3t"]}"#,
        r#"{"argv":["docker","login","-u","r","--password","s3cr3t"]}"#,
        r#"{"argv":["docker","login","-u","r","--password=s3cr3t"]}"#,
    ] {
        let server = Server::start(vec![route_of(&hub::ROBOT, [], Reply::json(200, body))]);
        let run = Hipc::against(&Home::new(), &server)
            .env("HIPPIUS_PROFILE", "work")
            .run(&["hub", "namespace", "robot", "get"]);
        assert_eq!(run.stdout(), "", "{body}");
        assert!(!run.stderr().contains("s3cr3t"), "{}", run.stderr());
        let error = run.fails(1, "internal");
        // The rotate command repeats the profile and the origin this run used.
        assert_eq!(
            error["hint"],
            format!(
                "log in with `docker login --password-stdin`, which reads the secret from \
                 stdin; `hipc --profile work --api-url {} hub namespace robot rotate --reveal \
                 --yes` issues a new one",
                server.url()
            )
        );
    }
}

/// A profile name clap would read as a flag is joined to `--profile` in the command the hint
/// names, so it runs as a value.
#[test]
fn namespace_robot_refusal_joins_a_flag_shaped_profile_to_its_flag() {
    let body = r#"{"argv":["docker","login","-u","r","-p","s3cr3t"]}"#;
    let server = Server::start(vec![route_of(&hub::ROBOT, [], Reply::json(200, body))]);
    let error = Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_PROFILE", "-work")
        .run(&["hub", "namespace", "robot", "get"])
        .fails(1, "internal");
    assert_eq!(
        error["hint"],
        format!(
            "log in with `docker login --password-stdin`, which reads the secret from stdin; \
             `hipc --profile=-work --api-url {} hub namespace robot rotate --reveal --yes` \
             issues a new one",
            server.url()
        )
    );
}
#[test]
fn namespace_robot_without_a_project_hints_at_namespace_create() {
    let server = Server::start(vec![route_of(
        &hub::ROBOT,
        [],
        Reply::json(
            403,
            r#"{"detail":"You must call /api/registry/provision/ first."}"#,
        ),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["hub", "namespace", "robot", "get"])
        .fails(1, "permission");
    assert_eq!(error["hint"], hub_provision_hint(&server));
}

#[test]
fn hub_reads_dry_run_name_one_get_and_send_nothing() {
    for (args, path) in [
        (
            &["hub", "namespace", "robot", "get"][..],
            "/api/registry/me/robot/",
        ),
        (&["hub", "usage"], "/api/registry/usage/"),
        (
            &["hub", "usage", "--repositories"],
            "/api/registry/usage/repositories/",
        ),
    ] {
        let server = Server::start(vec![]);
        let mut args = args.to_vec();
        args.push("--dry-run");
        let doc = Hipc::against(&Home::new(), &server).run(&args).dry_ok();
        assert_eq!(draft(&doc)["method"], "GET", "{args:?}");
        assert_eq!(
            draft(&doc)["url"],
            format!("{}{path}", server.url()),
            "{args:?}"
        );
        // A dry run lists its one request in `requests`, and nothing else beside it.
        assert_eq!(
            doc["requests"].as_array().map(Vec::len),
            Some(1),
            "{args:?}"
        );
        assert_eq!(
            doc.as_object().map(serde_json::Map::len),
            Some(2),
            "{args:?}"
        );
        assert!(server.requests().is_empty());
    }
}

/// The registry usage answer nests the live numbers under `live` and the 7-day history
/// under `history`; the object is printed as sent.
#[test]
fn usage_is_the_usage_object_as_sent() {
    let body = concat!(
        r#"{"live":{"storage_used_bytes":123456789012345678901,"storage_quota_bytes":10,"#,
        r#""repo_count":2,"artifact_count":null},"#,
        r#""history":[{"date":"2026-10-28","storage_used_bytes":5}]}"#
    );
    let server = Server::start(vec![route_of(&hub::USAGE, [], Reply::json(200, body))]);
    let run = Hipc::against(&Home::new(), &server).run(&["hub", "usage"]);
    // A number past `u64` keeps its digits.
    assert!(
        run.stdout().contains("123456789012345678901"),
        "{}",
        run.stdout()
    );
    let doc = run.ok();
    assert_eq!(doc["live"]["repo_count"], 2);
    assert_eq!(doc["history"][0]["date"], "2026-10-28");
    assert_eq!(server.only_request().header("x-hippius-account"), None);
}

#[test]
fn usage_without_a_project_hints_at_namespace_create() {
    for (args, path) in [
        (&["hub", "usage"][..], "/api/registry/usage/"),
        (
            &["hub", "usage", "--repositories"],
            "/api/registry/usage/repositories/",
        ),
    ] {
        let server = Server::start(vec![route(
            "GET",
            path,
            Reply::json(
                403,
                r#"{"detail":"You must call /api/registry/provision/ first."}"#,
            ),
        )]);
        let error = Hipc::against(&Home::new(), &server)
            .run(args)
            .fails(1, "permission");
        assert_eq!(error["hint"], hub_provision_hint(&server), "{args:?}");
    }
}

/// The console types the per-repository answer as DRF `{count, next, previous, results}`;
/// a bare array is read too, as every list is. A page's `count` stays beside `items`.
#[test]
fn usage_by_repository_is_an_envelope() {
    let item = json!({"name": "acme/app", "size_bytes": 42, "artifact_count": 3});
    for (body, envelope) in [
        (
            concat!(
                r#"{"count":1,"next":null,"previous":null,"#,
                r#""results":[{"name":"acme/app","size_bytes":42,"artifact_count":3}]}"#
            ),
            json!({"count": 1, "items": [item], "next_page_token": null}),
        ),
        (
            r#"[{"name":"acme/app","size_bytes":42,"artifact_count":3}]"#,
            json!({"items": [item], "next_page_token": null}),
        ),
    ] {
        let server = Server::start(vec![route_of(
            &hub::USAGE_BY_REPOSITORY,
            [],
            Reply::json(200, body),
        )]);
        let doc = Hipc::against(&Home::new(), &server)
            .run(&["hub", "usage", "--repositories"])
            .ok();
        assert_eq!(doc, envelope);
    }
}

/// Swagger lists no page parameter for the route, so `hipc` reads it whole and does not
/// print a short list as if it were complete.
#[test]
fn usage_by_repository_refuses_a_second_page() {
    let server = Server::start(vec![route_of(
        &hub::USAGE_BY_REPOSITORY,
        [],
        Reply::json(
            200,
            concat!(
                r#"{"count":2,"next":"https://x/api/registry/usage/repositories/?page=2","#,
                r#""results":[]}"#
            ),
        ),
    )]);
    Hipc::against(&Home::new(), &server)
        .run(&["hub", "usage", "--repositories"])
        .fails(1, "internal");
    assert_eq!(server.requests().len(), 1);
}

/// A run that wrote a hard-linked config file and then failed adds the split-link warning to
/// the JSON error object as `warnings`, beside `error`, so an agent sees it as text mode's
/// `warning:` line shows it to a person.
#[test]
fn a_failure_after_a_split_config_link_lists_the_warning_beside_the_error() {
    let refusing = Server::start(vec![
        plans(),
        subscriptions("[]"),
        route_of(
            &hub::SUBSCRIBE,
            [],
            Reply::json(402, r#"{"error":"Not enough credits"}"#),
        ),
    ]);
    let home = Home::new();
    home.write_config("");
    let other = home.path().join("linked-config.toml");
    std::fs::hard_link(home.config(), &other).unwrap();
    let run = subscribe_in(&home, &refusing, &["--plan", "builder", "--yes"]);
    run.fails(1, "payment_required");
    let object: Value = serde_json::from_str(run.stderr()).unwrap();
    assert_eq!(
        object["warnings"],
        json!([format!(
            "the config file `{}` had another hard link, which still holds its old contents, \
             tokens included; only this name was changed",
            home.config().display()
        )])
    );
}
