//! `hipc compute app-engine`: the reads, and the compose validate and fix. The compose goes
//! byte for byte as the JSON string `compose`, a file that is not UTF-8 or is too large is
//! refused before anything is sent, and an answer's `compose_sha256` must be the SHA-256 of the
//! bytes it hashed.

use std::os::unix::fs::PermissionsExt as _;

use serde_json::{Value, json};

use crate::common::{Hipc, Home, MEMBER, Reply, Server, draft, route};

const PROJECT: &str = "proj_0a1b2c3d4e5f60718293";
const BILLING: &str = "bill_9f8e7d6c5b4a39281706";

const INFO: &str = "/api/compute/app-engine/info/";
const VALIDATE: &str = "/api/compute/app-engine/validate/";
const FIX: &str = "/api/compute/app-engine/fix/";
const APPS: &str = "/api/compute/app-engine/apps/";
const APP: &str = "/api/compute/app-engine/apps/vm-1a2b/";

const COMPOSE: &str = "services:\n  web:\n    image: nginx\n";
/// `sha256sum` of [`COMPOSE`].
const COMPOSE_SHA256: &str = "5f6fad3dce5125e4e5595e34672d6d2bbe6149ab65095b42d9f77d66e3f8202c";
const FIXED: &str = "x-hippius:\n  secrets: []\nservices:\n  web:\n    image: nginx@sha256:00\n";
/// `sha256sum` of [`FIXED`].
const FIXED_SHA256: &str = "29231ebeed6e968cf76a4fa1f76c1f53ce69a9c847bacd11710e80402c8ad52b";

fn hipc(home: &Home, server: &Server, args: &[&str]) -> crate::common::Run {
    Hipc::against(home, server).run(args)
}

fn text(home: &Home, server: &Server, args: &[&str]) -> crate::common::Run {
    let mut all = vec!["--output", "text"];
    all.extend(args);
    Hipc::against(home, server).run(&all)
}

fn validated(sha256: &str) -> String {
    json!({
        "grammar_version": 1,
        "compose_sha256": sha256,
        "services": [{"name": "web", "image": "nginx"}],
        "secret_names": [],
        "issues": [],
    })
    .to_string()
}

fn fixed(sha256: &str, extra: &Value) -> String {
    let mut answer = json!({
        "fixed_compose": FIXED,
        "compose_sha256": sha256,
        "valid": true,
        "changes": [],
        "unresolved": [],
        "images": [],
        "issues": [],
    });
    if let (Value::Object(answer), Value::Object(extra)) = (&mut answer, extra) {
        answer.extend(extra.clone());
    }
    answer.to_string()
}

#[test]
fn get_reads_the_limits_and_says_when_the_account_may_not_use_it() {
    let home = Home::new();
    let on = r#"{"enabled":true,"grammar_version":1,"max_compose_bytes":65536}"#;
    let server = Server::start(vec![route("GET", INFO, Reply::json(200, on))]);
    let doc = hipc(&home, &server, &["compute", "app-engine", "get"]).ok();
    assert_eq!(doc["max_compose_bytes"], 65536);
    let run = text(&home, &server, &["compute", "app-engine", "get"]);
    assert_eq!(run.code(), Some(0));
    assert_eq!(run.stderr(), "");

    for (body, warning) in [
        (
            r#"{"enabled":false,"reason":"not-available"}"#,
            "warning: the account may not use App Engine: the API says `not-available`\n",
        ),
        (
            r#"{"enabled":false}"#,
            "warning: the account may not use App Engine\n",
        ),
    ] {
        let server = Server::start(vec![route("GET", INFO, Reply::json(200, body))]);
        // `enabled: false` is an answer, not a failure: exit 0, and JSON mode stays quiet.
        assert_eq!(
            hipc(&home, &server, &["compute", "app-engine", "get"]).ok()["enabled"],
            false
        );
        let run = text(&home, &server, &["compute", "app-engine", "get"]);
        assert_eq!(run.code(), Some(0));
        assert_eq!(run.stderr(), warning);
    }
}

/// The list route answers a bare array, printed as the list envelope (unpaged).
#[test]
fn apps_list_is_an_envelope() {
    let home = Home::new();
    let server = Server::start(vec![route(
        "GET",
        APPS,
        Reply::json(200, r#"[{"vm_id":"vm-1a2b","status":"running"}]"#),
    )]);
    let doc = hipc(&home, &server, &["compute", "app-engine", "apps", "list"]).ok();
    assert_eq!(
        doc,
        json!({"items": [{"vm_id": "vm-1a2b", "status": "running"}], "next_page_token": null})
    );
    let server = Server::start(vec![route("GET", APPS, Reply::json(200, "[]"))]);
    let doc = hipc(&home, &server, &["compute", "app-engine", "apps", "list"]).ok();
    assert_eq!(doc, json!({"items": [], "next_page_token": null}));
}

/// Every route is under `/api/compute/`, so each command sends the membership's account and
/// the project (tenet 12), the two POSTs included.
#[test]
fn every_command_sends_the_account_and_the_project() {
    let home = Home::new();
    let compose = home.file("compose.yaml", COMPOSE.as_bytes());
    let compose = compose.to_str().unwrap();
    let server = Server::start(vec![
        route("GET", INFO, Reply::json(200, r#"{"enabled":true}"#)),
        route(
            "POST",
            VALIDATE,
            Reply::json(200, &validated(COMPOSE_SHA256)),
        ),
        route(
            "POST",
            FIX,
            Reply::json(200, &fixed(FIXED_SHA256, &json!({}))),
        ),
        route("GET", APPS, Reply::json(200, "[]")),
        route("GET", APP, Reply::json(200, "{}")),
        route("GET", &format!("{APP}runtime/"), Reply::json(200, "{}")),
        route(
            "GET",
            &format!("{APP}guest-components/"),
            Reply::json(200, "{}"),
        ),
        route(
            "GET",
            &format!("{APP}guest-upgrade/"),
            Reply::json(200, r#"{"job":null}"#),
        ),
    ]);
    let commands: [&[&str]; 8] = [
        &["get"],
        &["validate", "--compose-file", compose],
        &["fix", "--compose-file", compose],
        &["apps", "list"],
        &["apps", "get", "vm-1a2b"],
        &["apps", "runtime", "vm-1a2b"],
        &["guest-components", "get", "vm-1a2b"],
        &["guest-components", "update-status", "vm-1a2b"],
    ];
    for words in commands {
        let args = [&["--project", PROJECT, "compute", "app-engine"][..], words].concat();
        Hipc::against(&home, &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&args)
            .ok();
    }
    let requests = server.requests();
    assert_eq!(requests.len(), 8);
    for request in &requests {
        assert_eq!(
            request.header("x-hippius-account"),
            Some(MEMBER),
            "{}",
            request.target
        );
        assert_eq!(
            request.header("x-hippius-project"),
            Some(PROJECT),
            "{}",
            request.target
        );
        assert_eq!(
            request.header("x-hippius-billing"),
            None,
            "{}",
            request.target
        );
    }
}

#[test]
fn a_billing_selection_is_refused_and_a_stored_one_is_skipped() {
    let home = Home::new();
    let compose = home.file("compose.yaml", COMPOSE.as_bytes());
    let compose = compose.to_str().unwrap();
    let server = Server::start(vec![
        route("GET", APPS, Reply::json(200, "[]")),
        route(
            "POST",
            VALIDATE,
            Reply::json(200, &validated(COMPOSE_SHA256)),
        ),
    ]);
    for args in [
        &["compute", "app-engine", "apps", "list"][..],
        &[
            "compute",
            "app-engine",
            "validate",
            "--compose-file",
            compose,
        ],
    ] {
        let mut all = vec!["--billing", BILLING];
        all.extend(args);
        hipc(&home, &server, &all).fails(2, "billing_scope_unsupported");
    }
    assert!(server.requests().is_empty());

    // Validate is a POST that changes nothing, so a stored billing account is skipped as on
    // a read, not refused as on a write. The test server rebuilds each request it gets from
    // its method alone, which would read the POST as a write, so the dry run shows it.
    Hipc::new(&home)
        .run(&["account", "billing-accounts", "use", BILLING])
        .ok();
    let doc = hipc(
        &home,
        &server,
        &[
            "--dry-run",
            "compute",
            "app-engine",
            "validate",
            "--compose-file",
            compose,
        ],
    )
    .ok();
    assert_eq!(draft(&doc)["billing_header"], Value::Null, "{doc}");
    hipc(&home, &server, &["compute", "app-engine", "apps", "list"]).ok();
    assert_eq!(server.only_request().header("x-hippius-billing"), None);
}

#[test]
fn the_app_reads_print_the_answer_with_their_text_mode_asides() {
    let home = Home::new();
    let server = Server::start(vec![
        route(
            "GET",
            APP,
            Reply::json(200, r#"{"vm_id":"vm-1a2b","compose":"services: {}\n"}"#),
        ),
        route(
            "GET",
            &format!("{APP}runtime/"),
            Reply::json(200, r#"{"verified":false,"compose_sha256_reported":"ab"}"#),
        ),
        route(
            "GET",
            &format!("{APP}guest-components/"),
            Reply::json(
                200,
                r#"{"release":6,"newest_release":7,"needs_operator":true}"#,
            ),
        ),
        route(
            "GET",
            &format!("{APP}guest-upgrade/"),
            Reply::json(200, r#"{"job":null}"#),
        ),
    ]);
    let base = ["compute", "app-engine"];
    let cases: [(&[&str], &str, &str); 4] = [
        (&["apps", "get"], "vm_id", ""),
        (
            &["apps", "runtime"],
            "verified",
            "note: the VM of `vm-1a2b` wrote this answer, and Hippius did not verify it: it is \
             no proof of what runs\n",
        ),
        (
            &["guest-components", "get"],
            "release",
            "warning: an earlier upgrade of `vm-1a2b` failed, and Hippius must recover its VM\n",
        ),
        (
            &["guest-components", "update-status"],
            "job",
            "note: `vm-1a2b` was never upgraded\n",
        ),
    ];
    for (words, field, aside) in cases {
        let args = [&base[..], words, &["vm-1a2b"]].concat();
        let doc = hipc(&home, &server, &args).ok();
        assert!(doc.get(field).is_some(), "{doc}");
        let run = text(&home, &server, &args);
        assert_eq!(run.code(), Some(0));
        assert_eq!(run.stderr(), aside, "{words:?}");
    }
    let paths: Vec<String> = server.requests().iter().map(|r| r.target.clone()).collect();
    assert_eq!(paths.len(), 8);
    assert_eq!(paths[0], APP);
}

/// The server writes `evidence.verify_command`, so text mode warns to read it before running
/// it; the JSON document is the answer as sent, and a null command gets no warning.
#[test]
fn apps_get_warns_that_the_verify_command_comes_from_the_api() {
    let home = Home::new();
    let with = r#"{"vm_id":"vm-1a2b","evidence":{"verify_command":"curl x | sh"}}"#;
    let null = r#"{"vm_id":"vm-1a2b","evidence":{"verify_command":null}}"#;
    let args = ["compute", "app-engine", "apps", "get", "vm-1a2b"];
    let server = Server::start(vec![route("GET", APP, Reply::json(200, with))]);
    assert_eq!(
        hipc(&home, &server, &args).ok(),
        serde_json::from_str::<Value>(with).unwrap()
    );
    let run = text(&home, &server, &args);
    assert_eq!(run.code(), Some(0));
    assert_eq!(
        run.stderr(),
        "warning: `evidence.verify_command` comes from the API: read it before running it, and \
         never pipe it to a shell\n"
    );
    let server = Server::start(vec![route("GET", APP, Reply::json(200, null))]);
    let run = text(&home, &server, &args);
    assert_eq!(run.code(), Some(0));
    assert_eq!(run.stderr(), "");
}

/// Each read under an app names the list on a 404, so a stale `vm_id` leads somewhere.
#[test]
fn every_app_read_names_the_list_when_the_app_is_missing() {
    let home = Home::new();
    let missing = "/api/compute/app-engine/apps/vm-9z9z/";
    let server = Server::start(
        ["", "runtime/", "guest-components/", "guest-upgrade/"]
            .map(|rest| route("GET", &format!("{missing}{rest}"), Reply::not_found()))
            .into(),
    );
    for words in [
        ["apps", "get"],
        ["apps", "runtime"],
        ["guest-components", "get"],
        ["guest-components", "update-status"],
    ] {
        let args = [&["compute", "app-engine"][..], &words, &["vm-9z9z"]].concat();
        let error = hipc(&home, &server, &args).fails(1, "not_found");
        assert_eq!(
            error["hint"], "run `hipc compute app-engine apps list` to see what exists",
            "{words:?}"
        );
    }
    let targets: Vec<String> = server.requests().iter().map(|r| r.target.clone()).collect();
    assert_eq!(
        targets,
        [
            "/api/compute/app-engine/apps/vm-9z9z/",
            "/api/compute/app-engine/apps/vm-9z9z/runtime/",
            "/api/compute/app-engine/apps/vm-9z9z/guest-components/",
            "/api/compute/app-engine/apps/vm-9z9z/guest-upgrade/",
        ]
    );
}

/// The asides need the values that call for them: an operator that is not needed and a job
/// that exists say nothing.
#[test]
fn a_healthy_guest_and_an_upgrade_job_say_nothing() {
    let home = Home::new();
    let server = Server::start(vec![
        route(
            "GET",
            &format!("{APP}guest-components/"),
            Reply::json(200, r#"{"release":7,"needs_operator":false}"#),
        ),
        route(
            "GET",
            &format!("{APP}guest-upgrade/"),
            Reply::json(200, r#"{"job":{"job_id":"j1","state":"done"}}"#),
        ),
    ]);
    for leaf in ["get", "update-status"] {
        let args = ["compute", "app-engine", "guest-components", leaf, "vm-1a2b"];
        let run = text(&home, &server, &args);
        assert_eq!(run.code(), Some(0));
        assert_eq!(run.stderr(), "", "{leaf}");
    }
}

#[test]
fn a_missing_app_names_the_list_and_an_unavailable_engine_names_get() {
    let home = Home::new();
    let unavailable = r#"{"error":"app-engine-unavailable","detail":"App Engine is not enabled"}"#;
    let runtime = format!("{APP}runtime/");
    let server = Server::start(vec![
        route("GET", APP, Reply::not_found()),
        route("GET", APPS, Reply::json(403, unavailable)),
        route("GET", &runtime, Reply::json(404, unavailable)),
    ]);
    let error = hipc(
        &home,
        &server,
        &["compute", "app-engine", "apps", "get", "vm-1a2b"],
    )
    .fails(1, "not_found");
    assert_eq!(
        error["hint"],
        "run `hipc compute app-engine apps list` to see what exists"
    );
    let error =
        hipc(&home, &server, &["compute", "app-engine", "apps", "list"]).fails(1, "permission");
    assert_eq!(
        error["hint"],
        "run `hipc compute app-engine get` to see whether the account may use it"
    );
    assert_eq!(error["details"]["error"], "app-engine-unavailable");
    // The engine's code on another status is not its gate, so no hint names `get`.
    let runtime_args = ["compute", "app-engine", "apps", "runtime", "vm-1a2b"];
    let error = hipc(&home, &server, &runtime_args).fails(1, "not_found");
    assert_eq!(error["details"]["error"], "app-engine-unavailable");
    assert!(
        !error["hint"].to_string().contains("app-engine get"),
        "{error}"
    );
    // Another code on a 403 is not the engine's gate either.
    let server = Server::start(vec![route(
        "GET",
        &runtime,
        Reply::json(403, r#"{"error":"forbidden"}"#),
    )]);
    let error = hipc(&home, &server, &runtime_args).fails(1, "permission");
    assert_eq!(error["details"]["error"], "forbidden");
    assert!(
        !error["hint"].to_string().contains("app-engine get"),
        "{error}"
    );
    // `get` is the command the hint names, so its own 403 does not send the caller to it.
    let server = Server::start(vec![route("GET", INFO, Reply::json(403, unavailable))]);
    let error = hipc(&home, &server, &["compute", "app-engine", "get"]).fails(1, "permission");
    assert_eq!(error["details"]["error"], "app-engine-unavailable");
    assert!(error["hint"].is_null(), "{error}");
}

#[test]
fn a_vm_id_that_is_not_an_id_is_refused_before_anything_is_sent() {
    let home = Home::new();
    let server = Server::start(vec![]);
    let error = hipc(
        &home,
        &server,
        &["compute", "app-engine", "apps", "get", "vm/1"],
    )
    .fails(2, "usage");
    assert!(
        error["message"].as_str().unwrap().contains("a VM id is"),
        "{error}"
    );
}

/// A compose with a byte-order mark and carriage returns.
const MARKED: &str = "\u{feff}services:\r\n  web: {image: nginx}\r\n";
/// `sha256sum` of [`MARKED`].
const MARKED_SHA256: &str = "498d7d5283665131b39a9ea4b9a991adbe3b41560aef9ced56b2d22d8fb4e6af";

/// The compose goes as the JSON string `compose`, byte for byte: a byte-order mark and a
/// carriage return included, which text mode warns about. No `secrets`.
#[test]
fn validate_sends_the_exact_bytes_and_nothing_else() {
    let home = Home::new();
    let raw = MARKED;
    let compose = home.file("compose.yaml", raw.as_bytes());
    let compose = compose.to_str().unwrap();
    let server = Server::start(vec![route(
        "POST",
        VALIDATE,
        Reply::json(
            200,
            &json!({"compose_sha256": MARKED_SHA256, "services": [], "issues": []}).to_string(),
        ),
    )]);
    let args = [
        "compute",
        "app-engine",
        "validate",
        "--compose-file",
        compose,
    ];
    let doc = hipc(&home, &server, &args).ok();
    assert_eq!(doc["issues"], json!([]));
    let request = server.only_request();
    assert_eq!(request.json(), json!({ "compose": raw }));
    assert_eq!(request.header("content-type"), Some("application/json"));

    let run = text(&home, &server, &args);
    assert_eq!(run.code(), Some(0));
    assert_eq!(run.stderr(), marked_warnings(compose));
}

fn marked_warnings(compose: &str) -> String {
    format!(
        "warning: the compose file `{compose}` starts with a byte-order mark, which hipc sends \
         as it is\nwarning: the compose file `{compose}` holds a carriage return, which hipc \
         sends as it is\n"
    )
}

/// The warning comes before the request, so it stands beside the server's refusal of the
/// bytes it names, for a fix as for a validate, and a fix that is answered warns too.
#[test]
fn a_marked_compose_warns_before_it_is_sent() {
    let home = Home::new();
    let compose = home.file("compose.yaml", MARKED.as_bytes());
    let compose = compose.to_str().unwrap();
    let refused = r#"{"error":"app-compose-invalid","issues":[{"rule":"bom","message":"a BOM"}]}"#;
    let server = Server::start(vec![
        route("POST", VALIDATE, Reply::json(400, refused)),
        route(
            "POST",
            FIX,
            Reply::json(400, r#"{"error":"app-fix-unparseable"}"#),
        ),
    ]);
    for (leaf, refusal) in [
        (
            "validate",
            "error: cannot validate the compose file `{compose}`: the API says \
             `app-compose-invalid`\nerror: `bom`: the API says `a BOM`\n",
        ),
        (
            "fix",
            "error: cannot fix the compose file `{compose}`: the API says \
             `app-fix-unparseable`\n",
        ),
    ] {
        let run = text(
            &home,
            &server,
            &["compute", "app-engine", leaf, "--compose-file", compose],
        );
        assert_eq!(run.code(), Some(1), "{leaf}");
        assert_eq!(
            run.stderr(),
            format!(
                "{}{}",
                marked_warnings(compose),
                refusal.replace("{compose}", compose)
            ),
            "{leaf}"
        );
    }
    let server = Server::start(vec![route(
        "POST",
        FIX,
        Reply::json(200, &fixed(FIXED_SHA256, &json!({}))),
    )]);
    let run = text(
        &home,
        &server,
        &["compute", "app-engine", "fix", "--compose-file", compose],
    );
    assert_eq!(run.code(), Some(0));
    assert_eq!(run.stderr(), marked_warnings(compose));
    // JSON mode has no warnings.
    let run = hipc(
        &home,
        &server,
        &["compute", "app-engine", "fix", "--compose-file", compose],
    );
    assert_eq!(run.code(), Some(0));
    assert_eq!(run.stderr(), "");
}

#[test]
fn validate_reads_stdin_for_a_dash_and_checks_the_hash() {
    let home = Home::new();
    let server = Server::start(vec![route(
        "POST",
        VALIDATE,
        Reply::json(200, &validated(&COMPOSE_SHA256.to_uppercase())),
    )]);
    let doc = Hipc::against(&home, &server)
        .stdin(COMPOSE.as_bytes())
        .run(&["compute", "app-engine", "validate", "--compose-file", "-"])
        .ok();
    assert_eq!(doc["services"][0]["name"], "web");
    assert_eq!(server.only_request().json()["compose"], COMPOSE);
    // The hash is of the bytes read from stdin: other bytes there do not match the answer.
    let error = Hipc::against(&home, &server)
        .stdin(FIXED.as_bytes())
        .run(&["compute", "app-engine", "validate", "--compose-file", "-"])
        .fails(1, "internal");
    assert_eq!(
        error["message"],
        format!(
            "cannot validate the compose from stdin: the API hashed other bytes than the \
             compose from stdin: its `compose_sha256` is `{}`, and the SHA-256 hipc computed \
             is `{FIXED_SHA256}`",
            COMPOSE_SHA256.to_uppercase()
        )
    );
}

#[test]
fn validate_refuses_an_answer_that_hashed_other_bytes() {
    let home = Home::new();
    let compose = home.file("compose.yaml", COMPOSE.as_bytes());
    let compose = compose.to_str().unwrap();
    for (answer, quoted) in [
        (validated(FIXED_SHA256), format!("`{FIXED_SHA256}`")),
        (r#"{"compose_sha256":7}"#.to_owned(), "`7`".to_owned()),
    ] {
        let server = Server::start(vec![route("POST", VALIDATE, Reply::json(200, &answer))]);
        let error = hipc(
            &home,
            &server,
            &[
                "compute",
                "app-engine",
                "validate",
                "--compose-file",
                compose,
            ],
        )
        .fails(1, "internal");
        assert_eq!(
            error["message"],
            format!(
                "cannot validate the compose file `{compose}`: the API hashed other bytes than \
                 the compose file `{compose}`: its `compose_sha256` is {quoted}, and the \
                 SHA-256 hipc computed is `{COMPOSE_SHA256}`"
            )
        );
        // The check is on a POST that changes nothing, so nothing was done.
        assert_eq!(error["side_effects"], "none");
    }
    // An answer without the hash, or with a null, shows nothing of what the server read.
    for answer in [
        r#"{"services":[],"issues":[]}"#,
        r#"{"compose_sha256":null}"#,
    ] {
        let server = Server::start(vec![route("POST", VALIDATE, Reply::json(200, answer))]);
        let error = hipc(
            &home,
            &server,
            &[
                "compute",
                "app-engine",
                "validate",
                "--compose-file",
                compose,
            ],
        )
        .fails(1, "internal");
        assert_eq!(
            error["message"],
            format!(
                "cannot validate the compose file `{compose}`: the API answered without the \
                 `compose_sha256` of the compose file `{compose}`"
            ),
            "{answer}"
        );
    }
}

#[test]
fn validate_refuses_a_file_it_cannot_send_unchanged_before_anything_is_sent() {
    let home = Home::new();
    let server = Server::start(vec![]);
    let latin1 = home.file("latin1.yaml", b"name: caf\xe9\n");
    let big = home.file("big.yaml", &vec![b'#'; 1024 * 1024 + 1]);
    let cases = [
        (
            latin1.to_str().unwrap().to_owned(),
            "is not UTF-8 text",
            "pass a file of UTF-8 text to `--compose-file`",
        ),
        (
            big.to_str().unwrap().to_owned(),
            "is larger than 1048576 bytes",
            "pass the file that holds the compose to `--compose-file`",
        ),
        (
            home.path()
                .join("missing.yaml")
                .to_str()
                .unwrap()
                .to_owned(),
            "cannot read the compose from",
            "pass a file that can be read to `--compose-file`",
        ),
    ];
    for (file, message, hint) in cases {
        for leaf in ["validate", "fix"] {
            let error = hipc(
                &home,
                &server,
                &["compute", "app-engine", leaf, "--compose-file", &file],
            )
            .fails(2, "usage");
            assert!(
                error["message"].as_str().unwrap().contains(message),
                "{error}"
            );
            assert_eq!(error["hint"], hint);
        }
    }
    assert!(server.requests().is_empty());
}

/// A symlink is never followed and only a regular file is read, before anything is sent and
/// before a dry run prints the body: a link to the config file would upload its token, and
/// a FIFO would wait for a writer.
#[test]
fn a_compose_that_is_a_symlink_or_not_a_regular_file_is_refused() {
    let home = Home::new();
    let server = Server::start(vec![]);
    let target = home.file("compose.yaml", COMPOSE.as_bytes());
    let link = home.path().join("linked.yaml");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let fifo = home.path().join("fifo.yaml");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(made.success());
    let dir = home.path().join("dir.yaml");
    std::fs::create_dir(&dir).unwrap();
    let cases = [
        (
            link.to_str().unwrap(),
            "is a symlink, and a symlink is never followed",
        ),
        (fifo.to_str().unwrap(), "is not a regular file"),
        (dir.to_str().unwrap(), "is not a regular file"),
    ];
    for (file, why) in cases {
        for leaf in ["validate", "fix"] {
            for dry_run in [false, true] {
                let mut args = vec!["compute", "app-engine", leaf, "--compose-file", file];
                if dry_run {
                    args.insert(0, "--dry-run");
                }
                let run = hipc(&home, &server, &args);
                assert_eq!(run.stdout(), "", "{file}");
                let error = run.fails(2, "usage");
                assert_eq!(error["message"], format!("`{file}` {why}"));
                assert_eq!(
                    error["hint"],
                    "pass a regular file to `--compose-file`, or `-` for stdin"
                );
            }
        }
    }
    assert!(server.requests().is_empty());
}

/// `/dev/stdin` and the `/dev/fd/N` path of `<(...)` are refused like any symlink or pipe (on
/// Linux a symlink into `/proc`, on macOS a pipe), so the hint names `-`, which reads stdin.
#[test]
fn a_compose_from_a_stdin_path_names_the_dash() {
    let home = Home::new();
    let server = Server::start(vec![]);
    for file in ["/dev/stdin", "/dev/fd/0"] {
        let run = Hipc::against(&home, &server)
            .stdin(COMPOSE.as_bytes())
            .run(&["compute", "app-engine", "validate", "--compose-file", file]);
        let error = run.fails(2, "usage");
        let message = error["message"].as_str().unwrap();
        assert!(
            message == format!("`{file}` is a symlink, and a symlink is never followed")
                || message == format!("`{file}` is not a regular file"),
            "{error}"
        );
        assert_eq!(
            error["hint"],
            "pass a regular file to `--compose-file`, or `-` for stdin"
        );
    }
    assert!(server.requests().is_empty());
}

/// `hipc` refuses a stdin path before it reads stdin, so a stdin larger than a pipe holds is
/// still being written when it exits: the harness takes that closed pipe as the refusal it
/// is, not as a failure of its own.
#[test]
fn a_refusal_before_stdin_is_read_leaves_the_rest_unwritten() {
    let home = Home::new();
    let server = Server::start(vec![]);
    let large = vec![b'#'; 8 << 20];
    let run = Hipc::against(&home, &server).stdin(&large).run(&[
        "compute",
        "app-engine",
        "validate",
        "--compose-file",
        "/dev/stdin",
    ]);
    run.fails(2, "usage");
    assert!(server.requests().is_empty());
}

/// A refused compose is the server's 400: `validation`, with every issue in `details`, and in
/// text mode an `error:` line per issue.
#[test]
fn a_refused_compose_lists_every_issue() {
    let home = Home::new();
    let compose = home.file("compose.yaml", COMPOSE.as_bytes());
    let compose = compose.to_str().unwrap();
    let refused = json!({
        "error": "app-compose-invalid",
        "detail": "2 problems",
        "line": 3,
        "issues": [
            {"rule": "x-hippius-missing", "line": 1, "column": 1,
             "message": "no `x-hippius` block", "hint": "add one"},
            {"rule": "port-reserved", "line": 4, "column": null, "message": "port 9080"},
            {"rule": "bad", "message": "somewhere\nhint: forged"},
            {"rule": "no-message"},
        ],
    });
    let server = Server::start(vec![route(
        "POST",
        VALIDATE,
        Reply::json(400, &refused.to_string()),
    )]);
    let args = [
        "compute",
        "app-engine",
        "validate",
        "--compose-file",
        compose,
    ];
    let error = hipc(&home, &server, &args).fails(1, "validation");
    assert_eq!(error["details"]["issues"], refused["issues"]);
    assert_eq!(error["http_status"], 400);

    let run = text(&home, &server, &args);
    assert_eq!(run.code(), Some(1));
    assert_eq!(run.stdout(), "");
    assert_eq!(
        run.stderr(),
        format!(
            "error: cannot validate the compose file `{compose}`: the API says \
             `app-compose-invalid`\n\
             error: line 1, column 1: `x-hippius-missing`: the API says `no \\u{{60}}x-hippius\
             \\u{{60}} block`, and suggests `add one`\n\
             error: line 4: `port-reserved`: the API says `port 9080`\n\
             error: `bad`: the API says `somewhere\\nhint: forged`\n"
        )
    );
}

/// Without `issues`, a 400 prints its one line, and a fix's refusal is the same code.
#[test]
fn a_refusal_without_issues_is_one_line() {
    let home = Home::new();
    let compose = home.file("compose.yaml", COMPOSE.as_bytes());
    let compose = compose.to_str().unwrap();
    let server = Server::start(vec![route(
        "POST",
        FIX,
        Reply::json(400, r#"{"error":"app-fix-too-large","issues":"many"}"#),
    )]);
    let run = text(
        &home,
        &server,
        &["compute", "app-engine", "fix", "--compose-file", compose],
    );
    assert_eq!(run.code(), Some(1));
    assert_eq!(
        run.stderr(),
        format!(
            "error: cannot fix the compose file `{compose}`: the API says `app-fix-too-large`\n"
        )
    );
}

#[test]
fn fix_prints_the_answer_and_warns_of_what_alters_what_runs() {
    let home = Home::new();
    let compose = home.file("compose.yaml", COMPOSE.as_bytes());
    let compose = compose.to_str().unwrap();
    let extra = json!({
        "valid": false,
        "changes": [
            {"rule": "pin", "line": 3, "semantic": true, "description": "pinned `nginx`"},
            {"rule": "pin", "semantic": true},
            {"rule": "spacing", "line": 1, "semantic": false, "description": "spacing"},
        ],
        "unresolved": [{"rule": "ports", "line": 2, "message": "port 22"}],
    });
    let server = Server::start(vec![route(
        "POST",
        FIX,
        Reply::json(200, &fixed(FIXED_SHA256, &extra)),
    )]);
    let args = ["compute", "app-engine", "fix", "--compose-file", compose];
    let doc = hipc(&home, &server, &args).ok();
    assert_eq!(doc["fixed_compose"], FIXED);
    assert_eq!(server.only_request().json(), json!({ "compose": COMPOSE }));

    let run = text(&home, &server, &args);
    assert_eq!(run.code(), Some(0));
    assert_eq!(
        run.stderr(),
        "warning: the change at line 3 alters what runs: the API says `pinned \\u{60}nginx\
         \\u{60}`\nwarning: the change alters what runs\nwarning: `fixed_compose` is still not \
         valid: `unresolved` lists what was not fixed\n"
    );
    // A clean answer says nothing more.
    let server = Server::start(vec![route(
        "POST",
        FIX,
        Reply::json(200, &fixed(FIXED_SHA256, &json!({"changes": "none"}))),
    )]);
    let run = text(&home, &server, &args);
    assert_eq!(run.code(), Some(0));
    assert_eq!(run.stderr(), "");
}

#[test]
fn fix_writes_the_fixed_compose_to_a_new_private_file() {
    let home = Home::new();
    let compose = home.file("compose.yaml", COMPOSE.as_bytes());
    let compose = compose.to_str().unwrap();
    let output = home.path().join("fixed.yaml");
    let out = output.to_str().unwrap();
    let server = Server::start(vec![route(
        "POST",
        FIX,
        Reply::json(200, &fixed(FIXED_SHA256, &json!({}))),
    )]);
    let args = [
        "compute",
        "app-engine",
        "fix",
        "--compose-file",
        compose,
        "--output-file",
        out,
    ];
    let run = text(&home, &server, &args);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        format!("note: wrote `fixed_compose` to `{out}`\n")
    );
    assert_eq!(std::fs::read(&output).unwrap(), FIXED.as_bytes());
    let mode = std::fs::metadata(&output).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);

    // The file exists now, so a repeat stops before anything is sent; so does the compose
    // file itself, which hipc never overwrites.
    for target in [out, compose] {
        let error = hipc(
            &home,
            &server,
            &[
                "compute",
                "app-engine",
                "fix",
                "--compose-file",
                compose,
                "--output-file",
                target,
            ],
        )
        .fails(2, "usage");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("hipc never overwrites it"),
            "{error}"
        );
    }
    let error = hipc(
        &home,
        &server,
        &[
            "compute",
            "app-engine",
            "fix",
            "--compose-file",
            compose,
            "--output-file",
            "-",
        ],
    )
    .fails(2, "usage");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("stdout already holds the answer"),
        "{error}"
    );
    assert_eq!(server.requests().len(), 1);
}

/// A `fixed_compose` whose hash is not the answer's, an answer without the hash, or no
/// `fixed_compose`, is `internal`, and no file is written.
#[test]
fn fix_writes_nothing_from_an_answer_it_cannot_trust() {
    let home = Home::new();
    let compose = home.file("compose.yaml", COMPOSE.as_bytes());
    let compose = compose.to_str().unwrap();
    let output = home.path().join("fixed.yaml");
    let out = output.to_str().unwrap();
    let cases = [
        (
            fixed(COMPOSE_SHA256, &json!({})),
            format!(
                "the API's `compose_sha256` does not match its `fixed_compose`: it is \
                 `{COMPOSE_SHA256}`, and the SHA-256 of `fixed_compose` is `{FIXED_SHA256}`"
            ),
        ),
        (
            json!({"fixed_compose": FIXED, "valid": true}).to_string(),
            "the API answered without the `compose_sha256` of `fixed_compose`".to_owned(),
        ),
        (
            json!({"compose_sha256": FIXED_SHA256, "valid": true}).to_string(),
            "the API answered without a `fixed_compose` string".to_owned(),
        ),
    ];
    for (answer, why) in &cases {
        let server = Server::start(vec![route("POST", FIX, Reply::json(200, answer))]);
        let error = hipc(
            &home,
            &server,
            &[
                "compute",
                "app-engine",
                "fix",
                "--compose-file",
                compose,
                "--output-file",
                out,
            ],
        )
        .fails(1, "internal");
        assert_eq!(
            error["message"],
            format!("cannot fix the compose file `{compose}`: {why}")
        );
        assert!(!output.exists());
        // Without `--output-file` the answer is not printed either.
        let run = hipc(
            &home,
            &server,
            &["compute", "app-engine", "fix", "--compose-file", compose],
        );
        assert_eq!(run.code(), Some(1));
        assert_eq!(run.stdout(), "");
    }
}

/// A dry run shows the POST with the compose and sends nothing, and writes no file.
#[test]
fn dry_runs_show_the_compose_and_write_nothing() {
    let home = Home::new();
    let compose = home.file("compose.yaml", COMPOSE.as_bytes());
    let compose = compose.to_str().unwrap();
    let output = home.path().join("fixed.yaml");
    let server = Server::start(vec![]);
    for (leaf, route) in [("validate", VALIDATE), ("fix", FIX)] {
        let mut args = vec!["--dry-run", "compute", "app-engine", leaf];
        args.extend(["--compose-file", compose]);
        if leaf == "fix" {
            args.extend(["--output-file", output.to_str().unwrap()]);
        }
        let doc = hipc(&home, &server, &args).ok();
        assert_eq!(draft(&doc)["method"], "POST");
        assert_eq!(draft(&doc)["url"], format!("{}{route}", server.url()));
        assert_eq!(draft(&doc)["body"], json!({ "compose": COMPOSE }));
        assert_eq!(doc["requests"].as_array().unwrap().len(), 1);
    }
    assert!(server.requests().is_empty());
    assert!(!output.exists());
}
