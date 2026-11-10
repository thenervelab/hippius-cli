//! `hipc compute kubernetes`: the cluster reads, delete, backups, node pools, operations, and
//! node removal, none of which takes the cluster key.

use serde_json::{Value, json};

use crate::common::{
    Hipc, Home, MEMBER, PROJECT, Recorded, Reply, Route, Run, Server, draft, one_request_dry_run,
    parsed, route,
};

/// The signature of the presigned snapshot link in [`backups`].
const SIGNATURE: &str = "X-Amz-Signature=5e3c0ffee";

/// The sealed key of the snapshot in [`backups`].
const SEALED_KEY: &str = "c2VhbGVkLWtleS1vZi10aGUtc25hcHNob3Q=";

/// A cluster, in the console's shape.
fn cluster(status: &str) -> Value {
    json!({
        "id": 7,
        "name": "prod",
        "mode": "standard",
        "cp_size": "m",
        "k8s_version": "v1.34.3+rke2r1",
        "region": "FR",
        "status": status,
        "error": null,
        "api_source_cidrs": ["203.0.113.7/32"],
        "endpoints": ["https://192.0.2.10:6443"],
        "nodes": [{"name": "m0", "role": "master", "state": "ready", "ready": true}],
        "created_at": "2026-10-27T10:00:00Z"
    })
}

fn pool() -> Value {
    json!({
        "id": 3,
        "name": "workers",
        "role": "worker",
        "flavor": "medium",
        "desired_count": 0,
        "labels": {},
        "taints": [],
        "signing_params": null,
        "nodes": []
    })
}

/// A `remove_node` operation; blocked, it waits on a disruption budget, which a plain
/// resume retries.
fn operation(state: &str) -> Value {
    json!({
        "id": 12,
        "seq": 4,
        "kind": "remove_node",
        "state": state,
        "step": "cordon_drain",
        "node": "work-3a5bf1ed",
        "reason": "a disruption budget blocks the drain",
        "params": {},
        "detail": {}
    })
}

/// An operation blocked on data the node would lose.
fn local_data_block() -> Value {
    json!({
        "id": 12,
        "kind": "remove_node",
        "state": "blocked",
        "step": "check_local_data",
        "node": "work-3a5bf1ed",
        "reason": "local data found",
        "params": {},
        "detail": { "local_data": [{ "kind": "pv", "name": "pgdata" }] }
    })
}

fn backups() -> Value {
    json!({
        "enabled": true,
        "error": null,
        "etcd_bucket": "hippius-k8s-7-etcd",
        "velero_bucket": "hippius-k8s-7-velero",
        "snapshots": [{
            "name": "20261027T120000Z-m0",
            "taken_at": "2026-10-27T12:00:00Z",
            "manifest": {
                "name": "20261027T120000Z-m0",
                "taken_at": "2026-10-27T12:00:00Z",
                "sealed_key": SEALED_KEY,
                "sha256": "00ff"
            },
            "signature": "c2lnbmF0dXJl",
            "download_url": format!(
                "https://s3.hippius.com/hippius-k8s-7-etcd/etcd/20261027T120000Z-m0.enc?\
                 X-Amz-Expires=900&{SIGNATURE}"
            )
        }]
    })
}

/// [`backups`] as the command prints it: the envelope of its `snapshots`, the other fields
/// beside them.
fn backups_envelope() -> Value {
    let mut envelope = backups();
    let snapshots = envelope["snapshots"].take();
    envelope
        .as_object_mut()
        .expect("the backups answer is an object")
        .shift_remove("snapshots");
    envelope["items"] = snapshots;
    envelope["next_page_token"] = Value::Null;
    envelope
}

fn events() -> Value {
    json!([{
        "at": "2026-10-27T10:00:00Z",
        "node": "m0",
        "kind": "rke2.ready",
        "message": "up",
        "details": {}
    }])
}

fn refusals() -> Value {
    json!([{
        "name": "big",
        "mode": "standard",
        "cp_size": "l",
        "reason": "no region can place three masters",
        "at": "2026-10-27T10:00:00Z"
    }])
}

fn cluster_route(method: &'static str, suffix: &str, reply: Reply) -> Route {
    route(
        method,
        &format!("/api/kubernetes/clusters/7/{suffix}"),
        reply,
    )
}

fn answer(status: u16, value: &Value) -> Reply {
    Reply::json(status, &value.to_string())
}

/// A route for every command of the group, each answering as the console reads it.
fn every_route() -> Vec<Route> {
    vec![
        route(
            "GET",
            "/api/kubernetes/clusters/",
            answer(200, &json!([cluster("running")])),
        ),
        route(
            "GET",
            "/api/kubernetes/clusters/refusals/",
            answer(200, &refusals()),
        ),
        cluster_route("GET", "", answer(200, &cluster("running"))),
        cluster_route("DELETE", "", answer(202, &cluster("decommissioning"))),
        cluster_route("GET", "events/", answer(200, &events())),
        cluster_route("GET", "backups/", answer(200, &backups())),
        cluster_route("GET", "node-pools/", answer(200, &json!([pool()]))),
        cluster_route("POST", "node-pools/", answer(201, &pool())),
        cluster_route(
            "GET",
            "operations/",
            answer(200, &json!([operation("blocked")])),
        ),
        cluster_route(
            "POST",
            "operations/12/cancel/",
            answer(200, &operation("cancelled")),
        ),
        cluster_route(
            "POST",
            "operations/12/resume/",
            answer(200, &operation("running")),
        ),
        cluster_route(
            "POST",
            "nodes/work-3a5bf1ed/remove/",
            answer(202, &operation("queued")),
        ),
    ]
}

/// One command of [`EVERY_COMMAND`]: its arguments, the read it sends before its question
/// (a path after `/api/kubernetes/clusters/`), and its own request, as the method, the path,
/// and the body.
type Command = (
    &'static [&'static str],
    Option<&'static str>,
    &'static str,
    &'static str,
    Option<&'static str>,
);

/// Every command once, confirmed and revealed where it needs it.
const EVERY_COMMAND: [Command; 12] = [
    (&["clusters", "list"], None, "GET", "", None),
    (&["clusters", "get", "7"], None, "GET", "7/", None),
    (
        &["clusters", "delete", "7", "--yes"],
        Some("7/"),
        "DELETE",
        "7/",
        None,
    ),
    (&["clusters", "events", "7"], None, "GET", "7/events/", None),
    (&["clusters", "refusals"], None, "GET", "refusals/", None),
    (
        &["backups", "list", "7", "--reveal"],
        None,
        "GET",
        "7/backups/",
        None,
    ),
    (
        &["node-pools", "list", "7"],
        None,
        "GET",
        "7/node-pools/",
        None,
    ),
    (
        &[
            "node-pools",
            "create",
            "7",
            "--name",
            "workers",
            "--flavor",
            "medium",
        ],
        None,
        "POST",
        "7/node-pools/",
        Some(r#"{"name":"workers","flavor":"medium","labels":{},"taints":[]}"#),
    ),
    (
        &["operations", "list", "7"],
        None,
        "GET",
        "7/operations/",
        None,
    ),
    (
        &["operations", "cancel", "7", "12", "--yes"],
        Some("7/operations/"),
        "POST",
        "7/operations/12/cancel/",
        Some("{}"),
    ),
    (
        &["operations", "resume", "7", "12", "--yes"],
        Some("7/operations/"),
        "POST",
        "7/operations/12/resume/",
        Some("{}"),
    ),
    (
        &["nodes", "delete", "7", "work-3a5bf1ed", "--yes"],
        None,
        "POST",
        "7/nodes/work-3a5bf1ed/remove/",
        Some("{}"),
    ),
];

/// How many reads [`EVERY_COMMAND`] sends before a question.
const READS: usize = 3;

fn argv<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut argv = vec!["compute", "kubernetes"];
    argv.extend(args);
    argv
}

fn hipc(server: &Server, args: &[&str]) -> Run {
    Hipc::against(&Home::new(), server).run(&argv(args))
}

fn message(error: &Value) -> &str {
    error["message"].as_str().expect("the message is a string")
}

#[test]
fn every_command_sends_its_requests_and_prints_the_answer() {
    let server = Server::start(every_route());
    let envelope = |item: Value| json!({ "items": [item], "next_page_token": null });
    let whole = |items: Value| json!({ "items": items, "next_page_token": null });
    let printed: [Value; EVERY_COMMAND.len()] = [
        envelope(cluster("running")),
        cluster("running"),
        cluster("decommissioning"),
        whole(events()),
        whole(refusals()),
        backups_envelope(),
        envelope(pool()),
        pool(),
        envelope(operation("blocked")),
        operation("cancelled"),
        operation("running"),
        operation("queued"),
    ];
    let mut requests = Vec::new();
    for ((args, read, method, suffix, body), document) in EVERY_COMMAND.iter().zip(printed) {
        assert_eq!(hipc(&server, args).ok(), document, "{args:?}");
        if let Some(read) = read {
            requests.push((args, "GET", *read, None));
        }
        requests.push((args, *method, *suffix, *body));
    }
    let recorded = server.requests();
    assert_eq!(recorded.len(), EVERY_COMMAND.len() + READS);
    for (request, (args, method, suffix, body)) in recorded.iter().zip(requests) {
        assert_eq!(request.method, method, "{args:?}");
        assert_eq!(
            request.path(),
            format!("/api/kubernetes/clusters/{suffix}"),
            "{args:?}"
        );
        match body {
            Some(body) => assert_eq!(request.json(), parsed(body), "{args:?}"),
            None => assert!(request.body.is_empty(), "{args:?}"),
        }
    }
}

/// Each list is a table in text mode: a header row of the item's fields, then one row per
/// item.
#[test]
fn every_list_in_text_mode_is_a_table() {
    let server = Server::start(every_route());
    for (args, header, row) in [
        (
            &["clusters", "list"][..],
            "id  name  mode",
            "7   prod  standard",
        ),
        (
            &["node-pools", "list", "7"],
            "id  name     role",
            "3   workers  worker",
        ),
        (
            &["operations", "list", "7"],
            "id  seq  kind",
            "12  4    remove_node",
        ),
        (
            &["clusters", "events", "7"],
            "at                    node  kind",
            "2026-10-27T10:00:00Z  m0    rke2.ready",
        ),
        (
            &["clusters", "refusals"],
            "name  mode      cp_size",
            "big   standard  l",
        ),
    ] {
        let mut text = vec!["--output", "text"];
        text.extend(argv(args));
        let run = Hipc::against(&Home::new(), &server).run(&text);
        assert_eq!(run.code(), Some(0), "{}", run.stderr());
        let lines: Vec<&str> = run.stdout().lines().collect();
        assert_eq!(lines.len(), 2, "{args:?}: {}", run.stdout());
        assert!(lines[0].starts_with(header), "{args:?}: {}", lines[0]);
        assert!(lines[1].starts_with(row), "{args:?}: {}", lines[1]);
    }
}

/// The five lists are read whole: a page with a `next` link would be printed as the whole
/// list, so it is `internal`; a last page keeps the server's other fields beside `items`.
#[test]
fn a_list_with_a_next_page_is_internal_and_a_last_page_keeps_its_fields() {
    let next = r#"{"count":2,"next":"{base}/api/kubernetes/clusters/?page=2","results":[]}"#;
    let last = r#"{"count":1,"next":null,"previous":null,"results":[{"id":3}]}"#;
    let server = Server::start(vec![
        route("GET", "/api/kubernetes/clusters/", Reply::json(200, next)),
        route(
            "GET",
            "/api/kubernetes/clusters/refusals/",
            Reply::json(200, last),
        ),
        cluster_route("GET", "node-pools/", Reply::json(200, last)),
        cluster_route("GET", "operations/", Reply::json(200, next)),
        cluster_route("GET", "events/", Reply::json(200, next)),
    ]);
    hipc(&server, &["clusters", "list"]).fails(1, "internal");
    hipc(&server, &["operations", "list", "7"]).fails(1, "internal");
    hipc(&server, &["clusters", "events", "7"]).fails(1, "internal");
    let kept = json!({ "count": 1, "items": [{ "id": 3 }], "next_page_token": null });
    assert_eq!(hipc(&server, &["node-pools", "list", "7"]).ok(), kept);
    assert_eq!(hipc(&server, &["clusters", "refusals"]).ok(), kept);
    // The read before a resume refuses a short list too, and sends no resume.
    hipc(&server, &["operations", "resume", "7", "12", "--yes"]).fails(1, "internal");
    assert_eq!(
        server.requests().len(),
        6,
        "no later page and no resume is sent"
    );
}

/// An empty journal and no refused launch are still envelopes, with no items.
#[test]
fn empty_events_and_refusals_are_envelopes() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/kubernetes/clusters/refusals/",
            Reply::json(200, "[]"),
        ),
        cluster_route("GET", "events/", Reply::json(200, "[]")),
    ]);
    let empty = json!({ "items": [], "next_page_token": null });
    assert_eq!(hipc(&server, &["clusters", "events", "7"]).ok(), empty);
    assert_eq!(hipc(&server, &["clusters", "refusals"]).ok(), empty);
}

#[test]
fn reads_map_their_errors_and_name_the_list() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/kubernetes/clusters/",
            Reply::json(503, r#"{"error":"down"}"#),
        ),
        cluster_route("GET", "", Reply::not_found()),
        cluster_route("GET", "events/", Reply::not_found()),
        cluster_route("GET", "backups/", Reply::not_found()),
        cluster_route("GET", "node-pools/", Reply::not_found()),
        cluster_route("GET", "operations/", Reply::not_found()),
    ]);
    let error = hipc(&server, &["clusters", "list"]).fails(1, "unavailable");
    assert_eq!(error["retryable"], true);
    for args in [
        &["clusters", "get", "7"][..],
        &["clusters", "events", "7"],
        &["backups", "list", "7", "--reveal"],
        &["node-pools", "list", "7"],
        &["operations", "list", "7"],
        // The reads before a question fail the same way, and nothing is written.
        &["clusters", "delete", "7", "--yes"],
        &["operations", "cancel", "7", "12", "--yes"],
    ] {
        let error = hipc(&server, args).fails(1, "not_found");
        assert_eq!(error["side_effects"], "none", "{args:?}");
        assert_eq!(
            error["hint"], "run `hipc compute kubernetes clusters list` to see what exists",
            "{args:?}"
        );
    }
    assert!(server.calls().iter().all(|r| r.starts_with("GET ")));
}

/// A 404 on a write names the list that holds what it named, and a 409 is `conflict`.
#[test]
fn writes_map_their_errors_and_name_the_list() {
    let server = Server::start(vec![
        cluster_route("GET", "", answer(200, &cluster("decommissioning"))),
        cluster_route(
            "DELETE",
            "",
            Reply::json(409, r#"{"error":"decommissioning"}"#),
        ),
        cluster_route(
            "GET",
            "operations/",
            answer(200, &json!([operation("blocked")])),
        ),
        cluster_route("POST", "operations/12/cancel/", Reply::not_found()),
        cluster_route("POST", "nodes/work-1/remove/", Reply::not_found()),
    ]);
    let error = hipc(&server, &["clusters", "delete", "7", "--yes"]).fails(1, "conflict");
    assert_eq!(error["side_effects"], "none");
    let error = hipc(&server, &["operations", "cancel", "7", "12", "--yes"]).fails(1, "not_found");
    assert_eq!(
        error["hint"],
        "run `hipc compute kubernetes operations list 7` to see what exists"
    );
    let error = hipc(&server, &["nodes", "delete", "7", "work-1", "--yes"]).fails(1, "not_found");
    assert_eq!(
        error["hint"],
        "run `hipc compute kubernetes node-pools list 7` to see what exists"
    );
}

/// An operation the list does not hold is `not_found` before the question, and nothing is
/// sent after the read.
#[test]
fn an_operation_the_list_does_not_hold_is_not_found() {
    let server = Server::start(vec![cluster_route(
        "GET",
        "operations/",
        answer(
            200,
            &json!([{ "id": 13, "kind": "add_node", "state": "queued" }]),
        ),
    )]);
    for verb in ["cancel", "resume"] {
        let error = hipc(&server, &["operations", verb, "7", "12"]).fails(1, "not_found");
        assert_eq!(
            error["message"],
            "the Kubernetes cluster `7` has no operation `12`"
        );
        assert_eq!(
            error["hint"],
            "run `hipc compute kubernetes operations list 7` to see what exists"
        );
    }
    assert_eq!(
        server.calls(),
        [
            "GET /api/kubernetes/clusters/7/operations/",
            "GET /api/kubernetes/clusters/7/operations/"
        ]
    );
}

#[test]
fn an_account_code_on_a_403_passes_through() {
    let owner = r#"{"code":"owner_only","error":"Only the owner can do this."}"#;
    let grant = r#"{"code":"insufficient_grant","error":"Operator or above."}"#;
    let server = Server::start(vec![
        cluster_route("GET", "", answer(200, &cluster("running"))),
        cluster_route("DELETE", "", Reply::json(403, owner)),
        cluster_route(
            "GET",
            "operations/",
            answer(200, &json!([operation("blocked")])),
        ),
        cluster_route("POST", "operations/12/cancel/", Reply::json(403, grant)),
    ]);
    let error = hipc(&server, &["clusters", "delete", "7", "--yes"]).fails(1, "owner_only");
    assert_eq!(error["http_status"], 403);
    assert_eq!(error["side_effects"], "none");
    hipc(&server, &["operations", "cancel", "7", "12", "--yes"]).fails(1, "insufficient_grant");
}

/// A resume the server refuses for any other reason comes back as sent, its body in
/// `details`; the request carried `{}`.
#[test]
fn a_refused_resume_surfaces_the_server_answer() {
    let refused = r#"{"error":"the operation is no longer blocked"}"#;
    let server = Server::start(vec![
        cluster_route(
            "GET",
            "operations/",
            answer(200, &json!([operation("blocked")])),
        ),
        cluster_route("POST", "operations/12/resume/", Reply::json(400, refused)),
    ]);
    let error = hipc(&server, &["operations", "resume", "7", "12", "--yes"]).fails(1, "validation");
    assert_eq!(error["details"], parsed(refused));
    assert_eq!(error["side_effects"], "none");
    assert!(
        message(&error).contains("the operation is no longer blocked"),
        "{error}"
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].path(),
        "/api/kubernetes/clusters/7/operations/12/resume/"
    );
    assert_eq!(requests[1].json(), json!({}));
}

/// A resume of an operation blocked on data the node would lose needs the console's signed
/// acceptance, so it is `conflict` after the read, with nothing written, and the hint names
/// the cancel with both ids; a cancel of it goes on.
#[test]
fn a_resume_of_a_local_data_block_is_refused_before_the_question() {
    let drain = json!({
        "id": 12,
        "kind": "remove_node",
        "state": "blocked",
        "step": "cordon_drain",
        "reason": "pgdata would be lost",
        "node": "work-1"
    });
    for blocked in [local_data_block(), drain] {
        let server = Server::start(vec![
            cluster_route("GET", "operations/", answer(200, &json!([blocked]))),
            cluster_route(
                "POST",
                "operations/12/cancel/",
                answer(200, &operation("cancelled")),
            ),
        ]);
        for extra in [&[][..], &["--yes"]] {
            let mut args = vec!["operations", "resume", "7", "12"];
            args.extend(extra);
            let error = hipc(&server, &args).fails(1, "conflict");
            assert_eq!(
                error["message"],
                "the operation `12` of the Kubernetes cluster `7` is blocked because data on \
                 the node would be lost, and only the console can sign the acceptance of that \
                 loss"
            );
            assert_eq!(
                error["hint"],
                "accept the data loss in the console, or run `hipc compute kubernetes \
                 operations cancel 7 12`"
            );
            assert_eq!(error["side_effects"], "none");
        }
        assert!(server.calls().iter().all(|r| r.starts_with("GET ")));
        hipc(&server, &["operations", "cancel", "7", "12", "--yes"]).ok();
        assert_eq!(server.requests().len(), 4);
    }
}

/// Only a `blocked` operation is blocked on data a node would lose: a `queued` one whose
/// `step` or `reason` still says so has not reached that check, so a resume of it is asked
/// about and sent as any other.
#[test]
fn a_queued_operation_is_not_blocked_on_data_loss() {
    let mut by_step = local_data_block();
    by_step["state"] = json!("queued");
    let mut by_reason = operation("queued");
    by_reason["reason"] = json!("pgdata would be lost");
    for op in [by_step, by_reason] {
        let server = Server::start(vec![
            cluster_route("GET", "operations/", answer(200, &json!([op]))),
            cluster_route(
                "POST",
                "operations/12/resume/",
                answer(200, &operation("running")),
            ),
        ]);
        let error =
            hipc(&server, &["operations", "resume", "7", "12"]).fails(2, "confirmation_required");
        assert!(
            message(&error)
                .starts_with("this would resume the `queued` `remove_node` operation `12`"),
            "{op}: {error}"
        );
        hipc(&server, &["operations", "resume", "7", "12", "--yes"]).ok();
        assert_eq!(
            server.calls(),
            [
                "GET /api/kubernetes/clusters/7/operations/",
                "GET /api/kubernetes/clusters/7/operations/",
                "POST /api/kubernetes/clusters/7/operations/12/resume/",
            ],
            "{op}"
        );
    }
}

/// The question names the operation's state: a resume of a `queued` or `blocked` operation,
/// and a cancel of a `queued`, `running`, or `blocked` one, each sent with `--yes`.
#[test]
fn the_question_names_the_state_of_the_operation() {
    let cases = [
        ("resume", "queued"),
        ("resume", "blocked"),
        ("cancel", "queued"),
        ("cancel", "running"),
        ("cancel", "blocked"),
    ];
    for (verb, state) in cases {
        let server = Server::start(vec![
            cluster_route(
                "GET",
                "operations/",
                answer(200, &json!([operation(state)])),
            ),
            cluster_route(
                "POST",
                &format!("operations/12/{verb}/"),
                answer(200, &operation("running")),
            ),
        ]);
        let error =
            hipc(&server, &["operations", verb, "7", "12"]).fails(2, "confirmation_required");
        assert!(
            message(&error).starts_with(&format!(
                "this would {verb} the `{state}` `remove_node` operation `12` of the Kubernetes \
                 cluster `7`, "
            )),
            "{verb} {state}: {error}"
        );
        hipc(&server, &["operations", verb, "7", "12", "--yes"]).ok();
        assert_eq!(
            server.calls(),
            [
                "GET /api/kubernetes/clusters/7/operations/".to_owned(),
                "GET /api/kubernetes/clusters/7/operations/".to_owned(),
                format!("POST /api/kubernetes/clusters/7/operations/12/{verb}/"),
            ],
            "{verb} {state}"
        );
    }
}

/// A state the verb cannot act on is `conflict` after the read, with or without `--yes`,
/// and nothing is sent after the read: a resume of an operation that is not `blocked` or
/// `queued`, and a cancel of one that is over.
#[test]
fn a_state_the_verb_cannot_act_on_is_a_conflict_before_the_question() {
    let no_state = json!({ "id": 12, "kind": "remove_node", "node": "work-1", "params": {} });
    let empty = |state: Value| json!({ "id": 12, "kind": "remove_node", "state": state });
    let cases = [
        ("resume", operation("running"), "its state is `running`"),
        ("resume", operation("done"), "its state is `done`"),
        ("resume", operation("failed"), "its state is `failed`"),
        ("resume", operation("cancelled"), "its state is `cancelled`"),
        ("resume", operation("paused"), "its state is `paused`"),
        ("resume", no_state, "the API gives it no state"),
        // An empty or null state is no state; neither prints as an empty literal.
        ("resume", empty(json!("")), "the API gives it no state"),
        ("resume", empty(Value::Null), "the API gives it no state"),
        // The state is checked before the data-loss block: a done operation whose step is
        // still `check_local_data` is refused for its state, whose hint names the list, not
        // for the data loss, whose hint would name a cancel a done operation then refuses.
        (
            "resume",
            json!({ "id": 12, "kind": "remove_node", "state": "done", "step": "check_local_data" }),
            "its state is `done`",
        ),
        ("cancel", operation("done"), "its state is `done`"),
        ("cancel", operation("failed"), "its state is `failed`"),
        ("cancel", operation("cancelled"), "its state is `cancelled`"),
    ];
    for (verb, op, why) in cases {
        let server = Server::start(vec![
            cluster_route("GET", "operations/", answer(200, &json!([op]))),
            cluster_route(
                "POST",
                &format!("operations/12/{verb}/"),
                answer(200, &operation("running")),
            ),
        ]);
        let rest = match verb {
            "resume" => "and only a `blocked` or `queued` operation can be resumed",
            _ => "so there is nothing left to cancel",
        };
        for extra in [&[][..], &["--yes"]] {
            let mut args = vec!["operations", verb, "7", "12"];
            args.extend(extra);
            let error = hipc(&server, &args).fails(1, "conflict");
            assert_eq!(
                error["message"],
                format!(
                    "cannot {verb} the operation `12` of the Kubernetes cluster `7`: {why}, \
                     {rest}"
                ),
                "{verb} {op}"
            );
            // No command changes a state the server reached, so there is no hint.
            assert_eq!(error["hint"], Value::Null, "{verb} {op}");
            assert_eq!(error["side_effects"], "none", "{verb} {op}");
        }
        assert_eq!(
            server.calls(),
            [
                "GET /api/kubernetes/clusters/7/operations/",
                "GET /api/kubernetes/clusters/7/operations/",
            ],
            "{verb} {op}"
        );
    }
    // A cancel without a state, or with an empty one, is left to the server.
    for op in [
        json!({ "id": 12, "kind": "remove_node", "node": "work-1", "params": {} }),
        empty(json!("")),
    ] {
        let server = Server::start(vec![
            cluster_route("GET", "operations/", answer(200, &json!([op]))),
            cluster_route(
                "POST",
                "operations/12/cancel/",
                answer(200, &operation("cancelled")),
            ),
        ]);
        hipc(&server, &["operations", "cancel", "7", "12", "--yes"]).ok();
        assert_eq!(
            server.calls(),
            [
                "GET /api/kubernetes/clusters/7/operations/",
                "POST /api/kubernetes/clusters/7/operations/12/cancel/",
            ],
            "{op}"
        );
    }
}

/// Without `--yes` a write sends nothing but the read that names its target, and the refusal
/// carries the whole action: the name, the state, the kind, the cost, and the data loss.
#[test]
fn a_write_without_yes_writes_nothing_and_says_what_it_would_do() {
    let server = Server::start(every_route());
    let cases: [(&[&str], &str); 4] = [
        (
            &["clusters", "delete", "7"],
            "delete the Kubernetes cluster `prod` (`7`): every VM in it stops and is \
             decommissioned, with the workloads and the data on it, and this cannot be undone",
        ),
        (
            &["operations", "cancel", "7", "12"],
            "cancel the `blocked` `remove_node` operation `12` of the Kubernetes cluster `7`, so \
             the node `work-3a5bf1ed` stays",
        ),
        (
            &["operations", "resume", "7", "12"],
            "resume the `blocked` `remove_node` operation `12` of the Kubernetes cluster `7`, \
             which drains the node `work-3a5bf1ed` and decommissions its VM",
        ),
        (
            &["nodes", "delete", "7", "work-3a5bf1ed"],
            "remove the node `work-3a5bf1ed` from the Kubernetes cluster `7`, draining its pods \
             to other nodes and decommissioning its VM; if data on the node would be lost, the \
             operation blocks until the loss is accepted in the console",
        ),
    ];
    for (args, action) in cases {
        let error = hipc(&server, args).fails(2, "confirmation_required");
        assert_eq!(
            error["message"],
            format!("this would {action}, and `--yes` was not passed"),
            "{args:?}"
        );
        assert!(
            error["hint"].as_str().expect("a hint").contains("--yes"),
            "{args:?}"
        );
        // Without a token, the token is checked before the read and the question.
        Hipc::tokenless(&Home::new(), &server)
            .run(&argv(args))
            .fails(1, "auth");
    }
    assert_eq!(
        server.calls(),
        [
            "GET /api/kubernetes/clusters/7/",
            "GET /api/kubernetes/clusters/7/operations/",
            "GET /api/kubernetes/clusters/7/operations/",
        ]
    );
}

/// A cluster the read finds without a name is named by its id alone.
#[test]
fn a_cluster_without_a_name_is_named_by_its_id() {
    let server = Server::start(vec![cluster_route(
        "GET",
        "",
        answer(200, &json!({ "id": 7 })),
    )]);
    let error = hipc(&server, &["clusters", "delete", "7"]).fails(2, "confirmation_required");
    assert!(
        message(&error).starts_with("this would delete the Kubernetes cluster `7`: every VM"),
        "{error}"
    );
}

#[test]
fn a_dry_run_lists_the_exact_requests_and_sends_nothing() {
    let server = Server::start(vec![]);
    let describe = |method: &str, suffix: &str, body: Value| {
        json!({
            "method": method,
            "url": format!("{}/api/kubernetes/clusters/{suffix}", server.url()),
            "account_header": null,
            "project_header": null,
            "billing_header": null,
            "body": body,
        })
    };
    for (args, read, method, suffix, body) in EVERY_COMMAND {
        let mut args = args.to_vec();
        args.retain(|word| *word != "--yes" && *word != "--reveal");
        args.push("--dry-run");
        let own = describe(method, suffix, body.map_or(Value::Null, parsed));
        let mut expected = one_request_dry_run(own.clone());
        if let Some(read) = read {
            expected["requests"] = json!([describe("GET", read, Value::Null), own]);
        }
        assert_eq!(hipc(&server, &args).dry_ok(), expected, "{args:?}");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn node_pool_create_sends_exactly_the_labels_and_taints() {
    let server = Server::start(vec![cluster_route(
        "POST",
        "node-pools/",
        answer(201, &pool()),
    )]);
    let create = |extra: &[&str]| {
        let mut args = vec![
            "node-pools",
            "create",
            "7",
            "--name",
            "gpu",
            "--flavor",
            "large",
        ];
        args.extend(extra);
        hipc(&server, &args)
    };
    create(&["--label", "a=b", "--taint", "t:NoSchedule"]).ok();
    create(&[
        "--label",
        "tier=gpu",
        "--label",
        "empty=",
        "--label",
        "x=y=z",
        "--taint",
        "gpu=true:NoSchedule",
        "--taint",
        "spot:NoExecute",
    ])
    .ok();
    let bodies: Vec<Value> = server.requests().iter().map(Recorded::json).collect();
    assert_eq!(
        bodies,
        [
            json!({ "name": "gpu", "flavor": "large", "labels": { "a": "b" }, "taints": ["t:NoSchedule"] }),
            json!({
                "name": "gpu",
                "flavor": "large",
                "labels": { "tier": "gpu", "empty": "", "x": "y=z" },
                "taints": ["gpu=true:NoSchedule", "spot:NoExecute"]
            }),
        ]
    );
    let before = server.requests().len();
    for extra in [
        &["--label", "a=b", "--label", "a=c"][..],
        &["--label", "a=1", "--label", "a=1", "--dry-run"],
        &["--label", "ab"],
        &["--label", "=b"],
    ] {
        let error = create(extra).fails(2, "usage");
        assert!(message(&error).contains("label"), "{extra:?}: {error}");
    }
    let error = create(&["--label", "a=b", "--label", "a=c"]).fails(2, "usage");
    assert_eq!(
        error["message"],
        "`--label` names the key `a` more than once"
    );
    let error = create(&["--label", "ab"]).fails(2, "usage");
    assert!(
        message(&error).ends_with("a label is `KEY=VALUE`, with a key before the `=`"),
        "{error}"
    );
    // A required flag left out.
    hipc(&server, &["node-pools", "create", "7", "--name", "gpu"]).fails(2, "usage");
    assert_eq!(server.requests().len(), before);
}

#[test]
fn a_bad_id_or_node_name_is_usage_with_zero_requests() {
    let server = Server::start(vec![]);
    for id in ["x", "-1", "1.5", "", "7/8", "18446744073709551616"] {
        for args in [
            &["clusters", "get", id][..],
            &["clusters", "delete", id, "--yes"],
            &["clusters", "events", id],
            &["backups", "list", id, "--reveal"],
            &["node-pools", "list", id],
            &[
                "node-pools",
                "create",
                id,
                "--name",
                "w",
                "--flavor",
                "medium",
                "--yes",
            ],
            &["operations", "list", id],
            &["operations", "cancel", id, "12", "--yes"],
            &["operations", "cancel", "7", id, "--yes"],
            &["operations", "resume", id, "12", "--yes"],
            &["operations", "resume", "7", id, "--yes"],
            &["nodes", "delete", id, "work-1", "--yes"],
        ] {
            hipc(&server, args).fails(2, "usage");
        }
    }
    let error = hipc(&server, &["operations", "cancel", "7", "x", "--yes"]).fails(2, "usage");
    assert!(
        message(&error).contains("from `hipc compute kubernetes operations list`"),
        "{error}"
    );
    let error = hipc(&server, &["clusters", "get", "x"]).fails(2, "usage");
    assert!(
        message(&error).contains("from `hipc compute kubernetes clusters list`"),
        "{error}"
    );
    let long = "n".repeat(65);
    for name in [
        "..",
        ".",
        "a/b",
        "../7",
        "a?b",
        "a#b",
        "a b",
        "a.b",
        "",
        long.as_str(),
    ] {
        for args in [
            &["nodes", "delete", "7", name, "--yes"][..],
            &["nodes", "delete", "7", name, "--dry-run"],
        ] {
            let error = hipc(&server, args).fails(2, "usage");
            assert!(
                message(&error).contains("a node name is"),
                "{name}: {error}"
            );
        }
    }
    hipc(
        &server,
        &["nodes", "delete", "7", &"n".repeat(64), "--dry-run"],
    )
    .ok();
    assert!(server.requests().is_empty());
}

/// Every route is account-scoped and takes a project: a membership and a project go on
/// every request, the reads before a question included, and a dry run names both.
#[test]
fn a_membership_and_a_project_go_on_every_route() {
    let server = Server::start(every_route());
    let home = Home::new();
    for (args, ..) in EVERY_COMMAND {
        Hipc::against(&home, &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .env("HIPPIUS_PROJECT", PROJECT)
            .run(&argv(args))
            .ok();
    }
    let requests = server.requests();
    assert_eq!(requests.len(), EVERY_COMMAND.len() + READS);
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
    }
    let dry = Hipc::against(&home, &server)
        .run(&argv(&[
            "clusters",
            "delete",
            "7",
            "--account",
            MEMBER,
            "--project",
            PROJECT,
            "--dry-run",
        ]))
        .ok();
    assert_eq!(draft(&dry)["account_header"], MEMBER);
    assert_eq!(draft(&dry)["project_header"], PROJECT);
    let error = Hipc::against(&home, &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&argv(&["nodes", "delete", "7", "work-3a5bf1ed"]))
        .fails(2, "confirmation_required");
    assert!(
        message(&error).contains(&format!(
            "accepted in the console in the account `{MEMBER}`"
        )),
        "{error}"
    );
    assert_eq!(server.requests().len(), EVERY_COMMAND.len() + READS);
}

/// The backups answer holds presigned links and sealed keys: without `--reveal` nothing is
/// sent, a dry run needs no flag, and with it the answer goes to stdout only.
#[test]
fn backups_need_reveal_and_print_the_links_on_stdout_only() {
    let server = Server::start(vec![cluster_route(
        "GET",
        "backups/",
        answer(200, &backups()),
    )]);
    let error = hipc(&server, &["backups", "list", "7"]).fails(2, "reveal_required");
    assert!(error["hint"].as_str().expect("a hint").contains("--reveal"));
    Hipc::tokenless(&Home::new(), &server)
        .run(&argv(&["backups", "list", "7"]))
        .fails(2, "reveal_required");
    let dry = hipc(&server, &["backups", "list", "7", "--dry-run"]).ok();
    assert_eq!(draft(&dry)["method"], "GET");
    assert!(server.requests().is_empty());
    for mode in ["json", "text"] {
        let run = Hipc::against(&Home::new(), &server).run(&[
            "--output",
            mode,
            "--debug",
            "compute",
            "kubernetes",
            "backups",
            "list",
            "7",
            "--reveal",
        ]);
        assert_eq!(run.code(), Some(0), "{}", run.stderr());
        for secret in [SIGNATURE, SEALED_KEY] {
            assert!(run.stdout().contains(secret), "{mode}: {}", run.stdout());
        }
        assert_eq!(
            run.stderr(),
            format!(
                "debug: GET {}/api/kubernetes/clusters/7/backups/\ndebug: -> 200\n",
                server.url()
            )
        );
    }
    let run = hipc(&server, &["backups", "list", "7", "--reveal"]);
    assert_eq!(run.ok(), backups_envelope());
    assert_eq!(run.stderr(), "");
}

/// `hipc api` needs `--reveal` on the backups route too, before anything is sent.
#[test]
fn hipc_api_needs_reveal_on_the_backups_route() {
    let server = Server::start(vec![cluster_route(
        "GET",
        "backups/",
        answer(200, &backups()),
    )]);
    let api = |extra: &[&str]| {
        let mut args = vec!["api", "GET", "/kubernetes/clusters/7/backups/"];
        args.extend(extra);
        Hipc::against(&Home::new(), &server).run(&args)
    };
    api(&[]).fails(2, "reveal_required");
    assert!(server.requests().is_empty());
    let run = api(&["--reveal"]);
    assert_eq!(run.ok(), backups());
    assert_eq!(server.requests().len(), 1);
}
