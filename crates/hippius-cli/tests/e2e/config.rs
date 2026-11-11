//! The config file's location and its guards, seen from the binary.

use std::os::unix::fs::PermissionsExt as _;

use serde_json::json;

use crate::common::{self, Hipc, Home, Reply, Server, TOKEN, no_server, route, status_routes};

fn chmod(path: &std::path::Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .expect("chmod a test file");
}

fn login(home: &Home) {
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
fn a_config_others_can_read_is_refused_with_the_chmod_that_fixes_it() {
    let server = Server::start(vec![route("GET", "/api/accounts/", Reply::json(200, "{}"))]);
    let home = Home::new();
    login(&home);
    chmod(&home.config(), 0o644);
    let error = Hipc::tokenless(&home, &server)
        .run(&["auth", "status"])
        .fails(2, "usage");
    assert_eq!(
        error["hint"],
        format!("run `chmod 600 {}`", home.config().display())
    );
    assert!(server.requests().is_empty());
    chmod(&home.config(), 0o600);
}

#[test]
fn a_config_directory_others_can_write_is_refused() {
    let home = Home::new();
    login(&home);
    let dir = home.config().parent().unwrap().to_owned();
    chmod(&dir, 0o777);
    let error = Hipc::new(&home)
        .run(&["account", "use", "self"])
        .fails(2, "usage");
    assert!(error["hint"].as_str().unwrap().contains("chmod 700"));
    chmod(&dir, 0o700);
}

/// A config directory the current user cannot open is theirs to fix, so it is exit 2
/// `usage` with a hint, not `internal` with a report-a-bug hint.
#[test]
fn a_config_directory_the_user_cannot_open_is_usage() {
    if !common::permission_bits_apply() {
        return;
    }
    let home = Home::new();
    login(&home);
    let dir = home.config().parent().unwrap().to_owned();
    chmod(&dir, 0o000);
    let error = Hipc::new(&home)
        .run(&["compute", "vms", "list"])
        .fails(2, "usage");
    chmod(&dir, 0o700);
    assert_eq!(
        error["message"],
        format!("cannot open `{}`: permission is denied", dir.display())
    );
    assert_eq!(
        error["hint"],
        format!(
            "check that the current user can enter `{}` and each directory above it",
            dir.display()
        )
    );
}

#[test]
fn a_broken_config_names_the_path_and_never_quotes_it() {
    let home = Home::new();
    std::fs::create_dir_all(home.config().parent().unwrap()).unwrap();
    chmod(home.config().parent().unwrap(), 0o700);
    std::fs::write(home.config(), format!("token = \"{TOKEN}\" [[[")).unwrap();
    chmod(&home.config(), 0o600);
    let error = Hipc::new(&home)
        .run(&["account", "use", "self"])
        .fails(2, "usage");
    assert!(error["message"].as_str().unwrap().contains("config.toml"));
}

/// A key this `hipc` does not know, such as one a newer `hipc` wrote, is accepted and kept
/// when the file is rewritten, and a warning names where it is, once per run, never its name
/// or value: in text mode on stderr, in JSON mode in `warnings`.
#[test]
fn a_config_key_this_hipc_does_not_know_is_kept_and_warned_about_once() {
    let home = Home::new();
    home.write_config(&format!(
        "[profiles.default]\ntoken = \"{TOKEN}\"\nsurprise = \"s3cr3t-value\"\n"
    ));
    let warning = format!(
        "the config file `{}` holds keys this hipc does not know, in the profile `default`; \
         they are kept as they are, and a newer hipc may use them",
        home.config().display()
    );
    let doc = Hipc::new(&home).run(&["account", "use", "self"]).ok();
    assert_eq!(doc["warnings"], json!([warning]));
    assert!(
        home.config_text().contains("surprise = \"s3cr3t-value\""),
        "{}",
        home.config_text()
    );
    let run = Hipc::new(&home).run(&["--output", "text", "account", "use", "self"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr()
            .matches("holds keys this hipc does not know")
            .count(),
        1,
        "{}",
        run.stderr()
    );
    assert!(
        !run.stderr().contains("s3cr3t") && !run.stderr().contains("surprise"),
        "{}",
        run.stderr()
    );
}

/// A result with no place for a `warnings` field, the array `--all` prints or a body printed
/// as the server sent it, keeps its shape, and JSON mode prints the warnings beside it on
/// stderr as one `{"warnings": [...]}` object.
#[test]
fn a_result_with_no_place_for_warnings_keeps_its_shape_and_warns_on_stderr() {
    let home = Home::new();
    home.write_config("[profiles.default]\nsurprise = \"s3cr3t-value\"\n");
    let server = Server::start(vec![route(
        "GET",
        "/api/ssh-keys/",
        Reply::json(200, r#"{"next":null,"results":[{"id":1}]}"#),
    )]);
    for (args, stdout) in [
        (&["api", "GET", "/ssh-keys/", "--all"][..], "[{\"id\":1}]\n"),
        (
            &["api", "GET", "/ssh-keys/"],
            r#"{"next":null,"results":[{"id":1}]}"#,
        ),
    ] {
        let run = Hipc::against(&home, &server).run(args);
        assert_eq!(run.code(), Some(0), "{}", run.stderr());
        assert_eq!(run.stdout(), stdout, "{args:?}");
        let warned: serde_json::Value = serde_json::from_str(run.stderr()).unwrap();
        let texts = warned["warnings"].as_array().unwrap();
        assert_eq!(texts.len(), 1, "{warned}");
        assert!(
            texts[0]
                .as_str()
                .unwrap()
                .contains("holds keys this hipc does not know"),
            "{warned}"
        );
        assert!(!run.stderr().contains("s3cr3t"), "{}", run.stderr());
    }
}

#[test]
fn without_home_the_config_location_is_usage() {
    Hipc::new(&Home::new())
        .env_remove("HOME")
        .run(&["account", "use", "self"])
        .fails(2, "usage");
}

/// An origin whose host holds a character no host has is refused before anything else, a
/// dry run included, so the dry run does not accept what the real run refuses, and a
/// login's question never quotes a broken literal.
#[test]
fn an_origin_with_a_character_no_host_has_is_usage_even_in_a_dry_run() {
    let home = Home::new();
    let error = Hipc::new(&home)
        .env("HIPPIUS_TOKEN", TOKEN)
        .run(&[
            "--api-url",
            "https://a\"b<c>{d}",
            "--dry-run",
            "compute",
            "vms",
            "list",
        ])
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        "`https://a\"b<c>{d}` from `--api-url` is not an API origin: its host holds a \
         character other than a letter, a digit, `-`, or `.`"
    );
    let token_file = home.file("token", TOKEN.as_bytes());
    let error = Hipc::new(&home)
        .env(
            "HIPPIUS_API_URL",
            "https://evil.example`,_named_by_the_default_`x",
        )
        .run(&[
            "auth",
            "login",
            "--token-file",
            token_file.to_str().unwrap(),
        ])
        .fails(2, "usage");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with(r"`https://evil.example\u{60},_named_by_the_default_\u{60}x` from"),
        "{error}"
    );
    assert_eq!(home.config_text(), "", "nothing was stored");
}

/// `https://` has a scheme and no host; the trailing slashes trimmed from an origin are
/// those after its authority, so the refusal says so.
#[test]
fn an_origin_with_a_scheme_and_no_host_says_it_has_no_host() {
    for origin in ["https://", "https:///", "https://api.hippius.com//"] {
        let run = Hipc::new(&Home::new())
            .env("HIPPIUS_API_URL", origin)
            .env("HIPPIUS_TOKEN", TOKEN)
            .run(&["--dry-run", "compute", "vms", "list"]);
        if origin.contains("api.") {
            // Every slash after the host is trimmed, as before.
            assert_eq!(run.code(), Some(0), "{}", run.stderr());
            continue;
        }
        let error = run.fails(2, "usage");
        assert_eq!(
            error["message"],
            "`https://` from `HIPPIUS_API_URL` is not an API origin: it has no host"
        );
    }
}

/// A profile name is 1 to 64 letters, digits, `.`, `_`, and `-`, so a name with a line
/// break, a path, or nothing at all is refused before it reaches the file.
#[test]
fn a_profile_name_outside_the_name_rules_is_usage_and_writes_nothing() {
    let home = Home::new();
    let long = "p".repeat(65);
    for bad in ["", "a b", "../x", "x\ny", "\"q\"", "é", long.as_str()] {
        let error = Hipc::new(&home)
            .run(&["--profile", bad, "account", "use", "self"])
            .fails(2, "usage");
        // The name is not repeated: it may be a secret given in its place.
        assert_eq!(
            error["message"],
            "the profile name from `--profile` breaks the name rules: a name is 1 to 64 \
             letters, digits, `.`, `_`, or `-`; it is not repeated here, in case it is a secret",
            "{bad:?}"
        );
        assert_eq!(error["hint"], "pass a `--profile` such as `default`");
    }
    let error = Hipc::new(&home)
        .env("HIPPIUS_PROFILE", "a b")
        .run(&["account", "use", "self"])
        .fails(2, "usage");
    assert!(
        error["message"].as_str().is_some_and(
            |message| message.starts_with("the profile name from `HIPPIUS_PROFILE` breaks")
        ),
        "{error}"
    );
    assert_eq!(
        error["hint"],
        "unset `HIPPIUS_PROFILE`, or set it to a profile name such as `default`"
    );
    assert_eq!(home.config_text(), "", "nothing was written");
    let at_limit = "p".repeat(64);
    for good in ["work.2_a-B", at_limit.as_str()] {
        Hipc::new(&home)
            .run(&["--profile", good, "account", "use", "self"])
            .ok();
    }
}

/// A profile an older `hip` stored under a name outside today's rules stays usable: a name
/// the file already has, as a profile or as `default_profile`, is taken as it is.
#[test]
fn a_stored_profile_with_an_old_style_name_can_still_be_selected() {
    let server = Server::start(status_routes());
    let home = Home::new();
    let long = "p".repeat(65);
    let names = ["my work", "travail é", long.as_str()];
    let mut config = String::new();
    for name in names {
        config.push_str(&format!(
            "[profiles.\"{name}\"]\ntoken = \"{TOKEN}\"\ntoken_origin = \"{}\"\n",
            server.url()
        ));
    }
    home.write_config(&format!("default_profile = \"new one\"\n{config}"));
    for name in names {
        let by_variable = Hipc::tokenless(&home, &server)
            .env("HIPPIUS_PROFILE", name)
            .run(&["auth", "status"])
            .ok();
        assert_eq!(by_variable["profile"], name);
        let by_flag = Hipc::tokenless(&home, &server)
            .run(&["--profile", name, "auth", "status"])
            .ok();
        assert_eq!(by_flag["profile"], name);
    }
    // `default_profile` names a profile not written yet; it is the file's own choice.
    let doc = Hipc::against(&home, &server).run(&["auth", "status"]).ok();
    assert_eq!(doc["profile"], "new one");
    let doc = Hipc::against(&home, &server)
        .run(&["--profile", "new one", "auth", "status"])
        .ok();
    assert_eq!(doc["profile"], "new one");
    // A new name outside the rules is still refused.
    Hipc::new(&home)
        .run(&["--profile", "other one", "auth", "status"])
        .fails(2, "usage");
}

/// A profile the overrides name that the file does not have is named in the refusal, so a
/// misspelled `--profile` does not read as a missing token.
#[test]
fn a_profile_the_file_does_not_have_is_named_when_no_token_is_found() {
    let home = Home::new();
    login(&home);
    let path = home.config();
    let error = Hipc::new(&home)
        .run(&["--profile", "nope", "compute", "vms", "list"])
        .fails(1, "auth");
    assert_eq!(
        error["message"],
        format!(
            "no API token is configured for `{}`: the config file `{}` has no profile `nope`",
            no_server(),
            path.display()
        )
    );
    assert_eq!(
        error["hint"],
        format!(
            "pass `--profile` the name of a profile in `{}`",
            path.display()
        )
    );
    let error = Hipc::new(&home)
        .env("HIPPIUS_PROFILE", "nope")
        .run(&["compute", "vms", "list"])
        .fails(1, "auth");
    assert_eq!(
        error["hint"],
        format!(
            "set `HIPPIUS_PROFILE` to the name of a profile in `{}`",
            path.display()
        )
    );
    // The default profile, or a new name with no config file or no profiles in it, keeps
    // the plain refusal, whose fix is a login.
    // The message names the file it read, or the profile in it, and the hint says who can
    // sign in and where each kind of account gets its token.
    let no_file = Home::new();
    let with_file = Home::new();
    let work_token = with_file.file("token", b"work-token-wxyz");
    Hipc::without_api_url(&with_file)
        .run(&[
            "--profile",
            "work",
            "auth",
            "login",
            "--token-file",
            work_token.to_str().unwrap(),
        ])
        .ok();
    for (home, args, found, login) in [
        (
            &no_file,
            &["compute", "vms", "list"][..],
            format!(
                "the config file `{}` does not exist",
                no_file.config().display()
            ),
            "`hipc auth login`",
        ),
        (
            &no_file,
            &["--profile", "work", "compute", "vms", "list"][..],
            format!(
                "the config file `{}` does not exist",
                no_file.config().display()
            ),
            "`hipc auth login --profile work`",
        ),
        (
            &with_file,
            &["compute", "vms", "list"][..],
            format!(
                "the profile `default` in `{}` holds no token",
                with_file.config().display()
            ),
            "`hipc auth login`",
        ),
    ] {
        let error = Hipc::new(home).run(args).fails(1, "auth");
        assert_eq!(
            error["message"],
            format!("no API token is configured for `{}`: {found}", no_server())
        );
        let hint = error["hint"].as_str().unwrap();
        assert_eq!(
            hint,
            format!(
                "ask the user to run {login} in their own terminal, with a token created at \
                 `https://console.hippius.com/dashboard/settings`"
            )
        );
    }
}

#[test]
fn hippius_config_points_at_another_file_and_the_profile_flag_selects() {
    let server = Server::start(status_routes());
    let home = Home::new();
    let custom = home.path().join("alt").join("c.toml");
    let token_file = home.file("token", b"work-token-wxyz");
    Hipc::new(&home)
        .env("HIPPIUS_CONFIG", custom.to_str().unwrap())
        .env("HIPPIUS_API_URL", server.url())
        .run(&[
            "auth",
            "login",
            "--yes",
            "--token-file",
            token_file.to_str().unwrap(),
            "--profile",
            "work",
        ])
        .ok();
    let doc = Hipc::new(&home)
        .env("HIPPIUS_CONFIG", custom.to_str().unwrap())
        .env("HIPPIUS_API_URL", server.url())
        .run(&["auth", "status", "--profile", "work"])
        .ok();
    assert_eq!(doc["profile"], "work");
    assert_eq!(doc["token_preview"], "wxyz");
    let error = Hipc::new(&home)
        .env("HIPPIUS_CONFIG", custom.to_str().unwrap())
        .env("HIPPIUS_API_URL", server.url())
        .run(&["auth", "status"])
        .fails(1, "auth");
    // The refusal names the file the variable chose, and the profile it read there.
    assert_eq!(
        error["message"],
        format!(
            "no API token is configured for `{}`: the profile `default` in `{}` holds no token",
            server.url(),
            custom.display()
        )
    );
}

#[test]
fn a_type_error_in_the_config_never_echoes_the_value() {
    let home = Home::new();
    std::fs::create_dir_all(home.config().parent().expect("a parent")).expect("mkdir");
    chmod(home.config().parent().expect("a parent"), 0o700);
    // A token where a table belongs: toml's own message would quote it.
    std::fs::write(
        home.config(),
        format!("[profiles]\ndefault = \"{TOKEN}\"\n"),
    )
    .expect("write");
    chmod(&home.config(), 0o600);
    let error = Hipc::new(&home)
        .run(&["account", "use", "self"])
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        format!(
            "the config file `{}` is not valid: line 2 has a value that is not a table",
            home.config().display()
        )
    );
}

#[test]
fn concurrent_logins_to_different_profiles_keep_both_tokens() {
    let home = Home::new();
    let token_file = home.file("token", TOKEN.as_bytes());
    let path = token_file.to_str().expect("a UTF-8 path").to_owned();
    let profiles: Vec<String> = (0..8).map(|i| format!("p{i}")).collect();
    std::thread::scope(|scope| {
        for profile in &profiles {
            let (home, path) = (&home, &path);
            scope.spawn(move || {
                Hipc::without_api_url(home)
                    .run(&["auth", "login", "--token-file", path, "--profile", profile])
                    .ok();
            });
        }
    });
    let text = home.config_text();
    for profile in &profiles {
        assert!(
            text.contains(&format!("[profiles.{profile}]")),
            "{profile} lost: {text}"
        );
    }
}

#[test]
fn a_whitespace_hippius_token_is_usage() {
    let server = Server::start(vec![]);
    Hipc::tokenless(&Home::new(), &server)
        .with_env_token(server.url(), "   ")
        .run(&["auth", "status"])
        .fails(2, "usage");
    assert!(server.requests().is_empty());
}

#[test]
fn a_variable_that_is_not_utf8_is_usage_not_unset() {
    let server = Server::start(vec![]);
    for name in [
        "HIPPIUS_ACCOUNT",
        "HIPPIUS_TOKEN",
        "HIPPIUS_PROFILE",
        "HIPPIUS_API_URL",
    ] {
        let mut hipc = Hipc::against(&Home::new(), &server).env_bytes(name, b"acct_\xff");
        if name == "HIPPIUS_API_URL" {
            hipc = hipc.env("HIPPIUS_TOKEN", "tok_0123456789abcdefghij");
        }
        let error = hipc.run(&["account", "get"]).fails(2, "usage");
        assert!(error["message"].as_str().unwrap().contains(name), "{error}");
    }
    assert!(server.requests().is_empty());
}

/// Invocations whose argv or missing `--reveal` refuses them, with the code and a piece of
/// the message, for [`argv_and_reveal_come_before_the_config_file`].
const REFUSED_BEFORE_THE_CONFIG: &[(&[&str], &str, &str)] = &[
    (
        &["s3", "buckets", "create", "AB"],
        "usage",
        "a bucket name is",
    ),
    (
        &["s3", "buckets", "delete", "AB"],
        "usage",
        "a bucket name is",
    ),
    (
        &[
            "s3", "tokens", "create", "--kind", "master", "--bucket", "b",
        ],
        "usage",
        "are for sub tokens",
    ),
    (
        &["s3", "tokens", "create", "--kind", "master"],
        "reveal_required",
        "",
    ),
    (
        &["s3", "tokens", "revoke", "--kind", "sub", "x"],
        "usage",
        "a token id is a UUID",
    ),
    (
        &["s3", "tokens", "rotate", "--kind", "sub", "x", "--reveal"],
        "usage",
        "a token id is a UUID",
    ),
    (
        &["s3", "tokens", "rotate", "--kind", "sub", KEY],
        "reveal_required",
        "",
    ),
    (
        &["hub", "namespace", "availability", "A"],
        "usage",
        "a namespace name is",
    ),
    (
        &["hub", "namespace", "create", "A", "--reveal"],
        "usage",
        "a namespace name is",
    ),
    (&["hub", "namespace", "create", "ok"], "reveal_required", ""),
    (
        &[
            "hub",
            "subscription",
            "subscribe",
            "--plan",
            "1",
            "--pay-from",
            "card",
        ],
        "usage",
        "`--pay-from`",
    ),
    (
        &[
            "account", "members", "update", "7", "--grant", "none", "--grant", "none",
        ],
        "usage",
        "cannot be combined",
    ),
    (
        &["account", "members", "update", "7"],
        "usage",
        "nothing to change",
    ),
    (
        &[
            "account",
            "invites",
            "create",
            "--email",
            "a@example.com",
            "--grant",
            "vms=viewer",
            "--grant",
            "vms=admin",
        ],
        "usage",
        "more than once",
    ),
    (
        &[
            "compute",
            "firewall",
            "update",
            KEY,
            "--rules-file",
            MISSING,
        ],
        "usage",
        "cannot read the firewall rule list",
    ),
    (
        &[
            "compute",
            "exposures",
            "create",
            KEY,
            "--port",
            "53",
            "--protocol",
            "udp",
        ],
        "usage",
        "needs `--mode raw`",
    ),
    (
        &[
            "compute",
            "ssh-keys",
            "create",
            "--name",
            "k",
            "--public-key-file",
            MISSING,
        ],
        "usage",
        "cannot read the public key",
    ),
    (
        &[
            "compute",
            "databases",
            "create",
            "--tier",
            "starter",
            "--size",
            "s",
            "--engine-version",
            "18",
        ],
        "reveal_required",
        "",
    ),
    (
        &["compute", "databases", "restore", "7"],
        "reveal_required",
        "",
    ),
    (
        &["compute", "runners", "settings", "update"],
        "usage",
        "--max-concurrent",
    ),
    (
        &[
            "compute",
            "runners",
            "settings",
            "update",
            "--allowed-size",
            "small",
            "--allowed-size",
            "small",
        ],
        "usage",
        "more than once",
    ),
    (
        &["cdn", "estimate", "--traffic", "FR=1", "--traffic", "FR=2"],
        "usage",
        "more than once",
    ),
    (
        &["cdn", "zones", "update", "zab12cd34ef56"],
        "usage",
        "nothing to change",
    ),
    (
        &[
            "cdn",
            "rules",
            "update",
            "zab12cd34ef56",
            "--rules-file",
            MISSING,
        ],
        "usage",
        "cannot read the cache rules",
    ),
];

/// An S3 key id, which the token id parser accepts, and a VM id.
const KEY: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

/// A file that does not exist.
const MISSING: &str = "/nonexistent/hipc-test-file";

/// Argv and `--reveal` are checked before the config file is read (tenet 5): with a config
/// that does not parse, each of these fails on its own invocation, not on the file.
#[test]
fn argv_and_reveal_come_before_the_config_file() {
    let home = Home::new();
    home.write_config("[profiles\n");
    for (args, code, says) in REFUSED_BEFORE_THE_CONFIG {
        let error = Hipc::new(&home).run(args).fails(2, code);
        let message = error["message"].as_str().expect("a message");
        assert!(message.contains(says), "{args:?}: {message}");
        assert!(!message.contains("config file"), "{args:?}: {message}");
    }
    let empty = home.file("empty", b"");
    for verb in ["preview", "accept"] {
        let args = ["account", "invites", verb, "--token-file"];
        let args = [&args[..], &[empty.to_str().expect("a UTF-8 path")]].concat();
        let error = Hipc::new(&home).run(&args).fails(2, "usage");
        assert!(
            error["message"]
                .as_str()
                .expect("a message")
                .ends_with("is empty"),
            "{error}"
        );
    }
}
