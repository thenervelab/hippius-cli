//! `hipc auth login`, `status`, and `logout`.

use std::os::unix::fs::PermissionsExt as _;

use hippius::routes::account;
use serde_json::Value;

use crate::common::{
    ACCOUNTS, Hipc, Home, ME, MEMBER, Recorded, Reply, Server, TOKEN, draft, no_server, route,
    route_of, status_routes,
};

fn mode(path: &std::path::Path) -> u32 {
    std::fs::metadata(path)
        .expect("the file exists")
        .permissions()
        .mode()
        & 0o777
}

#[test]
fn login_stores_the_token_in_a_private_dot_hippius() {
    let home = Home::new();
    let token_file = home.file("token", format!("{TOKEN}\n").as_bytes());
    let doc = Hipc::without_api_url(&home)
        .run(&[
            "auth",
            "login",
            "--token-file",
            token_file.to_str().unwrap(),
        ])
        .ok();
    assert_eq!(doc["profile"], "default");
    assert_eq!(doc["token_stored"], true);
    assert_eq!(doc["config_path"], home.config().display().to_string());
    assert_eq!(mode(&home.config()), 0o600);
    assert_eq!(mode(home.config().parent().unwrap()), 0o700);
    let text = home.config_text();
    assert!(text.contains(&format!("token = \"{TOKEN}\"")), "{text}");
}

#[test]
fn login_reads_stdin_for_a_dash_and_trims_whitespace() {
    let home = Home::new();
    Hipc::without_api_url(&home)
        .stdin(format!("  {TOKEN}\r\n").as_bytes())
        .run(&["auth", "login", "--token-file", "-"])
        .ok();
    assert!(home.config_text().contains(&format!("\"{TOKEN}\"")));
}

#[test]
fn login_into_a_named_profile() {
    let home = Home::new();
    let token_file = home.file("token", TOKEN.as_bytes());
    let path = token_file.to_str().unwrap();
    let doc = Hipc::without_api_url(&home)
        .run(&["auth", "login", "--token-file", path, "--profile", "work"])
        .ok();
    assert_eq!(doc["profile"], "work");
    let doc = Hipc::without_api_url(&home)
        .env("HIPPIUS_PROFILE", "env-profile")
        .run(&["auth", "login", "--token-file", path])
        .ok();
    assert_eq!(doc["profile"], "env-profile");
    let text = home.config_text();
    assert!(text.contains("[profiles.work]") && text.contains("[profiles.env-profile]"));
}

#[test]
fn login_refuses_bad_token_sources_and_writes_nothing() {
    let home = Home::new();
    let empty = home.file("empty", b" \n\t");
    let binary = home.file("binary", &[0xff, 0xfe, b'a']);
    let huge = home.file("huge", &vec![b'a'; 64 * 1024 + 1]);
    let missing = home.path().join("missing");
    for path in [&empty, &binary, &huge, &missing] {
        Hipc::new(&home)
            .run(&["auth", "login", "--token-file", path.to_str().unwrap()])
            .fails(2, "usage");
    }
    assert!(!home.config().exists());
}

/// A token file that cannot be opened is named, with the reason in hipc's words rather than
/// the system's sentence.
#[test]
fn login_says_why_the_token_file_cannot_be_read() {
    let home = Home::new();
    let missing = home.path().join("missing");
    let path = missing.to_str().unwrap();
    let error = Hipc::new(&home)
        .run(&["auth", "login", "--token-file", path])
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        "cannot read the token from the file `--token-file` names: it does not exist; the path \
         is not repeated here, in case it is the token itself"
    );
    // A directory is not a regular file, which every file flag reads.
    let path = home.path().to_str().unwrap();
    let error = Hipc::new(&home)
        .run(&["auth", "login", "--token-file", path])
        .fails(2, "usage");
    assert_eq!(error["message"], format!("`{path}` is not a regular file"));
    // Stdin is read whatever it is; a read that fails is named with its reason.
    let error = Hipc::new(&home)
        .stdin_from(std::fs::File::open(home.path()).unwrap())
        .run(&["--json", "auth", "login", "--token-file", "-"])
        .fails(2, "usage");
    let message = error["message"].as_str().unwrap();
    assert!(
        message.contains("cannot read the token from stdin: it is a directory"),
        "{message}"
    );
}

#[test]
fn login_without_a_token_file_is_usage_even_when_hippius_token_is_set() {
    let home = Home::new();
    let run = Hipc::new(&home)
        .env("HIPPIUS_TOKEN", TOKEN)
        .run(&["auth", "login"]);
    let error = run.fails(2, "usage");
    // Off a terminal nobody is asked for the token: the hint names the flag that gives it.
    assert_eq!(
        error["message"],
        "`--token-file` is required when nobody at a terminal can be asked for the token"
    );
    assert_eq!(
        error["hint"],
        "pass `--token-file <path>`, or `--token-file -` to read the token from stdin"
    );
    assert!(!home.config().exists());
}

/// With `HIPPIUS_TOKEN` set, a login warns that later commands keep sending it, not the token
/// just stored, and names no `hipc auth status` to check it, since that would check the
/// variable.
#[test]
fn a_login_under_hippius_token_warns_that_the_variable_still_wins() {
    let home = Home::new();
    let token_file = home.file("token", b"tok_stored_by_the_login_0123456789abcdef\n");
    let run = Hipc::without_api_url(&home)
        .env("HIPPIUS_TOKEN", TOKEN)
        .run(&[
            "--output",
            "text",
            "auth",
            "login",
            "--token-file",
            token_file.to_str().unwrap(),
        ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        "warning: `HIPPIUS_TOKEN` is set, so later commands send it, not the token just stored; \
         unset it to act as the stored token's owner\n"
    );
    assert!(
        home.config_text()
            .contains("tok_stored_by_the_login_0123456789abcdef")
    );
    // Without the variable there is no warning, and text mode names the check instead.
    let run = Hipc::without_api_url(&home).run(&[
        "--output",
        "text",
        "auth",
        "login",
        "--token-file",
        token_file.to_str().unwrap(),
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(!run.stderr().contains("HIPPIUS_TOKEN"), "{}", run.stderr());
    assert!(
        run.stderr().contains("run `hipc auth status` to check it"),
        "{}",
        run.stderr()
    );
}

/// The access-key login is gone: each of its flags is a plain usage error from the argument
/// parser, which names the flag and sends and stores nothing.
#[test]
fn the_removed_access_key_flags_are_usage_errors() {
    let home = Home::new();
    let server = Server::start(vec![]);
    let key_file = home.file("key.txt", b"x\n");
    let key_file = key_file.to_str().unwrap();
    for (args, flag) in [
        (vec!["auth", "login", "--access-key"], "--access-key"),
        (
            vec!["auth", "login", "--access-key-file", key_file],
            "--access-key-file",
        ),
        (vec!["auth", "login", "--new-account"], "--new-account"),
    ] {
        let error = Hipc::new(&home)
            .env("HIPPIUS_API_URL", server.url())
            .run(&args)
            .fails(2, "usage");
        let message = error["message"].as_str().unwrap();
        assert!(message.contains(&format!("`{flag}`")), "{message}");
    }
    assert!(server.requests().is_empty());
    assert!(!home.config().exists());
}

/// A seed phrase typed on the command line by mistake, as stray words after the removed
/// `--access-key` or as the value of a flag, is refused without being repeated in any output,
/// and nothing is sent.
#[test]
fn a_seed_phrase_on_the_command_line_is_refused_without_being_repeated() {
    let home = Home::new();
    let server = Server::start(vec![]);
    let phrase = crate::common::SEED_PHRASES[0];
    let words: Vec<&str> = phrase.split_whitespace().collect();
    let mut stray = vec!["auth", "login", "--access-key"];
    stray.extend_from_slice(&words);
    let mut as_one = vec!["--output", "text", "auth", "login", "--access-key"];
    as_one.push(phrase);
    let upper = phrase.to_uppercase();
    for args in [
        stray,
        as_one,
        vec!["auth", "login", "--access-key-file", phrase, "--yes"],
        vec!["auth", "login", "--token-file", &upper, "--yes"],
        vec![
            "billing",
            "gamma",
            "hotkeys",
            "create",
            "--hotkey-file",
            phrase,
            "--yes",
        ],
        // A value clap takes, which a later check would refuse by quoting it.
        vec![
            "s3",
            "configure",
            "--access-key-id",
            "hip_A",
            "--secret-key-file",
            "-",
            "--aws-profile",
            phrase,
            "--dry-run",
        ],
    ] {
        let run = Hipc::new(&home)
            .env("HIPPIUS_API_URL", server.url())
            .run(&args);
        assert_eq!(run.code(), Some(2), "{args:?}");
        let out = format!("{}{}", run.stdout(), run.stderr());
        // Every run refuses argv that holds the words, before anything reads a flag.
        assert!(
            out.contains("the command line holds what looks like a seed phrase"),
            "{args:?}: {out}"
        );
        let shown: Vec<String> = out
            .split(|c: char| !c.is_ascii_alphabetic())
            .map(str::to_lowercase)
            .collect();
        for word in &words {
            assert!(!shown.iter().any(|w| w == word), "{word} in {out}");
        }
    }
    assert!(server.requests().is_empty());
}

#[test]
fn login_dry_run_writes_nothing() {
    let home = Home::new();
    let token_file = home.file("token", TOKEN.as_bytes());
    let doc = Hipc::without_api_url(&home)
        .run(&[
            "auth",
            "login",
            "--token-file",
            token_file.to_str().unwrap(),
            "--dry-run",
        ])
        .ok();
    assert_eq!(doc["dry_run"], true);
    assert_eq!(doc["token_origin"], "https://api.hippius.com");
    // The profile is named once, as the real run names it, with no `target` beside it.
    let fields: Vec<&str> = doc
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        fields,
        [
            "dry_run",
            "action",
            "profile",
            "token_stored",
            "replaces_token",
            "token_origin",
            "config_path"
        ]
    );
    assert_eq!(doc["replaces_token"], false);
    assert!(!home.config().exists());
}

/// A login over a different stored token moves every later command to the new token's
/// owner, so it needs `--yes`; the same token again, or a first login, does not.
#[test]
fn login_over_a_different_stored_token_needs_yes() {
    let home = Home::new();
    home.write_config(
        "[profiles.default]\ntoken = \"tok_old_0123456789\"\ntoken_origin = \"https://api.hippius.com\"\n",
    );
    let file = home.file("token", TOKEN.as_bytes());
    let path = file.to_str().unwrap();
    let login = |extra: &[&str]| {
        let mut args = vec!["auth", "login", "--token-file", path];
        args.extend(extra);
        Hipc::without_api_url(&home).run(&args)
    };
    let dry = login(&["--dry-run"]).ok();
    assert_eq!(dry["replaces_token"], true);
    let refused = login(&[]).fails(2, "confirmation_required");
    assert_eq!(
        refused["message"],
        "this would replace the token stored in the profile `default` with a different one, \
         and `--yes` was not passed"
    );
    assert!(home.config_text().contains("tok_old_0123456789"));
    assert!(!home.config_text().contains(TOKEN));
    login(&["--yes"]).ok();
    assert!(home.config_text().contains(TOKEN));
    assert!(!home.config_text().contains("tok_old_0123456789"));
    // The same token again replaces nothing.
    assert_eq!(login(&["--dry-run"]).ok()["replaces_token"], false);
    login(&[]).ok();
    // Another profile is not the one replaced.
    login(&["--profile", "work"]).ok();
    // An empty stored token, which a hand-edited file can hold, is no token to replace.
    home.write_config("[profiles.default]\ntoken = \"\"\n");
    assert_eq!(login(&["--dry-run"]).ok()["replaces_token"], false);
    login(&[]).ok();
}

/// A dry run names the origin as the real run stores it, and refuses an origin the real
/// run refuses, before anything is written.
#[test]
fn login_dry_run_parses_the_origin_as_the_real_run_does() {
    let home = Home::new();
    let token_file = home.file("token", TOKEN.as_bytes());
    let path = token_file.to_str().unwrap();
    let login = |api_url: &str, dry_run: bool| {
        let mut args = vec!["--json", "--yes", "auth", "login", "--token-file", path];
        if dry_run {
            args.push("--dry-run");
        }
        Hipc::new(&home).env("HIPPIUS_API_URL", api_url).run(&args)
    };
    let refused = login("ftp://x", true).fails(2, "usage");
    assert_eq!(login("ftp://x", false).fails(2, "usage"), refused);
    assert!(!home.config().exists());
    let doc = login("http://127.0.0.1:8/", true).ok();
    assert_eq!(doc["token_origin"], "http://127.0.0.1:8");
    assert!(!home.config().exists());
    let doc = login("http://127.0.0.1:8/", false).ok();
    assert_eq!(doc["token_origin"], "http://127.0.0.1:8");
}

#[test]
fn login_honors_hippius_config() {
    let home = Home::new();
    let token_file = home.file("token", TOKEN.as_bytes());
    let elsewhere = home.path().join("custom").join("hipc.toml");
    let doc = Hipc::without_api_url(&home)
        .env("HIPPIUS_CONFIG", elsewhere.to_str().unwrap())
        .run(&[
            "auth",
            "login",
            "--token-file",
            token_file.to_str().unwrap(),
        ])
        .ok();
    assert_eq!(doc["config_path"], elsewhere.display().to_string());
    assert_eq!(mode(&elsewhere), 0o600);
    assert!(!home.config().exists());
}

#[test]
fn status_proves_the_token_without_the_account_header() {
    let server = Server::start(status_routes());
    let home = Home::new();
    let doc = Hipc::against(&home, &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["auth", "status"])
        .ok();
    // The document is the status itself, with no `ok` field: exit 0 says it worked.
    assert_eq!(doc.get("ok"), None);
    assert_eq!(doc["profile"], "default");
    assert_eq!(doc["account"], MEMBER);
    assert_eq!(doc["token_preview"], &TOKEN[TOKEN.len() - 4..]);
    assert_eq!(doc["token_source"], "environment");
    assert_eq!(doc["api_url"], server.url());
    // The user is named as the server sent it, and the read fails nothing.
    assert_eq!(doc["user"], serde_json::from_str::<Value>(ME).unwrap());
    assert!(doc.get("user_error").is_none());
    // The token check, then the user read: both personal, so neither carries the header.
    let requests = server.requests();
    let paths: Vec<&str> = requests.iter().map(Recorded::path).collect();
    assert_eq!(paths, ["/api/accounts/", "/api/users/me/"]);
    for request in &requests {
        assert_eq!(
            request.header("authorization"),
            Some(&*format!("Token {TOKEN}"))
        );
        assert_eq!(request.header("x-hippius-account"), None);
        assert_eq!(
            request.header("user-agent"),
            Some(&*format!("hipc/{}", env!("CARGO_PKG_VERSION")))
        );
        assert_eq!(request.header("accept"), Some("application/json"));
    }
}

/// Swagger documents `GET /api/users/me/` as a page of `User` rows: a page that holds
/// exactly one user names it.
#[test]
fn status_reads_the_user_from_a_page_of_one() {
    let page = format!(r#"{{"count":1,"next":null,"previous":null,"results":[{ME}]}}"#);
    let server = Server::start(vec![
        route_of(&account::ACCOUNTS, [], Reply::json(200, ACCOUNTS)),
        route_of(&account::USER, [], Reply::json(200, &page)),
    ]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["auth", "status"])
        .ok();
    assert_eq!(doc["user"]["username"], "ana");
    assert_eq!(doc["user"]["email"], "ana@example.com");
}

/// A failed user read keeps the token check: exit 0, `ok` true, `user` null, and
/// `user_error` with the failure's code and message; text mode warns on stderr. An
/// answer `hipc` does not read is such a failure, never a user.
#[test]
fn status_keeps_the_token_check_when_the_user_read_fails() {
    let unreadable = "cannot read the signed-in user: the API answered with a shape `hipc` \
                      does not read";
    for (reply, code, message) in [
        (
            Reply::json(500, r#"{"detail":"boom"}"#),
            "internal",
            "cannot read the signed-in user: the API says `boom`",
        ),
        (
            Reply::json(403, r#"{"detail":"nope"}"#),
            "permission",
            "cannot read the signed-in user: the API says `nope`",
        ),
        (Reply::json(200, r#"{"id":7}"#), "internal", unreadable),
        (
            Reply::json(200, r#"{"username":7}"#),
            "internal",
            unreadable,
        ),
        (
            Reply::json(200, r#"{"results":[]}"#),
            "internal",
            unreadable,
        ),
        (
            Reply::json(200, &format!(r#"{{"results":[{ME},{ME}]}}"#)),
            "internal",
            unreadable,
        ),
        (Reply::json(200, "[]"), "internal", unreadable),
    ] {
        let server = Server::start(vec![
            route_of(&account::ACCOUNTS, [], Reply::json(200, ACCOUNTS)),
            route_of(&account::USER, [], reply),
        ]);
        let doc = Hipc::against(&Home::new(), &server)
            .run(&["auth", "status"])
            .ok();
        assert_eq!(doc.get("ok"), None);
        assert_eq!(doc["user"], Value::Null);
        assert_eq!(doc["user_error"]["code"], code, "{doc}");
        assert_eq!(doc["user_error"]["message"], message, "{doc}");
        let run = Hipc::against(&Home::new(), &server).run(&["--output", "text", "auth", "status"]);
        assert_eq!(run.code(), Some(0));
        assert!(
            run.stderr().contains(&format!(
                "warning: the token works, but the signed-in user is unknown: {message}"
            )),
            "{}",
            run.stderr()
        );
    }
}

#[test]
fn status_with_a_stored_token() {
    let server = Server::start(status_routes());
    let home = Home::new();
    let token_file = home.file("token", b"stored-token-abcd");
    Hipc::tokenless(&home, &server)
        .run(&[
            "auth",
            "login",
            "--yes",
            "--token-file",
            token_file.to_str().unwrap(),
        ])
        .ok();
    let doc = Hipc::tokenless(&home, &server)
        .run(&["auth", "status"])
        .ok();
    assert_eq!(doc["token_preview"], "abcd");
    assert_eq!(doc["token_source"], "config");
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    for request in requests {
        assert_eq!(
            request.header("authorization"),
            Some("Token stored-token-abcd")
        );
    }
}

#[test]
fn status_with_a_rejected_token_is_auth() {
    let server = Server::start(vec![route_of(
        &account::ACCOUNTS,
        [],
        Reply::json(401, r#"{"detail":"Invalid token."}"#),
    )]);
    let home = Home::new();
    let error = Hipc::against(&home, &server)
        .run(&["auth", "status"])
        .fails(1, "auth");
    assert_eq!(
        error["message"],
        "cannot check the token: the API says `Invalid token.`"
    );
    assert_eq!(error["http_status"], 401);
    // `HIPPIUS_TOKEN` wins over a stored token, so a login cannot fix it.
    assert_eq!(
        error["hint"],
        "unset `HIPPIUS_TOKEN`, or set it to a valid console token"
    );
    // A stored token is fixed by storing a valid one.
    home.write_config(&format!(
        "[profiles.default]\ntoken = \"stored-token-abcd\"\ntoken_origin = \"{}\"\n",
        server.url()
    ));
    let error = Hipc::tokenless(&home, &server)
        .run(&["auth", "status"])
        .fails(1, "auth");
    assert_eq!(
        error["hint"],
        "run `hipc auth login --token-file <path>` to store a valid token"
    );
}

#[test]
fn status_without_any_token_sends_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let error = Hipc::tokenless(&home, &server)
        .run(&["auth", "status"])
        .fails(1, "auth");
    assert!(error["hint"].as_str().unwrap().contains("hipc auth login"));
    assert!(server.requests().is_empty());
}

#[test]
fn status_dry_run_describes_the_request() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let doc = Hipc::against(&home, &server)
        .run(&["auth", "status", "--dry-run"])
        .ok();
    assert_eq!(doc["dry_run"], true);
    assert_eq!(doc["requests"][0]["method"], "GET");
    assert_eq!(
        doc["requests"][0]["url"],
        format!("{}/api/accounts/", server.url())
    );
    // A real run reads the user too, so the dry run lists both reads.
    let urls: Vec<&str> = doc["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["url"].as_str().unwrap())
        .collect();
    let base = server.url();
    assert_eq!(
        urls,
        [
            format!("{base}/api/accounts/"),
            format!("{base}/api/users/me/")
        ]
    );
    assert!(server.requests().is_empty());
}

fn logged_in(home: &Home) {
    let token_file = home.file("token", TOKEN.as_bytes());
    Hipc::without_api_url(home)
        .run(&[
            "auth",
            "login",
            "--token-file",
            token_file.to_str().expect("a UTF-8 temp path"),
        ])
        .ok();
}

#[test]
fn logout_on_a_pipe_needs_yes_and_keeps_the_token() {
    let home = Home::new();
    logged_in(&home);
    let error = Hipc::new(&home)
        .run(&["auth", "logout"])
        .fails(2, "confirmation_required");
    assert_eq!(error["hint"], "pass `--yes` to confirm");
    assert!(home.config_text().contains(TOKEN));
    let error = Hipc::new(&home)
        .run(&["auth", "logout", "--no-input"])
        .fails(2, "confirmation_required");
    assert_eq!(error["side_effects"], "none");
}

#[test]
fn logout_with_yes_removes_the_token_and_repeats_cleanly() {
    let home = Home::new();
    logged_in(&home);
    let doc = Hipc::new(&home).run(&["auth", "logout", "--yes"]).ok();
    assert_eq!(
        doc,
        serde_json::json!({"profile": "default", "token_stored": false})
    );
    assert!(!home.config_text().contains(TOKEN));
    Hipc::new(&home).run(&["auth", "logout", "--yes"]).ok();
    Hipc::new(&Home::new())
        .run(&["auth", "logout", "--yes"])
        .ok();
}

/// A logout from a config file with another hard link warns that the other link still holds
/// the token: the rewrite replaced this name only. Text mode warns on stderr; JSON mode keeps
/// stderr empty and adds the same text to a `warnings` array in the document. The exit code
/// does not change. A file with one link gets no warning.
#[test]
fn logout_from_a_hard_linked_config_warns_that_the_other_link_keeps_the_token() {
    let home = Home::new();
    logged_in(&home);
    let other = home.path().join("dotfiles-config.toml");
    std::fs::hard_link(home.config(), &other).unwrap();
    let run = Hipc::new(&home).run(&["--output", "text", "auth", "logout", "--yes"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        format!(
            "warning: the config file `{}` had another hard link, which still holds its old \
             contents, tokens included; only this name was changed\n",
            home.config().display()
        )
    );
    assert!(!home.config_text().contains(TOKEN));
    assert!(std::fs::read_to_string(&other).unwrap().contains(TOKEN));
    // The link is split now, so a second write has nothing to warn about.
    logged_in(&home);
    let run = Hipc::new(&home).run(&["--output", "text", "auth", "logout", "--yes"]);
    assert_eq!(run.stderr(), "");
    // JSON mode keeps stderr empty on success (tenet 3), and says it in the document.
    logged_in(&home);
    std::fs::remove_file(&other).unwrap();
    std::fs::hard_link(home.config(), &other).unwrap();
    let run = Hipc::new(&home).run(&["--json", "auth", "logout", "--yes"]);
    assert_eq!(run.code(), Some(0));
    assert_eq!(run.stderr(), "");
    assert_eq!(
        run.json(),
        serde_json::json!({
            "profile": "default",
            "token_stored": false,
            "warnings": [format!(
                "the config file `{}` had another hard link, which still holds its old \
                 contents, tokens included; only this name was changed",
                home.config().display()
            )],
        })
    );
    // With no other link, no `warnings` field.
    logged_in(&home);
    let doc = Hipc::new(&home)
        .run(&["--json", "auth", "logout", "--yes"])
        .ok();
    assert!(doc.get("warnings").is_none(), "{doc}");
}

#[test]
fn logout_dry_run_keeps_the_token() {
    let home = Home::new();
    logged_in(&home);
    let doc = Hipc::new(&home).run(&["auth", "logout", "--dry-run"]).ok();
    // One shape for every local dry run, `dry_run` first, then the document a real run
    // prints.
    assert_eq!(
        doc.to_string(),
        concat!(
            r#"{"dry_run":true,"action":"remove_token","#,
            r#""profile":"default","token_stored":false}"#
        )
    );
    assert!(home.config_text().contains(TOKEN));
}

#[test]
fn a_short_token_has_no_preview() {
    let server = Server::start(status_routes());
    let doc = Hipc::tokenless(&Home::new(), &server)
        .with_env_token(server.url(), "short")
        .run(&["auth", "status"])
        .ok();
    assert_eq!(doc["token_preview"], serde_json::Value::Null);
}

#[test]
fn a_stored_token_is_sent_only_to_the_origin_it_was_stored_for() {
    let api = Server::start(status_routes());
    let other = Server::start(status_routes());
    let home = Home::new();
    let file = home.file("token", TOKEN.as_bytes());
    let doc = Hipc::new(&home)
        .env("HIPPIUS_API_URL", &api.url().to_uppercase())
        .run(&[
            "auth",
            "login",
            "--yes",
            "--token-file",
            file.to_str().unwrap(),
        ])
        .ok();
    assert_eq!(
        doc["token_origin"],
        api.url(),
        "printed as stored, normalized"
    );
    Hipc::tokenless(&home, &api).run(&["auth", "status"]).ok();
    assert_eq!(
        api.requests()[0].header("authorization"),
        Some(&*format!("Token {TOKEN}"))
    );
    // Another origin, by flag or by variable: the stored token is withheld, nothing is sent.
    for (hipc, args) in [
        (Hipc::tokenless(&home, &other), vec!["auth", "status"]),
        (
            Hipc::tokenless(&home, &api),
            vec!["--api-url", other.url(), "auth", "status"],
        ),
        // Neither is set, so the run targets the default origin: the hint cannot be to
        // remove them. The check refuses before anything is sent.
        (Hipc::without_api_url(&home), vec!["auth", "status"]),
    ] {
        let error = hipc.run(&args).fails(1, "auth");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("stored token is for")
        );
        // Whichever origin the run targets and wherever it came from, the flag wins.
        assert_eq!(
            error["hint"],
            format!(
                "pass `--api-url {}` to send the stored token to the origin it was stored for",
                api.url()
            )
        );
    }
    assert!(
        other.requests().is_empty(),
        "the token never reached the other origin"
    );
    // HIPPIUS_TOKEN goes to another origin only when HIPPIUS_TOKEN_ORIGIN names it.
    Hipc::tokenless(&home, &other)
        .with_env_token(other.url(), "tok_for_the_other_host_0123456789")
        .run(&["auth", "status"])
        .ok();
    assert_eq!(other.requests().len(), 2);
}

/// A command whose routes need no token, such as `hipc api`, refuses a withheld
/// `HIPPIUS_TOKEN` too, before anything is sent: the run asked for a token, and a request
/// without it would answer for no one. A dry run still works.
#[test]
fn a_withheld_token_variable_refuses_every_command_before_sending() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let body = home.file("body.json", b"{}");
    let hipc = || {
        Hipc::tokenless(&home, &server)
            .env("HIPPIUS_TOKEN", TOKEN)
            .env_remove("HIPPIUS_TOKEN_ORIGIN")
    };
    for args in [
        &["api", "GET", "/accounts/"][..],
        &[
            "api",
            "POST",
            "/ssh-keys/",
            "--body-file",
            body.to_str().unwrap(),
            "--yes",
        ],
        &["drive", "plans"],
    ] {
        let error = hipc().run(args).fails(1, "auth");
        let hint = error["hint"].as_str().unwrap();
        assert!(hint.contains("HIPPIUS_TOKEN_ORIGIN"), "{args:?}: {hint}");
    }
    hipc().run(&["api", "GET", "/accounts/", "--dry-run"]).ok();
    assert!(server.requests().is_empty());
}

/// `HIPPIUS_TOKEN` is bound to the default origin, or to the one `HIPPIUS_TOKEN_ORIGIN`
/// names: `--api-url` or `HIPPIUS_API_URL` alone cannot send it elsewhere, nor can a
/// `HIPPIUS_TOKEN_ORIGIN` that names another origin, and the stored token does not stand in.
/// A spelling of the same origin (case, a trailing `/`, the default port) is that origin.
#[test]
fn the_token_variable_is_sent_only_to_the_origin_it_is_bound_to() {
    let other = Server::start(status_routes());
    let elsewhere = Server::start(vec![]);
    let home = Home::new();
    let file = home.file("token", TOKEN.as_bytes());
    // A stored token for this origin, which a withheld variable must not fall back to.
    Hipc::tokenless(&home, &other)
        .run(&[
            "auth",
            "login",
            "--yes",
            "--token-file",
            file.to_str().unwrap(),
        ])
        .ok();
    let variable = "tok_for_the_other_host_0123456789";
    let port = other.url().rsplit(':').next().unwrap().to_owned();
    for (bound, by_flag) in [
        (None, false),
        (None, true),
        (Some(elsewhere.url().to_owned()), false),
        (Some(format!("https://127.0.0.1:{port}")), false),
        (Some("https://api.hippius.com".to_owned()), true),
    ] {
        // The harness binds the variable to its dead origin; `None` is no binding at all.
        let mut hipc = Hipc::new(&home)
            .env("HIPPIUS_TOKEN", variable)
            .env_remove("HIPPIUS_TOKEN_ORIGIN");
        if let Some(bound) = &bound {
            hipc = hipc.env("HIPPIUS_TOKEN_ORIGIN", bound);
        }
        let args: Vec<&str> = if by_flag {
            vec!["--api-url", other.url(), "auth", "status"]
        } else {
            hipc = hipc.env("HIPPIUS_API_URL", other.url());
            vec!["auth", "status"]
        };
        let error = hipc.run(&args).fails(1, "auth");
        let expected_bound = bound.as_deref().unwrap_or("https://api.hippius.com");
        assert_eq!(
            error["message"],
            format!(
                "`HIPPIUS_TOKEN` is for `{expected_bound}`, and this command targets `{}`, so \
                 the token was not sent",
                other.url()
            ),
            "{bound:?}"
        );
        assert_eq!(
            error["hint"],
            "set `HIPPIUS_TOKEN_ORIGIN` to the origin `HIPPIUS_TOKEN` is meant for; it is sent \
             to no other"
        );
    }
    assert!(other.requests().is_empty(), "nothing was sent");
    assert!(elsewhere.requests().is_empty());
    // A value that is no origin is refused, naming the variable.
    let error = Hipc::tokenless(&home, &other)
        .with_env_token(other.url(), variable)
        .env("HIPPIUS_TOKEN_ORIGIN", "127.0.0.1")
        .run(&["auth", "status"])
        .fails(2, "usage");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("`HIPPIUS_TOKEN_ORIGIN`"),
        "{error}"
    );
    // The same origin in another spelling is that origin.
    for spelling in [
        other.url().replace("http://", "HTTP://"),
        format!("{}/", other.url()),
    ] {
        Hipc::tokenless(&home, &other)
            .with_env_token(&spelling, variable)
            .run(&["auth", "status"])
            .ok();
    }
    for request in other.requests() {
        assert_eq!(
            request.header("authorization"),
            Some(&*format!("Token {variable}"))
        );
    }
    assert_eq!(other.requests().len(), 4);
}

#[test]
fn a_profile_stored_before_token_origins_holds_a_token_for_the_default_api() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let config = home.config();
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::set_permissions(
        config.parent().unwrap(),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    std::fs::write(
        &config,
        format!("[profiles.default]\ntoken = \"{TOKEN}\"\n"),
    )
    .unwrap();
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    let error = Hipc::tokenless(&home, &server)
        .run(&["auth", "status"])
        .fails(1, "auth");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("https://api.hippius.com"),
        "{error}"
    );
    assert!(server.requests().is_empty());
}

#[test]
fn the_token_is_checked_after_argv_reveal_and_dry_run_and_before_the_question() {
    let server = Server::start(vec![]);
    let no_token = |args: &[&str]| Hipc::tokenless(&Home::new(), &server).run(args);
    // A dry run needs no token: the document has no header values.
    let doc = no_token(&["s3", "tokens", "create", "--kind", "master", "--dry-run"]).ok();
    assert_eq!(draft(&doc)["method"], "POST");
    no_token(&[
        "hub",
        "subscription",
        "subscribe",
        "--plan",
        "3",
        "--pay-from",
        "card",
    ])
    .fails(2, "usage");
    no_token(&["s3", "tokens", "create", "--kind", "master"]).fails(2, "reveal_required");
    // Then the token, before the confirmation: neither --yes nor its absence matters.
    for args in [
        &["s3", "tokens", "create", "--kind", "master", "--reveal"][..],
        &["compute", "vms", "delete", "vm-1"],
        &["hub", "subscription", "subscribe", "--plan", "3"],
        &["hub", "subscription", "subscribe", "--plan", "3", "--yes"],
        &["compute", "vms", "list"],
    ] {
        no_token(args).fails(1, "auth");
    }
    // A membership the route cannot use is refused first, for a read and a write alike,
    // as under --dry-run.
    for args in [
        &["drive", "subscription", "get"][..],
        &["drive", "subscription", "cancel"],
        &["drive", "subscription", "cancel", "--dry-run"],
    ] {
        Hipc::tokenless(&Home::new(), &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(args)
            .fails(2, "account_scope_unsupported");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn a_token_stored_for_another_origin_still_allows_a_dry_run() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let file = home.file("token", TOKEN.as_bytes());
    Hipc::without_api_url(&home)
        .run(&[
            "auth",
            "login",
            "--token-file",
            file.to_str().unwrap(),
            "--yes",
        ])
        .ok();
    let hipc = || Hipc::tokenless(&home, &server);
    hipc()
        .run(&["compute", "vms", "delete", "vm-1", "--dry-run"])
        .ok();
    let error = hipc()
        .run(&["compute", "vms", "delete", "vm-1"])
        .fails(1, "auth");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("https://api.hippius.com"),
        "{error}"
    );
    assert!(server.requests().is_empty());
}

#[test]
fn login_refuses_a_token_with_a_line_break_inside_and_writes_nothing() {
    let home = Home::new();
    let file = home.file("token", b"tok_aaaaaaaaaaaa\ntok_bbbbbbbbbbbb\n");
    Hipc::new(&home)
        .run(&["auth", "login", "--token-file", file.to_str().unwrap()])
        .fails(2, "usage");
    assert!(!home.config().exists());
    Hipc::new(&home)
        .env("HIPPIUS_TOKEN", "tok_a\ntok_b")
        .run(&["auth", "status"])
        .fails(2, "usage");
}

#[test]
fn login_repairs_a_profile_whose_stored_token_is_broken() {
    let home = Home::new();
    home.write_config(
        "[profiles.default]\ntoken = \"tok_a\\ntok_b\"\ntoken_origin = \"not a url\"\n",
    );
    let file = home.file("token", TOKEN.as_bytes());
    let args = ["auth", "login", "--token-file", file.to_str().unwrap()];
    // The broken token is still another token, so the replacement is confirmed.
    Hipc::without_api_url(&home)
        .run(&args)
        .fails(2, "confirmation_required");
    let doc = Hipc::without_api_url(&home)
        .run(&[&args[..], &["--yes"]].concat())
        .ok();
    assert_eq!(doc["token_origin"], "https://api.hippius.com");
    assert!(home.config_text().contains(TOKEN));
}

#[test]
fn a_legacy_profile_with_its_own_api_url_keeps_its_token_there() {
    let server = Server::start(status_routes());
    let home = Home::new();
    home.write_config(&format!(
        "[profiles.default]\napi_url = \"{}\"\ntoken = \"{TOKEN}\"\n",
        server.url()
    ));
    Hipc::without_api_url(&home).run(&["auth", "status"]).ok();
    assert_eq!(
        server.requests()[0].header("authorization"),
        Some(&*format!("Token {TOKEN}"))
    );
}

#[test]
fn public_routes_and_hipc_api_go_to_another_origin_without_the_stored_token() {
    let home_api = Server::start(vec![]);
    let other = Server::start(vec![
        route("GET", "/api/x/", Reply::json(200, "{}")),
        route("GET", "/api/drive/plans/", Reply::json(200, "[]")),
    ]);
    let home = Home::new();
    let file = home.file("token", TOKEN.as_bytes());
    Hipc::tokenless(&home, &home_api)
        .run(&[
            "auth",
            "login",
            "--yes",
            "--token-file",
            file.to_str().unwrap(),
        ])
        .ok();
    for args in [
        vec!["--api-url", other.url(), "api", "GET", "/x/"],
        vec!["--api-url", other.url(), "drive", "plans"],
    ] {
        let run = Hipc::new(&home).run(&args);
        assert_eq!(run.code(), Some(0), "{args:?}: {}", run.stderr());
    }
    assert_eq!(other.requests().len(), 2);
    for request in other.requests() {
        assert_eq!(request.header("authorization"), None, "{}", request.target);
    }
}

#[test]
fn login_for_an_origin_other_than_the_default_needs_yes_and_names_its_source() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let file = home.file("token", TOKEN.as_bytes());
    let error = Hipc::tokenless(&home, &server)
        .run(&["auth", "login", "--token-file", file.to_str().unwrap()])
        .fails(2, "confirmation_required");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("named by `HIPPIUS_API_URL`"),
        "{error}"
    );
    let error = Hipc::new(&home)
        .run(&[
            "--api-url",
            server.url(),
            "auth",
            "login",
            "--token-file",
            file.to_str().unwrap(),
        ])
        .fails(2, "confirmation_required");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("named by `--api-url`"),
        "{error}"
    );
    assert!(!home.config().exists(), "nothing was stored");
    // The default origin needs no confirmation.
    Hipc::without_api_url(&home)
        .run(&["auth", "login", "--token-file", file.to_str().unwrap()])
        .ok();
}

#[test]
fn login_names_a_profile_api_url_as_the_source() {
    let home = Home::new();
    home.write_config(&format!(
        "[profiles.default]\napi_url = \"{}\"\n",
        no_server()
    ));
    let file = home.file("token", TOKEN.as_bytes());
    let error = Hipc::without_api_url(&home)
        .run(&["auth", "login", "--token-file", file.to_str().unwrap()])
        .fails(2, "confirmation_required");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("the `api_url` of the profile `default`"),
        "{error}"
    );
}

/// A word of token length that a command refuses as an unknown argument, value, or command
/// is not repeated by any command, not only by `hipc auth`, and the hint names the
/// command's help.
#[test]
fn a_token_shaped_word_is_never_repeated_by_any_usage_error() {
    let pasted = "tok_pasted_in_the_wrong_place_0123456789";
    for args in [
        &["completions", pasted][..],
        &["compute", "vms", "list", pasted],
        &["compute", "vms", "lisst-pasted-somewhere-wrong"],
    ] {
        let run = Hipc::new(&Home::new()).run(args);
        let error = run.fails(2, "usage");
        assert!(
            !run.stderr().contains("pasted"),
            "{args:?}: {}",
            run.stderr()
        );
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("so the word given is not repeated here"),
            "{args:?}: {error}"
        );
        assert!(
            error["hint"].as_str().unwrap().ends_with(" --help`"),
            "{args:?}: {error}"
        );
    }
    // A short word is still quoted, so a typo is easy to see.
    let error = Hipc::new(&Home::new())
        .run(&["completions", "lots"])
        .fails(2, "usage");
    assert!(
        error["message"].as_str().unwrap().contains("`lots`"),
        "{error}"
    );
}

/// A token passed where a path or no argument belongs is refused without being repeated: a
/// stray word after `auth login`, a known token anywhere on a command line, parsed or not, and
/// a secret-file flag whose path names no file.
#[test]
fn a_token_in_the_wrong_place_is_never_repeated() {
    let home = Home::new();
    let pasted = "tok_pasted_in_the_wrong_place_0123456789";
    let stored = "tok_stored_0123456789abcdef";
    home.write_config(&format!("[profiles.default]\ntoken = \"{stored}\"\n"));
    let cases: [(Vec<&str>, &str); 10] = [
        (
            vec!["auth", pasted],
            "`hipc auth` takes no such argument or value, so the word given is not repeated here",
        ),
        (
            vec!["auth", "status", pasted],
            "`hipc auth status` takes no such argument or value, so the word given is not repeated here",
        ),
        (
            vec!["auth", "login", pasted],
            "`hipc auth login` takes no such argument or value, so the word given is not repeated here",
        ),
        (
            vec!["--output", "text", "auth", "login", "--yes", pasted],
            "`hipc auth login` takes no such argument or value, so the word given is not repeated here",
        ),
        (
            vec!["version", stored],
            "the command line holds an API token, so it is not repeated here",
        ),
        (
            vec!["compute", "vms", "get", "--bogus", stored],
            "the command line holds an API token, so it is not repeated here",
        ),
        // Values clap takes, which a later check or message would quote.
        (
            vec![
                "s3",
                "configure",
                "--access-key-id",
                stored,
                "--secret-key-file",
                "-",
                "--dry-run",
            ],
            "the command line holds an API token, so it is not repeated here",
        ),
        (
            vec!["--profile", stored, "auth", "status"],
            "the command line holds an API token, so it is not repeated here",
        ),
        (
            vec!["auth", "login", "--token-file", pasted, "--yes"],
            "cannot read the token from the file `--token-file` names: it does not exist; the \
             path is not repeated here",
        ),
        (
            vec![
                "s3",
                "configure",
                "--access-key-id",
                "hip_A",
                "--secret-key-file",
                pasted,
                "--yes",
            ],
            "cannot read the secret key from the file `--secret-key-file` names",
        ),
    ];
    for (args, says) in cases {
        let run = Hipc::new(&home).run(&args);
        assert_eq!(run.code(), Some(2), "{args:?}: {}", run.stderr());
        let out = format!("{}{}", run.stdout(), run.stderr());
        assert!(out.contains(says), "{args:?}: {out}");
        assert!(!out.contains("0123456789"), "{args:?}: {out}");
    }
    // A flag clap does not know is still quoted, as is a word after another command.
    let run = Hipc::new(&home).run(&["auth", "login", "--tokn-file", "x"]);
    assert!(run.stderr().contains("`--tokn-file`"), "{}", run.stderr());
    let run = Hipc::new(&home).run(&["version", "extra"]);
    assert!(run.stderr().contains("`extra`"), "{}", run.stderr());
}
