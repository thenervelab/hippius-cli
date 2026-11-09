//! `hipc auth login --access-key-file`: an access key phrase, its challenge, both signatures,
//! and the token the API answers.

use serde_json::{Value, json};

use crate::common::{ACCOUNTS, Hipc, Home, MEMBER, PHRASES, Reply, Route, Server, TOKEN, route};

const MNEMONIC: &str = "/api/auth/mnemonic/";
const VERIFY: &str = "/api/auth/verify/";
const ACCOUNTS_PATH: &str = "/api/accounts/";

/// The addresses of `PHRASES[0]`, Substrate's development phrase.
const ADDRESS: &str = "0xf24FF3a9CF04c71Dbc94D0b566f7A27B94566cac";
const SUBSTRATE: &str = "5DfhGyQdFobKM8NsWvEeAKk5EQQgYe9AydgJ7rMB6E1EqRzV";
/// The token a verify answers. Not [`TOKEN`], so a test can tell the two apart.
const NEW_TOKEN: &str = "tok_issued_by_the_login_0123456789abcdef";
const MESSAGE: &str = "Sign this message to log in to Hippius: nonce 8f2c";

fn challenge() -> Route {
    let body = json!({"challenge": "8f2c", "message": MESSAGE, "expires_in": 300});
    route("POST", MNEMONIC, Reply::json(200, &body.to_string()))
}

/// The read that checks the answered token before it is stored.
fn accounts() -> Route {
    route("GET", ACCOUNTS_PATH, Reply::json(200, ACCOUNTS))
}

fn verified(is_new: bool) -> Route {
    let body = json!({
        "token": NEW_TOKEN,
        "user_id": 41,
        "username": "u_5dfhgy",
        "substrate_address": SUBSTRATE,
        "is_new": is_new,
    });
    route("POST", VERIFY, Reply::json(200, &body.to_string()))
}

/// `hipc` against `server` with no token in the environment, the way a first login runs.
fn hipc(home: &Home, server: &Server) -> Hipc {
    Hipc::new(home).env("HIPPIUS_API_URL", server.url())
}

fn phrase_file(home: &Home) -> String {
    let path = home.file("phrase.txt", format!("{}\n", PHRASES[0]).as_bytes());
    path.to_str()
        .expect("the temporary path is UTF-8")
        .to_owned()
}

#[test]
fn a_new_account_is_stored_with_new_account() {
    let home = Home::new();
    let server = Server::start(vec![challenge(), verified(true), accounts()]);
    let file = phrase_file(&home);
    let doc = hipc(&home, &server)
        .run(&[
            "auth",
            "login",
            "--access-key-file",
            &file,
            "--new-account",
            "--yes",
        ])
        .ok();
    assert_eq!(doc["is_new"], true);
    assert_eq!(doc["token_stored"], true);
    assert_eq!(doc["profile"], "default");
    assert_eq!(doc["address"], ADDRESS);
    assert_eq!(doc["substrate_address"], SUBSTRATE);
    assert_eq!(doc["user_id"], 41);
    assert_eq!(doc["username"], "u_5dfhgy");
    assert_eq!(doc["token_origin"], server.url());
    assert!(
        doc.get("token").is_none(),
        "the token is stored, not printed"
    );
    let config = home.config_text();
    assert!(
        config.contains(&format!("token = \"{NEW_TOKEN}\"")),
        "{config}"
    );
    assert!(
        !config.contains("bottom"),
        "the phrase is not stored: {config}"
    );

    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].path(), MNEMONIC);
    // The token is checked with the read `hipc auth status` makes, and only then stored.
    assert_eq!(requests[2].path(), ACCOUNTS_PATH);
    assert_eq!(
        requests[2].header("authorization"),
        Some(format!("Token {NEW_TOKEN}").as_str())
    );
    assert_eq!(
        requests[0].json(),
        json!({"address": ADDRESS, "substrate_address": SUBSTRATE})
    );
    let verify = requests[1].json();
    assert_eq!(requests[1].path(), VERIFY);
    let key = hippius::AccessKey::from_phrase(PHRASES[0], &hippius::Source::Default).unwrap();
    // The Ethereum signature is deterministic (RFC 6979), so it is the library's exactly; the
    // sr25519 one is randomized, and its verification is the library's unit test.
    assert_eq!(
        verify["signature"],
        key.sign_login(MESSAGE, "8f2c").unwrap().signature
    );
    let substrate = verify["substrate_signature"].as_str().unwrap();
    assert_eq!(substrate.len(), 130);
    assert!(substrate.starts_with("0x"));
    assert!(substrate[2..].bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(verify["address"], ADDRESS);
    assert_eq!(verify["substrate_address"], SUBSTRATE);
    assert_eq!(verify.as_object().unwrap().len(), 4);
    for request in &requests[..2] {
        assert_eq!(request.header("authorization"), None);
        assert_eq!(request.header("x-hippius-account"), None);
    }
    assert_eq!(requests[2].header("x-hippius-account"), None);
}

#[test]
fn a_new_account_is_explained_in_text_mode() {
    let home = Home::new();
    let server = Server::start(vec![challenge(), verified(true), accounts()]);
    let file = phrase_file(&home);
    let run = hipc(&home, &server).run(&[
        "--output",
        "text",
        "auth",
        "login",
        "--access-key-file",
        &file,
        "--new-account",
        "--yes",
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        format!(
            "note: a new account was opened for `{SUBSTRATE}`: it has no email and no free Drive \
             plan, and S3 needs no subscription\n"
        )
    );
    assert!(run.stdout().contains("is_new"), "{}", run.stdout());
}

/// A phrase the API has not seen opens a new, empty account, and so does one with a mistyped
/// word: without `--new-account` the login is refused and stores nothing, and says that the
/// server has opened the account.
#[test]
fn a_new_account_is_refused_without_new_account() {
    let home = Home::new();
    let server = Server::start(vec![challenge(), verified(true), accounts()]);
    let file = phrase_file(&home);
    let error = hipc(&home, &server)
        .run(&["auth", "login", "--access-key-file", &file, "--yes"])
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        format!(
            "the access key's words are new to the API, so it opened a new, empty account for \
             `{SUBSTRATE}`; no token was stored"
        )
    );
    assert_eq!(
        error["hint"],
        "check the 12 words and log in again; pass `--new-account` only to keep a new account \
         on purpose"
    );
    assert_eq!(error["side_effects"], "partial");
    assert_eq!(error["retryable"], false);
    assert!(!home.config().exists(), "no token is stored");
    // The token is not even checked: the challenge and the verify, and nothing after them.
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].path(), VERIFY);
}

/// The refused login's token is in no output, `--debug` lines included.
#[test]
fn a_refused_new_account_prints_no_token() {
    let home = Home::new();
    let server = Server::start(vec![challenge(), verified(true)]);
    let file = phrase_file(&home);
    let run = hipc(&home, &server).run(&[
        "--debug",
        "auth",
        "login",
        "--access-key-file",
        &file,
        "--yes",
    ]);
    assert_eq!(run.code(), Some(2), "{}", run.stderr());
    let all = format!("{}{}", run.stdout(), run.stderr());
    assert!(all.contains("debug: POST"), "{all}");
    assert!(!all.contains(NEW_TOKEN), "{all}");
}

/// The refusal leaves a stored token in place, and says so in text mode too.
#[test]
fn a_refused_new_account_keeps_the_stored_token() {
    let home = Home::new();
    home.write_config(&format!("[profiles.default]\ntoken = \"{TOKEN}\"\n"));
    let server = Server::start(vec![challenge(), verified(true), accounts()]);
    let file = phrase_file(&home);
    let before = home.config_text();
    let run = hipc(&home, &server).run(&[
        "--output",
        "text",
        "auth",
        "login",
        "--access-key-file",
        &file,
        "--yes",
    ]);
    assert_eq!(run.code(), Some(2), "{}", run.stderr());
    assert_eq!(run.stdout(), "");
    assert!(
        run.stderr().starts_with(&format!(
            "error: the access key's words are new to the API, so it opened a new, empty \
             account for `{SUBSTRATE}`; no token was stored\nhint: check the 12 words and log \
             in again"
        )),
        "{}",
        run.stderr()
    );
    assert_eq!(home.config_text(), before, "the config is left as it was");
}

/// The refusal guards one run: the server opened the account, so the next login with the same
/// words is not new, and logs in to it. The help and the docs say so.
#[test]
fn a_login_after_a_refusal_finds_the_opened_account() {
    let home = Home::new();
    let server = Server::start(vec![challenge(), verified(true)]);
    let file = phrase_file(&home);
    hipc(&home, &server)
        .run(&["auth", "login", "--access-key-file", &file, "--yes"])
        .fails(2, "usage");
    let server = Server::start(vec![challenge(), verified(false), accounts()]);
    let doc = hipc(&home, &server)
        .run(&["auth", "login", "--access-key-file", &file, "--yes"])
        .ok();
    assert_eq!(doc["is_new"], false);
    assert_eq!(doc["token_stored"], true);
}

/// A dry run cannot know whether the account is new, so `--new-account` changes nothing in it.
#[test]
fn a_dry_run_is_the_same_with_new_account() {
    let home = Home::new();
    let server = Server::start(vec![]);
    let file = phrase_file(&home);
    let plain = hipc(&home, &server)
        .run(&["auth", "login", "--access-key-file", &file, "--dry-run"])
        .ok();
    let with = hipc(&home, &server)
        .run(&[
            "auth",
            "login",
            "--access-key-file",
            &file,
            "--new-account",
            "--dry-run",
        ])
        .ok();
    assert_eq!(plain, with);
    assert!(server.requests().is_empty());
}

/// `--new-account` goes only with an access key, and does not change a known account's login.
#[test]
fn new_account_needs_an_access_key() {
    let home = Home::new();
    let server = Server::start(vec![]);
    let token = home.file("token.txt", TOKEN.as_bytes());
    let token = token.to_str().unwrap();
    for args in [
        ["auth", "login", "--new-account", "--yes"].as_slice(),
        &["auth", "login", "--token-file", token, "--new-account"],
    ] {
        hipc(&home, &server).run(args).fails(2, "usage");
    }
    assert!(server.requests().is_empty());
    assert!(!home.config().exists());

    let server = Server::start(vec![challenge(), verified(false), accounts()]);
    let file = phrase_file(&home);
    let doc = hipc(&home, &server)
        .run(&[
            "auth",
            "login",
            "--access-key-file",
            &file,
            "--new-account",
            "--yes",
        ])
        .ok();
    assert_eq!(doc["is_new"], false);
    assert_eq!(doc["token_stored"], true);
}

#[test]
fn a_known_phrase_logs_in_without_a_note() {
    let home = Home::new();
    let server = Server::start(vec![challenge(), verified(false), accounts()]);
    let file = phrase_file(&home);
    let run = hipc(&home, &server).run(&[
        "--output",
        "text",
        "auth",
        "login",
        "--access-key-file",
        &file,
        "--yes",
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(run.stderr(), "");
    let doc = hipc(&home, &server)
        .run(&["auth", "login", "--access-key-file", &file, "--yes"])
        .ok();
    assert_eq!(doc["is_new"], false);
}

#[test]
fn the_phrase_comes_from_stdin_and_any_whitespace() {
    let home = Home::new();
    let server = Server::start(vec![challenge(), verified(false), accounts()]);
    let spaced = format!("\n  {}  \r\n", PHRASES[0].replace(' ', " \t "));
    let doc = hipc(&home, &server)
        .stdin(spaced.as_bytes())
        .run(&["auth", "login", "--access-key-file", "-", "--yes"])
        .ok();
    assert_eq!(doc["substrate_address"], SUBSTRATE);
}

#[test]
fn a_stored_token_and_a_membership_are_not_sent_with_a_login() {
    let home = Home::new();
    let server = Server::start(vec![challenge(), verified(false), accounts()]);
    let file = phrase_file(&home);
    hipc(&home, &server)
        .env("HIPPIUS_TOKEN", TOKEN)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["auth", "login", "--access-key-file", &file, "--yes"])
        .ok();
    let requests = server.requests();
    for request in &requests[..2] {
        assert_eq!(request.header("authorization"), None);
        assert_eq!(request.header("x-hippius-account"), None);
    }
    assert_eq!(
        requests[2].header("authorization"),
        Some(format!("Token {NEW_TOKEN}").as_str()),
        "the check sends the new token, not the stored one"
    );
    assert_eq!(requests[2].header("x-hippius-account"), None);
}

#[test]
fn a_refused_signature_stores_nothing() {
    for (status, code) in [(400, "validation"), (401, "auth")] {
        let home = Home::new();
        let refused = route(
            "POST",
            VERIFY,
            Reply::json(status, r#"{"error":"Invalid signature"}"#),
        );
        let server = Server::start(vec![challenge(), refused]);
        let file = phrase_file(&home);
        let error = hipc(&home, &server)
            .run(&["auth", "login", "--access-key-file", &file, "--yes"])
            .fails(1, code);
        assert_eq!(error["http_status"], status);
        assert!(!home.config().exists(), "no token is stored");
    }
}

#[test]
fn a_rate_limited_login_says_to_wait() {
    for (path, other) in [(MNEMONIC, None), (VERIFY, Some(challenge()))] {
        let home = Home::new();
        let limited = route(
            "POST",
            path,
            Reply::json(429, r#"{"detail":"Too many requests."}"#).header("Retry-After", "60"),
        );
        let server = Server::start(other.into_iter().chain([limited]).collect());
        let file = phrase_file(&home);
        let error = hipc(&home, &server)
            .run(&["auth", "login", "--access-key-file", &file, "--yes"])
            .fails(1, "rate_limited");
        assert_eq!(
            error["hint"],
            "wait for the `Retry-After` delay before logging in again: the API limits logins \
             and new accounts from one client address"
        );
        assert_eq!(error["retry_after_ms"], 60_000);
    }
}

#[test]
fn an_unreadable_answer_stores_nothing() {
    let other = json!({
        "token": NEW_TOKEN,
        "is_new": false,
        "substrate_address": "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
    });
    let no_address = json!({"token": NEW_TOKEN, "is_new": false});
    let wrong_eth = json!({
        "token": NEW_TOKEN,
        "is_new": false,
        "substrate_address": SUBSTRATE,
        "address": "0x0000000000000000000000000000000000000001",
    });
    let numeric_eth = json!({
        "token": NEW_TOKEN,
        "is_new": false,
        "substrate_address": SUBSTRATE,
        "address": 42,
    });
    let cases: [(Route, Route, &str); 9] = [
        (
            challenge(),
            route("POST", VERIFY, Reply::json(200, &numeric_eth.to_string())),
            "the login answered for `42`, not for the access key's \
             `5DfhGyQdFobKM8NsWvEeAKk5EQQgYe9AydgJ7rMB6E1EqRzV`, so its token was not stored",
        ),
        (
            challenge(),
            route("POST", VERIFY, Reply::json(200, &no_address.to_string())),
            "cannot log in with the access key: the API answered with a shape `hipc` does not \
             read",
        ),
        (
            challenge(),
            route("POST", VERIFY, Reply::json(200, &wrong_eth.to_string())),
            "the login answered for `0x0000000000000000000000000000000000000001`, not for the \
             access key's `5DfhGyQdFobKM8NsWvEeAKk5EQQgYe9AydgJ7rMB6E1EqRzV`, so its token was \
             not stored",
        ),
        (
            route(
                "POST",
                MNEMONIC,
                Reply::json(200, &json!({"message": MESSAGE}).to_string()),
            ),
            verified(true),
            "cannot ask for a login challenge: the API answered with a shape `hipc` does not \
             read",
        ),
        (
            route("POST", MNEMONIC, Reply::json(200, r#"{"challenge":"c"}"#)),
            verified(true),
            "cannot ask for a login challenge: the API answered with a shape `hipc` does not \
             read",
        ),
        (
            challenge(),
            route("POST", VERIFY, Reply::json(200, r#"{"is_new":true}"#)),
            "cannot log in with the access key: the API answered with a shape `hipc` does not \
             read",
        ),
        (
            challenge(),
            route(
                "POST",
                VERIFY,
                Reply::json(200, r#"{"token":"two words","is_new":true}"#),
            ),
            "cannot log in with the access key: the API answered with a shape `hipc` does not \
             read",
        ),
        (
            challenge(),
            route(
                "POST",
                VERIFY,
                Reply::json(200, &format!(r#"{{"token":"{NEW_TOKEN}"}}"#)),
            ),
            "cannot log in with the access key: the API answered with a shape `hipc` does not \
             read",
        ),
        (
            challenge(),
            route("POST", VERIFY, Reply::json(200, &other.to_string())),
            "the login answered for `5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY`, not \
             for the access key's `5DfhGyQdFobKM8NsWvEeAKk5EQQgYe9AydgJ7rMB6E1EqRzV`, so its \
             token was not stored",
        ),
    ];
    for (first, second, message) in cases {
        let home = Home::new();
        let server = Server::start(vec![first, second]);
        let file = phrase_file(&home);
        let error = hipc(&home, &server)
            .run(&["auth", "login", "--access-key-file", &file, "--yes"])
            .fails(1, "internal");
        assert_eq!(error["message"], message);
        assert!(!home.config().exists(), "no token is stored");
    }
}

#[test]
fn both_files_or_neither_is_usage_and_sends_nothing() {
    let home = Home::new();
    let server = Server::start(vec![]);
    let file = phrase_file(&home);
    let token = home.file("token", TOKEN.as_bytes());
    let token = token.to_str().unwrap();
    for args in [
        vec![
            "auth",
            "login",
            "--access-key-file",
            &file,
            "--token-file",
            token,
        ],
        vec![
            "auth",
            "login",
            "--token-file",
            token,
            "--access-key-file",
            &file,
        ],
        vec!["auth", "login"],
    ] {
        let error = hipc(&home, &server).run(&args).fails(2, "usage");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("--access-key-file")
                || error["message"].as_str().unwrap().contains("--token-file"),
            "{error}"
        );
    }
    assert!(server.requests().is_empty());
    assert!(!home.config().exists());
}

#[test]
fn another_origin_needs_yes_before_anything_is_sent() {
    let home = Home::new();
    let server = Server::start(vec![]);
    let file = phrase_file(&home);
    let error = hipc(&home, &server)
        .run(&["auth", "login", "--access-key-file", &file])
        .fails(2, "confirmation_required");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with("this would log in with the access key at `http://127.0.0.1:"),
        "{error}"
    );
    assert!(server.requests().is_empty());
}

#[test]
fn a_dry_run_shows_both_requests_and_signs_and_stores_nothing() {
    let home = Home::new();
    let server = Server::start(vec![]);
    let file = phrase_file(&home);
    let doc = hipc(&home, &server)
        .run(&["--dry-run", "auth", "login", "--access-key-file", &file])
        .ok();
    assert_eq!(doc["dry_run"], true);
    assert!(doc.get("action").is_none(), "it sends requests: {doc}");
    assert_eq!(doc["token_stored"], true);
    assert_eq!(doc["token_origin"], server.url());
    let requests = doc["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2]["method"], "GET");
    assert_eq!(
        requests[2]["url"],
        format!("{}{ACCOUNTS_PATH}", server.url())
    );
    assert_eq!(requests[0]["method"], "POST");
    assert_eq!(requests[0]["url"], format!("{}{MNEMONIC}", server.url()));
    assert_eq!(
        requests[0]["body"],
        json!({"address": ADDRESS, "substrate_address": SUBSTRATE})
    );
    assert_eq!(requests[1]["url"], format!("{}{VERIFY}", server.url()));
    assert_eq!(
        requests[1]["body"],
        json!({
            "signature": "<the Ethereum signature of the challenge message>",
            "substrate_signature": "<the sr25519 signature of the challenge message>",
            "address": ADDRESS,
            "substrate_address": SUBSTRATE,
        })
    );
    for request in requests {
        assert_eq!(request["account_header"], Value::Null);
    }
    assert!(server.requests().is_empty());
    assert!(!home.config().exists());
}

#[test]
fn a_bad_phrase_is_usage_and_never_quoted() {
    let home = Home::new();
    let server = Server::start(vec![]);
    let sentinel = PHRASES[1].replace("junk", "zzqsentinel");
    let checksum = PHRASES[1].replace("junk", "test");
    let cases = [
        (
            sentinel.as_str(),
            "word 12 of the access key phrase from `{file}` is not on the English BIP-39 word \
             list",
        ),
        (
            checksum.as_str(),
            "the access key phrase from `{file}` fails its BIP-39 checksum: a word is wrong or \
             out of place",
        ),
        ("", "the access key phrase from `{file}` is empty"),
    ];
    for (phrase, message) in cases {
        let path = home.file("bad.txt", phrase.as_bytes());
        let file = path.to_str().unwrap();
        for mode in ["json", "text"] {
            let run = hipc(&home, &server).run(&[
                "--output",
                mode,
                "--debug",
                "auth",
                "login",
                "--access-key-file",
                file,
                "--yes",
            ]);
            assert_eq!(run.code(), Some(2));
            let stderr = run.stderr();
            assert!(
                stderr.contains(&message.replace("{file}", file)),
                "{stderr}"
            );
            assert!(!stderr.contains("zzqsentinel"), "{stderr}");
            assert_eq!(run.stdout(), "");
        }
    }
    let missing = home.path().join("missing");
    hipc(&home, &server)
        .run(&[
            "auth",
            "login",
            "--access-key-file",
            missing.to_str().unwrap(),
            "--yes",
        ])
        .fails(2, "usage");
    let huge = home.file("huge.txt", &vec![b'a'; 64 * 1024 + 1]);
    hipc(&home, &server)
        .run(&[
            "auth",
            "login",
            "--access-key-file",
            huge.to_str().unwrap(),
            "--yes",
        ])
        .fails(2, "usage");
    let binary = home.file("binary.txt", &[0xff, 0xfe]);
    hipc(&home, &server)
        .run(&[
            "auth",
            "login",
            "--access-key-file",
            binary.to_str().unwrap(),
            "--yes",
        ])
        .fails(2, "usage");
    assert!(server.requests().is_empty());
}

/// The phrase is in no output and no `--debug` line, in either mode, on success and on every
/// failure; the shared check in `Run` holds every run of the suite to the same.
#[test]
fn the_phrase_never_reaches_any_output() {
    let home = Home::new();
    let server = Server::start(vec![challenge(), verified(true), accounts()]);
    let file = phrase_file(&home);
    for mode in ["json", "text"] {
        let run = hipc(&home, &server).run(&[
            "--output",
            mode,
            "--debug",
            "auth",
            "login",
            "--access-key-file",
            &file,
            "--new-account",
            "--yes",
        ]);
        assert_eq!(run.code(), Some(0), "{}", run.stderr());
        let all = format!("{}{}", run.stdout(), run.stderr());
        assert!(all.contains("debug: POST"), "{all}");
        for word in ["bottom", "curtain", "basket", "lonely"] {
            assert!(!all.contains(word), "{word} in {all}");
        }
    }
}

/// A message that does not read as a login challenge is signed by neither key: the verify is
/// never sent, so no signature leaves the machine.
#[test]
fn a_message_that_is_not_a_login_challenge_is_never_signed() {
    let long = format!(
        "Sign in to Hippius with challenge 8f2c {}",
        "x".repeat(1024)
    );
    let cases = [
        (
            "Sign in to Hippius, nonce 8f2c",
            "8f2c",
            "message is 32 bytes or shorter",
        ),
        (long.as_str(), "8f2c", "message is longer than 1024 bytes"),
        (
            "example.com wants you to sign in with your Ethereum account:\nNonce: 8f2c",
            "8f2c",
            "message holds a control character",
        ),
        (MESSAGE, "another", "message does not hold its `challenge`"),
        (MESSAGE, "", "has no `challenge`"),
    ];
    for (message, nonce, why) in cases {
        let home = Home::new();
        let body = json!({"challenge": nonce, "message": message, "expires_in": 300});
        let server = Server::start(vec![route(
            "POST",
            MNEMONIC,
            Reply::json(200, &body.to_string()),
        )]);
        let file = phrase_file(&home);
        let error = hipc(&home, &server)
            .run(&["auth", "login", "--access-key-file", &file, "--yes"])
            .fails(1, "internal");
        assert_eq!(
            error["message"],
            format!(
                "the login challenge {why}, so `hipc` signed nothing: a message that is not a \
                 login challenge could be a payment or a sign-in elsewhere"
            )
        );
        assert_eq!(server.requests().len(), 1, "no verify is sent");
        assert!(!home.config().exists());
    }
}

#[test]
fn a_token_the_api_refuses_is_not_stored() {
    let home = Home::new();
    let refused = route(
        "GET",
        ACCOUNTS_PATH,
        Reply::json(401, r#"{"detail":"Invalid token."}"#),
    );
    let server = Server::start(vec![challenge(), verified(true), refused]);
    let file = phrase_file(&home);
    let error = hipc(&home, &server)
        .run(&[
            "auth",
            "login",
            "--access-key-file",
            &file,
            "--new-account",
            "--yes",
        ])
        .fails(1, "auth");
    assert_eq!(
        error["hint"],
        "run the login again; the token it answered was not stored"
    );
    assert!(!home.config().exists());
}

/// The phrase file is held to the config file's checks; stdin has none to hold it to.
#[test]
fn the_phrase_file_must_be_private_and_not_a_symlink() {
    use std::os::unix::fs::PermissionsExt as _;
    let home = Home::new();
    let server = Server::start(vec![]);
    let file = phrase_file(&home);
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let error = hipc(&home, &server)
        .run(&["auth", "login", "--access-key-file", &file, "--yes"])
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        format!(
            "the access key file `{file}` has permission bits for the group or others, and it \
             holds an access key phrase"
        )
    );
    assert_eq!(error["hint"], format!("run `chmod 600 {file}`"));
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let link = home.path().join("link.txt");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    let link = link.to_str().unwrap();
    let error = hipc(&home, &server)
        .run(&["auth", "login", "--access-key-file", link, "--yes"])
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        format!("the access key file `{link}` is a symlink, and a symlink is never followed")
    );
    let missing = home.path().join("missing.txt");
    let missing = missing.to_str().unwrap();
    let error = hipc(&home, &server)
        .run(&["auth", "login", "--access-key-file", missing, "--yes"])
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        format!("the access key file `{missing}` does not exist")
    );
    assert!(server.requests().is_empty());
}

#[test]
fn a_phrase_on_stdin_is_held_to_the_size_and_text_rules() {
    let home = Home::new();
    let server = Server::start(vec![]);
    for (stdin, message) in [
        (
            vec![b'a'; 64 * 1024 + 1],
            "stdin is larger than 65536 bytes, the most hipc reads for the access key phrase",
        ),
        (
            vec![0xff, 0xfe],
            "the access key phrase from stdin is not UTF-8 text",
        ),
    ] {
        let error = hipc(&home, &server)
            .stdin(&stdin)
            .run(&["auth", "login", "--access-key-file", "-", "--yes"])
            .fails(2, "usage");
        assert_eq!(error["message"], message);
    }
    assert!(server.requests().is_empty());
}

/// `--access-key` asks only a person at a terminal: a run without one is refused before
/// anything is read or sent, and the hint names `--access-key-file`. It takes no value, and
/// goes with neither file flag.
#[test]
fn the_access_key_prompt_needs_a_terminal() {
    let home = Home::new();
    let server = Server::start(vec![]);
    let file = phrase_file(&home);
    for extra in [&[][..], &["--dry-run"]] {
        let mut args = vec!["auth", "login", "--access-key", "--yes"];
        args.extend_from_slice(extra);
        let error = hipc(&home, &server).run(&args).fails(2, "usage");
        assert_eq!(
            error["message"],
            "`--access-key` asks a person at a terminal for the 12 words, and nobody at a \
             terminal can be asked"
        );
        assert_eq!(
            error["hint"],
            "pass `--access-key-file <path>`, or `--access-key-file -` to read the 12 words \
             from stdin"
        );
    }
    for args in [
        vec!["auth", "login", "--access-key", "--access-key-file", &file],
        vec!["auth", "login", "--access-key", "--token-file", &file],
    ] {
        hipc(&home, &server).run(&args).fails(2, "usage");
    }
    assert!(server.requests().is_empty());
    assert!(!home.config().exists());
}

/// A profile that stores a token asks before a login replaces it, since every later command
/// then acts as the access key's account: without `--yes` nothing is sent; with it the token
/// is replaced; the dry run says `replaces_token`.
#[test]
fn a_stored_token_is_replaced_only_after_the_question() {
    let home = Home::new();
    let file = phrase_file(&home);
    let server = Server::start(vec![challenge(), verified(false), accounts()]);
    let url = server.url().to_owned();
    home.write_config(&format!(
        "[profiles.default]\ntoken = \"{TOKEN}\"\ntoken_origin = \"{url}\"\n"
    ));
    let doc = hipc(&home, &server)
        .run(&["--dry-run", "auth", "login", "--access-key-file", &file])
        .ok();
    assert_eq!(doc["replaces_token"], true);
    // Asked about the origin first, then about the token; `--yes` answers both.
    let refused = hipc(&home, &server)
        .run(&["auth", "login", "--access-key-file", &file])
        .fails(2, "confirmation_required");
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .starts_with("this would log in with the access key at"),
        "{refused}"
    );
    assert!(server.requests().is_empty());
    assert!(home.config_text().contains(TOKEN));
    hipc(&home, &server)
        .run(&["auth", "login", "--access-key-file", &file, "--yes"])
        .ok();
    let config = home.config_text();
    assert!(config.contains(NEW_TOKEN), "{config}");
    assert!(!config.contains(TOKEN), "{config}");
    let fresh = Home::new();
    let file = phrase_file(&fresh);
    let doc = hipc(&fresh, &server)
        .run(&["--dry-run", "auth", "login", "--access-key-file", &file])
        .ok();
    assert_eq!(doc["replaces_token"], false);
}

/// An access key typed on the command line by mistake, as stray words or as the value of
/// `--access-key-file`, is refused without being repeated in any output.
#[test]
fn an_access_key_on_the_command_line_is_refused_without_being_repeated() {
    let home = Home::new();
    let server = Server::start(vec![]);
    let words: Vec<&str> = PHRASES[0].split_whitespace().collect();
    let mut stray = vec!["auth", "login", "--access-key"];
    stray.extend_from_slice(&words);
    let mut as_one = vec!["--output", "text", "auth", "login", "--access-key"];
    as_one.push(PHRASES[0]);
    let upper = PHRASES[0].to_uppercase();
    for args in [
        stray,
        as_one,
        vec!["auth", "login", "--access-key-file", PHRASES[0], "--yes"],
        vec!["auth", "login", "--access-key-file", &upper, "--yes"],
    ] {
        let run = hipc(&home, &server).run(&args);
        assert_eq!(run.code(), Some(2), "{args:?}");
        let out = format!("{}{}", run.stdout(), run.stderr());
        assert!(
            out.contains("the command line holds what looks like an access key"),
            "{out}"
        );
        let shown: Vec<String> = out
            .split(|c: char| !c.is_ascii_alphabetic())
            .map(str::to_lowercase)
            .collect();
        for word in &words {
            assert!(!shown.iter().any(|w| w == word), "{word} in {out}");
        }
    }
    // Eleven words in a row are not taken for a key.
    assert!(!hippius::holds_phrase(&words[..11].join(" ")));
    assert!(hippius::holds_phrase(&format!("x {} y", PHRASES[0])));
    assert!(server.requests().is_empty());
}

/// A phrase written one word to a line whose words also appear in the messages' own text is
/// never redacted word by word: the message reads as written, and shows no word of the phrase
/// by its absence.
#[test]
fn a_phrase_one_word_to_a_line_is_not_redacted_word_by_word() {
    let home = Home::new();
    let server = Server::start(vec![]);
    // Valid list words that hipc's messages use, with a checksum that fails.
    let path = home.file(
        "lines.txt",
        b"access\nkey\nword\nlist\nphrase\nfile\nabandon\nabandon\nabandon\nabandon\nabandon\nabandon\n",
    );
    let file = path.to_str().unwrap();
    let error = hipc(&home, &server)
        .run(&["auth", "login", "--access-key-file", file, "--yes"])
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        format!(
            "the access key phrase from `{file}` fails its BIP-39 checksum: a word is wrong or \
             out of place"
        )
    );
    assert_eq!(
        error["hint"],
        "leave only the access key's words in the file `--access-key-file` names"
    );
    assert!(server.requests().is_empty());
}

/// With `HIPPIUS_TOKEN` set, a login warns that later commands keep sending it, not the token
/// just stored.
#[test]
fn a_login_under_hippius_token_warns_that_the_variable_still_wins() {
    let home = Home::new();
    let server = Server::start(vec![challenge(), verified(false), accounts()]);
    let file = phrase_file(&home);
    let run = hipc(&home, &server).env("HIPPIUS_TOKEN", TOKEN).run(&[
        "--output",
        "text",
        "auth",
        "login",
        "--access-key-file",
        &file,
        "--yes",
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        "warning: `HIPPIUS_TOKEN` is set, so later commands send it, not the token just stored; \
         unset it to act as the access key's account\n"
    );
}

/// stdin that cannot be read, such as a directory, is `usage` that names stdin, and nothing
/// is sent.
#[test]
fn a_stdin_that_cannot_be_read_is_usage() {
    let home = Home::new();
    let server = Server::start(vec![]);
    let directory = std::fs::File::open(home.path()).expect("open the home directory");
    let output = crate::common::command(&home)
        .env("HIPPIUS_API_URL", server.url())
        .args(["auth", "login", "--access-key-file", "-", "--yes"])
        .stdin(directory)
        .output()
        .expect("run hipc");
    assert_eq!(output.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&output.stderr).expect("a JSON error");
    assert_eq!(error["error"]["code"], "usage");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("cannot read the access key phrase from stdin: "),
        "{error}"
    );
    assert!(server.requests().is_empty());
}
