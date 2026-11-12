//! `hipc hub events list` and `hipc hub models`: the namespace's audit events, read
//! whole, and the model index, which needs no token and pages by `total`.

use hippius::routes::hub;
use serde_json::{Value, json};

use crate::common::{Hipc, Home, MEMBER, Reply, Server, TOKEN, draft, route_of};

/// A model row in the shape the Hub's model pages read.
const MODEL: &str = concat!(
    r#"{"project":"acme","repo":"llama","digest":"sha256:0a1b","primary_tag":"v1","#,
    r#""format":"gguf","total_size_bytes":12345678901234567890,"is_public":true}"#
);

fn models_page(page: u64, page_size: u64, total: u64, rows: usize) -> String {
    let rows = vec![MODEL; rows].join(",");
    format!(r#"{{"page":{page},"page_size":{page_size},"total":{total},"results":[{rows}]}}"#)
}

/// A page of one model's versions: the index's paging, with the items under `artifacts` and
/// the model named beside them.
fn versions_page(page: u64, page_size: u64, total: u64, rows: usize) -> String {
    let rows = vec![MODEL; rows].join(",");
    format!(
        concat!(
            r#"{{"project":"acme","repo":"llama","page":{},"page_size":{},"#,
            r#""total":{},"artifacts":[{}]}}"#,
        ),
        page, page_size, total, rows
    )
}

#[test]
fn events_list_is_read_whole_as_an_envelope() {
    let server = Server::start(vec![route_of(
        &hub::EVENTS,
        [],
        Reply::json(200, r#"[{"id":1,"type":"push","repository":"acme/app"}]"#),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "events", "list"])
        .ok();
    assert_eq!(
        doc,
        json!({
            "items": [{"id": 1, "type": "push", "repository": "acme/app"}],
            "next_page_token": null,
        })
    );
    assert!(server.only_request().header("authorization").is_some());
    // A `next` link is a list this command would cut short.
    let server = Server::start(vec![route_of(
        &hub::EVENTS,
        [],
        Reply::json(
            200,
            r#"{"count":2,"next":"http://x/api/registry/events/?page=2","results":[{"id":1}]}"#,
        ),
    )]);
    Hipc::against(&Home::new(), &server)
        .run(&["hub", "events", "list"])
        .fails(1, "internal");
}

#[test]
fn events_list_without_a_project_hints_at_provision() {
    let server = Server::start(vec![route_of(
        &hub::EVENTS,
        [],
        Reply::json(
            403,
            r#"{"detail":"You must call /api/registry/provision/ first."}"#,
        ),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["hub", "events", "list"])
        .fails(1, "permission");
    assert!(
        error["hint"]
            .as_str()
            .unwrap()
            .contains("hub namespace create <namespace> --reveal --yes`"),
        "{error}"
    );
}

/// One page, with its `page`, `page_size`, and `total` beside the items, a large number kept
/// as the server wrote it, and the next page from `total`.
#[test]
fn models_list_prints_one_page_and_the_next_from_the_total() {
    let server = Server::start(vec![route_of(
        &hub::MODELS,
        [],
        Reply::json(200, &models_page(1, 2, 5, 2)),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&["hub", "models", "list", "--limit", "2"]);
    let doc = run.ok();
    assert_eq!(doc["next_page_token"], "2");
    assert_eq!(doc["total"], 5);
    assert_eq!(doc["items"].as_array().unwrap().len(), 2);
    assert!(
        run.stdout().contains("12345678901234567890"),
        "{}",
        run.stdout()
    );
    assert_eq!(server.only_request().target, "/api/models/?page_size=2");
}

/// The model index is public and its rows are written by other accounts, so text mode
/// escapes a control character in a row rather than let it reach the terminal (tenet 15).
#[test]
fn models_list_in_text_escapes_the_control_characters_of_a_row() {
    let row = r#"{"project":"acme","repo":"l\u001b]0;owned\u0007m","primary_tag":"v\u001b[2K1","format":"g\u0007guf"}"#;
    let server = Server::start(vec![route_of(
        &hub::MODELS,
        [],
        Reply::json(
            200,
            &format!(r#"{{"page":1,"page_size":100,"total":1,"results":[{row}]}}"#),
        ),
    )]);
    let run =
        Hipc::against(&Home::new(), &server).run(&["--output", "text", "hub", "models", "list"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    let shown = run.stdout();
    assert!(!shown.contains(['\u{1b}', '\u{7}']), "{shown}");
    for escaped in [r"l\u{1b}]0;owned\u{7}m", r"v\u{1b}[2K1", r"g\u{7}guf"] {
        assert!(shown.contains(escaped), "{escaped}: {shown}");
    }
}

#[test]
fn models_list_last_page_has_no_next() {
    for (body, args) in [
        // Exactly full.
        (
            models_page(2, 2, 4, 2),
            &["--page-token", "2", "--limit", "2"][..],
        ),
        // Short.
        (
            models_page(3, 2, 5, 1),
            &["--page-token", "3", "--limit", "2"],
        ),
        // An empty page past the end, as `total` puts every item on the pages before it.
        (
            models_page(26, 2, 50, 0),
            &["--page-token", "26", "--limit", "2"],
        ),
        // An empty last page of an empty list.
        (models_page(1, 2, 0, 0), &["--limit", "2"]),
    ] {
        let server = Server::start(vec![route_of(&hub::MODELS, [], Reply::json(200, &body))]);
        let mut argv = vec!["hub", "models", "list"];
        argv.extend(args);
        let doc = Hipc::against(&Home::new(), &server).run(&argv).ok();
        assert_eq!(doc["next_page_token"], Value::Null, "{body}");
    }
}

/// `--all` follows the pages `total` implies and prints one array.
#[test]
fn models_list_all_follows_the_total() {
    let server = Server::start(vec![route_each_page()]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "models", "list", "--all", "--limit", "2"])
        .ok();
    assert_eq!(doc.as_array().unwrap().len(), 5);
    let paths: Vec<String> = server.requests().iter().map(|r| r.target.clone()).collect();
    assert_eq!(
        paths,
        [
            "/api/models/?page_size=2",
            "/api/models/?page_size=2&page=2",
            "/api/models/?page_size=2&page=3",
        ]
    );
}

fn route_each_page() -> crate::common::Route {
    crate::common::route_each_of(
        &hub::MODELS,
        [],
        vec![
            Reply::json(200, &models_page(1, 2, 5, 2)),
            Reply::json(200, &models_page(2, 2, 5, 2)),
            Reply::json(200, &models_page(3, 2, 5, 1)),
        ],
    )
}

/// The three numbers are read as leniently as a list paged by `next` reads `page`: a
/// decimal string or a float without a fraction is the number it spells.
#[test]
fn models_list_reads_its_numbers_as_strings_or_floats() {
    let body = format!(r#"{{"page":"2","page_size":2.0,"total":"5","results":[{MODEL},{MODEL}]}}"#);
    let server = Server::start(vec![route_of(&hub::MODELS, [], Reply::json(200, &body))]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "models", "list", "--page-token", "2", "--limit", "2"])
        .ok();
    assert_eq!(doc["next_page_token"], "3");
}

/// A `next` link wins over the total, and a page without the three numbers is refused rather
/// than read as the last one.
#[test]
fn models_list_reads_a_next_link_and_refuses_a_page_it_cannot_place() {
    let body = format!(r#"{{"next":"http://x/api/models/?page=4","results":[{MODEL}]}}"#);
    let server = Server::start(vec![route_of(&hub::MODELS, [], Reply::json(200, &body))]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "models", "list"])
        .ok();
    assert_eq!(doc["next_page_token"], "4");
    for body in [
        format!(r#"{{"results":[{MODEL}]}}"#),
        format!(r#"{{"page":1,"page_size":2,"results":[{MODEL}]}}"#),
        format!(r#"{{"page":"x","page_size":2,"total":5,"results":[{MODEL}]}}"#),
        format!(r#"{{"page":1.5,"page_size":2,"total":5,"results":[{MODEL}]}}"#),
        format!("[{MODEL}]"),
    ] {
        let server = Server::start(vec![route_of(&hub::MODELS, [], Reply::json(200, &body))]);
        let error = Hipc::against(&Home::new(), &server)
            .run(&["hub", "models", "list"])
            .fails(1, "internal");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("whether more pages exist is unknown"),
            "{error}"
        );
    }
}

/// A page of size 0 with items would hand out a next page forever, a page other than the
/// one asked for would hand out the same next page forever, and an empty page before the
/// end `total` sets would end the list early, so all three are refused.
#[test]
fn models_list_refuses_a_page_size_of_0_and_a_page_it_did_not_ask_for() {
    let cases: [(&[&str], String, &str); 8] = [
        (
            &[],
            models_page(1, 0, 5, 1),
            "a page of items with a `page_size` of 0",
        ),
        (
            &["--page-token", "2"],
            models_page(1, 2, 5, 2),
            "answered page `1` to a request for page `2`",
        ),
        (
            &[],
            models_page(2, 2, 5, 2),
            "answered page `2` to a request for page `1`",
        ),
        // A `next` link of its own does not excuse a page other than the one asked for.
        (
            &["--page-token", "2"],
            models_page(1, 2, 5, 2).replace(
                r#"{"page""#,
                r#"{"next":"http://x/api/models/?page=2","page""#,
            ),
            "answered page `1` to a request for page `2`",
        ),
        // A hole: page 9 of 2 items each holds items 17 and 18 of 50, yet it is empty.
        (
            &["--page-token", "9", "--limit", "2"],
            models_page(9, 2, 50, 0),
            "answered page `9` empty though its `total` of `50`",
        ),
        (&[], models_page(1, 2, 3, 0), "answered page `1` empty"),
        // The last page: page 2 of 2 items each holds item 3 of 3, yet it is empty.
        (
            &["--page-token", "2", "--limit", "2"],
            models_page(2, 2, 3, 0),
            "answered page `2` empty though its `total` of `3`",
        ),
        (
            &["--limit", "10"],
            models_page(1, 10, 5, 0),
            "answered page `1` empty though its `total` of `5`",
        ),
    ];
    for (flags, body, message) in cases {
        let server = Server::start(vec![route_of(&hub::MODELS, [], Reply::json(200, &body))]);
        let error = Hipc::against(&Home::new(), &server)
            .run(&[&["hub", "models", "list"][..], flags].concat())
            .fails(1, "internal");
        assert!(
            error["message"].as_str().unwrap().contains(message),
            "{error}"
        );
        // A page other than the one a `--page-token` asked for most likely means the list
        // changed since the token was printed, so the hint says to start over; without a
        // token, or for a size of 0 or a hole, the server is at fault: the bug-report hint.
        let start_over = "run again without `--page-token`, as the list may have changed \
                          while it was read";
        if message.contains("to a request for page") && !flags.is_empty() {
            assert_eq!(error["hint"], start_over, "{error}");
        } else {
            assert!(
                error["hint"].as_str().unwrap().contains("--debug"),
                "{error}"
            );
        }
    }
    // A page of size 0 with no items is the empty last page, not an error.
    let server = Server::start(vec![route_of(
        &hub::MODELS,
        [],
        Reply::json(200, &models_page(1, 0, 0, 0)),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "models", "list"])
        .ok();
    assert_eq!(doc["next_page_token"], Value::Null);
}

/// Without `--reference`, a model's versions page as the index does, under `artifacts`, so
/// one page names the next from `total` and `--all` follows them.
#[test]
fn models_get_pages_its_versions_by_total() {
    let pages = || {
        crate::common::route_each_of(
            &hub::MODEL,
            [&"acme/llama"],
            vec![
                Reply::json(200, &versions_page(1, 2, 3, 2)),
                Reply::json(200, &versions_page(2, 2, 3, 1)),
            ],
        )
    };
    let server = Server::start(vec![pages()]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "models", "get", "acme/llama", "--limit", "2"])
        .ok();
    assert_eq!(doc["items"].as_array().unwrap().len(), 2);
    assert_eq!(doc["next_page_token"], "2");
    assert_eq!(
        server.only_request().target,
        "/api/models/acme/llama/?page_size=2"
    );
    let server = Server::start(vec![pages()]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&[
            "hub",
            "models",
            "get",
            "acme/llama",
            "--all",
            "--limit",
            "2",
        ])
        .ok();
    assert_eq!(doc.as_array().unwrap().len(), 3);
    let paths: Vec<String> = server.requests().iter().map(|r| r.target.clone()).collect();
    assert_eq!(
        paths,
        [
            "/api/models/acme/llama/?page_size=2",
            "/api/models/acme/llama/?page_size=2&page=2",
        ]
    );
    // An answer without `artifacts` is not read as an empty list.
    let server = Server::start(vec![route_of(
        &hub::MODEL,
        [&"acme/llama"],
        Reply::json(200, r#"{"page":1,"page_size":2,"total":0}"#),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["hub", "models", "get", "acme/llama"])
        .fails(1, "internal");
    assert_eq!(
        error["message"],
        "the API answered a list request with something other than a list"
    );
}

/// The index needs no token: without one nothing is sent in `Authorization`, and with one it
/// goes along, so the account's own private models show too.
#[test]
fn models_need_no_token_and_send_one_when_configured() {
    let routes = || {
        vec![
            route_of(
                &hub::MODELS,
                [],
                Reply::json(200, &models_page(1, 100, 1, 1)),
            ),
            route_of(
                &hub::MODEL_FORMATS,
                [],
                Reply::json(
                    200,
                    r#"{"formats":["gguf"],"architectures":[],"quantizations":[]}"#,
                ),
            ),
            route_of(
                &hub::MODEL,
                [&"acme/llama"],
                Reply::json(200, &versions_page(1, 100, 0, 0)),
            ),
            route_of(&hub::MODEL, [&"acme/llama/v1"], Reply::json(200, MODEL)),
        ]
    };
    let commands: [&[&str]; 4] = [
        &["hub", "models", "list"],
        &["hub", "models", "formats"],
        &["hub", "models", "get", "acme/llama"],
        &["hub", "models", "get", "acme/llama", "--reference", "v1"],
    ];
    for args in commands {
        let server = Server::start(routes());
        Hipc::tokenless(&Home::new(), &server).run(args).ok();
        assert_eq!(
            server.only_request().header("authorization"),
            None,
            "{args:?}"
        );
        let server = Server::start(routes());
        Hipc::against(&Home::new(), &server).run(args).ok();
        assert_eq!(
            server.only_request().header("authorization"),
            Some(format!("Token {TOKEN}").as_str()),
            "{args:?}"
        );
    }
}

#[test]
fn models_formats_and_a_version_print_the_answer_as_sent() {
    let formats = r#"{"formats":["gguf","onnx"],"architectures":["llama"],"quantizations":[]}"#;
    let versions = versions_page(1, 50, 1, 1);
    let server = Server::start(vec![
        route_of(&hub::MODEL_FORMATS, [], Reply::json(200, formats)),
        route_of(&hub::MODEL, [&"acme/llama"], Reply::json(200, &versions)),
        route_of(
            &hub::MODEL,
            [&"acme/llama/sha256:0a1b"],
            Reply::json(200, MODEL),
        ),
    ]);
    let home = Home::new();
    let doc = Hipc::against(&home, &server)
        .run(&["hub", "models", "formats"])
        .ok();
    assert_eq!(doc, serde_json::from_str::<Value>(formats).unwrap());
    let doc = Hipc::against(&home, &server)
        .run(&["hub", "models", "get", "acme/llama"])
        .ok();
    // The versions print as a list, their paging beside the items.
    assert_eq!(doc["total"], 1);
    assert_eq!(doc["items"][0]["repo"], "llama");
    assert_eq!(doc["next_page_token"], Value::Null);
    let run = Hipc::against(&home, &server).run(&[
        "hub",
        "models",
        "get",
        "acme/llama",
        "--reference",
        "sha256:0a1b",
    ]);
    assert_eq!(run.ok()["digest"], "sha256:0a1b");
    assert!(run.stdout().contains("12345678901234567890"));
}

#[test]
fn a_missing_model_names_where_to_look() {
    let server = Server::start(vec![
        route_of(&hub::MODEL, [&"acme/gone"], Reply::not_found()),
        route_of(&hub::MODEL, [&"acme/llama/v9"], Reply::not_found()),
    ]);
    let home = Home::new();
    let error = Hipc::against(&home, &server)
        .run(&["hub", "models", "get", "acme/gone"])
        .fails(1, "not_found");
    assert_eq!(
        error["hint"],
        "run `hipc hub models list` to see what exists"
    );
    let error = Hipc::against(&home, &server)
        .run(&["hub", "models", "get", "acme/llama", "--reference", "v9"])
        .fails(1, "not_found");
    assert_eq!(
        error["hint"],
        "run `hipc hub models get acme/llama` to see what exists"
    );
}

/// A model is exactly namespace/repository, each a safe path segment, and a reference is a
/// tag or a digest; anything else is `usage` with nothing sent.
#[test]
fn bad_model_names_and_references_are_usage() {
    let server = Server::start(vec![]);
    for args in [
        &["hub", "models", "get", "llama"][..],
        &["hub", "models", "get", "acme/llama/extra"],
        &["hub", "models", "get", "acme/"],
        &["hub", "models", "get", "/llama"],
        &["hub", "models", "get", "../llama"],
        &["hub", "models", "get", "acme/.."],
        &["hub", "models", "get", "acme/./"],
        &["hub", "models", "get", "acme/ll ama"],
        &["hub", "models", "get", "acme/llama?x=1"],
        &["hub", "models", "get", "acme/llama", "--reference", "v1/x"],
        &["hub", "models", "get", "acme/llama", "--reference", ".v1"],
        &[
            "hub",
            "models",
            "get",
            "acme/llama",
            "--reference",
            "v1",
            "--all",
        ],
        &[
            "hub",
            "models",
            "get",
            "acme/llama",
            "--reference",
            "v1",
            "--limit",
            "2",
        ],
        &[
            "hub",
            "models",
            "get",
            "acme/llama",
            "--reference",
            "v1",
            "--page-token",
            "2",
        ],
        &["hub", "models", "list", "--limit", "0"],
        &["hub", "models", "list", "--page-token", "x"],
        &["hub", "models", "formats", "--limit", "2"],
        &["hub", "events", "list", "--all"],
    ] {
        let error = Hipc::against(&Home::new(), &server)
            .run(args)
            .fails(2, "usage");
        if args[2] == "get" && !args.contains(&"--reference") {
            assert!(
                error["message"]
                    .as_str()
                    .unwrap()
                    .contains("a model is named namespace/repository"),
                "{error}"
            );
        }
    }
    assert!(server.requests().is_empty());
    // Dots and dashes inside a part are fine.
    let server = Server::start(vec![route_of(
        &hub::MODEL,
        [&"my-team/llama-3.1_q4"],
        Reply::json(200, &versions_page(1, 100, 0, 0)),
    )]);
    Hipc::against(&Home::new(), &server)
        .run(&["hub", "models", "get", "my-team/llama-3.1_q4"])
        .ok();
}

#[test]
fn reads_dry_run_name_one_get_and_send_nothing() {
    let server = Server::start(vec![]);
    let base = server.url();
    for (args, path) in [
        (&["hub", "events", "list"][..], "/api/registry/events/"),
        (&["hub", "models", "list"], "/api/models/?page_size=100"),
        (&["hub", "models", "formats"], "/api/models/formats/"),
        (
            &["hub", "models", "get", "acme/llama"],
            "/api/models/acme/llama/?page_size=100",
        ),
        (
            &["hub", "models", "get", "acme/llama", "--reference", "v1"],
            "/api/models/acme/llama/v1/",
        ),
    ] {
        let doc = Hipc::new(&Home::new())
            .env("HIPPIUS_API_URL", base)
            .run(&[&["--dry-run"][..], args].concat())
            .ok();
        assert_eq!(draft(&doc)["method"], "GET", "{args:?}");
        assert_eq!(draft(&doc)["url"], format!("{base}{path}"), "{args:?}");
    }
    assert!(server.requests().is_empty());
}

/// Hub routes act on the caller's own account only, so a membership is refused before
/// anything is sent, the model index included: it would answer with the caller's own private
/// models, not the member account's.
#[test]
fn events_and_models_refuse_a_membership_before_sending() {
    let server = Server::start(vec![]);
    for args in [
        &["hub", "events", "list"][..],
        &["hub", "models", "list"],
        &["hub", "models", "formats"],
        &["hub", "models", "get", "acme/llama"],
        &[
            "hub",
            "models",
            "get",
            "acme/llama",
            "--reference",
            "v1",
            "--dry-run",
        ],
    ] {
        Hipc::against(&Home::new(), &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(args)
            .fails(2, "account_scope_unsupported");
    }
    assert!(server.requests().is_empty());
}
