//! `hipc compute app-engine apps update` and `guest-components update`. An update relaunches
//! the VM. Secrets are read from a private file and redacted from every printed copy, as on
//! `apps create`. A guest upgrade that finds the newest release already running exits 0 and
//! posts nothing.

use hippius::routes::compute;
use serde_json::{Value, json};

use crate::common::{Hipc, Home, Reply, Run, Server, draft, route_each_of, route_of};

const VALIDATE: &str = "/api/compute/app-engine/validate/";
const COMPOSE_PUT: &str = "/api/compute/app-engine/apps/vm-1a2b/compose/";
const APP: &str = "/api/compute/app-engine/apps/vm-1a2b/";
const GUEST: &str = "/api/compute/app-engine/apps/vm-1a2b/guest-components/";
const GUEST_POST: &str = "/api/compute/app-engine/apps/vm-1a2b/guest-upgrade/";

const COMPOSE: &str = "services:\n  web:\n    image: nginx\n";
/// `sha256sum` of [`COMPOSE`].
const COMPOSE_SHA256: &str = "5f6fad3dce5125e4e5595e34672d6d2bbe6149ab65095b42d9f77d66e3f8202c";

const SENTINEL: &str = "app-secret-Vq93/never printed";
const OTHER_SECRET: &str = "second \"quoted\" value";

const LIST_HINT: &str = "run `hipc compute app-engine apps list` to see what exists";
const GET_HINT: &str = "run `hipc compute app-engine get` to see whether the account may use it";

fn secrets_json() -> String {
    json!({ "DB_PASSWORD": SENTINEL, "API_KEY": OTHER_SECRET }).to_string()
}

fn assert_clean(text: &str) {
    for secret in [SENTINEL, OTHER_SECRET, "second \\\"quoted"] {
        assert!(!text.contains(secret), "a secret value leaked: {text}");
    }
}

fn assert_run_clean(run: &Run) {
    assert_clean(run.stdout());
    assert_clean(run.stderr());
}

fn path_text(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

struct Files {
    home: Home,
    compose: String,
    secrets: String,
}

fn files() -> Files {
    let home = Home::new();
    let compose = path_text(&home.file("compose.yaml", COMPOSE.as_bytes()));
    let secrets = path_text(&home.file("secrets.json", secrets_json().as_bytes()));
    Files {
        home,
        compose,
        secrets,
    }
}

fn validated(names: &[&str]) -> String {
    json!({
        "grammar_version": 1,
        "compose_sha256": COMPOSE_SHA256,
        "secret_names": names,
    })
    .to_string()
}

fn app_doc(sha: &str) -> String {
    json!({
        "vm_id": "vm-1a2b",
        "status": "running",
        "compose_sha256": sha,
        "secret_names": ["DB_PASSWORD", "API_KEY"],
        "note": SENTINEL,
    })
    .to_string()
}

fn validate_then(names: &[&str], put: Reply) -> Server {
    Server::start(vec![
        route_of(
            &compute::APP_ENGINE_VALIDATE,
            [],
            Reply::json(200, &validated(names)),
        ),
        route_of(&compute::APP_ENGINE_COMPOSE_UPDATE, [&"vm-1a2b"], put),
    ])
}

fn update_args<'a>(files: &'a Files, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "compute",
        "app-engine",
        "apps",
        "update",
        "vm-1a2b",
        "--compose-file",
        &files.compose,
        "--secrets-file",
        &files.secrets,
    ];
    args.extend(extra);
    args
}

fn compose_only<'a>(files: &'a Files, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "compute",
        "app-engine",
        "apps",
        "update",
        "vm-1a2b",
        "--compose-file",
        &files.compose,
    ];
    args.extend(extra);
    args
}

fn secrets_only<'a>(files: &'a Files, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "compute",
        "app-engine",
        "apps",
        "update",
        "vm-1a2b",
        "--secrets-file",
        &files.secrets,
    ];
    args.extend(extra);
    args
}

/// Compose and secrets both go in the one PUT, after validate of the compose alone. The
/// printed copy redacts the value the answer echoed; the hash and the id stay.
#[test]
fn both_files_validate_then_put_and_the_printed_copy_is_redacted() {
    let files = files();
    let server = validate_then(
        &["DB_PASSWORD", "API_KEY"],
        Reply::json(202, &app_doc(COMPOSE_SHA256)),
    );
    let run = Hipc::against(&files.home, &server).run(&update_args(&files, &["--yes"]));
    assert_run_clean(&run);
    let doc = run.ok();
    assert_eq!(doc["vm_id"], "vm-1a2b");
    assert_eq!(doc["compose_sha256"], COMPOSE_SHA256);
    assert_eq!(doc["note"], "<redacted>");
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path(), VALIDATE);
    assert_eq!(requests[0].json(), json!({ "compose": COMPOSE }));
    assert_eq!(requests[1].method, "PUT");
    assert_eq!(requests[1].path(), COMPOSE_PUT);
    assert_eq!(
        requests[1].json(),
        json!({
            "compose": COMPOSE,
            "secrets": { "DB_PASSWORD": SENTINEL, "API_KEY": OTHER_SECRET },
        })
    );
}

/// A compose alone sends no `secrets`. Secrets alone skip validate and read the app first.
#[test]
fn compose_only_and_secrets_only_send_just_that_field() {
    let files = files();
    let server = Server::start(vec![
        route_of(
            &compute::APP_ENGINE_VALIDATE,
            [],
            Reply::json(200, &validated(&[])),
        ),
        route_of(
            &compute::APP_ENGINE_COMPOSE_UPDATE,
            [&"vm-1a2b"],
            Reply::json(202, &app_doc(COMPOSE_SHA256)),
        ),
    ]);
    let doc = Hipc::against(&files.home, &server)
        .run(&compose_only(&files, &["--yes"]))
        .ok();
    assert_eq!(doc["status"], "running");
    assert_eq!(server.requests()[1].json(), json!({ "compose": COMPOSE }));

    let server = Server::start(vec![
        route_of(
            &compute::APP_ENGINE_APP,
            [&"vm-1a2b"],
            Reply::json(200, &validated(&["DB_PASSWORD", "API_KEY"])),
        ),
        route_of(
            &compute::APP_ENGINE_COMPOSE_UPDATE,
            [&"vm-1a2b"],
            Reply::json(202, r#"{"vm_id":"vm-1a2b","status":"running"}"#),
        ),
    ]);
    Hipc::against(&files.home, &server)
        .run(&secrets_only(&files, &["--yes"]))
        .ok();
    let requests = server.requests();
    assert_eq!(requests[0].path(), APP);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(
        requests[1].json(),
        json!({ "secrets": { "DB_PASSWORD": SENTINEL, "API_KEY": OTHER_SECRET } })
    );
}

/// Neither file, and an empty secrets file with no compose, is usage before any request.
#[test]
fn neither_file_is_usage_and_an_empty_secrets_file_sends_no_secrets() {
    let files = files();
    let server = Server::start(vec![]);
    let error = Hipc::against(&files.home, &server)
        .run(&["compute", "app-engine", "apps", "update", "vm-1a2b"])
        .fails(2, "usage");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("`--compose-file`, `--secrets-file`, or both"),
        "{error}"
    );
    assert!(server.requests().is_empty());

    let empty = path_text(&files.home.file("empty.json", b"{}"));
    Hipc::against(&files.home, &server)
        .run(&[
            "compute",
            "app-engine",
            "apps",
            "update",
            "vm-1a2b",
            "--secrets-file",
            &empty,
            "--dry-run",
        ])
        .fails(2, "usage");
    assert!(server.requests().is_empty());

    let server = Server::start(vec![
        route_of(
            &compute::APP_ENGINE_VALIDATE,
            [],
            Reply::json(200, &validated(&[])),
        ),
        route_of(
            &compute::APP_ENGINE_COMPOSE_UPDATE,
            [&"vm-1a2b"],
            Reply::json(202, &app_doc(COMPOSE_SHA256)),
        ),
    ]);
    Hipc::against(&files.home, &server)
        .run(&[
            "compute",
            "app-engine",
            "apps",
            "update",
            "vm-1a2b",
            "--compose-file",
            &files.compose,
            "--secrets-file",
            &empty,
            "--yes",
        ])
        .ok();
    assert_eq!(
        server.requests()[1].json(),
        json!({ "compose": COMPOSE }),
        "an empty secrets file adds no secrets key"
    );
}

/// A missing app on the secrets read is `not_found` with the list hint, and nothing is put.
#[test]
fn a_missing_app_on_the_secrets_read_is_not_found_before_the_question() {
    let files = files();
    let server = Server::start(vec![route_of(
        &compute::APP_ENGINE_APP,
        [&"vm-1a2b"],
        Reply::json(404, r#"{"error":"not-found","detail":"no such app"}"#),
    )]);
    let error = Hipc::against(&files.home, &server)
        .run(&secrets_only(&files, &["--yes"]))
        .fails(1, "not_found");
    assert_eq!(error["hint"], LIST_HINT);
    assert_eq!(server.only_request().path(), APP);
}

/// Without `--yes` the refusal comes after the read, names only the action, and puts nothing.
#[test]
fn without_yes_the_update_is_refused_after_the_read() {
    let files = files();
    let cases = [
        (
            compose_only(&files, &[]),
            "replace the compose of the app `vm-1a2b`",
            VALIDATE,
            &[][..],
        ),
        (
            secrets_only(&files, &[]),
            "replace the secrets of the app `vm-1a2b`",
            APP,
            &["DB_PASSWORD", "API_KEY"][..],
        ),
        (
            update_args(&files, &[]),
            "replace the compose and the secrets of the app `vm-1a2b`",
            VALIDATE,
            &["DB_PASSWORD", "API_KEY"][..],
        ),
    ];
    for (args, action, first, names) in cases {
        let answered = validated(names);
        let server = Server::start(vec![
            route_of(
                &compute::APP_ENGINE_VALIDATE,
                [],
                Reply::json(200, &answered),
            ),
            route_of(
                &compute::APP_ENGINE_APP,
                [&"vm-1a2b"],
                Reply::json(200, &answered),
            ),
            route_of(
                &compute::APP_ENGINE_COMPOSE_UPDATE,
                [&"vm-1a2b"],
                Reply::json(202, &app_doc(COMPOSE_SHA256)),
            ),
        ]);
        let error = Hipc::against(&files.home, &server)
            .run(&args)
            .fails(2, "confirmation_required");
        assert_eq!(
            error["message"],
            format!("this would {action}, and `--yes` was not passed")
        );
        assert_eq!(server.only_request().path(), first);
    }
}

/// The dry run sends nothing. Compose lists validate then the PUT; secrets alone list the
/// app read then the PUT, with a placeholder for each value.
#[test]
fn the_dry_run_lists_the_read_and_the_put_with_placeholders() {
    let files = files();
    let server = Server::start(vec![]);
    let both = Hipc::against(&files.home, &server)
        .run(&update_args(&files, &["--dry-run"]))
        .ok();
    assert_eq!(both["confirmation"], true);
    assert_eq!(both["confirmation_when"], Value::Null);
    assert_eq!(draft(&both)["method"], "PUT");
    assert!(draft(&both)["url"].as_str().unwrap().ends_with(COMPOSE_PUT));
    assert_eq!(
        draft(&both)["body"],
        json!({
            "compose": COMPOSE,
            "secrets": {
                "DB_PASSWORD": "<the DB_PASSWORD from --secrets-file>",
                "API_KEY": "<the API_KEY from --secrets-file>",
            },
        })
    );
    assert!(
        both["requests"][0]["url"]
            .as_str()
            .unwrap()
            .ends_with(VALIDATE)
    );
    assert_run_clean(
        &Hipc::against(&files.home, &server).run(&update_args(&files, &["--dry-run"])),
    );

    let secrets = Hipc::against(&files.home, &server)
        .run(&secrets_only(&files, &["--dry-run"]))
        .dry_ok();
    assert!(
        secrets["requests"][0]["url"]
            .as_str()
            .unwrap()
            .ends_with(APP)
    );
    assert_eq!(
        draft(&secrets)["body"],
        json!({
            "secrets": {
                "DB_PASSWORD": "<the DB_PASSWORD from --secrets-file>",
                "API_KEY": "<the API_KEY from --secrets-file>",
            },
        })
    );
    assert!(server.requests().is_empty());
}

/// A declared name with no value, and a value the compose or the app does not declare, are
/// usage after the read and before the question.
#[test]
fn the_secrets_must_be_the_ones_declared() {
    let files = files();
    let server = Server::start(vec![route_of(
        &compute::APP_ENGINE_VALIDATE,
        [],
        Reply::json(200, &validated(&["DB_PASSWORD"])),
    )]);
    let error = Hipc::against(&files.home, &server)
        .run(&compose_only(&files, &["--yes"]))
        .fails(2, "usage");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("the compose file")
            && error["message"].as_str().unwrap().contains("DB_PASSWORD"),
        "{error}"
    );
    assert_eq!(server.only_request().path(), VALIDATE);

    let server = Server::start(vec![route_of(
        &compute::APP_ENGINE_APP,
        [&"vm-1a2b"],
        Reply::json(200, &validated(&["DB_PASSWORD"])),
    )]);
    let error = Hipc::against(&files.home, &server)
        .run(&secrets_only(&files, &["--yes"]))
        .fails(2, "usage");
    let message = error["message"].as_str().unwrap();
    assert!(
        message.contains("the app `vm-1a2b`") && message.contains("does not declare"),
        "{error}"
    );
    assert_eq!(server.only_request().path(), APP);
}

/// A validate answer that hashed other bytes stops before the question. The update's own
/// answer may leave the hash out; one that names another hash fails and still prints the app.
#[test]
fn the_validate_hash_is_required_and_the_update_hash_is_checked_when_named() {
    let files = files();
    let other = json!({ "compose_sha256": "00".repeat(32), "secret_names": [] });
    let server = Server::start(vec![route_of(
        &compute::APP_ENGINE_VALIDATE,
        [],
        Reply::json(200, &other.to_string()),
    )]);
    let error = Hipc::against(&files.home, &server)
        .run(&compose_only(&files, &["--yes"]))
        .fails(1, "internal");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("hashed other bytes"),
        "{error}"
    );
    assert_eq!(server.only_request().path(), VALIDATE);

    let server = validate_then(
        &[],
        Reply::json(202, r#"{"vm_id":"vm-1a2b","status":"running"}"#),
    );
    let doc = Hipc::against(&files.home, &server)
        .run(&compose_only(&files, &["--yes"]))
        .ok();
    assert_eq!(doc["vm_id"], "vm-1a2b");

    let wrong = app_doc(&"ab".repeat(32));
    let server = validate_then(&[], Reply::json(202, &wrong));
    let run = Hipc::against(&files.home, &server).run(&compose_only(&files, &["--yes"]));
    assert_eq!(run.code(), Some(1), "{}", run.stderr());
    assert_eq!(run.json()["vm_id"], "vm-1a2b");
    let error: Value = serde_json::from_str(run.stderr().lines().last().unwrap()).unwrap();
    assert_eq!(error["error"]["code"], "internal");
    assert_eq!(error["error"]["side_effects"], "partial");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("update answer"),
        "{error}"
    );
}

/// A refusal of the PUT keeps the fields that say what to fix, with every value redacted, in
/// JSON and in text.
#[test]
fn a_refused_update_redacts_the_secret_in_json_and_in_text() {
    let files = files();
    let refused = json!({
        "error": "app-secrets-invalid",
        "detail": format!("DB_PASSWORD is {SENTINEL}"),
        "echo": SENTINEL,
        "issues": [{
            "rule": "secret-name",
            "line": 1,
            "column": 1,
            "message": format!("{SENTINEL} is not allowed"),
            "hint": "rename it",
        }],
    });
    let server = validate_then(
        &["DB_PASSWORD", "API_KEY"],
        Reply::json(400, &refused.to_string()),
    );
    let run = Hipc::against(&files.home, &server).run(&update_args(&files, &["--yes"]));
    assert_run_clean(&run);
    let error = run.fails(1, "validation");
    assert_eq!(error["details"]["detail"], "DB_PASSWORD is <redacted>");
    assert!(error["details"].get("echo").is_none(), "{error}");
    assert_eq!(
        error["details"]["issues"][0]["message"],
        "<redacted> is not allowed"
    );
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("`app-secrets-invalid`"),
        "{error}"
    );

    let server = validate_then(
        &["DB_PASSWORD", "API_KEY"],
        Reply::json(400, &refused.to_string()),
    );
    let text =
        Hipc::against(&files.home, &server).run(&with_text(&update_args(&files, &["--yes"])));
    assert_eq!(text.code(), Some(1), "{}", text.stderr());
    assert_run_clean(&text);
    assert!(
        text.stderr().contains("app-secrets-invalid"),
        "{}",
        text.stderr()
    );
}

/// Each documented status of the PUT: 400 validation, 404 `not_found` with the list hint,
/// 409 conflict, 502 unavailable, and 403 `app-engine-unavailable` names `app-engine get`.
#[test]
fn each_documented_update_status_keeps_its_code() {
    let files = files();
    let cases = [
        (
            400,
            r#"{"error":"app-compose-invalid"}"#,
            "validation",
            None,
        ),
        (
            404,
            r#"{"error":"not-found"}"#,
            "not_found",
            Some(LIST_HINT),
        ),
        (409, r#"{"error":"app-lineage-missing"}"#, "conflict", None),
        (409, r#"{"error":"app-unchanged"}"#, "conflict", None),
        (409, r#"{"error":"app-not-running"}"#, "conflict", None),
        (409, r#"{"error":"app-update-in-flight"}"#, "conflict", None),
        (
            502,
            r#"{"error":"control-plane-unavailable"}"#,
            "unavailable",
            None,
        ),
        (
            403,
            r#"{"error":"app-engine-unavailable"}"#,
            "permission",
            Some(GET_HINT),
        ),
    ];
    for (status, body, code, hint) in cases {
        let server = Server::start(vec![
            route_of(
                &compute::APP_ENGINE_VALIDATE,
                [],
                Reply::json(200, &validated(&[])),
            ),
            route_of(
                &compute::APP_ENGINE_COMPOSE_UPDATE,
                [&"vm-1a2b"],
                Reply::json(status, body),
            ),
        ]);
        let error = Hipc::against(&files.home, &server)
            .run(&compose_only(&files, &["--yes"]))
            .fails(1, code);
        if let Some(hint) = hint {
            assert_eq!(error["hint"], hint, "{status} {body}");
        }
        assert_eq!(server.requests().len(), 2, "{status}");
    }
}

fn guest_read(body: &str) -> Server {
    Server::start(vec![route_of(
        &compute::APP_ENGINE_GUEST_COMPONENTS,
        [&"vm-1a2b"],
        Reply::json(200, body),
    )])
}

/// `--output text` in front of `args`, so a note on stderr is kept.
fn with_text<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut text = vec!["--output", "text"];
    text.extend(args);
    text
}

fn guest_args<'a>(extra: &'a [&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "compute",
        "app-engine",
        "guest-components",
        "update",
        "vm-1a2b",
    ];
    args.extend(extra);
    args
}

fn components(release: &Value, newest: &Value, available: Option<bool>) -> String {
    let mut doc = json!({ "release": release, "newest_release": newest });
    if let Some(available) = available {
        doc["update_available"] = json!(available);
    }
    doc.to_string()
}

/// Already on the newest release: exit 0, the read is printed, nothing is posted, and
/// `--yes` is not required. Text mode notes it.
#[test]
fn an_app_already_on_the_newest_release_exits_0_without_a_post() {
    let home = Home::new();
    let body = components(&json!(7), &json!(9), Some(false));
    let server = guest_read(&body);
    let doc = Hipc::against(&home, &server).run(&guest_args(&[])).ok();
    assert_eq!(doc["release"], 7);
    assert_eq!(doc["newest_release"], 9);
    assert_eq!(server.only_request().method, "GET");
    assert_eq!(server.only_request().path(), GUEST);

    let server = guest_read(&body);
    let text = Hipc::against(&home, &server).run(&with_text(&guest_args(&[])));
    assert_eq!(text.code(), Some(0), "{}", text.stderr());
    assert_eq!(
        text.stderr(),
        "note: `vm-1a2b` is already on the newest guest components release\n"
    );
    assert_eq!(server.requests().len(), 1);
}

/// A newer release is posted only with `--yes`. The body names both releases. The bare base
/// sends `expected_current_release: null`. Text mode names the status command.
#[test]
fn a_newer_release_is_posted_after_yes_and_refused_without_it() {
    let home = Home::new();
    let offered = components(&json!(3), &json!(7), Some(true));
    let server = Server::start(vec![
        route_of(
            &compute::APP_ENGINE_GUEST_COMPONENTS,
            [&"vm-1a2b"],
            Reply::json(200, &offered),
        ),
        route_of(
            &compute::APP_ENGINE_GUEST_UPGRADE_CREATE,
            [&"vm-1a2b"],
            Reply::json(202, r#"{"vm_id":"vm-1a2b","state":"scheduled"}"#),
        ),
    ]);
    let error = Hipc::against(&home, &server)
        .run(&guest_args(&[]))
        .fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        "this would upgrade the guest components of the app `vm-1a2b` from release `3` to \
         release `7`, and `--yes` was not passed"
    );
    assert_eq!(server.only_request().path(), GUEST);

    let server = Server::start(vec![
        route_of(
            &compute::APP_ENGINE_GUEST_COMPONENTS,
            [&"vm-1a2b"],
            Reply::json(200, &offered),
        ),
        route_of(
            &compute::APP_ENGINE_GUEST_UPGRADE_CREATE,
            [&"vm-1a2b"],
            Reply::json(202, r#"{"vm_id":"vm-1a2b","state":"scheduled"}"#),
        ),
    ]);
    let doc = Hipc::against(&home, &server)
        .run(&guest_args(&["--yes"]))
        .ok();
    assert_eq!(doc["state"], "scheduled");
    assert_eq!(server.requests()[1].method, "POST");
    assert_eq!(server.requests()[1].path(), GUEST_POST);
    assert_eq!(
        server.requests()[1].json(),
        json!({ "confirm": true, "release": 7, "expected_current_release": 3 })
    );

    let bare = components(&Value::Null, &json!(7), Some(true));
    let server = Server::start(vec![
        route_of(
            &compute::APP_ENGINE_GUEST_COMPONENTS,
            [&"vm-1a2b"],
            Reply::json(200, &bare),
        ),
        route_of(
            &compute::APP_ENGINE_GUEST_UPGRADE_CREATE,
            [&"vm-1a2b"],
            Reply::json(202, r#"{"vm_id":"vm-1a2b"}"#),
        ),
    ]);
    Hipc::against(&home, &server)
        .run(&guest_args(&["--yes"]))
        .ok();
    assert_eq!(
        server.requests()[1].json(),
        json!({ "confirm": true, "release": 7, "expected_current_release": null })
    );

    let server = Server::start(vec![
        route_of(
            &compute::APP_ENGINE_GUEST_COMPONENTS,
            [&"vm-1a2b"],
            Reply::json(200, &offered),
        ),
        route_of(
            &compute::APP_ENGINE_GUEST_UPGRADE_CREATE,
            [&"vm-1a2b"],
            Reply::json(202, r#"{"vm_id":"vm-1a2b"}"#),
        ),
    ]);
    let text = Hipc::against(&home, &server).run(&with_text(&guest_args(&["--yes"])));
    assert_eq!(text.code(), Some(0), "{}", text.stderr());
    // The test points `hipc` at the server with `HIPPIUS_API_URL`, which the note repeats.
    assert_eq!(
        text.stderr(),
        format!(
            "note: poll the upgrade with `hipc --api-url {} compute app-engine \
             guest-components update-status vm-1a2b`\n",
            server.url()
        )
    );
}

/// `guest-already-current` exits 0 and prints the read taken again, even when that read still
/// says an update is available. Another 409 is a conflict. A read hipc cannot act on is
/// internal, and nothing is posted.
#[test]
fn guest_already_current_exits_0_and_a_nameless_read_is_internal() {
    let home = Home::new();
    let offered = components(&json!(3), &json!(7), Some(true));
    let again = components(&json!(7), &json!(7), Some(true));
    let server = Server::start(vec![
        route_each_of(
            &compute::APP_ENGINE_GUEST_COMPONENTS,
            [&"vm-1a2b"],
            vec![Reply::json(200, &offered), Reply::json(200, &again)],
        ),
        route_of(
            &compute::APP_ENGINE_GUEST_UPGRADE_CREATE,
            [&"vm-1a2b"],
            Reply::json(409, r#"{"error":"guest-already-current"}"#),
        ),
    ]);
    let doc = Hipc::against(&home, &server)
        .run(&guest_args(&["--yes"]))
        .ok();
    assert_eq!(doc["release"], 7);
    assert_eq!(doc["update_available"], true);
    assert_eq!(server.requests().len(), 3);
    assert_eq!(server.requests()[2].path(), GUEST);

    let server = Server::start(vec![
        route_of(
            &compute::APP_ENGINE_GUEST_COMPONENTS,
            [&"vm-1a2b"],
            Reply::json(200, &offered),
        ),
        route_of(
            &compute::APP_ENGINE_GUEST_UPGRADE_CREATE,
            [&"vm-1a2b"],
            Reply::json(409, r#"{"error":"guest-release-changed"}"#),
        ),
    ]);
    Hipc::against(&home, &server)
        .run(&guest_args(&["--yes"]))
        .fails(1, "conflict");
    assert_eq!(server.requests().len(), 2);

    let odd = r#"{"update_available":true,"newest_release":null,"release":3}"#;
    let server = guest_read(odd);
    let error = Hipc::against(&home, &server)
        .run(&guest_args(&["--yes"]))
        .fails(1, "internal");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("cannot tell which guest components release"),
        "{error}"
    );
    assert!(
        error["hint"]
            .as_str()
            .unwrap()
            .contains("guest-components get vm-1a2b"),
        "{error}"
    );
    assert_eq!(server.only_request().method, "GET");
}

/// A missing app, each documented status of the post, and a 403 outside the feature.
#[test]
fn each_documented_guest_status_keeps_its_code() {
    let home = Home::new();
    let missing = Server::start(vec![route_of(
        &compute::APP_ENGINE_GUEST_COMPONENTS,
        [&"vm-1a2b"],
        Reply::json(404, r#"{"error":"not-found"}"#),
    )]);
    let error = Hipc::against(&home, &missing)
        .run(&guest_args(&["--yes"]))
        .fails(1, "not_found");
    assert_eq!(error["hint"], LIST_HINT);
    assert_eq!(missing.only_request().path(), GUEST);

    let offered = components(&json!(3), &json!(7), Some(true));
    let cases = [
        (
            400,
            r#"{"error":"guest-upgrade-unconfirmed"}"#,
            "validation",
            None,
        ),
        (
            400,
            r#"{"error":"guest-release-invalid"}"#,
            "validation",
            None,
        ),
        (
            400,
            r#"{"error":"guest-expected-release-invalid"}"#,
            "validation",
            None,
        ),
        (
            404,
            r#"{"error":"not-found"}"#,
            "not_found",
            Some(LIST_HINT),
        ),
        (
            409,
            r#"{"error":"guest-upgrade-in-flight"}"#,
            "conflict",
            None,
        ),
        (
            409,
            r#"{"error":"guest-release-not-newest"}"#,
            "conflict",
            None,
        ),
        (409, r#"{"error":"app-not-running"}"#, "conflict", None),
        (
            502,
            r#"{"error":"control-plane-unavailable"}"#,
            "unavailable",
            None,
        ),
        (
            403,
            r#"{"error":"app-engine-unavailable"}"#,
            "permission",
            Some(GET_HINT),
        ),
    ];
    for (status, body, code, hint) in cases {
        let server = Server::start(vec![
            route_of(
                &compute::APP_ENGINE_GUEST_COMPONENTS,
                [&"vm-1a2b"],
                Reply::json(200, &offered),
            ),
            route_of(
                &compute::APP_ENGINE_GUEST_UPGRADE_CREATE,
                [&"vm-1a2b"],
                Reply::json(status, body),
            ),
        ]);
        let error = Hipc::against(&home, &server)
            .run(&guest_args(&["--yes"]))
            .fails(1, code);
        if let Some(hint) = hint {
            assert_eq!(error["hint"], hint, "{status} {body}");
        }
    }
}

/// `needs_operator` warns and does not block the upgrade. The dry run lists the read and the
/// post, with placeholders, and sends nothing.
#[test]
fn needs_operator_warns_and_the_dry_run_uses_placeholders() {
    let home = Home::new();
    let mut offered = json!({"release": 3, "newest_release": 7, "update_available": true});
    offered["needs_operator"] = json!(true);
    let server = Server::start(vec![
        route_of(
            &compute::APP_ENGINE_GUEST_COMPONENTS,
            [&"vm-1a2b"],
            Reply::json(200, &offered.to_string()),
        ),
        route_of(
            &compute::APP_ENGINE_GUEST_UPGRADE_CREATE,
            [&"vm-1a2b"],
            Reply::json(202, r#"{"vm_id":"vm-1a2b"}"#),
        ),
    ]);
    let text = Hipc::against(&home, &server).run(&with_text(&guest_args(&["--yes"])));
    assert_eq!(text.code(), Some(0), "{}", text.stderr());
    assert!(
        text.stderr().contains(
            "warning: an earlier upgrade of `vm-1a2b` failed, and Hippius must recover its VM"
        ),
        "{}",
        text.stderr()
    );
    assert_eq!(server.requests()[1].method, "POST");

    let server = Server::start(vec![]);
    let doc = Hipc::against(&home, &server)
        .run(&guest_args(&["--dry-run"]))
        .ok();
    assert_eq!(doc["confirmation"], true);
    assert_eq!(
        doc["confirmation_when"],
        "a newer guest components release is offered"
    );
    assert!(doc["requests"][0]["url"].as_str().unwrap().ends_with(GUEST));
    assert_eq!(draft(&doc)["method"], "POST");
    assert!(draft(&doc)["url"].as_str().unwrap().ends_with(GUEST_POST));
    assert_eq!(
        draft(&doc)["body"],
        json!({
            "confirm": true,
            "release": "<the newest guest components release>",
            "expected_current_release": "<the release the app runs>",
        })
    );
    assert!(server.requests().is_empty());
}
