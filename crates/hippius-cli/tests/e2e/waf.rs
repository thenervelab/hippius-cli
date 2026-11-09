//! `hipc cdn waf` and `hipc cdn lists`: a zone's WAF settings, rules, rate limits, and
//! versions, and the account's WAF lists.

use serde_json::{Value, json};

use crate::common::{Hipc, Home, MEMBER, Reply, Route, Run, Server, draft, route};

const ZONE: &str = "zab12cd34ef56";

fn waf_route(method: &'static str, suffix: &str, reply: Reply) -> Route {
    route(
        method,
        &format!("/api/cdn/zones/{ZONE}/waf/{suffix}"),
        reply,
    )
}

fn list_route(method: &'static str, suffix: &str, reply: Reply) -> Route {
    route(method, &format!("/api/cdn/lists/{suffix}"), reply)
}

fn hipc(server: &Server, args: &[&str]) -> Run {
    let mut argv = vec!["cdn"];
    argv.extend(args);
    Hipc::against(&Home::new(), server).run(&argv)
}

/// `hipc cdn <args>` with `input` on stdin.
fn piped(server: &Server, input: &str, args: &[&str]) -> Run {
    let mut argv = vec!["cdn"];
    argv.extend(args);
    Hipc::against(&Home::new(), server)
        .stdin(input.as_bytes())
        .run(&argv)
}

/// The method and path of every request the server received.
fn targets(server: &Server) -> Vec<String> {
    server
        .requests()
        .iter()
        .map(|r| format!("{} {}", r.method, r.target))
        .collect()
}

#[test]
fn waf_reads_print_as_the_server_sent_them() {
    let settings = r#"{"mode":"on","tier":"standard","active_version":{"number":3}}"#;
    let rules =
        r#"{"rules":[{"id":"r1","expression":"ip.src eq 1.2.3.4","action":"block"}],"version":3}"#;
    let server = Server::start(vec![
        waf_route("GET", "", Reply::json(200, settings)),
        waf_route("GET", "status/", Reply::json(200, r#"{"nodes":2}"#)),
        waf_route("GET", "rules/", Reply::json(200, rules)),
        waf_route("GET", "rate-limits/", Reply::json(200, r#"{"rules":[]}"#)),
        waf_route("GET", "versions/3/", Reply::json(200, r#"{"number":3}"#)),
        list_route("GET", "blocked_ips/", Reply::json(200, r#"{"items":[]}"#)),
    ]);
    assert_eq!(
        hipc(&server, &["waf", "get", ZONE]).stdout().trim_end(),
        settings
    );
    assert_eq!(hipc(&server, &["waf", "status", ZONE]).ok()["nodes"], 2);
    assert_eq!(
        hipc(&server, &["waf", "rules", "get", ZONE])
            .stdout()
            .trim_end(),
        rules
    );
    assert_eq!(
        hipc(&server, &["waf", "rate-limits", "get", ZONE]).ok(),
        json!({"rules": []})
    );
    assert_eq!(
        hipc(&server, &["waf", "versions", "get", ZONE, "3"]).ok()["number"],
        3
    );
    assert_eq!(
        hipc(&server, &["lists", "get", "blocked_ips"]).ok(),
        json!({"items": []})
    );
    assert!(server.requests().iter().all(|r| r.method == "GET"));
    assert_eq!(server.requests().len(), 6);
}

#[test]
fn version_and_list_lists_read_an_object_or_a_bare_array() {
    let server = Server::start(vec![
        waf_route(
            "GET",
            "versions/",
            Reply::json(
                200,
                r#"{"versions":[{"number":2},{"number":1}],"active":2}"#,
            ),
        ),
        list_route("GET", "", Reply::json(200, r#"[{"name":"a"}]"#)),
    ]);
    assert_eq!(
        hipc(&server, &["waf", "versions", "list", ZONE]).ok(),
        json!({"items": [{"number": 2}, {"number": 1}], "active": 2, "next_page_token": null})
    );
    assert_eq!(
        hipc(&server, &["lists", "list"]).ok(),
        json!({"items": [{"name": "a"}], "next_page_token": null})
    );
}

#[test]
fn analytics_and_events_send_only_the_query_parameters_given() {
    let server = Server::start(vec![
        waf_route("GET", "analytics/", Reply::json(200, "{}")),
        waf_route(
            "GET",
            "events/",
            Reply::json(200, r#"{"events":[],"cursor":null}"#),
        ),
    ]);
    hipc(&server, &["waf", "analytics", ZONE]).ok();
    hipc(
        &server,
        &[
            "waf",
            "analytics",
            ZONE,
            "--from",
            "2026-10-01",
            "--to",
            "2026-10-02",
            "--step",
            "1h",
            "--group-by",
            "country",
        ],
    )
    .ok();
    let doc = hipc(
        &server,
        &[
            "waf",
            "events",
            ZONE,
            "--rule-id",
            "r1",
            "--action",
            "block",
            "--ip",
            "1.2.3.4",
        ],
    )
    .ok();
    assert_eq!(doc, json!({"events": [], "cursor": null}));
    hipc(
        &server,
        &["waf", "events", ZONE, "--from", "a", "--to", "b"],
    )
    .ok();
    let waf = format!("/api/cdn/zones/{ZONE}/waf");
    assert_eq!(
        targets(&server),
        [
            format!("GET {waf}/analytics/"),
            format!("GET {waf}/analytics/?from=2026-10-01&to=2026-10-02&step=1h&group_by=country"),
            format!("GET {waf}/events/?rule=r1&action=block&ip=1.2.3.4"),
            format!("GET {waf}/events/?from=a&to=b"),
        ]
    );
    hipc(&server, &["waf", "analytics", ZONE, "--group-by", "path"]).fails(2, "usage");
    hipc(&server, &["waf", "events", ZONE, "--action", "drop"]).fails(2, "usage");
    assert_eq!(server.requests().len(), 4);
}

#[test]
fn waf_update_sends_only_the_settings_given_and_always_needs_yes() {
    let server = Server::start(vec![waf_route(
        "PATCH",
        "",
        Reply::json(200, r#"{"mode":"on"}"#),
    )]);
    let home = Home::new();
    let page = home.file("page.html", b"<p>Blocked: {ref}</p>");
    let page = page.to_str().unwrap();
    let doc = Hipc::against(&home, &server)
        .run(&[
            "cdn",
            "waf",
            "update",
            ZONE,
            "--mode",
            "on",
            "--tier",
            "advanced",
            "--event-sampling",
            "false",
            "--block-status",
            "429",
            "--block-content-type",
            "html",
            "--block-body-file",
            page,
            "--yes",
        ])
        .ok();
    assert_eq!(doc, json!({"mode": "on"}));
    assert_eq!(
        server.only_request().json(),
        json!({
            "mode": "on",
            "tier": "advanced",
            "event_sampling": false,
            "block_response": {
                "status": 429,
                "content_type": "html",
                "body": "<p>Blocked: {ref}</p>",
            },
        })
    );
    // One block response field is sent alone in its object.
    let doc = hipc(
        &server,
        &["waf", "update", ZONE, "--block-status", "403", "--dry-run"],
    )
    .ok();
    let body = &draft(&doc)["body"];
    assert_eq!(*body, json!({"block_response": {"status": 403}}));
    // Every change is confirmed: each one publishes a version, and a mode change bills or
    // stops the protection. The refusal names what would change.
    let error =
        hipc(&server, &["waf", "update", ZONE, "--mode", "log"]).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!(
            "this would update the WAF of the CDN zone `{ZONE}`: set `mode` to `log`, which is \
             billed from the balance while the mode is not `off`, at the price of the tier that \
             `hipc cdn prices` lists under `waf`, and `--yes` was not passed"
        )
    );
    // A tier names the billing even beside `off`: it is the price once the mode is on again.
    for args in [
        &["--tier", "standard"][..],
        &["--mode", "off", "--tier", "standard"],
    ] {
        let mut argv = vec!["waf", "update", ZONE];
        argv.extend(args);
        let error = hipc(&server, &argv).fails(2, "confirmation_required");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("which is billed"),
            "{error}"
        );
    }
    // A change that bills nothing still publishes a version, so it is confirmed too.
    for args in [&["--event-sampling", "true"][..], &["--mode", "off"]] {
        let mut argv = vec!["waf", "update", ZONE];
        argv.extend(args);
        let error = hipc(&server, &argv).fails(2, "confirmation_required");
        assert!(
            !error["message"].as_str().unwrap().contains("billed"),
            "{error}"
        );
    }
    let error = hipc(&server, &["waf", "update", ZONE, "--yes"]).fails(2, "usage");
    assert_eq!(
        error["message"],
        "`hipc cdn waf update` was given nothing to change"
    );
    for bad in [
        &["--mode", "block"][..],
        &["--tier", "pro"],
        &["--block-status", "404"],
        &["--block-content-type", "xml"],
    ] {
        let mut argv = vec!["waf", "update", ZONE, "--yes"];
        argv.extend(bad);
        hipc(&server, &argv).fails(2, "usage");
    }
    assert_eq!(server.requests().len(), 1);
}

/// Swagger: the block page body is at most 4 KB. A larger file, or one that is not UTF-8
/// text, is refused before anything is sent.
#[test]
fn waf_update_refuses_a_block_body_past_4_kb_or_not_text() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let big = home.file("big.html", &[b'a'; 4097]);
    let error = Hipc::against(&home, &server)
        .run(&[
            "cdn",
            "waf",
            "update",
            ZONE,
            "--block-body-file",
            big.to_str().unwrap(),
            "--yes",
        ])
        .fails(2, "usage");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .ends_with("is larger than 4096 bytes, the most hipc reads for the block page body"),
        "{error}"
    );
    let exact = home.file("exact.html", &[b'a'; 4096]);
    let doc = Hipc::against(&home, &server)
        .run(&[
            "cdn",
            "waf",
            "update",
            ZONE,
            "--block-body-file",
            exact.to_str().unwrap(),
            "--dry-run",
        ])
        .ok();
    assert_eq!(
        draft(&doc)["body"]["block_response"]["body"]
            .as_str()
            .unwrap()
            .len(),
        4096
    );
    let binary = home.file("binary.html", &[0xff, 0xfe]);
    let error = Hipc::against(&home, &server)
        .run(&[
            "cdn",
            "waf",
            "update",
            ZONE,
            "--block-body-file",
            binary.to_str().unwrap(),
            "--yes",
        ])
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        format!(
            "the block page body from `{}` is not UTF-8 text",
            binary.display()
        )
    );
    assert!(server.requests().is_empty());
}

#[test]
fn waf_test_sends_the_request_and_changes_nothing() {
    let server = Server::start(vec![waf_route(
        "POST",
        "test/",
        Reply::json(201, r#"{"match":true}"#),
    )]);
    let request = r#"{"ip":"1.2.3.4","path":"/admin","headers":{"user-agent":"x"}}"#;
    let doc = piped(
        &server,
        request,
        &[
            "waf",
            "test",
            ZONE,
            "--request-file",
            "-",
            "--expression",
            "ip.src eq 1.2.3.4",
        ],
    )
    .ok();
    assert_eq!(doc, json!({"match": true}));
    assert_eq!(
        server.only_request().json(),
        json!({
            "expression": "ip.src eq 1.2.3.4",
            "request": {"ip": "1.2.3.4", "path": "/admin", "headers": {"user-agent": "x"}},
        })
    );
    // Without an expression it tests the published rules.
    let doc = piped(
        &server,
        "{}",
        &["waf", "test", ZONE, "--request-file", "-", "--dry-run"],
    )
    .ok();
    assert_eq!(draft(&doc)["body"], json!({"request": {}}));
    // The docs list the request's keys exactly; another key is refused, like a rule key.
    let error = piped(
        &server,
        r#"{"ip":"1.2.3.4","url":"/"}"#,
        &["waf", "test", ZONE, "--request-file", "-"],
    )
    .fails(2, "usage");
    assert_eq!(
        error["message"],
        "the test request from stdin has the key `url`, which a test request does not have: \
         it has only `ip`, `country`, `asn`, `host`, `method`, `path`, `query`, `headers`, \
         `version`, and `ssl`"
    );
    for shape in ["[]", "null", r#""ip""#] {
        let error = piped(
            &server,
            shape,
            &["waf", "test", ZONE, "--request-file", "-"],
        )
        .fails(2, "usage");
        assert_eq!(
            error["message"],
            "the test request from stdin is not a JSON object"
        );
    }
    piped(&server, "{", &["waf", "test", ZONE, "--request-file", "-"]).fails(2, "usage");
    assert_eq!(server.requests().len(), 1);
}

/// A test is a POST that changes nothing (`hipc schema`: `read_only`), so a failure is a
/// read's: nothing changed, and a retry is safe.
#[test]
fn a_failed_test_is_a_read_that_changed_nothing() {
    let server = Server::start(vec![waf_route(
        "POST",
        "test/",
        Reply::json(503, r#"{"error":"busy","code":"waf_unavailable"}"#),
    )]);
    let error =
        piped(&server, "{}", &["waf", "test", ZONE, "--request-file", "-"]).fails(1, "unavailable");
    assert_eq!(error["side_effects"], "none");
    assert_eq!(error["retryable"], true);
}

/// The zone's custom rules now: a block, then a log rule.
const CUSTOM_RULES: &str = concat!(
    r#"{"rules":[{"id":"r1","expression":"ip.src in $bad","action":"block"},"#,
    r#"{"id":"r2","description":"admin","expression":"http.request.uri.path eq \"/admin\"","#,
    r#""action":"log","enabled":true}],"version":7}"#
);

/// The zone's rate limits now.
const RATE_LIMITS: &str = concat!(
    r#"{"rules":[{"id":"l1","expression":"true","action":"block","key":"ip","period":60,"#,
    r#""requests":100,"timeout":60},{"id":"l2","expression":"true","action":"log","#,
    r#""key":"ip+host","period":10,"requests":5,"timeout":10}],"version":7}"#
);

fn rule_routes(kind: &str, current: &str) -> Vec<Route> {
    vec![
        waf_route("GET", kind, Reply::json(200, current)),
        waf_route("PUT", kind, Reply::json(200, current)),
    ]
}

/// `--check` has the server check a list without publishing: the update's PUT with
/// `?dry_run=1`, with no `--yes`, sent as the update would be (after the read, under the
/// whole-list guard, with the version read as `If-Match`), and its answer printed. A failure
/// is a read's. The local checks of the file run first, and a dry run shows the read and the
/// PUT.
#[test]
fn update_check_sends_the_list_with_dry_run_and_publishes_nothing() {
    let answer = r#"{"valid":true,"size":120,"limit":65536}"#;
    for (kind, file) in [
        (
            "rules",
            r#"{"rules":[{"expression":"true","action":"log"}]}"#,
        ),
        (
            "rate-limits",
            concat!(
                r#"{"rules":[{"expression":"true","action":"block","key":"ip","period":60,"#,
                r#""requests":300,"timeout":600}]}"#
            ),
        ),
    ] {
        let path = format!("{kind}/");
        let current = versioned(r#"{"rules":[]}"#);
        let server = Server::start(vec![
            waf_route("GET", &path, Reply::json(200, &current)),
            waf_route("PUT", &path, Reply::json(200, answer)),
        ]);
        let args = ["waf", kind, "update", ZONE, "--rules-file", "-", "--check"];
        let doc = piped(&server, file, &args).ok();
        assert_eq!(
            doc,
            serde_json::from_str::<Value>(answer).unwrap(),
            "{kind}"
        );
        assert_eq!(
            targets(&server),
            [
                format!("GET /api/cdn/zones/{ZONE}/waf/{kind}/"),
                format!("PUT /api/cdn/zones/{ZONE}/waf/{kind}/?dry_run=1"),
            ]
        );
        let put = &server.requests()[1];
        assert_eq!(put.header("if-match"), Some("3"), "{kind}");
        assert_eq!(
            put.json(),
            serde_json::from_str::<Value>(file).unwrap(),
            "{kind}"
        );
        // A failure is a read's: nothing was published, and a retry is safe.
        let refused = Server::start(vec![
            waf_route("GET", &path, Reply::json(200, &current)),
            waf_route(
                "PUT",
                &path,
                Reply::json(503, r#"{"error":"busy","code":"waf_unavailable"}"#),
            ),
        ]);
        let error = piped(&refused, file, &args).fails(1, "unavailable");
        assert_eq!(error["side_effects"], "none", "{kind}");
        assert_eq!(error["retryable"], true, "{kind}");
        // The file is still checked before anything is sent.
        let empty = Server::start(vec![]);
        piped(&empty, r#"{"rules":[{"nope":1}]}"#, &args).fails(2, "usage");
        assert!(empty.requests().is_empty(), "{kind}");
        // A dry run shows the read, then the PUT with the query and the version to come.
        let mut dry = args.to_vec();
        dry.push("--dry-run");
        let doc = piped(&empty, file, &dry).ok();
        let requests = doc["requests"].as_array().expect("requests");
        assert_eq!(requests.len(), 2, "{doc}");
        assert_eq!(requests[0]["method"], "GET", "{doc}");
        assert!(
            draft(&doc)["url"]
                .as_str()
                .is_some_and(|url| url.ends_with(&format!("/waf/{kind}/?dry_run=1"))),
            "{doc}"
        );
        assert_eq!(draft(&doc)["if_match"], "<the version the read returns>");
    }
}

/// A check goes under the update's whole-list guard, so a server that ignored `dry_run` could
/// publish only what the update would: a file that drops a rule the zone has is refused
/// without `--remove-others`, and nothing is sent after the read; with it, the check is sent,
/// still without `--yes`.
#[test]
fn update_check_keeps_the_whole_list_guard() {
    let answer = r#"{"valid":true,"size":1,"limit":65536}"#;
    let server = Server::start(vec![
        waf_route("GET", "rules/", Reply::json(200, CUSTOM_RULES)),
        waf_route("PUT", "rules/", Reply::json(200, answer)),
    ]);
    let file = r#"{"rules":[]}"#;
    let args = [
        "waf",
        "rules",
        "update",
        ZONE,
        "--rules-file",
        "-",
        "--check",
    ];
    piped(&server, file, &args).fails(2, "remove_others_required");
    assert_eq!(
        targets(&server),
        [format!("GET /api/cdn/zones/{ZONE}/waf/rules/")]
    );
    let mut both = args.to_vec();
    both.push("--remove-others");
    piped(&server, file, &both).ok();
    assert_eq!(
        targets(&server).last().map(String::as_str),
        Some(format!("PUT /api/cdn/zones/{ZONE}/waf/rules/?dry_run=1").as_str())
    );
}

/// The exit code of `--check` says whether the list is valid: `valid: false` is `validation`,
/// exit 1, with the answer in `details`. An answer with no boolean `valid`, or one shaped like
/// a published list, is `internal` with `partial` side effects, since a server that ignored
/// `dry_run` would have published the file.
#[test]
fn update_check_fails_on_an_invalid_list_and_on_an_answer_that_may_have_published() {
    let file = r#"{"rules":[{"expression":"nope(","action":"log"}]}"#;
    let args = [
        "waf",
        "rules",
        "update",
        ZONE,
        "--rules-file",
        "-",
        "--check",
    ];
    let invalid = r#"{"valid":false,"size":12,"limit":65536,"rule_id":0}"#;
    let current = versioned(r#"{"rules":[]}"#);
    let server = Server::start(vec![
        waf_route("GET", "rules/", Reply::json(200, &current)),
        waf_route("PUT", "rules/", Reply::json(200, invalid)),
    ]);
    let error = piped(&server, file, &args).fails(1, "validation");
    assert_eq!(
        error["details"],
        serde_json::from_str::<Value>(invalid).unwrap()
    );
    assert_eq!(error["side_effects"], "none");
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.ends_with("the API says the list is not valid")),
        "{error}"
    );
    for answer in [
        r#"{"rules":[{"expression":"nope(","action":"log"}],"version":8}"#,
        r#"{"valid":true,"version":8}"#,
        r#"{"valid":"yes"}"#,
        "{}",
    ] {
        let server = Server::start(vec![
            waf_route("GET", "rules/", Reply::json(200, &current)),
            waf_route("PUT", "rules/", Reply::json(200, answer)),
        ]);
        let error = piped(&server, file, &args).fails(1, "internal");
        assert_eq!(error["side_effects"], "partial", "{answer}");
        assert_eq!(
            error["hint"],
            format!("run `hipc cdn waf rules get {ZONE}` to see what is published"),
            "{answer}"
        );
    }
}

/// The whole-list guard on the custom rules: they run top to bottom and a block or an allow
/// stops the rest, so a rule is its place, its expression, its action, and whether it is
/// on. Leaving one out, moving it, or changing any of those is refused without
/// `--remove-others`, after the read, and the PUT is never sent.
#[test]
fn rules_update_that_drops_moves_or_changes_a_rule_is_refused() {
    let r2 = concat!(
        r#"`{"position":1,"expression":"http.request.uri.path eq \"/admin\"","#,
        r#""action":"log","enabled":true}`"#
    );
    let server = Server::start(rule_routes("rules/", CUSTOM_RULES));
    let one = r#"{"rules":[{"expression":"ip.src in $bad","action":"block"}]}"#;
    let error = piped(
        &server,
        one,
        &["waf", "rules", "update", ZONE, "--rules-file", "-", "--yes"],
    )
    .fails(2, "remove_others_required");
    assert_eq!(
        error["message"],
        format!(
            "this would remove 1 WAF custom rule that the CDN zone `{ZONE}` has now, and \
             `--remove-others` was not passed"
        )
    );
    assert_eq!(
        error["hint"],
        format!(
            "run `hipc cdn waf rules update {ZONE} --rules-file - --yes --remove-others` to \
             remove {r2}"
        )
    );
    let rule = |expression: &str, action: &str, extra: &str| {
        format!(
            r#"{{"expression":{},"action":"{action}"{extra}}}"#,
            json!(expression)
        )
    };
    let bad = "ip.src in $bad";
    let admin = "http.request.uri.path eq \"/admin\"";
    for changed in [
        // Moved.
        vec![rule(admin, "log", ""), rule(bad, "block", "")],
        // A new rule ahead of the others.
        vec![
            rule("true", "allow", ""),
            rule(bad, "block", ""),
            rule(admin, "log", ""),
        ],
        // A log rule turned into a block, and a rule turned off.
        vec![rule(bad, "block", ""), rule(admin, "block", "")],
        vec![
            rule(bad, "block", r#","enabled":false"#),
            rule(admin, "log", ""),
        ],
    ] {
        let file = format!(r#"{{"rules":[{}]}}"#, changed.join(","));
        piped(
            &server,
            &file,
            &["waf", "rules", "update", ZONE, "--rules-file", "-", "--yes"],
        )
        .fails(2, "remove_others_required");
    }
    assert!(server.requests().iter().all(|r| r.method == "GET"));
    // `--remove-others` compares nothing: it reads the rules for their version alone, and
    // sends the file as it is, conditional on that version.
    let server = Server::start(rule_routes("rules/", CUSTOM_RULES));
    piped(
        &server,
        one,
        &[
            "waf",
            "rules",
            "update",
            ZONE,
            "--rules-file",
            "-",
            "--yes",
            "--remove-others",
        ],
    )
    .ok();
    assert_eq!(
        targets(&server),
        [
            format!("GET /api/cdn/zones/{ZONE}/waf/rules/"),
            format!("PUT /api/cdn/zones/{ZONE}/waf/rules/"),
        ]
    );
    assert_eq!(server.requests()[1].body, one.as_bytes());
    assert_eq!(server.requests()[1].header("if-match"), Some("7"));
    // Without `--yes` the question refuses before the read: the read is the command's own,
    // so it comes after the question (tenet 5).
    let server = Server::start(rule_routes("rate-limits/", RATE_LIMITS));
    piped(
        &server,
        RATE_LIMITS,
        &[
            "waf",
            "rate-limits",
            "update",
            ZONE,
            "--rules-file",
            "-",
            "--remove-others",
        ],
    )
    .fails(2, "confirmation_required");
    assert!(server.requests().is_empty());
}

/// A rule kept at its place may change its id and description; a missing `enabled` is
/// `true`; a rule added at the end removes nothing; the file goes out byte for byte.
#[test]
fn rules_update_that_keeps_every_rule_in_place_reads_then_puts() {
    let server = Server::start(rule_routes("rules/", CUSTOM_RULES));
    let kept = concat!(
        "{ \"rules\": [{\"expression\":\"ip.src in $bad\",\"action\":\"block\",\"enabled\":true,",
        "\"description\":\"bad\"},{\"id\":\"r2\",\"expression\":\"http.request.uri.path eq ",
        "\\\"/admin\\\"\",\"action\":\"log\"},{\"expression\":\"true\",\"action\":\"log\"}] }\n"
    );
    let doc = piped(
        &server,
        kept,
        &["waf", "rules", "update", ZONE, "--rules-file", "-", "--yes"],
    )
    .ok();
    assert_eq!(doc["version"], 7);
    assert_eq!(
        targets(&server),
        [
            format!("GET /api/cdn/zones/{ZONE}/waf/rules/"),
            format!("PUT /api/cdn/zones/{ZONE}/waf/rules/"),
        ]
    );
    assert_eq!(server.requests()[1].body, kept.as_bytes());
    // The PUT is conditional on the version the guard read.
    assert_eq!(server.requests()[1].header("if-match"), Some("7"));
    assert_eq!(server.requests()[0].header("if-match"), None);
}

/// Rate limits each count on their own, so their order is not part of them: a reorder is
/// accepted, while a looser limit, another key, or a rate limit left out is a removal.
#[test]
fn rate_limits_update_compares_each_limit_by_what_it_counts() {
    let l1 = concat!(
        r#"{"expression":"true","action":"block","key":"ip","period":60,"#,
        r#""requests":100,"timeout":60}"#
    );
    let l2 = concat!(
        r#"{"expression":"true","action":"log","key":"ip+host","period":10,"#,
        r#""requests":5,"timeout":10,"enabled":true}"#
    );
    let server = Server::start(rule_routes("rate-limits/", RATE_LIMITS));
    let update = |file: &str| {
        piped(
            &server,
            file,
            &[
                "waf",
                "rate-limits",
                "update",
                ZONE,
                "--rules-file",
                "-",
                "--yes",
            ],
        )
    };
    update(&format!(r#"{{"rules":[{l2},{l1}]}}"#)).ok();
    let looser = l1.replace("\"requests\":100", "\"requests\":1000");
    let error =
        update(&format!(r#"{{"rules":[{looser},{l2}]}}"#)).fails(2, "remove_others_required");
    assert_eq!(
        error["message"],
        format!(
            "this would remove 1 WAF rate limit that the CDN zone `{ZONE}` has now, and \
             `--remove-others` was not passed"
        )
    );
    assert!(
        error["hint"].as_str().unwrap().ends_with(concat!(
            r#"to remove `{"expression":"true","action":"block","key":"ip","period":60,"#,
            r#""requests":100,"timeout":60,"enabled":true}`"#
        )),
        "{error}"
    );
    update(&format!(r#"{{"rules":[{l1}]}}"#)).fails(2, "remove_others_required");
    let methods: Vec<String> = server.requests().iter().map(|r| r.method.clone()).collect();
    assert_eq!(methods, ["GET", "PUT", "GET", "GET"]);
}

/// As for `cdn rules`, a key swagger does not list for a rule is refused
/// before anything is read or sent, under `--dry-run` too. So is a file that is not a
/// `rules` object, or a rule without a key swagger requires.
#[test]
fn rule_files_refuse_unknown_keys_and_other_shapes() {
    let server = Server::start(vec![]);
    let custom = "a rule has only `id`, `description`, `enabled`, `expression`, and `action`";
    let limit = "a rule has only `id`, `description`, `enabled`, `expression`, `action`, `key`, \
                 `period`, `requests`, and `timeout`";
    for (kind, noun, keys, rule) in [
        (
            "rules",
            "WAF custom rule",
            custom,
            r#"{"expression":"true","action":"block","expr":"x"}"#,
        ),
        (
            "rate-limits",
            "WAF rate limit",
            limit,
            concat!(
                r#"{"expression":"true","action":"block","key":"ip","period":10,"#,
                r#""requests":1,"timeout":10,"expr":"x"}"#
            ),
        ),
    ] {
        let file = format!(r#"{{"rules":[{rule}]}}"#);
        for flags in [
            &["--yes"][..],
            &["--dry-run"],
            &["--yes", "--remove-others"],
        ] {
            let mut argv = vec!["waf", kind, "update", ZONE, "--rules-file", "-"];
            argv.extend(flags);
            let error = piped(&server, &file, &argv).fails(2, "usage");
            assert_eq!(
                error["message"],
                format!(
                    "`rules[0]` from stdin has the key `expr`, which a {noun} does not have: {keys}"
                )
            );
            assert_eq!(
                error["hint"],
                "fix `rules[0]`; the zone keeps its rules until then"
            );
        }
    }
    let custom_shape = "the WAF custom rules from stdin are not a JSON object with a `rules` \
                        array of rules that each have `expression` and `action`";
    for shape in [
        "[]",
        r#"{"rule":[]}"#,
        r#"{"rules":{}}"#,
        "null",
        r#"{"rules":[{"expression":"true"}]}"#,
        r#"{"rules":["block"]}"#,
    ] {
        let error = piped(
            &server,
            shape,
            &[
                "waf",
                "rules",
                "update",
                ZONE,
                "--rules-file",
                "-",
                "--yes",
                "--remove-others",
            ],
        )
        .fails(2, "usage");
        assert_eq!(error["message"], custom_shape, "{shape}");
        assert_eq!(
            error["hint"],
            "write the rules as `{\"rules\": [...]}`, the shape `hipc cdn waf rules get` prints"
        );
    }
    let error = piped(
        &server,
        r#"{"rules":[{"expression":"true","action":"block"}]}"#,
        &[
            "waf",
            "rate-limits",
            "update",
            ZONE,
            "--rules-file",
            "-",
            "--yes",
        ],
    )
    .fails(2, "usage");
    assert_eq!(
        error["message"],
        "the WAF rate limits from stdin are not a JSON object with a `rules` array of rules \
         that each have `expression`, `action`, `key`, `period`, `requests`, and `timeout`"
    );
    piped(
        &server,
        "rules",
        &["waf", "rules", "update", ZONE, "--rules-file", "-"],
    )
    .fails(2, "usage");
    assert!(server.requests().is_empty());
}

#[test]
fn rules_update_dry_run_names_the_read_and_an_unreadable_answer_is_internal() {
    let server = Server::start(vec![]);
    let file = r#"{"rules":[]}"#;
    let doc = piped(
        &server,
        file,
        &[
            "waf",
            "rules",
            "update",
            ZONE,
            "--rules-file",
            "-",
            "--dry-run",
        ],
    )
    .ok();
    let url = format!("{}/api/cdn/zones/{ZONE}/waf/rules/", server.url());
    assert_eq!(doc["reads_first"]["method"], "GET");
    assert_eq!(doc["reads_first"]["url"], url);
    let methods: Vec<&Value> = doc["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| &r["method"])
        .collect();
    assert_eq!(methods, [&json!("GET"), &json!("PUT")]);
    assert_eq!(draft(&doc)["body"], json!({"rules": []}));
    assert_eq!(draft(&doc)["if_match"], "<the version the read returns>");
    assert_eq!(
        doc["requests"][1]["if_match"],
        "<the version the read returns>"
    );
    assert!(doc["requests"][0].get("if_match").is_none(), "{doc}");
    let doc = piped(
        &server,
        file,
        &[
            "waf",
            "rules",
            "update",
            ZONE,
            "--rules-file",
            "-",
            "--dry-run",
            "--remove-others",
        ],
    )
    .ok();
    // `--remove-others` still reads, for the version: the read, then the conditional PUT.
    assert_eq!(draft(&doc)["if_match"], "<the version the read returns>");
    assert_eq!(doc["reads_first"]["url"], url);
    assert_eq!(doc["requests"][0]["method"], "GET");
    assert_eq!(doc["requests"][1]["method"], "PUT");
    assert_eq!(
        doc["requests"][1]["if_match"],
        "<the version the read returns>"
    );
    // A read without a whole `version` cannot make the PUT conditional, so it is unreadable.
    for answer in [
        "{}",
        "[]",
        r#"{"rules":[{"action":"block"}],"version":1}"#,
        r#"{"rules":[]}"#,
        r#"{"rules":[],"version":"7"}"#,
        r#"{"rules":[],"version":null}"#,
        r#"{"rules":[],"version":-1}"#,
    ] {
        let server = Server::start(rule_routes("rate-limits/", answer));
        let error = piped(
            &server,
            file,
            &[
                "waf",
                "rate-limits",
                "update",
                ZONE,
                "--rules-file",
                "-",
                "--yes",
            ],
        )
        .fails(1, "internal");
        assert_eq!(
            error["message"],
            format!(
                "cannot get the WAF rate limits of the CDN zone `{ZONE}`: the API answered with \
                 a shape `hipc` does not read"
            )
        );
        assert_eq!(
            targets(&server),
            [format!("GET /api/cdn/zones/{ZONE}/waf/rate-limits/")]
        );
    }
}

#[test]
fn rollback_needs_yes_and_sends_an_empty_object() {
    let server = Server::start(vec![waf_route(
        "POST",
        "versions/3/rollback/",
        Reply::json(201, r#"{"number":8}"#),
    )]);
    let error = hipc(&server, &["waf", "versions", "rollback", ZONE, "3"])
        .fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!(
            "this would roll the WAF of the CDN zone `{ZONE}` back to version `3`, and `--yes` \
             was not passed"
        )
    );
    assert_eq!(
        hipc(
            &server,
            &["waf", "versions", "rollback", ZONE, "3", "--yes"]
        )
        .ok()["number"],
        8
    );
    assert_eq!(server.only_request().json(), json!({}));
    hipc(
        &server,
        &["waf", "versions", "rollback", ZONE, "x", "--yes"],
    )
    .fails(2, "usage");
    hipc(&server, &["waf", "versions", "get", ZONE, "../1"]).fails(2, "usage");
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn lists_create_update_and_delete() {
    let server = Server::start(vec![
        list_route("POST", "", Reply::json(201, r#"{"name":"bad_ips"}"#)),
        list_route(
            "PATCH",
            "bad_ips/",
            Reply::json(200, r#"{"description":"x"}"#),
        ),
        list_route("DELETE", "bad_ips/", Reply::json(204, "")),
    ]);
    // Items are one per line; `#` starts a comment, and blank lines are skipped.
    let items = "203.0.113.7\n# office\n  198.51.100.0/24  # vpn\n\n";
    let doc = piped(
        &server,
        items,
        &[
            "lists",
            "create",
            "bad_ips",
            "--kind",
            "ip",
            "--description",
            "Bad",
            "--items-file",
            "-",
        ],
    )
    .ok();
    assert_eq!(doc["name"], "bad_ips");
    assert_eq!(
        server.only_request().json(),
        json!({
            "name": "bad_ips",
            "kind": "ip",
            "description": "Bad",
            "items": ["203.0.113.7", "198.51.100.0/24"],
        })
    );
    // A list costs nothing, so neither a create nor a description change asks.
    let doc = hipc(
        &server,
        &[
            "lists",
            "create",
            "bad_ips",
            "--kind",
            "country",
            "--dry-run",
        ],
    )
    .ok();
    assert_eq!(
        draft(&doc)["body"],
        json!({"name": "bad_ips", "kind": "country"})
    );
    hipc(
        &server,
        &["lists", "update", "bad_ips", "--description", "x"],
    )
    .ok();
    assert_eq!(server.requests()[1].json(), json!({"description": "x"}));
    hipc(&server, &["lists", "delete", "bad_ips"]).fails(2, "confirmation_required");
    assert_eq!(
        hipc(&server, &["lists", "delete", "bad_ips", "--yes"]).ok(),
        json!({"name": "bad_ips", "deleted": true})
    );
    assert_eq!(server.requests().len(), 3);
}

/// A list name goes into the URL path, so it is held to swagger's pattern, and a
/// description to its 256 characters, before anything is sent.
#[test]
fn list_names_and_descriptions_are_checked_without_a_request() {
    let server = Server::start(vec![]);
    let long = "d".repeat(257);
    for bad in [
        &["lists", "get", "Bad"][..],
        &["lists", "get", "a/b"],
        &["lists", "get", &"a".repeat(33)],
        &["lists", "create", "ok", "--kind", "url"],
        &[
            "lists",
            "create",
            "ok",
            "--kind",
            "ip",
            "--description",
            &long,
        ],
        &["lists", "update", "ok"],
    ] {
        hipc(&server, bad).fails(2, "usage");
    }
    hipc(&server, &["lists", "get", &"a".repeat(32), "--dry-run"]).ok();
    let wide = "é".repeat(256);
    hipc(
        &server,
        &["lists", "update", "ok", "--description", &wide, "--dry-run"],
    )
    .ok();
    assert!(server.requests().is_empty());
}

#[test]
fn list_items_add_and_remove_send_the_items_in_the_body() {
    let server = Server::start(vec![
        list_route(
            "POST",
            "bad_ips/items/",
            Reply::json(201, r#"{"items":["1.2.3.4"]}"#),
        ),
        list_route("DELETE", "bad_ips/items/", Reply::json(204, "")),
    ]);
    // An added item changes what every zone using the list lets through, so it is confirmed.
    let refused = piped(
        &server,
        "1.2.3.4\n",
        &["lists", "items", "create", "bad_ips", "--items-file", "-"],
    )
    .fails(2, "confirmation_required");
    assert_eq!(
        refused["message"],
        "this would add 1 item to the WAF list `bad_ips`, and `--yes` was not passed"
    );
    assert!(server.requests().is_empty());
    let add = piped(
        &server,
        "1.2.3.4\n",
        &[
            "lists",
            "items",
            "create",
            "bad_ips",
            "--items-file",
            "-",
            "--yes",
        ],
    );
    assert_eq!(add.ok(), json!({"items": ["1.2.3.4"]}));
    // `hipc api` cannot send a body on DELETE; this command sends one itself.
    piped(
        &server,
        "1.2.3.4\n",
        &["lists", "items", "delete", "bad_ips", "--items-file", "-"],
    )
    .fails(2, "confirmation_required");
    let doc = piped(
        &server,
        "1.2.3.4\n5.6.7.8",
        &[
            "lists",
            "items",
            "delete",
            "bad_ips",
            "--items-file",
            "-",
            "--yes",
        ],
    )
    .ok();
    assert_eq!(
        doc,
        json!({"name": "bad_ips", "removed": ["1.2.3.4", "5.6.7.8"]})
    );
    let requests = server.requests();
    assert_eq!(requests[1].method, "DELETE");
    assert_eq!(requests[1].json(), json!({"items": ["1.2.3.4", "5.6.7.8"]}));
    assert_eq!(requests[1].header("content-type"), Some("application/json"));
    let dry = piped(
        &server,
        "1.2.3.4",
        &[
            "lists",
            "items",
            "delete",
            "bad_ips",
            "--items-file",
            "-",
            "--dry-run",
        ],
    )
    .ok();
    assert_eq!(draft(&dry)["body"], json!({"items": ["1.2.3.4"]}));
    assert_eq!(server.requests().len(), 2);
}

/// Adding or removing nothing is not a change, and an item file is UTF-8 text.
#[test]
fn an_item_file_with_no_item_or_not_text_is_refused() {
    let server = Server::start(vec![]);
    for verb in ["create", "delete"] {
        let error = piped(
            &server,
            "# only a comment\n\n",
            &[
                "lists",
                "items",
                verb,
                "bad_ips",
                "--items-file",
                "-",
                "--yes",
            ],
        )
        .fails(2, "usage");
        assert_eq!(
            error["message"],
            "the list of items from stdin holds no item"
        );
        assert_eq!(
            error["hint"],
            "write one item per line in the file passed to `--items-file`"
        );
    }
    let home = Home::new();
    let binary = home.file("items.txt", &[0xff]);
    let error = Hipc::against(&home, &server)
        .run(&[
            "cdn",
            "lists",
            "items",
            "create",
            "bad_ips",
            "--items-file",
            binary.to_str().unwrap(),
        ])
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        format!(
            "the list of items from `{}` is not UTF-8 text",
            binary.display()
        )
    );
    assert!(server.requests().is_empty());
}

/// A replace of a list's items is a whole-list write, so it takes the whole-list guard: an
/// item the list stores now that the file leaves out is refused without `--remove-others`.
/// The list stores items in canonical form, so the file is compared with that form.
#[test]
fn list_items_update_is_refused_when_it_drops_an_item() {
    let current = r#"{"name":"bad_ips","kind":"ip","items":["198.51.100.0/24","203.0.113.7"]}"#;
    let server = Server::start(vec![
        list_route("GET", "bad_ips/", Reply::json(200, current)),
        list_route("PUT", "bad_ips/items/", Reply::json(200, r#"{"items":[]}"#)),
    ]);
    let update = |items: &str, extra: &[&str]| {
        let mut argv = vec!["lists", "items", "update", "bad_ips", "--items-file", "-"];
        argv.extend(extra);
        piped(&server, items, &argv)
    };
    let error = update("203.0.113.7\n", &["--yes"]).fails(2, "remove_others_required");
    assert_eq!(
        error["message"],
        "this would remove 1 item that the WAF list `bad_ips` has now, and `--remove-others` \
         was not passed"
    );
    assert_eq!(
        error["hint"],
        "run `hipc cdn lists items update bad_ips --items-file - --yes --remove-others` to \
         remove `198.51.100.0/24`"
    );
    update("203.0.113.7\n198.51.100.0/24\n192.0.2.1\n", &["--yes"]).ok();
    assert_eq!(
        server.requests()[2].json(),
        json!({"items": ["203.0.113.7", "198.51.100.0/24", "192.0.2.1"]})
    );
    // An empty file clears the list, which is a removal.
    update("", &["--yes"]).fails(2, "remove_others_required");
    update("", &["--yes", "--remove-others"]).ok();
    assert_eq!(
        server.requests().last().unwrap().json(),
        json!({"items": []})
    );
    let doc = update("1.1.1.1", &["--dry-run"]).ok();
    assert_eq!(doc["reads_first"]["method"], "GET");
    for answer in [r#"{"items":[1]}"#, "{}", "[]"] {
        let server = Server::start(vec![list_route(
            "GET",
            "bad_ips/",
            Reply::json(200, answer),
        )]);
        let error = piped(
            &server,
            "1",
            &[
                "lists",
                "items",
                "update",
                "bad_ips",
                "--items-file",
                "-",
                "--yes",
            ],
        )
        .fails(1, "internal");
        assert_eq!(
            error["message"],
            "cannot get the WAF list `bad_ips`: the API answered with a shape `hipc` does not read"
        );
    }
}

#[test]
fn estimate_sends_waf_zones_by_tier() {
    let server = Server::start(vec![route(
        "POST",
        "/api/cdn/estimate/",
        Reply::json(201, "{}"),
    )]);
    hipc(
        &server,
        &["estimate", "--waf", "standard=2", "--waf", "advanced=0"],
    )
    .ok();
    assert_eq!(
        server.only_request().body,
        br#"{"waf":{"standard":2,"advanced":0}}"#
    );
    let error = hipc(
        &server,
        &["estimate", "--waf", "standard=1", "--waf", "standard=2"],
    )
    .fails(2, "usage");
    assert_eq!(
        error["message"],
        "`--waf` names the tier `standard` more than once"
    );
    for bad in [
        "standard",
        "pro=1",
        "standard=-1",
        "standard=1000001",
        "advanced=x",
    ] {
        hipc(&server, &["estimate", "--waf", bad]).fails(2, "usage");
    }
    hipc(
        &server,
        &["estimate", "--waf", "advanced=1000000", "--dry-run"],
    )
    .ok();
    assert_eq!(server.requests().len(), 1);
}

/// A rule file as the read answers it, with the version it is in.
fn versioned(rules: &str) -> String {
    let mut answer: Value = serde_json::from_str(rules).expect("a rule file is JSON");
    answer["version"] = json!(3);
    answer.to_string()
}

/// A version another client published between the guard's read and the PUT is not
/// overwritten: the server refuses the stale `If-Match` with 412 `waf_conflict`, which is a
/// `conflict` that changed nothing, and the hint runs the command again, which reads again.
#[test]
fn a_rules_update_racing_another_publish_is_a_conflict() {
    for kind in ["rules/", "rate-limits/"] {
        let current = if kind == "rules/" {
            CUSTOM_RULES
        } else {
            RATE_LIMITS
        };
        let server = Server::start(vec![
            waf_route("GET", kind, Reply::json(200, current)),
            waf_route(
                "PUT",
                kind,
                Reply::json(
                    412,
                    r#"{"error":"the WAF changed since version 7","code":"waf_conflict"}"#,
                ),
            ),
        ]);
        let command = kind.trim_end_matches('/');
        let error = piped(
            &server,
            current,
            &["waf", command, "update", ZONE, "--rules-file", "-", "--yes"],
        )
        .fails(1, "conflict");
        assert_eq!(error["http_status"], 412);
        assert_eq!(error["side_effects"], "none");
        assert_eq!(
            error["hint"],
            format!(
                "run `hipc cdn waf {command} update {ZONE} --rules-file - --yes` again, which \
                 reads the rules again and checks the file against them"
            )
        );
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].header("if-match"), Some("7"));
        // `--remove-others` sends the read's version too, and checks no file on a rerun.
        let error = piped(
            &server,
            current,
            &[
                "waf",
                command,
                "update",
                ZONE,
                "--rules-file",
                "-",
                "--yes",
                "--remove-others",
            ],
        )
        .fails(1, "conflict");
        assert_eq!(
            error["hint"],
            format!(
                "run `hipc cdn waf {command} update {ZONE} --rules-file - --yes --remove-others` \
                 again, which reads the rules again"
            )
        );
        assert_eq!(server.requests()[3].header("if-match"), Some("7"));
    }
}

/// Every command, as its arguments and its input on stdin, with the routes that answer it.
fn every_command() -> (Vec<(Vec<&'static str>, &'static str)>, Vec<Route>) {
    let rule = r#"{"rules":[{"expression":"true","action":"log"}]}"#;
    let limit = concat!(
        r#"{"rules":[{"expression":"true","action":"log","key":"ip","period":10,"#,
        r#""requests":1,"timeout":10}]}"#
    );
    let commands = vec![
        (vec!["waf", "get", ZONE], ""),
        (vec!["waf", "update", ZONE, "--mode", "off"], ""),
        (vec!["waf", "status", ZONE], ""),
        (vec!["waf", "analytics", ZONE], ""),
        (vec!["waf", "events", ZONE], ""),
        (vec!["waf", "test", ZONE, "--request-file", "-"], "{}"),
        (vec!["waf", "rules", "get", ZONE], ""),
        (
            vec!["waf", "rules", "update", ZONE, "--rules-file", "-"],
            rule,
        ),
        (vec!["waf", "rate-limits", "get", ZONE], ""),
        (
            vec!["waf", "rate-limits", "update", ZONE, "--rules-file", "-"],
            limit,
        ),
        (vec!["waf", "versions", "list", ZONE], ""),
        (vec!["waf", "versions", "get", ZONE, "1"], ""),
        (vec!["waf", "versions", "rollback", ZONE, "1"], ""),
        (vec!["lists", "list"], ""),
        (vec!["lists", "get", "l"], ""),
        (vec!["lists", "create", "l", "--kind", "asn"], ""),
        (vec!["lists", "update", "l", "--description", "d"], ""),
        (vec!["lists", "delete", "l"], ""),
        (
            vec!["lists", "items", "create", "l", "--items-file", "-"],
            "1",
        ),
        (
            vec!["lists", "items", "update", "l", "--items-file", "-"],
            "1",
        ),
        (
            vec!["lists", "items", "delete", "l", "--items-file", "-"],
            "1",
        ),
    ];
    let routes = vec![
        waf_route("GET", "", Reply::json(200, "{}")),
        waf_route("PATCH", "", Reply::json(200, "{}")),
        waf_route("GET", "status/", Reply::json(200, "{}")),
        waf_route("GET", "analytics/", Reply::json(200, "{}")),
        waf_route("GET", "events/", Reply::json(200, "{}")),
        waf_route("POST", "test/", Reply::json(201, "{}")),
        waf_route("GET", "rules/", Reply::json(200, &versioned(rule))),
        waf_route("PUT", "rules/", Reply::json(200, "{}")),
        waf_route("GET", "rate-limits/", Reply::json(200, &versioned(limit))),
        waf_route("PUT", "rate-limits/", Reply::json(200, "{}")),
        waf_route("GET", "versions/", Reply::json(200, "[]")),
        waf_route("GET", "versions/1/", Reply::json(200, "{}")),
        waf_route("POST", "versions/1/rollback/", Reply::json(201, "{}")),
        list_route("GET", "", Reply::json(200, "[]")),
        list_route("GET", "l/", Reply::json(200, r#"{"items":["1"]}"#)),
        list_route("POST", "", Reply::json(201, "{}")),
        list_route("PATCH", "l/", Reply::json(200, "{}")),
        list_route("DELETE", "l/", Reply::json(204, "")),
        list_route("POST", "l/items/", Reply::json(201, "{}")),
        list_route("PUT", "l/items/", Reply::json(200, "{}")),
        list_route("DELETE", "l/items/", Reply::json(204, "")),
    ];
    (commands, routes)
}

#[test]
fn a_membership_sends_the_account_header_on_every_route() {
    let (commands, routes) = every_command();
    let server = Server::start(routes);
    let home = Home::new();
    for (args, input) in &commands {
        let mut argv = vec!["cdn"];
        argv.extend(args);
        argv.push("--yes");
        Hipc::against(&home, &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .stdin(input.as_bytes())
            .run(&argv)
            .ok();
    }
    let requests = server.requests();
    // The three replaces read the current list first.
    assert_eq!(requests.len(), commands.len() + 3);
    for request in requests {
        assert_eq!(
            request.header("x-hippius-account"),
            Some(MEMBER),
            "{} {}",
            request.method,
            request.target
        );
    }
}

#[test]
fn every_command_dry_runs_without_a_request_or_a_token() {
    let (commands, _) = every_command();
    let server = Server::start(vec![]);
    let home = Home::new();
    for (args, input) in &commands {
        let mut argv = vec!["cdn"];
        argv.extend(args);
        argv.push("--dry-run");
        let doc = Hipc::new(&home)
            .env("HIPPIUS_API_URL", server.url())
            .stdin(input.as_bytes())
            .run(&argv)
            .ok();
        assert_eq!(doc["dry_run"], true, "{args:?}");
    }
    let no_token = |args: &[&str]| {
        Hipc::new(&Home::new())
            .env("HIPPIUS_API_URL", server.url())
            .run(args)
    };
    no_token(&["cdn", "lists", "list"]).fails(1, "auth");
    no_token(&["cdn", "waf", "versions", "rollback", ZONE, "1", "--yes"]).fails(1, "auth");
    assert!(server.requests().is_empty());
}
