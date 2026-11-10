//! `hipc compute backups`: a VM's backup policy, its backups, and restoring its disk from one
//! of them.

use serde_json::{Value, json};

use crate::common::{Hipc, Home, MEMBER, Recorded, Reply, Route, Run, Server, draft, route};

const VM: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

const POINT: &str = "run_7c1d";

/// The VM document, trimmed to the fields a restore reads.
const VM_DOC: &str = r#"{"vm_id":"0f8fad5b-d9cb-469f-a165-70867728950e","name":"db-1"}"#;

/// The backup offer, shaped as swagger documents it.
const OPTIONS: &str = concat!(
    r#"{"enabled":true,"free":false,"intervals":[{"seconds":900,"label":"15 min"}],"#,
    r#""retention_days":7,"max_disk_gb":0,"#,
    r#""price":{"free":false,"price_gb_month":"0.01","currency":"USD"},"#,
    r#""failover_auto_available":true}"#
);

/// The policy document, shaped as swagger documents it.
const POLICY: &str = concat!(
    r#"{"enabled":true,"interval_s":900,"retention_days":7,"state":"pending","#,
    r#""restore_point":null,"stored_bytes":0,"eligible":true,"max_disk_gb":0,"#,
    r#""price_gb_month":"0.01","currency":"USD","failover_mode":"manual"}"#
);

/// The backup list, shaped as swagger documents it: an object, not a list.
const BACKUPS: &str = concat!(
    r#"{"state":"ok","stored_bytes":1073741824,"rollback_capable":null,"#,
    r#""restore_points":[{"run_id":"run_7c1d","kind":"full","status":"done","#,
    r#""restorable":true,"class":"current-boot","eta_s":120}]}"#
);

/// The restore answer, shaped as swagger documents it.
const RESTORE: &str = concat!(
    r#"{"restore":{"id":"5b1f6c1e-0d3a-4b2e-9a51-1f4c8f0e2a77","point_id":"run_7c1d","#,
    r#""status":"pending","pct":null,"error":null,"eta_s":120,"rollback":false,"#,
    r#""usable":false}}"#
);

fn vm_route(method: &'static str, suffix: &str, reply: Reply) -> Route {
    route(method, &format!("/api/compute/vms/{VM}/{suffix}"), reply)
}

fn hipc(server: &Server, args: &[&str]) -> Run {
    let mut argv = vec!["compute", "backups"];
    argv.extend(args);
    Hipc::against(&Home::new(), server).run(&argv)
}

/// `hipc compute backups` in text mode.
fn text(server: &Server, args: &[&str]) -> Run {
    let mut argv = vec!["--output", "text", "compute", "backups"];
    argv.extend(args);
    Hipc::against(&Home::new(), server).run(&argv)
}

fn parsed(text: &str) -> Value {
    serde_json::from_str(text).expect("the fixture is JSON")
}

/// The method and path of every request the server received.
fn targets(server: &Server) -> Vec<String> {
    server
        .requests()
        .iter()
        .map(|r| format!("{} {}", r.method, r.target))
        .collect()
}

/// Exit 0 with a key-value block, not a JSON document, on stdout.
fn assert_text(run: &Run, needle: &str) {
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(run.stdout().contains(needle), "{}", run.stdout());
    assert!(
        !run.stdout().trim_start().starts_with('{'),
        "{}",
        run.stdout()
    );
}

#[test]
fn the_reads_print_the_object_as_sent_in_json_and_text() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/compute/backup-options/",
            Reply::json(200, OPTIONS),
        ),
        vm_route("GET", "backup/", Reply::json(200, POLICY)),
        vm_route("GET", "backups/", Reply::json(200, BACKUPS)),
        vm_route("GET", "backups/restore/", Reply::json(200, RESTORE)),
    ]);
    let reads: [(&[&str], &str, &str); 3] = [
        (&["options"], OPTIONS, "price_gb_month"),
        (&["policy", "get", VM], POLICY, "interval_s"),
        (&["restore-status", VM], RESTORE, "eta_s"),
    ];
    for (args, body, field) in &reads[..2] {
        assert_eq!(hipc(&server, args).ok(), parsed(body), "{args:?}");
        assert_text(&text(&server, args), field);
    }
    // The list is the envelope of `restore_points`, with the answer's other fields beside it.
    let mut envelope = parsed(BACKUPS);
    let points = envelope["restore_points"].take();
    envelope
        .as_object_mut()
        .unwrap()
        .shift_remove("restore_points");
    envelope["items"] = points;
    envelope["next_page_token"] = Value::Null;
    assert_eq!(hipc(&server, &["list", VM]).ok(), envelope);
    assert_text(&text(&server, &["list", VM]), "run_7c1d");
    let (args, body, field) = reads[2];
    assert_eq!(hipc(&server, args).ok(), parsed(body), "{args:?}");
    assert_text(&text(&server, args), field);
    let path = |suffix: &str| format!("/api/compute/vms/{VM}/{suffix}");
    let once = [
        "GET /api/compute/backup-options/".to_owned(),
        format!("GET {}", path("backup/")),
        format!("GET {}", path("backups/")),
        format!("GET {}", path("backups/restore/")),
    ];
    let expected: Vec<String> = once.iter().flat_map(|t| [t.clone(), t.clone()]).collect();
    assert_eq!(targets(&server), expected);
}

/// Swagger requires no field of the list answer: one without `restore_points`, or with
/// `null`, such as a VM whose backups are off, is an empty list. Points that are not a list,
/// and a `next` link, are `internal`: neither is the whole list.
#[test]
fn a_backups_list_without_points_is_empty_and_a_bad_one_is_internal() {
    for answer in [
        r#"{"state":"disabled"}"#,
        r#"{"state":"disabled","restore_points":null}"#,
    ] {
        let server = Server::start(vec![vm_route("GET", "backups/", Reply::json(200, answer))]);
        assert_eq!(
            hipc(&server, &["list", VM]).ok(),
            json!({"state": "disabled", "items": [], "next_page_token": null})
        );
    }
    for answer in [
        r#"{"state":"ok","restore_points":"x"}"#,
        r#"{"results":[],"next":"https://api.hippius.com/api/x/?page=2"}"#,
    ] {
        let server = Server::start(vec![vm_route("GET", "backups/", Reply::json(200, answer))]);
        hipc(&server, &["list", VM]).fails(1, "internal");
    }
}

#[test]
fn the_reads_map_their_failures() {
    for (status, code, retryable) in [
        (404, "not_found", false),
        (502, "unavailable", true),
        (503, "unavailable", true),
    ] {
        let failing = || Reply::json(status, r#"{"error":"x"}"#);
        let server = Server::start(vec![
            vm_route("GET", "backup/", failing()),
            vm_route("GET", "backups/", failing()),
            vm_route("GET", "backups/restore/", failing()),
        ]);
        for args in [
            &["policy", "get", VM][..],
            &["list", VM],
            &["restore-status", VM],
        ] {
            let error = hipc(&server, args).fails(1, code);
            assert_eq!(error["retryable"], retryable, "{status} {args:?}");
            assert_eq!(error["side_effects"], "none", "{status} {args:?}");
        }
    }
}

#[test]
fn policy_update_needs_yes_and_says_stored_backups_are_billed() {
    let server = Server::start(vec![vm_route("PUT", "backup/", Reply::json(201, POLICY))]);
    let error = hipc(&server, &["policy", "update", VM, "--interval", "900"])
        .fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!("this would back up the VM `{VM}` every 900 seconds, and `--yes` was not passed")
    );
    let args = [
        "policy",
        "update",
        VM,
        "--interval",
        "3600",
        "--failover",
        "auto",
    ];
    let error = hipc(&server, &args).fails(2, "confirmation_required");
    let message = error["message"].as_str().expect("the message is a string");
    assert!(
        message.contains(
            "every 3600 seconds, with failover `auto`, which restarts it on another server \
             from its latest backup if its server dies, losing what was written after that \
             backup, and"
        ),
        "{message}"
    );
    let args = [
        "policy",
        "update",
        VM,
        "--interval",
        "3600",
        "--failover",
        "manual",
    ];
    let error = hipc(&server, &args).fails(2, "confirmation_required");
    let message = error["message"].as_str().expect("the message is a string");
    assert!(
        message.contains("every 3600 seconds, with failover `manual`, and"),
        "{message}"
    );
    assert!(server.requests().is_empty());
}

#[test]
fn policy_update_sends_the_interval_and_the_failover_only_when_passed() {
    // Swagger: 201 when the PUT turns backups on, 200 when it changes them.
    for status in [200, 201] {
        let server = Server::start(vec![vm_route(
            "PUT",
            "backup/",
            Reply::json(status, POLICY),
        )]);
        let args = ["policy", "update", VM, "--interval", "86400", "--yes"];
        assert_eq!(hipc(&server, &args).ok(), parsed(POLICY));
        let request = server.only_request();
        assert_eq!(request.method, "PUT");
        assert_eq!(request.path(), format!("/api/compute/vms/{VM}/backup/"));
        assert_eq!(request.json(), json!({ "interval_s": 86400 }));
    }
    let server = Server::start(vec![vm_route("PUT", "backup/", Reply::json(200, POLICY))]);
    for (interval, mode) in [("900", "manual"), ("21600", "auto")] {
        let args = [
            "policy",
            "update",
            VM,
            "--interval",
            interval,
            "--failover",
            mode,
            "--yes",
        ];
        hipc(&server, &args).ok();
    }
    let bodies: Vec<Value> = server.requests().iter().map(Recorded::json).collect();
    assert_eq!(
        bodies,
        [
            json!({ "interval_s": 900, "failover_mode": "manual" }),
            json!({ "interval_s": 21600, "failover_mode": "auto" }),
        ]
    );
    let run = text(
        &server,
        &["policy", "update", VM, "--interval", "3600", "--yes"],
    );
    assert_text(&run, "failover_mode");
    assert_eq!(run.stderr(), "");
    assert_eq!(server.requests()[2].json(), json!({ "interval_s": 3600 }));
}

#[test]
fn policy_update_refuses_values_off_the_swagger_enum_with_zero_requests() {
    let server = Server::start(vec![]);
    let update = |extra: &[&'static str]| {
        let mut argv = vec!["policy", "update", VM];
        argv.extend(extra);
        argv.push("--yes");
        hipc(&server, &argv).fails(2, "usage");
    };
    for interval in ["600", "0", "15m", "", "-900", "900.0"] {
        update(&["--interval", interval]);
    }
    update(&[]);
    update(&["--interval", "900", "--failover", "always"]);
    update(&["--interval", "900", "--failover"]);
    assert!(server.requests().is_empty());
}

#[test]
fn policy_update_maps_the_documented_refusals() {
    let cases = [
        (400, "bad-interval", "validation"),
        (402, "compute_runway", "payment_required"),
        (404, "Not found", "not_found"),
        (409, "disk-too-large", "conflict"),
        (409, "not-golden", "conflict"),
        (409, "vm-not-live", "conflict"),
        (502, "Control plane unreachable", "unavailable"),
        (503, "Not available", "unavailable"),
    ];
    for (status, slug, code) in cases {
        let body = json!({ "error": slug }).to_string();
        let server = Server::start(vec![vm_route("PUT", "backup/", Reply::json(status, &body))]);
        let args = ["policy", "update", VM, "--interval", "900", "--yes"];
        let error = hipc(&server, &args).fails(1, code);
        assert_eq!(error["http_status"], status, "{slug}");
        assert_eq!(error["details"], parsed(&body), "{slug}");
        // A 5xx to the PUT may have changed the policy, so a retry is not offered.
        let effects = if status >= 500 { "partial" } else { "none" };
        assert_eq!(error["side_effects"], effects, "{slug}");
        assert_eq!(error["retryable"], false, "{slug}");
    }
}

#[test]
fn policy_delete_needs_yes_and_names_the_vm_on_204() {
    let server = Server::start(vec![vm_route("DELETE", "backup/", Reply::json(204, ""))]);
    let error = hipc(&server, &["policy", "delete", VM]).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!("this would turn off the backups of the VM `{VM}`, and `--yes` was not passed")
    );
    assert!(server.requests().is_empty());
    let doc = hipc(&server, &["policy", "delete", VM, "--yes"]).ok();
    assert_eq!(doc, json!({ "vm_id": VM, "enabled": false }));
    let request = server.only_request();
    assert_eq!(request.method, "DELETE");
    assert_eq!(request.path(), format!("/api/compute/vms/{VM}/backup/"));
    assert_text(
        &text(&server, &["policy", "delete", VM, "--yes"]),
        "enabled",
    );
    let with_body = Server::start(vec![vm_route(
        "DELETE",
        "backup/",
        Reply::json(200, r#"{"enabled":false,"state":"disabled"}"#),
    )]);
    let doc = hipc(&with_body, &["policy", "delete", VM, "--yes"]).ok();
    assert_eq!(doc["state"], "disabled");
}

#[test]
fn policy_delete_maps_its_failures() {
    for (status, code, effects) in [
        (404, "not_found", "none"),
        (502, "unavailable", "partial"),
        (503, "unavailable", "partial"),
    ] {
        let server = Server::start(vec![vm_route(
            "DELETE",
            "backup/",
            Reply::json(status, r#"{"error":"x"}"#),
        )]);
        let error = hipc(&server, &["policy", "delete", VM, "--yes"]).fails(1, code);
        assert_eq!(error["side_effects"], effects, "{status}");
        assert_eq!(error["retryable"], false, "{status}");
    }
}

/// The routes a confirmed restore uses: the VM read, then the POST answering `status`.
fn restore_server(status: u16, body: &str) -> Server {
    Server::start(vec![
        vm_route("GET", "", Reply::json(200, VM_DOC)),
        vm_route("POST", "backups/restore/", Reply::json(status, body)),
    ])
}

/// A confirmed restore of [`POINT`].
fn restore(server: &Server) -> Run {
    hipc(server, &["restore", VM, "--run-id", POINT, "--yes"])
}

#[test]
fn restore_without_yes_writes_nothing_and_names_the_vm_it_would_overwrite() {
    let server = restore_server(202, RESTORE);
    let error =
        hipc(&server, &["restore", VM, "--run-id", POINT]).fails(2, "confirmation_required");
    // The question names the VM read just before it, so the name the server checks is one
    // the person saw.
    assert_eq!(
        error["message"],
        format!(
            "this would overwrite the disk of the VM `db-1` (`{VM}`) with the backup \
             `{POINT}`, and `--yes` was not passed"
        )
    );
    let args = ["restore", VM, "--run-id", POINT, "--accept-rollback"];
    let error = hipc(&server, &args).fails(2, "confirmation_required");
    let message = error["message"].as_str().expect("the message is a string");
    assert!(
        message.contains(&format!(
            "the backup `{POINT}`, which may be a point from before its last reboot, and"
        )),
        "{message}"
    );
    // Only the reads that name the VM went out; nothing was written.
    let methods: Vec<String> = server.requests().iter().map(|r| r.method.clone()).collect();
    assert_eq!(methods, ["GET", "GET"]);
}

#[test]
fn restore_reads_the_vm_name_then_posts_it_as_confirm() {
    let server = restore_server(202, RESTORE);
    assert_eq!(restore(&server).ok(), parsed(RESTORE));
    assert_eq!(
        targets(&server),
        [
            format!("GET /api/compute/vms/{VM}/"),
            format!("POST /api/compute/vms/{VM}/backups/restore/"),
        ]
    );
    assert_eq!(
        server.requests()[1].json(),
        json!({ "point_id": POINT, "mode": "in_place", "confirm": "db-1" })
    );
    let rollback = restore_server(202, RESTORE);
    let args = [
        "restore",
        VM,
        "--run-id",
        POINT,
        "--accept-rollback",
        "--yes",
    ];
    hipc(&rollback, &args).ok();
    assert_eq!(
        rollback.requests()[1].json(),
        json!({
            "point_id": POINT,
            "mode": "in_place",
            "confirm": "db-1",
            "accept_rollback": true,
        })
    );
    let run = text(&server, &["restore", VM, "--run-id", POINT, "--yes"]);
    assert_text(&run, "pending");
    assert_eq!(run.stderr(), "");
}

#[test]
fn restore_sends_no_post_when_the_vm_read_fails() {
    let missing = Server::start(vec![
        vm_route("GET", "", Reply::not_found()),
        vm_route("POST", "backups/restore/", Reply::json(202, RESTORE)),
    ]);
    let error = restore(&missing).fails(1, "not_found");
    assert_eq!(error["http_status"], 404);
    assert_eq!(error["side_effects"], "none");
    assert_eq!(targets(&missing), [format!("GET /api/compute/vms/{VM}/")]);
    let down = Server::start(vec![vm_route(
        "GET",
        "",
        Reply::json(503, r#"{"error":"down"}"#),
    )]);
    let error = restore(&down).fails(1, "unavailable");
    assert_eq!(error["retryable"], true, "a failed read changed nothing");
    assert_eq!(down.requests().len(), 1);
}

#[test]
fn restore_without_a_vm_name_is_internal_and_sends_no_post() {
    for vm in [r#"{"vm_id":"x"}"#, r#"{"vm_id":"x","name":null}"#, "[]"] {
        let server = Server::start(vec![vm_route("GET", "", Reply::json(200, vm))]);
        let error = restore(&server).fails(1, "internal");
        assert_eq!(
            error["message"],
            format!("the VM `{VM}` has no `name`, so the restore's `confirm` cannot be filled"),
            "{vm}"
        );
        assert!(
            error["hint"]
                .as_str()
                .is_some_and(|hint| hint.contains("`--debug`")
                    && hint.contains("https://github.com/thenervelab/hippius-cli/issues")),
            "{error}"
        );
        assert_eq!(error["side_effects"], "none");
        assert_eq!(server.requests().len(), 1, "{vm}");
    }
}

#[test]
fn restore_dry_run_reads_nothing_and_shows_a_placeholder_name() {
    let server = Server::start(vec![]);
    let doc = hipc(&server, &["restore", VM, "--run-id", POINT, "--dry-run"]).dry_ok();
    let vms = format!("{}/api/compute/vms/{VM}/", server.url());
    let post = json!({
        "method": "POST",
        "url": format!("{vms}backups/restore/"),
        "account_header": null,
        "project_header": null,
        "billing_header": null,
        "body": { "point_id": POINT, "mode": "in_place", "confirm": "<the VM's name>" },
    });
    // The document is the POST, and `requests` adds the read of the VM a real run sends
    // first to fill `confirm`.
    let mut expected = json!({ "dry_run": true });
    expected["requests"] = json!([
        {
            "method": "GET",
            "url": vms,
            "account_header": null,
            "project_header": null,
            "billing_header": null,
            "body": null,
        },
        post,
    ]);
    assert_eq!(doc, expected);
    let args = [
        "restore",
        VM,
        "--run-id",
        POINT,
        "--accept-rollback",
        "--dry-run",
    ];
    let doc = hipc(&server, &args).dry_ok();
    assert_eq!(draft(&doc)["body"]["accept_rollback"], true);
    assert_eq!(doc["requests"][1]["body"], draft(&doc)["body"]);
    let run = text(&server, &["restore", VM, "--run-id", POINT, "--dry-run"]);
    assert_eq!(run.code(), Some(0));
    assert_eq!(run.stderr(), "", "a dry run restores nothing");
    assert!(server.requests().is_empty());
}

#[test]
fn restore_rollback_refusal_keeps_the_body_and_hints_the_flag() {
    for body in [
        r#"{"error":"rollback-not-accepted"}"#,
        r#"{"error":"A rollback needs accept_rollback","code":"rollback-not-accepted"}"#,
    ] {
        let server = restore_server(400, body);
        let error = restore(&server).fails(1, "validation");
        assert_eq!(error["details"], parsed(body));
        assert_eq!(
            error["hint"],
            "pass `--accept-rollback` to restore a point from before the VM's last reboot"
        );
        assert_eq!(error["side_effects"], "none");
    }
    // Another 400 gets no hint: `--accept-rollback` would not fix it.
    for body in [r#"{"error":"confirm-mismatch"}"#, "not json"] {
        let server = restore_server(400, body);
        assert_eq!(restore(&server).fails(1, "validation")["hint"], Value::Null);
    }
    let server = restore_server(409, r#"{"error":"rollback-not-accepted"}"#);
    let error = restore(&server).fails(1, "conflict");
    assert_eq!(error["hint"], Value::Null, "only a 400 is that refusal");
}

#[test]
fn restore_maps_the_documented_refusals() {
    let cases = [
        (402, "Compute usage unpaid", "payment_required", false),
        (409, "point-not-restorable", "conflict", false),
        (409, "job-in-flight", "conflict", false),
        (429, "rollback-rate-limited", "rate_limited", true),
        (502, "Control plane unreachable", "unavailable", false),
        (503, "restore-unavailable", "unavailable", false),
    ];
    for (status, slug, code, retryable) in cases {
        let body = json!({ "error": slug }).to_string();
        let server = restore_server(status, &body);
        let error = restore(&server).fails(1, code);
        assert_eq!(error["details"], parsed(&body), "{slug}");
        assert_eq!(error["retryable"], retryable, "{slug}");
        // A 5xx to the POST may have started the restore.
        let effects = if status >= 500 { "partial" } else { "none" };
        assert_eq!(error["side_effects"], effects, "{slug}");
        assert_eq!(server.requests().len(), 2, "{slug}");
    }
}

#[test]
fn bad_ids_and_points_are_usage_with_zero_requests() {
    let server = Server::start(vec![]);
    let long = "a".repeat(65);
    for id in ["", "../x", "a/b", "a?b", "a.b", "a b", "%2e", long.as_str()] {
        for args in [
            &["policy", "get", id][..],
            &["policy", "update", id, "--interval", "900", "--yes"],
            &["policy", "delete", id, "--yes"],
            &["list", id],
            &["restore", id, "--run-id", POINT, "--yes"],
            &["restore-status", id],
            &["restore", VM, "--run-id", id, "--yes"],
        ] {
            hipc(&server, args).fails(2, "usage");
        }
    }
    hipc(&server, &["restore", VM, "--yes"]).fails(2, "usage");
    assert!(server.requests().is_empty());
}

#[test]
fn a_membership_sends_the_header_on_every_route() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/compute/backup-options/",
            Reply::json(200, OPTIONS),
        ),
        vm_route("GET", "backup/", Reply::json(200, POLICY)),
        vm_route("PUT", "backup/", Reply::json(200, POLICY)),
        vm_route("DELETE", "backup/", Reply::json(204, "")),
        vm_route("GET", "backups/", Reply::json(200, BACKUPS)),
        vm_route("GET", "", Reply::json(200, VM_DOC)),
        vm_route("POST", "backups/restore/", Reply::json(202, RESTORE)),
        vm_route("GET", "backups/restore/", Reply::json(200, RESTORE)),
    ]);
    let home = Home::new();
    let member = |args: &[&str]| {
        let mut argv = vec!["compute", "backups"];
        argv.extend(args);
        Hipc::against(&home, &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&argv)
    };
    member(&["options"]).ok();
    member(&["policy", "get", VM]).ok();
    member(&["policy", "update", VM, "--interval", "900", "--yes"]).ok();
    member(&["policy", "delete", VM, "--yes"]).ok();
    member(&["list", VM]).ok();
    member(&["restore", VM, "--run-id", POINT, "--yes"]).ok();
    member(&["restore-status", VM]).ok();
    let requests = server.requests();
    assert_eq!(requests.len(), 8, "the restore reads the VM, then posts");
    for request in requests {
        assert_eq!(
            request.header("x-hippius-account"),
            Some(MEMBER),
            "{}",
            request.target
        );
    }
    let dry = member(&["restore", VM, "--run-id", POINT, "--dry-run"]).ok();
    assert_eq!(draft(&dry)["account_header"], MEMBER);
    // The read a real run sends first carries the same account header.
    for request in dry["requests"]
        .as_array()
        .expect("a dry run lists its requests")
    {
        assert_eq!(request["account_header"], MEMBER, "{request}");
    }
    let error = member(&["restore", VM, "--run-id", POINT]).fails(2, "confirmation_required");
    let message = error["message"].as_str().expect("the message is a string");
    assert!(
        message.contains(&format!("`{POINT}` in the account `{MEMBER}`, and")),
        "{message}"
    );
    // The refused restore read the VM to name it, and wrote nothing.
    assert_eq!(server.requests().len(), 9);
    assert_eq!(server.requests()[8].method, "GET");
}

#[test]
fn restore_checks_the_token_before_the_question() {
    let server = restore_server(202, RESTORE);
    Hipc::new(&Home::new())
        .env("HIPPIUS_API_URL", server.url())
        .run(&["compute", "backups", "restore", VM, "--run-id", POINT])
        .fails(1, "auth");
    // The token is checked before the read that names the VM, so nothing goes out.
    assert!(server.requests().is_empty());
}
