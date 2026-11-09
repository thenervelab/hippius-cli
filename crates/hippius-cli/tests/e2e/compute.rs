//! `hipc compute`: the catalog, VMs, the launch wait, and SSH keys.

use std::io::{BufRead as _, Read as _, Write as _};

use serde_json::{Value, json};

use crate::common::{Hipc, Home, MEMBER, Reply, Run, Server, draft, route};

const VM: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

/// A positive attestation, as swagger documents the fields.
const ATTESTED: &str = concat!(
    r#"{"verdict":"attested","attested":true,"attestation_status":"evidence-recorded","#,
    r#""kbs_evidence_error":null,"checked_at":"2026-10-27T10:00:00Z"}"#
);

/// No evidence recorded: unproven, not negative, and `attested` is null, never false.
const UNPROVEN: &str = concat!(
    r#"{"verdict":"unknown","attested":null,"attestation_status":"no-evidence-recorded","#,
    r#""kbs_evidence_error":null,"checked_at":"2026-10-27T10:00:00Z"}"#
);

fn vm(status: &str) -> String {
    format!(r#"{{"vm_id":"{VM}","name":"web","status":"{status}","flavor":"small"}}"#)
}

fn vm_route(method: &'static str, suffix: &str, reply: Reply) -> crate::common::Route {
    route(method, &format!("/api/compute/vms/{VM}/{suffix}"), reply)
}

fn hipc(server: &Server, args: &[&str]) -> Run {
    let mut argv = vec!["compute"];
    argv.extend(args);
    Hipc::against(&Home::new(), server).run(&argv)
}

fn create(server: &Server, extra: &[&str]) -> Run {
    let mut args = vec!["vms", "create", "--flavor", "small", "--ssh-key-id", "7"];
    args.extend(extra);
    hipc(server, &args)
}

/// The tenet 4 failure with the documented stdout exception: stdout is the last VM.
fn fails_with_vm(run: &Run, code: &str) -> Value {
    assert_eq!(run.code(), Some(1), "{}", run.stderr());
    let error: Value = serde_json::from_str(run.stderr()).expect("stderr is one JSON object");
    assert_eq!(error["error"]["code"], code, "{}", run.stderr());
    assert_eq!(error["error"]["side_effects"], "partial");
    assert!(
        error["error"]["hint"]
            .as_str()
            .expect("the failure has a hint")
            .contains(&format!("hipc compute vms get {VM}")),
        "{error}"
    );
    assert_eq!(run.json()["vm_id"], VM);
    run.json()
}

#[test]
fn objects_print_as_sent_and_lists_are_envelopes() {
    let images =
        r#"{"images":[{"name":"ubuntu-24.04","default_user":"ubuntu"}],"gateway_available":true}"#;
    let groups = r#"{"available":true,"groups":[{"name":"web","vms":2}]}"#;
    let server = Server::start(vec![
        route(
            "GET",
            "/api/compute/prices/",
            Reply::json(200, r#"{"currency":"USD","vm":{"small":1}}"#),
        ),
        route(
            "GET",
            "/api/compute/quota/",
            Reply::json(200, r#"{"runway":{"gate":true,"hours":24}}"#),
        ),
        route("GET", "/api/compute/images/", Reply::json(200, images)),
        route(
            "GET",
            "/api/compute/placement-groups/",
            Reply::json(200, groups),
        ),
        route(
            "GET",
            "/api/compute/flavors/",
            Reply::json(200, r#"[{"code":"small","vcpus":1}]"#),
        ),
        route(
            "GET",
            "/api/compute/regions/",
            Reply::json(200, r#"[{"code":"FR"},{"code":"AU"}]"#),
        ),
        route(
            "GET",
            "/api/compute/vms/",
            Reply::json(200, &format!("[{}]", vm("running"))),
        ),
    ]);
    assert_eq!(hipc(&server, &["prices"]).ok()["currency"], "USD");
    assert_eq!(hipc(&server, &["quota"]).ok()["runway"]["hours"], 24);
    assert_eq!(
        hipc(&server, &["images"]).ok(),
        serde_json::from_str::<Value>(images).unwrap()
    );
    assert_eq!(
        hipc(&server, &["placement-groups"]).ok(),
        serde_json::from_str::<Value>(groups).unwrap(),
        "the object, not a list envelope"
    );
    let flavors = hipc(&server, &["flavors"]).ok();
    assert_eq!(flavors["items"][0]["code"], "small");
    assert_eq!(flavors["next_page_token"], Value::Null);
    assert_eq!(hipc(&server, &["regions"]).ok()["items"][1]["code"], "AU");
    assert_eq!(
        hipc(&server, &["vms", "list"]).ok()["items"][0]["vm_id"],
        VM
    );
    for request in server.requests() {
        assert!(request.header("authorization").is_some());
    }
}

#[test]
fn catalog_errors_map_to_their_codes() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/compute/flavors/",
            Reply::json(403, r#"{"detail":"Compute is in private beta."}"#),
        ),
        route(
            "GET",
            "/api/compute/regions/",
            Reply::json(503, r#"{"detail":"down"}"#),
        ),
        route(
            "GET",
            "/api/compute/quota/",
            Reply::json(429, r#"{"detail":"slow down"}"#).header("Retry-After", "3"),
        ),
    ]);
    let error = hipc(&server, &["flavors"]).fails(1, "permission");
    assert_eq!(
        error["message"],
        "cannot list the VM flavors: the API says `Compute is in private beta.`"
    );
    hipc(&server, &["regions"]).fails(1, "unavailable");
    let error = hipc(&server, &["quota"]).fails(1, "rate_limited");
    assert_eq!(error["retryable"], true);
}

/// A raised cap with an applied raise request. The request has swagger's fields. Swagger
/// leaves `charge_cap` an untyped object, so its fields here are made up: `hipc` prints it as
/// sent.
const CHARGE_CAP: &str = concat!(
    r#"{"charge_cap":{"cap":"5000000","overridden":true},"request":{"id":7,"#,
    r#""status":"applied","trigger":"customer","target_cap":"5000000","#,
    r#""current_cap":"1000000","applied_cap":"5000000","#,
    r#""created_at":"2026-10-27T10:00:00Z","applied_at":"2026-10-27T10:05:00Z"}}"#
);

#[test]
fn charge_cap_prints_the_object_as_sent_nulls_included() {
    for body in [CHARGE_CAP, r#"{"charge_cap":null,"request":null}"#] {
        let server = Server::start(vec![route(
            "GET",
            "/api/compute/charge-cap/",
            Reply::json(200, body),
        )]);
        assert_eq!(
            hipc(&server, &["charge-cap"]).ok(),
            serde_json::from_str::<Value>(body).unwrap(),
            "the object, not a list envelope"
        );
        let request = server.only_request();
        assert!(request.header("authorization").is_some());
        assert_eq!(request.header("x-hippius-account"), None);
    }
}

#[test]
fn charge_cap_in_a_membership_sends_the_header_and_debug_shows_the_url() {
    let server = Server::start(vec![route(
        "GET",
        "/api/compute/charge-cap/",
        Reply::json(200, CHARGE_CAP),
    )]);
    let run = Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["compute", "charge-cap", "--debug"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(run.json()["request"]["status"], "applied");
    assert_eq!(
        run.stderr(),
        format!(
            "debug: GET {}/api/compute/charge-cap/\ndebug: -> 200\n",
            server.url()
        )
    );
    assert_eq!(
        server.only_request().header("x-hippius-account"),
        Some(MEMBER)
    );
}

#[test]
fn charge_cap_errors_map_to_their_codes() {
    for (status, body, code) in [
        (401, r#"{"detail":"Invalid token."}"#, "auth"),
        (403, r#"{"code":"not_a_member"}"#, "not_a_member"),
        (
            403,
            r#"{"code":"insufficient_grant"}"#,
            "insufficient_grant",
        ),
        (404, r#"{"detail":"Not found."}"#, "not_found"),
        (429, r#"{"detail":"slow down"}"#, "rate_limited"),
        (500, r#"{"detail":"boom"}"#, "internal"),
        (503, r#"{"detail":"down"}"#, "unavailable"),
    ] {
        let server = Server::start(vec![route(
            "GET",
            "/api/compute/charge-cap/",
            Reply::json(status, body),
        )]);
        let error = hipc(&server, &["charge-cap"]).fails(1, code);
        assert_eq!(error["http_status"], status, "{error}");
        assert!(
            error["message"]
                .as_str()
                .expect("the failure has a message")
                .starts_with("cannot get the compute charge cap"),
            "{error}"
        );
    }
}

/// The server's account-scope refusal is fixed by a flag, as the local one is: exit 2, with
/// the hint that names it. Without a membership the request carried no account header, so the
/// flag would change nothing and the hint does not name it.
#[test]
fn a_server_account_scope_refusal_exits_2_with_the_account_hint() {
    let server = Server::start(vec![route(
        "GET",
        "/api/compute/charge-cap/",
        Reply::json(403, r#"{"code":"account_scope_unsupported"}"#),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["compute", "charge-cap"])
        .fails(2, "account_scope_unsupported");
    assert_eq!(error["http_status"], 403, "{error}");
    assert_eq!(
        error["hint"],
        "pass `--account self` to act in the token's own account"
    );
    let error = hipc(&server, &["charge-cap"]).fails(2, "account_scope_unsupported");
    assert!(
        error["hint"]
            .as_str()
            .unwrap()
            .starts_with("the request carried no `X-Hippius-Account`"),
        "{error}"
    );
}

#[test]
fn a_membership_sends_the_header_on_compute_but_never_on_ssh_keys() {
    let server = Server::start(vec![
        route("GET", "/api/compute/vms/", Reply::json(200, "[]")),
        route(
            "GET",
            "/api/compute/placement-groups/",
            Reply::json(200, r#"{"available":false,"groups":[]}"#),
        ),
        vm_route("GET", "attestation/", Reply::json(200, ATTESTED)),
        route(
            "GET",
            "/api/ssh-keys/",
            Reply::json(
                200,
                r#"{"count":0,"next":null,"previous":null,"results":[]}"#,
            ),
        ),
    ]);
    for args in [
        &["vms", "list"][..],
        &["placement-groups"],
        &["vms", "attestation", VM],
        &["ssh-keys", "list"],
    ] {
        let mut argv = vec!["compute"];
        argv.extend(args);
        Hipc::against(&Home::new(), &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&argv)
            .ok();
    }
    let requests = server.requests();
    for scoped in &requests[..3] {
        assert_eq!(
            scoped.header("x-hippius-account"),
            Some(MEMBER),
            "{}",
            scoped.path()
        );
    }
    assert_eq!(requests[3].header("x-hippius-account"), None);
}

#[test]
fn get_is_the_vm_and_a_missing_one_is_not_found() {
    let server = Server::start(vec![vm_route("GET", "", Reply::json(200, &vm("running")))]);
    assert_eq!(hipc(&server, &["vms", "get", VM]).ok()["status"], "running");
    let missing = Server::start(vec![vm_route("GET", "", Reply::not_found())]);
    hipc(&missing, &["vms", "get", VM]).fails(1, "not_found");
}

#[test]
fn attestation_prints_the_server_object_unchanged() {
    for body in [ATTESTED, UNPROVEN] {
        let server = Server::start(vec![vm_route(
            "GET",
            "attestation/",
            Reply::json(200, body),
        )]);
        let doc = hipc(&server, &["vms", "attestation", VM]).ok();
        // `unknown` and a null `attested` stay as sent: JSON is the server's answer.
        assert_eq!(doc, serde_json::from_str::<Value>(body).unwrap());
        assert_eq!(
            server.only_request().path(),
            format!("/api/compute/vms/{VM}/attestation/")
        );
    }
}

#[test]
fn attestation_text_mode_reads_unknown_as_not_proven() {
    let text = |body: &str| {
        let server = Server::start(vec![vm_route(
            "GET",
            "attestation/",
            Reply::json(200, body),
        )]);
        let run = Hipc::against(&Home::new(), &server).run(&[
            "--output",
            "text",
            "compute",
            "vms",
            "attestation",
            VM,
        ]);
        assert_eq!(run.code(), Some(0), "{}", run.stderr());
        assert_eq!(run.stderr(), "");
        run.stdout().to_owned()
    };
    let unproven = text(UNPROVEN);
    let verdict = unproven
        .lines()
        .find(|line| line.starts_with("verdict"))
        .expect("a verdict line");
    assert!(verdict.ends_with("not proven"), "{unproven}");
    assert!(!unproven.contains("unknown"), "{unproven}");
    assert!(!unproven.contains("unattested"), "{unproven}");
    assert!(
        !unproven.contains("false"),
        "a null `attested` is not a no: {unproven}"
    );
    // The raw status is the server's own word, and is left as sent.
    assert!(unproven.contains("no-evidence-recorded"), "{unproven}");
    let attested = text(ATTESTED);
    let verdict = attested
        .lines()
        .find(|line| line.starts_with("verdict"))
        .expect("a verdict line");
    assert_eq!(
        verdict.split_whitespace().last(),
        Some("attested"),
        "{attested}"
    );
}

#[test]
fn attestation_errors_map_to_their_codes_and_a_dry_run_sends_nothing() {
    let missing = Server::start(vec![vm_route("GET", "attestation/", Reply::not_found())]);
    hipc(&missing, &["vms", "attestation", VM]).fails(1, "not_found");
    let down = Server::start(vec![vm_route(
        "GET",
        "attestation/",
        Reply::json(502, r#"{"error":"control plane unreachable"}"#),
    )]);
    let error = hipc(&down, &["vms", "attestation", VM]).fails(1, "unavailable");
    assert_eq!(error["retryable"], true, "a read can be retried");
    assert_eq!(error["side_effects"], "none");
    let dry = Server::start(vec![]);
    let doc = hipc(&dry, &["vms", "attestation", VM, "--dry-run"]).ok();
    assert_eq!(draft(&doc)["method"], "GET");
    assert_eq!(
        draft(&doc)["url"],
        format!("{}/api/compute/vms/{VM}/attestation/", dry.url())
    );
    assert!(dry.requests().is_empty());
}

#[test]
fn a_vm_id_that_could_change_the_route_is_usage_without_a_request() {
    let server = Server::start(vec![]);
    let long = "a".repeat(65);
    for id in ["", "../x", "a/b", "a?b", "a.b", "a b", "%2e", long.as_str()] {
        for command in ["get", "attestation", "start", "stop"] {
            hipc(&server, &["vms", command, id]).fails(2, "usage");
        }
        hipc(&server, &["vms", "delete", id, "--yes"]).fails(2, "usage");
    }
    assert!(server.requests().is_empty());
    let longest = "a".repeat(64);
    let ok = Server::start(vec![route(
        "GET",
        &format!("/api/compute/vms/{longest}/"),
        Reply::json(200, "{}"),
    )]);
    hipc(&ok, &["vms", "get", &longest]).ok();
}

#[test]
fn create_sends_exactly_the_flags_given() {
    let server = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(202, &vm("pending")),
    )]);
    create(&server, &["--no-wait", "--yes"]).ok();
    assert_eq!(
        server.only_request().json(),
        json!({"flavor": "small", "ssh_key_ids": [7]})
    );
    hipc(
        &server,
        &[
            "vms",
            "create",
            "--flavor",
            "large",
            "--name",
            "web",
            "--image",
            "ubuntu-24.04",
            "--region",
            "fr",
            "--ssh-key-id",
            "7",
            "--ssh-key-id",
            "9",
            "--no-wait",
            "--yes",
        ],
    )
    .ok();
    // The region is a country code, checked and sent in uppercase as every command that
    // takes one does.
    for bad in ["France", "F", "F1", ""] {
        create(&server, &["--region", bad, "--no-wait", "--yes"]).fails(2, "usage");
    }
    assert_eq!(server.requests().len(), 2);
    assert_eq!(
        server.requests()[1].json(),
        json!({
            "flavor": "large",
            "name": "web",
            "image": "ubuntu-24.04",
            "region": "FR",
            "ssh_key_ids": [7, 9]
        })
    );
}

#[test]
fn create_refusals_send_nothing() {
    let server = Server::start(vec![]);
    let error = create(&server, &[]).fails(2, "confirmation_required");
    assert!(error["message"].as_str().unwrap().contains("small"));
    for args in [
        &["vms", "create", "--flavor", "small", "--yes"][..],
        &["vms", "create", "--ssh-key-id", "7", "--yes"],
        &[
            "vms",
            "create",
            "--flavor",
            "small",
            "--ssh-key-id",
            "x",
            "--yes",
        ],
        &[
            "vms",
            "create",
            "--flavor",
            "small",
            "--ssh-key-id",
            "7",
            "--wait-timeout",
            "0",
            "--yes",
        ],
        &[
            "vms",
            "create",
            "--flavor",
            "small",
            "--ssh-key-id",
            "7",
            "--wait-timeout",
            "3601",
            "--yes",
        ],
        &[
            "vms",
            "create",
            "--flavor",
            "small",
            "--ssh-key-id",
            "7",
            "--no-wait",
            "--wait-timeout",
            "5",
            "--yes",
        ],
        &[
            "vms",
            "create",
            "--flavor",
            "small",
            "--ssh-key-id",
            "7",
            "--wait",
            "--no-wait",
            "--yes",
        ],
    ] {
        hipc(&server, args).fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn create_dry_run_shows_the_post_and_does_not_wait() {
    let server = Server::start(vec![]);
    let doc = create(&server, &["--dry-run"]).dry_ok();
    assert_eq!(
        doc,
        crate::common::one_request_dry_run(json!({
            "method": "POST",
            "url": format!("{}/api/compute/vms/", server.url()),
            "account_header": null,
            "project_header": null,
            "billing_header": null,
            "body": {"flavor": "small", "ssh_key_ids": [7]}
        }))
    );
    assert!(server.requests().is_empty());
}

/// Swagger: `ssh_key_ids` holds at most 20 ids, so a 21st is refused before anything is
/// sent rather than by the server after the confirmation.
#[test]
fn more_than_twenty_ssh_key_ids_is_usage_without_a_request() {
    let server = Server::start(vec![]);
    let keys = |count: usize| {
        let mut args = vec!["vms", "create", "--flavor", "small"];
        for _ in 0..count {
            args.extend(["--ssh-key-id", "7"]);
        }
        args
    };
    for flag in ["--dry-run", "--yes"] {
        let mut args = keys(21);
        args.push(flag);
        let error = hipc(&server, &args).fails(2, "usage");
        assert_eq!(
            error["message"],
            "`--ssh-key-id` is given 21 times, and a launch installs at most 20 stored keys"
        );
        assert_eq!(
            error["hint"],
            "pass at most 20 `--ssh-key-id`, and install more keys once the VM runs"
        );
    }
    assert!(server.requests().is_empty());
    let mut args = keys(20);
    args.push("--dry-run");
    let doc = hipc(&server, &args).ok();
    assert_eq!(draft(&doc)["body"]["ssh_key_ids"], json!(vec![7; 20]));
}

/// A launch needs a key, from either flag, and the refusal names both.
#[test]
fn a_launch_without_a_key_names_both_key_flags() {
    let server = Server::start(vec![]);
    let error = hipc(
        &server,
        &["vms", "create", "--flavor", "small", "--dry-run"],
    )
    .fails(2, "usage");
    let message = error["message"].as_str().expect("the message is a string");
    assert!(
        message.contains("--ssh-key-id") && message.contains("--ssh-public-key-file"),
        "{message}"
    );
    assert!(server.requests().is_empty());
}

#[test]
fn a_placement_group_adds_exactly_its_field_to_the_body() {
    let server = Server::start(vec![]);
    let doc = create(&server, &["--placement-group", "web", "--dry-run"]).ok();
    assert_eq!(
        draft(&doc)["body"],
        json!({"flavor": "small", "ssh_key_ids": [7], "placement_group": "web"})
    );
    let longest = "a".repeat(64);
    for name in ["0-db-9", "-", longest.as_str()] {
        let doc = create(&server, &["--placement-group", name, "--dry-run"]).ok();
        assert_eq!(draft(&doc)["body"]["placement_group"], name);
    }
    assert!(server.requests().is_empty());
    let sent = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(202, &vm("pending")),
    )]);
    create(&sent, &["--placement-group", "web", "--no-wait", "--yes"]).ok();
    assert_eq!(sent.only_request().json()["placement_group"], "web");
}

#[test]
fn a_bad_placement_group_is_usage_without_a_request() {
    let server = Server::start(vec![]);
    let long = "a".repeat(65);
    for name in [
        "Web_1",
        "Web",
        "a_b",
        "a.b",
        "a/b",
        "a b",
        "",
        "é",
        long.as_str(),
    ] {
        let error = create(&server, &["--placement-group", name, "--yes"]).fails(2, "usage");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("a placement group is 1 to 64"),
            "{error}"
        );
    }
    assert!(server.requests().is_empty());
}

/// The 503 `placement-groups-unavailable` stays `partial` and not retryable, as
/// every 5xx to a write, and its hint names the VM list to check before a retry. Another
/// 503 to the launch, or the same slug on another status, keeps the generic hint.
#[test]
fn placement_groups_unavailable_hints_to_check_the_vm_list() {
    let unavailable = r#"{"error":"placement-groups-unavailable"}"#;
    // Swagger names the slug, not its field, so a `code` carries it too.
    let under_code = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(503, r#"{"code":"placement-groups-unavailable"}"#),
    )]);
    let error = create(&under_code, &["--yes"]).fails(1, "unavailable");
    assert!(
        error["hint"]
            .as_str()
            .unwrap()
            .contains("hipc compute vms list")
    );
    let server = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(503, unavailable).header("Retry-After", "30"),
    )]);
    let error = create(&server, &["--placement-group", "web", "--yes"]).fails(1, "unavailable");
    assert_eq!(error["side_effects"], "partial");
    assert_eq!(error["retryable"], false);
    assert_eq!(error["retry_after_ms"], Value::Null);
    assert_eq!(error["http_status"], 503);
    assert_eq!(
        error["hint"],
        "run `hipc compute vms list` to check that no VM was launched before running the \
         command again"
    );
    assert_eq!(
        error["message"],
        "cannot launch a VM of flavor `small`: the API says `placement-groups-unavailable`"
    );

    let generic = "check whether the change was made before running the command again";
    for (status, body, code) in [
        (503, r#"{"error":"region-unavailable"}"#, "unavailable"),
        (
            503,
            r#"{"detail":"placement-groups-unavailable"}"#,
            "unavailable",
        ),
        (502, unavailable, "unavailable"),
    ] {
        let server = Server::start(vec![route(
            "POST",
            "/api/compute/vms/",
            Reply::json(status, body),
        )]);
        let error = create(&server, &["--yes"]).fails(1, code);
        assert_eq!(error["side_effects"], "partial", "{body}");
        assert_eq!(error["hint"], generic, "{body}");
    }
    let server = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(400, r#"{"error":"bad-placement-group"}"#),
    )]);
    let error = create(&server, &["--yes"]).fails(1, "validation");
    assert_eq!(error["hint"], Value::Null);
}

/// A launch failed for anti-affinity should keep the VM on stdout. Swagger's
/// `placement_group` field on `POST /api/compute/vms/` says such a launch "fails with
/// `placement-anti-affinity-unsatisfiable`"; which VM field carries that reason is not
/// documented, so `error` here is a guess. The wait reads only `status`, so the guess
/// cannot make this test pass wrongly.
#[test]
fn a_launch_that_breaks_anti_affinity_keeps_the_vm_on_stdout() {
    let failed = json!({
        "vm_id": VM,
        "status": "failed",
        "placement_group": "web",
        "error": "placement-anti-affinity-unsatisfiable",
    })
    .to_string();
    let server = Server::start(vec![
        route(
            "POST",
            "/api/compute/vms/",
            Reply::json(202, &vm("pending")),
        ),
        vm_route("GET", "", Reply::json(200, &failed)),
    ]);
    let run = create(&server, &["--wait", "--placement-group", "web", "--yes"]);
    let doc = fails_with_vm(&run, "partial_failure");
    assert_eq!(doc["error"], "placement-anti-affinity-unsatisfiable");
    assert_eq!(doc["placement_group"], "web");
}

#[test]
fn no_wait_prints_the_launch_answer_without_polling() {
    let server = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(202, &vm("pending")),
    )]);
    let doc = create(&server, &["--no-wait", "--yes"]).ok();
    assert_eq!(doc["status"], "pending");
    assert_eq!(server.requests().len(), 1);
}

/// A pipe is a program's stdout, so without `--wait` the launch is one POST and its 202
/// document, as `--no-wait` prints it.
#[test]
fn a_pipe_gets_the_launch_answer_without_a_wait() {
    let server = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(202, &vm("pending")),
    )]);
    let doc = create(&server, &["--yes"]).ok();
    assert_eq!(doc, serde_json::from_str::<Value>(&vm("pending")).unwrap());
    assert_eq!(server.only_request().method, "POST");
    // `--output text` changes the mode, not the reader: a pipe still gets no wait.
    let run = Hipc::against(&Home::new(), &server).run(&[
        "--output",
        "text",
        "compute",
        "vms",
        "create",
        "--flavor",
        "small",
        "--ssh-key-id",
        "7",
        "--yes",
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(run.stderr(), "", "no wait, so no note");
    assert_eq!(server.requests().len(), 2, "one POST per run");
}

/// `--wait-timeout` asks for a wait, so it waits on a pipe too, where the launch does not
/// wait by default: a flag wins over detection (tenet 15), and the timeout it names is the
/// one the wait uses. A dry run takes it as well.
#[test]
fn a_wait_timeout_waits_on_a_pipe_without_wait() {
    let server = Server::start(vec![
        route(
            "POST",
            "/api/compute/vms/",
            Reply::json(202, &vm("pending")),
        ),
        vm_route("GET", "", Reply::json(200, &vm("running"))),
    ]);
    let doc = create(&server, &["--wait-timeout", "5", "--yes"]).ok();
    assert_eq!(doc["status"], "running");
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].path(), format!("/api/compute/vms/{VM}/"));
    let still = Server::start(vec![
        route(
            "POST",
            "/api/compute/vms/",
            Reply::json(202, &vm("pending")),
        ),
        vm_route("GET", "", Reply::json(200, &vm("provisioning"))),
    ]);
    let run = create(&still, &["--wait-timeout", "1", "--yes"]);
    fails_with_vm(&run, "timeout");
    assert!(
        run.stderr().contains("after 1 second\""),
        "{}",
        run.stderr()
    );
    let dry = Server::start(vec![]);
    assert_eq!(
        create(&dry, &["--wait-timeout", "5", "--dry-run"]).ok()["requests"][0]["method"],
        "POST"
    );
    assert!(dry.requests().is_empty());
}

#[test]
fn a_launch_already_running_is_printed_without_polling() {
    let server = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(202, &vm("running")),
    )]);
    assert_eq!(
        create(&server, &["--wait", "--yes"]).ok()["status"],
        "running"
    );
    assert_eq!(server.requests().len(), 1);
}

/// `--wait` waits on a pipe, where the launch does not wait by default.
#[test]
fn the_wait_polls_the_vm_until_it_runs() {
    let server = Server::start(vec![
        route(
            "POST",
            "/api/compute/vms/",
            Reply::json(202, &vm("pending")),
        ),
        vm_route("GET", "", Reply::json(200, &vm("running"))),
    ]);
    let doc = create(&server, &["--wait", "--yes"]).ok();
    assert_eq!(doc["status"], "running");
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].method, "GET");
    assert_eq!(requests[1].path(), format!("/api/compute/vms/{VM}/"));
}

#[test]
fn a_wait_that_runs_out_is_timeout_and_keeps_the_vm_on_stdout() {
    // "booting" is not a status this version knows, so the wait goes on until the end.
    for status in ["provisioning", "booting"] {
        let server = Server::start(vec![
            route(
                "POST",
                "/api/compute/vms/",
                Reply::json(202, &vm("pending")),
            ),
            vm_route("GET", "", Reply::json(200, &vm(status))),
        ]);
        let run = create(&server, &["--wait", "--wait-timeout", "1", "--yes"]);
        assert_eq!(fails_with_vm(&run, "timeout")["status"], status);
        assert!(
            run.stderr().contains("after 1 second\""),
            "{}",
            run.stderr()
        );
        let run = create(&server, &["--wait", "--wait-timeout", "2", "--yes"]);
        fails_with_vm(&run, "timeout");
        assert!(
            run.stderr().contains("after 2 seconds\""),
            "{}",
            run.stderr()
        );
    }
}

#[test]
fn a_terminal_status_other_than_running_is_a_partial_failure_and_keeps_the_vm() {
    for status in ["failed", "decommissioning", "decommissioned"] {
        let server = Server::start(vec![route(
            "POST",
            "/api/compute/vms/",
            Reply::json(202, &vm(status)),
        )]);
        let run = create(&server, &["--wait", "--yes"]);
        assert_eq!(fails_with_vm(&run, "partial_failure")["status"], status);
        assert!(run.stderr().contains(status), "{}", run.stderr());
    }
}

/// Four 502s in a row: three are tolerated, the fourth ends the wait.
#[test]
fn a_failing_poll_keeps_the_last_vm_on_stdout() {
    let server = Server::start(vec![
        route(
            "POST",
            "/api/compute/vms/",
            Reply::json(202, &vm("pending")),
        ),
        vm_route("GET", "", Reply::json(502, r#"{"detail":"bad gateway"}"#)),
    ]);
    let run = create(&server, &["--wait", "--yes"]);
    assert_eq!(fails_with_vm(&run, "unavailable")["status"], "pending");
    assert_eq!(server.requests().len(), 5, "one launch and four polls");
    // The 502 alone would be retryable, but a retry of the create launches a second VM.
    let error: Value = serde_json::from_str(run.stderr()).unwrap();
    assert_eq!(error["error"]["retryable"], false);
    assert!(
        error["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("where the launch stands")
    );
}

#[test]
fn a_poll_answer_that_is_not_json_or_empty_keeps_the_last_vm() {
    for body in ["<html>", ""] {
        let server = Server::start(vec![
            route(
                "POST",
                "/api/compute/vms/",
                Reply::json(202, &vm("pending")),
            ),
            vm_route("GET", "", Reply::json(200, body)),
        ]);
        let run = create(&server, &["--wait", "--yes"]);
        assert_eq!(fails_with_vm(&run, "internal")["status"], "pending");
        let error: Value = serde_json::from_str(run.stderr()).unwrap();
        assert_eq!(error["error"]["retryable"], false);
    }
}

#[test]
fn a_launch_answer_without_a_status_keeps_waiting() {
    let server = Server::start(vec![
        route(
            "POST",
            "/api/compute/vms/",
            Reply::json(202, &format!(r#"{{"vm_id":"{VM}"}}"#)),
        ),
        vm_route("GET", "", Reply::json(200, &vm("running"))),
    ]);
    assert_eq!(
        create(&server, &["--wait", "--yes"]).ok()["status"],
        "running"
    );
    assert_eq!(server.requests().len(), 2);
}

/// The launch answer's `vm_id` is server text, so a backtick in it is escaped in the
/// message and the hint (tenet 15).
#[test]
fn a_backtick_in_the_launched_vm_id_cannot_end_the_quote() {
    let server = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(202, r#"{"vm_id":"v`x","status":"failed"}"#),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&[
        "compute",
        "vms",
        "create",
        "--wait",
        "--flavor",
        "small",
        "--ssh-key-id",
        "7",
        "--yes",
    ]);
    assert_eq!(run.code(), Some(1));
    // The launch went through, so the VM document stays on stdout.
    assert_eq!(
        serde_json::from_str::<Value>(run.stdout()).unwrap()["vm_id"],
        "v`x"
    );
    let error = serde_json::from_str::<Value>(run.stderr()).unwrap()["error"].clone();
    assert_eq!(error["code"], "partial_failure");
    assert_eq!(
        error["message"],
        r"the VM `v\u{60}x` reached `failed` instead of `running`"
    );
    // An id the CLI would not take as an argument is not put in a command to run.
    assert_eq!(
        error["hint"],
        "run `hipc compute vms list` to find the new VM"
    );
}

/// A hint is a command an agent runs as written (tenet 15), so a launched `vm_id` that is
/// not a VM id never reaches one: shell syntax in it would run.
#[test]
fn a_launched_vm_id_that_is_not_an_id_never_reaches_a_hint() {
    for id in [
        "x; hipc compute vms delete victim --yes",
        "$(hipc compute vms delete victim --yes)",
        "x\nhip compute vms delete victim --yes",
        "-x",
    ] {
        let body = serde_json::json!({ "vm_id": id, "status": "failed" }).to_string();
        let server = Server::start(vec![route(
            "POST",
            "/api/compute/vms/",
            Reply::json(202, &body),
        )]);
        let run = Hipc::against(&Home::new(), &server).run(&[
            "compute",
            "vms",
            "create",
            "--wait",
            "--flavor",
            "small",
            "--ssh-key-id",
            "7",
            "--yes",
        ]);
        assert_eq!(run.code(), Some(1), "{id}");
        let error = serde_json::from_str::<Value>(run.stderr()).unwrap()["error"].clone();
        assert_eq!(
            error["hint"], "run `hipc compute vms list` to find the new VM",
            "{id}"
        );
    }
    // A real id still gets the command that shows the VM.
    let server = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(202, r#"{"vm_id":"vm-1_a","status":"failed"}"#),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&[
        "compute",
        "vms",
        "create",
        "--wait",
        "--flavor",
        "small",
        "--ssh-key-id",
        "7",
        "--yes",
    ]);
    let error = serde_json::from_str::<Value>(run.stderr()).unwrap()["error"].clone();
    assert_eq!(
        error["hint"],
        "run `hipc compute vms get vm-1_a` to see where the launch stands"
    );
}

#[test]
fn text_mode_names_the_vm_before_the_wait_and_not_when_none_follows() {
    let server = Server::start(vec![
        route(
            "POST",
            "/api/compute/vms/",
            Reply::json(202, &vm("pending")),
        ),
        vm_route("GET", "", Reply::json(200, &vm("running"))),
    ]);
    let args = [
        "--output",
        "text",
        "compute",
        "vms",
        "create",
        "--wait",
        "--flavor",
        "small",
        "--ssh-key-id",
        "7",
        "--yes",
    ];
    let run = Hipc::against(&Home::new(), &server).run(&args);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        format!("note: launched the VM `{VM}`; waiting until it is running\n")
    );
    assert_eq!(server.requests().len(), 2, "the launch was polled once");
    let running = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(202, &vm("running")),
    )]);
    let run = Hipc::against(&Home::new(), &running).run(&args);
    assert_eq!(run.code(), Some(0));
    assert_eq!(run.stderr(), "", "nothing waits, so nothing is said");
    assert_eq!(
        running.requests().len(),
        1,
        "a running launch is not polled"
    );
    let failed = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(202, &vm("failed")),
    )]);
    let run = Hipc::against(&Home::new(), &failed).run(&args);
    assert_eq!(run.code(), Some(1));
    assert!(!run.stderr().contains("launched"), "{}", run.stderr());
    assert_eq!(failed.requests().len(), 1, "a failed launch is not polled");
}

#[test]
fn a_launch_answer_without_a_vm_id_is_internal_and_printed() {
    let server = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(202, r#"{"status":"pending"}"#),
    )]);
    let run = create(&server, &["--wait", "--yes"]);
    assert_eq!(run.code(), Some(1));
    assert_eq!(run.json(), json!({"status": "pending"}));
    let error: Value = serde_json::from_str(run.stderr()).unwrap();
    assert_eq!(error["error"]["code"], "internal");
    assert_eq!(error["error"]["side_effects"], "partial");
}

#[test]
fn a_refused_launch_prints_nothing_on_stdout() {
    let server = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(402, r#"{"detail":"Balance does not cover 24 hours"}"#),
    )]);
    let error = create(&server, &["--yes"]).fails(1, "payment_required");
    assert_eq!(error["side_effects"], "none");
    // The hint names the reads that tell how much balance a launch needs.
    assert_eq!(
        error["hint"],
        "run `hipc billing summary` and `hipc compute quota`, then top up until the balance \
         covers the runway hours at the hourly burn, this launch included"
    );
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn start_needs_no_confirmation_and_stop_needs_yes_and_warns_only_in_text_mode() {
    let server = Server::start(vec![
        vm_route("POST", "start/", Reply::json(200, &vm("running"))),
        vm_route("POST", "stop/", Reply::json(200, &vm("stopped"))),
    ]);
    assert_eq!(
        hipc(&server, &["vms", "start", VM]).ok()["status"],
        "running"
    );
    // A stop takes down what the VM serves, so without `--yes` nothing is sent.
    let refused = hipc(&server, &["vms", "stop", VM]).fails(2, "confirmation_required");
    assert_eq!(
        refused["message"],
        format!("this would stop the VM `{VM}`, and `--yes` was not passed")
    );
    assert_eq!(server.requests().len(), 1, "the refused stop sent nothing");
    assert_eq!(
        hipc(&server, &["vms", "stop", VM, "--yes"]).ok()["status"],
        "stopped"
    );
    let text = Hipc::against(&Home::new(), &server)
        .run(&["--output", "text", "compute", "vms", "stop", VM, "--yes"]);
    assert_eq!(text.code(), Some(0));
    assert_eq!(
        text.stderr(),
        format!(
            "warning: the VM `{VM}` is still billed while it is stopped, until it is deleted\n"
        )
    );
    let dry = Hipc::against(&Home::new(), &server).run(&[
        "--output",
        "text",
        "compute",
        "vms",
        "stop",
        VM,
        "--dry-run",
    ]);
    assert_eq!(dry.code(), Some(0));
    assert_eq!(dry.stderr(), "", "a dry run stops nothing");
    assert_eq!(server.requests().len(), 3);
    let methods: Vec<String> = server
        .requests()
        .iter()
        .map(|r| format!("{} {}", r.method, r.path()))
        .collect();
    assert_eq!(
        methods[..2],
        [
            format!("POST /api/compute/vms/{VM}/start/"),
            format!("POST /api/compute/vms/{VM}/stop/")
        ]
    );
}

#[test]
fn power_errors_map_to_their_codes() {
    let server = Server::start(vec![
        vm_route(
            "POST",
            "start/",
            Reply::json(409, r#"{"detail":"Already running"}"#),
        ),
        vm_route(
            "POST",
            "stop/",
            Reply::json(502, r#"{"detail":"node down"}"#),
        ),
    ]);
    hipc(&server, &["vms", "start", VM]).fails(1, "conflict");
    hipc(&server, &["vms", "stop", VM, "--yes"]).fails(1, "unavailable");
}

#[test]
fn reboot_and_delete_need_yes() {
    let server = Server::start(vec![
        vm_route("GET", "", Reply::json(200, &vm("running"))),
        vm_route("POST", "reboot/", Reply::json(200, &vm("running"))),
        vm_route("DELETE", "", Reply::json(202, &vm("decommissioning"))),
    ]);
    let error = hipc(&server, &["vms", "reboot", VM]).fails(2, "confirmation_required");
    assert!(error["message"].as_str().unwrap().contains(VM));
    assert!(server.requests().is_empty());
    // The delete reads the VM first, so the question names it, and then refuses.
    let error = hipc(&server, &["vms", "delete", VM]).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!(
            "this would delete the VM `web` (`{VM}`) and its disks, and `--yes` was not passed"
        )
    );
    let read = server.only_request();
    assert_eq!(read.method, "GET");
    assert_eq!(read.target, format!("/api/compute/vms/{VM}/"));
    hipc(&server, &["vms", "reboot", VM, "--yes"]).ok();
    let doc = hipc(&server, &["vms", "delete", VM, "--yes"]).ok();
    assert_eq!(doc["status"], "decommissioning");
    let sent: Vec<_> = server
        .requests()
        .into_iter()
        .map(|r| format!("{} {}", r.method, r.target))
        .collect();
    let vm_path = format!("/api/compute/vms/{VM}/");
    assert_eq!(
        sent,
        [
            format!("GET {vm_path}"),
            format!("POST {vm_path}reboot/"),
            format!("GET {vm_path}"),
            format!("DELETE {vm_path}"),
        ]
    );
    let gone = Server::start(vec![
        vm_route("GET", "", Reply::json(200, &vm("decommissioning"))),
        vm_route(
            "DELETE",
            "",
            Reply::json(409, r#"{"detail":"Already decommissioned"}"#),
        ),
    ]);
    hipc(&gone, &["vms", "delete", VM, "--yes"]).fails(1, "conflict");
}

#[test]
fn a_delete_of_a_vm_with_no_name_names_its_id_and_a_failed_read_deletes_nothing() {
    let server = Server::start(vec![vm_route(
        "GET",
        "",
        Reply::json(200, &format!(r#"{{"vm_id":"{VM}","status":"running"}}"#)),
    )]);
    let error = hipc(&server, &["vms", "delete", VM]).fails(2, "confirmation_required");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with(&format!("this would delete the VM `{VM}` and its disks,")),
        "{error}"
    );
    // A VM the read cannot find is not deleted: the read's error is the command's.
    let missing = Server::start(vec![vm_route("GET", "", Reply::not_found())]);
    let error = hipc(&missing, &["vms", "delete", VM, "--yes"]).fails(1, "not_found");
    assert_eq!(error["side_effects"], "none");
    let sent = missing.only_request();
    assert_eq!(sent.method, "GET");
}

#[test]
fn a_delete_dry_run_lists_the_read_and_the_delete_and_sends_nothing() {
    let server = Server::start(vec![]);
    let doc = hipc(&server, &["vms", "delete", VM, "--dry-run"]).dry_ok();
    let url = format!("{}/api/compute/vms/{VM}/", server.url());
    let request = |method: &str| {
        json!({
            "method": method,
            "url": url,
            "account_header": null,
            "project_header": null,
            "billing_header": null,
            "body": null,
        })
    };
    let expected = json!({
        "dry_run": true,
        "requests": [request("GET"), request("DELETE")],
    });
    assert_eq!(doc, expected);
    assert!(server.requests().is_empty());
}

#[test]
fn ssh_keys_list_follows_drf_pages() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/ssh-keys/?page_size=100",
            Reply::json(
                200,
                concat!(
                    r#"{"count":2,"next":"{base}/api/ssh-keys/?page=2&page_size=100","#,
                    r#""previous":null,"results":[{"id":1}]}"#
                ),
            ),
        ),
        route(
            "GET",
            "/api/ssh-keys/?page_size=100&page=2",
            Reply::json(
                200,
                r#"{"count":2,"next":null,"previous":null,"results":[{"id":2}]}"#,
            ),
        ),
    ]);
    let page = hipc(&server, &["ssh-keys", "list"]).ok();
    assert_eq!(page["items"], json!([{"id": 1}]));
    assert_eq!(page["next_page_token"], "2");
    let all = hipc(&server, &["ssh-keys", "list", "--all"]).ok();
    assert_eq!(all, json!([{"id": 1}, {"id": 2}]));
}

#[test]
fn ssh_keys_create_uploads_the_public_key_file() {
    let server = Server::start(vec![route(
        "POST",
        "/api/ssh-keys/",
        Reply::json(201, r#"{"id":3,"name":"laptop"}"#),
    )]);
    let home = Home::new();
    let key = home.file("id.pub", b"ssh-ed25519 AAAAC3Nza me@laptop\n");
    let key = key.to_str().unwrap();
    let args = [
        "compute",
        "ssh-keys",
        "create",
        "--name",
        "laptop",
        "--public-key-file",
        key,
    ];
    Hipc::against(&home, &server)
        .run(&args)
        .fails(2, "confirmation_required");
    assert!(server.requests().is_empty());
    let mut yes = args.to_vec();
    yes.push("--yes");
    assert_eq!(Hipc::against(&home, &server).run(&yes).ok()["id"], 3);
    assert_eq!(
        server.only_request().json(),
        json!({"name": "laptop", "public_key": "ssh-ed25519 AAAAC3Nza me@laptop"})
    );
    Hipc::against(&home, &server)
        .stdin(b"ssh-rsa AAAAB3 other\n")
        .run(&[
            "compute",
            "ssh-keys",
            "create",
            "--name",
            "stdin",
            "--public-key-file",
            "-",
            "--yes",
        ])
        .ok();
    assert_eq!(
        server.requests()[1].json()["public_key"],
        "ssh-rsa AAAAB3 other"
    );
}

#[test]
fn ssh_keys_create_refuses_a_private_key_or_a_bad_file_without_sending() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let private = home.file(
        "id",
        concat!(
            "-----BEGIN OPENSSH PRIVATE KEY-----\n",
            "b3BlbnNzaC1rZXktdjE\n",
            "-----END OPENSSH PRIVATE KEY-----\n"
        )
        .as_bytes(),
    );
    let empty = home.file("empty.pub", b"  \n");
    let two = home.file("two.pub", b"ssh-ed25519 A one\nssh-ed25519 B two\n");
    let missing = home.path().join("nope.pub");
    // A one-line file that is not an OpenSSH key, such as a token, is never sent, nor is a
    // key whose type OpenSSH does not write, even through a symlink.
    let token = home.file("token", b"tok_ab12cd34ef56\n");
    let unknown = home.file("unknown.pub", b"ssh-foo AAAA me@laptop\n");
    let not_base64 = home.file("text.pub", b"ssh-ed25519 tok_ab12cd34ef56! me\n");
    let link = home.path().join("link.pub");
    std::os::unix::fs::symlink(&token, &link).unwrap();
    for (path, wanted) in [
        (&private, "private key"),
        (&empty, "one line"),
        (&two, "one line"),
        (&missing, "cannot read"),
        (&token, "OpenSSH format"),
        (&unknown, "OpenSSH format"),
        (&not_base64, "OpenSSH format"),
        (&link, "OpenSSH format"),
    ] {
        let run = Hipc::against(&home, &server).run(&[
            "compute",
            "ssh-keys",
            "create",
            "--name",
            "k",
            "--public-key-file",
            path.to_str().unwrap(),
            "--yes",
        ]);
        let error = run.fails(2, "usage");
        assert!(
            error["message"].as_str().unwrap().contains(wanted),
            "{error}"
        );
        assert!(!run.stderr().contains("b3BlbnNzaC1rZXktdjE"));
        assert!(!run.stderr().contains("tok_ab12cd34ef56"));
    }
    assert!(server.requests().is_empty());
}

/// `hipc compute vms create` with `home`, so the key files written there can be named.
fn create_in(home: &Home, server: &Server, args: &[&str]) -> Run {
    let mut argv = vec!["compute", "vms", "create", "--flavor", "small"];
    argv.extend(args);
    Hipc::against(home, server).run(&argv)
}

/// `--public-ip` and `--ssh-public-key-file` add exactly their fields; the key file stands
/// alone or beside `--ssh-key-id`, as swagger combines them.
#[test]
fn create_sends_a_public_ip_and_a_public_key_only_when_given() {
    let none = Server::start(vec![]);
    let home = Home::new();
    let key = home.file("id.pub", b"  ssh-ed25519 AAAAC3Nza me@laptop\n");
    let key = key.to_str().unwrap();
    let pub_key = "ssh-ed25519 AAAAC3Nza me@laptop";
    for (args, body) in [
        (
            &["--public-ip", "--ssh-key-id", "7"][..],
            json!({"flavor": "small", "ssh_key_ids": [7], "public_ip": true}),
        ),
        (
            &["--ssh-public-key-file", key],
            json!({"flavor": "small", "ssh_public_key": pub_key}),
        ),
        (
            &[
                "--ssh-key-id",
                "7",
                "--ssh-public-key-file",
                key,
                "--public-ip",
            ],
            json!({
                "flavor": "small",
                "ssh_key_ids": [7],
                "ssh_public_key": pub_key,
                "public_ip": true
            }),
        ),
    ] {
        let mut args = args.to_vec();
        args.push("--dry-run");
        assert_eq!(
            create_in(&home, &none, &args).ok()["requests"][0]["body"],
            body,
            "{args:?}"
        );
    }
    assert!(none.requests().is_empty());

    let server = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(202, &vm("pending")),
    )]);
    let args = [
        "--ssh-public-key-file",
        key,
        "--public-ip",
        "--no-wait",
        "--yes",
    ];
    create_in(&home, &server, &args).ok();
    assert_eq!(
        server.only_request().json(),
        json!({"flavor": "small", "ssh_public_key": pub_key, "public_ip": true})
    );

    // Swagger: with no free address the launch is refused with 409 `no-free-public-ip`
    // before anything is bought, so it is a plain `conflict` with the body kept.
    let full = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(409, r#"{"error":"no-free-public-ip"}"#),
    )]);
    let error = create(&full, &["--public-ip", "--no-wait", "--yes"]).fails(1, "conflict");
    assert_eq!(error["http_status"], 409);
    assert_eq!(error["details"]["error"], "no-free-public-ip");
}

/// `vms create --flavor small --ssh-key-id 7 <extra>` in text mode against a server that
/// answers the launch with `answer`.
fn launch_in_text(answer: &str, extra: &[&str]) -> Run {
    let server = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(202, answer),
    )]);
    let mut args = vec!["--output", "text"];
    args.extend(extra);
    create(&server, &args)
}

/// A launch with `--public-ip` warns in text mode, with the words of `public-ip create`,
/// that the address blocks inbound traffic; JSON mode and a launch without the flag say
/// nothing, and stdout is the launch answer either way.
#[test]
fn a_launch_with_a_public_ip_says_inbound_is_blocked_only_in_text_mode() {
    let blocked = format!(
        "warning: all inbound traffic to the address is blocked until a firewall rule accepts \
         it, which `hipc compute firewall update {VM} --rules-file <path>` adds\n"
    );
    let run = launch_in_text(&vm("pending"), &["--public-ip", "--no-wait", "--yes"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(run.stderr(), blocked);
    assert!(run.stdout().contains(VM), "{}", run.stdout());
    let run = launch_in_text(&vm("pending"), &["--no-wait", "--yes"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(run.stderr(), "", "no warning without `--public-ip`");
    // A launch answer with no usable `vm_id` puts a placeholder in the command.
    let run = launch_in_text(
        r#"{"vm_id":"a;b","status":"pending"}"#,
        &["--public-ip", "--no-wait", "--yes"],
    );
    assert!(
        run.stderr()
            .contains("`hipc compute firewall update <vm_id> --rules-file <path>`"),
        "{}",
        run.stderr()
    );

    let server = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(202, &vm("pending")),
    )]);
    let doc = create(&server, &["--public-ip", "--no-wait", "--yes"]).ok();
    assert_eq!(
        doc["vm_id"], VM,
        "JSON mode: stdout is the answer, stderr empty"
    );
}

/// When the launch answer carries `public_ip_error`, text mode warns that the VM runs
/// without the address and quotes the server's words; stdout is the answer unchanged, and
/// JSON mode says nothing on stderr.
#[test]
fn a_launch_whose_address_failed_says_so_and_quotes_the_server() {
    let answer = format!(
        concat!(
            r#"{{"vm_id":"{}","status":"pending","public_ip":null,"#,
            r#""public_ip_error":"no `free` address"}}"#
        ),
        VM
    );
    let run = launch_in_text(&answer, &["--public-ip", "--no-wait", "--yes"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        "warning: the VM launched without its public IPv4: the API says `no \\u{60}free\\u{60} \
         address`\n"
    );
    assert!(
        run.stdout().contains("no `free` address"),
        "{}",
        run.stdout()
    );
    let server = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(202, &answer),
    )]);
    let doc = create(&server, &["--public-ip", "--no-wait", "--yes"]).ok();
    assert_eq!(doc, serde_json::from_str::<Value>(&answer).unwrap());
}

/// With a wait, the printed document is the polled VM, not the 202. The failed attach is
/// reported whether the polled VM or only the 202 carries `public_ip_error`, and never
/// as an address that blocks inbound traffic.
#[test]
fn a_launch_that_waits_reports_a_failed_address_from_either_answer() {
    let failed = |status: &str| {
        format!(r#"{{"vm_id":"{VM}","status":"{status}","public_ip_error":"none free"}}"#)
    };
    let warned = "warning: the VM launched without its public IPv4: the API says `none free`\n";
    for (answer, polled) in [
        (vm("pending"), failed("running")),
        (failed("pending"), vm("running")),
    ] {
        let server = Server::start(vec![
            route("POST", "/api/compute/vms/", Reply::json(202, &answer)),
            vm_route("GET", "", Reply::json(200, &polled)),
        ]);
        let args = ["--output", "text", "--public-ip", "--wait", "--yes"];
        let run = create(&server, &args);
        assert_eq!(run.code(), Some(0), "{}", run.stderr());
        assert!(run.stderr().ends_with(warned), "{answer}: {}", run.stderr());
        assert!(
            !run.stderr().contains("inbound"),
            "{answer}: {}",
            run.stderr()
        );
        assert_eq!(server.requests().len(), 2, "{answer}");
    }
}

/// The refusal with `--public-ip` names the address and where its hourly price is shown,
/// and sends nothing: not the launch, and not a read of the price.
#[test]
fn create_with_a_public_ip_names_its_price_in_the_refusal() {
    let server = Server::start(vec![]);
    let error = create(&server, &["--public-ip"]).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        "this would launch a VM of flavor `small` with a public IPv4, billed per second at the \
         price that `hipc compute public-ip availability` shows (`price_hourly`, or free until \
         `free_until` while it is set, then `price_hourly_after_free`), and `--yes` was not \
         passed"
    );
    let error = create(&server, &[]).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        "this would launch a VM of flavor `small`, and `--yes` was not passed"
    );
    assert!(server.requests().is_empty());
}

/// `--ssh-public-key-file` has the checks of `ssh-keys create`: a private key, an empty
/// file, several lines, or a file that cannot be read is refused before anything is sent,
/// a dry run included, and the hint names the flag that was passed.
#[test]
fn create_refuses_a_private_key_or_a_bad_key_file_without_sending() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let private = home.file(
        "id",
        concat!(
            "-----BEGIN OPENSSH PRIVATE KEY-----\n",
            "b3BlbnNzaC1rZXktdjE\n",
            "-----END OPENSSH PRIVATE KEY-----\n"
        )
        .as_bytes(),
    );
    let empty = home.file("empty.pub", b"  \n");
    let two = home.file("two.pub", b"ssh-ed25519 A one\nssh-ed25519 B two\n");
    let missing = home.path().join("nope.pub");
    // A one-line file that is not an OpenSSH key, such as a token, is never sent, nor is a
    // key whose type OpenSSH does not write, even through a symlink.
    let token = home.file("token", b"tok_ab12cd34ef56\n");
    let unknown = home.file("unknown.pub", b"ssh-foo AAAA me@laptop\n");
    let not_base64 = home.file("text.pub", b"ssh-ed25519 tok_ab12cd34ef56! me\n");
    let link = home.path().join("link.pub");
    std::os::unix::fs::symlink(&token, &link).unwrap();
    for (path, wanted) in [
        (&private, "private key"),
        (&empty, "one line"),
        (&two, "one line"),
        (&missing, "cannot read"),
        (&token, "OpenSSH format"),
        (&unknown, "OpenSSH format"),
        (&not_base64, "OpenSSH format"),
        (&link, "OpenSSH format"),
    ] {
        for last in ["--yes", "--dry-run"] {
            let path = path.to_str().unwrap();
            let args = ["--ssh-key-id", "7", "--ssh-public-key-file", path, last];
            let run = create_in(&home, &server, &args);
            let error = run.fails(2, "usage");
            assert!(
                error["message"].as_str().unwrap().contains(wanted),
                "{error}"
            );
            assert!(
                error["hint"]
                    .as_str()
                    .unwrap()
                    .contains("`--ssh-public-key-file`"),
                "{error}"
            );
            assert!(!run.stderr().contains("b3BlbnNzaC1rZXktdjE"));
            assert!(!run.stderr().contains("tok_ab12cd34ef56"));
        }
    }
    // No key flag at all, and a key-file flag with no path, are argv errors.
    create_in(&home, &server, &["--public-ip", "--yes"]).fails(2, "usage");
    create_in(&home, &server, &["--ssh-public-key-file"]).fails(2, "usage");
    assert!(server.requests().is_empty());
}

#[test]
fn ssh_keys_delete_needs_yes_and_names_the_key_on_204() {
    let server = Server::start(vec![route(
        "DELETE",
        "/api/ssh-keys/3/",
        Reply::json(204, ""),
    )]);
    hipc(&server, &["ssh-keys", "delete", "3"]).fails(2, "confirmation_required");
    assert!(server.requests().is_empty());
    let doc = hipc(&server, &["ssh-keys", "delete", "3", "--yes"]).ok();
    assert_eq!(doc, json!({"id": 3, "deleted": true}));
    hipc(&server, &["ssh-keys", "delete", "x", "--yes"]).fails(2, "usage");
    hipc(&server, &["ssh-keys", "delete", "-1", "--yes"]).fails(2, "usage");
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn ssh_keys_delete_prints_a_body_when_there_is_one() {
    let server = Server::start(vec![route(
        "DELETE",
        "/api/ssh-keys/3/",
        Reply::json(200, r#"{"id":3,"deleted":true,"note":"x"}"#),
    )]);
    let doc = hipc(&server, &["ssh-keys", "delete", "3", "--yes"]).ok();
    assert_eq!(doc["note"], "x");
}

#[test]
fn these_compute_commands_dry_run_without_a_request() {
    let server = Server::start(vec![]);
    for args in [
        &["prices"][..],
        &["quota"],
        &["charge-cap"],
        &["flavors"],
        &["images"],
        &["apps"],
        &["regions"],
        &["feasibility", "--flavor", "small"],
        &["account-ssh-keys"],
        &["vms", "list"],
        &["vms", "get", VM],
        &["vms", "failovers", VM],
        &["vms", "start", VM],
        &["vms", "stop", VM],
        &["vms", "reboot", VM],
        &["vms", "resize-options", VM],
        &["vms", "resize", VM, "--flavor", "large"],
        &["vms", "resize-status", VM, "7"],
        &["vms", "delete", VM],
        &["public-ip", "availability"],
        &["public-ip", "availability", "--region", "FR"],
        &["public-ip", "get", VM],
        &["public-ip", "create", VM],
        &["public-ip", "delete", VM],
        &["public-ip", "usage", VM],
        &["public-ip", "usage", VM, "--range", "7d"],
        &["firewall", "get", VM],
        &["exposures", "list", VM],
        &["exposures", "create", VM, "--port", "8080"],
        &[
            "exposures",
            "create",
            VM,
            "--port",
            "53",
            "--mode",
            "raw",
            "--protocol",
            "udp",
        ],
        &["exposures", "delete", VM, "7"],
        &["backups", "options"],
        &["backups", "policy", "get", VM],
        &["backups", "policy", "update", VM, "--interval", "900"],
        &["backups", "policy", "delete", VM],
        &["backups", "list", VM],
        &["backups", "restore", VM, "--run-id", "run_1"],
        &["backups", "restore-status", VM],
        &["runners", "settings", "get"],
        &["runners", "settings", "update", "--enabled", "true"],
        &["runners", "installations", "list"],
        &["runners", "installations", "delete", "7"],
        &["runners", "jobs"],
        &["runners", "connect"],
        &["ssh-keys", "list"],
        &["ssh-keys", "get", "3"],
        &["ssh-keys", "delete", "3"],
        &["databases", "list"],
        &["databases", "get", "7"],
        &[
            "databases",
            "create",
            "--tier",
            "ha",
            "--size",
            "xl",
            "--engine-version",
            "16",
        ],
        &["databases", "delete", "7"],
        &["databases", "restart", "7"],
        &["databases", "restore", "7"],
        &["databases", "exposure", "create", "7"],
        &["databases", "exposure", "delete", "7"],
        &["databases", "events", "7"],
        &["databases", "metrics", "7"],
        &["databases", "refusals"],
    ] {
        let mut argv = args.to_vec();
        argv.push("--dry-run");
        assert_eq!(hipc(&server, &argv).ok()["dry_run"], true, "{args:?}");
    }
    let home = Home::new();
    let key = home.file("id.pub", b"ssh-ed25519 AAAA me@laptop\n");
    let doc = Hipc::against(&home, &server)
        .run(&[
            "compute",
            "ssh-keys",
            "create",
            "--name",
            "k",
            "--public-key-file",
            key.to_str().unwrap(),
            "--dry-run",
        ])
        .ok();
    let key = &draft(&doc)["body"]["public_key"];
    assert!(key == "ssh-ed25519 AAAA me@laptop" && server.requests().is_empty());
}

#[test]
fn compute_needs_a_token() {
    let server = Server::start(vec![]);
    Hipc::new(&Home::new())
        .env("HIPPIUS_API_URL", server.url())
        .run(&["compute", "prices"])
        .fails(1, "auth");
    assert!(server.requests().is_empty());
}

/// Every `.rs` file under `dir`, recursively.
fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read a source directory") {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_source_encodes_a_price() {
    // A scan of the built binary is not used: the Linux debug build holds `$18` in bytes
    // that are not strings hipc prints, so the sources are the reliable place to look.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    sources(&root.join("hippius-cli/src"), &mut files);
    sources(&root.join("hippius/src"), &mut files);
    assert!(files.len() > 10, "found the sources: {files:?}");
    // The documented compute and CDN amounts, which must never be written in code. Prices
    // come from `hipc compute prices` and `hipc cdn prices`, never from the code.
    for file in &files {
        let text = std::fs::read_to_string(file).expect("read a source file");
        for amount in [
            "$18", "$2.50", "$0.10", "$32", "$0.0438", "$1024", "$0.005", "$0.010", "$0.060",
            "$0.002", "$1 per", "$20",
        ] {
            assert!(!text.contains(amount), "{amount} in {}", file.display());
        }
    }
}

#[test]
fn a_list_read_whole_that_the_server_paginates_is_internal() {
    let server = Server::start(vec![route(
        "GET",
        "/api/compute/vms/",
        Reply::json(
            200,
            r#"{"count":3,"next":"{base}/api/compute/vms/?page=2","previous":null,"results":[{}]}"#,
        ),
    )]);
    let error = hipc(&server, &["vms", "list"]).fails(1, "internal");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("split a list into pages"),
        "{error}"
    );
}

#[test]
fn a_launch_answered_2xx_with_an_unreadable_body_is_partial() {
    let server = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(202, "<html>ok</html>"),
    )]);
    let error = create(&server, &["--yes"]).fails(1, "internal");
    assert_eq!(error["side_effects"], "partial", "the VM was launched");
    assert_eq!(error["http_status"], 202);
    assert_eq!(error["retryable"], false);
    // Not "run the command again": that would launch, and bill, a second VM.
    assert_eq!(error["hint"], hippius::CHECK_FIRST_HINT);
}

#[test]
fn a_launch_answered_5xx_is_partial_and_not_retryable() {
    let server = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(503, r#"{"detail":"busy"}"#),
    )]);
    let error = create(&server, &["--yes"]).fails(1, "unavailable");
    assert_eq!(error["side_effects"], "partial");
    assert_eq!(
        error["retryable"], false,
        "a retry could launch a second VM"
    );
    // A read that gets a 5xx did nothing, so it may be retried.
    let read = Server::start(vec![route(
        "GET",
        "/api/compute/vms/",
        Reply::json(503, r#"{"detail":"busy"}"#),
    )]);
    let error = hipc(&read, &["vms", "list"]).fails(1, "unavailable");
    assert_eq!(error["side_effects"], "none");
    assert_eq!(error["retryable"], true);
}

/// The server's `vm_id` goes into the poll URL, so it is held to the `vm_id` argument's
/// rule before the first poll, and a launch that went through never exits 2 (tenet 4).
#[test]
fn a_launched_vm_whose_id_cannot_be_polled_is_internal_not_usage() {
    for id in ["a b", "a?b=1", "a/b", "..", ""] {
        let launched = json!({ "vm_id": id, "status": "pending" }).to_string();
        let server = Server::start(vec![route(
            "POST",
            "/api/compute/vms/",
            Reply::json(202, &launched),
        )]);
        let run = create(&server, &["--wait", "--yes"]);
        assert_eq!(run.code(), Some(1), "a launch went through: never exit 2");
        assert_eq!(run.json()["vm_id"], id);
        let error: Value = serde_json::from_str(run.stderr()).unwrap();
        assert_eq!(error["error"]["code"], "internal");
        assert_eq!(error["error"]["side_effects"], "partial");
        assert_eq!(
            error["error"]["message"],
            format!(
                "the launch answer names the VM `{id}`, which `hipc` cannot poll: a VM id is 1 \
                 to 64 letters, digits, `-`, and `_`"
            )
        );
        assert_eq!(
            error["error"]["hint"],
            "run `hipc compute vms list` to find the new VM"
        );
        assert_eq!(server.requests().len(), 1, "{id}: no poll is sent");
    }
}

/// A poll answer for another VM, or for none, is not this launch: the wait ends `internal`
/// with the last document of this VM on stdout, rather than print the other one.
#[test]
fn a_poll_answer_for_another_vm_ends_the_wait() {
    for (answer, named) in [
        (
            r#"{"vm_id":"vm_zzz","status":"running"}"#,
            "the VM `vm_zzz`",
        ),
        (r#"{"status":"running"}"#, "no VM"),
        (r#"{"vm_id":7,"status":"running"}"#, "no VM"),
    ] {
        let server = Server::start(vec![
            route(
                "POST",
                "/api/compute/vms/",
                Reply::json(202, &vm("pending")),
            ),
            vm_route("GET", "", Reply::json(200, answer)),
        ]);
        let run = create(&server, &["--wait", "--yes"]);
        let printed = fails_with_vm(&run, "internal");
        assert_eq!(
            printed["status"], "pending",
            "{answer}: this VM's last document"
        );
        let stderr: Value = serde_json::from_str(run.stderr()).unwrap();
        assert_eq!(
            stderr["error"]["message"],
            format!("the poll of the VM `{VM}` answered with the document of {named}")
        );
        assert_eq!(server.requests().len(), 2);
    }
}

#[test]
fn a_confirmation_in_a_membership_names_the_account() {
    let server = Server::start(vec![vm_route("GET", "", Reply::json(200, &vm("running")))]);
    let error = Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["compute", "vms", "delete", VM])
        .fails(2, "confirmation_required");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains(&format!("and its disks in the account `{MEMBER}`, and")),
        "{error}"
    );
    let own = Hipc::against(&Home::new(), &server)
        .run(&["compute", "vms", "delete", VM])
        .fails(2, "confirmation_required");
    assert!(!own["message"].as_str().unwrap().contains("in the account"));
    // Each run read the VM to name it, and deleted nothing.
    assert!(server.requests().iter().all(|r| r.method == "GET"));
}

#[test]
fn a_page_number_with_a_leading_zero_is_the_same_page() {
    let server = Server::start(vec![route(
        "GET",
        "/api/ssh-keys/",
        Reply::json(
            200,
            r#"{"next":"{base}/api/ssh-keys/?page=01","previous":null,"results":[]}"#,
        ),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["compute", "ssh-keys", "list", "--all"])
        .fails(1, "internal");
    assert!(
        error["message"].as_str().unwrap().contains("leads back to"),
        "{error}"
    );
}

#[test]
fn a_page_token_with_a_leading_zero_asks_for_that_page() {
    let server = Server::start(vec![route(
        "GET",
        "/api/ssh-keys/",
        Reply::json(
            200,
            r#"{"count":0,"next":null,"previous":null,"results":[]}"#,
        ),
    )]);
    hipc(&server, &["ssh-keys", "list", "--page-token", "007"]).ok();
    assert!(
        server.only_request().target.contains("page=7"),
        "{}",
        server.only_request().target
    );
}

/// A server that answers each connection with the next of `replies` (status, body), in
/// order, and returns the request lines it saw.
fn in_sequence(replies: Vec<(u16, String)>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a local port");
    let url = format!("http://{}", listener.local_addr().expect("a bound address"));
    let handle = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for (status, body) in replies {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            // The whole request, headers and body, before the answer.
            let mut reader = std::io::BufReader::new(&stream);
            let mut line = String::new();
            drop(reader.read_line(&mut line));
            seen.push(line.trim_end().to_owned());
            let mut length = 0;
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                    break;
                }
                if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut request_body = vec![0_u8; length];
            drop(reader.read_exact(&mut request_body));
            drop(reader);
            let head = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n",
                body.len()
            );
            drop(stream.write_all(format!("{head}{body}").as_bytes()));
        }
        seen
    });
    (url, handle)
}

#[test]
fn a_passing_poll_failure_is_tolerated_and_the_wait_goes_on() {
    let (url, server) = in_sequence(vec![
        (202, vm("pending")),
        (502, r#"{"detail":"bad gateway"}"#.to_owned()),
        (200, vm("running")),
    ]);
    let doc = Hipc::new(&Home::new())
        .env("HIPPIUS_API_URL", &url)
        .with_env_token(&url, crate::common::TOKEN)
        .run(&[
            "compute",
            "vms",
            "create",
            "--wait",
            "--flavor",
            "small",
            "--ssh-key-id",
            "7",
            "--yes",
        ])
        .ok();
    assert_eq!(doc["status"], "running");
    assert_eq!(server.join().unwrap().len(), 3);
}

#[test]
fn a_non_retryable_poll_failure_ends_the_wait_at_once() {
    let (url, server) = in_sequence(vec![
        (202, vm("pending")),
        (404, r#"{"detail":"Not found."}"#.to_owned()),
    ]);
    let run = Hipc::new(&Home::new())
        .env("HIPPIUS_API_URL", &url)
        .with_env_token(&url, crate::common::TOKEN)
        .run(&[
            "compute",
            "vms",
            "create",
            "--wait",
            "--flavor",
            "small",
            "--ssh-key-id",
            "7",
            "--yes",
        ]);
    assert_eq!(fails_with_vm(&run, "not_found")["status"], "pending");
    assert_eq!(server.join().unwrap().len(), 2, "no poll after a 404");
}

#[test]
fn a_success_resets_the_count_of_failed_polls() {
    let bad = || (502, r#"{"detail":"bad gateway"}"#.to_owned());
    let (url, server) = in_sequence(vec![
        (202, vm("pending")),
        bad(),
        (200, vm("provisioning")),
        bad(),
        bad(),
        bad(),
        (200, vm("running")),
    ]);
    let doc = Hipc::new(&Home::new())
        .env("HIPPIUS_API_URL", &url)
        .with_env_token(&url, crate::common::TOKEN)
        .run(&[
            "compute",
            "vms",
            "create",
            "--wait",
            "--flavor",
            "small",
            "--ssh-key-id",
            "7",
            "--yes",
        ])
        .ok();
    assert_eq!(doc["status"], "running");
    assert_eq!(server.join().unwrap().len(), 7);
}

fn resize(status: &str) -> String {
    format!(
        concat!(
            r#"{{"resize":{{"id":7,"status":"{status}","#,
            r#""from_flavor":"small","to_flavor":"large","rolled_back":false}}}}"#
        ),
        status = status
    )
}

#[test]
fn resize_options_and_status_print_the_object_as_sent() {
    let options = concat!(
        r#"{"options":[{"flavor":{"code":"large"},"available":true}],"#,
        r#""blocked":null,"cooldown_until":null,"in_flight":null}"#
    );
    let server = Server::start(vec![
        vm_route("GET", "resize/", Reply::json(200, options)),
        vm_route("GET", "resize/7/", Reply::json(200, &resize("done"))),
    ]);
    assert_eq!(
        hipc(&server, &["vms", "resize-options", VM]).ok(),
        serde_json::from_str::<Value>(options).unwrap()
    );
    let status = hipc(&server, &["vms", "resize-status", VM, "7"]).ok();
    assert_eq!(status["resize"]["status"], "done");
    let paths: Vec<String> = server
        .requests()
        .iter()
        .map(|r| format!("{} {}", r.method, r.path()))
        .collect();
    assert_eq!(
        paths,
        [
            format!("GET /api/compute/vms/{VM}/resize/"),
            format!("GET /api/compute/vms/{VM}/resize/7/")
        ]
    );
    let missing = Server::start(vec![vm_route("GET", "resize/8/", Reply::not_found())]);
    hipc(&missing, &["vms", "resize-status", VM, "8"]).fails(1, "not_found");
}

#[test]
fn resize_needs_yes_and_says_the_vm_reboots() {
    let server = Server::start(vec![vm_route(
        "POST",
        "resize/",
        Reply::json(202, &resize("pending")),
    )]);
    let error = hipc(&server, &["vms", "resize", VM, "--flavor", "large"])
        .fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!("this would resize the VM `{VM}` to `large`, and `--yes` was not passed")
    );
    assert!(server.requests().is_empty());
    let doc = hipc(
        &server,
        &["vms", "resize", VM, "--flavor", "large", "--yes"],
    )
    .ok();
    assert_eq!(doc["resize"]["id"], 7);
    // The command does not wait: one POST, and no poll of the resize.
    let request = server.only_request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path(), format!("/api/compute/vms/{VM}/resize/"));
    assert_eq!(request.json(), json!({ "flavor": "large" }));
}

#[test]
fn resize_dry_run_shows_the_flavor_body_without_a_request() {
    let server = Server::start(vec![]);
    let doc = hipc(
        &server,
        &["vms", "resize", VM, "--flavor", "large", "--dry-run"],
    )
    .ok();
    assert_eq!(draft(&doc)["method"], "POST");
    assert_eq!(draft(&doc)["body"], json!({ "flavor": "large" }));
    assert_eq!(
        draft(&doc)["url"],
        format!("{}/api/compute/vms/{VM}/resize/", server.url())
    );
    assert!(server.requests().is_empty());
}

#[test]
fn resize_errors_map_to_their_codes() {
    let cases: [(u16, &str, &str); 6] = [
        (400, r#"{"error":"same-flavor"}"#, "validation"),
        (400, r#"{"error":"unknown-flavor"}"#, "validation"),
        (402, r#"{"error":"Balance too low"}"#, "payment_required"),
        (409, r#"{"error":"resize-no-capacity"}"#, "conflict"),
        (
            429,
            r#"{"error":"resize-cooldown","retry_after_s":600}"#,
            "rate_limited",
        ),
        (503, r#"{"error":"resize-unavailable"}"#, "unavailable"),
    ];
    for (status, body, code) in cases {
        let server = Server::start(vec![vm_route("POST", "resize/", Reply::json(status, body))]);
        let error = hipc(
            &server,
            &["vms", "resize", VM, "--flavor", "large", "--yes"],
        )
        .fails(1, code);
        assert_eq!(error["http_status"], status, "{body}");
        match status {
            // Refused before any work, so a retry is safe. The body's `retry_after_s` is
            // not the contract's `retry_after_ms`: only a `Retry-After` header sets it.
            429 => {
                assert_eq!(error["retryable"], true);
                assert_eq!(error["retry_after_ms"], Value::Null);
                assert_eq!(error["side_effects"], "none");
            }
            // A 5xx to a write may have started the resize, so a retry could do it twice.
            503 => {
                assert_eq!(error["retryable"], false);
                assert_eq!(error["side_effects"], "partial");
            }
            _ => {
                assert_eq!(error["retryable"], false, "{body}");
                assert_eq!(error["side_effects"], "none", "{body}");
            }
        }
    }
}

#[test]
fn a_resize_cooldown_with_retry_after_sets_the_delay() {
    let server = Server::start(vec![vm_route(
        "POST",
        "resize/",
        Reply::json(429, r#"{"error":"resize-cooldown","retry_after_s":600}"#)
            .header("Retry-After", "600"),
    )]);
    let error = hipc(
        &server,
        &["vms", "resize", VM, "--flavor", "large", "--yes"],
    )
    .fails(1, "rate_limited");
    assert_eq!(error["retry_after_ms"], 600_000);
}

#[test]
fn a_resize_id_that_is_not_a_number_is_usage_without_a_request() {
    let server = Server::start(vec![]);
    for id in ["", "x", "-1", "7/../8", "7.0", "1e3"] {
        hipc(&server, &["vms", "resize-status", VM, id]).fails(2, "usage");
    }
    for id in ["../x", "a/b", "a?b"] {
        hipc(&server, &["vms", "resize-options", id]).fails(2, "usage");
        hipc(&server, &["vms", "resize-status", id, "7"]).fails(2, "usage");
        hipc(
            &server,
            &["vms", "resize", id, "--flavor", "large", "--yes"],
        )
        .fails(2, "usage");
    }
    hipc(&server, &["vms", "resize", VM, "--yes"]).fails(2, "usage");
    assert!(server.requests().is_empty());
}

#[test]
fn a_membership_sends_the_header_on_every_resize_route() {
    let server = Server::start(vec![
        vm_route("GET", "resize/", Reply::json(200, "{}")),
        vm_route("POST", "resize/", Reply::json(202, &resize("pending"))),
        vm_route("GET", "resize/7/", Reply::json(200, &resize("stopping"))),
    ]);
    let member = |args: &[&str]| {
        let mut argv = vec!["compute", "vms"];
        argv.extend(args);
        Hipc::against(&Home::new(), &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&argv)
    };
    member(&["resize-options", VM]).ok();
    member(&["resize", VM, "--flavor", "large", "--yes"]).ok();
    member(&["resize-status", VM, "7"]).ok();
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    for request in requests {
        assert_eq!(request.header("x-hippius-account"), Some(MEMBER));
    }
    let dry = member(&["resize", VM, "--flavor", "large", "--dry-run"]).ok();
    assert_eq!(draft(&dry)["account_header"], MEMBER);
    let error = member(&["resize", VM, "--flavor", "large"]).fails(2, "confirmation_required");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains(&format!("to `large` in the account `{MEMBER}`, and")),
        "{error}"
    );
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn resize_prints_a_text_block_in_text_mode() {
    let server = Server::start(vec![
        vm_route("POST", "resize/", Reply::json(202, &resize("pending"))),
        vm_route("GET", "resize/7/", Reply::json(200, &resize("relaunching"))),
    ]);
    // The test points `hipc` at the server with `HIPPIUS_API_URL`, which the note repeats.
    let next = format!(
        "note: `hipc --api-url {} compute vms resize-status {VM} 7` shows its progress\n",
        server.url()
    );
    for (args, stderr) in [
        (
            &["vms", "resize", VM, "--flavor", "large", "--yes"][..],
            &*next,
        ),
        (&["vms", "resize-status", VM, "7"], ""),
    ] {
        let mut argv = vec!["--output", "text", "compute"];
        argv.extend(args);
        let run = Hipc::against(&Home::new(), &server).run(&argv);
        assert_eq!(run.code(), Some(0), "{}", run.stderr());
        assert_eq!(run.stderr(), stderr);
        assert!(
            !run.stdout().trim_start().starts_with('{'),
            "{}",
            run.stdout()
        );
        assert!(run.stdout().contains("large"), "{}", run.stdout());
    }
}

/// A VM document with `status` and the extra fields of `fields`, such as `network_ready`
/// or `boot_stalled` (swagger, `GET /api/compute/vms/{vm_id}/`).
fn vm_with(status: &str, fields: Value) -> String {
    let mut doc: Value = serde_json::from_str(&vm(status)).expect("the fixture is JSON");
    let Value::Object(fields) = fields else {
        panic!("the fields are an object: {fields}");
    };
    for (key, value) in fields {
        doc[key] = value;
    }
    doc.to_string()
}

/// `vms create --flavor small --ssh-key-id 7 --wait --yes <extra>` against a server that
/// answers each request with the next of `replies`, and the request lines it saw.
///
/// When hipc stops before every reply was taken, empty connections take the rest, so the
/// server thread ends and the test fails on its assertions instead of hanging; they are
/// left out of the lines returned.
fn launch_in_sequence(replies: Vec<(u16, String)>, extra: &[&str]) -> (Run, Vec<String>) {
    let (url, server) = in_sequence(replies);
    let address = url.trim_start_matches("http://").to_owned();
    let mut args = vec![
        "compute",
        "vms",
        "create",
        "--flavor",
        "small",
        "--ssh-key-id",
        "7",
        "--wait",
        "--yes",
    ];
    args.extend(extra);
    let run = Hipc::new(&Home::new())
        .env("HIPPIUS_API_URL", &url)
        .with_env_token(&url, crate::common::TOKEN)
        .run(&args);
    while !server.is_finished() {
        drop(std::net::TcpStream::connect(&address));
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let seen = server.join().expect("the server thread ends");
    (
        run,
        seen.into_iter().filter(|line| !line.is_empty()).collect(),
    )
}

/// With `--public-ip` the wait goes on past `running` until `network_ready` is true, which
/// swagger says can lag the opened path by up to about 30 seconds.
#[test]
fn a_public_ip_launch_waits_for_network_ready_that_arrives_late() {
    let (run, seen) = launch_in_sequence(
        vec![
            (202, vm_with("pending", json!({"network_ready": null}))),
            (200, vm_with("running", json!({"network_ready": null}))),
            (200, vm_with("running", json!({"network_ready": false}))),
            (200, vm_with("running", json!({"network_ready": true}))),
        ],
        &["--public-ip"],
    );
    let doc = run.ok();
    assert_eq!(doc["status"], "running");
    assert_eq!(doc["network_ready"], true);
    assert_eq!(seen.len(), 4, "{seen:?}");
    assert!(
        seen[1..]
            .iter()
            .all(|line| line.starts_with(&format!("GET /api/compute/vms/{VM}/ "))),
        "{seen:?}"
    );
}

/// Without `--public-ip`, `running` ends the wait whatever `network_ready` says.
#[test]
fn a_launch_without_a_public_ip_does_not_wait_for_network_ready() {
    let (run, seen) = launch_in_sequence(
        vec![
            (202, vm("pending")),
            (200, vm_with("running", json!({"network_ready": false}))),
        ],
        &[],
    );
    assert_eq!(run.ok()["network_ready"], false);
    assert_eq!(seen.len(), 2, "{seen:?}");
}

/// A launch answer already running and reachable needs no poll; one running but not yet
/// reachable is polled, and text mode says what the wait is for.
#[test]
fn a_public_ip_launch_answer_is_polled_only_until_it_is_reachable() {
    let ready = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(
            202,
            &vm_with(
                "running",
                json!({"network_ready": true, "public_ip": "203.0.113.7"}),
            ),
        ),
    )]);
    let doc = create(&ready, &["--public-ip", "--wait", "--yes"]).ok();
    assert_eq!(doc["network_ready"], true);
    assert_eq!(ready.requests().len(), 1);

    let server = Server::start(vec![
        route(
            "POST",
            "/api/compute/vms/",
            Reply::json(202, &vm_with("running", json!({"network_ready": false}))),
        ),
        vm_route(
            "GET",
            "",
            Reply::json(200, &vm_with("running", json!({"network_ready": true}))),
        ),
    ]);
    let args = ["--output", "text", "--public-ip", "--wait", "--yes"];
    let run = create(&server, &args);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(
        run.stderr().starts_with(&format!(
            "note: launched the VM `{VM}`; waiting until it is running and its public IPv4 is \
             reachable\n"
        )),
        "{}",
        run.stderr()
    );
    assert_eq!(server.requests().len(), 2);
}

/// `--wait-timeout` bounds the whole wait, the part after `running` included: a VM that runs
/// but whose address never turns `network_ready` is `timeout`, with the VM on stdout and a
/// message that says what was missing.
#[test]
fn a_wait_for_network_ready_that_runs_out_is_timeout_and_keeps_the_vm() {
    let server = Server::start(vec![
        route(
            "POST",
            "/api/compute/vms/",
            Reply::json(202, &vm("pending")),
        ),
        vm_route(
            "GET",
            "",
            Reply::json(200, &vm_with("running", json!({"network_ready": false}))),
        ),
    ]);
    let run = create(&server, &["--public-ip", "--wait-timeout", "1", "--yes"]);
    let doc = fails_with_vm(&run, "timeout");
    assert_eq!(doc["status"], "running");
    assert_eq!(doc["network_ready"], false);
    let error: Value = serde_json::from_str(run.stderr()).expect("one JSON object");
    assert_eq!(
        error["error"]["message"],
        format!(
            "the VM `{VM}` was running, but its public IPv4 was not reachable \
             (`network_ready`) after 1 second"
        )
    );
}

/// A failed attach (`public_ip_error`) means no address will turn `network_ready`, so the
/// wait ends at `running`, whichever answer carries the error.
#[test]
fn a_launch_whose_address_failed_does_not_wait_for_network_ready() {
    let failed = |status: &str| {
        vm_with(
            status,
            json!({"public_ip_error": "none free", "network_ready": null}),
        )
    };
    for (answer, polled) in [
        (vm("pending"), failed("running")),
        (
            failed("pending"),
            vm_with("running", json!({"network_ready": null})),
        ),
    ] {
        let (run, seen) = launch_in_sequence(vec![(202, answer), (200, polled)], &["--public-ip"]);
        assert_eq!(run.ok()["status"], "running");
        assert_eq!(seen.len(), 2, "{seen:?}");
    }
}

/// Swagger: a VM whose boot stalled is shown `failed` with `boot_stalled` true, and returns to
/// `running` by itself, so the wait goes on through it.
#[test]
fn a_stalled_boot_shown_failed_is_waited_through() {
    let stalled = vm_with("failed", json!({"boot_stalled": true}));
    let (run, seen) = launch_in_sequence(
        vec![
            (202, vm("pending")),
            (200, stalled.clone()),
            (200, vm_with("running", json!({"boot_stalled": false}))),
        ],
        &[],
    );
    assert_eq!(run.ok()["status"], "running");
    assert_eq!(seen.len(), 3, "{seen:?}");

    // A launch answer that is already stalled is polled too.
    let (run, seen) = launch_in_sequence(
        vec![
            (202, stalled),
            (200, vm_with("running", json!({"boot_stalled": false}))),
        ],
        &[],
    );
    assert_eq!(run.ok()["status"], "running");
    assert_eq!(seen.len(), 2, "{seen:?}");
}

/// A boot that stays stalled until `--wait-timeout` is `timeout`, not `unavailable`, and the
/// message says the boot stalled.
#[test]
fn a_stalled_boot_that_does_not_recover_is_timeout() {
    let server = Server::start(vec![
        route(
            "POST",
            "/api/compute/vms/",
            Reply::json(202, &vm("pending")),
        ),
        vm_route(
            "GET",
            "",
            Reply::json(200, &vm_with("failed", json!({"boot_stalled": true}))),
        ),
    ]);
    let run = create(&server, &["--wait-timeout", "1", "--yes"]);
    assert_eq!(fails_with_vm(&run, "timeout")["boot_stalled"], true);
    let error: Value = serde_json::from_str(run.stderr()).expect("one JSON object");
    assert_eq!(
        error["error"]["message"],
        format!(
            "the VM `{VM}` was not running after 1 second: its boot stalled (`boot_stalled`) \
             and has not recovered"
        )
    );
}

/// `failed` without `boot_stalled`, or with it false, stays terminal, and so do the two
/// decommission statuses even when `boot_stalled` is true.
#[test]
fn failed_without_a_stall_and_decommissioning_stay_terminal() {
    for answer in [
        vm_with("failed", json!({"boot_stalled": false})),
        vm_with("failed", json!({"boot_stalled": null})),
        vm_with("decommissioning", json!({"boot_stalled": true})),
        vm_with("decommissioned", json!({"boot_stalled": true})),
    ] {
        let server = Server::start(vec![
            route(
                "POST",
                "/api/compute/vms/",
                Reply::json(202, &vm("pending")),
            ),
            vm_route("GET", "", Reply::json(200, &answer)),
        ]);
        let run = create(&server, &["--public-ip", "--wait", "--yes"]);
        fails_with_vm(&run, "partial_failure");
        assert_eq!(server.requests().len(), 2, "{answer}");
    }
}

/// `--os-disk-tier`, `--dedicated-disk`, and `--backup-interval` each add their
/// swagger field to the launch body only when given. A dedicated disk also sends
/// `pinned_ack: true`, which swagger requires beside `dedicated_disks`.
#[test]
fn launch_options_add_exactly_their_fields_to_the_body() {
    let server = Server::start(vec![]);
    let body = |extra: &[&str]| {
        let mut args = extra.to_vec();
        args.push("--dry-run");
        create(&server, &args).ok()["requests"][0]["body"].clone()
    };
    assert_eq!(
        body(&["--os-disk-tier", "hdd"]),
        json!({"flavor": "small", "ssh_key_ids": [7], "os_disk_tier": "hdd"})
    );
    for tier in ["nvme", "ssd", "hdd"] {
        assert_eq!(body(&["--os-disk-tier", tier])["os_disk_tier"], tier);
    }
    assert_eq!(
        body(&["--backup-interval", "3600"]),
        json!({"flavor": "small", "ssh_key_ids": [7], "backup_interval_s": 3600})
    );
    for seconds in [900, 3600, 21600, 86400] {
        let given = seconds.to_string();
        assert_eq!(
            body(&["--backup-interval", &given])["backup_interval_s"],
            seconds
        );
    }
    assert_eq!(
        body(&["--dedicated-disk", "2000:ssd"]),
        json!({
            "flavor": "small",
            "ssh_key_ids": [7],
            "dedicated_disks": {"tier": "ssd", "count": 1, "min_size_gb": 2000},
            "pinned_ack": true
        })
    );
    let eight: Vec<&str> = ["--dedicated-disk", "1000:hdd"].repeat(8);
    assert_eq!(
        body(&eight)["dedicated_disks"],
        json!({"tier": "hdd", "count": 8, "min_size_gb": 1000})
    );
    assert_eq!(
        body(&[
            "--os-disk-tier",
            "ssd",
            "--dedicated-disk",
            "4000:nvme",
            "--dedicated-disk",
            "4000:nvme",
        ]),
        json!({
            "flavor": "small",
            "ssh_key_ids": [7],
            "os_disk_tier": "ssd",
            "dedicated_disks": {"tier": "nvme", "count": 2, "min_size_gb": 4000},
            "pinned_ack": true
        })
    );
    assert!(server.requests().is_empty());

    let sent = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(202, &vm("pending")),
    )]);
    let args = [
        "--os-disk-tier",
        "hdd",
        "--backup-interval",
        "86400",
        "--no-wait",
        "--yes",
    ];
    create(&sent, &args).ok();
    assert_eq!(
        sent.only_request().json(),
        json!({
            "flavor": "small",
            "ssh_key_ids": [7],
            "os_disk_tier": "hdd",
            "backup_interval_s": 86400
        })
    );
}

/// Every launch option is checked against swagger before anything is sent, a dry run
/// included: the tier enum, the backup interval set, a dedicated disk's size (at least
/// 1000 GiB) and count (1 to 8, one tier and size for all), and no backups beside
/// dedicated disks (swagger: 409 `dedicated-pinned`).
#[test]
fn bad_launch_options_are_usage_without_a_request() {
    let server = Server::start(vec![]);
    let nine: Vec<&str> = ["--dedicated-disk", "1000:hdd"].repeat(9);
    let cases: [(&[&str], &str); 15] = [
        (&["--os-disk-tier", "sata"], "sata"),
        (&["--os-disk-tier", "HDD"], "HDD"),
        (&["--backup-interval", "600"], "600"),
        (&["--backup-interval", "1h"], "1h"),
        (&["--dedicated-disk", "999:hdd"], "a dedicated disk is"),
        (&["--dedicated-disk", "1000"], "a dedicated disk is"),
        (&["--dedicated-disk", "1000:tape"], "a dedicated disk is"),
        (&["--dedicated-disk", "x:hdd"], "a dedicated disk is"),
        (&["--dedicated-disk", ":hdd"], "a dedicated disk is"),
        (&["--dedicated-disk", "1000:"], "a dedicated disk is"),
        (&["--dedicated-disk", "1000:hdd:ssd"], "a dedicated disk is"),
        (&["--dedicated-disk", "+1000:hdd"], "a dedicated disk is"),
        (&nine, "`--dedicated-disk` is given 9 times"),
        (
            &[
                "--dedicated-disk",
                "1000:hdd",
                "--dedicated-disk",
                "2000:hdd",
            ],
            "share one size and tier",
        ),
        (
            &["--dedicated-disk", "1000:hdd", "--backup-interval", "3600"],
            "--backup-interval",
        ),
    ];
    for (extra, said) in cases {
        for flag in ["--dry-run", "--yes"] {
            let mut args = extra.to_vec();
            args.push(flag);
            let error = create(&server, &args).fails(2, "usage");
            let message = error["message"].as_str().expect("the message is a string");
            assert!(message.contains(said), "{extra:?}: {message}");
        }
    }
    let mixed = [
        "--dedicated-disk",
        "1000:hdd",
        "--dedicated-disk",
        "1000:ssd",
        "--yes",
    ];
    let error = create(&server, &mixed).fails(2, "usage");
    assert_eq!(
        error["message"],
        "the `--dedicated-disk` values differ, and a launch's dedicated disks share one size \
         and tier"
    );
    assert_eq!(
        error["hint"],
        "repeat `--dedicated-disk` with the same size and tier, once per disk"
    );
    assert!(server.requests().is_empty());
}

/// The refusal names what each launch option costs, and where its price is shown, without
/// a read of the price: the binary holds none.
#[test]
fn launch_options_name_their_cost_in_the_refusal() {
    let server = Server::start(vec![]);
    let message = |extra: &[&str]| {
        create(&server, extra).fails(2, "confirmation_required")["message"]
            .as_str()
            .expect("the message is a string")
            .to_owned()
    };
    assert_eq!(
        message(&["--os-disk-tier", "hdd"]),
        "this would launch a VM of flavor `small` with an OS disk on tier `hdd` or faster, \
         fixed for the VM's life and billed at the `hdd` price that `hipc compute prices` shows \
         (`vm_by_os_disk_tier`), and `--yes` was not passed"
    );
    assert_eq!(
        message(&["--backup-interval", "3600"]),
        "this would launch a VM of flavor `small` with backups every 3600 seconds, stored \
         backups billed per GB-hour at the price that `hipc compute backups options` shows, \
         and `--yes` was not passed"
    );
    assert_eq!(
        message(&[
            "--dedicated-disk",
            "2000:ssd",
            "--dedicated-disk",
            "2000:ssd"
        ]),
        "this would launch a VM of flavor `small` with 2 dedicated disks of at least 2000 GiB \
         on tier `ssd` or faster, each billed per GiB-hour at its size class (floored to 100 \
         GiB), while the VM is stopped too, at the price that `hipc compute prices` shows \
         (`dedicated_disks`); the disks pin the VM to its server: no backup, migration, or \
         failover, and `--yes` was not passed"
    );
    assert_eq!(
        message(&["--dedicated-disk", "1000:hdd"]),
        "this would launch a VM of flavor `small` with 1 dedicated disk of at least 1000 GiB \
         on tier `hdd` or faster, each billed per GiB-hour at its size class (floored to 100 \
         GiB), while the VM is stopped too, at the price that `hipc compute prices` shows \
         (`dedicated_disks`); the disks pin the VM to its server: no backup, migration, or \
         failover, and `--yes` was not passed"
    );
    let all = message(&[
        "--public-ip",
        "--os-disk-tier",
        "ssd",
        "--backup-interval",
        "900",
    ]);
    assert!(
        all.starts_with("this would launch a VM of flavor `small` with a public IPv4, billed"),
        "{all}"
    );
    assert!(
        all.contains("`price_hourly_after_free`); with an OS disk on tier `ssd` or faster"),
        "{all}"
    );
    assert!(
        all.contains("(`vm_by_os_disk_tier`); with backups every 900 seconds"),
        "{all}"
    );
    assert!(server.requests().is_empty());
}

/// `hipc schema` lists the launch options with their values, so an agent learns
/// them without reading the help: the tier and interval enums, and that a dedicated disk
/// repeats.
#[test]
fn schema_lists_the_launch_options() {
    let server = Server::start(vec![]);
    let doc = Hipc::against(&Home::new(), &server).run(&["schema"]).ok();
    let flag = |path: &[&str], name: &str| {
        let command = doc["commands"]
            .as_array()
            .expect("commands is an array")
            .iter()
            .find(|command| command["path"] == json!(path))
            .expect("the command is in the schema");
        command["flags"]
            .as_array()
            .expect("flags is an array")
            .iter()
            .find(|flag| flag["name"] == name)
            .expect("the flag is in the schema")
            .clone()
    };
    let create = ["compute", "vms", "create"];
    let tiers = json!(["nvme", "ssd", "hdd"]);
    assert_eq!(flag(&create, "--os-disk-tier")["possible_values"], tiers);
    assert_eq!(
        flag(&["compute", "feasibility"], "--os-disk-tier")["possible_values"],
        tiers
    );
    assert_eq!(
        flag(&create, "--backup-interval")["possible_values"],
        json!(["900", "3600", "21600", "86400"])
    );
    let disk = flag(&create, "--dedicated-disk");
    assert_eq!(disk["repeats"], true);
    assert_eq!(disk["value_name"], "SIZE:TIER");
    assert!(server.requests().is_empty());
}

/// A VM document with `backup_error` set to `error` (a JSON value), as swagger documents
/// the field on the 202 and on the VM detail.
fn with_backup_error(status: &str, error: &str) -> String {
    format!(r#"{{"vm_id":"{VM}","status":"{status}","backup_error":{error}}}"#)
}

/// A launch with `--backup-interval` whose VM carries
/// `backup_error` (swagger: "Why the backups asked for at launch were not turned on
/// (yet)") ends with one text-mode warning that quotes the server; stdout is the answer
/// unchanged, and JSON mode says nothing on stderr.
#[test]
fn a_launch_whose_backups_failed_says_so_and_quotes_the_server() {
    let answer = with_backup_error("pending", r#""not `golden`""#);
    let args = ["--backup-interval", "3600", "--no-wait", "--yes"];
    let run = launch_in_text(&answer, &args);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        "warning: the VM launched without its scheduled backups: the API says `not \
         \\u{60}golden\\u{60}`\n"
    );
    assert!(run.stdout().contains("not `golden`"), "{}", run.stdout());

    let server = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(202, &answer),
    )]);
    let run = create(&server, &args);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(run.stderr(), "", "JSON mode says nothing on stderr");
    assert_eq!(run.json(), serde_json::from_str::<Value>(&answer).unwrap());

    // The field belongs to backups asked for at launch: without the flag there is no warning,
    // and a null or missing `backup_error` is none either.
    let quiet = launch_in_text(&answer, &["--no-wait", "--yes"]);
    assert_eq!(quiet.stderr(), "", "without --backup-interval");
    for answer in [with_backup_error("pending", "null"), vm("pending")] {
        let run = launch_in_text(&answer, &args);
        assert_eq!(run.stderr(), "", "{answer}");
    }
}

/// With a wait, the printed document is the polled VM. Its `backup_error` decides; the 202's
/// is used only when the polled VM omits the field. A polled null means no failure is
/// reported yet, so it prints no warning even when the 202 carried a reason.
#[test]
fn a_launch_that_waits_reports_failed_backups_from_the_polled_vm() {
    let warned = "warning: the VM launched without its scheduled backups: the API says `nope`\n";
    for (answer, polled, stderr) in [
        (
            vm("pending"),
            with_backup_error("running", r#""nope""#),
            warned,
        ),
        (
            with_backup_error("pending", r#""nope""#),
            vm("running"),
            warned,
        ),
        (
            with_backup_error("pending", r#""nope""#),
            with_backup_error("running", "null"),
            "",
        ),
    ] {
        let server = Server::start(vec![
            route("POST", "/api/compute/vms/", Reply::json(202, &answer)),
            vm_route("GET", "", Reply::json(200, &polled)),
        ]);
        let args = [
            "--output",
            "text",
            "--backup-interval",
            "3600",
            "--wait",
            "--yes",
        ];
        let run = create(&server, &args);
        assert_eq!(run.code(), Some(0), "{}", run.stderr());
        // The `vm_id` note is not a `warning:` line, so filtering leaves only the warning.
        let warning = run
            .stderr()
            .lines()
            .filter(|line| line.starts_with("warning:"));
        assert_eq!(
            warning.collect::<String>(),
            stderr.trim_end(),
            "{answer} then {polled}"
        );
        assert_eq!(server.requests().len(), 2, "{answer}");
    }
}

/// `--failover` and `--on-guest-poweroff` are placement constraints honoured only at launch,
/// so `vms create` takes them as `apps create` does: each is sent only when given, a failover
/// needs `--backup-interval` (and so cannot go with a dedicated disk), and the question names
/// what `auto` does.
#[test]
fn vms_create_takes_the_failover_and_the_power_policy_at_launch() {
    let server = Server::start(vec![]);
    let doc = create(
        &server,
        &[
            "--backup-interval",
            "900",
            "--failover",
            "auto",
            "--on-guest-poweroff",
            "stop",
            "--dry-run",
        ],
    )
    .ok();
    assert_eq!(
        draft(&doc)["body"],
        json!({
            "flavor": "small",
            "ssh_key_ids": [7],
            "backup_interval_s": 900,
            "failover_mode": "auto",
            "on_guest_poweroff": "stop",
        })
    );
    let doc = create(
        &server,
        &[
            "--failover",
            "manual",
            "--backup-interval",
            "900",
            "--dry-run",
        ],
    )
    .ok();
    assert_eq!(draft(&doc)["body"]["failover_mode"], "manual");
    assert!(draft(&doc)["body"].get("on_guest_poweroff").is_none());
    let doc = create(&server, &["--on-guest-poweroff", "restart", "--dry-run"]).ok();
    assert_eq!(draft(&doc)["body"]["on_guest_poweroff"], "restart");
    assert!(draft(&doc)["body"].get("failover_mode").is_none());
    for args in [
        &["--failover", "auto", "--yes"][..],
        &[
            "--failover",
            "auto",
            "--dedicated-disk",
            "1000:hdd",
            "--yes",
        ],
        &[
            "--backup-interval",
            "900",
            "--failover",
            "sometimes",
            "--yes",
        ],
        &["--on-guest-poweroff", "halt", "--yes"],
    ] {
        create(&server, args).fails(2, "usage");
    }
    let message = |extra: &[&str]| {
        create(&server, extra).fails(2, "confirmation_required")["message"]
            .as_str()
            .expect("the message is a string")
            .to_owned()
    };
    assert_eq!(
        message(&["--backup-interval", "3600", "--failover", "auto"]),
        "this would launch a VM of flavor `small` with backups every 3600 seconds and failover \
         `auto`, which restarts it on another server from its latest backup if its server dies, \
         losing what was written after that backup, stored backups billed per GB-hour at the price that `hipc compute backups options` \
         shows, and `--yes` was not passed"
    );
    assert!(
        message(&["--backup-interval", "3600", "--failover", "manual"])
            .contains("backups every 3600 seconds and failover `manual`, stored backups"),
    );
    // `stop` keeps billing a VM its guest powered off, so the question names it; `restart`,
    // the server's default, is not named.
    assert_eq!(
        message(&["--on-guest-poweroff", "stop"]),
        "this would launch a VM of flavor `small` with a power-off from inside the VM leaving it \
         stopped, and billed as a stopped VM until it is started or deleted, and `--yes` was not \
         passed"
    );
    assert_eq!(
        message(&["--on-guest-poweroff", "restart"]),
        "this would launch a VM of flavor `small`, and `--yes` was not passed"
    );
    assert!(server.requests().is_empty());
}

/// The flavor and the SSH key name are text the caller typed, so the questions quote them as
/// every other value: a backtick in one cannot end the quote.
#[test]
fn the_launch_question_quotes_the_flavor() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let key = home.file("k.pub", b"ssh-ed25519 AAAA me\n");
    let error = Hipc::against(&home, &server)
        .run(&[
            "compute",
            "ssh-keys",
            "create",
            "--name",
            "a`b",
            "--public-key-file",
            key.to_str().unwrap(),
        ])
        .fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        r"this would store the SSH key `a\u{60}b`, and `--yes` was not passed"
    );
    let run = hipc(
        &server,
        &["vms", "create", "--flavor", "a`b", "--ssh-key-id", "7"],
    );
    let error = run.fails(2, "confirmation_required");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with(r"this would launch a VM of flavor `a\u{60}b`"),
        "{error}"
    );
}

/// The catalogue's flavor codes are lowercase, so a code with a capital is usage before
/// anything is sent, wherever `--flavor` is taken, rather than a code the server may not know.
#[test]
fn a_flavor_with_a_capital_is_refused_before_anything_is_sent() {
    let server = Server::start(vec![]);
    for args in [
        &["vms", "create", "--flavor", "SMALL", "--ssh-key-id", "7"][..],
        &["vms", "resize", "vm-1a2b", "--flavor", "Small"],
        &["feasibility", "--flavor", "smalL"],
        &[
            "kubernetes",
            "node-pools",
            "create",
            "7",
            "--name",
            "w",
            "--flavor",
            "Small",
        ],
        &[
            "app-engine",
            "apps",
            "create",
            "--flavor",
            "SMALL",
            "--compose-file",
            "never-made.yaml",
        ],
    ] {
        let error = hipc(&server, args).fails(2, "usage");
        let message = error["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("in lowercase, such as `small`"),
            "{args:?}: {message}"
        );
    }
    assert!(server.requests().is_empty());
}

/// The grammar holds what the help states: a flavor is a code (not empty, no space), an SSH
/// key name is 1 to 100 characters, a node pool name is 1 to 20 of `a-z`, `0-9`, and `-`, and
/// a taint is `KEY[=VALUE]:EFFECT` with a Kubernetes effect. Each is usage before anything is
/// sent, a dry run included.
#[test]
fn the_compute_grammar_refuses_what_its_help_rules_out() {
    let server = Server::start(vec![]);
    let long = "k".repeat(101);
    let pool = |extra: &[&'static str]| {
        let mut args = vec!["kubernetes", "node-pools", "create", "7"];
        args.extend(extra);
        args
    };
    let refused: Vec<(Vec<&str>, &str)> = vec![
        (
            vec!["vms", "create", "--flavor", "", "--ssh-key-id", "7"],
            "a flavor is a code",
        ),
        (vec!["vms", "resize", "vm-1a2b", "--flavor", ""], "a flavor"),
        (vec!["feasibility", "--flavor", "a b"], "a flavor"),
        (
            vec![
                "app-engine",
                "apps",
                "create",
                "--flavor",
                "",
                "--compose-file",
                "c.yaml",
            ],
            "a flavor",
        ),
        (pool(&["--name", "w", "--flavor", ""]), "a flavor"),
        (pool(&["--name", "", "--flavor", "small"]), "1 to 20"),
        (pool(&["--name", "Workers", "--flavor", "small"]), "1 to 20"),
        (pool(&["--name", "a_b", "--flavor", "small"]), "1 to 20"),
        (
            pool(&["--name", "abcdefghijklmnopqrstu", "--flavor", "small"]),
            "1 to 20",
        ),
        (
            pool(&["--name", "w", "--flavor", "small", "--taint", "gpu"]),
            "KEY[=VALUE]:EFFECT",
        ),
        (
            pool(&[
                "--name",
                "w",
                "--flavor",
                "small",
                "--taint",
                "gpu=true:Sometimes",
            ]),
            "KEY[=VALUE]:EFFECT",
        ),
        (
            pool(&[
                "--name",
                "w",
                "--flavor",
                "small",
                "--taint",
                "=x:NoSchedule",
            ]),
            "KEY[=VALUE]:EFFECT",
        ),
        (
            pool(&["--name", "w", "--flavor", "small", "--taint", ":NoExecute"]),
            "KEY[=VALUE]:EFFECT",
        ),
        (
            vec![
                "ssh-keys",
                "create",
                "--name",
                "",
                "--public-key-file",
                "k.pub",
            ],
            "1 to 100",
        ),
        (
            vec![
                "ssh-keys",
                "create",
                "--name",
                &long,
                "--public-key-file",
                "k.pub",
            ],
            "1 to 100",
        ),
    ];
    for (args, said) in refused {
        for last in ["--yes", "--dry-run"] {
            let mut all = args.clone();
            all.push(last);
            let error = hipc(&server, &all).fails(2, "usage");
            let message = error["message"].as_str().expect("the message is a string");
            assert!(message.contains(said), "{all:?}: {message}");
        }
    }
    assert!(server.requests().is_empty());
}

/// What the help allows parses: a 20-character pool name, a taint with or without a value,
/// and a 100-character SSH key name.
#[test]
fn the_compute_grammar_takes_what_its_help_allows() {
    let server = Server::start(vec![]);
    let pool = |extra: &[&'static str]| {
        let mut args = vec!["kubernetes", "node-pools", "create", "7"];
        args.extend(extra);
        args
    };
    let doc = hipc(
        &server,
        &pool(&[
            "--name",
            "abcdefghijklmnopqrst",
            "--flavor",
            "small",
            "--taint",
            "gpu=true:NoSchedule",
            "--taint",
            "spot:PreferNoSchedule",
            "--dry-run",
        ]),
    )
    .ok();
    assert_eq!(
        draft(&doc)["body"]["taints"],
        json!(["gpu=true:NoSchedule", "spot:PreferNoSchedule"])
    );
    let home = Home::new();
    // A key file reached through a symlink, as dotfile managers make them, is read.
    let key = home.path().join("linked.pub");
    std::os::unix::fs::symlink(home.file("k.pub", b"ssh-ed25519 AAAA me\n"), &key).unwrap();
    let name = "k".repeat(100);
    let doc = Hipc::against(&home, &server)
        .run(&[
            "compute",
            "ssh-keys",
            "create",
            "--name",
            &name,
            "--public-key-file",
            key.to_str().unwrap(),
            "--dry-run",
        ])
        .ok();
    assert_eq!(draft(&doc)["body"]["name"], name);
    assert!(server.requests().is_empty());
}

/// The help pages where a reader decides what compute and its neighbours cost say what the
/// money controls are, so a reader does not take the chain's hourly cap for a budget, the
/// cheapest disk for a production one, or a stopped VM for a free one.
#[test]
fn the_help_says_where_the_money_controls_are() {
    let help = |args: &[&str]| {
        let run = Hipc::new(&Home::new()).run(&[args, &["--help"]].concat());
        assert_eq!(run.code(), Some(0), "{}", run.stderr());
        run.stdout()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    for (args, says) in [
        (&["compute"][..], "hipc account projects budget update"),
        (&["compute", "charge-cap"], "not a budget"),
        (
            &["compute", "charge-cap"],
            "hipc account projects budget update",
        ),
        (
            &["compute", "vms", "create"],
            "hdd: The slowest and cheapest",
        ),
        (&["compute", "vms", "create"], "running or stopped"),
        (
            &["compute", "runners"],
            "not yet enabled on api.hippius.com",
        ),
        (
            &["compute", "runners", "limit", "update"],
            "(1 credit = $1)",
        ),
        (
            &["compute", "databases", "create"],
            "hipc compute public-prices",
        ),
        (&["cdn", "cap"], "answered 503"),
        (&["cdn", "cap"], "WAF add-on"),
        (&["cdn", "cap", "update"], "(1 credit = $1)"),
        (&["cdn", "zones", "create"], "(1 credit = $1)"),
        (&["cdn", "estimate"], "needs a token"),
        (&["cdn", "prices"], "the WAF tiers"),
        (
            &["drive", "subscription", "subscribe"],
            "hipc drive subscription change --plan",
        ),
        (
            &["s3", "subscription", "subscribe"],
            "not a ceiling on the bill",
        ),
        (&["s3", "buckets", "objects", "presign"], "604800 (7 days)"),
    ] {
        let text = help(args);
        assert!(text.contains(says), "{args:?} lacks {says:?}: {text}");
    }
}

/// `--open-port` and `--open-ssh` set one `tcp` rule for their ports, each once and in order,
/// right after the launch answer, and the launch's question names them.
#[test]
fn a_launch_with_open_ports_sets_the_firewall_after_the_launch() {
    let server = Server::start(vec![
        route(
            "POST",
            "/api/compute/vms/",
            Reply::json(202, &vm("pending")),
        ),
        vm_route(
            "PUT",
            "firewall/",
            Reply::json(200, r#"{"rules":[],"revision":"r2"}"#),
        ),
    ]);
    let opens = ["--open-port", "443", "--open-ssh", "--open-port", "22"];
    let mut argv = opens.to_vec();
    argv.extend(["--no-wait", "--yes"]);
    let doc = create(&server, &argv).ok();
    assert_eq!(doc["vm_id"], VM, "the launch answer is printed");
    // With the rules the PUT set beside it.
    assert_eq!(
        doc["firewall"],
        json!({"rules": [{
            "protocol": "tcp",
            "ports": ["22", "443"],
            "description": "opened at launch by hipc",
        }]})
    );
    let sent = server.requests();
    assert_eq!(sent.len(), 2);
    assert_eq!(
        (sent[0].method.as_str(), sent[1].method.as_str()),
        ("POST", "PUT")
    );
    assert_eq!(sent[1].path(), format!("/api/compute/vms/{VM}/firewall/"));
    assert_eq!(
        sent[1].json(),
        json!({"rules": [{
            "protocol": "tcp",
            "ports": ["22", "443"],
            "description": "opened at launch by hipc",
        }]})
    );
    // Without `--yes` nothing is sent, and the refusal names the ports.
    let error = create(&server, &opens).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        "this would launch a VM of flavor `small` with the TCP ports `22` and `443` open to \
         every IPv4 address, and `--yes` was not passed"
    );
    assert_eq!(server.requests().len(), 2);
    // The dry run lists the launch, then the firewall request with the id a placeholder.
    let mut argv = vec!["--dry-run", "--open-ssh"];
    argv.push("--yes");
    let doc = create(&server, &argv).ok();
    let requests = doc["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 2, "{doc}");
    assert_eq!(requests[0]["method"], "POST");
    assert_eq!(requests[1]["method"], "PUT");
    assert!(
        requests[1]["url"]
            .as_str()
            .unwrap()
            .ends_with("/api/compute/vms/<the vm_id the launch returns>/firewall/"),
        "{doc}"
    );
    assert_eq!(requests[1]["body"]["rules"][0]["ports"], json!(["22"]));
    assert_eq!(doc["confirmation"], true);
    assert_eq!(server.requests().len(), 2, "a dry run sends nothing");
    // A port out of range is usage, with nothing sent.
    for bad in ["0", "65536", "ssh"] {
        create(&server, &["--open-port", bad, "--yes"]).fails(2, "usage");
    }
    assert_eq!(server.requests().len(), 2);
}

/// The VM a waited launch prints carries the rules `--open-port` set, after a wait that ends
/// early too; a launch without `--open-port` adds no `firewall`, and one the server sent is
/// printed as it came.
#[test]
fn a_waited_launch_prints_the_rules_it_set() {
    let rules = json!({"rules": [{
        "protocol": "tcp",
        "ports": ["22"],
        "description": "opened at launch by hipc",
    }]});
    let launch = || {
        route(
            "POST",
            "/api/compute/vms/",
            Reply::json(202, &vm("pending")),
        )
    };
    let opened = || vm_route("PUT", "firewall/", Reply::json(200, "{}"));
    let server = Server::start(vec![
        launch(),
        opened(),
        vm_route("GET", "", Reply::json(200, &vm("running"))),
    ]);
    let doc = create(&server, &["--open-ssh", "--wait-timeout", "5", "--yes"]).ok();
    assert_eq!(doc["status"], "running");
    assert_eq!(doc["firewall"], rules);
    let still = Server::start(vec![
        launch(),
        opened(),
        vm_route("GET", "", Reply::json(200, &vm("provisioning"))),
    ]);
    let run = create(&still, &["--open-ssh", "--wait-timeout", "1", "--yes"]);
    let vm = fails_with_vm(&run, "timeout");
    assert_eq!(vm["firewall"], rules);
    // A launch without `--open-port` sets no rule and adds no field.
    let plain = Server::start(vec![launch()]);
    let doc = create(&plain, &["--no-wait", "--yes"]).ok();
    assert!(doc.get("firewall").is_none(), "{doc}");
    // A `firewall` the server sent is the server's, and stays.
    let theirs = r#"{"vm_id":"VM","status":"pending","firewall":"server"}"#.replace("VM", VM);
    let server = Server::start(vec![
        route("POST", "/api/compute/vms/", Reply::json(202, &theirs)),
        opened(),
    ]);
    let doc = create(&server, &["--open-ssh", "--no-wait", "--yes"]).ok();
    assert_eq!(doc["firewall"], "server");
}

/// A launch that went through and a firewall request that did not: the VM is printed on
/// stdout, the error is `partial_failure` with the failed step and the rules to set, and the
/// hint is the firewall update that sets them.
#[test]
fn a_launch_whose_ports_cannot_be_opened_is_a_partial_failure() {
    let server = Server::start(vec![
        route(
            "POST",
            "/api/compute/vms/",
            Reply::json(202, &vm("pending")),
        ),
        vm_route(
            "PUT",
            "firewall/",
            Reply::json(422, r#"{"detail":"bad rule"}"#),
        ),
    ]);
    let run = create(&server, &["--open-ssh", "--no-wait", "--yes"]);
    assert_eq!(run.code(), Some(1), "{}", run.stderr());
    assert_eq!(run.json()["vm_id"], VM, "the VM is not lost");
    let error: Value = serde_json::from_str(run.stderr()).unwrap();
    let error = &error["error"];
    assert_eq!(error["code"], "partial_failure");
    assert_eq!(error["side_effects"], "partial");
    assert_eq!(error["retryable"], false);
    assert!(
        error["message"].as_str().unwrap().starts_with(&format!(
            "launched the VM `{VM}`, but could not open the TCP port `22`"
        )),
        "{error}"
    );
    assert_eq!(error["details"][0]["step"], "firewall");
    assert_eq!(error["details"][0]["code"], "validation");
    assert_eq!(
        error["details"][0]["rules"]["rules"][0]["ports"],
        json!(["22"])
    );
    assert_eq!(
        error["hint"],
        format!(
            "run `hipc compute firewall update {VM} --rules-file - --yes` with \
             `{{\"rules\":[{{\"protocol\":\"tcp\",\"ports\":[\"22\"],\"description\":\"opened at \
             launch by hipc\"}}]}}` on stdin"
        )
    );
    // A launch answer without a usable id cannot be given its rules: `internal`, the answer
    // on stdout, and no firewall request.
    let server = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(202, r#"{"status":"pending"}"#),
    )]);
    let run = create(&server, &["--open-port", "80", "--no-wait", "--yes"]);
    assert_eq!(run.code(), Some(1), "{}", run.stderr());
    assert_eq!(run.json(), json!({"status": "pending"}));
    let error: Value = serde_json::from_str(run.stderr()).unwrap();
    assert_eq!(error["error"]["code"], "internal");
    assert_eq!(error["error"]["side_effects"], "partial");
    assert_eq!(server.requests().len(), 1);
}

/// The launch answer's id goes into the firewall `PUT` when it keeps to the `vm_id` rule,
/// a leading `-` included, and the hint of a failed `PUT` then puts it after `--`, so the
/// command runs as written. An id that could change the route sends no `PUT`.
#[test]
fn a_launched_id_with_a_leading_dash_still_gets_its_rules() {
    let launched = r#"{"vm_id":"-vm1","status":"pending"}"#;
    let server = Server::start(vec![
        route("POST", "/api/compute/vms/", Reply::json(202, launched)),
        route(
            "PUT",
            "/api/compute/vms/-vm1/firewall/",
            Reply::json(422, r#"{"detail":"bad rule"}"#),
        ),
    ]);
    let run = create(&server, &["--open-ssh", "--no-wait", "--yes"]);
    assert_eq!(run.code(), Some(1), "{}", run.stderr());
    let error: Value = serde_json::from_str(run.stderr()).unwrap();
    assert_eq!(error["error"]["code"], "partial_failure");
    let hint = error["error"]["hint"].as_str().unwrap();
    assert!(
        hint.starts_with("run `hipc compute firewall update --rules-file - --yes -- -vm1` with "),
        "{hint}"
    );
    let sent: Vec<String> = server.requests().iter().map(|r| r.target.clone()).collect();
    assert_eq!(
        sent,
        ["/api/compute/vms/", "/api/compute/vms/-vm1/firewall/"]
    );
    // The hint's command parses: its id is not read as a flag.
    let mut argv = vec![
        "--dry-run",
        "compute",
        "firewall",
        "update",
        "--rules-file",
        "-",
    ];
    argv.extend(["--yes", "--", "-vm1"]);
    let rules = r#"{"rules":[{"protocol":"tcp","ports":["22"]}]}"#;
    Hipc::new(&Home::new())
        .stdin(rules.as_bytes())
        .run(&argv)
        .dry_ok();
    let server = Server::start(vec![route(
        "POST",
        "/api/compute/vms/",
        Reply::json(202, r#"{"vm_id":"a/b","status":"pending"}"#),
    )]);
    let run = create(&server, &["--open-ssh", "--no-wait", "--yes"]);
    assert_eq!(run.code(), Some(1), "{}", run.stderr());
    let error: Value = serde_json::from_str(run.stderr()).unwrap();
    assert_eq!(error["error"]["code"], "internal");
    assert_eq!(server.requests().len(), 1);
}

/// In text mode a launch that opened ports says so, and with `--public-ip` it does not warn
/// that every port is closed.
#[test]
fn a_launch_with_open_ports_notes_them_in_text_mode() {
    let server = Server::start(vec![
        route(
            "POST",
            "/api/compute/vms/",
            Reply::json(202, &vm("pending")),
        ),
        vm_route("PUT", "firewall/", Reply::json(200, "{}")),
    ]);
    let run = Hipc::against(&Home::new(), &server).run(&[
        "--output",
        "text",
        "compute",
        "vms",
        "create",
        "--flavor",
        "small",
        "--ssh-key-id",
        "7",
        "--public-ip",
        "--open-ssh",
        "--no-wait",
        "--yes",
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        format!("note: opened the TCP port `22` to every IPv4 address on the VM `{VM}`\n")
    );
    // Without `--public-ip` the note says the rule waits for an address.
    let run = Hipc::against(&Home::new(), &server).run(&[
        "--output",
        "text",
        "compute",
        "vms",
        "create",
        "--flavor",
        "small",
        "--ssh-key-id",
        "7",
        "--open-ssh",
        "--no-wait",
        "--yes",
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(
        run.stderr().ends_with(
            "; the VM has no public IPv4 yet, and the rule applies once `hipc compute \
             public-ip create` attaches one\n"
        ),
        "{}",
        run.stderr()
    );
}
