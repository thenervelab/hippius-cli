//! `hipc compute public-ip` and `hipc compute firewall`: a VM's public IPv4 and the inbound
//! rules on it.

use serde_json::{Value, json};

use crate::common::{Hipc, Home, MEMBER, Recorded, Reply, Run, Server, VM, draft, parsed, route};

/// The attach answer, shaped as swagger documents it.
const ATTACHED: &str = concat!(
    r#"{"public_ip":{"address":"203.0.113.7","region":"FR","edge":"e1","#,
    r#""attached_at":"2026-10-27T10:00:00Z","state":"applying","applied_at":null,"#,
    r#""guest_inbound":{"state":"pending","last_seen_at":null},"network_ready":false},"#,
    r#""price_hourly":"0.5","currency":"USD","free_until":null,"#,
    r#""price_hourly_after_free":null}"#
);

/// The firewall document, shaped as swagger documents it.
const RULES: &str = concat!(
    r#"{"rules":[{"id":1,"protocol":"tcp","ports":["22"],"source_cidrs":[],"#,
    r#""description":"ssh","enabled":true}],"max_rules":50,"revision":"r2","#,
    r#""applied_revision":"r1","applied_at":null,"state":"applying","#,
    r#""guest_inbound":{"state":"active","last_seen_at":null},"network_ready":false}"#
);

fn vm_route(method: &'static str, suffix: &str, reply: Reply) -> crate::common::Route {
    route(method, &format!("/api/compute/vms/{VM}/{suffix}"), reply)
}

fn hipc(server: &Server, args: &[&str]) -> Run {
    let mut argv = vec!["compute"];
    argv.extend(args);
    Hipc::against(&Home::new(), server).run(&argv)
}

/// `firewall update` with `contents` as the rules file, plus `extra` flags.
fn update(server: &Server, contents: &str, extra: &[&str]) -> Run {
    let home = Home::new();
    let file = home.file("fw.json", contents.as_bytes());
    let mut argv = vec![
        "compute",
        "firewall",
        "update",
        VM,
        "--rules-file",
        file.to_str().expect("the test home path is UTF-8"),
    ];
    argv.extend(extra);
    Hipc::against(&home, server).run(&argv)
}

#[test]
fn availability_sends_the_region_only_when_given() {
    let body = r#"{"total_free":3,"regions":[],"price_hourly":"0.5","currency":"USD"}"#;
    let server = Server::start(vec![route(
        "GET",
        "/api/compute/public-ip/availability/",
        Reply::json(200, body),
    )]);
    assert_eq!(
        hipc(&server, &["public-ip", "availability"]).ok(),
        parsed(body)
    );
    hipc(&server, &["public-ip", "availability", "--region", "fr"]).ok();
    assert_eq!(
        server.targets(),
        [
            "GET /api/compute/public-ip/availability/",
            "GET /api/compute/public-ip/availability/?region=FR"
        ]
    );
    for bad in ["", "F", "FRA", "F1", "F/", "é"] {
        hipc(&server, &["public-ip", "availability", "--region", bad]).fails(2, "usage");
    }
    assert_eq!(server.requests().len(), 2);
    let failing = Server::start(vec![route(
        "GET",
        "/api/compute/public-ip/availability/",
        Reply::json(400, r#"{"error":"Malformed region"}"#),
    )]);
    hipc(&failing, &["public-ip", "availability", "--region", "ZZ"]).fails(1, "validation");
}

#[test]
fn get_prints_the_document_as_sent_and_a_missing_vm_is_not_found() {
    let none = r#"{"public_ip":null,"price_hourly":"0.5","currency":"USD"}"#;
    let server = Server::start(vec![vm_route("GET", "public-ip/", Reply::json(200, none))]);
    assert_eq!(hipc(&server, &["public-ip", "get", VM]).ok(), parsed(none));
    let missing = Server::start(vec![vm_route("GET", "public-ip/", Reply::not_found())]);
    hipc(&missing, &["public-ip", "get", VM]).fails(1, "not_found");
}

#[test]
fn create_needs_yes_and_sends_an_empty_post() {
    let server = Server::start(vec![vm_route(
        "POST",
        "public-ip/",
        Reply::json(201, ATTACHED),
    )]);
    let error = hipc(&server, &["public-ip", "create", VM]).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!("this would attach a public IPv4 to the VM `{VM}`, and `--yes` was not passed")
    );
    assert!(server.requests().is_empty());
    let doc = hipc(&server, &["public-ip", "create", VM, "--yes"]).ok();
    assert_eq!(doc, parsed(ATTACHED));
    let request = server.only_request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path(), format!("/api/compute/vms/{VM}/public-ip/"));
    assert_eq!(request.header("content-length"), Some("0"));
    assert_eq!(request.body, b"");
}

#[test]
fn create_says_inbound_is_blocked_only_in_text_mode() {
    let server = Server::start(vec![vm_route(
        "POST",
        "public-ip/",
        Reply::json(201, ATTACHED),
    )]);
    let text = |extra: &[&str]| {
        let mut argv = vec!["--output", "text", "compute", "public-ip", "create", VM];
        argv.extend(extra);
        Hipc::against(&Home::new(), &server).run(&argv)
    };
    let run = text(&["--yes"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        format!(
            "warning: all inbound traffic to the address is blocked until a firewall rule \
             accepts it, which `hipc compute firewall update {VM} --rules-file <path>` adds\n"
        )
    );
    assert!(run.stdout().contains("203.0.113.7"), "{}", run.stdout());
    assert!(
        !run.stdout().trim_start().starts_with('{'),
        "{}",
        run.stdout()
    );
    assert_eq!(text(&["--yes", "--quiet"]).stderr(), "");
    // An id that reads as a flag, given after `--`, goes after `--` in the warning too, so
    // the command it names runs as written.
    let dashed = Server::start(vec![route(
        "POST",
        "/api/compute/vms/-vm1/public-ip/",
        Reply::json(201, ATTACHED),
    )]);
    let argv = [
        "--output",
        "text",
        "compute",
        "public-ip",
        "create",
        "--yes",
        "--",
        "-vm1",
    ];
    let run = Hipc::against(&Home::new(), &dashed).run(&argv);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(
        run.stderr()
            .ends_with("`hipc compute firewall update --rules-file <path> -- -vm1` adds\n"),
        "{}",
        run.stderr()
    );
    let dry = text(&["--dry-run"]);
    assert_eq!(dry.code(), Some(0));
    assert_eq!(dry.stderr(), "", "a dry run attaches nothing");
    assert_eq!(server.requests().len(), 2);
    // JSON mode prints no note: `ok` asserts an empty stderr.
    hipc(&server, &["public-ip", "create", VM, "--yes"]).ok();
}

#[test]
fn create_errors_map_to_their_codes() {
    let cases: [(u16, &str, &str); 5] = [
        (409, r#"{"error":"no-free-public-ip"}"#, "conflict"),
        (409, r#"{"error":"vm-not-live"}"#, "conflict"),
        (403, r#"{"error":"Managed service"}"#, "permission"),
        (
            502,
            r#"{"error":"Control plane unreachable"}"#,
            "unavailable",
        ),
        (503, r#"{"error":"Not available"}"#, "unavailable"),
    ];
    for (status, body, code) in cases {
        let server = Server::start(vec![vm_route(
            "POST",
            "public-ip/",
            Reply::json(status, body),
        )]);
        let error = hipc(&server, &["public-ip", "create", VM, "--yes"]).fails(1, code);
        assert_eq!(error["http_status"], status, "{body}");
        assert_eq!(error["details"], parsed(body));
        assert_eq!(error["retryable"], false, "{body}");
        // A 5xx to the POST may have attached the address, so a retry is not offered.
        let effects = if status >= 500 { "partial" } else { "none" };
        assert_eq!(error["side_effects"], effects, "{body}");
    }
}

#[test]
fn delete_needs_yes_and_names_the_vm_on_204() {
    let server = Server::start(vec![vm_route("DELETE", "public-ip/", Reply::json(204, ""))]);
    let error = hipc(&server, &["public-ip", "delete", VM]).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!("this would detach the public IPv4 from the VM `{VM}`, and `--yes` was not passed")
    );
    assert!(server.requests().is_empty());
    let doc = hipc(&server, &["public-ip", "delete", VM, "--yes"]).ok();
    assert_eq!(doc, json!({ "vm_id": VM, "public_ip": null }));
    assert_eq!(server.only_request().method, "DELETE");
    let with_body = Server::start(vec![vm_route(
        "DELETE",
        "public-ip/",
        Reply::json(200, r#"{"vm_id":"x","public_ip":null,"note":"y"}"#),
    )]);
    let doc = hipc(&with_body, &["public-ip", "delete", VM, "--yes"]).ok();
    assert_eq!(doc["note"], "y");
}

#[test]
fn delete_errors_map_to_their_codes() {
    for (status, code, effects) in [
        (404, "not_found", "none"),
        (502, "unavailable", "partial"),
        (503, "unavailable", "partial"),
    ] {
        let server = Server::start(vec![vm_route(
            "DELETE",
            "public-ip/",
            Reply::json(status, r#"{"error":"x"}"#),
        )]);
        let error = hipc(&server, &["public-ip", "delete", VM, "--yes"]).fails(1, code);
        assert_eq!(error["retryable"], false, "{status}");
        assert_eq!(error["side_effects"], effects, "{status}");
    }
}

#[test]
fn usage_sends_the_range_only_when_given_and_refuses_an_unknown_one() {
    let server = Server::start(vec![vm_route(
        "GET",
        "public-ip/usage/",
        Reply::json(200, r#"{"range":"24h","step_s":300,"points":[]}"#),
    )]);
    assert_eq!(
        hipc(&server, &["public-ip", "usage", VM]).ok()["step_s"],
        300
    );
    for range in ["1h", "24h", "7d", "30d"] {
        hipc(&server, &["public-ip", "usage", VM, "--range", range]).ok();
    }
    let path = format!("/api/compute/vms/{VM}/public-ip/usage/");
    assert_eq!(
        server.targets(),
        [
            format!("GET {path}"),
            format!("GET {path}?range=1h"),
            format!("GET {path}?range=24h"),
            format!("GET {path}?range=7d"),
            format!("GET {path}?range=30d"),
        ]
    );
    for bad in ["2h", "", "1H", "week"] {
        hipc(&server, &["public-ip", "usage", VM, "--range", bad]).fails(2, "usage");
    }
    assert_eq!(server.requests().len(), 5);
}

#[test]
fn a_vm_id_that_could_change_the_route_is_usage_without_a_request() {
    let server = Server::start(vec![]);
    let long = "a".repeat(65);
    for id in ["", "../x", "a/b", "a?b", "a.b", "a b", "%2e", long.as_str()] {
        for args in [
            &["public-ip", "get", id][..],
            &["public-ip", "create", id, "--yes"],
            &["public-ip", "delete", id, "--yes"],
            &["public-ip", "usage", id],
            &["firewall", "get", id],
            &["firewall", "update", id, "--rules-file", "-", "--yes"],
        ] {
            hipc(&server, args).fails(2, "usage");
        }
    }
    assert!(server.requests().is_empty());
}

#[test]
fn a_membership_sends_the_header_on_every_route() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/compute/public-ip/availability/",
            Reply::json(200, "{}"),
        ),
        vm_route("GET", "public-ip/", Reply::json(200, ATTACHED)),
        vm_route("POST", "public-ip/", Reply::json(201, ATTACHED)),
        vm_route("DELETE", "public-ip/", Reply::json(204, "")),
        vm_route("GET", "public-ip/usage/", Reply::json(200, "{}")),
        vm_route("GET", "firewall/", Reply::json(200, RULES)),
        vm_route("PUT", "firewall/", Reply::json(200, RULES)),
    ]);
    let home = Home::new();
    // The rule `RULES` holds, so the update removes nothing, and both its read and its PUT
    // go out.
    let file = home.file(
        "fw.json",
        br#"{"rules":[{"protocol":"tcp","ports":["22"],"source_cidrs":[]}]}"#,
    );
    let file = file.to_str().expect("the test home path is UTF-8");
    let member = |args: &[&str]| {
        let mut argv = vec!["compute"];
        argv.extend(args);
        Hipc::against(&home, &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&argv)
    };
    member(&["public-ip", "availability"]).ok();
    member(&["public-ip", "get", VM]).ok();
    member(&["public-ip", "create", VM, "--yes"]).ok();
    member(&["public-ip", "delete", VM, "--yes"]).ok();
    member(&["public-ip", "usage", VM]).ok();
    member(&["firewall", "get", VM]).ok();
    member(&["firewall", "update", VM, "--rules-file", file, "--yes"]).ok();
    let requests = server.requests();
    assert_eq!(requests.len(), 8);
    for request in requests {
        assert_eq!(
            request.header("x-hippius-account"),
            Some(MEMBER),
            "{}",
            request.target
        );
    }
    let dry = member(&["public-ip", "create", VM, "--dry-run"]).ok();
    assert_eq!(draft(&dry)["account_header"], MEMBER);
    let error = member(&["public-ip", "create", VM]).fails(2, "confirmation_required");
    assert!(
        error["message"]
            .as_str()
            .expect("the message is a string")
            .contains(&format!("to the VM `{VM}` in the account `{MEMBER}`, and")),
        "{error}"
    );
    assert_eq!(server.requests().len(), 8);
}

#[test]
fn firewall_get_prints_the_rules_as_sent() {
    let server = Server::start(vec![vm_route("GET", "firewall/", Reply::json(200, RULES))]);
    assert_eq!(hipc(&server, &["firewall", "get", VM]).ok(), parsed(RULES));
    let text = Hipc::against(&Home::new(), &server)
        .run(&["--output", "text", "compute", "firewall", "get", VM]);
    assert_eq!(text.code(), Some(0), "{}", text.stderr());
    assert!(text.stdout().contains("r2"), "{}", text.stdout());
    assert!(!text.stdout().trim_start().starts_with('{'));
    let missing = Server::start(vec![vm_route("GET", "firewall/", Reply::not_found())]);
    hipc(&missing, &["firewall", "get", VM]).fails(1, "not_found");
}

#[test]
fn firewall_update_sends_the_rules_unchanged_and_the_revision_only_when_given() {
    let server = Server::start(vec![vm_route("PUT", "firewall/", Reply::json(200, RULES))]);
    // The rules as `get` prints them, with a number no float holds. Only `rules` goes out,
    // each rule byte for byte.
    let rules = concat!(
        r#"[{"id":12345678901234567890123,"protocol":"tcp","ports":["22","8000-8080"],"#,
        r#""source_cidrs":["203.0.113.0/24"],"description":"web","enabled":true},"#,
        r#"{"protocol":"icmp","ports":[]},{"protocol":"udp"}]"#
    );
    let file = format!(r#"{{"rules":{rules},"max_rules":50,"revision":"r2"}}"#);
    let doc = update(&server, &file, &["--yes", "--remove-others"]).ok();
    assert_eq!(doc["revision"], "r2");
    update(
        &server,
        &file,
        &["--yes", "--remove-others", "--base-revision", "r2"],
    )
    .ok();
    let requests = server.requests();
    let sent: Vec<&[u8]> = requests.iter().map(|r| r.body.as_slice()).collect();
    assert_eq!(
        sent,
        [
            format!(r#"{{"rules":{rules}}}"#).as_bytes(),
            format!(r#"{{"rules":{rules},"base_revision":"r2"}}"#).as_bytes(),
        ]
    );
    for request in requests {
        assert_eq!(request.method, "PUT");
        assert_eq!(request.path(), format!("/api/compute/vms/{VM}/firewall/"));
    }
}

#[test]
fn firewall_update_reads_the_rules_from_stdin() {
    let server = Server::start(vec![vm_route("PUT", "firewall/", Reply::json(200, RULES))]);
    Hipc::against(&Home::new(), &server)
        .stdin(br#"{"rules":[{"protocol":"tcp","ports":["443"]}]}"#)
        .run(&[
            "compute",
            "firewall",
            "update",
            VM,
            "--rules-file",
            "-",
            "--yes",
            "--remove-others",
        ])
        .ok();
    assert_eq!(
        server.only_request().json(),
        json!({"rules": [{"protocol": "tcp", "ports": ["443"]}]})
    );
    let refused = Hipc::against(&Home::new(), &server)
        .stdin(br#"{"rules":[{"protocol":"tcp","ports":["443"]}]}"#)
        .run(&[
            "compute",
            "firewall",
            "update",
            VM,
            "--rules-file",
            "-",
            "--remove-others",
        ])
        .fails(2, "confirmation_required");
    let message = refused["message"]
        .as_str()
        .expect("the message is a string");
    assert!(
        message.contains("with 1 rule from stdin, which accepts every IPv4 address"),
        "{message}"
    );
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn firewall_update_needs_yes_and_says_what_it_opens_or_removes() {
    let server = Server::start(vec![vm_route("PUT", "firewall/", Reply::json(200, RULES))]);
    let message = |contents: &str| {
        let error =
            update(&server, contents, &["--remove-others"]).fails(2, "confirmation_required");
        error["message"]
            .as_str()
            .expect("the message is a string")
            .to_owned()
    };
    assert_eq!(
        message(r#"{"rules":[]}"#),
        format!(
            "this would remove every inbound rule of the VM `{VM}`, and `--yes` was not passed"
        )
    );
    let one = message(r#"{"rules":[{"protocol":"tcp","ports":["22"]}]}"#);
    let start = format!("this would replace every inbound rule of the VM `{VM}` with 1 rule from");
    assert!(one.starts_with(&start), "{one}");
    // Swagger: an empty `source_cidrs` is any source, so the refusal says so, as the prompt
    // does.
    assert!(
        one.ends_with(concat!(
            "fw.json`, which accepts every IPv4 address (no `source_cidrs`, or a `/0`), and ",
            "`--yes` was not passed"
        )),
        "{one}"
    );
    let two = message(r#"{"rules":[{"protocol":"tcp"},{"protocol":"udp"}]}"#);
    assert!(two.contains("with 2 rules from `"), "{two}");
    assert!(
        two.contains(", of which 2 accept every IPv4 address"),
        "{two}"
    );
    // A `/0` anywhere in a rule's sources is every address too.
    let wide = message(concat!(
        r#"{"rules":[{"protocol":"tcp","ports":["22"]},{"protocol":"udp","ports":["53"],"#,
        r#""source_cidrs":["203.0.113.0/24","10.0.0.0/0"]}]}"#
    ));
    assert!(
        wide.contains(", of which 2 accept every IPv4 address"),
        "{wide}"
    );
    let one_of_two = message(concat!(
        r#"{"rules":[{"protocol":"tcp","ports":["22"],"source_cidrs":["10.0.0.0/8"]},"#,
        r#"{"protocol":"udp","ports":["53"],"source_cidrs":["0.0.0.0/0"]}]}"#
    ));
    assert!(
        one_of_two.contains(", of which 1 accepts every IPv4 address"),
        "{one_of_two}"
    );
    // A rule held to a narrow source names no exposure.
    let narrow = message(concat!(
        r#"{"rules":[{"protocol":"tcp","ports":["22"],"#,
        r#""source_cidrs":["203.0.113.0/24","198.51.100.7/32","1.2.3.0/10","10.0.0.0/20"]}]}"#
    ));
    assert!(!narrow.contains("every IPv4 address"), "{narrow}");
    assert!(!narrow.contains("every port"), "{narrow}");
    // Swagger: an empty `ports` is every port, so a TCP or UDP rule without ports is named
    // too. An ICMP rule has no ports, so it opens no port.
    let every_port = message(r#"{"rules":[{"protocol":"udp","source_cidrs":["10.0.0.0/8"]}]}"#);
    assert!(
        every_port.ends_with(concat!(
            "fw.json`, which accepts every port (no `ports`, or `[]`), and `--yes` was not ",
            "passed"
        )),
        "{every_port}"
    );
    let both = message(r#"{"rules":[{"protocol":"tcp","ports":[]}]}"#);
    assert!(
        both.ends_with(concat!(
            "fw.json`, which accepts every IPv4 address (no `source_cidrs`, or a `/0`), and ",
            "which accepts every port (no `ports`, or `[]`), and `--yes` was not passed"
        )),
        "{both}"
    );
    let mixed = message(concat!(
        r#"{"rules":[{"protocol":"tcp","source_cidrs":["10.0.0.0/8"]},"#,
        r#"{"protocol":"udp","source_cidrs":["10.0.0.0/8"]},"#,
        r#"{"protocol":"icmp","source_cidrs":["10.0.0.0/8"]},"#,
        r#"{"protocol":"tcp","ports":["22"]}]}"#
    ));
    assert!(mixed.contains(" with 4 rules from `"), "{mixed}");
    assert!(
        mixed.ends_with(concat!(
            "fw.json`, of which 1 accepts every IPv4 address (no `source_cidrs`, or a `/0`), ",
            "and of which 2 accept every port (no `ports`, or `[]`), and `--yes` was not passed"
        )),
        "{mixed}"
    );
    let icmp = message(r#"{"rules":[{"protocol":"icmp","source_cidrs":["10.0.0.0/8"]}]}"#);
    assert!(!icmp.contains("every port"), "{icmp}");
    assert!(server.requests().is_empty());
    // An empty list is allowed: it removes every rule.
    update(&server, r#"{"rules":[]}"#, &["--yes", "--remove-others"]).ok();
    assert_eq!(server.only_request().json(), json!({ "rules": [] }));
}

#[test]
fn firewall_update_dry_run_shows_the_body_without_a_request() {
    let server = Server::start(vec![]);
    let doc = update(
        &server,
        r#"{"rules":[{"protocol":"tcp","ports":["22"]}],"revision":"r9"}"#,
        &["--base-revision", "r9", "--dry-run"],
    )
    .ok();
    assert_eq!(draft(&doc)["method"], "PUT");
    assert_eq!(
        draft(&doc)["url"],
        format!("{}/api/compute/vms/{VM}/firewall/", server.url())
    );
    assert_eq!(
        draft(&doc)["body"],
        json!({"rules": [{"protocol": "tcp", "ports": ["22"]}], "base_revision": "r9"})
    );
    // A malformed file is refused under `--dry-run` too.
    update(&server, "{}", &["--dry-run"]).fails(2, "usage");
    assert!(server.requests().is_empty());
}

#[test]
fn a_changed_firewall_is_conflict_and_the_hint_names_firewall_get() {
    let server = Server::start(vec![vm_route(
        "PUT",
        "firewall/",
        Reply::json(409, r#"{"error":"firewall-changed"}"#),
    )]);
    let error = update(
        &server,
        r#"{"rules":[]}"#,
        &["--yes", "--remove-others", "--base-revision", "r1"],
    )
    .fails(1, "conflict");
    assert_eq!(error["details"], json!({ "error": "firewall-changed" }));
    assert_eq!(
        error["hint"],
        format!(
            "run `hipc compute firewall get {VM}`, edit the rules it prints, and pass its \
             `revision` as `--base-revision`"
        )
    );
    for (status, code, effects) in [
        (400, "validation", "none"),
        (403, "permission", "none"),
        (503, "unavailable", "partial"),
    ] {
        let server = Server::start(vec![vm_route(
            "PUT",
            "firewall/",
            Reply::json(status, r#"{"error":"x"}"#),
        )]);
        let error =
            update(&server, r#"{"rules":[]}"#, &["--yes", "--remove-others"]).fails(1, code);
        // Only a 409 names `firewall get`; a 503 may have changed the rules.
        let hint = (status == 503)
            .then_some("check whether the change was made before running the command again");
        assert_eq!(error["hint"].as_str(), hint, "{status}");
        assert_eq!(error["side_effects"], effects, "{status}");
        assert_eq!(error["retryable"], false, "{status}");
    }
}

#[test]
fn a_rules_file_that_is_not_a_rules_object_is_usage_without_a_request() {
    let server = Server::start(vec![]);
    for contents in [
        "",
        "not json",
        r#"{"rules":[]"#,
        "null",
        "[1,",
        r#"{"rule":[]}"#,
        r#"{"rules":{}}"#,
        r#"{"rules":null}"#,
    ] {
        let error = update(&server, contents, &["--yes"]).fails(2, "usage");
        let message = error["message"].as_str().expect("the message is a string");
        assert!(
            message.ends_with(
                "fw.json` is not a JSON object with a `rules` array, or an array of rules"
            ),
            "{contents}: {message}"
        );
        assert_eq!(
            error["hint"],
            "write the rules as `{\"rules\": [...]}`, the shape `hipc compute firewall get` \
             prints, or as a bare array"
        );
    }
    Hipc::against(&Home::new(), &server)
        .run(&[
            "compute",
            "firewall",
            "update",
            VM,
            "--rules-file",
            "/nonexistent/fw.json",
            "--yes",
        ])
        .fails(2, "usage");
    let huge = format!(r#"{{"rules":[],"pad":"{}"}}"#, "x".repeat(1024 * 1024));
    let error = update(&server, &huge, &["--yes"]).fails(2, "usage");
    let message = error["message"].as_str().expect("the message is a string");
    assert!(message.contains("larger than 1048576 bytes"), "{message}");
    assert!(server.requests().is_empty());
}

/// A bare array is the rule list, as `cdn rules update` reads one: it is sent as the
/// `rules` of the PUT body.
#[test]
fn a_bare_array_is_the_rule_list() {
    let server = Server::start(vec![vm_route("PUT", "firewall/", Reply::json(200, RULES))]);
    let rule = r#"{"protocol":"tcp","ports":["443"],"source_cidrs":["203.0.113.0/24"]}"#;
    update(&server, &format!("[{rule}]"), &["--yes", "--remove-others"]).ok();
    update(&server, "[]", &["--yes", "--remove-others"]).ok();
    let sent: Vec<Value> = server.requests().iter().map(Recorded::json).collect();
    assert_eq!(
        sent,
        [json!({ "rules": [parsed(rule)] }), json!({ "rules": [] }),]
    );
}

/// A rule key swagger does not list is refused: only swagger's keys are sent, and
/// anything else is `usage`. Nothing is sent, with
/// `--yes` and under `--dry-run` alike.
#[test]
fn a_rule_key_swagger_does_not_list_is_usage_without_a_request() {
    let server = Server::start(vec![vm_route("PUT", "firewall/", Reply::json(200, RULES))]);
    for (key, rule) in [
        (
            "sources",
            r#"{"protocol":"tcp","ports":["5432"],"sources":["203.0.113.7/32"]}"#,
        ),
        ("port", r#"{"protocol":"tcp","port":["22"]}"#),
        ("proto", r#"{"proto":"tcp"}"#),
        ("Enabled", r#"{"protocol":"tcp","Enabled":false}"#),
    ] {
        for flags in [&["--yes", "--remove-others"][..], &["--dry-run"]] {
            let contents = format!(r#"{{"rules":[{{"protocol":"udp"}},{rule}]}}"#);
            let error = update(&server, &contents, flags).fails(2, "usage");
            let message = error["message"].as_str().expect("the message is a string");
            assert!(message.starts_with("`rules[1]` from `"), "{message}");
            assert!(
                message.ends_with(&format!(
                    "fw.json` has the key `{key}`, which a firewall rule does not have: a rule \
                     has only `id`, `protocol`, `ports`, `source_cidrs`, `description`, and \
                     `enabled`"
                )),
                "{message}"
            );
            assert_eq!(
                error["hint"],
                "fix `rules[1]`; the VM keeps its rules until then"
            );
        }
    }
    assert!(server.requests().is_empty());
    // Every key swagger lists is accepted.
    let full = concat!(
        r#"[{"id":3,"protocol":"tcp","ports":["22"],"source_cidrs":["10.0.0.0/8"],"#,
        r#""description":"ssh","enabled":false}]"#
    );
    update(&server, full, &["--yes", "--remove-others"]).ok();
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn more_than_fifty_rules_is_usage_without_a_request() {
    let server = Server::start(vec![vm_route("PUT", "firewall/", Reply::json(200, RULES))]);
    let rule = r#"{"protocol":"tcp","ports":["22"],"source_cidrs":["203.0.113.0/24"]}"#;
    let list = |count: usize| format!(r#"{{"rules":[{}]}}"#, vec![rule; count].join(","));
    let error = update(&server, &list(51), &["--yes"]).fails(2, "usage");
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|m| m.contains("has 51 rules, and a VM's firewall holds at most 50")),
        "{error}"
    );
    assert!(server.requests().is_empty());
    update(&server, &list(50), &["--yes", "--remove-others"]).ok();
}

#[test]
fn a_malformed_rule_refuses_the_whole_list_without_a_request() {
    let server = Server::start(vec![]);
    let no_protocol = "has no `protocol` of `tcp`, `udp`, or `icmp`";
    let cases = [
        ("1", "is not a JSON object"),
        (r#"{"ports":["22"]}"#, no_protocol),
        (r#"{"protocol":"TCP"}"#, no_protocol),
        (r#"{"protocol":6}"#, no_protocol),
        (
            r#"{"protocol":"icmp","ports":["22"]}"#,
            "is `icmp`, which has no ports, so its `ports` must be `[]`",
        ),
        (
            r#"{"protocol":"tcp","ports":"22"}"#,
            "has a `ports` that is not an array of strings",
        ),
        (
            r#"{"protocol":"tcp","ports":[22]}"#,
            "has a `ports` that is not an array of strings",
        ),
        (
            r#"{"protocol":"tcp","source_cidrs":"0.0.0.0/0"}"#,
            "has a `source_cidrs` that is not an array of strings",
        ),
        // A leading zero is a second spelling of a prefix: `/00` is every address.
        (
            r#"{"protocol":"tcp","source_cidrs":["0.0.0.0/00"]}"#,
            "has the source `0.0.0.0/00`: a source is an IPv4 CIDR, such as `203.0.113.0/24`",
        ),
        (
            r#"{"protocol":"tcp","source_cidrs":["10.0.0.0/08"]}"#,
            "has the source `10.0.0.0/08`: a source is an IPv4 CIDR, such as `203.0.113.0/24`",
        ),
    ];
    for (rule, problem) in cases {
        // The bad rule is second, after a good one, so the place it names is `rules[1]`.
        let contents = format!(r#"{{"rules":[{{"protocol":"tcp"}},{rule}]}}"#);
        let error = update(&server, &contents, &["--yes"]).fails(2, "usage");
        let message = error["message"].as_str().expect("the message is a string");
        assert!(message.starts_with("`rules[1]` from `"), "{message}");
        assert!(
            message.ends_with(&format!("fw.json` {problem}")),
            "{message}"
        );
        assert_eq!(
            error["hint"],
            "fix `rules[1]`; the VM keeps its rules until then"
        );
    }
    for port in [
        "0",
        "65536",
        "",
        "-",
        "22-",
        "-22",
        "80-22",
        "+22",
        " 22",
        "1-2-3",
        "123456",
        "ab",
        // A leading zero is a second spelling of a port, as it is of a prefix.
        "022",
        "08000-8080",
        "8000-08080",
        "00",
    ] {
        let contents = format!(r#"{{"rules":[{{"protocol":"tcp","ports":["22","{port}"]}}]}}"#);
        let error = update(&server, &contents, &["--yes"]).fails(2, "usage");
        let message = error["message"].as_str().expect("the message is a string");
        assert!(
            message.ends_with(&format!(
                "has the port `{port}`: a port is `1` to `65535`, or a range such as `8000-8080`"
            )),
            "{message}"
        );
    }
    for cidr in [
        "10.0.0.0",
        "10.0.0.0/33",
        "10.0.0.0/",
        "10.0.0.0/123",
        "10.0.0.0/+8",
        "10.0.0/8",
        "256.0.0.0/8",
        "::/0",
        "",
    ] {
        let contents = format!(r#"{{"rules":[{{"protocol":"udp","source_cidrs":["{cidr}"]}}]}}"#);
        let error = update(&server, &contents, &["--yes"]).fails(2, "usage");
        let message = error["message"].as_str().expect("the message is a string");
        assert!(
            message.ends_with(&format!(
                "has the source `{cidr}`: a source is an IPv4 CIDR, such as `203.0.113.0/24`"
            )),
            "{message}"
        );
    }
    assert!(server.requests().is_empty());
}

#[test]
fn the_edges_of_ports_and_sources_are_accepted() {
    let server = Server::start(vec![vm_route("PUT", "firewall/", Reply::json(200, RULES))]);
    let contents = concat!(
        r#"{"rules":[{"protocol":"tcp","ports":["1","65535","1-65535","443-443"],"#,
        r#""source_cidrs":["0.0.0.0/0","255.255.255.255/32","10.1.2.3/8"]}]}"#
    );
    update(&server, contents, &["--yes", "--remove-others"]).ok();
    assert_eq!(server.requests().len(), 1);
}
/// `firewall update` with `rules` on stdin, against a VM whose firewall is `RULES` (one rule,
/// `tcp` port `22` from any source), plus `extra` flags.
fn update_stdin(server: &Server, rules: &str, extra: &[&str]) -> Run {
    let mut argv = vec!["compute", "firewall", "update", VM, "--rules-file", "-"];
    argv.extend(extra);
    Hipc::against(&Home::new(), server)
        .stdin(rules.as_bytes())
        .run(&argv)
}

/// The firewall routes, with `RULES` as the current list.
fn firewall_routes() -> Vec<crate::common::Route> {
    vec![
        vm_route("GET", "firewall/", Reply::json(200, RULES)),
        vm_route("PUT", "firewall/", Reply::json(200, RULES)),
    ]
}

/// The whole-list guard: a rule the VM has now that the file leaves out is refused without
/// `--remove-others`, after the read and before the question, and the PUT is never sent.
#[test]
fn firewall_update_that_drops_a_rule_is_refused_without_remove_others() {
    let ssh = r#"`{"protocol":"tcp","ports":["22"],"source_cidrs":[],"enabled":true}`"#;
    for rules in [
        r#"{"rules":[]}"#,
        r#"{"rules":[{"protocol":"udp","ports":["53"]}]}"#,
        // Another port, or a narrower source, accepts something else: the old rule goes.
        r#"{"rules":[{"protocol":"tcp","ports":["2222"]}]}"#,
        r#"{"rules":[{"protocol":"tcp","ports":["22"],"source_cidrs":["10.0.0.0/8"]}]}"#,
        // Disabling the rule closes its port, as removing it would.
        r#"{"rules":[{"protocol":"tcp","ports":["22"],"enabled":false}]}"#,
    ] {
        let server = Server::start(firewall_routes());
        let error = update_stdin(&server, rules, &["--yes"]).fails(2, "remove_others_required");
        assert_eq!(
            error["message"],
            format!(
                "this would remove 1 inbound rule that the VM `{VM}` has now, and \
                 `--remove-others` was not passed"
            ),
            "{rules}"
        );
        assert_eq!(
            error["hint"],
            format!(
                "run `hipc compute firewall update {VM} --rules-file - --yes --remove-others` \
                 to remove {ssh}"
            )
        );
        assert_eq!(error["side_effects"], "none");
        assert_eq!(
            server.targets(),
            [format!("GET /api/compute/vms/{VM}/firewall/")]
        );
        // Without `--yes` the guard still comes first: the read decides before the question.
        update_stdin(&server, rules, &[]).fails(2, "remove_others_required");
        // `--remove-others` sends the list as given, and reads nothing.
        let server = Server::start(firewall_routes());
        update_stdin(&server, rules, &["--yes", "--remove-others"]).ok();
        assert_eq!(server.only_request().method, "PUT");
    }
}

/// A rule is what it accepts and whether it is enabled: its description, `id`, and the
/// order of its ports and sources may change, and the rules may be reordered or added to,
/// without a removal. A rule without `enabled` counts as enabled.
#[test]
fn firewall_update_that_keeps_every_rule_needs_no_remove_others() {
    for kept in [
        r#"{"id":9,"protocol":"tcp","ports":["22","22"],"description":"new","enabled":true}"#,
        r#"{"protocol":"tcp","ports":["22"]}"#,
        // Swagger reads an empty `source_cidrs` as any source, which a `/0` also is.
        r#"{"protocol":"tcp","ports":["22"],"source_cidrs":["0.0.0.0/0"]}"#,
        r#"{"protocol":"tcp","ports":["22"],"source_cidrs":["10.0.0.0/8","0.0.0.0/0"]}"#,
    ] {
        let server = Server::start(firewall_routes());
        let rules = format!(r#"{{"rules":[{{"protocol":"udp","ports":["53"]}},{kept}]}}"#);
        update_stdin(&server, &rules, &["--yes"]).ok();
        assert_eq!(
            server.targets(),
            [
                format!("GET /api/compute/vms/{VM}/firewall/"),
                format!("PUT /api/compute/vms/{VM}/firewall/"),
            ]
        );
        // Nothing removed and no `--yes`: the read goes out, then the question refuses.
        let server = Server::start(firewall_routes());
        update_stdin(&server, &rules, &[]).fails(2, "confirmation_required");
        assert_eq!(server.only_request().method, "GET");
    }
}

/// Rules are compared by what they accept, so a rule the server spells another way is kept:
/// a port with leading zeros, and a `/0` source against `[]`. A rule that
/// accepts less is still a removal, and so is another spelling of the same ports, `22-22`
/// for `22` or `1-65535` for `[]`: the file is sent unchanged, and swagger defines only `[]`
/// as every port.
#[test]
fn firewall_update_compares_rules_by_what_they_accept() {
    let current = concat!(
        r#"{"rules":[{"protocol":"tcp","ports":["022"],"source_cidrs":["0.0.0.0/0"]},"#,
        r#"{"protocol":"udp","ports":["01-065535"],"source_cidrs":[]},"#,
        r#"{"protocol":"tcp","ports":["443","080"],"source_cidrs":["203.0.113.0/24"]}],"#,
        r#""revision":"r3"}"#
    );
    let routes = || {
        vec![
            vm_route("GET", "firewall/", Reply::json(200, current)),
            vm_route("PUT", "firewall/", Reply::json(200, current)),
        ]
    };
    let rest = r#"{"protocol":"tcp","ports":["80","443"],"source_cidrs":["203.0.113.0/24"]}]"#;
    let same = format!(
        r#"[{{"protocol":"tcp","ports":["22"]}},{{"protocol":"udp","ports":["1-65535"]}},{rest}"#
    );
    let server = Server::start(routes());
    update_stdin(&server, &same, &["--yes"]).ok();
    assert_eq!(server.requests().len(), 2);
    // A narrower source than any, one port fewer, or another spelling of the same ports:
    // each removes its rule.
    for changed in [
        r#"[{"protocol":"tcp","ports":["22"],"source_cidrs":["10.0.0.0/8"]},"#,
        r#"[{"protocol":"tcp","ports":["22-22"]},"#,
    ] {
        let rules = format!(r#"{changed}{{"protocol":"udp","ports":["1-65535"]}},{rest}"#);
        let server = Server::start(routes());
        update_stdin(&server, &rules, &["--yes"]).fails(2, "remove_others_required");
        assert_eq!(server.only_request().method, "GET");
    }
    let ssh = r#"{"protocol":"tcp","ports":["22"]}"#;
    for narrower in [
        format!(r#"[{ssh},{{"protocol":"udp","ports":["1-65534"]}},{rest}"#),
        format!(r#"[{ssh},{{"protocol":"udp"}},{rest}"#),
    ] {
        let narrower = narrower.as_str();
        let server = Server::start(routes());
        update_stdin(&server, narrower, &["--yes"]).fails(2, "remove_others_required");
        assert_eq!(server.only_request().method, "GET");
    }
    // A server port or range `hipc` cannot read matches no port of a file, so leaving it out
    // is a removal: the guard fails safe. Such a range is kept as its text, its leading zero
    // too, where a range of two ports loses it.
    for port in ["http", "080-http"] {
        let odd =
            format!(r#"{{"rules":[{{"protocol":"tcp","ports":["{port}"]}}],"revision":"r4"}}"#);
        let server = Server::start(vec![vm_route("GET", "firewall/", Reply::json(200, &odd))]);
        let error = update_stdin(
            &server,
            r#"[{"protocol":"tcp","ports":["80"]}]"#,
            &["--yes"],
        )
        .fails(2, "remove_others_required");
        let removed =
            format!(r#"{{"protocol":"tcp","ports":["{port}"],"source_cidrs":[],"enabled":true}}"#);
        let hint = format!(
            "run `hipc compute firewall update {VM} --rules-file - --yes --remove-others` to \
             remove `{removed}`"
        );
        assert_eq!(error["hint"], hint);
    }
}

/// The rules are a multiset: two current rules with one key need two in the file, so one
/// copy does not keep both.
#[test]
fn firewall_update_keeps_a_repeated_rule_only_with_as_many_copies() {
    let twice = concat!(
        r#"{"rules":[{"protocol":"tcp","ports":["22"]},{"protocol":"tcp","ports":["22"]}],"#,
        r#""revision":"r2"}"#
    );
    let routes = || {
        vec![
            vm_route("GET", "firewall/", Reply::json(200, twice)),
            vm_route("PUT", "firewall/", Reply::json(200, twice)),
        ]
    };
    let server = Server::start(routes());
    let once = r#"{"rules":[{"protocol":"tcp","ports":["22"]}]}"#;
    let error = update_stdin(&server, once, &["--yes"]).fails(2, "remove_others_required");
    assert!(
        error["message"]
            .as_str()
            .expect("a message")
            .starts_with("this would remove 1 inbound rule that"),
        "{error}"
    );
    let server = Server::start(routes());
    update_stdin(&server, twice, &["--yes"]).ok();
    assert_eq!(server.requests().len(), 2);
}

/// The guard's read makes the PUT conditional: without `--base-revision` the PUT carries
/// the `revision` the read returned, so a rule another client adds in between is refused
/// with 409 rather than deleted. A `--base-revision` given is sent as it is.
#[test]
fn firewall_update_sends_the_revision_it_read() {
    let rules = r#"{"rules":[{"protocol":"tcp","ports":["22"]}]}"#;
    let server = Server::start(firewall_routes());
    update_stdin(&server, rules, &["--yes"]).ok();
    let put = &server.requests()[1];
    assert_eq!(put.method, "PUT");
    assert_eq!(
        put.json(),
        json!({"rules": [{"protocol": "tcp", "ports": ["22"]}], "base_revision": "r2"})
    );
    let server = Server::start(firewall_routes());
    update_stdin(&server, rules, &["--yes", "--base-revision", "r1"]).ok();
    assert_eq!(server.requests()[1].json()["base_revision"], "r1");
    // `--remove-others` reads nothing, so it has no revision to send.
    let server = Server::start(firewall_routes());
    update_stdin(&server, rules, &["--yes", "--remove-others"]).ok();
    assert!(server.only_request().json().get("base_revision").is_none());
}

#[test]
fn firewall_update_dry_run_names_the_read_that_finds_what_it_removes() {
    let server = Server::start(vec![]);
    let doc = update_stdin(&server, r#"{"rules":[]}"#, &["--dry-run"]).dry_ok();
    let url = format!("{}/api/compute/vms/{VM}/firewall/", server.url());
    let read = json!({
        "method": "GET",
        "url": url,
        "account_header": null,
        "project_header": null,
        "billing_header": null,
        "body": null,
    });
    // A real run sends the `revision` its read returns, so the PUT shows a placeholder.
    let put = json!({
        "method": "PUT",
        "url": url,
        "account_header": null,
        "project_header": null,
        "billing_header": null,
        "body": {"rules": [], "base_revision": "<the revision the read returns>"},
    });
    let expected = |put: &Value| {
        let mut doc = json!({ "dry_run": true });
        doc["reads_first"] = read.clone();
        doc["requests"] = json!([read, put]);
        doc
    };
    assert_eq!(doc, expected(&put));
    // A `--base-revision` given is sent as it is, after the same read.
    let doc = update_stdin(
        &server,
        r#"{"rules":[]}"#,
        &["--dry-run", "--base-revision", "r1"],
    )
    .dry_ok();
    let mut given = put.clone();
    given["body"]["base_revision"] = json!("r1");
    assert_eq!(doc, expected(&given));
    // `--remove-others` reads nothing, so it sends one request with no revision.
    let doc = update_stdin(
        &server,
        r#"{"rules":[]}"#,
        &["--dry-run", "--remove-others"],
    )
    .dry_ok();
    assert!(doc.get("reads_first").is_none(), "{doc}");
    assert_eq!(doc["requests"].as_array().map(Vec::len), Some(1), "{doc}");
    assert_eq!(draft(&doc)["body"], json!({"rules": []}));
    // An `enabled` that is not a boolean is refused in the file, before anything.
    let error = update_stdin(
        &server,
        r#"{"rules":[{"protocol":"tcp","enabled":"yes"}]}"#,
        &["--yes"],
    )
    .fails(2, "usage");
    assert!(
        error["message"]
            .as_str()
            .expect("a message")
            .ends_with("has an `enabled` that is not `true` or `false`"),
        "{error}"
    );
    assert!(server.requests().is_empty());
}

/// A read that fails, or answers a shape the guard does not read, ends the update before
/// the PUT. A malformed `ports`, `source_cidrs`, or `enabled` is not read as empty or
/// enabled, which could match a rule that opens everything, and a missing `revision` would
/// leave the PUT unconditional.
#[test]
fn firewall_update_whose_read_fails_sends_no_put() {
    let missing = Server::start(vec![
        vm_route("GET", "firewall/", Reply::not_found()),
        vm_route("PUT", "firewall/", Reply::json(200, RULES)),
    ]);
    update_stdin(&missing, r#"{"rules":[]}"#, &["--yes"]).fails(1, "not_found");
    assert_eq!(missing.only_request().method, "GET");
    for answer in [
        "{}",
        r#"{"rules":{}}"#,
        "[]",
        r#"{"rules":[7],"revision":"r"}"#,
        r#"{"rules":[{"protocol":"tcp","ports":"22"}],"revision":"r"}"#,
        r#"{"rules":[{"protocol":"tcp","source_cidrs":[1]}],"revision":"r"}"#,
        r#"{"rules":[{"protocol":"tcp","enabled":"yes"}],"revision":"r"}"#,
        r#"{"rules":[]}"#,
        r#"{"rules":[],"revision":7}"#,
    ] {
        let odd = Server::start(vec![
            vm_route("GET", "firewall/", Reply::json(200, answer)),
            vm_route("PUT", "firewall/", Reply::json(200, RULES)),
        ]);
        let error = update_stdin(&odd, r#"{"rules":[]}"#, &["--yes"]).fails(1, "internal");
        assert_eq!(
            error["message"],
            format!(
                "cannot get the firewall of the VM `{VM}`: the API answered with a shape `hipc` \
                 does not read"
            ),
            "{answer}"
        );
        assert_eq!(odd.only_request().method, "GET");
    }
    // With `--base-revision` the read's revision is not needed.
    let given = Server::start(vec![
        vm_route("GET", "firewall/", Reply::json(200, r#"{"rules":[]}"#)),
        vm_route("PUT", "firewall/", Reply::json(200, RULES)),
    ]);
    update_stdin(
        &given,
        r#"{"rules":[]}"#,
        &["--yes", "--base-revision", "r1"],
    )
    .ok();
    assert_eq!(given.requests().len(), 2);
}

/// A 409 on a PUT whose revision came from the guard's own read means the rules changed
/// since that read: running the same command again reads them again, so the hint says to.
#[test]
fn a_conflict_after_the_guards_read_says_to_run_the_command_again() {
    let server = Server::start(vec![
        vm_route("GET", "firewall/", Reply::json(200, RULES)),
        vm_route(
            "PUT",
            "firewall/",
            Reply::json(409, r#"{"error":"firewall-changed"}"#),
        ),
    ]);
    let rules = r#"{"rules":[{"protocol":"tcp","ports":["22"]}]}"#;
    let error = update_stdin(&server, rules, &["--yes"]).fails(1, "conflict");
    assert_eq!(
        error["hint"],
        format!(
            "run `hipc compute firewall update {VM} --rules-file - --yes` again, which reads \
             the rules again and checks the file against them"
        )
    );
    assert_eq!(server.requests()[1].json()["base_revision"], "r2");
}
/// With `--remove-others` nothing is read and the PUT carries no revision, so a 409 there
/// gets the get-and-edit hint, not the one that says the command reads the rules again.
#[test]
fn a_conflict_with_remove_others_names_firewall_get() {
    let server = Server::start(vec![vm_route(
        "PUT",
        "firewall/",
        Reply::json(409, r#"{"error":"firewall-changed"}"#),
    )]);
    let rules = r#"{"rules":[{"protocol":"tcp","ports":["22"]}]}"#;
    let error = update_stdin(&server, rules, &["--yes", "--remove-others"]).fails(1, "conflict");
    assert_eq!(
        error["hint"],
        format!(
            "run `hipc compute firewall get {VM}`, edit the rules it prints, and pass its \
             `revision` as `--base-revision`"
        )
    );
    assert!(server.only_request().json().get("base_revision").is_none());
}
