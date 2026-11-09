//! `hipc compute databases`: managed PostgreSQL and its one-time credentials.

use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::common::{Hipc, Home, MEMBER, Reply, Run, Server, draft, route};

const PASSWORD: &str = "pw-0n3-t1me-9f2c";
const CLIENT_KEY: &str = "MIIclientkey0f3a";
const PASSPHRASE: &str = "bk-cipher-77aa";

/// Every one-time secret the create fixture carries.
const SECRETS: [&str; 3] = [PASSWORD, CLIENT_KEY, PASSPHRASE];

/// A database as swagger documents it, with no credentials.
fn database(status: &str) -> Value {
    json!({
        "id": 7,
        "name": "db-7",
        "tier": "starter",
        "size": "s",
        "engine_version": "18",
        "status": status,
        "suspended": false,
        "connection": {
            "host": null,
            "port": 5432,
            "database": "app",
            "sslmode": "verify-ca",
            "connection_string": null,
            "reachable": false
        },
        "error": "",
        "created_at": "2026-10-27T10:00:00Z"
    })
}

/// The `credentials` object of a database create answer.
fn credentials() -> Value {
    json!({
        "username": "app",
        "password": PASSWORD,
        "database": "app",
        "shown_once": true,
        "backup_cipher_passphrase": PASSPHRASE,
        "tls": {
            "client_cert": "-----BEGIN CERTIFICATE-----\nMIIcert\n-----END CERTIFICATE-----\n",
            "client_key": format!(
                "-----BEGIN PRIVATE KEY-----\n{CLIENT_KEY}\n-----END PRIVATE KEY-----\n"
            ),
            "ca_cert": "-----BEGIN CERTIFICATE-----\nMIIca\n-----END CERTIFICATE-----\n"
        }
    })
}

/// `document` with the credentials added, as the create answers.
fn with_credentials(mut document: Value) -> Value {
    document["credentials"] = credentials();
    document
}

fn created(status: &str) -> String {
    with_credentials(database(status)).to_string()
}

fn db_route(method: &'static str, suffix: &str, reply: Reply) -> crate::common::Route {
    route(method, &format!("/api/databases/7/{suffix}"), reply)
}

fn hipc(server: &Server, args: &[&str]) -> Run {
    let mut argv = vec!["compute", "databases"];
    argv.extend(args);
    Hipc::against(&Home::new(), server).run(&argv)
}

/// The create arguments every test uses, then `extra`.
fn create_args<'a>(extra: &[&'a str]) -> Vec<&'a str> {
    let mut argv = vec![
        "compute",
        "databases",
        "create",
        "--tier",
        "starter",
        "--size",
        "s",
        "--engine-version",
        "18",
    ];
    argv.extend(extra);
    argv
}

fn create(server: &Server, extra: &[&str]) -> Run {
    Hipc::against(&Home::new(), server).run(&create_args(extra))
}

/// Every secret is on stdout, and none on stderr.
fn secrets_on_stdout_only(run: &Run) {
    for secret in SECRETS {
        assert!(run.stdout().contains(secret), "{secret}: {}", run.stdout());
        assert!(!run.stderr().contains(secret), "{secret}: {}", run.stderr());
    }
}

/// No secret in any output.
fn no_secret(run: &Run) {
    for secret in SECRETS {
        assert!(!run.stdout().contains(secret), "{secret}: {}", run.stdout());
        assert!(!run.stderr().contains(secret), "{secret}: {}", run.stderr());
    }
}

#[test]
fn list_and_get_print_the_documents_and_never_a_credential() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/databases/",
            Reply::json(200, &format!("[{}]", created("running"))),
        ),
        db_route(
            "GET",
            "",
            Reply::json(
                200,
                &json!({ "id": 7, "nested": [with_credentials(json!({}))] }).to_string(),
            ),
        ),
    ]);
    let run = hipc(&server, &["list"]);
    assert_eq!(
        run.ok(),
        json!({ "items": [database("running")], "next_page_token": null })
    );
    no_secret(&run);
    let run = hipc(&server, &["get", "7"]);
    assert_eq!(run.ok(), json!({ "id": 7, "nested": [{}] }));
    no_secret(&run);
    let paths: Vec<String> = server
        .requests()
        .iter()
        .map(|r| format!("{} {}", r.method, r.path()))
        .collect();
    assert_eq!(paths, ["GET /api/databases/", "GET /api/databases/7/"]);
    let text = Hipc::against(&Home::new(), &server).run(&[
        "--output",
        "text",
        "compute",
        "databases",
        "list",
    ]);
    assert_eq!(text.code(), Some(0), "{}", text.stderr());
    assert!(text.stdout().contains("db-7"), "{}", text.stdout());
    assert!(!text.stdout().trim_start().starts_with('{'));
    no_secret(&text);
}

#[test]
fn reads_map_their_errors() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/databases/",
            Reply::json(503, r#"{"error":"down"}"#),
        ),
        route("GET", "/api/databases/7/", Reply::not_found()),
        route("GET", "/api/databases/7/events/", Reply::not_found()),
        route("GET", "/api/databases/7/metrics/", Reply::not_found()),
        route(
            "GET",
            "/api/databases/8/metrics/",
            Reply::json(503, r#"{"error":"down"}"#),
        ),
    ]);
    let error = hipc(&server, &["list"]).fails(1, "unavailable");
    assert_eq!(error["retryable"], true);
    for args in [&["get", "7"][..], &["events", "7"], &["metrics", "7"]] {
        let error = hipc(&server, args).fails(1, "not_found");
        assert_eq!(error["side_effects"], "none", "{args:?}");
        assert_eq!(
            error["hint"], "run `hipc compute databases list` to see what exists",
            "{args:?}"
        );
    }
    let error = hipc(&server, &["metrics", "8"]).fails(1, "unavailable");
    assert_eq!(error["retryable"], true);
}

/// The metrics answer, printed as sent, and its `--debug` lines.
#[test]
fn metrics_print_the_series_as_sent_without_credentials() {
    let metrics = concat!(
        r#"{"samples":[{"at":"2026-10-27T10:00:00Z","cpu_percent":12.5,"#,
        r#""memory_used_mb":512,"memory_total_mb":null}],"latest":null,"#,
        r#""agent_last_seen":"2026-10-27T10:01:00Z"}"#
    );
    let server = Server::start(vec![db_route("GET", "metrics/", Reply::json(200, metrics))]);
    let parsed: Value = serde_json::from_str(metrics).expect("a JSON fixture");
    assert_eq!(hipc(&server, &["metrics", "7"]).ok(), parsed);
    let run = hipc(&server, &["metrics", "7", "--debug"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        format!(
            "debug: GET {}/api/databases/7/metrics/\ndebug: -> 200\n",
            server.url()
        )
    );
    let stripped = Server::start(vec![db_route(
        "GET",
        "metrics/",
        Reply::json(200, &created("running")),
    )]);
    let run = hipc(&stripped, &["metrics", "7"]);
    assert_eq!(run.ok(), database("running"));
    no_secret(&run);
}

#[test]
fn events_and_refusals_print_the_list_envelope_without_credentials() {
    // The console reads the journal as `{events: [...]}`, and swagger the refusals as
    // `{refusals: [...]}`: each list is the envelope's `items`, the other fields beside it.
    let events = concat!(
        r#"{"events":[{"at":"2026-10-27T10:00:00Z","kind":"boot","#,
        r#""message":"up","details":{}}],"since":"2026-10-27T00:00:00Z"}"#
    );
    let refusals = concat!(
        r#"{"refusals":[{"id":9,"name":"db-9","tier":"ha","size":"xl","#,
        r#""reason":"no capacity","at":"2026-10-27T10:00:00Z"}]}"#
    );
    let server = Server::start(vec![
        db_route("GET", "events/", Reply::json(200, events)),
        route(
            "GET",
            "/api/databases/refusals/",
            Reply::json(200, refusals),
        ),
    ]);
    let parsed = |text: &str| serde_json::from_str::<Value>(text).expect("a JSON fixture");
    let enveloped = |text: &str, key: &str| {
        let mut doc = parsed(text);
        let items = doc[key].take();
        doc.as_object_mut().unwrap().shift_remove(key);
        doc["items"] = items;
        doc["next_page_token"] = Value::Null;
        doc
    };
    assert_eq!(
        hipc(&server, &["events", "7"]).ok(),
        enveloped(events, "events")
    );
    assert_eq!(
        hipc(&server, &["refusals"]).ok(),
        enveloped(refusals, "refusals")
    );
    // A bare array or a page of events is the list too; a `next` link is not printed as data.
    let pages = Server::start(vec![
        db_route("GET", "events/", Reply::json(200, r#"[{"kind":"boot"}]"#)),
        route(
            "GET",
            "/api/databases/8/events/",
            Reply::json(
                200,
                r#"{"results":[],"next":"https://api.hippius.com/api/databases/8/events/?page=2"}"#,
            ),
        ),
        route(
            "GET",
            "/api/databases/refusals/",
            Reply::json(200, r#"{"refused":[]}"#),
        ),
    ]);
    assert_eq!(
        hipc(&pages, &["events", "7"]).ok(),
        json!({"items": [{"kind": "boot"}], "next_page_token": null})
    );
    hipc(&pages, &["events", "8"]).fails(1, "internal");
    hipc(&pages, &["refusals"]).fails(1, "internal");
    // A `credentials` field at any depth is removed.
    let stripped = Server::start(vec![route(
        "GET",
        "/api/databases/refusals/",
        Reply::json(200, &format!(r#"{{"refusals":[{}]}}"#, created("failed"))),
    )]);
    let run = hipc(&stripped, &["refusals"]);
    assert_eq!(run.ok()["items"][0], database("failed"));
    no_secret(&run);
}

#[test]
fn create_checks_usage_then_reveal_then_the_token_then_confirmation_with_zero_requests() {
    let server = Server::start(vec![]);
    // A bad value is usage even without `--reveal`.
    let run = Hipc::against(&Home::new(), &server).run(&[
        "compute",
        "databases",
        "create",
        "--tier",
        "gold",
        "--size",
        "s",
        "--engine-version",
        "18",
    ]);
    run.fails(2, "usage");
    for extra in [&[][..], &["--yes"]] {
        let error = create(&server, extra).fails(2, "reveal_required");
        assert!(error["hint"].as_str().expect("a hint").contains("--reveal"));
    }
    let error = create(&server, &["--reveal"]).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        "this would create a `starter` PostgreSQL 18 database of size `s`, and `--yes` was not \
         passed"
    );
    assert!(server.requests().is_empty());
    // With no token, `--reveal` is still checked first, and the token before the question.
    let no_token = |extra: &[&str]| {
        Hipc::new(&Home::new())
            .env("HIPPIUS_API_URL", server.url())
            .run(&create_args(extra))
    };
    no_token(&[]).fails(2, "reveal_required");
    no_token(&["--reveal"]).fails(1, "auth");
    assert!(server.requests().is_empty());
}

#[test]
fn bad_tiers_sizes_versions_and_wait_flags_are_usage_with_zero_requests() {
    let server = Server::start(vec![]);
    let base = |tier: &'static str, size: &'static str, version: &'static str| {
        vec![
            "compute",
            "databases",
            "create",
            "--tier",
            tier,
            "--size",
            size,
            "--engine-version",
            version,
            "--reveal",
            "--yes",
        ]
    };
    for argv in [
        base("Starter", "s", "18"),
        base("enterprise", "s", "18"),
        base("starter", "xxl", "18"),
        base("starter", "S", "18"),
        base("starter", "s", "15"),
        base("starter", "s", "18.1"),
        base("starter", "s", ""),
    ] {
        Hipc::against(&Home::new(), &server)
            .run(&argv)
            .fails(2, "usage");
    }
    // The create does not wait, so it has no wait flags.
    for extra in [&["--no-wait"][..], &["--wait-timeout", "5"]] {
        let mut argv = create_args(extra);
        argv.extend(["--reveal", "--yes"]);
        Hipc::against(&Home::new(), &server)
            .run(&argv)
            .fails(2, "usage");
    }
    // A required flag left out.
    Hipc::against(&Home::new(), &server)
        .run(&[
            "compute",
            "databases",
            "create",
            "--tier",
            "ha",
            "--size",
            "m",
            "--reveal",
            "--yes",
        ])
        .fails(2, "usage");
    assert!(server.requests().is_empty());
}

#[test]
fn create_dry_run_needs_neither_flag_and_shows_exactly_the_body() {
    let server = Server::start(vec![]);
    let run = create(&server, &["--dry-run"]);
    let doc = run.ok();
    assert_eq!(draft(&doc)["method"], "POST");
    assert!(
        draft(&doc)["url"]
            .as_str()
            .expect("a URL")
            .ends_with("/api/databases/")
    );
    assert_eq!(
        draft(&doc)["body"],
        json!({ "tier": "starter", "size": "s", "engine_version": "18" })
    );
    let doc = create(&server, &["--name", "orders", "--dry-run"]).ok();
    assert_eq!(
        draft(&doc)["body"],
        json!({ "tier": "starter", "size": "s", "engine_version": "18", "name": "orders" })
    );
    assert!(server.requests().is_empty());
}

#[test]
fn every_tier_size_and_version_is_sent_as_its_api_code() {
    let server = Server::start(vec![]);
    for (tier, size, version) in [
        ("starter", "s", "16"),
        ("production", "m", "17"),
        ("ha", "l", "18"),
        ("ha", "xl", "16"),
    ] {
        let doc = Hipc::against(&Home::new(), &server)
            .run(&[
                "compute",
                "databases",
                "create",
                "--tier",
                tier,
                "--size",
                size,
                "--engine-version",
                version,
                "--dry-run",
            ])
            .ok();
        assert_eq!(
            draft(&doc)["body"],
            json!({ "tier": tier, "size": size, "engine_version": version })
        );
    }
    assert!(server.requests().is_empty());
}

#[test]
fn create_sends_exactly_the_flags_and_prints_the_credentials_on_stdout_only() {
    let server = Server::start(vec![route(
        "POST",
        "/api/databases/",
        Reply::json(202, &created("running")),
    )]);
    let run = create(&server, &["--reveal", "--yes"]);
    assert_eq!(run.ok(), with_credentials(database("running")));
    secrets_on_stdout_only(&run);
    let request = server.only_request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path(), "/api/databases/");
    assert_eq!(
        request.json(),
        json!({ "tier": "starter", "size": "s", "engine_version": "18" })
    );
    let run = Hipc::against(&Home::new(), &server).run(&[
        "compute",
        "databases",
        "create",
        "--tier",
        "ha",
        "--size",
        "xl",
        "--engine-version",
        "16",
        "--name",
        "orders",
        "--reveal",
        "--yes",
    ]);
    run.ok();
    assert_eq!(
        server.requests()[1].json(),
        json!({ "tier": "ha", "size": "xl", "engine_version": "16", "name": "orders" })
    );
}

#[test]
fn a_refused_create_prints_nothing_and_never_a_credential_in_the_error() {
    let server = Server::start(vec![route(
        "POST",
        "/api/databases/",
        Reply::json(400, &created("failed")),
    )]);
    let run = create(&server, &["--reveal", "--yes"]);
    let error = run.fails(1, "validation");
    assert_eq!(error["side_effects"], "none");
    assert_eq!(error["details"], database("failed"));
    no_secret(&run);
}

#[test]
fn delete_needs_yes_and_prints_the_decommissioning_database() {
    let server = Server::start(vec![db_route(
        "DELETE",
        "",
        Reply::json(202, &created("decommissioning")),
    )]);
    let error = hipc(&server, &["delete", "7"]).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        "this would decommission the database `7` and every VM behind it, and `--yes` was not \
         passed"
    );
    assert!(server.requests().is_empty());
    let run = hipc(&server, &["delete", "7", "--yes"]);
    assert_eq!(run.ok(), database("decommissioning"));
    no_secret(&run);
    let request = server.only_request();
    assert_eq!(request.method, "DELETE");
    assert_eq!(request.path(), "/api/databases/7/");
    for (status, code) in [(404, "not_found"), (409, "conflict")] {
        let server = Server::start(vec![db_route(
            "DELETE",
            "",
            Reply::json(status, r#"{"error":"Already decommissioned"}"#),
        )]);
        let error = hipc(&server, &["delete", "7", "--yes"]).fails(1, code);
        assert_eq!(error["side_effects"], "none");
        assert_eq!(error["http_status"], status);
    }
    // In text mode the delete names what the server keeps, and how to remove it.
    let server = Server::start(vec![db_route(
        "DELETE",
        "",
        Reply::json(202, &created("decommissioning")),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&[
        "--output",
        "text",
        "compute",
        "databases",
        "delete",
        "7",
        "--yes",
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        "note: the server keeps the backup bucket `hippius-db-7-backups` and the S3 sub token \
         `db-7-backup-writer` it made for the database: empty the bucket with an S3 client and \
         run `hipc s3 buckets delete hippius-db-7-backups --yes`, and revoke the token with \
         `hipc s3 tokens revoke --kind sub` and its id from `hipc s3 tokens list`\n"
    );
}

#[test]
fn restart_needs_yes_sends_no_body_and_notes_that_it_is_queued() {
    let answer = r#"{"restart":{"pending":true,"requested_at":"2026-10-27T10:00:00Z"}}"#;
    let server = Server::start(vec![db_route("POST", "restart/", Reply::json(201, answer))]);
    let error = hipc(&server, &["restart", "7"]).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        "this would restart PostgreSQL on the database `7`, and `--yes` was not passed"
    );
    let dry = hipc(&server, &["restart", "7", "--dry-run"]).ok();
    assert_eq!(draft(&dry)["method"], "POST");
    assert!(server.requests().is_empty());
    let doc = hipc(&server, &["restart", "7", "--yes"]).ok();
    assert_eq!(doc["restart"]["pending"], true);
    let request = server.only_request();
    assert_eq!(request.path(), "/api/databases/7/restart/");
    assert_eq!(request.header("content-length"), Some("0"));
    let run = Hipc::against(&Home::new(), &server).run(&[
        "--output",
        "text",
        "compute",
        "databases",
        "restart",
        "7",
        "--yes",
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        "note: the restart is queued until the guest's next heartbeat; `restart.pending` in \
         `hipc compute databases get 7` clears once it is done\n"
    );
    let dry = Hipc::against(&Home::new(), &server).run(&[
        "--output",
        "text",
        "compute",
        "databases",
        "restart",
        "7",
        "--dry-run",
    ]);
    assert_eq!(dry.stderr(), "", "a dry run queues nothing");
}

#[test]
fn restore_checks_reveal_then_confirmation_and_sends_the_target_time() {
    let server = Server::start(vec![db_route(
        "POST",
        "restore/",
        Reply::json(
            201,
            &json!({ "id": 8, "credentials": credentials() }).to_string(),
        ),
    )]);
    let time = "2026-10-27T09:30:00Z";
    hipc(&server, &["restore", "7"]).fails(2, "reveal_required");
    hipc(&server, &["restore", "7", "--yes"]).fails(2, "reveal_required");
    let error = hipc(&server, &["restore", "7", "--reveal"]).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        "this would create a new database from the backups of the database `7`, up to the \
         end of its backups, and `--yes` was not passed"
    );
    let argv = ["restore", "7", "--target-time", time, "--reveal"];
    let error = hipc(&server, &argv).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!(
            "this would create a new database from the backups of the database `7`, as it was \
             at `{time}`, and `--yes` was not passed"
        )
    );
    for bad in ["yesterday", "2026-10-27", "2026-10-27 09:30:00Z"] {
        let argv = ["restore", "7", "--target-time", bad, "--reveal", "--yes"];
        hipc(&server, &argv).fails(2, "usage");
    }
    assert!(server.requests().is_empty());
    let dry = hipc(
        &server,
        &["restore", "7", "--target-time", time, "--dry-run"],
    )
    .ok();
    assert_eq!(draft(&dry)["body"], json!({ "target_time": time }));
    assert!(server.requests().is_empty());
    let run = hipc(
        &server,
        &["restore", "7", "--target-time", time, "--reveal", "--yes"],
    );
    assert_eq!(run.ok(), json!({ "id": 8, "credentials": credentials() }));
    secrets_on_stdout_only(&run);
    hipc(&server, &["restore", "7", "--reveal", "--yes"]).ok();
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path(), "/api/databases/7/restore/");
    assert_eq!(requests[0].json(), json!({ "target_time": time }));
    assert!(requests[1].body.is_empty(), "no target time, no body");
}

/// A 2xx to a create or a restore that is not JSON may hold the only copy of the credentials,
/// so its body goes on stdout as `unparsed_answer`, cut at 1 MiB with any invalid UTF-8
/// replaced, beside an `internal` error. An empty body has nothing to keep.
#[test]
fn an_unreadable_create_or_restore_answer_is_printed_as_unparsed_answer() {
    let raw = b"password=pg-secret-1 \xff<html>".to_vec();
    let mut large = vec![b'a'; (1 << 20) + 10];
    large[0] = b'<';
    for (path, argv) in [
        ("/api/databases/", create_args(&["--reveal", "--yes"])),
        (
            "/api/databases/7/restore/",
            vec!["compute", "databases", "restore", "7", "--reveal", "--yes"],
        ),
    ] {
        let server = Server::start(vec![route(
            "POST",
            path,
            Reply::bytes(202, "text/html", &raw),
        )]);
        let run = Hipc::against(&Home::new(), &server).run(&argv);
        assert_eq!(run.code(), Some(1), "{}", run.stderr());
        assert_eq!(
            run.json(),
            json!({ "unparsed_answer": "password=pg-secret-1 \u{fffd}<html>" })
        );
        let error: Value = serde_json::from_str(run.stderr()).unwrap();
        assert_eq!(error["error"]["code"], "internal");
        assert_eq!(error["error"]["side_effects"], "partial");
        assert!(!run.stderr().contains("pg-secret-1"), "{}", run.stderr());
        assert!(
            error["error"]["hint"]
                .as_str()
                .unwrap()
                .contains("unparsed_answer"),
            "{error}"
        );

        let server = Server::start(vec![route(
            "POST",
            path,
            Reply::bytes(202, "text/html", &large),
        )]);
        let run = Hipc::against(&Home::new(), &server).run(&argv);
        assert_eq!(run.code(), Some(1), "{}", run.stderr());
        let kept = run.json()["unparsed_answer"].as_str().unwrap().len();
        assert_eq!(kept, 1 << 20);

        let server = Server::start(vec![route("POST", path, Reply::json(202, " "))]);
        let error = Hipc::against(&Home::new(), &server)
            .run(&argv)
            .fails(1, "internal");
        assert_eq!(error["side_effects"], "partial");
    }
}

#[test]
fn restore_errors_map_to_their_codes_without_a_credential() {
    for (status, code) in [(400, "validation"), (404, "not_found"), (409, "conflict")] {
        let server = Server::start(vec![db_route(
            "POST",
            "restore/",
            Reply::json(status, &created("running")),
        )]);
        let run = hipc(&server, &["restore", "7", "--reveal", "--yes"]);
        let error = run.fails(1, code);
        assert_eq!(error["side_effects"], "none");
        no_secret(&run);
    }
}

#[test]
fn exposure_create_and_delete_need_yes_and_print_what_was_done() {
    let exposed = r#"{"id":7,"exposed":true,"host":"db-7.edge.example","port":5432}"#;
    let server = Server::start(vec![
        db_route("POST", "expose/", Reply::json(201, exposed)),
        db_route("DELETE", "expose/", Reply::json(204, "")),
    ]);
    let error = hipc(&server, &["exposure", "create", "7"]).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        "this would publish the primary of the database `7` on the public edge, where anyone \
         can reach it, and `--yes` was not passed"
    );
    let error = hipc(&server, &["exposure", "delete", "7"]).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        "this would take the primary of the database `7` off the public edge, and `--yes` was \
         not passed"
    );
    assert!(server.requests().is_empty());
    let doc = hipc(&server, &["exposure", "create", "7", "--yes"]).ok();
    assert_eq!(doc["host"], "db-7.edge.example");
    let doc = hipc(&server, &["exposure", "delete", "7", "--yes"]).ok();
    assert_eq!(doc, json!({ "id": 7, "exposed": false }));
    let paths: Vec<String> = server
        .requests()
        .iter()
        .map(|r| format!("{} {}", r.method, r.path()))
        .collect();
    assert_eq!(
        paths,
        [
            "POST /api/databases/7/expose/",
            "DELETE /api/databases/7/expose/"
        ]
    );
    let with_body = Server::start(vec![db_route(
        "DELETE",
        "expose/",
        Reply::json(200, &created("running")),
    )]);
    let run = hipc(&with_body, &["exposure", "delete", "7", "--yes"]);
    assert_eq!(run.ok(), database("running"));
    no_secret(&run);
}

#[test]
fn an_id_that_is_not_a_number_is_usage_with_zero_requests() {
    let server = Server::start(vec![]);
    for id in ["x", "-1", "1.5", "", "7/8", "18446744073709551616"] {
        for args in [
            &["get", id][..],
            &["delete", id, "--yes"],
            &["restart", id, "--yes"],
            &["restore", id, "--reveal", "--yes"],
            &["exposure", "create", id, "--yes"],
            &["exposure", "delete", id, "--yes"],
            &["events", id],
            &["metrics", id],
        ] {
            hipc(&server, args).fails(2, "usage");
        }
    }
    assert!(server.requests().is_empty());
}

#[test]
fn a_membership_sends_the_header_on_every_route() {
    let server = Server::start(vec![
        route("GET", "/api/databases/", Reply::json(200, "[]")),
        route(
            "POST",
            "/api/databases/",
            Reply::json(202, &created("running")),
        ),
        route(
            "GET",
            "/api/databases/refusals/",
            Reply::json(200, r#"{"refusals":[]}"#),
        ),
        db_route(
            "GET",
            "",
            Reply::json(200, &database("running").to_string()),
        ),
        db_route(
            "DELETE",
            "",
            Reply::json(202, &database("decommissioning").to_string()),
        ),
        db_route("POST", "restart/", Reply::json(201, "{}")),
        db_route("POST", "restore/", Reply::json(201, r#"{"id":8}"#)),
        db_route("POST", "expose/", Reply::json(201, "{}")),
        db_route("DELETE", "expose/", Reply::json(204, "")),
        db_route("GET", "events/", Reply::json(200, "[]")),
        db_route("GET", "metrics/", Reply::json(200, r#"{"samples":[]}"#)),
    ]);
    let home = Home::new();
    let member = |args: &[&str]| {
        let mut argv = vec!["compute", "databases"];
        argv.extend(args);
        Hipc::against(&home, &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&argv)
    };
    for args in [
        &["list"][..],
        &["get", "7"],
        &[
            "create",
            "--tier",
            "starter",
            "--size",
            "s",
            "--engine-version",
            "17",
            "--reveal",
            "--yes",
        ],
        &["delete", "7", "--yes"],
        &["restart", "7", "--yes"],
        &["restore", "7", "--reveal", "--yes"],
        &["exposure", "create", "7", "--yes"],
        &["exposure", "delete", "7", "--yes"],
        &["events", "7"],
        &["metrics", "7"],
        &["refusals"],
    ] {
        member(args).ok();
    }
    let requests = server.requests();
    assert_eq!(requests.len(), 11);
    for request in requests {
        assert_eq!(
            request.header("x-hippius-account"),
            Some(MEMBER),
            "{}",
            request.target
        );
    }
    let dry = member(&["restore", "7", "--dry-run"]).ok();
    assert_eq!(draft(&dry)["account_header"], MEMBER);
    let error = member(&["delete", "7"]).fails(2, "confirmation_required");
    assert!(
        error["message"]
            .as_str()
            .expect("the message is a string")
            .contains(&format!("behind it in the account `{MEMBER}`, and")),
        "{error}"
    );
    assert_eq!(server.requests().len(), 11);
}

/// The create sends one POST and prints its answer, `credentials` included, even while
/// the database is still `provisioning`: it does not poll.
#[test]
fn create_sends_one_post_and_prints_the_credentials_without_waiting() {
    let server = Server::start(vec![
        route(
            "POST",
            "/api/databases/",
            Reply::json(202, &created("provisioning")),
        ),
        db_route(
            "GET",
            "",
            Reply::json(200, &database("running").to_string()),
        ),
    ]);
    let run = create(&server, &["--reveal", "--yes"]);
    assert_eq!(run.ok(), with_credentials(database("provisioning")));
    secrets_on_stdout_only(&run);
    let request = server.only_request();
    assert_eq!(
        format!("{} {}", request.method, request.path()),
        "POST /api/databases/"
    );
}

#[test]
fn debug_lines_never_carry_a_credential() {
    let server = Server::start(vec![route(
        "POST",
        "/api/databases/",
        Reply::json(202, &created("provisioning")),
    )]);
    let run = create(&server, &["--reveal", "--yes", "--debug"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    secrets_on_stdout_only(&run);
    let lines: Vec<&str> = run.stderr().lines().collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines.iter().all(|line| line.starts_with("debug: ")),
        "{lines:?}"
    );
}

#[test]
fn text_mode_says_the_credentials_are_shown_only_now_and_how_to_follow_the_create() {
    let mut argv = vec!["--output", "text"];
    argv.extend(create_args(&["--reveal", "--yes"]));
    let text = |answer: &str| {
        let server = Server::start(vec![route(
            "POST",
            "/api/databases/",
            Reply::json(202, answer),
        )]);
        Hipc::against(&Home::new(), &server).run(&argv)
    };
    let run = text(&created("provisioning"));
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        "note: the `credentials` are shown only now, in this document; run \
         `hipc compute databases get 7` to follow the database to `running`\n"
    );
    secrets_on_stdout_only(&run);
    let run = text(&database("provisioning").to_string());
    assert_eq!(
        run.stderr(),
        "note: the answer holds no `credentials`; run `hipc compute databases get 7` to follow the \
         database to `running`\n"
    );
    // Without an `id` there is nothing to follow, and the answer is still printed whole.
    let run = text(&with_credentials(json!({ "status": "provisioning" })).to_string());
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        "note: the `credentials` are shown only now, in this document\n"
    );
    secrets_on_stdout_only(&run);
    argv.push("--quiet");
    let server = Server::start(vec![route(
        "POST",
        "/api/databases/",
        Reply::json(202, &created("provisioning")),
    )]);
    let quiet = Hipc::against(&Home::new(), &server).run(&argv);
    assert_eq!(quiet.code(), Some(0));
    assert_eq!(quiet.stderr(), "");
}

/// The three files `--credentials-dir` writes, each with the `credentials.tls` field it
/// holds.
const TLS_FILES: [(&str, &str); 3] = [
    ("client.crt", "client_cert"),
    ("client.key", "client_key"),
    ("ca.crt", "ca_cert"),
];

/// `databases create ... --credentials-dir <dir> --reveal --yes <extra>` in `home`.
fn create_into(home: &Home, server: &Server, dir: &Path, extra: &[&str]) -> Run {
    let dir = dir.to_str().expect("the test directory is UTF-8");
    let mut argv = create_args(&["--credentials-dir", dir, "--reveal", "--yes"]);
    argv.extend(extra);
    Hipc::against(home, server).run(&argv)
}

/// A directory under `home` named `name`, created empty.
fn empty_dir(home: &Home, name: &str) -> PathBuf {
    let dir = home.path().join(name);
    std::fs::create_dir(&dir).expect("create the credentials directory");
    dir
}

/// The two files `--credentials-dir` writes beside the TLS three.
const OTHER_FILES: [&str; 2] = ["credentials.json", ".env"];

/// No file `--credentials-dir` writes is in `dir`, not even a dangling symlink.
fn no_tls_file(dir: &Path) {
    let tls = TLS_FILES.map(|(file, _)| file);
    for file in tls.iter().chain(&OTHER_FILES) {
        assert!(
            std::fs::symlink_metadata(dir.join(file)).is_err(),
            "{file} was written"
        );
    }
}

/// The document `--credentials-dir <dir>` prints for [`created`]: the database, with
/// `credentials` cut to the fields that hold no secret and the paths of the five files.
fn saved_in(dir: &Path) -> Value {
    let named = |file: &str| Value::from(dir.join(file).to_str().expect("the test fixture holds"));
    let mut expected = database("provisioning");
    expected["credentials"] = json!({
        "username": "app",
        "database": "app",
        "shown_once": true,
        "tls": {
            "client_cert_file": named("client.crt"),
            "client_key_file": named("client.key"),
            "ca_cert_file": named("ca.crt"),
        },
        "json_file": named("credentials.json"),
        "env_file": named(".env"),
    });
    expected
}

/// The `.env` written into `dir` for [`created`], whose `connection` names a port and no host.
fn env_for(dir: &Path) -> String {
    let dir = std::path::absolute(dir).expect("the test fixture holds");
    let at = |file: &str| dir.join(file).display().to_string();
    format!(
        "# libpq settings for this database, written once by hipc; add PGHOST and PGPORT from\n\
         # `hipc compute databases get` when they are missing. Load with: set -a; . ./.env; set +a\n\
         PGPORT='5432'\n\
         PGUSER='app'\n\
         PGPASSWORD='{PASSWORD}'\n\
         PGDATABASE='app'\n\
         PGSSLMODE='verify-ca'\n\
         PGSSLROOTCERT='{}'\n\
         PGSSLCERT='{}'\n\
         PGSSLKEY='{}'\n",
        at("ca.crt"),
        at("client.crt"),
        at("client.key")
    )
}

/// Every file `--credentials-dir` writes holds what it should, with mode 0600.
fn files_written(dir: &Path) {
    let tls = &credentials()["tls"];
    for (file, field) in TLS_FILES {
        let path = dir.join(file);
        assert_eq!(
            std::fs::read_to_string(&path).expect("the file was written"),
            tls[field].as_str().expect("a PEM string")
        );
    }
    let json =
        std::fs::read_to_string(dir.join("credentials.json")).expect("the test fixture holds");
    assert!(json.ends_with("}\n"), "{json}");
    assert_eq!(
        serde_json::from_str::<Value>(&json).expect("the test fixture holds"),
        credentials()
    );
    assert_eq!(
        std::fs::read_to_string(dir.join(".env")).expect("the test fixture holds"),
        env_for(dir)
    );
    let tls = TLS_FILES.map(|(file, _)| file);
    for file in tls.iter().chain(&OTHER_FILES) {
        let mode = std::fs::metadata(dir.join(file))
            .expect("the test fixture holds")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "{file}");
    }
}

/// Every credential goes only to five new files of mode 0600, which the document names in
/// place of the secrets: the TLS three, `credentials.json`, and `.env`.
#[test]
fn credentials_dir_writes_the_tls_files_and_the_document_names_them() {
    let server = Server::start(vec![route(
        "POST",
        "/api/databases/",
        Reply::json(202, &created("provisioning")),
    )]);
    let home = Home::new();
    let dir = empty_dir(&home, "creds");
    let run = create_into(&home, &server, &dir, &[]);
    assert_eq!(run.ok(), saved_in(&dir));
    files_written(&dir);
    no_secret(&run);
    assert_eq!(server.requests().len(), 1);
}

/// `--credentials-dir` is the reveal: without `--reveal` a create and a restore still send,
/// and write every credential only to the directory's files.
#[test]
fn credentials_dir_needs_no_reveal() {
    let server = Server::start(vec![
        route(
            "POST",
            "/api/databases/",
            Reply::json(202, &created("provisioning")),
        ),
        db_route(
            "POST",
            "restore/",
            Reply::json(202, &created("provisioning")),
        ),
    ]);
    let home = Home::new();
    let dir = empty_dir(&home, "created");
    let path = dir.to_str().unwrap();
    let run =
        Hipc::against(&home, &server).run(&create_args(&["--credentials-dir", path, "--yes"]));
    assert_eq!(run.ok(), saved_in(&dir));
    files_written(&dir);
    no_secret(&run);
    let dir = empty_dir(&home, "restored");
    let path = dir.to_str().unwrap();
    let run = Hipc::against(&home, &server).run(&[
        "compute",
        "databases",
        "restore",
        "7",
        "--credentials-dir",
        path,
        "--yes",
    ]);
    assert_eq!(run.ok(), saved_in(&dir));
    files_written(&dir);
    no_secret(&run);
    assert_eq!(server.requests().len(), 2);
}

/// A relative directory is named as given in the document, and by its absolute path in
/// `.env`, so the file works from any directory; a host the answer names goes in too.
#[test]
fn credentials_dir_env_paths_are_absolute_and_name_a_known_host() {
    let mut answer = with_credentials(database("running"));
    answer["connection"]["host"] = "db-7.example.net".into();
    let server = Server::start(vec![route(
        "POST",
        "/api/databases/",
        Reply::json(202, &answer.to_string()),
    )]);
    let home = Home::new();
    let dir = empty_dir(&home, "rel");
    let argv = create_args(&["--credentials-dir", "rel", "--reveal", "--yes"]);
    let run = Hipc::against(&home, &server)
        .current_dir(home.path())
        .run(&argv);
    let doc = run.ok();
    assert_eq!(doc["credentials"]["env_file"], "rel/.env");
    assert_eq!(doc["credentials"]["tls"]["ca_cert_file"], "rel/ca.crt");
    let env = std::fs::read_to_string(dir.join(".env")).unwrap();
    assert!(
        env.contains("PGHOST='db-7.example.net'\nPGPORT='5432'\n"),
        "{env}"
    );
    // The run's current directory is the one the system reports, which on macOS is the real
    // path behind the `/var` link, so the absolute path `.env` names starts there.
    let ca = std::fs::canonicalize(&dir)
        .expect("the directory exists")
        .join("ca.crt");
    assert!(
        env.contains(&format!("PGSSLROOTCERT='{}'\n", ca.display())),
        "{env}"
    );
    no_secret(&run);
}

/// A value `.env` cannot hold single-quoted, with a single quote, a backslash, or a control
/// character, writes no
/// file at all, and prints the whole answer as `internal`, since the database exists.
#[test]
fn a_value_env_cannot_quote_writes_nothing_and_keeps_the_answer_on_stdout() {
    for (field, value) in [
        ("password", "it's"),
        ("password", "back\\slash"),
        ("username", "a\nb"),
        ("database", "a\u{1b}b"),
    ] {
        let mut answer = with_credentials(database("provisioning"));
        answer["credentials"][field] = value.into();
        let server = Server::start(vec![route(
            "POST",
            "/api/databases/",
            Reply::json(202, &answer.to_string()),
        )]);
        let home = Home::new();
        let dir = empty_dir(&home, "creds");
        let run = create_into(&home, &server, &dir, &[]);
        assert_eq!(run.code(), Some(1), "{}", run.stderr());
        assert_eq!(run.json(), answer);
        let error: Value = serde_json::from_str(run.stderr()).expect("one JSON object");
        assert_eq!(error["error"]["code"], "internal");
        assert_eq!(error["error"]["side_effects"], "partial");
        let message = error["error"]["message"].as_str().unwrap();
        assert!(
            message.contains("single quote, a backslash, or a control character"),
            "{message}"
        );
        assert!(message.contains("no file was written"), "{message}");
        no_tls_file(&dir);
    }
}

/// A `connection.sslmode` in the answer is the one `.env` names.
#[test]
fn the_env_file_takes_the_answers_sslmode() {
    let mut answer = with_credentials(database("provisioning"));
    answer["connection"]["sslmode"] = "verify-full".into();
    answer["connection"]
        .as_object_mut()
        .unwrap()
        .shift_remove("port");
    let server = Server::start(vec![route(
        "POST",
        "/api/databases/",
        Reply::json(202, &answer.to_string()),
    )]);
    let home = Home::new();
    let dir = empty_dir(&home, "creds");
    create_into(&home, &server, &dir, &[]).ok();
    let env = std::fs::read_to_string(dir.join(".env")).unwrap();
    assert!(env.contains("PGSSLMODE='verify-full'\n"), "{env}");
    assert!(!env.contains("PGPORT="), "{env}");
}

/// A trailing `/` or `/.` names the same directory, and the files go into it.
#[test]
fn credentials_dir_with_a_trailing_slash_writes_into_the_directory() {
    let server = Server::start(vec![route(
        "POST",
        "/api/databases/",
        Reply::json(202, &created("provisioning")),
    )]);
    let home = Home::new();
    for suffix in ["/", "/."] {
        let dir = empty_dir(&home, &format!("creds{}", suffix.len()));
        let named = PathBuf::from(format!("{}{suffix}", dir.display()));
        let doc = create_into(&home, &server, &named, &[]).ok();
        assert_eq!(
            doc["credentials"]["tls"]["client_key_file"],
            dir.join("client.key").to_str().unwrap()
        );
        assert!(dir.join("client.key").is_file());
    }
}

/// The directory is checked at parse time: it must exist, be a directory and not a
/// symlink to one, and hold none of the three names, not even as a dangling symlink. A bad
/// one is usage, and nothing is sent or written.
#[test]
fn a_bad_credentials_dir_is_usage_with_zero_requests() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let real = empty_dir(&home, "real");
    let link = home.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let file = home.file("plain", b"x");
    let mut bad = vec![
        (home.path().join("missing"), "does not exist"),
        (file, "not a directory"),
        (link.clone(), "symbolic link"),
        (
            PathBuf::from(format!("{}/", link.display())),
            "symbolic link",
        ),
        (
            PathBuf::from(format!("{}/.", link.display())),
            "symbolic link",
        ),
        (PathBuf::new(), "directory"),
    ];
    let tls = TLS_FILES.map(|(file, _)| file);
    for taken in tls.into_iter().chain(OTHER_FILES) {
        let dir = empty_dir(&home, &format!("has-{taken}"));
        if taken == "client.key" {
            std::os::unix::fs::symlink(home.path().join("nowhere"), dir.join(taken)).unwrap();
        } else {
            std::fs::write(dir.join(taken), "old").unwrap();
        }
        bad.push((dir, taken));
    }
    for (dir, words) in bad {
        let error = create_into(&home, &server, &dir, &[]).fails(2, "usage");
        let message = error["message"].as_str().expect("a message");
        assert!(message.contains(words), "{}: {message}", dir.display());
        assert!(
            message.contains("--credentials-dir"),
            "{}: {message}",
            dir.display()
        );
    }
    // A dry run checks it too, and a good one changes nothing in the request.
    create_into(&home, &server, &home.path().join("missing"), &["--dry-run"]).fails(2, "usage");
    let doc = create_into(&home, &server, &real, &["--dry-run"]).ok();
    assert_eq!(
        draft(&doc)["body"],
        json!({ "tier": "starter", "size": "s", "engine_version": "18" })
    );
    no_tls_file(&real);
    assert!(server.requests().is_empty());
    assert_eq!(
        std::fs::read_to_string(home.path().join("has-client.crt/client.crt")).unwrap(),
        "old"
    );
}

/// A directory that passed the check but cannot be written fails after the create: the
/// whole answer, every credential included, is on stdout, the error is `internal` with
/// `side_effects` `partial`, and nothing is retryable, since a rerun creates a second
/// database.
#[test]
fn a_write_failure_after_the_create_keeps_the_answer_on_stdout() {
    let server = Server::start(vec![route(
        "POST",
        "/api/databases/",
        Reply::json(202, &created("provisioning")),
    )]);
    let home = Home::new();
    let dir = empty_dir(&home, "locked");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let run = create_into(&home, &server, &dir, &[]);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(run.code(), Some(1), "{}", run.stderr());
    assert_eq!(run.json(), with_credentials(database("provisioning")));
    secrets_on_stdout_only(&run);
    let error: Value = serde_json::from_str(run.stderr()).expect("one JSON object");
    let error = &error["error"];
    assert_eq!(error["code"], "internal");
    assert_eq!(error["side_effects"], "partial");
    assert_eq!(error["retryable"], false);
    let message = error["message"].as_str().unwrap();
    assert!(
        message.contains("client.crt`: permission is denied"),
        "{message}"
    );
    assert!(message.contains("stdout"), "{message}");
    no_tls_file(&dir);
}

/// A file that appears after the check is not overwritten: the files written before it
/// stay, the message names them, and the answer is on stdout whole.
#[test]
fn a_file_made_during_the_create_is_kept_and_the_answer_stays_on_stdout() {
    let home = Home::new();
    let dir = empty_dir(&home, "raced");
    let quoted = |file: &str| format!("`{}`", dir.join(file).display());
    for (planted, written) in [
        (
            "client.key",
            format!("{} was written", quoted("client.crt")),
        ),
        (
            "ca.crt",
            format!(
                "{} and {} were written",
                quoted("client.crt"),
                quoted("client.key")
            ),
        ),
        (
            "credentials.json",
            format!(
                "{}, {}, and {} were written",
                quoted("client.crt"),
                quoted("client.key"),
                quoted("ca.crt")
            ),
        ),
        (
            ".env",
            format!(
                "{}, {}, {}, and {} were written",
                quoted("client.crt"),
                quoted("client.key"),
                quoted("ca.crt"),
                quoted("credentials.json")
            ),
        ),
    ] {
        let tls = TLS_FILES.map(|(file, _)| file);
        for file in tls.iter().chain(&OTHER_FILES) {
            drop(std::fs::remove_file(dir.join(file)));
        }
        let at = dir.join(planted);
        // The server thread must not panic, so a failure to plant the file is left to the
        // assertions below.
        let server = Server::start(vec![route(
            "POST",
            "/api/databases/",
            Reply::json(202, &created("provisioning"))
                .before(move || drop(std::fs::write(&at, "someone else's"))),
        )]);
        let run = create_into(&home, &server, &dir, &[]);
        assert_eq!(run.code(), Some(1), "{}", run.stderr());
        assert_eq!(run.json(), with_credentials(database("provisioning")));
        let error: Value = serde_json::from_str(run.stderr()).expect("one JSON object");
        let message = error["error"]["message"].as_str().unwrap();
        assert!(
            message.contains(&format!("{planted}`: the system says `File exists")),
            "{message}"
        );
        assert!(message.contains(&written), "{message}");
        assert_eq!(
            std::fs::read_to_string(dir.join(planted)).unwrap(),
            "someone else's"
        );
        // The TLS files, written first, hold no password.
        for (file, _) in TLS_FILES {
            if let Ok(text) = std::fs::read_to_string(dir.join(file)) {
                assert!(!text.contains(PASSWORD), "{file}");
            }
        }
    }
    assert!(dir.join("client.crt").is_file());
    assert!(dir.join("client.key").is_file());
}

/// The files are created in the open directory, so a directory that can be written but not
/// opened (mode 0300) fails before any file is written, with the whole answer on stdout.
#[test]
fn a_directory_that_cannot_be_opened_keeps_the_answer_on_stdout() {
    let server = Server::start(vec![route(
        "POST",
        "/api/databases/",
        Reply::json(202, &created("provisioning")),
    )]);
    let home = Home::new();
    let dir = empty_dir(&home, "write-only");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o300)).unwrap();
    let run = create_into(&home, &server, &dir, &[]);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(run.code(), Some(1), "{}", run.stderr());
    assert_eq!(run.json(), with_credentials(database("provisioning")));
    let error: Value = serde_json::from_str(run.stderr()).expect("one JSON object");
    assert_eq!(error["error"]["code"], "internal");
    assert_eq!(error["error"]["side_effects"], "partial");
    assert_eq!(
        error["error"]["message"],
        format!(
            "cannot create `{}`: its directory cannot be opened: permission is denied; every \
             credential is in the answer on stdout",
            dir.join("client.crt").display()
        )
    );
    no_tls_file(&dir);
}

/// A directory another user can write lets them swap `ca.crt` (which `sslmode=verify-ca`
/// trusts) or delete `client.key`, so one that is world-writable, or group-writable by a group
/// other than the user's private group, is usage before anything is sent. A sticky one, such as `/tmp`,
/// passes: no one else may rename or delete a file in it.
#[test]
fn a_credentials_dir_others_can_write_is_usage_with_zero_requests() {
    let server = Server::start(vec![route(
        "POST",
        "/api/databases/",
        Reply::json(202, &created("provisioning")),
    )]);
    let home = Home::new();
    for mode in [0o777, 0o757] {
        let dir = empty_dir(&home, &format!("open-{mode:o}"));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode)).unwrap();
        for extra in [&[][..], &["--dry-run"]] {
            let error = create_into(&home, &server, &dir, extra).fails(2, "usage");
            let message = error["message"].as_str().unwrap();
            assert!(
                message.contains("can be written by other users"),
                "{mode:o}: {message}"
            );
            assert!(message.contains("chmod go-w"), "{message}");
        }
        no_tls_file(&dir);
    }
    assert!(server.requests().is_empty());
    let sticky = empty_dir(&home, "sticky");
    std::fs::set_permissions(&sticky, std::fs::Permissions::from_mode(0o1777)).unwrap();
    let run = create_into(&home, &server, &sticky, &[]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(sticky.join("client.key").is_file());
    // The user's private group may write it: its umask 002 makes every directory 0775.
    let own = empty_dir(&home, "own-group");
    std::fs::set_permissions(&own, std::fs::Permissions::from_mode(0o775)).unwrap();
    // Only a user private group (the primary group, with the user's own id) may write it; on
    // macOS a new directory takes its parent's group, and many users share `staff`.
    let run = create_into(&home, &server, &own, &[]);
    let gid = own.metadata().unwrap().gid();
    if gid == rustix::process::getegid().as_raw() && gid == rustix::process::geteuid().as_raw() {
        assert_eq!(run.code(), Some(0), "{}", run.stderr());
    } else {
        run.fails(2, "usage");
    }
}

/// A directory whose parent cannot be searched cannot be checked, so it is usage too.
#[test]
fn a_credentials_dir_that_cannot_be_read_is_usage() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let parent = empty_dir(&home, "closed");
    let dir = parent.join("creds");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o000)).unwrap();
    let run = create_into(&home, &server, &dir, &[]);
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
    let error = run.fails(2, "usage");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .ends_with("cannot read that directory: permission is denied"),
        "{error}"
    );
    assert!(server.requests().is_empty());
}

/// An answer without the three TLS strings writes nothing and keeps the answer on stdout.
#[test]
fn an_answer_without_tls_files_writes_nothing() {
    let mut partial = with_credentials(database("provisioning"));
    partial["credentials"]["tls"]["ca_cert"] = Value::Null;
    for answer in [database("provisioning"), partial] {
        let server = Server::start(vec![route(
            "POST",
            "/api/databases/",
            Reply::json(202, &answer.to_string()),
        )]);
        let home = Home::new();
        let dir = empty_dir(&home, "creds");
        let run = create_into(&home, &server, &dir, &[]);
        assert_eq!(run.code(), Some(1), "{}", run.stderr());
        assert_eq!(run.json(), answer);
        let error: Value = serde_json::from_str(run.stderr()).expect("one JSON object");
        assert_eq!(error["error"]["code"], "internal");
        assert_eq!(error["error"]["side_effects"], "partial");
        no_tls_file(&dir);
    }
}

/// Text mode names the five files; no secret is in any output.
#[test]
fn text_mode_names_the_tls_files() {
    let server = Server::start(vec![route(
        "POST",
        "/api/databases/",
        Reply::json(202, &created("provisioning")),
    )]);
    let home = Home::new();
    let dir = empty_dir(&home, "creds");
    let run = create_into(&home, &server, &dir, &["--output", "text"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(run.stderr(), note_for(&dir));
    no_secret(&run);
}

/// The text-mode note of a create or a restore into `dir`.
fn note_for(dir: &Path) -> String {
    let quoted = |file: &str| format!("`{}`", dir.join(file).display());
    format!(
        "note: the `credentials` are in {}, {}, {}, {}, and {}, and only there: no command \
         shows them again; run `hipc compute databases get 7` to follow the database to \
         `running`\n",
        quoted("client.crt"),
        quoted("client.key"),
        quoted("ca.crt"),
        quoted("credentials.json"),
        quoted(".env")
    )
}

/// `databases restore ... --credentials-dir <dir> --reveal --yes <extra>` in `home`.
fn restore_into(home: &Home, server: &Server, dir: &Path, extra: &[&str]) -> Run {
    let dir = dir.to_str().expect("the test directory is UTF-8");
    let mut argv = vec![
        "compute",
        "databases",
        "restore",
        "7",
        "--credentials-dir",
        dir,
        "--reveal",
        "--yes",
    ];
    argv.extend(extra);
    Hipc::against(home, server).run(&argv)
}

/// A server whose restore of the database `7` answers `answer`, the new database.
fn restore_server(answer: &str) -> Server {
    Server::start(vec![db_route("POST", "restore/", Reply::json(202, answer))])
}

/// A restore makes a new database with its own credentials, so `--credentials-dir` writes
/// its files as a create's are: five new files of mode 0600, named in the document in place
/// of the secrets, and named by the text-mode note.
#[test]
fn restore_credentials_dir_writes_the_tls_files_and_the_document_names_them() {
    let server = restore_server(&created("provisioning"));
    let home = Home::new();
    let dir = empty_dir(&home, "creds");
    let run = restore_into(&home, &server, &dir, &[]);
    assert_eq!(run.ok(), saved_in(&dir));
    files_written(&dir);
    no_secret(&run);
    let text_dir = empty_dir(&home, "text");
    let run = restore_into(&home, &server, &text_dir, &["--output", "text"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    no_secret(&run);
    assert_eq!(run.stderr(), note_for(&text_dir));
    let sent = server.requests();
    assert_eq!(sent.len(), 2);
    assert!(sent.iter().all(|r| r.path() == "/api/databases/7/restore/"));
}

/// The restore's directory is held to the create's rules at parse time: missing, a
/// symlink, writable by others, or holding one of the three names is usage, in a dry run
/// too, and nothing is sent or written.
#[test]
fn a_bad_restore_credentials_dir_is_usage_with_zero_requests() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let real = empty_dir(&home, "real");
    let link = home.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let open = empty_dir(&home, "open");
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
    let taken = empty_dir(&home, "taken");
    std::fs::write(taken.join("client.key"), "old").unwrap();
    for (dir, words) in [
        (home.path().join("missing"), "does not exist"),
        (link, "symbolic link"),
        (open.clone(), "can be written by other users"),
        (taken.clone(), "client.key"),
    ] {
        for extra in [&[][..], &["--dry-run"]] {
            let error = restore_into(&home, &server, &dir, extra).fails(2, "usage");
            let message = error["message"].as_str().expect("a message");
            assert!(message.contains(words), "{}: {message}", dir.display());
            assert!(message.contains("--credentials-dir"), "{message}");
        }
    }
    let doc = restore_into(&home, &server, &real, &["--dry-run"]).ok();
    assert_eq!(doc["requests"][0]["method"], "POST");
    assert!(server.requests().is_empty());
    no_tls_file(&real);
    no_tls_file(&open);
    assert_eq!(
        std::fs::read_to_string(taken.join("client.key")).unwrap(),
        "old"
    );
}

/// After the restore was sent, a file that cannot be written, or an answer without the
/// three TLS strings, keeps the whole answer on stdout as `internal` and `partial`: the new
/// database exists, and a rerun would make a second one. A file written before the failure
/// stays and is named.
#[test]
fn a_restore_whose_files_cannot_be_written_keeps_the_answer_on_stdout() {
    let home = Home::new();
    let dir = empty_dir(&home, "raced");
    let at = dir.join("client.key");
    let server = Server::start(vec![db_route(
        "POST",
        "restore/",
        Reply::json(202, &created("provisioning"))
            .before(move || drop(std::fs::write(&at, "someone else's"))),
    )]);
    let run = restore_into(&home, &server, &dir, &[]);
    assert_eq!(run.code(), Some(1), "{}", run.stderr());
    assert_eq!(run.json(), with_credentials(database("provisioning")));
    secrets_on_stdout_only(&run);
    let error: Value = serde_json::from_str(run.stderr()).expect("one JSON object");
    let error = &error["error"];
    assert_eq!(error["code"], "internal");
    assert_eq!(error["side_effects"], "partial");
    let message = error["message"].as_str().unwrap();
    assert!(
        message.contains("client.key`: the system says `File exists"),
        "{message}"
    );
    assert!(
        message.contains(&format!(
            "`{}` was written",
            dir.join("client.crt").display()
        )),
        "{message}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("client.key")).unwrap(),
        "someone else's"
    );
    // An answer that holds no credentials writes nothing.
    let server = restore_server(&database("provisioning").to_string());
    let empty = empty_dir(&home, "empty");
    let run = restore_into(&home, &server, &empty, &[]);
    assert_eq!(run.code(), Some(1), "{}", run.stderr());
    assert_eq!(run.json(), database("provisioning"));
    let error: Value = serde_json::from_str(run.stderr()).expect("one JSON object");
    assert_eq!(error["error"]["code"], "internal");
    assert_eq!(error["error"]["side_effects"], "partial");
    no_tls_file(&empty);
}

/// A field outside `credentials` that repeats a withheld value, such as a connection string
/// with the password in it, shows `<redacted>` in its place.
#[test]
fn credentials_dir_redacts_a_secret_repeated_outside_credentials() {
    let mut answer = with_credentials(database("provisioning"));
    answer["connection"]["connection_string"] =
        format!("postgresql://app:{PASSWORD}@db-7.example.net/app").into();
    answer["backup"] = json!({ "note": format!("passphrase {PASSPHRASE}") });
    // A field the fixture does not know, a list, is withheld and redacted too.
    answer["credentials"]["extra"] = json!(["xtra-secret-41", 7, ""]);
    answer["hosts"] = json!(["h xtra-secret-41", 7]);
    let server = Server::start(vec![route(
        "POST",
        "/api/databases/",
        Reply::json(202, &answer.to_string()),
    )]);
    let home = Home::new();
    let dir = empty_dir(&home, "creds");
    let run = create_into(&home, &server, &dir, &[]);
    let doc = run.ok();
    no_secret(&run);
    assert_eq!(
        doc["connection"]["connection_string"],
        "postgresql://app:<redacted>@db-7.example.net/app"
    );
    assert_eq!(doc["backup"]["note"], "passphrase <redacted>");
    assert_eq!(doc["hosts"], json!(["h <redacted>", 7]));
    assert!(doc["credentials"].get("extra").is_none(), "{doc}");
    assert_eq!(doc["credentials"]["username"], "app");
}

/// A directory whose path `.env` cannot hold single-quoted is usage before anything is sent.
#[test]
fn a_credentials_dir_path_env_cannot_quote_is_usage_with_zero_requests() {
    let server = Server::start(vec![]);
    let home = Home::new();
    for name in ["it's", "back\\slash", "line\nbreak"] {
        let dir = empty_dir(&home, name);
        let error = create_into(&home, &server, &dir, &[]).fails(2, "usage");
        let message = error["message"].as_str().expect("a message");
        assert!(
            message.contains("no single quote, backslash, or control character"),
            "{message}"
        );
        no_tls_file(&dir);
    }
    assert!(server.requests().is_empty());
}

/// A create the API refuses for the balance names the reads that tell how much it needs.
#[test]
fn a_create_refused_for_the_balance_names_the_reads_that_size_it() {
    let server = Server::start(vec![route(
        "POST",
        "/api/databases/",
        Reply::json(402, r#"{"detail":"Balance does not cover 24 hours"}"#),
    )]);
    let error = create(&server, &["--reveal", "--yes"]).fails(1, "payment_required");
    assert!(
        error["hint"]
            .as_str()
            .unwrap()
            .starts_with("run `hipc billing summary` and `hipc compute quota`"),
        "{error}"
    );
}
