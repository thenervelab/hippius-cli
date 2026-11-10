//! `hipc api <METHOD> <PATH>`.

use serde_json::json;

use crate::common::{Hipc, Home, MEMBER, Reply, Server, TOKEN, api, draft, route};

/// The flag that sends a request body from a file.
const BODY: &str = "--body-file";

#[test]
fn a_2xx_body_is_printed_byte_for_byte() {
    // Odd spacing and a number too large for u64 or f64: re-serializing would change both.
    let body = "{ \"paid_per_month\" : 99000000000000000000 , \"x\":1.0 }";
    let server = Server::start(vec![route(
        "GET",
        "/api/registry/subscriptions/",
        Reply::json(200, body),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&["api", "GET", "/registry/subscriptions/"]);
    assert_eq!(run.code(), Some(0));
    assert_eq!(run.stdout_bytes(), body.as_bytes());
    assert_eq!(run.stderr(), "");
}

/// An answer of exactly 32 MiB is read, and only a larger one is `internal`.
#[test]
fn a_body_of_exactly_32_mib_is_read_and_one_byte_more_is_not() {
    const CAP: usize = 32 * 1024 * 1024;
    // A JSON string of `len` bytes, quotes included.
    let body = |len: usize| format!("\"{}\"", "a".repeat(len - 2));
    let at_cap = body(CAP);
    let server = Server::start(vec![route("GET", "/api/big/", Reply::json(200, &at_cap))]);
    let run = Hipc::against(&Home::new(), &server).run(&["api", "GET", "/big/"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(run.stdout_bytes().len(), CAP);

    let server = Server::start(vec![route(
        "GET",
        "/api/big/",
        Reply::json(200, &body(CAP + 1)),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&["api", "GET", "/big/"]);
    assert_eq!(run.stdout(), "");
    let error = run.fails(1, "internal");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .ends_with("is larger than 32 MiB"),
        "{error}"
    );
}

#[test]
fn paths_join_under_api_once_and_full_urls_on_the_origin_work() {
    let server = Server::start(vec![route(
        "GET",
        "/api/drive/plans/",
        Reply::json(200, "[]"),
    )]);
    let home = Home::new();
    let full = format!("{}/api/drive/plans/", server.url());
    for path in ["/drive/plans/", "/api/drive/plans/", full.as_str()] {
        let run = Hipc::against(&home, &server).run(&["api", "get", path]);
        assert_eq!(run.code(), Some(0), "{path}: {}", run.stderr());
    }
    for request in server.requests() {
        assert_eq!(request.target, "/api/drive/plans/");
        assert_eq!(
            request.header("authorization"),
            Some(&*format!("Token {TOKEN}"))
        );
    }
}

/// `hipc api` is no general HTTP client: a full URL off the configured origin is refused
/// before anything is sent, whatever the method, body, `--yes`, `--all`, or `--dry-run`,
/// plain `http` to another loopback port included; the hint names curl.
#[test]
fn a_full_url_on_another_origin_is_refused_and_sends_nothing() {
    let api = Server::start(vec![]);
    let other = Server::start(vec![]);
    let home = Home::new();
    let body = home.path().join("body.json");
    std::fs::write(&body, r#"{"secret":"x"}"#).unwrap();
    let body = body.to_str().unwrap();
    let loopback = format!("{}/api/compute/vms/", other.url());
    let api_port = api.url().rsplit(':').next().unwrap().to_owned();
    let https_loopback = format!("https://127.0.0.1:{api_port}/api/compute/vms/");
    for target in [
        loopback.as_str(),
        https_loopback.as_str(),
        "https://collector.example/collect?d=1",
        "http://collector.example/x/",
        "https://api.hippius.com/api/accounts/",
        "http://[::1]:9/api/accounts/",
    ] {
        for args in [
            &["api", "GET", target][..],
            &["api", "HEAD", target],
            &["api", "GET", target, "--all"],
            &["api", "GET", target, "--dry-run"],
            &["api", "POST", target, BODY, body, "--yes"],
            &["api", "DELETE", target, "--yes", "--dry-run"],
        ] {
            let error = Hipc::against(&home, &api)
                .env("HIPPIUS_ACCOUNT", MEMBER)
                .run(args)
                .fails(2, "usage");
            let message = error["message"].as_str().unwrap();
            assert!(
                message.contains("is not on the API origin"),
                "{args:?}: {message}"
            );
            assert!(
                !message.contains("d=1"),
                "a query value is quoted: {message}"
            );
            assert_eq!(
                error["hint"],
                "pass a path such as `/compute/vms/`, or use curl for another host"
            );
        }
    }
    assert!(api.requests().is_empty());
    assert!(other.requests().is_empty());
}

/// The configured origin in another spelling, its scheme and host in upper case, or its
/// port written out, is the same origin, and gets the token.
#[test]
fn a_full_url_on_the_api_origin_in_another_spelling_is_sent() {
    let server = Server::start(vec![route(
        "GET",
        "/api/drive/plans/",
        Reply::json(200, "[]"),
    )]);
    let home = Home::new();
    let upper = format!(
        "{}/api/drive/plans/",
        server.url().replace("http://", "HTTP://")
    );
    let run = Hipc::against(&home, &server).run(&["api", "GET", &upper]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        server.only_request().header("authorization"),
        Some(&*format!("Token {TOKEN}"))
    );
}

#[test]
fn bad_methods_and_paths_are_usage_without_a_request() {
    let server = Server::start(vec![]);
    let home = Home::new();
    for args in [
        &["api", "TRACE", "/x/"][..],
        &["api", "GET", "relative/"],
        &["api", "GET", "ftp://example.com/x"],
        &["api", "GET", "/x/", BODY, "-"],
        &["api", "HEAD", "/x/", BODY, "-"],
        &["api", "POST", "/x/", "--all"],
        // `--paginate` was renamed `--all` before 1.0, with no alias.
        &["api", "GET", "/x/", "--paginate"],
    ] {
        Hipc::against(&home, &server).run(args).fails(2, "usage");
    }
    // A path the HTTP library cannot put in a request line is refused before it connects.
    let error = Hipc::against(&home, &server)
        .run(&["api", "GET", "/a b/"])
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        "the HTTP library refuses to build the request: `invalid uri character`"
    );
    assert_eq!(
        error["hint"],
        "write the path with only the characters a URL allows"
    );
    assert!(server.requests().is_empty());
}

#[test]
fn a_write_on_a_pipe_needs_yes_and_sends_nothing_without_it() {
    let server = Server::start(vec![route(
        "POST",
        "/api/ssh-keys/",
        Reply::json(201, "{}"),
    )]);
    let home = Home::new();
    for method in ["POST", "PUT", "PATCH", "DELETE"] {
        let error = Hipc::against(&home, &server)
            .run(&["api", method, "/ssh-keys/"])
            .fails(2, "confirmation_required");
        assert_eq!(error["side_effects"], "none");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn a_confirmed_post_sends_the_input_body_as_json() {
    let server = Server::start(vec![route(
        "POST",
        "/api/ssh-keys/",
        Reply::json(201, r#"{"id":7}"#),
    )]);
    let home = Home::new();
    let input = home.file(
        "key.json",
        br#"{"name":"laptop","public_key":"ssh-ed25519 AAAA"}"#,
    );
    let run = Hipc::against(&home, &server).run(&[
        "api",
        "POST",
        "/ssh-keys/",
        BODY,
        input.to_str().unwrap(),
        "--yes",
    ]);
    assert_eq!(run.stdout(), r#"{"id":7}"#);
    let request = server.only_request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.header("content-type"), Some("application/json"));
    assert_eq!(
        request.json(),
        json!({"name": "laptop", "public_key": "ssh-ed25519 AAAA"})
    );
    let run = Hipc::against(&home, &server)
        .stdin(br#"{"name":"stdin"}"#)
        .run(&["api", "POST", "/ssh-keys/", BODY, "-", "--yes"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(server.requests()[1].json(), json!({"name": "stdin"}));
}

/// `DELETE /api/cdn/lists/<name>/items/` removes the items its body names (swagger
/// `cdn_lists_items_delete`), so a DELETE takes `--body-file` too.
#[test]
fn a_delete_sends_the_input_body_and_one_without_input_sends_none() {
    let server = Server::start(vec![
        route(
            "DELETE",
            "/api/cdn/lists/blocked/items/",
            Reply::bytes(204, "application/json", b""),
        ),
        route(
            "DELETE",
            "/api/ssh-keys/7/",
            Reply::bytes(204, "application/json", b""),
        ),
    ]);
    let home = Home::new();
    let items = home.file("items.json", br#"{"items":["203.0.113.7"]}"#);
    let items = items.to_str().unwrap();
    let path = "/cdn/lists/blocked/items/";
    let dry = Hipc::against(&home, &server)
        .run(&["api", "DELETE", path, BODY, items, "--dry-run"])
        .ok();
    assert_eq!(draft(&dry)["method"], "DELETE");
    assert_eq!(draft(&dry)["body"], json!({"items": ["203.0.113.7"]}));
    Hipc::against(&home, &server)
        .run(&["api", "DELETE", path, BODY, items])
        .fails(2, "confirmation_required");
    assert!(server.requests().is_empty());

    let run = Hipc::against(&home, &server).run(&["api", "DELETE", path, BODY, items, "--yes"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    let run = Hipc::against(&home, &server).run(&["api", "DELETE", "/ssh-keys/7/", "--yes"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    let requests = server.requests();
    assert_eq!(requests[0].method, "DELETE");
    assert_eq!(requests[0].header("content-type"), Some("application/json"));
    assert_eq!(requests[0].json(), json!({"items": ["203.0.113.7"]}));
    assert_eq!(requests[1].header("content-type"), None);
    assert_eq!(requests[1].header("content-length"), None);
    assert_eq!(requests[1].body, b"");
}

/// A body does not get past a route rule: a firewall replace still needs
/// `--remove-others`.
#[test]
fn a_write_with_input_still_meets_its_route_rule() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let body = home.file("body.json", b"{}");
    let error = Hipc::against(&home, &server)
        .run(&[
            "api",
            "PUT",
            "/compute/vms/vm-1/firewall/",
            BODY,
            body.to_str().unwrap(),
            "--yes",
        ])
        .fails(2, "remove_others_required");
    let hint = error["hint"].as_str().unwrap();
    assert!(hint.contains("--remove-others"), "{hint}");
    assert!(server.requests().is_empty());
}

#[test]
fn input_must_be_readable_json() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let not_json = home.file("bad.json", b"{name:");
    let missing = home.path().join("missing.json");
    for path in [&not_json, &missing] {
        Hipc::against(&home, &server)
            .run(&["api", "POST", "/x/", BODY, path.to_str().unwrap(), "--yes"])
            .fails(2, "usage");
    }
    let huge = home.file("huge.json", &vec![b' '; 8 * 1024 * 1024 + 1]);
    Hipc::against(&home, &server)
        .run(&["api", "PUT", "/x/", BODY, huge.to_str().unwrap(), "--yes"])
        .fails(2, "usage");
    assert!(server.requests().is_empty());
}

#[test]
fn a_non_2xx_puts_the_body_in_details() {
    let server = Server::start(vec![route(
        "GET",
        "/api/compute/vms/abc/",
        Reply::json(404, r#"{"error":"Not found","vm_id":"abc"}"#).header("X-Request-Id", "r-42"),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["api", "GET", "/compute/vms/abc/"])
        .fails(1, "not_found");
    assert_eq!(
        error["message"],
        format!(
            "cannot call `GET {}/api/compute/vms/abc/`: the API says `Not found`",
            server.url()
        )
    );
    assert_eq!(
        error["details"],
        json!({"error": "Not found", "vm_id": "abc"})
    );
    assert_eq!(error["request_id"], "r-42");
    assert_eq!(error["http_status"], 404);
}

#[test]
fn dry_run_describes_and_sends_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let input = home.file("body.json", br#"{"plan":"pro","rail":"credits"}"#);
    let doc = Hipc::against(&home, &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&[
            "api",
            "POST",
            "/s3/subscription/",
            BODY,
            input.to_str().unwrap(),
            "--dry-run",
        ])
        .dry_ok();
    assert_eq!(
        doc,
        crate::common::one_request_dry_run(json!({
            "method": "POST",
            "url": format!("{}/api/s3/subscription/", server.url()),
            "account_header": MEMBER,
            "project_header": null,
            "billing_header": null,
            "body": {"plan": "pro", "rail": "credits"}
        }))
    );
    assert!(server.requests().is_empty());
}

/// A dry run's `url` keeps its query values in JSON, where an agent checks exactly what
/// would be sent, and redacts them in text mode, which a person may paste elsewhere.
#[test]
fn a_dry_run_redacts_query_values_in_text_mode_only() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let args = ["api", "GET", "/x/?token=s3cr3t&page=2", "--dry-run"];
    let doc = Hipc::against(&home, &server).run(&args).ok();
    assert_eq!(
        draft(&doc)["url"],
        format!("{}/api/x/?token=s3cr3t&page=2", server.url())
    );
    let text = Hipc::against(&home, &server)
        .run(&[&["--output", "text"][..], &args].concat())
        .stdout()
        .to_owned();
    assert!(!text.contains("s3cr3t"), "{text}");
    assert!(
        text.contains("/api/x/?token=<redacted>&page=<redacted>"),
        "{text}"
    );
    assert!(server.requests().is_empty());
}

#[test]
fn the_account_header_follows_the_route_class() {
    let server = Server::start(vec![
        route("GET", "/api/compute/vms/", Reply::json(200, "[]")),
        route("GET", "/api/s3/subscription/", Reply::json(200, "{}")),
        route(
            "GET",
            "/api/billing/credits/balance/",
            Reply::json(200, "{}"),
        ),
        route("GET", "/api/ssh-keys/", Reply::json(200, "[]")),
        route(
            "GET",
            "/api/accounts/",
            Reply::json(200, r#"{"accounts":[]}"#),
        ),
    ]);
    let home = Home::new();
    let member = |path: &str| {
        Hipc::against(&home, &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&["api", "GET", path])
    };
    for path in [
        "/compute/vms/",
        "/s3/subscription/",
        "/billing/credits/balance/",
    ] {
        assert_eq!(member(path).code(), Some(0), "{path}");
    }
    for path in ["/ssh-keys/", "/accounts/"] {
        assert_eq!(member(path).code(), Some(0), "{path}");
    }
    let sent: Vec<(String, Option<String>)> = server
        .requests()
        .iter()
        .map(|r| {
            (
                r.path().to_owned(),
                r.header("x-hippius-account").map(str::to_owned),
            )
        })
        .collect();
    assert_eq!(
        sent,
        [
            ("/api/compute/vms/".to_owned(), Some(MEMBER.to_owned())),
            ("/api/s3/subscription/".to_owned(), Some(MEMBER.to_owned())),
            (
                "/api/billing/credits/balance/".to_owned(),
                Some(MEMBER.to_owned())
            ),
            ("/api/ssh-keys/".to_owned(), None),
            ("/api/accounts/".to_owned(), None),
        ]
    );
    for path in [
        "/drive/subscription/",
        "/registry/subscriptions/",
        "/billing/transactions/",
    ] {
        member(path).fails(2, "account_scope_unsupported");
    }
    assert_eq!(server.requests().len(), 5, "refused routes sent nothing");
    Hipc::against(&home, &server)
        .run(&["api", "GET", "/compute/vms/"])
        .code();
    assert_eq!(server.requests()[5].header("x-hippius-account"), None);
}

#[test]
fn all_follows_next_links_into_one_array() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/ssh-keys/?page=2",
            Reply::json(
                200,
                r#"{"count":3,"next":null,"previous":"x","results":[{"id":3}]}"#,
            ),
        ),
        route(
            "GET",
            "/api/ssh-keys/",
            Reply::json(
                200,
                concat!(
                    r#"{"count":3,"next":"{base}/api/ssh-keys/?page=2","previous":null,"#,
                    r#""results":[{"id":1},{"id":2}]}"#
                ),
            ),
        ),
    ]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["api", "GET", "/ssh-keys/", "--all"])
        .ok();
    assert_eq!(doc, json!([{"id": 1}, {"id": 2}, {"id": 3}]));
    let targets: Vec<String> = server.requests().iter().map(|r| r.target.clone()).collect();
    assert_eq!(targets, ["/api/ssh-keys/", "/api/ssh-keys/?page=2"]);
    for request in server.requests() {
        assert_eq!(
            request.header("authorization"),
            Some(&*format!("Token {TOKEN}"))
        );
    }
}

#[test]
fn all_refuses_a_next_link_that_loops() {
    let server = Server::start(vec![route(
        "GET",
        "/api/loop/",
        Reply::json(200, r#"{"next":"{base}/api/loop/","results":[1]}"#),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["api", "GET", "/loop/", "--all"])
        .fails(1, "internal");
    assert!(error["message"].as_str().unwrap().contains("leads back to"));
    // The repeat is caught before it is sent.
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn all_dry_run_describes_the_first_page_only() {
    let server = Server::start(vec![]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["api", "GET", "/ssh-keys/", "--all", "--dry-run"])
        .ok();
    assert_eq!(doc["dry_run"], true);
    assert!(server.requests().is_empty());
}

#[test]
fn all_takes_a_bare_array_as_one_page() {
    let server = Server::start(vec![route(
        "GET",
        "/api/drive/plans/",
        Reply::json(200, r#"[{"code":"solo"}]"#),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["api", "GET", "/drive/plans/", "--all"])
        .ok();
    assert_eq!(doc, json!([{"code": "solo"}]));
}

#[test]
fn all_refuses_a_foreign_next_and_a_non_list() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/foreign/",
            Reply::json(
                200,
                r#"{"next":"https://evil.example/api/x/?page=2","results":[]}"#,
            ),
        ),
        route("GET", "/api/object/", Reply::json(200, r#"{"a":1}"#)),
        route("GET", "/api/string/", Reply::json(200, r#""x""#)),
    ]);
    let home = Home::new();
    let foreign = Hipc::against(&home, &server)
        .run(&["api", "GET", "/foreign/", "--all"])
        .fails(1, "internal");
    assert!(
        foreign["message"]
            .as_str()
            .unwrap()
            .contains("evil.example")
    );
    for path in ["/object/", "/string/"] {
        Hipc::against(&home, &server)
            .run(&["api", "GET", path, "--all"])
            .fails(1, "internal");
    }
}

#[test]
fn a_path_the_server_could_reroute_is_refused_before_sending() {
    let server = Server::start(vec![]);
    let home = Home::new();
    for path in [
        "/ssh-keys/../compute/vms/",
        "/compute/%2e%2e/drive/",
        "/compute\\vms/",
        // A front end that drops path parameters reads these as the token and the deletion.
        "/auth/token/;",
        "/account/deletion/;x",
        "/ssh-keys/\u{ff0e}\u{ff0e}/compute/vms/",
        "/compute/vms/v\u{e9}/",
    ] {
        Hipc::against(&home, &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&["api", "GET", path])
            .fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn a_url_in_the_query_is_part_of_the_path() {
    let server = Server::start(vec![route("GET", "/api/x/", Reply::json(200, "{}"))]);
    let run = Hipc::against(&Home::new(), &server).run(&["api", "GET", "/x/?next=https://y/z"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(server.only_request().target, "/api/x/?next=https://y/z");
}

#[test]
fn all_follows_a_next_link_given_as_a_path() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/k/?page=2",
            Reply::json(200, r#"{"next":null,"results":[2]}"#),
        ),
        route(
            "GET",
            "/api/k/",
            Reply::json(200, r#"{"next":"/api/k/?page=2","results":[1]}"#),
        ),
    ]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["api", "GET", "/k/", "--all"])
        .ok();
    assert_eq!(doc, json!([1, 2]));
}

#[test]
fn all_stops_after_a_thousand_pages() {
    let server = crate::common::endless_pages(&api("/e/"), |served| {
        format!(
            r#"{{"next":"{{base}}/api/e/?page={}","results":[]}}"#,
            served + 1
        )
    });
    let error = Hipc::against(&Home::new(), &server)
        .run(&["api", "GET", "/e/", "--all"])
        .fails(1, "internal");
    assert!(
        error["message"].as_str().unwrap().contains("1000 pages"),
        "{error}"
    );
    assert_eq!(server.requests().len(), 1000);
}

#[test]
fn the_confirmation_question_never_shows_a_query_value() {
    let server = Server::start(vec![]);
    let run = Hipc::against(&Home::new(), &server).run(&["api", "POST", "/x/?api_key=SUPERSECRET"]);
    let error = run.fails(2, "confirmation_required");
    assert!(!run.stderr().contains("SUPERSECRET"), "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("api_key=<redacted>")
    );
}

#[test]
fn a_fragment_in_an_api_path_is_refused_before_sending() {
    let server = Server::start(vec![]);
    Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["api", "GET", "/accounts/#/../drive/subscription/"])
        .fails(2, "usage");
    assert!(server.requests().is_empty());
}

#[test]
fn a_next_link_the_client_refuses_is_internal_not_usage() {
    let server = Server::start(vec![route(
        "GET",
        "/api/k/",
        Reply::json(200, r#"{"next":"{base}/api/k/../x/?page=2","results":[]}"#),
    )]);
    Hipc::against(&Home::new(), &server)
        .run(&["api", "GET", "/k/", "--all"])
        .fails(1, "internal");
}

/// A URL longer than the HTTP library builds is refused with its own message and hint,
/// a dry run included, so the dry run does not accept what the real run refuses.
/// Every character of it is one a URL allows.
#[test]
fn a_url_too_long_to_send_is_refused_with_a_hint_that_fits() {
    let server = Server::start(vec![]);
    let path = format!("/compute/vms/?q={}", "x".repeat(70_000));
    for extra in [&[][..], &["--dry-run"][..]] {
        let args = [&["api", "GET", path.as_str()][..], extra].concat();
        let error = Hipc::against(&Home::new(), &server)
            .run(&args)
            .fails(2, "usage");
        assert_eq!(
            error["message"],
            format!(
                "the URL is {} bytes long, and the HTTP library sends at most 65534",
                server.url().len() + "/api".len() + path.len()
            )
        );
        assert_eq!(error["hint"], "shorten the path or its query");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn a_first_page_path_the_client_refuses_is_the_callers_usage_error() {
    let server = Server::start(vec![]);
    Hipc::against(&Home::new(), &server)
        .run(&["api", "GET", "/x/../y/", "--all"])
        .fails(2, "usage");
    assert!(server.requests().is_empty());
}

#[test]
fn a_rerun_hint_never_repeats_a_query_value() {
    let server = Server::start(vec![]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["api", "POST", "/registry/robot/rotate/?sig=s3cr3t", "--yes"])
        .fails(2, "reveal_required");
    assert_eq!(error["hint"], "pass `--reveal` after the command");
    assert!(server.requests().is_empty());
    // Without a query the hint is the whole command line to run.
    let error = Hipc::against(&Home::new(), &server)
        .run(&["api", "POST", "/registry/robot/rotate/", "--yes"])
        .fails(2, "reveal_required");
    assert_eq!(
        error["hint"],
        "run `hipc api POST /registry/robot/rotate/ --yes --reveal`"
    );
}

/// `--reveal` comes before the selected account's refusals (tenet 5): a membership on a
/// route that cannot act in another account is refused only once `--reveal` is given.
#[test]
fn reveal_is_checked_before_the_account_refusal() {
    let server = Server::start(vec![]);
    let hipc = || Hipc::against(&Home::new(), &server).env("HIPPIUS_ACCOUNT", MEMBER);
    hipc()
        .run(&["api", "POST", "/registry/provision/", "--yes"])
        .fails(2, "reveal_required");
    hipc()
        .run(&["api", "POST", "/registry/provision/", "--yes", "--reveal"])
        .fails(2, "account_scope_unsupported");
    assert!(server.requests().is_empty());
}

#[test]
fn a_post_that_issues_a_one_time_secret_needs_reveal_first() {
    let server = Server::start(vec![
        route(
            "POST",
            "/api/registry/robot/rotate/",
            Reply::json(201, r#"{"robot_secret":"s3cr3t"}"#),
        ),
        route("POST", "/api/ssh-keys/", Reply::json(201, "{}")),
        route(
            "POST",
            "/api/objectstore/master-tokens/x/y/rotate/",
            Reply::not_found(),
        ),
    ]);
    for path in [
        "/registry/robot/rotate/",
        "/objectstore/master-tokens/",
        "/objectstore/sub-tokens/0f8fad5b-d9cb-469f-a165-70867728950e/rotate/",
        "/registry/keys/7/rotate/",
        "/registry/provision/",
        "/databases/",
        "/databases/7/restore/",
        "/network/peers/enroll/",
        "/api/registry/keys/",
        // Without the trailing slash a server or proxy that adds it reaches the same route.
        "/objectstore/master-tokens",
        "/registry/keys/7/rotate",
    ] {
        let error = Hipc::against(&Home::new(), &server)
            .run(&["api", "POST", path, "--yes"])
            .fails(2, "reveal_required");
        assert!(
            error["hint"].as_str().unwrap().contains("--reveal"),
            "{path}"
        );
    }
    // Before the confirmation: --reveal without --yes is the confirmation refusal.
    Hipc::against(&Home::new(), &server)
        .run(&["api", "POST", "/registry/robot/rotate/", "--reveal"])
        .fails(2, "confirmation_required");
    assert!(server.requests().is_empty());
    let doc = Hipc::against(&Home::new(), &server)
        .run(&[
            "api",
            "POST",
            "/registry/robot/rotate/",
            "--reveal",
            "--yes",
        ])
        .ok();
    assert_eq!(doc["robot_secret"], "s3cr3t");
    // Other routes, a GET, and a dry run need no --reveal.
    Hipc::against(&Home::new(), &server)
        .run(&["api", "POST", "/ssh-keys/", "--yes"])
        .ok();
    Hipc::against(&Home::new(), &server)
        .run(&["api", "POST", "/registry/robot/rotate/", "--dry-run"])
        .ok();
    // Not a secret route, so it is sent, and the server answers 404.
    Hipc::against(&Home::new(), &server)
        .run(&[
            "api",
            "POST",
            "/objectstore/master-tokens/x/y/rotate/",
            "--yes",
        ])
        .fails(1, "not_found");
    // A front end could decode or merge these onto a secret route, so they are refused.
    for path in [
        "/registry/robot/rot%61te/",
        "/registry//keys/",
        "/objectstore/master-tokens//rotate/",
    ] {
        Hipc::against(&Home::new(), &server)
            .run(&["api", "POST", path, "--yes"])
            .fails(2, "usage");
    }
}

/// The account token route is never sent, on any method, whatever the flags, a dry run
/// included: its answer is the account's own API token. The refusal comes before the
/// `--reveal` check, so it is not hidden behind a flag that would only lead to it.
#[test]
fn the_account_token_route_is_never_sent() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/auth/token/",
            Reply::json(200, r#"{"access_token":"m4st3r","token_type":"Bearer"}"#),
        ),
        route(
            "GET",
            "/api/objectstore/master-tokens/",
            Reply::json(200, "[]"),
        ),
    ]);
    let hipc = || Hipc::against(&Home::new(), &server);
    for method in ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"] {
        for path in ["/auth/token/", "/auth/token", "/api/auth/token/?x=1"] {
            for extra in [
                &[][..],
                &["--reveal"],
                &["--reveal", "--yes"],
                &["--dry-run"],
            ] {
                let mut args = vec!["api", method, path];
                args.extend(extra);
                let error = hipc().run(&args).fails(2, "usage");
                assert_eq!(
                    error["message"],
                    format!(
                        "`hipc api` does not send `{method} /api/auth/token/`: {}",
                        if matches!(method, "GET" | "HEAD") {
                            "its answer is the account's API token"
                        } else {
                            "the route hands out the account's API token"
                        }
                    ),
                    "{args:?}"
                );
                assert_eq!(
                    error["hint"],
                    "create a token in the console settings at \
                     `https://console.hippius.com/dashboard/settings`"
                );
            }
        }
    }
    hipc()
        .run(&["api", "GET", "/auth/token/", "--all", "--reveal"])
        .fails(2, "usage");
    assert!(server.requests().is_empty());
    // A GET to a route whose POST issues a secret is a plain read.
    hipc()
        .run(&["api", "GET", "/objectstore/master-tokens/"])
        .ok();
    assert_eq!(server.requests().len(), 1);
}

/// A terminal ticket opens a shell on the VM for about a minute.
#[test]
fn a_terminal_ticket_needs_reveal_first() {
    let server = Server::start(vec![route(
        "POST",
        "/api/compute/vms/vm-1/terminal-ticket/",
        Reply::json(200, r#"{"ticket":"t1ck3t","url":null,"expires_in":60}"#),
    )]);
    let hipc = || Hipc::against(&Home::new(), &server);
    let path = "/compute/vms/vm-1/terminal-ticket/";
    let error = hipc()
        .run(&["api", "POST", path, "--yes"])
        .fails(2, "reveal_required");
    assert_eq!(
        error["hint"],
        format!("run `hipc api POST {path} --yes --reveal`")
    );
    // Before the confirmation, as for every secret route.
    hipc()
        .run(&["api", "POST", path])
        .fails(2, "reveal_required");
    hipc()
        .run(&["api", "POST", path, "--reveal"])
        .fails(2, "confirmation_required");
    assert!(server.requests().is_empty());
    let doc = hipc().run(&["api", "POST", path, "--yes", "--reveal"]).ok();
    assert_eq!(doc["ticket"], "t1ck3t");
    assert_eq!(server.requests().len(), 1);
}

/// The pages of one list share its path, so a `next` link to another path is refused
/// before it is sent: it could reach a route whose answer holds a secret, such as the
/// account's master token, without `--reveal`.
#[test]
fn all_refuses_a_next_link_to_another_path() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/auth/token/",
            Reply::json(
                200,
                r#"{"next":null,"results":[{"token":"MASTER-SECRET"}]}"#,
            ),
        ),
        route(
            "GET",
            "/api/compute/vms/",
            Reply::json(
                200,
                r#"{"next":"{base}/api/auth/token/?page=2","results":[{"id":1}]}"#,
            ),
        ),
    ]);
    let run = Hipc::against(&Home::new(), &server).run(&["api", "GET", "/compute/vms/", "--all"]);
    let error = run.fails(1, "internal");
    assert_eq!(
        error["message"],
        format!(
            "the API's `next` link `{}/api/auth/token/?page=<redacted>` leaves \
             `/api/compute/vms/`, the path of the list",
            server.url()
        )
    );
    assert!(!run.stderr().contains("MASTER-SECRET"));
    assert_eq!(server.requests().len(), 1, "the token page is never sent");
}

/// A `next` link that adds the trailing `/` the first page's path left off stays on the
/// same route, by the rule the secret and route tables match by, so the list goes on.
#[test]
fn all_follows_a_next_link_that_only_adds_the_trailing_slash() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/compute/vms/?page=2",
            Reply::json(200, r#"{"next":null,"results":[2]}"#),
        ),
        route(
            "GET",
            "/api/compute/vms",
            Reply::json(
                200,
                r#"{"next":"{base}/api/compute/vms/?page=2","results":[1]}"#,
            ),
        ),
    ]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["api", "GET", "/compute/vms", "--all"])
        .ok();
    assert_eq!(doc, json!([1, 2]));
    assert_eq!(server.requests().len(), 2);
}

/// The server wrote the link, so its refusal is not the caller's to fix: `internal`, not the
/// exit-2 `account_scope_unsupported`, whose `--account self` would only meet the route
/// check next.
#[test]
fn a_next_link_onto_a_route_the_membership_cannot_use_is_internal() {
    let server = Server::start(vec![route(
        "GET",
        "/api/compute/vms/",
        Reply::json(
            200,
            r#"{"next":"{base}/api/drive/subscription/?page=2","results":[]}"#,
        ),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["api", "GET", "/compute/vms/", "--all"])
        .fails(1, "internal");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with("the API's `next` link is refused: "),
        "{error}"
    );
    assert_eq!(server.requests().len(), 1, "the drive page is never sent");
}

/// An account deletion request is left to the console: `hipc api` refuses it whatever the
/// flags, a dry run and a membership included, and sends nothing.
#[test]
fn an_account_deletion_request_is_never_sent() {
    let server = Server::start(vec![
        route("POST", "/api/account/deletion/", Reply::json(202, "{}")),
        route("GET", "/api/account/deletion/", Reply::json(200, "{}")),
        route("DELETE", "/api/account/deletion/", Reply::json(200, "{}")),
    ]);
    let hipc = || Hipc::against(&Home::new(), &server);
    let full_url = format!("{}/api/account/deletion/", server.url());
    for path in [
        "/account/deletion/",
        "/account/deletion",
        "/api/account/deletion/?confirm=1",
        &full_url,
    ] {
        for extra in [
            &[][..],
            &["--yes"],
            &["--yes", "--change-isolation", "--reveal"],
            &["--dry-run"],
        ] {
            let mut argv = vec!["api", "POST", path];
            argv.extend(extra);
            let error = hipc().run(&argv).fails(2, "usage");
            assert_eq!(
                error["hint"],
                "request the deletion in the console, under Settings, Delete Account",
                "{argv:?}"
            );
        }
    }
    // Before the account refusal: `/api/account/` cannot act in another account.
    hipc()
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["api", "POST", "/account/deletion/", "--yes"])
        .fails(2, "usage");
    assert!(server.requests().is_empty());
    // Its other methods read or cancel the deletion, and are sent as any request is.
    hipc().run(&["api", "GET", "/account/deletion/"]).ok();
    hipc()
        .run(&["api", "DELETE", "/account/deletion/", "--yes"])
        .ok();
    assert_eq!(server.requests().len(), 2);
}

/// A cluster's sealed kubeconfig is reached through `clusters kubeconfig` only: `hipc api`
/// refuses its read, GET or HEAD, whatever the flags, slashless, with a format suffix, under
/// `--all` and a dry run included, and sends nothing.
#[test]
fn a_cluster_secrets_read_is_never_sent() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/kubernetes/clusters/7/secrets/",
            Reply::json(200, "{}"),
        ),
        route("GET", "/api/kubernetes/clusters/7/", Reply::json(200, "{}")),
    ]);
    let hipc = || Hipc::against(&Home::new(), &server);
    let full_url = format!("{}/api/kubernetes/clusters/7/secrets/", server.url());
    // A HEAD runs the GET handler on the server, so it is refused the same way.
    for path in [
        "/kubernetes/clusters/7/secrets/",
        "/kubernetes/clusters/7/secrets.json",
    ] {
        for extra in [&["--yes"][..], &["--dry-run"]] {
            let mut argv = vec!["api", "HEAD", path];
            argv.extend(extra);
            let error = hipc().run(&argv).fails(2, "usage");
            assert_eq!(
                error["message"],
                "`hipc api` does not send `HEAD /api/kubernetes/clusters/<cluster_id>/secrets/`: \
                 the route answers the cluster's sealed kubeconfig",
                "{argv:?}"
            );
        }
    }
    for path in [
        "/kubernetes/clusters/7/secrets/",
        "/kubernetes/clusters/7/secrets",
        "/kubernetes/clusters/7/secrets.json/",
        "/kubernetes/clusters/7/secrets.api",
        "/api/kubernetes/clusters/7/secrets/?format=json",
        &full_url,
    ] {
        for extra in [
            &[][..],
            &["--yes", "--change-isolation", "--reveal"],
            &["--all"],
            &["--dry-run"],
        ] {
            let mut argv = vec!["api", "GET", path];
            argv.extend(extra);
            let error = hipc().run(&argv).fails(2, "usage");
            assert_eq!(
                error["message"],
                "`hipc api` does not send `GET /api/kubernetes/clusters/<cluster_id>/secrets/`: \
                 its answer holds the cluster's sealed kubeconfig",
                "{argv:?}"
            );
            assert_eq!(
                error["hint"],
                "run `hipc compute kubernetes clusters kubeconfig`, which opens the kubeconfig \
                 with the cluster key file and writes it to a new file",
                "{argv:?}"
            );
        }
    }
    // Before the token. The route takes a membership and a project, so no selector here
    // could show the order against the account refusals; the account deletion test does.
    Hipc::tokenless(&Home::new(), &server)
        .run(&["api", "GET", "/kubernetes/clusters/7/secrets/"])
        .fails(2, "usage");
    assert!(server.requests().is_empty());
    // The cluster read is sent as any request is.
    hipc().run(&["api", "GET", "/kubernetes/clusters/7/"]).ok();
    assert_eq!(server.requests().len(), 1);
}

/// A CDN hostname's DNS setup goes through `cdn hostnames dns-setup` only: `hipc api` refuses
/// it whatever the flags, slashless, with a format suffix, and under a dry run, and sends
/// nothing, since its body and answer hold a DNS provider credential.
#[test]
fn a_dns_setup_is_never_sent() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let input = home.file("dns.json", br#"{"provider":"cloudflare","credentials":{}}"#);
    let input = input.to_string_lossy().into_owned();
    for path in [
        "/cdn/zones/zab12cd34ef56/hostnames/cdn.example.com/dns-setup/",
        "/cdn/zones/zab12cd34ef56/hostnames/cdn.example.com/dns-setup",
        "/cdn/zones/zab12cd34ef56/hostnames/cdn.example.com/dns-setup.json/",
        "/api/cdn/zones/zab12cd34ef56/hostnames/cdn.example.com/dns-setup.api",
    ] {
        for extra in [&["--yes"][..], &["--dry-run"], &["--yes", "--reveal"]] {
            let mut argv = vec!["api", "POST", path, "--body-file", &input];
            argv.extend(extra);
            let error = Hipc::against(&home, &server).run(&argv).fails(2, "usage");
            assert_eq!(
                error["message"],
                "`hipc api` does not send \
                 `POST /api/cdn/zones/<zone_id>/hostnames/<hostname>/dns-setup/`: its body and \
                 its answer hold a DNS provider credential",
                "{argv:?}"
            );
            assert_eq!(
                error["hint"],
                "run `hipc cdn hostnames dns-setup`, which reads the credential from a private \
                 file and keeps it out of every output"
            );
        }
    }
    // Before the token: with none configured, the refusal is still `usage`.
    Hipc::tokenless(&Home::new(), &server)
        .run(&[
            "api",
            "POST",
            "/cdn/zones/zab12cd34ef56/hostnames/cdn.example.com/dns-setup/",
            "--body-file",
            &input,
            "--yes",
        ])
        .fails(2, "usage");
    assert!(server.requests().is_empty());
}

/// A Hub subscribe through `hipc api` reads no current plan, so it could start a second paid
/// subscription: it is refused whatever the flags, a dry run included, and sends nothing.
#[test]
fn a_hub_subscribe_is_never_sent() {
    let server = Server::start(vec![route(
        "POST",
        "/api/registry/subscribe/",
        Reply::json(201, "{}"),
    )]);
    let home = Home::new();
    let input = home.file("plan.json", br#"{"plan_id":2}"#);
    let input = input.to_string_lossy().into_owned();
    for path in [
        "/registry/subscribe/",
        "/registry/subscribe",
        "/registry/subscribe.json/",
        "/api/registry/subscribe/?x=1",
    ] {
        for extra in [&["--yes"][..], &["--dry-run"], &["--yes", "--reveal"]] {
            let mut argv = vec!["api", "POST", path, "--body-file", &input];
            argv.extend(extra);
            let error = Hipc::against(&home, &server).run(&argv).fails(2, "usage");
            assert_eq!(
                error["message"],
                "`hipc api` does not send `POST /api/registry/subscribe/`: it reads no current \
                 plan first, so it could start a second paid Hub subscription",
                "{argv:?}"
            );
            assert_eq!(
                error["hint"],
                "run `hipc hub subscription subscribe`, which refuses a second plan and replaces \
                 one only with `--replace`"
            );
        }
    }
    assert!(server.requests().is_empty());
}

/// A signed download URL for a chat attachment is a bearer credential for 15 minutes, and the
/// answers of the sign-in, session, enrolment, and invite writes may hold one: each needs
/// `--reveal`, as every route of the secret table does. The presigned-URL `HEAD` has no body
/// to print, so it needs none.
#[test]
fn signed_urls_and_sign_in_answers_need_reveal_first() {
    let server = Server::start(vec![route(
        "HEAD",
        "/api/objectstore/buckets/b1/presigned-url/",
        Reply::json(200, ""),
    )]);
    let hipc = || Hipc::against(&Home::new(), &server);
    for (method, path) in [
        ("GET", "/chat/conversations/7/attachments/3/"),
        ("GET", "/support/chat/tickets/7/attachments/3/"),
        ("GET", "/objectstore/buckets/b1/presigned-url/?key=k"),
        ("GET", "/databases/12/backup-token/"),
        ("POST", "/databases/12/backup-token/"),
        ("POST", "/auth/sso/ticket/"),
        ("POST", "/auth/session/"),
        ("POST", "/auth/sign-in-methods/step-up/verify/"),
        ("POST", "/network/infra/enroll/"),
        ("POST", "/chat/workspaces/w1/invites/"),
    ] {
        hipc()
            .run(&["api", method, path, "--yes"])
            .fails(2, "reveal_required");
    }
    assert!(server.requests().is_empty());
    let run = hipc().run(&["api", "HEAD", "/objectstore/buckets/b1/presigned-url/"]);
    assert_eq!(run.code(), Some(0));
    assert_eq!(server.requests().len(), 1);
}

/// A Hub repository or artifact delete needs `--yes` alone, as any write does (tenet 5):
/// without it the question refuses, and the flag that once stood beside it is gone.
#[test]
fn a_hub_delete_needs_only_yes() {
    let server = Server::start(vec![
        route(
            "DELETE",
            "/api/registry/repositories/team/app/",
            Reply::json(200, "{}"),
        ),
        route(
            "DELETE",
            "/api/registry/repositories/app/artifacts/v1/",
            Reply::json(200, "{}"),
        ),
    ]);
    let hipc = || Hipc::against(&Home::new(), &server);
    let paths = [
        "/registry/repositories/team/app/",
        "/registry/repositories/app/artifacts/v1/",
    ];
    for path in paths {
        hipc()
            .run(&["api", "DELETE", path])
            .fails(2, "confirmation_required");
        hipc()
            .run(&["api", "DELETE", path, "--irreversible", "--yes"])
            .fails(2, "usage");
    }
    assert!(server.requests().is_empty());
    for path in paths {
        hipc().run(&["api", "DELETE", path, "--yes"]).ok();
    }
    let sent: Vec<String> = server
        .requests()
        .iter()
        .map(|r| format!("{} {}", r.method, r.path()))
        .collect();
    assert_eq!(
        sent,
        [
            "DELETE /api/registry/repositories/team/app/",
            "DELETE /api/registry/repositories/app/artifacts/v1/",
        ]
    );
}

/// A network isolation change needs `--change-isolation` as well as `--yes`, whichever
/// mode the body asks for, and refuses with the flag's own code without it.
#[test]
fn a_network_isolation_change_needs_change_isolation_first() {
    let server = Server::start(vec![
        route(
            "POST",
            "/api/network/isolation/",
            Reply::json(200, r#"{"isolation":"deny_intra"}"#),
        ),
        route(
            "GET",
            "/api/network/isolation/",
            Reply::json(200, r#"{"isolation":"allow_intra"}"#),
        ),
    ]);
    let home = Home::new();
    let body = home.file("body.json", br#"{"mode":"deny_intra"}"#);
    let body = body.to_str().unwrap();
    let hipc = || Hipc::against(&home, &server);
    for path in ["/network/isolation/", "/network/isolation"] {
        let argv = ["api", "POST", path, BODY, body, "--yes"];
        let error = hipc().run(&argv).fails(2, "change_isolation_required");
        assert_eq!(
            error["message"],
            "this would change which peers of the network can reach each other, and \
             `--change-isolation` was not passed"
        );
        let hint = error["hint"].as_str().unwrap();
        assert!(hint.ends_with(" --change-isolation`"), "{hint}");
    }
    hipc()
        .run(&["api", "POST", "/network/isolation/", "--change-isolation"])
        .fails(2, "confirmation_required");
    assert!(server.requests().is_empty());
    // A read of the mode is not guarded.
    hipc().run(&["api", "GET", "/network/isolation/"]).ok();
    let doc = hipc()
        .run(&[
            "api",
            "POST",
            "/network/isolation/",
            BODY,
            body,
            "--change-isolation",
            "--yes",
        ])
        .ok();
    assert_eq!(doc["isolation"], "deny_intra");
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].json()["mode"], "deny_intra");
}

/// A guard the API documents for one write method holds for every write method of its
/// route, and in any letter case: a `PUT` where swagger lists `PATCH` still needs the flag,
/// and so does a path a case-folding front end would deliver to the route. Nothing is sent.
/// (A dry run needs none of the flags, as everywhere.)
#[test]
fn a_guard_holds_for_every_write_method_and_letter_case() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let file = |name: &str, body: &[u8]| home.file(name, body).to_str().unwrap().to_owned();
    let origin = file("origin.json", br#"{"origin":{"bucket":"b"}}"#);
    let grants = file("grants.json", br#"{"grants":[]}"#);
    let items = file("items.json", br#"{"items":[]}"#);
    let public = file("public.json", br#"{"public":true}"#);
    let mode = file("mode.json", br#"{"mode":"deny_intra"}"#);
    let cases: [(&str, &str, &str, &str); 9] = [
        ("PUT", "/cdn/zones/z1/", &origin, "public_required"),
        ("POST", "/cdn/zones/z1/", &origin, "public_required"),
        (
            "PUT",
            "/accounts/members/7/",
            &grants,
            "remove_others_required",
        ),
        (
            "PUT",
            "/cdn/lists/blocked/",
            &items,
            "remove_others_required",
        ),
        ("PUT", "/registry/me/publicity/", &public, "public_required"),
        (
            "PUT",
            "/network/isolation/",
            &mode,
            "change_isolation_required",
        ),
        (
            "PATCH",
            "/network/isolation/",
            &mode,
            "change_isolation_required",
        ),
        (
            "POST",
            "/Network/Isolation/",
            &mode,
            "change_isolation_required",
        ),
        ("PATCH", "/CDN/zones/z1/", &origin, "public_required"),
    ];
    for (method, path, body, code) in cases {
        let argv = ["api", method, path, BODY, body, "--yes"];
        Hipc::against(&home, &server).run(&argv).fails(2, code);
    }
    // A refusal holds for every write method too, and its message names the method sent.
    let error = Hipc::against(&home, &server)
        .run(&[
            "api",
            "PUT",
            "/cdn/zones/z1/hostnames/cdn.example.com/dns-setup/",
            "--yes",
        ])
        .fails(2, "usage");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with("`hipc api` does not send `PUT /api/cdn/zones/<zone_id>/hostnames/"),
        "{error}"
    );
    Hipc::against(&home, &server)
        .run(&["api", "PATCH", "/registry/subscribe/", "--yes"])
        .fails(2, "usage");
    // The network summary's first read creates the network, in any letter case.
    for path in ["/Network/me/", "/NETWORK/ME"] {
        Hipc::against(&home, &server)
            .run(&["api", "GET", path])
            .fails(2, "confirmation_required");
    }
    assert!(server.requests().is_empty());
    // A `DELETE` of the deletion request withdraws it, and is sent.
    let withdraw = Server::start(vec![route(
        "DELETE",
        "/api/account/deletion/",
        Reply::json(200, "{}"),
    )]);
    Hipc::against(&home, &withdraw)
        .run(&["api", "DELETE", "/account/deletion/", "--yes"])
        .ok();
    assert_eq!(withdraw.requests().len(), 1);
}

/// A CDN zone create, and a zone PATCH that moves the origin, need `--public` as the
/// curated `cdn zones create` and `update` do, with the same code, before the account
/// refusal, the token, and the question.
#[test]
fn a_zone_that_may_publish_a_bucket_needs_public_first() {
    let server = Server::start(vec![
        route(
            "POST",
            "/api/cdn/zones/",
            Reply::json(201, r#"{"id":"z1"}"#),
        ),
        route(
            "PATCH",
            "/api/cdn/zones/z1/",
            Reply::json(200, r#"{"id":"z1"}"#),
        ),
    ]);
    let home = Home::new();
    let create = home.file("create.json", br#"{"name":"z","origin":{"bucket":"b"}}"#);
    let moved = home.file("moved.json", br#"{"origin":{"prefix":"site/"}}"#);
    let renamed = home.file("renamed.json", br#"{"name":"z2"}"#);
    let [create, moved, renamed] = [&create, &moved, &renamed].map(|p| p.to_str().unwrap());
    let hipc = || Hipc::against(&home, &server);
    for argv in [
        ["api", "POST", "/cdn/zones/", BODY, create, "--yes"],
        ["api", "POST", "/cdn/zones", BODY, create, "--yes"],
        ["api", "PATCH", "/cdn/zones/z1/", BODY, moved, "--yes"],
    ] {
        let error = hipc().run(&argv).fails(2, "public_required");
        assert_eq!(
            error["message"],
            "this may make a private bucket readable by anyone, and `--public` was not passed"
        );
        assert_eq!(
            error["hint"],
            format!("run `hipc {} --public`", argv.join(" "))
        );
        // Before the account refusal and the token: a membership, or no token, still meets
        // this refusal first.
        hipc()
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&argv)
            .fails(2, "public_required");
        Hipc::tokenless(&Home::new(), &server)
            .run(&argv)
            .fails(2, "public_required");
    }
    // A PATCH that keeps the origin publishes nothing new, so only the question applies.
    hipc()
        .run(&["api", "PATCH", "/cdn/zones/z1/", BODY, renamed])
        .fails(2, "confirmation_required");
    hipc()
        .run(&["api", "POST", "/cdn/zones/", BODY, create, "--public"])
        .fails(2, "confirmation_required");
    // A dry run sends nothing, so it needs no flag.
    let doc = hipc()
        .run(&["api", "POST", "/cdn/zones/", BODY, create, "--dry-run"])
        .ok();
    assert_eq!(draft(&doc)["method"], "POST");
    assert!(server.requests().is_empty());
    let public = ["--public", "--yes"];
    hipc()
        .run(&[&["api", "POST", "/cdn/zones/", BODY, create][..], &public].concat())
        .ok();
    hipc()
        .run(
            &[
                &["api", "PATCH", "/cdn/zones/z1/", BODY, moved][..],
                &public,
            ]
            .concat(),
        )
        .ok();
    hipc()
        .run(&["api", "PATCH", "/cdn/zones/z1/", BODY, renamed, "--yes"])
        .ok();
    let sent: Vec<String> = server
        .requests()
        .iter()
        .map(|r| format!("{} {} {}", r.method, r.path(), r.json()))
        .collect();
    assert_eq!(
        sent,
        [
            r#"POST /api/cdn/zones/ {"name":"z","origin":{"bucket":"b"}}"#,
            r#"PATCH /api/cdn/zones/z1/ {"origin":{"prefix":"site/"}}"#,
            r#"PATCH /api/cdn/zones/z1/ {"name":"z2"}"#,
        ]
    );
}

/// The `--remove-others` hint of a path with a query names the flag alone, so no query
/// value is repeated (tenet 9).
#[test]
fn a_whole_list_hint_never_repeats_a_query_value() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let rules = home.file("rules.json", br#"{"rules":[]}"#);
    let rules = rules.to_str().unwrap();
    let query = "/cdn/zones/z1/rules/?v=s3cr3t";
    let error = Hipc::against(&home, &server)
        .run(&["api", "PUT", query, BODY, rules, "--yes"])
        .fails(2, "remove_others_required");
    assert_eq!(
        error["hint"],
        "pass `--remove-others` after the command to replace it whole"
    );
    assert!(server.requests().is_empty());
}

/// The firewall PUT, the cache rules PUT, and a member PATCH that sends `grants` replace a
/// whole list, so they need `--remove-others`, with the curated commands' code. `hipc api`
/// reads nothing first: it refuses outright.
#[test]
fn a_whole_list_replace_needs_remove_others_first() {
    let server = Server::start(vec![
        route(
            "PUT",
            "/api/compute/vms/vm-1/firewall/",
            Reply::json(200, "{}"),
        ),
        route("PUT", "/api/cdn/zones/z1/rules", Reply::json(200, "{}")),
        route("PATCH", "/api/accounts/members/7/", Reply::json(200, "{}")),
    ]);
    let home = Home::new();
    let rules = home.file("rules.json", br#"{"rules":[]}"#);
    let grants = home.file(
        "grants.json",
        br#"{"grants":[{"scope":"vms","level":"viewer"}]}"#,
    );
    let billing = home.file("billing.json", br#"{"can_billing":true}"#);
    let [rules, grants, billing] = [&rules, &grants, &billing].map(|p| p.to_str().unwrap());
    let hipc = || Hipc::against(&home, &server);
    let writes = [
        ["api", "PUT", "/compute/vms/vm-1/firewall/", BODY, rules],
        ["api", "PUT", "/cdn/zones/z1/rules", BODY, rules],
        ["api", "PATCH", "/accounts/members/7/", BODY, grants],
    ];
    for write in writes {
        let argv = [&write[..], &["--yes"]].concat();
        let error = hipc().run(&argv).fails(2, "remove_others_required");
        assert_eq!(
            error["message"],
            "this would replace a whole list or document, removing every entry it leaves out, \
             and `--remove-others` was not passed"
        );
        assert_eq!(
            error["hint"],
            format!(
                "run `hipc {} --remove-others` to replace it whole",
                argv.join(" ")
            )
        );
        hipc()
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&argv)
            .fails(2, "remove_others_required");
        Hipc::tokenless(&Home::new(), &server)
            .run(&argv)
            .fails(2, "remove_others_required");
        // With the flag, the question still applies; a dry run needs no flag and reads
        // nothing.
        let flagged = [&write[..], &["--remove-others"]].concat();
        hipc().run(&flagged).fails(2, "confirmation_required");
        let dry = [&write[..], &["--dry-run"]].concat();
        assert!(hipc().run(&dry).ok().get("reads_first").is_none());
    }
    // A member PATCH that keeps the grants replaces no list.
    hipc()
        .run(&["api", "PATCH", "/accounts/members/7/", BODY, billing])
        .fails(2, "confirmation_required");
    // `--public` is the other guard's flag, and does not stand in for this one.
    hipc()
        .run(&[&writes[0][..], &["--yes", "--public"]].concat())
        .fails(2, "remove_others_required");
    assert!(server.requests().is_empty());
    for write in writes {
        hipc()
            .run(&[&write[..], &["--remove-others", "--yes"]].concat())
            .ok();
    }
    hipc()
        .run(&[
            "api",
            "PATCH",
            "/accounts/members/7/",
            BODY,
            billing,
            "--yes",
        ])
        .ok();
    let sent: Vec<String> = server
        .requests()
        .iter()
        .map(|r| format!("{} {}", r.method, r.path()))
        .collect();
    assert_eq!(
        sent,
        [
            "PUT /api/compute/vms/vm-1/firewall/",
            "PUT /api/cdn/zones/z1/rules",
            "PATCH /api/accounts/members/7/",
            "PATCH /api/accounts/members/7/",
        ]
    );
}

/// The whole-list and whole-document replaces swagger documents without a curated command
/// need `--remove-others` too: a zone's WAF rules and rate limits, a WAF list's items (and
/// a list PATCH that sends `items`), and an operator node's storage document.
#[test]
fn the_uncurated_whole_replaces_need_remove_others() {
    let writes = [
        ("PUT", "/cdn/zones/z1/waf/rules/"),
        ("PUT", "/cdn/zones/z1/waf/rate-limits"),
        ("PUT", "/cdn/lists/blocked/items/"),
        ("PATCH", "/cdn/lists/blocked/"),
        ("PUT", "/v1/operator/nodes/n1/storage/"),
    ];
    let server = Server::start(
        writes
            .iter()
            .map(|&(method, path)| route(method, &format!("/api{path}"), Reply::json(200, "{}")))
            .collect(),
    );
    let home = Home::new();
    let body = home.file("body.json", br#"{"items":["10.0.0.1"],"rules":[]}"#);
    let described = home.file("described.json", br#"{"description":"x"}"#);
    let [body, described] = [&body, &described].map(|p| p.to_str().unwrap());
    let hipc = || Hipc::against(&home, &server);
    for (method, path) in writes {
        let write = ["api", method, path, BODY, body];
        hipc()
            .run(&[&write[..], &["--yes"]].concat())
            .fails(2, "remove_others_required");
        hipc()
            .run(&[&write[..], &["--remove-others"]].concat())
            .fails(2, "confirmation_required");
    }
    // A list PATCH that keeps the items replaces nothing.
    hipc()
        .run(&["api", "PATCH", "/cdn/lists/blocked/", BODY, described])
        .fails(2, "confirmation_required");
    assert!(server.requests().is_empty());
    for (method, path) in writes {
        hipc()
            .run(&["api", method, path, BODY, body, "--remove-others", "--yes"])
            .ok();
    }
    assert_eq!(server.requests().len(), writes.len());
}

/// A bucket ACL set replaces the bucket policy, and a policy can publish the bucket, so it
/// needs `--public` and then `--remove-others`, whatever the body.
#[test]
fn a_bucket_acl_set_needs_public_and_remove_others() {
    let server = Server::start(vec![route(
        "PUT",
        "/api/objectstore/buckets/b/acl/set/",
        Reply::json(200, "{}"),
    )]);
    let home = Home::new();
    let grants = home.file("grants.json", br#"{"grants":{}}"#);
    let grants = grants.to_str().unwrap();
    let hipc = || Hipc::against(&home, &server);
    let write = [
        "api",
        "PUT",
        "/objectstore/buckets/b/acl/set/",
        BODY,
        grants,
    ];
    let error = hipc()
        .run(&[&write[..], &["--yes"]].concat())
        .fails(2, "public_required");
    assert_eq!(
        error["message"],
        "this may make a private bucket readable by anyone, and `--public` was not passed"
    );
    let error = hipc()
        .run(&[&write[..], &["--yes", "--public"]].concat())
        .fails(2, "remove_others_required");
    let hint = error["hint"].as_str().unwrap();
    assert!(
        hint.ends_with("--public --remove-others` to replace it whole"),
        "{hint}"
    );
    hipc()
        .run(&[&write[..], &["--remove-others"]].concat())
        .fails(2, "public_required");
    hipc()
        .run(&[&write[..], &["--public", "--remove-others"]].concat())
        .fails(2, "confirmation_required");
    assert!(server.requests().is_empty());
    hipc()
        .run(&[&write[..], &["--public", "--remove-others", "--yes"]].concat())
        .ok();
    assert_eq!(server.requests().len(), 1);
}

/// A format suffix on the last segment (`.json`, which Django REST framework can
/// serve beside a route) meets every guard of the plain route: `--reveal`,
/// the console-only refusal, `--remove-others`, `--public`, and the selector refusals, with
/// or without the trailing `/`. None of them reaches the server.
#[test]
fn a_format_suffix_meets_the_guards_of_its_plain_route() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let hipc = || Hipc::against(&home, &server);
    let cases = [
        ("POST", "/objectstore/master-tokens", "reveal_required"),
        (
            "POST",
            "/objectstore/master-tokens/1/rotate",
            "reveal_required",
        ),
        ("GET", "/auth/token", "usage"),
        ("POST", "/account/deletion", "usage"),
        ("PUT", "/compute/vms/abc/firewall", "remove_others_required"),
        ("POST", "/cdn/zones", "public_required"),
    ];
    for (method, path, code) in cases {
        for suffix in [".json", ".api"] {
            for slash in ["", "/"] {
                let path = format!("{path}{suffix}{slash}");
                hipc().run(&["api", method, &path, "--yes"]).fails(2, code);
                // A membership is refused later, so the route's own refusal comes first.
                hipc()
                    .env("HIPPIUS_ACCOUNT", MEMBER)
                    .run(&["api", method, &path, "--yes"])
                    .fails(2, code);
            }
        }
    }
    // A dry run needs no flag, but the console-only route stays refused even then.
    hipc()
        .run(&["api", "POST", "/account/deletion.json", "--dry-run"])
        .fails(2, "usage");
    let doc = hipc()
        .run(&["api", "POST", "/cdn/zones.json", "--dry-run"])
        .ok();
    assert_eq!(draft(&doc)["method"], "POST");
    // A membership is refused on a suffixed route it is refused on without the suffix,
    // and a personal route still goes without the header.
    hipc()
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["api", "GET", "/drive/subscription.json/", "--dry-run"])
        .fails(2, "account_scope_unsupported");
    let doc = hipc()
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["api", "GET", "/drive/plans.json/", "--dry-run"])
        .ok();
    assert_eq!(draft(&doc)["account_header"], serde_json::Value::Null);
    // A project goes on the S3 plan read with a suffix as without, on `GET` only, and the
    // Default project's master keys stay refused one.
    let project = "proj_0a1b2c3d4e5f60718293";
    let doc = hipc()
        .run(&[
            "--project",
            project,
            "api",
            "GET",
            "/s3/subscription.json/",
            "--dry-run",
        ])
        .ok();
    assert_eq!(draft(&doc)["project_header"], project);
    for (method, path) in [
        ("POST", "/s3/subscription.json/"),
        ("GET", "/objectstore/master-tokens.json"),
    ] {
        hipc()
            .run(&["--project", project, "api", method, path, "--dry-run"])
            .fails(2, "project_scope_unsupported");
    }
    assert!(server.requests().is_empty());
}

/// A dotted bucket name is not a format suffix that changes the route: the bucket routes
/// classify it as any bucket, so a read takes the project and needs no flag, and an ACL
/// set needs its two flags as for any bucket.
#[test]
fn a_dotted_bucket_name_is_handled_as_any_bucket() {
    let server = Server::start(vec![route(
        "GET",
        "/api/objectstore/buckets/my.bucket/",
        Reply::json(200, "{}"),
    )]);
    let home = Home::new();
    let hipc = || Hipc::against(&home, &server);
    let project = "proj_0a1b2c3d4e5f60718293";
    let doc = hipc()
        .run(&[
            "--project",
            project,
            "api",
            "GET",
            "/objectstore/buckets/my.bucket/",
            "--dry-run",
        ])
        .ok();
    assert_eq!(draft(&doc)["project_header"], project);
    hipc()
        .run(&[
            "api",
            "PUT",
            "/objectstore/buckets/my.bucket/acl/set/",
            "--yes",
        ])
        .fails(2, "public_required");
    assert!(server.requests().is_empty());
    hipc()
        .run(&["api", "GET", "/objectstore/buckets/my.bucket/"])
        .ok();
    assert_eq!(server.requests().len(), 1);
}

/// A path without its trailing `/` takes the account header the slashed path takes: a
/// personal route goes out without it and a scoped one with it, and `--all` sends the same
/// header on a page whose link adds the slash.
#[test]
fn a_slashless_path_takes_the_account_header_of_the_slashed_one() {
    let server = Server::start(vec![
        route("GET", "/api/ssh-keys", Reply::json(200, "[]")),
        route(
            "GET",
            "/api/billing/invoices/?page=2",
            Reply::json(200, r#"{"next":null,"results":[{"id":2}]}"#),
        ),
        route(
            "GET",
            "/api/billing/invoices",
            Reply::json(
                200,
                r#"{"next":"{base}/api/billing/invoices/?page=2","results":[{"id":1}]}"#,
            ),
        ),
    ]);
    let home = Home::new();
    let member = || Hipc::against(&home, &server).env("HIPPIUS_ACCOUNT", MEMBER);
    member().run(&["api", "GET", "/ssh-keys"]).ok();
    let doc = member()
        .run(&["api", "GET", "/billing/invoices", "--all"])
        .ok();
    assert_eq!(doc, json!([{"id": 1}, {"id": 2}]));
    let sent: Vec<(String, Option<String>)> = server
        .requests()
        .iter()
        .map(|r| {
            (
                r.target.clone(),
                r.header("x-hippius-account").map(str::to_owned),
            )
        })
        .collect();
    assert_eq!(
        sent,
        [
            ("/api/ssh-keys".to_owned(), None),
            ("/api/billing/invoices".to_owned(), Some(MEMBER.to_owned())),
            (
                "/api/billing/invoices/?page=2".to_owned(),
                Some(MEMBER.to_owned())
            ),
        ]
    );
}

/// Swagger: the first `GET /api/network/me/` for an account creates its network, so `hipc
/// api` confirms it, whatever form the path takes, and sends nothing without `--yes`. On a
/// pipe the refusal names the action only, as every confirmation's does; the consequence
/// `hipc network get` names is shown at the prompt, and that `hipc api` takes the same gate
/// is a unit test of `commands::api`. `--dry-run` describes it and sends nothing. `--all`
/// is refused before anything else, so the route is never sent unconfirmed to follow
/// pages. A HEAD is confirmed too: Django runs it through the GET handler.
#[test]
fn a_get_that_creates_the_network_needs_yes_in_every_form() {
    let server = Server::start(vec![]);
    let hipc = || Hipc::against(&Home::new(), &server);
    for path in [
        "/network/me/",
        "/network/me",
        "/api/network/me/",
        "/network/me.json/",
        "/network/me.json",
        "/network/me/?x=1",
    ] {
        // The question names the URL with its query values redacted, as every quoted URL.
        let shown = path.replace("?x=1", "?x=<redacted>");
        let shown = shown.strip_prefix("/api").unwrap_or(&shown);
        for method in ["GET", "HEAD"] {
            let error = hipc()
                .run(&["api", method, path])
                .fails(2, "confirmation_required");
            assert_eq!(
                error["message"],
                format!(
                    "this would call `{method} {}/api{shown}`, and `--yes` was not passed",
                    server.url()
                ),
                "{method} {path}"
            );
            assert_eq!(error["hint"], "pass `--yes` to confirm", "{method} {path}");
        }
    }
    let error = hipc()
        .run(&["api", "GET", "/network/me/", "--all", "--yes"])
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        "`--all` sends the first page without a confirmation, and `GET /network/me/` may \
         create the account's network"
    );
    assert_eq!(error["hint"], "remove `--all`");
    let doc = hipc()
        .run(&["api", "GET", "/network/me/", "--dry-run"])
        .ok();
    assert_eq!(draft(&doc)["method"], "GET");
    assert_eq!(
        draft(&doc)["url"],
        format!("{}/api/network/me/", server.url())
    );
    assert!(server.requests().is_empty());
}

/// With `--yes` the request is sent and its body printed, as a GET or a HEAD, in the
/// slashless and the suffixed forms as well.
#[test]
fn a_confirmed_network_get_is_sent_and_its_body_printed() {
    const SUMMARY: &str = r#"{"status":"active","groups":1,"networks":1,"peers":0}"#;
    let server = Server::start(vec![
        route("GET", "/api/network/me/", Reply::json(200, SUMMARY)),
        route("GET", "/api/network/me", Reply::json(200, SUMMARY)),
        route("GET", "/api/network/me.json/", Reply::json(200, SUMMARY)),
        route("HEAD", "/api/network/me/", Reply::json(200, "")),
    ]);
    let hipc = || Hipc::against(&Home::new(), &server);
    for path in ["/network/me/", "/network/me", "/network/me.json/"] {
        let run = hipc().run(&["api", "GET", path, "--yes"]);
        assert_eq!(run.code(), Some(0), "{path}: {}", run.stderr());
        assert_eq!(run.stdout_bytes(), SUMMARY.as_bytes(), "{path}");
    }
    let run = hipc().run(&["api", "HEAD", "/network/me/", "--yes"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    let sent: Vec<String> = server
        .requests()
        .iter()
        .map(|r| format!("{} {}", r.method, r.target))
        .collect();
    assert_eq!(
        sent,
        [
            "GET /api/network/me/",
            "GET /api/network/me",
            "GET /api/network/me.json/",
            "HEAD /api/network/me/",
        ]
    );
}

/// The network is account-wide, and the request may create it, so a project stored in the
/// profile is refused on it rather than left off, a dry run included, and nothing is sent; a
/// plain read beside it still skips the stored project. A stored billing account is left
/// off it, as off every route that takes none, and the request goes out without it.
#[test]
fn a_stored_project_is_refused_on_the_network_get() {
    let server = Server::start(vec![
        route("GET", "/api/network/peers/", Reply::json(200, "[]")),
        route("GET", "/api/network/me/", Reply::json(200, "{}")),
    ]);
    let home = Home::new();
    let project = "proj_0a1b2c3d4e5f60718293";
    Hipc::new(&home)
        .run(&["account", "projects", "use", project])
        .ok();
    let hipc = || Hipc::against(&home, &server);
    hipc().run(&["api", "GET", "/network/peers/"]).ok();
    for extra in ["--yes", "--dry-run"] {
        for path in ["/network/me/", "/network/me", "/network/me.json"] {
            let error = hipc()
                .run(&["api", "GET", path, extra])
                .fails(2, "project_scope_unsupported");
            let message = error["message"].as_str().unwrap();
            assert!(message.contains("`GET "), "{path} {extra}: {message}");
        }
    }
    hipc()
        .run(&["api", "HEAD", "/network/me/", "--yes"])
        .fails(2, "project_scope_unsupported");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path(), "/api/network/peers/");
    assert_eq!(requests[0].header("x-hippius-project"), None);

    let home = Home::new();
    Hipc::new(&home)
        .run(&[
            "account",
            "billing-accounts",
            "use",
            "bill_9f8e7d6c5b4a39281706",
        ])
        .ok();
    let doc = Hipc::against(&home, &server)
        .run(&["api", "GET", "/network/me/", "--dry-run"])
        .ok();
    assert_eq!(
        draft(&doc).get("billing_header"),
        Some(&serde_json::Value::Null),
        "{doc}"
    );
    Hipc::against(&home, &server)
        .run(&["api", "GET", "/network/me/", "--yes"])
        .ok();
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].path(), "/api/network/me/");
    assert_eq!(requests[1].header("x-hippius-billing"), None);
}

/// The request counts as a write once sent: a timeout or a 5xx may have created the
/// network, so it is `partial` and not retryable, where a plain GET's timeout is `none` and
/// retryable; a 4xx is `none`.
#[test]
fn a_failed_network_get_may_have_created_the_network() {
    let server = Server::start(vec![
        route("GET", "/api/network/me/", Reply::never()),
        route("GET", "/api/network/peers/", Reply::never()),
    ]);
    let hipc = || Hipc::against(&Home::new(), &server);
    let error = hipc()
        .run(&[
            "api",
            "GET",
            "/network/me/",
            "--request-timeout",
            "1",
            "--yes",
        ])
        .fails(1, "timeout");
    assert_eq!(error["side_effects"], "partial");
    assert_eq!(error["retryable"], false);
    let error = hipc()
        .run(&["api", "GET", "/network/peers/", "--request-timeout", "1"])
        .fails(1, "timeout");
    assert_eq!(error["side_effects"], "none");
    assert_eq!(error["retryable"], true);
    let failing = Server::start(vec![route(
        "GET",
        "/api/network/me/",
        Reply::json(502, r#"{"detail":"bad gateway"}"#),
    )]);
    let error = Hipc::against(&Home::new(), &failing)
        .run(&["api", "GET", "/network/me/", "--yes"])
        .fails(1, "unavailable");
    assert_eq!(error["side_effects"], "partial");
    let suspended = Server::start(vec![route(
        "GET",
        "/api/network/me/",
        Reply::json(403, r#"{"detail":"suspended"}"#),
    )]);
    let error = Hipc::against(&Home::new(), &suspended)
        .run(&["api", "GET", "/network/me/", "--yes"])
        .fails(1, "permission");
    assert_eq!(error["side_effects"], "none");
}

/// The neighbours of the route stay plain reads: sent without `--yes`, with a stored
/// project left off them.
#[test]
fn the_neighbours_of_the_network_get_stay_reads() {
    let server = Server::start(vec![
        route("GET", "/api/network/", Reply::json(200, "{}")),
        route("GET", "/api/network/me/peers/", Reply::json(200, "[]")),
        route("GET", "/api/network/peers/", Reply::json(200, "[]")),
        route("GET", "/api/networks/me/", Reply::json(200, "{}")),
    ]);
    let home = Home::new();
    Hipc::new(&home)
        .run(&["account", "projects", "use", "proj_0a1b2c3d4e5f60718293"])
        .ok();
    for path in [
        "/network/",
        "/network/me/peers/",
        "/network/peers/",
        "/networks/me/",
    ] {
        Hipc::against(&home, &server)
            .run(&["api", "GET", path])
            .ok();
    }
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    for request in requests {
        assert_eq!(
            request.header("x-hippius-project"),
            None,
            "{}",
            request.target
        );
    }
}

/// A support write through `hipc api` whose body holds an access key, or a token split by
/// whitespace, is refused as one with the token whole is, and the refusal quotes neither.
#[test]
fn a_support_write_with_an_access_key_or_a_split_token_is_refused() {
    let server = Server::start(vec![route(
        "POST",
        "/api/support/tickets/7/messages/",
        Reply::json(201, "{}"),
    )]);
    let home = Home::new();
    let stored = "tok_stored_0123456789";
    home.write_config(&format!(
        "[profiles.other]\ntoken = \"{stored}\"\ntoken_origin = \"https://api.hippius.com\"\n"
    ));
    let wrapped = format!("{}\n{}", &stored[..8], &stored[8..]);
    for (body, what, it) in [
        (
            json!({ "body": crate::common::PHRASES[0] }).to_string(),
            "what looks like an access key",
            "the 12 words",
        ),
        (
            json!({ "body": wrapped }).to_string(),
            "an API token",
            "the token",
        ),
    ] {
        let file = home.file("body.json", body.as_bytes());
        let argv = [
            "api",
            "POST",
            "/support/tickets/7/messages/",
            BODY,
            file.to_str().unwrap(),
            "--yes",
        ];
        let error = Hipc::against(&home, &server).run(&argv).fails(2, "usage");
        assert_eq!(
            error["message"],
            format!(
                "`hipc api` does not send a support ticket write whose body holds {what}, which \
                 a person on the Hippius support team would read"
            )
        );
        assert_eq!(
            error["hint"],
            format!("remove {it} from the body; the support team never needs it")
        );
        assert!(!error.to_string().contains("curtain"), "{error}");
    }
    assert!(server.requests().is_empty());
}

/// A support write through `hipc api` (any method but `GET` and `HEAD` under `/api/support/`)
/// is refused, as `hipc support tickets create` and `reply` are, when its body holds
/// `HIPPIUS_TOKEN` or a stored token: as written, inside a key or a nested array, or behind a
/// `\u` escape. A dry run and `--yes` included, nothing is sent, and the refusal
/// quotes neither the body nor the path. A body with no token, or another route, is sent.
#[test]
fn a_support_ticket_write_that_holds_a_token_is_refused() {
    let server = Server::start(vec![
        route("POST", "/api/support/tickets/", Reply::json(201, "{}")),
        route(
            "POST",
            "/api/support/tickets/7/messages/",
            Reply::json(201, "{}"),
        ),
        route("POST", "/api/compute/vms/", Reply::json(201, "{}")),
    ]);
    let home = Home::new();
    let stored = "tok_stored_0123456789";
    home.write_config(&format!(
        "[profiles.other]\ntoken = \"{stored}\"\ntoken_origin = \"https://api.hippius.com\"\n"
    ));
    let escaped = format!("\\u{:04x}{}", u32::from(stored.as_bytes()[0]), &stored[1..]);
    let bodies = [
        json!({ "subject": "Help", "description": format!("my token is {TOKEN}") }).to_string(),
        json!({ "body": format!("config: token = {stored}") }).to_string(),
        json!({ stored.to_owned(): "x" }).to_string(),
        json!({ "attachments": [1, ["note", format!("tok {stored}")]] }).to_string(),
        format!(r#"{{"body":"{escaped}"}}"#),
    ];
    for body in &bodies {
        let file = home.file("body.json", body.as_bytes());
        let file = file.to_str().unwrap();
        for (method, path) in [
            ("POST", "/support/tickets/"),
            ("POST", "/support/tickets/7/messages"),
            ("POST", "/support/tickets.json"),
            // A front end that folds letter case would deliver these to the same routes.
            ("POST", "/support/Tickets/"),
            ("PATCH", "/SUPPORT/tickets/7/"),
            ("PATCH", "/support/tickets/7/"),
            ("PUT", "/support/tickets/7/"),
            ("POST", "/support/tickets/7/handoff/"),
            ("POST", "/support/chat/tickets/7/messages/"),
            ("DELETE", "/support/tickets/7/"),
        ] {
            for extra in [&["--yes"][..], &["--dry-run"]] {
                let mut argv = vec!["api", method, path, BODY, file];
                argv.extend(extra);
                let error = Hipc::against(&home, &server).run(&argv).fails(2, "usage");
                assert_eq!(
                    error["message"],
                    "`hipc api` does not send a support ticket write whose body holds an API \
                     token, which a person on the Hippius support team would read",
                    "{method} {path} {body}"
                );
                assert!(!error.to_string().contains("456789"), "{error}");
                assert!(!error.to_string().contains("tickets"), "{error}");
            }
        }
    }
    assert!(server.requests().is_empty());
    // Without a token in it the body goes out, and another route is not checked.
    let file = home.file("clean.json", br#"{"body":"hello"}"#);
    let file = file.to_str().unwrap();
    Hipc::against(&home, &server)
        .run(&[
            "api",
            "POST",
            "/support/tickets/7/messages/",
            BODY,
            file,
            "--yes",
        ])
        .ok();
    let file = home.file("vm.json", format!(r#"{{"name":"{stored}"}}"#).as_bytes());
    Hipc::against(&home, &server)
        .run(&[
            "api",
            "POST",
            "/compute/vms/",
            BODY,
            file.to_str().unwrap(),
            "--yes",
        ])
        .ok();
    assert_eq!(server.requests().len(), 2);
}
