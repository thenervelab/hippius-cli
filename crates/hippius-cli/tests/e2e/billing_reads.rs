//! The billing reads: `hipc billing summary`, `usage`, `invoices list`, and
//! `auto-reload get` and `history`, and `hipc drive|s3 subscription history`. The invoice PDF is in
//! `invoices.rs`.

use serde_json::{Value, json};

use crate::common::{Hipc, Home, MEMBER, Reply, Server, draft, route};

/// A billing account the auto-reload reads take beside a membership.
const BILLING: &str = "bill_9f8e7d6c5b4a39281706";

/// The compute summary, in the shape the console reads, with a base unit amount past
/// `u64` and `f64`.
const SUMMARY: &str = concat!(
    r#"{"currency":"USD","decimals":18,"billed":true,"#,
    r#""burn_rate_per_hour":"123456789012345678901234","#,
    r#""items":[{"kind":"vm","ref":"vm_1","vm_id":"vm_1","name":"web","#,
    r#""resource":"vm:small","per_hour":"123456789012345678901234"}],"#,
    r#""balance":"12.5","runway_hours":null,"arrears":null,"arrears_since":null,"#,
    r#""enforcement":{"enabled":false,"stop_after_hours":0,"destroy_after_days":0,"#,
    r#""stop_at":null,"destroy_at":null}}"#
);

/// One month of compute usage, in the shape the console reads.
const USAGE: &str = concat!(
    r#"{"month":"2026-09","currency":"USD","decimals":18,"hash_version":1,"#,
    r#""total":"1000000000000000000","hours":[{"period":499000,"#,
    r#""start":"2026-09-01T00:00:00Z","total":"1000000000000000000","#,
    r#""usage_hash":"ab","status":"charged","lines":[]}]}"#
);

/// The auto-reload setting, in the shape of swagger `auto_reload_get`.
const AUTO_RELOAD: &str = concat!(
    r#"{"available":true,"can_edit":false,"enabled":true,"threshold":"20.00","#,
    r#""target":"50.00","monthly_cap":null,"state":"armed","#,
    r#""card":{"brand":"visa","last4":"4242","exp":"04/29"},"month_spent":"0.00"}"#
);

/// Two auto-reload attempts, in the shape of swagger `auto_reload_history`.
const RELOADS: &str = concat!(
    r#"{"results":[{"id":"6f1c0a52-3c1e-4d7e-9a43-5e0f2b9b8c11","status":"failed","#,
    r#""credits":"50.00","balance_before":"4.20","capped":false,"trigger":"schedule","#,
    r#""failure_reason":"card_declined","created_at":"2026-10-02T00:00:00Z","#,
    r#""updated_at":"2026-10-02T00:00:05Z","invoice":null},"#,
    r#"{"id":"0f8fad5b-d9cb-469f-a165-70867728950e","status":"succeeded","#,
    r#""credits":"50.00","balance_before":"4.90","capped":false,"trigger":"enabled","#,
    r#""failure_reason":null,"created_at":"2026-09-01T00:00:00Z","#,
    r#""updated_at":"2026-09-01T00:00:03Z","#,
    r#""invoice":{"id":12,"number":"HIP-0012","pdf_url":"/api/billing/invoices/12/pdf/"}}]}"#
);

/// One DRF page of invoices, with a link to the next page.
const INVOICES: &str = concat!(
    r#"{"count":3,"next":"{base}/api/billing/invoices/?page=2&page_size=2","#,
    r#""previous":null,"results":[{"id":12,"number":"HIP-0012","kind":"invoice"},"#,
    r#"{"id":11,"number":"HIP-0011","kind":"credit_note"}]}"#
);

/// One DRF page of a plan history, the last one.
const HISTORY: &str = concat!(
    r#"{"count":1,"next":null,"previous":null,"results":[{"id":"credits:7","#,
    r#""kind":"subscribe","plan":"pro","amount_credits":"10.00","provider":"credits","#,
    r#""status":"completed","created_at":"2026-09-01T00:00:00Z"}]}"#
);

#[test]
fn summary_is_the_server_body_with_every_digit() {
    let server = Server::start(vec![route(
        "GET",
        "/api/compute/billing/summary/",
        Reply::json(200, SUMMARY),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&["billing", "summary"]);
    assert_eq!(run.ok(), serde_json::from_str::<Value>(SUMMARY).unwrap());
    // The base unit amount is printed as sent, not rounded through a float.
    assert!(run.stdout().contains("\"123456789012345678901234\""));
    assert_eq!(
        server.only_request().target,
        "/api/compute/billing/summary/"
    );
}

/// The value text mode prints for `key` in a key-value block.
fn text_value<'a>(stdout: &'a str, key: &str) -> &'a str {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix("  "))
        .map_or_else(|| panic!("no `{key}` line in {stdout}"), str::trim_start)
}

/// Text mode shows the base-unit amounts in credits, the balance's unit, and says
/// which fields it changed; JSON keeps them as sent (the test above).
#[test]
fn text_mode_shows_the_summary_amounts_in_credits() {
    let server = Server::start(vec![route(
        "GET",
        "/api/compute/billing/summary/",
        Reply::json(
            200,
            &SUMMARY.replace(r#""arrears":null"#, r#""arrears":"250""#),
        ),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&["--output", "text", "billing", "summary"]);
    assert_eq!(run.code(), Some(0));
    let stdout = run.stdout();
    assert_eq!(
        text_value(stdout, "burn_rate_per_hour"),
        "123456.789012345678901234"
    );
    assert_eq!(text_value(stdout, "arrears"), "0.00000000000000025");
    assert_eq!(text_value(stdout, "balance"), "12.5");
    assert!(
        text_value(stdout, "items").contains(r#""per_hour":"123456.789012345678901234""#),
        "{stdout}"
    );
    assert_eq!(
        run.stderr(),
        "note: `burn_rate_per_hour`, `arrears`, and `per_hour` are in credits here \
         (1 credit = 1 USD); the JSON output keeps base units (10^18 per credit)\n"
    );
    let note = run.stderr().trim_end().strip_prefix("note: ").unwrap();
    crate::common::assert_voice(note);
}

/// A usage month, with one priced line, for the text-mode test.
const USAGE_LINE: &str = concat!(
    r#"{"month":"2026-09","currency":"USD","decimals":18,"hash_version":1,"#,
    r#""total":"2500000000000000000","hours":[{"period":499000,"#,
    r#""start":"2026-09-01T00:00:00Z","total":"2500000000000000000","#,
    r#""usage_hash":"ab","status":"charged","lines":[{"kind":"vm","#,
    r#""resource":"vm:small","ref":"vm_1","unit":"hour","started_at":0,"ended_at":3600,"#,
    r#""quantity":"3600","minimum_topup":0,"unit_price":"694444444444444","#,
    r#""amount":"2500000000000000000"}]}]}"#
);

#[test]
fn text_mode_shows_the_usage_amounts_in_credits_and_leaves_quantities() {
    let server = Server::start(vec![route(
        "GET",
        "/api/compute/usage/",
        Reply::json(200, USAGE_LINE),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&["--output", "text", "billing", "usage"]);
    let stdout = run.stdout();
    assert_eq!(text_value(stdout, "total"), "2.5");
    let hours = text_value(stdout, "hours");
    for shown in [
        r#""total":"2.5""#,
        r#""unit_price":"0.000694444444444444""#,
        r#""amount":"2.5""#,
        r#""quantity":"3600""#,
    ] {
        assert!(hours.contains(shown), "{shown} in {hours}");
    }
    assert!(run.stderr().starts_with(
        "note: `total`, `unit_price`, and `amount` are in credits here (1 credit = 1 USD)"
    ));
    let json = Hipc::against(&Home::new(), &server).run(&["billing", "usage"]);
    assert_eq!(
        json.ok(),
        serde_json::from_str::<Value>(USAGE_LINE).unwrap()
    );
}

/// An answer that names another scale is not in the base units swagger documents, so text
/// mode leaves it as sent, and with `--quiet` no note is printed either.
#[test]
fn text_mode_leaves_another_scale_and_quiet_drops_the_note() {
    let server = Server::start(vec![route(
        "GET",
        "/api/compute/billing/summary/",
        Reply::json(200, &SUMMARY.replace(r#""decimals":18"#, r#""decimals":6"#)),
    )]);
    let home = Home::new();
    let run = Hipc::against(&home, &server).run(&["--output", "text", "billing", "summary"]);
    assert_eq!(
        text_value(run.stdout(), "burn_rate_per_hour"),
        "123456789012345678901234"
    );
    assert_eq!(run.stderr(), "");
    let server = Server::start(vec![route(
        "GET",
        "/api/compute/billing/summary/",
        Reply::json(200, SUMMARY),
    )]);
    let quiet =
        Hipc::against(&home, &server).run(&["--output", "text", "--quiet", "billing", "summary"]);
    assert_eq!(
        text_value(quiet.stdout(), "burn_rate_per_hour"),
        "123456.789012345678901234"
    );
    assert_eq!(quiet.stderr(), "");
}

/// The note names only the fields that changed: none when no amount is an integer string
/// (`null`, or text the server wrote another way), and one field in the singular.
#[test]
fn the_credits_note_names_only_what_changed() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/compute/usage/?month=2026-08",
            Reply::json(
                200,
                r#"{"decimals":18,"total":null,"hours":[{"total":"1.5"}]}"#,
            ),
        ),
        route(
            "GET",
            "/api/compute/usage/?month=2026-09",
            Reply::json(200, r#"{"total":"5","hours":[]}"#),
        ),
    ]);
    let home = Home::new();
    let text = |month: &str| {
        Hipc::against(&home, &server)
            .run(&["--output", "text", "billing", "usage", "--month", month])
    };
    let unchanged = text("2026-08");
    assert!(
        unchanged.stdout().contains(r#"[{"total":"1.5"}]"#),
        "{}",
        unchanged.stdout()
    );
    assert_eq!(unchanged.stderr(), "");
    let one = text("2026-09");
    assert_eq!(text_value(one.stdout(), "total"), "0.000000000000000005");
    assert!(one.stderr().starts_with("note: `total` is in credits here"));
}

#[test]
fn usage_sends_the_month_only_when_given() {
    let server = Server::start(vec![route(
        "GET",
        "/api/compute/usage/",
        Reply::json(200, USAGE),
    )]);
    let home = Home::new();
    let doc = Hipc::against(&home, &server)
        .run(&["billing", "usage", "--month", "2026-09"])
        .ok();
    assert_eq!(doc["hours"][0]["status"], "charged");
    Hipc::against(&home, &server)
        .run(&["billing", "usage"])
        .ok();
    let targets: Vec<String> = server.requests().into_iter().map(|r| r.target).collect();
    assert_eq!(
        targets,
        ["/api/compute/usage/?month=2026-09", "/api/compute/usage/"]
    );
}

#[test]
fn a_month_that_is_not_yyyy_mm_is_usage_without_a_request() {
    let server = Server::start(vec![]);
    for month in [
        "2026-9",
        "2026-00",
        "2026-13",
        "2026-20",
        "26-09",
        "2026/09",
        "2026-09-01",
        "",
        "abcd-ef",
    ] {
        let error = Hipc::against(&Home::new(), &server)
            .run(&["billing", "usage", "--month", month])
            .fails(2, "usage");
        assert!(
            error["message"].as_str().unwrap().contains("YYYY-MM"),
            "{month:?}: {error}"
        );
    }
    for month in ["2026-01", "2026-10", "2026-12", "0000-09"] {
        Hipc::against(&Home::new(), &server)
            .run(&["--dry-run", "billing", "usage", "--month", month])
            .ok();
    }
    assert!(server.requests().is_empty());
}

#[test]
fn a_bad_month_from_the_server_is_validation() {
    let server = Server::start(vec![route(
        "GET",
        "/api/compute/usage/",
        Reply::json(400, r#"{"error":"Bad month"}"#),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["billing", "usage", "--month", "1999-01"])
        .fails(1, "validation");
    assert_eq!(
        error["message"],
        "cannot get the compute usage of `1999-01`: the API says `Bad month`"
    );
}

#[test]
fn auto_reload_is_the_server_body() {
    let server = Server::start(vec![route(
        "GET",
        "/api/billing/auto-reload/",
        Reply::json(200, AUTO_RELOAD),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["billing", "auto-reload", "get"])
        .ok();
    assert_eq!(doc, serde_json::from_str::<Value>(AUTO_RELOAD).unwrap());
    // The feature off answers `available: false` alone, which is printed as it is.
    let off = Server::start(vec![route(
        "GET",
        "/api/billing/auto-reload/",
        Reply::json(200, r#"{"available":false}"#),
    )]);
    let doc = Hipc::against(&Home::new(), &off)
        .run(&["billing", "auto-reload", "get"])
        .ok();
    assert_eq!(doc, json!({"available": false}));
}

/// The auto-reload attempts, newest first, as swagger `auto_reload_history` sends
/// them: `{results}`, with no pages. The document is the server's, so it claims nothing about
/// attempts past `--limit`; `--limit` is sent every time, 100 unless given, the most the
/// route returns.
#[test]
fn auto_reload_history_is_the_server_body_with_the_limit() {
    let server = Server::start(vec![route(
        "GET",
        "/api/billing/auto-reload/history/",
        Reply::json(200, RELOADS),
    )]);
    let home = Home::new();
    let doc = Hipc::against(&home, &server)
        .run(&["billing", "auto-reload", "history"])
        .ok();
    assert_eq!(doc, serde_json::from_str::<Value>(RELOADS).unwrap());
    Hipc::against(&home, &server)
        .run(&["billing", "auto-reload", "history", "--limit", "5"])
        .ok();
    let targets: Vec<String> = server.requests().into_iter().map(|r| r.target).collect();
    assert_eq!(
        targets,
        [
            "/api/billing/auto-reload/history/?limit=100",
            "/api/billing/auto-reload/history/?limit=5"
        ]
    );
}

#[test]
fn auto_reload_history_limit_is_1_to_100() {
    let server = Server::start(vec![]);
    for limit in ["0", "101", "-1", "x"] {
        Hipc::against(&Home::new(), &server)
            .run(&["billing", "auto-reload", "history", "--limit", limit])
            .fails(2, "usage");
    }
    for limit in ["1", "100"] {
        Hipc::against(&Home::new(), &server)
            .run(&[
                "--dry-run",
                "billing",
                "auto-reload",
                "history",
                "--limit",
                limit,
            ])
            .ok();
    }
    assert!(server.requests().is_empty());
}

#[test]
fn invoices_list_one_page_with_the_limit_and_the_token() {
    let server = Server::start(vec![route(
        "GET",
        "/api/billing/invoices/",
        Reply::json(200, INVOICES),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["billing", "invoices", "list", "--limit", "2"])
        .ok();
    assert_eq!(doc["items"][1]["kind"], "credit_note");
    assert_eq!(doc["next_page_token"], "2");
    // The page's `count` stays beside the items; its links are the token.
    let keys: Vec<&String> = doc.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["count", "items", "next_page_token"]);
    assert_eq!(doc["count"], 3);
    assert_eq!(
        server.only_request().target,
        "/api/billing/invoices/?page_size=2"
    );
}

#[test]
fn invoices_list_all_follows_every_page_into_one_array() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/billing/invoices/?page_size=2",
            Reply::json(200, INVOICES),
        ),
        route(
            "GET",
            "/api/billing/invoices/?page_size=2&page=2",
            Reply::json(200, r#"{"count":3,"next":null,"results":[{"id":10}]}"#),
        ),
    ]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["billing", "invoices", "list", "--limit", "2", "--all"])
        .ok();
    let ids: Vec<u64> = doc
        .as_array()
        .unwrap()
        .iter()
        .map(|invoice| invoice["id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, [12, 11, 10]);
}

#[test]
fn history_reads_each_plan_ledger_one_page_at_a_time() {
    for service in ["drive", "s3"] {
        let path = format!("/api/{service}/subscription/history/");
        let server = Server::start(vec![route("GET", &path, Reply::json(200, HISTORY))]);
        let doc = Hipc::against(&Home::new(), &server)
            .run(&[service, "subscription", "history", "--page-token", "3"])
            .ok();
        assert_eq!(doc["items"][0]["id"], "credits:7", "{service}");
        assert_eq!(doc["next_page_token"], Value::Null, "{service}");
        assert_eq!(
            server.only_request().target,
            format!("{path}?page_size=100&page=3")
        );
    }
}

/// Swagger caps the history `page_size` at 200, below the 500 of the other lists, so a
/// larger `--limit` is refused rather than silently cut.
#[test]
fn history_limit_is_at_most_200() {
    let server = Server::start(vec![]);
    for service in ["drive", "s3"] {
        let run = |limit: &str| {
            Hipc::against(&Home::new(), &server).run(&[
                service,
                "subscription",
                "history",
                "--limit",
                limit,
                "--dry-run",
            ])
        };
        for limit in ["201", "500", "0"] {
            let error = run(limit).fails(2, "usage");
            assert!(
                error["message"]
                    .as_str()
                    .is_some_and(|m| m.ends_with("is not in `1..=200`")),
                "{service} {limit}: {error}"
            );
        }
        let doc = run("200").ok();
        assert!(
            draft(&doc)["url"]
                .as_str()
                .is_some_and(|url| url.ends_with("?page_size=200")),
            "{doc}"
        );
    }
    assert!(server.requests().is_empty());
}

#[test]
fn history_all_follows_the_pages() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/s3/subscription/history/?page_size=100",
            Reply::json(
                200,
                r#"{"next":"{base}/api/s3/subscription/history/?page=2","results":[1]}"#,
            ),
        ),
        route(
            "GET",
            "/api/s3/subscription/history/?page_size=100&page=2",
            Reply::json(200, r#"{"next":null,"results":[2]}"#),
        ),
    ]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["s3", "subscription", "history", "--all"])
        .ok();
    assert_eq!(doc, json!([1, 2]));
}

/// The reads the console scopes to the selected account send `X-Hippius-Account` there.
#[test]
fn a_membership_sends_the_account_header_on_the_scoped_reads() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/compute/billing/summary/",
            Reply::json(200, SUMMARY),
        ),
        route("GET", "/api/compute/usage/", Reply::json(200, USAGE)),
        route("GET", "/api/billing/invoices/", Reply::json(200, INVOICES)),
        route(
            "GET",
            "/api/billing/auto-reload/",
            Reply::json(200, AUTO_RELOAD),
        ),
        route(
            "GET",
            "/api/billing/auto-reload/history/",
            Reply::json(200, RELOADS),
        ),
        route(
            "GET",
            "/api/s3/subscription/history/",
            Reply::json(200, HISTORY),
        ),
    ]);
    // The auto-reload reads take the account only beside a billing account, as the console
    // sends them; without one they are refused (`billing_writes`).
    for (args, billing) in [
        (&["billing", "summary"][..], None),
        (&["billing", "usage"], None),
        (&["billing", "invoices", "list"], None),
        (&["billing", "auto-reload", "get"], Some(BILLING)),
        (&["billing", "auto-reload", "history"], Some(BILLING)),
        (&["s3", "subscription", "history"], None),
    ] {
        let mut hipc = Hipc::against(&Home::new(), &server).env("HIPPIUS_ACCOUNT", MEMBER);
        if let Some(billing) = billing {
            hipc = hipc.env("HIPPIUS_BILLING", billing);
        }
        hipc.run(args).ok();
    }
    let requests = server.requests();
    assert_eq!(requests.len(), 6);
    for request in requests {
        assert_eq!(
            request.header("x-hippius-account"),
            Some(MEMBER),
            "{}",
            request.target
        );
        let billing = request.path().starts_with("/api/billing/auto-reload/");
        assert_eq!(
            request.header("x-hippius-billing"),
            billing.then_some(BILLING),
            "{}",
            request.target
        );
    }
}

#[test]
fn the_drive_history_refuses_a_membership_before_sending() {
    let server = Server::start(vec![]);
    for args in [
        &["drive", "subscription", "history"][..],
        &["--dry-run", "drive", "subscription", "history"],
    ] {
        let error = Hipc::against(&Home::new(), &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(args)
            .fails(2, "account_scope_unsupported");
        assert!(error["hint"].as_str().unwrap().contains("--account self"));
    }
    assert!(server.requests().is_empty());
}

#[test]
fn debug_prints_the_request_and_the_status_and_nothing_else() {
    let server = Server::start(vec![route(
        "GET",
        "/api/compute/usage/",
        Reply::json(200, USAGE),
    )]);
    let run = Hipc::against(&Home::new(), &server)
        .run(&["--debug", "billing", "usage", "--month", "2026-09"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    // The query value is redacted in the debug line, as in every URL hipc quotes.
    assert_eq!(
        run.stderr(),
        format!(
            "debug: GET {}/api/compute/usage/?month=<redacted>\ndebug: -> 200\n",
            server.url()
        )
    );
    assert_eq!(run.json()["month"], "2026-09");
}

#[test]
fn each_read_maps_the_server_errors_to_their_codes() {
    let reads: [(&[&str], &str); 7] = [
        (&["billing", "summary"], "/api/compute/billing/summary/"),
        (&["billing", "usage"], "/api/compute/usage/"),
        (&["billing", "invoices", "list"], "/api/billing/invoices/"),
        (
            &["billing", "auto-reload", "get"],
            "/api/billing/auto-reload/",
        ),
        (
            &["billing", "auto-reload", "history"],
            "/api/billing/auto-reload/history/",
        ),
        (
            &["drive", "subscription", "history"],
            "/api/drive/subscription/history/",
        ),
        (
            &["s3", "subscription", "history"],
            "/api/s3/subscription/history/",
        ),
    ];
    let answers = [
        (401, r#"{"detail":"Invalid token."}"#, "auth"),
        (
            403,
            r#"{"code":"billing_required","error":"no"}"#,
            "billing_required",
        ),
        (403, r#"{"detail":"no"}"#, "permission"),
        (404, r#"{"detail":"Not found."}"#, "not_found"),
        (429, "{}", "rate_limited"),
        (500, "{}", "internal"),
        (503, "{}", "unavailable"),
    ];
    for (args, path) in reads {
        for (status, body, code) in answers {
            let server = Server::start(vec![route("GET", path, Reply::json(status, body))]);
            let error = Hipc::against(&Home::new(), &server)
                .run(args)
                .fails(1, code);
            assert_eq!(error["http_status"], status, "{args:?}");
            assert_eq!(error["side_effects"], "none", "{args:?}");
        }
    }
}

#[test]
fn an_answer_that_is_not_json_is_internal() {
    let server = Server::start(vec![route(
        "GET",
        "/api/compute/billing/summary/",
        Reply::json(200, "<html>"),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["billing", "summary"])
        .fails(1, "internal");
    assert_eq!(
        error["message"],
        "cannot get the compute burn rate: the API answered 200 with a body that is not JSON"
    );
}

#[test]
fn the_reads_need_a_token_and_dry_run_sends_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    for args in [
        &["billing", "summary"][..],
        &["billing", "usage"],
        &["billing", "invoices", "list"],
        &["billing", "auto-reload", "get"],
        &["billing", "auto-reload", "history"],
        &["drive", "subscription", "history"],
        &["s3", "subscription", "history"],
    ] {
        Hipc::new(&home)
            .env("HIPPIUS_API_URL", server.url())
            .run(args)
            .fails(1, "auth");
        let mut dry = vec!["--dry-run"];
        dry.extend_from_slice(args);
        let doc = Hipc::against(&home, &server).run(&dry).ok();
        assert_eq!(draft(&doc)["method"], "GET", "{args:?}");
    }
    assert!(server.requests().is_empty());
}
