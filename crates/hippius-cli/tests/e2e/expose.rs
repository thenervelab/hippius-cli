//! `hipc compute exposures`: a VM's ports published on the public edge.

use serde_json::{Value, json};

use crate::common::{Hipc, Home, MEMBER, Reply, Run, Server, draft, route};

const VM: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

/// An `http` exposure, shaped as swagger documents it: the edge can read it.
const HTTP: &str = concat!(
    r#"{"id":7,"hostname":"a1b2.edge.example","url":"https://a1b2.edge.example","#,
    r#""port":8080,"mode":"http","protocol":"tcp","public_port":null,"#,
    r#""endpoint":"https://a1b2.edge.example","enabled":true,"edge_sees_traffic":true,"#,
    r#""created_at":"2026-10-27T10:00:00Z"}"#
);

/// An `sni` exposure: end to end, so the edge cannot read it.
const SNI: &str = concat!(
    r#"{"id":8,"hostname":"c3d4.edge.example","url":null,"port":443,"mode":"sni","#,
    r#""protocol":"tcp","public_port":null,"endpoint":"c3d4.edge.example","enabled":true,"#,
    r#""edge_sees_traffic":false,"created_at":"2026-10-27T10:00:00Z"}"#
);

fn expose_route(method: &'static str, suffix: &str, reply: Reply) -> crate::common::Route {
    route(
        method,
        &format!("/api/compute/vms/{VM}/expose/{suffix}"),
        reply,
    )
}

fn hipc(server: &Server, args: &[&str]) -> Run {
    let mut argv = vec!["compute", "exposures"];
    argv.extend(args);
    Hipc::against(&Home::new(), server).run(&argv)
}

/// `exposures create` in text mode, with `extra` after the VM id.
fn create_text(server: &Server, extra: &[&str]) -> Run {
    let mut argv = vec!["--output", "text", "compute", "exposures", "create", VM];
    argv.extend(extra);
    Hipc::against(&Home::new(), server).run(&argv)
}

fn parsed(text: &str) -> Value {
    serde_json::from_str(text).expect("the fixture is JSON")
}

#[test]
fn list_prints_the_envelope_from_a_bare_array() {
    let server = Server::start(vec![expose_route(
        "GET",
        "",
        Reply::json(200, &format!("[{HTTP},{SNI}]")),
    )]);
    let doc = hipc(&server, &["list", VM]).ok();
    assert_eq!(
        doc,
        json!({ "items": [parsed(HTTP), parsed(SNI)], "next_page_token": null })
    );
    let request = server.only_request();
    assert_eq!(request.method, "GET");
    assert_eq!(request.path(), format!("/api/compute/vms/{VM}/expose/"));
    let text = Hipc::against(&Home::new(), &server).run(&[
        "--output",
        "text",
        "compute",
        "exposures",
        "list",
        VM,
    ]);
    assert_eq!(text.code(), Some(0), "{}", text.stderr());
    assert!(
        text.stdout().contains("c3d4.edge.example"),
        "{}",
        text.stdout()
    );
    assert!(!text.stdout().trim_start().starts_with('{'));
    let missing = Server::start(vec![expose_route("GET", "", Reply::not_found())]);
    hipc(&missing, &["list", VM]).fails(1, "not_found");
}

#[test]
fn create_needs_yes_and_its_question_says_what_the_edge_can_read() {
    let server = Server::start(vec![]);
    let edge_reads = "where the edge terminates TLS and can read the traffic";
    let cases: [(&[&str], String); 5] = [
        (
            &[],
            format!(
                "port `8080` of the VM `{VM}` to the internet in `http` mode, the server's \
                 default, {edge_reads}"
            ),
        ),
        (
            &["--mode", "http"],
            format!("port `8080` of the VM `{VM}` to the internet in `http` mode, {edge_reads}"),
        ),
        (
            &["--mode", "sni", "--protocol", "tcp"],
            format!(
                "`tcp` port `8080` of the VM `{VM}` to the internet in `sni` mode, end to end: \
                 the edge forwards ciphertext it cannot open"
            ),
        ),
        (
            &["--mode", "raw", "--protocol", "udp"],
            format!(
                "`udp` port `8080` of the VM `{VM}` to the internet in `raw` mode, where the \
                 edge forwards the bytes as sent and can read the traffic unless the protocol \
                 inside encrypts it"
            ),
        ),
        (
            &["--mode", "raw"],
            format!(
                "port `8080` of the VM `{VM}` to the internet in `raw` mode, where the edge \
                 forwards the bytes as sent and can read the traffic unless the protocol inside \
                 encrypts it"
            ),
        ),
    ];
    for (extra, exposed) in cases {
        let mut argv = vec!["create", VM, "--port", "8080"];
        argv.extend(extra);
        let error = hipc(&server, &argv).fails(2, "confirmation_required");
        assert_eq!(
            error["message"],
            format!("this would expose {exposed}, and `--yes` was not passed"),
            "{extra:?}"
        );
    }
    assert!(server.requests().is_empty());
}

#[test]
fn create_sends_only_the_flags_passed() {
    let server = Server::start(vec![expose_route("POST", "", Reply::json(201, HTTP))]);
    let cases: [(&[&str], &str); 4] = [
        (&[], r#"{"port":8080}"#),
        (&["--mode", "sni"], r#"{"port":8080,"mode":"sni"}"#),
        (&["--protocol", "tcp"], r#"{"port":8080,"protocol":"tcp"}"#),
        (
            &["--mode", "raw", "--protocol", "udp"],
            r#"{"port":8080,"mode":"raw","protocol":"udp"}"#,
        ),
    ];
    for (extra, _) in cases {
        let mut argv = vec!["create", VM, "--port", "8080", "--yes"];
        argv.extend(extra);
        assert_eq!(hipc(&server, &argv).ok(), parsed(HTTP), "{extra:?}");
    }
    let requests = server.requests();
    let sent: Vec<&[u8]> = requests.iter().map(|r| r.body.as_slice()).collect();
    let expected: Vec<&[u8]> = cases.iter().map(|(_, body)| body.as_bytes()).collect();
    assert_eq!(sent, expected);
    for request in requests {
        assert_eq!(request.method, "POST");
        assert_eq!(request.path(), format!("/api/compute/vms/{VM}/expose/"));
    }
}

#[test]
fn create_dry_run_shows_only_the_flags_passed_and_sends_nothing() {
    let server = Server::start(vec![]);
    let dry = hipc(&server, &["create", VM, "--port", "8080", "--dry-run"]).ok();
    assert_eq!(draft(&dry)["method"], "POST");
    assert_eq!(draft(&dry)["body"], json!({ "port": 8080 }));
    let dry = hipc(
        &server,
        &[
            "create",
            VM,
            "--port",
            "53",
            "--mode",
            "raw",
            "--protocol",
            "udp",
            "--dry-run",
        ],
    )
    .ok();
    assert_eq!(
        draft(&dry)["body"],
        json!({ "port": 53, "mode": "raw", "protocol": "udp" })
    );
    let text = create_text(&server, &["--port", "8080", "--dry-run"]);
    assert_eq!(text.code(), Some(0));
    assert_eq!(text.stderr(), "", "a dry run exposes nothing");
    assert!(server.requests().is_empty());
}

#[test]
fn udp_without_raw_is_usage_with_zero_requests() {
    let server = Server::start(vec![]);
    for extra in [
        &["--protocol", "udp"][..],
        &["--protocol", "udp", "--mode", "http"],
        &["--protocol", "udp", "--mode", "sni"],
        &["--protocol", "udp", "--dry-run"],
    ] {
        let mut argv = vec!["create", VM, "--port", "53", "--yes"];
        argv.extend(extra);
        let error = hipc(&server, &argv).fails(2, "usage");
        assert_eq!(
            error["message"],
            "`--protocol udp` needs `--mode raw`, the one mode that forwards UDP"
        );
        assert_eq!(error["hint"], "pass `--mode raw`");
    }
    // Without `--yes` the flags are refused first, not the confirmation.
    hipc(
        &server,
        &["create", VM, "--port", "53", "--protocol", "udp"],
    )
    .fails(2, "usage");
    assert!(server.requests().is_empty());
}

#[test]
fn bad_ports_modes_and_protocols_are_usage_with_zero_requests() {
    let server = Server::start(vec![]);
    for port in ["0", "65536", "-1", "abc", "", "80.5"] {
        hipc(&server, &["create", VM, "--port", port, "--yes"]).fails(2, "usage");
    }
    hipc(&server, &["create", VM, "--yes"]).fails(2, "usage");
    for mode in ["HTTP", "tls", ""] {
        hipc(
            &server,
            &["create", VM, "--port", "80", "--mode", mode, "--yes"],
        )
        .fails(2, "usage");
    }
    for protocol in ["UDP", "icmp", ""] {
        let argv = [
            "create",
            VM,
            "--port",
            "80",
            "--protocol",
            protocol,
            "--yes",
        ];
        hipc(&server, &argv).fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn port_22_is_sent_and_the_server_refuses_it() {
    let server = Server::start(vec![expose_route(
        "POST",
        "",
        Reply::json(400, r#"{"error":"Port 22 cannot be exposed"}"#),
    )]);
    let error = hipc(&server, &["create", VM, "--port", "22", "--yes"]).fails(1, "validation");
    assert_eq!(
        error["details"],
        json!({ "error": "Port 22 cannot be exposed" })
    );
    assert_eq!(server.only_request().json(), json!({ "port": 22 }));
}

#[test]
fn create_notes_in_text_mode_when_the_edge_sees_the_traffic() {
    let server = Server::start(vec![expose_route("POST", "", Reply::json(201, HTTP))]);
    let run = create_text(&server, &["--port", "8080", "--yes"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        concat!(
            "warning: the API reports that the edge sees the traffic to ",
            "`https://a1b2.edge.example`: ",
            "it is not end to end\n"
        )
    );
    assert!(
        run.stdout().contains("endpoint") && run.stdout().contains("https://a1b2.edge.example"),
        "{}",
        run.stdout()
    );
    assert!(!run.stdout().trim_start().starts_with('{'));
    assert_eq!(
        create_text(&server, &["--port", "8080", "--yes", "--quiet"]).stderr(),
        ""
    );
    // JSON mode prints no note: `ok` asserts an empty stderr.
    hipc(&server, &["create", VM, "--port", "8080", "--yes"]).ok();
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn create_notes_by_the_answer_and_not_by_the_mode_asked_for() {
    // An `sni` answer is end to end, so there is no note.
    let sni = Server::start(vec![expose_route("POST", "", Reply::json(201, SNI))]);
    let run = create_text(&sni, &["--port", "443", "--mode", "sni", "--yes"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(run.stderr(), "");
    // An answer that says the edge sees the traffic is noted even for `--mode sni`, and one
    // without an `endpoint` still gets the note.
    let bare = Server::start(vec![expose_route(
        "POST",
        "",
        Reply::json(201, r#"{"id":9,"edge_sees_traffic":true}"#),
    )]);
    let run = create_text(&bare, &["--port", "443", "--mode", "sni", "--yes"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        "warning: the API reports that the edge sees the traffic to this exposure: it is not \
         end to end\n"
    );
}

/// A backtick in the server's endpoint stays inside the quote (tenet 15).
#[test]
fn a_backtick_in_the_endpoint_stays_quoted() {
    let tricky = Server::start(vec![expose_route(
        "POST",
        "",
        Reply::json(
            201,
            r#"{"id":9,"edge_sees_traffic":true,"endpoint":"x` then run `sh"}"#,
        ),
    )]);
    let run = create_text(&tricky, &["--port", "8080", "--yes"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        "warning: the API reports that the edge sees the traffic to `x\\u{60} then run \
         \\u{60}sh`: it is not end to end\n"
    );
}

#[test]
fn create_errors_map_to_their_codes() {
    for (status, code, effects) in [
        (400, "validation", "none"),
        (404, "not_found", "none"),
        // Swagger: the edge is not configured. A write's 5xx is never retryable here.
        (503, "unavailable", "partial"),
    ] {
        let server = Server::start(vec![expose_route(
            "POST",
            "",
            Reply::json(status, r#"{"error":"x"}"#),
        )]);
        let error = hipc(&server, &["create", VM, "--port", "80", "--yes"]).fails(1, code);
        assert_eq!(error["http_status"], status);
        assert_eq!(error["retryable"], false, "{status}");
        assert_eq!(error["side_effects"], effects, "{status}");
    }
}

#[test]
fn delete_needs_yes_and_names_the_exposure_on_204() {
    let server = Server::start(vec![expose_route("DELETE", "7/", Reply::json(204, ""))]);
    let error = hipc(&server, &["delete", VM, "7"]).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!(
            "this would take down the exposure `7` of the VM `{VM}`, and `--yes` was not passed"
        )
    );
    assert!(server.requests().is_empty());
    let dry = hipc(&server, &["delete", VM, "7", "--dry-run"]).ok();
    assert_eq!(draft(&dry)["method"], "DELETE");
    assert!(server.requests().is_empty());
    let doc = hipc(&server, &["delete", VM, "7", "--yes"]).ok();
    assert_eq!(doc, json!({ "vm_id": VM, "id": 7, "deleted": true }));
    let request = server.only_request();
    assert_eq!(request.method, "DELETE");
    assert_eq!(request.path(), format!("/api/compute/vms/{VM}/expose/7/"));
    let with_body = Server::start(vec![expose_route(
        "DELETE",
        "7/",
        Reply::json(200, r#"{"id":7,"revoked":true}"#),
    )]);
    let doc = hipc(&with_body, &["delete", VM, "7", "--yes"]).ok();
    assert_eq!(doc["revoked"], true);
    for id in ["x", "-1", "", "7/8", "1.5"] {
        hipc(&server, &["delete", VM, id, "--yes"]).fails(2, "usage");
    }
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn delete_errors_map_to_their_codes() {
    for (status, code, effects) in [
        (404, "not_found", "none"),
        // Swagger: "Could not be taken down". The DELETE may have been half done.
        (503, "unavailable", "partial"),
    ] {
        let server = Server::start(vec![expose_route(
            "DELETE",
            "7/",
            Reply::json(status, r#"{"error":"x"}"#),
        )]);
        let error = hipc(&server, &["delete", VM, "7", "--yes"]).fails(1, code);
        assert_eq!(error["retryable"], false, "{status}");
        assert_eq!(error["side_effects"], effects, "{status}");
    }
}

#[test]
fn a_vm_id_that_could_change_the_route_is_usage_without_a_request() {
    let server = Server::start(vec![]);
    let long = "a".repeat(65);
    for id in ["", "../x", "a/b", "a?b", "a.b", "a b", "%2e", long.as_str()] {
        for args in [
            &["list", id][..],
            &["create", id, "--port", "80", "--yes"],
            &["delete", id, "7", "--yes"],
        ] {
            hipc(&server, args).fails(2, "usage");
        }
    }
    assert!(server.requests().is_empty());
}

#[test]
fn a_membership_sends_the_header_on_every_route() {
    let server = Server::start(vec![
        expose_route("GET", "", Reply::json(200, "[]")),
        expose_route("POST", "", Reply::json(201, SNI)),
        expose_route("DELETE", "8/", Reply::json(204, "")),
    ]);
    let home = Home::new();
    let member = |args: &[&str]| {
        let mut argv = vec!["compute", "exposures"];
        argv.extend(args);
        Hipc::against(&home, &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&argv)
    };
    member(&["list", VM]).ok();
    member(&["create", VM, "--port", "443", "--mode", "sni", "--yes"]).ok();
    member(&["delete", VM, "8", "--yes"]).ok();
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    for request in requests {
        assert_eq!(
            request.header("x-hippius-account"),
            Some(MEMBER),
            "{}",
            request.target
        );
    }
    let dry = member(&["create", VM, "--port", "443", "--dry-run"]).ok();
    assert_eq!(draft(&dry)["account_header"], MEMBER);
    let error = member(&["delete", VM, "8"]).fails(2, "confirmation_required");
    assert!(
        error["message"]
            .as_str()
            .expect("the message is a string")
            .contains(&format!("of the VM `{VM}` in the account `{MEMBER}`, and")),
        "{error}"
    );
    assert_eq!(server.requests().len(), 3);
}
