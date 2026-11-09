//! `hipc account invites preview` and `accept`: an emailed invite, read from a token file. The
//! invite token is a bearer secret, so every run here also checks that it is printed nowhere.

use std::io::{Read as _, Write as _};
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::common::{Hipc, Home, MEMBER, Recorded, Reply, Route, Run, Server, TOKEN, route};

/// The invite token every test sends.
const INVITE: &str = "inv_Qx9Wb2-secret_token";

/// The invite preview, in the shape the console reads.
const PREVIEW: &str = concat!(
    r#"{"account_name":"Boss Co","invited_by":"boss@example.com","#,
    r#""email":"me@example.com","grants":[{"scope":"vms","level":"operator"}],"#,
    r#""can_manage_team":false,"can_billing":false,"#,
    r#""expires_at":"2026-11-03T00:00:00Z","valid":true,"reason":null}"#
);

const JOINED: &str =
    r#"{"account":{"id":"acct_0123456789abcdef0123","name":"Boss Co","is_owner":false}}"#;

fn preview_route(reply: Reply) -> Route {
    route("POST", "/api/accounts/invites/preview/", reply)
}

fn accept_route(reply: Reply) -> Route {
    route("POST", "/api/accounts/invites/accept/", reply)
}

/// The invite token, in a private file of `home`.
fn token_file(home: &Home) -> PathBuf {
    home.file("invite.txt", format!("{INVITE}\n").as_bytes())
}

/// Runs `hipc account invites <verb> --token-file <file> <extra>` with the test API token.
fn invites(server: &Server, verb: &str, extra: &[&str]) -> Run {
    let home = Home::new();
    let path = token_file(&home);
    let mut argv = vec![
        "account",
        "invites",
        verb,
        "--token-file",
        path.to_str().expect("a temporary path is UTF-8"),
    ];
    argv.extend(extra);
    no_invite_token(Hipc::against(&home, server).run(&argv))
}

/// `run`, after checking that the invite token is in neither of its streams.
fn no_invite_token(run: Run) -> Run {
    for (stream, text) in [("stdout", run.stdout()), ("stderr", run.stderr())] {
        assert!(
            !text.contains(INVITE),
            "the invite token leaked into {stream}: {text}"
        );
    }
    run
}

#[test]
fn preview_posts_the_token_in_the_body_and_prints_the_offer() {
    let server = Server::start(vec![preview_route(Reply::json(201, PREVIEW))]);
    let doc = invites(&server, "preview", &["--yes"]).ok();
    assert_eq!(doc, serde_json::from_str::<Value>(PREVIEW).unwrap());
    let request = server.only_request();
    assert_eq!(request.json(), json!({ "token": INVITE }));
    assert!(request.header("authorization").is_some());
    assert_eq!(request.header("x-hippius-account"), None);
}

/// The preview works signed out, so it needs no token, and is personal, so a membership
/// sends no account header.
#[test]
fn preview_needs_no_token_and_never_sends_the_account_header() {
    let server = Server::start(vec![preview_route(Reply::json(201, PREVIEW))]);
    let home = Home::new();
    let doc = Hipc::new(&home)
        .env("HIPPIUS_API_URL", server.url())
        .stdin(format!("  {INVITE}\n").as_bytes())
        .run(&[
            "account",
            "invites",
            "preview",
            "--token-file",
            "-",
            "--yes",
        ]);
    let doc = no_invite_token(doc).ok();
    assert_eq!(doc["account_name"], "Boss Co");
    let request = server.only_request();
    assert_eq!(request.header("authorization"), None);
    assert_eq!(request.json(), json!({ "token": INVITE }), "trimmed");

    let member = Server::start(vec![preview_route(Reply::json(201, PREVIEW))]);
    let path = token_file(&home);
    let run = Hipc::against(&home, &member)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&[
            "account",
            "invites",
            "preview",
            "--token-file",
            path.to_str().unwrap(),
            "--yes",
        ]);
    no_invite_token(run).ok();
    assert_eq!(member.only_request().header("x-hippius-account"), None);
}

/// The invite token follows the API token's origin rule (tenet 8): an origin other than
/// `https://api.hippius.com` gets it only with `--yes` or a yes at the prompt, and the
/// refusal names the origin and where it came from. Nothing is sent without it.
#[test]
fn preview_at_another_origin_needs_yes_and_names_its_source() {
    let server = Server::start(vec![preview_route(Reply::json(201, PREVIEW))]);
    let home = Home::new();
    let path = token_file(&home);
    let path = path.to_str().unwrap();
    let preview = ["account", "invites", "preview", "--token-file", path];
    let refusal = |run: Run, source: &str| {
        let error = no_invite_token(run).fails(2, "confirmation_required");
        assert_eq!(
            error["message"],
            format!(
                "this would send the invite token to `{}`, named by {source}, and `--yes` was \
                 not passed",
                server.url()
            )
        );
        assert_eq!(error["hint"], "pass `--yes` to confirm");
    };
    refusal(
        Hipc::new(&home)
            .env("HIPPIUS_API_URL", server.url())
            .run(&preview),
        "`HIPPIUS_API_URL`",
    );
    let text = [&["--output", "text"][..], &preview].concat();
    let run = Hipc::new(&home)
        .env("HIPPIUS_API_URL", server.url())
        .run(&text);
    assert_eq!(run.code(), Some(2), "{}", run.stderr());
    assert!(
        run.stderr().contains("send the invite token to"),
        "{}",
        run.stderr()
    );
    let flag = [&["--api-url", server.url()][..], &preview].concat();
    refusal(Hipc::without_api_url(&home).run(&flag), "`--api-url`");
    home.write_config(&format!(
        "[profiles.default]\napi_url = \"{}\"\n",
        server.url()
    ));
    let config = home.config();
    refusal(
        Hipc::without_api_url(&home).run(&preview),
        &format!(
            "the `api_url` of the profile `default` in `{}`",
            config.display()
        ),
    );
    assert!(server.requests().is_empty());

    let yes = [&preview[..], &["--yes"]].concat();
    let doc = no_invite_token(Hipc::without_api_url(&home).run(&yes)).ok();
    assert_eq!(doc["account_name"], "Boss Co");
    assert_eq!(server.only_request().json(), json!({ "token": INVITE }));
}

/// The default origin asks nothing, as before. Tests never reach `api.hippius.com`, so a
/// local proxy stands in for it: the preview goes out without a question as far as the
/// proxy's `CONNECT`, which the proxy refuses, so nothing leaves the machine. The same
/// holds for `accept`, whose preview is this one; its join question, which names the
/// account, is pinned at a terminal (`prompt.rs`).
#[test]
fn preview_at_the_default_origin_asks_nothing() {
    let proxy = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a local port");
    let proxy_url = format!(
        "http://127.0.0.1:{}",
        proxy.local_addr().expect("a bound address").port()
    );
    let connect = std::thread::spawn(move || {
        let (mut stream, _) = proxy.accept().expect("hipc connects to the proxy");
        let mut head = Vec::new();
        let mut byte = [0_u8; 1];
        while !head.ends_with(b"\r\n\r\n") && stream.read_exact(&mut byte).is_ok() {
            head.push(byte[0]);
        }
        drop(stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n"));
        String::from_utf8_lossy(&head).into_owned()
    });
    let home = Home::new();
    let path = token_file(&home);
    let run = Hipc::without_api_url(&home)
        // ureq reads `ALL_PROXY` before `HTTPS_PROXY` (ureq 3 `Proxy::try_from_env`), so this
        // one variable routes the request; the `CONNECT` assert below fails if it did not.
        .env("ALL_PROXY", &proxy_url)
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .run(&[
            "account",
            "invites",
            "preview",
            "--token-file",
            path.to_str().unwrap(),
        ]);
    // The proxy's refusal, not a confirmation: the question was never asked.
    no_invite_token(run).fails(1, "unavailable");
    let head = connect.join().expect("the proxy thread does not panic");
    assert!(head.starts_with("CONNECT api.hippius.com:443 "), "{head:?}");
}

/// A server that repeats the token, in an answer or an error, does not get it printed.
#[test]
fn a_token_the_server_repeats_is_redacted() {
    let echo = format!(r#"{{"account_name":"Boss Co","token":"{INVITE}"}}"#);
    let server = Server::start(vec![preview_route(Reply::json(201, &echo))]);
    let doc = invites(&server, "preview", &["--yes"]).ok();
    assert_eq!(
        doc,
        json!({"account_name": "Boss Co", "token": "<redacted>"})
    );

    let refusal = format!(r#"{{"error":"no invite {INVITE}"}}"#);
    // The request id header can echo it too.
    let server = Server::start(vec![preview_route(
        Reply::json(404, &refusal).header("X-Request-Id", INVITE),
    )]);
    let error = invites(&server, "preview", &["--yes"]).fails(1, "not_found");
    assert_eq!(error["request_id"], "<redacted>");
    assert_eq!(
        error["message"],
        "cannot preview the invite: the API says `no invite <redacted>`"
    );
    assert_eq!(error["details"], json!({"error": "no invite <redacted>"}));
    let text = invites(&server, "preview", &["--output", "text", "--yes"]);
    assert!(text.stderr().contains("<redacted>"), "{}", text.stderr());

    let joined = format!(r#"{{"account":{{"name":"Boss Co"}},"echo":"{INVITE}"}}"#);
    let server = Server::start(vec![
        preview_route(Reply::json(201, PREVIEW)),
        accept_route(Reply::json(201, &joined)),
    ]);
    let doc = invites(&server, "accept", &["--yes"]).ok();
    assert_eq!(doc["echo"], "<redacted>");
    let server = Server::start(vec![
        preview_route(Reply::json(201, PREVIEW)),
        accept_route(Reply::json(400, &refusal).header("X-Request-Id", INVITE)),
    ]);
    let error = invites(&server, "accept", &["--yes"]).fails(1, "validation");
    assert_eq!(error["request_id"], "<redacted>");
    assert_eq!(error["details"], json!({"error": "no invite <redacted>"}));
}

/// The preview is a POST that changes nothing (`hipc schema`: `read_only`), so a 503 or a
/// lost answer is a read's failure: nothing changed, and a retry is safe. That holds for the
/// preview `accept` sends first, before anything is accepted.
#[test]
fn a_failed_preview_is_a_read_that_changed_nothing() {
    let server = Server::start(vec![preview_route(Reply::json(
        503,
        r#"{"detail":"Busy"}"#,
    ))]);
    for verb in ["preview", "accept"] {
        let error = invites(&server, verb, &["--yes"]).fails(1, "unavailable");
        assert_eq!(error["side_effects"], "none", "{verb}");
        assert_eq!(error["retryable"], true, "{verb}");
        assert_eq!(
            error["hint"], "wait a few minutes, then run the command again",
            "{verb}"
        );
    }
    assert_eq!(server.requests().len(), 2, "no accept was sent");
}

#[test]
fn a_bad_token_file_is_usage_and_sends_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let cases: [(&str, &[u8], &str); 4] = [
        ("empty", b"", "is empty"),
        ("blank", b" \n\t", "holds only whitespace"),
        ("two words", b"inv_a inv_b\n", "holds a space, a line break"),
        ("not text", b"\xff\xfe", "is not UTF-8 text"),
    ];
    for verb in ["preview", "accept"] {
        for (name, contents, problem) in cases {
            let path = home.file(name, contents);
            let path = path.to_str().unwrap();
            let error = Hipc::against(&home, &server)
                .run(&["account", "invites", verb, "--token-file", path, "--yes"])
                .fails(2, "usage");
            let message = error["message"].as_str().unwrap();
            assert!(message.contains(problem), "{name}: {message}");
            assert!(!error.to_string().contains("inv_a"), "{name}: {error}"); // never quoted
            assert!(message.contains(&format!("`{path}`")), "{name}: {message}");
            assert!(
                error["hint"].as_str().unwrap().contains("`--token-file`"),
                "{error}"
            );
        }
        let error = Hipc::against(&home, &server)
            .stdin(b"\n")
            .run(&["account", "invites", verb, "--token-file", "-"])
            .fails(2, "usage");
        assert_eq!(
            error["message"],
            "the invite token from stdin holds only whitespace"
        );
        assert_eq!(
            error["hint"],
            "pipe only the invite token, the last part of the invite link, to `--token-file -`"
        );
        let missing = home.path().join("missing.txt");
        Hipc::against(&home, &server)
            .run(&[
                "account",
                "invites",
                verb,
                "--token-file",
                missing.to_str().unwrap(),
            ])
            .fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

/// A dry run shows where the token goes, never the token, and sends nothing.
#[test]
fn a_dry_run_shows_a_placeholder_for_the_token() {
    let server = Server::start(vec![]);
    let doc = invites(&server, "preview", &["--dry-run"]).dry_ok();
    let preview = json!({
        "method": "POST",
        "url": format!("{}/api/accounts/invites/preview/", server.url()),
        "account_header": null,
        "project_header": null,
        "billing_header": null,
        "body": {"token": "<the invite token from --token-file>"}
    });
    assert_eq!(doc, crate::common::one_request_dry_run(preview.clone()));
    let doc = invites(&server, "accept", &["--dry-run"]).dry_ok();
    let accept = json!({
        "method": "POST",
        "url": format!("{}/api/accounts/invites/accept/", server.url()),
        "account_header": null,
        "project_header": null,
        "billing_header": null,
        "body": {
            "token": "<the invite token from --token-file>",
            "confirm_without_email": false
        }
    });
    // The document is the accept, and `requests` adds the preview a real run sends first
    // to name the account.
    let mut expected = json!({ "dry_run": true });
    expected["requests"] = json!([preview, accept]);
    assert_eq!(doc, expected);
    let text = invites(&server, "accept", &["--dry-run", "--output", "text"]);
    assert_eq!(text.code(), Some(0));
    assert!(server.requests().is_empty());
}

/// At an origin other than the default, the invite token goes out only once the origin is
/// confirmed (tenet 8), so without `--yes` nothing is sent, not even the preview that names
/// the account. The question that names the account comes after the preview, at a terminal
/// (`prompt.rs`).
#[test]
fn accept_at_another_origin_without_yes_sends_nothing() {
    let server = Server::start(vec![]);
    let error = invites(&server, "accept", &[]).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!(
            "this would send the invite token to `{}`, named by `HIPPIUS_API_URL`, and `--yes` \
             was not passed",
            server.url()
        )
    );
    assert!(server.requests().is_empty());
}

#[test]
fn accept_with_yes_previews_then_joins_as_the_caller() {
    let server = Server::start(vec![
        preview_route(Reply::json(201, PREVIEW)),
        accept_route(Reply::json(201, JOINED)),
    ]);
    let home = Home::new();
    let path = token_file(&home);
    let run = Hipc::against(&home, &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&[
            "--debug",
            "account",
            "invites",
            "accept",
            "--token-file",
            path.to_str().unwrap(),
            "--yes",
        ]);
    // Checked before the asserts: an assert message runs only when the assert fails.
    let run = no_invite_token(run);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(run.stderr().contains("debug: "), "{}", run.stderr());
    assert_eq!(run.json(), serde_json::from_str::<Value>(JOINED).unwrap());
    let requests = server.requests();
    let paths: Vec<&str> = requests.iter().map(Recorded::path).collect();
    assert_eq!(
        paths,
        [
            "/api/accounts/invites/preview/",
            "/api/accounts/invites/accept/"
        ]
    );
    assert_eq!(
        requests[1].json(),
        json!({"token": INVITE, "confirm_without_email": false})
    );
    for request in &requests {
        assert_eq!(request.header("x-hippius-account"), None);
        assert!(request.header("authorization").is_some());
    }
}

#[test]
fn accept_needs_a_token_before_anything_is_sent() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let path = token_file(&home);
    let run = Hipc::new(&home).env("HIPPIUS_API_URL", server.url()).run(&[
        "account",
        "invites",
        "accept",
        "--token-file",
        path.to_str().unwrap(),
        "--yes",
    ]);
    no_invite_token(run).fails(1, "auth");
    assert!(server.requests().is_empty());
}

/// A token stored for the default origin is withheld from another one, so the accept stops
/// at the token check, before the preview would send the invite token there.
#[test]
fn accept_with_a_withheld_token_sends_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    home.write_config(&format!(
        "[profiles.default]\ntoken = \"{TOKEN}\"\n\
         token_origin = \"https://api.hippius.com\"\n"
    ));
    let path = token_file(&home);
    let run = Hipc::new(&home).env("HIPPIUS_API_URL", server.url()).run(&[
        "account",
        "invites",
        "accept",
        "--token-file",
        path.to_str().unwrap(),
        "--yes",
    ]);
    let error = no_invite_token(run).fails(1, "auth");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("https://api.hippius.com"),
        "{error}"
    );
    assert!(server.requests().is_empty());
}

/// A preview that fails, or does not name the account, ends the command before the
/// question, and no accept is sent.
#[test]
fn a_preview_that_cannot_name_the_account_sends_no_accept() {
    let server = Server::start(vec![preview_route(Reply::json(
        404,
        r#"{"error":"Invitation not found."}"#,
    ))]);
    let error = invites(&server, "accept", &["--yes"]).fails(1, "not_found");
    assert_eq!(
        error["message"],
        "cannot preview the invite: the API says `Invitation not found.`"
    );
    assert_eq!(server.requests().len(), 1);

    let server = Server::start(vec![preview_route(Reply::json(201, r#"{"valid":true}"#))]);
    let error = invites(&server, "accept", &["--yes"]).fails(1, "internal");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("has no `account_name`"),
        "{error}"
    );
    assert_eq!(server.requests().len(), 1);
}

/// An invite the preview marks as not valid is refused before the question, naming the
/// server's `reason`, and no accept is sent.
#[test]
fn an_invite_the_preview_marks_not_valid_is_refused_before_the_question() {
    for (preview, message) in [
        (
            r#"{"account_name":"Boss Co","valid":false,"reason":"expired"}"#,
            "cannot accept the invite: the API says it is `expired`",
        ),
        (
            r#"{"account_name":"Boss Co","valid":false,"reason":null}"#,
            "cannot accept the invite: the API says it is not valid",
        ),
    ] {
        let server = Server::start(vec![preview_route(Reply::json(201, preview))]);
        let error = invites(&server, "accept", &["--yes"]).fails(1, "conflict");
        assert_eq!(error["message"], message);
        assert_eq!(error["side_effects"], "none");
        assert_eq!(server.requests().len(), 1, "only the preview");
    }
}

/// The two refusals the console explains get a hint; the route is personal, so a
/// membership does not turn a 403 into a hint to pass `--account self`.
#[test]
fn accept_refusals_get_the_fix_the_console_offers() {
    let cases = [
        (
            403,
            r#"{"error":"mismatch","code":"email_mismatch"}"#,
            "permission",
            Some("with a token of the user the invite was emailed to"),
        ),
        (
            409,
            r#"{"error":"confirm","code":"email_confirmation_required"}"#,
            "conflict",
            Some("accept the invite in the console"),
        ),
        (403, r#"{"error":"no","code":"other"}"#, "permission", None),
        (
            409,
            r#"{"error":"x","code":"email_mismatch"}"#,
            "conflict",
            None,
        ),
    ];
    for (status, body, code, hint) in cases {
        let server = Server::start(vec![
            preview_route(Reply::json(201, PREVIEW)),
            accept_route(Reply::json(status, body)),
        ]);
        let home = Home::new();
        let path = token_file(&home);
        let run = Hipc::against(&home, &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&[
                "account",
                "invites",
                "accept",
                "--token-file",
                path.to_str().unwrap(),
                "--yes",
            ]);
        let error = no_invite_token(run).fails(1, code);
        match hint {
            Some(hint) => assert!(
                error["hint"].as_str().unwrap().contains(hint),
                "{body}: {error}"
            ),
            None => assert_eq!(error["hint"], Value::Null, "{body}"),
        }
        assert_eq!(error["side_effects"], "none");
    }
}
