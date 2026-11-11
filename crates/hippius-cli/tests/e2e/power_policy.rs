//! `hipc compute vms power-policy get | update`: what a VM does when its own operating system
//! powers it off. The read is the policy fields of the VM; the update is not confirmed, as
//! `hipc compute vms start` and `stop` are not.

use hippius::routes::compute;
use serde_json::{Value, json};

use crate::common::{Hipc, Home, MEMBER, Reply, Run, Server, VM, draft, route_of};

/// A VM in the shape of swagger `GET /api/compute/vms/{vm_id}/`, with the policy fields; a
/// pending policy awaits the acknowledgement of the VM's server.
fn vm(policy: &str, pending: bool) -> String {
    vm_pending_for(policy, pending.then_some("awaiting-ack"))
}

/// A VM whose policy is pending for `reason` (swagger `on_guest_poweroff_reason`), or applied.
fn vm_pending_for(policy: &str, reason: Option<&str>) -> String {
    format!(
        concat!(
            r#"{{"vm_id":"{}","name":"web","status":"running","flavor":"small","#,
            r#""on_guest_poweroff":"{}","on_guest_poweroff_effective":"restart","#,
            r#""on_guest_poweroff_pending":{},"on_guest_poweroff_reason":{},"stop_reason":null}}"#
        ),
        VM,
        policy,
        reason.is_some(),
        reason.map_or_else(|| "null".to_owned(), |reason| format!(r#""{reason}""#)),
    )
}

fn vm_path() -> String {
    format!("/api/compute/vms/{VM}/")
}

fn policy_path() -> String {
    format!("/api/compute/vms/{VM}/power-policy/")
}

fn hipc(server: &Server, args: &[&str]) -> Run {
    let mut argv = vec!["compute", "vms", "power-policy"];
    argv.extend(args);
    Hipc::against(&Home::new(), server).run(&argv)
}

/// The read is `GET` of the VM, and prints its `vm_id` and the policy fields, each `null` when
/// the answer leaves it out, but `vm_id`, which is the one asked for then; the VM's other
/// fields are not printed. The route is account-scoped.
#[test]
fn get_prints_the_policy_fields_of_the_vm() {
    let server = Server::start(vec![route_of(
        &compute::VM,
        [&VM],
        Reply::json(200, &vm("stop", true)),
    )]);
    let doc = hipc(&server, &["get", VM]).ok();
    assert_eq!(
        doc,
        json!({
            "vm_id": VM,
            "on_guest_poweroff": "stop",
            "on_guest_poweroff_effective": "restart",
            "on_guest_poweroff_pending": true,
            "on_guest_poweroff_reason": "awaiting-ack",
            "stop_reason": null,
        })
    );
    let request = server.only_request();
    assert_eq!(request.method, "GET");
    assert_eq!(request.path(), vm_path());

    let sparse = Server::start(vec![route_of(
        &compute::VM,
        [&VM],
        Reply::json(200, r#"{"on_guest_poweroff":"restart"}"#),
    )]);
    let doc = Hipc::against(&Home::new(), &sparse)
        .run(&[
            "--account",
            MEMBER,
            "compute",
            "vms",
            "power-policy",
            "get",
            VM,
        ])
        .ok();
    assert_eq!(
        doc,
        json!({
            "vm_id": VM,
            "on_guest_poweroff": "restart",
            "on_guest_poweroff_effective": null,
            "on_guest_poweroff_pending": null,
            "on_guest_poweroff_reason": null,
            "stop_reason": null,
        })
    );
    assert_eq!(
        sparse.only_request().header("x-hippius-account"),
        Some(MEMBER)
    );
}

/// The read prints a policy swagger does not list as the server sent it, and fills a `vm_id`
/// that is `null` with the one asked for. `update_refuses_an_answer_with_another_policy`
/// holds the update to the policy it sent.
#[test]
fn get_prints_a_policy_swagger_does_not_list() {
    let server = Server::start(vec![route_of(
        &compute::VM,
        [&VM],
        Reply::json(200, r#"{"vm_id":null,"on_guest_poweroff":"hibernate"}"#),
    )]);
    let doc = hipc(&server, &["get", VM]).ok();
    assert_eq!(doc["on_guest_poweroff"], "hibernate");
    assert_eq!(doc["vm_id"], VM);
}

/// A 200 with an empty body to the read is `internal`, and nothing was written.
#[test]
fn get_refuses_an_empty_answer() {
    let server = Server::start(vec![route_of(&compute::VM, [&VM], Reply::json(200, ""))]);
    let error = hipc(&server, &["get", VM]).fails(1, "internal");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with(&format!("cannot get the power policy of the VM `{VM}`: ")),
        "{error}"
    );
    assert_eq!(error["side_effects"], "none");
}

/// A VM answer without the policy as text is `internal`; nothing was written, so the side
/// effects are `none`.
#[test]
fn get_refuses_an_answer_without_the_policy() {
    for body in [
        r#"{"vm_id":"x"}"#,
        r#"{"on_guest_poweroff":null}"#,
        r#"{"on_guest_poweroff":1}"#,
        "[]",
    ] {
        let server = Server::start(vec![route_of(&compute::VM, [&VM], Reply::json(200, body))]);
        let error = hipc(&server, &["get", VM]).fails(1, "internal");
        assert_eq!(
            error["message"],
            format!(
                "cannot get the power policy of the VM `{VM}`: the API answered without \
                 `on_guest_poweroff`"
            ),
            "{body}"
        );
        assert_eq!(error["side_effects"], "none");
    }
    let missing = Server::start(vec![route_of(
        &compute::VM,
        [&VM],
        Reply::json(404, r#"{"detail":"Not found."}"#),
    )]);
    hipc(&missing, &["get", VM]).fails(1, "not_found");
}

/// The update is one `PUT` of `on_guest_poweroff`, without `--yes` or a question, and prints
/// the policy from the VM the server answers with.
#[test]
fn update_puts_the_policy_and_prints_it() {
    for value in ["restart", "stop"] {
        let server = Server::start(vec![route_of(
            &compute::VM_POWER_POLICY_UPDATE,
            [&VM],
            Reply::json(200, &vm(value, false)),
        )]);
        let doc = hipc(&server, &["update", VM, "--on-guest-poweroff", value]).ok();
        assert_eq!(doc["on_guest_poweroff"], value);
        assert_eq!(doc["vm_id"], VM);
        assert_eq!(doc.get("flavor"), None, "only the policy is printed");
        let request = server.only_request();
        assert_eq!(request.method, "PUT");
        assert_eq!(request.path(), policy_path());
        assert_eq!(request.json(), json!({ "on_guest_poweroff": value }));
    }
}

/// An answer that holds another policy, or none, after the `PUT` is `internal`, and `partial`:
/// the write went through, and what it set is unknown.
#[test]
fn update_refuses_an_answer_with_another_policy() {
    let cases = [
        (
            vm("restart", false),
            "the API answered with `on_guest_poweroff` `restart`",
        ),
        (
            r#"{"vm_id":"x"}"#.to_owned(),
            "the API answered without `on_guest_poweroff`",
        ),
    ];
    for (body, why) in cases {
        let server = Server::start(vec![route_of(
            &compute::VM_POWER_POLICY_UPDATE,
            [&VM],
            Reply::json(200, &body),
        )]);
        let error =
            hipc(&server, &["update", VM, "--on-guest-poweroff", "stop"]).fails(1, "internal");
        assert_eq!(
            error["message"],
            format!(
                "cannot set the VM `{VM}` to stay stopped when its own operating system powers \
                 it off: {why}"
            )
        );
        assert_eq!(error["side_effects"], "partial");
        assert_eq!(error["http_status"], 200);
    }
}

const BILLED: &str = "note: once applied, a VM shut down from inside stays stopped, and is \
                      still billed as a stopped VM until it is deleted\n";
const PENDING: &str = "note: the VM's server has not applied the policy yet: \
                       `on_guest_poweroff_pending` stays true until it does\n";
const HOST_UNSUPPORTED: &str = "note: the VM's server does not support `stop` yet, so it \
                                restarts the VM meanwhile when its own operating system powers \
                                it off\n";

/// Runs `hipc --output text compute vms power-policy update VM --on-guest-poweroff <policy>`
/// against a server that answers with `body`, and returns what it printed on stderr.
fn text_update_notes(policy: &str, body: &str) -> String {
    let server = Server::start(vec![route_of(
        &compute::VM_POWER_POLICY_UPDATE,
        [&VM],
        Reply::json(200, body),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&[
        "--output",
        "text",
        "compute",
        "vms",
        "power-policy",
        "update",
        VM,
        "--on-guest-poweroff",
        policy,
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    run.stderr().to_owned()
}

/// Text mode notes that `stop` leaves a VM shut down from inside billed until it is deleted
/// (swagger: "exactly like a stop"), and, until the VM's server acknowledges the policy, that
/// it is pending. A server that does not support `stop` gets only its own note, which the bill
/// note would contradict. `restart` applied gets no note, and JSON mode prints none.
#[test]
fn update_notes_the_bill_and_a_policy_the_server_has_not_applied() {
    assert_eq!(
        text_update_notes("stop", &vm("stop", true)),
        format!("{BILLED}{PENDING}")
    );
    assert_eq!(text_update_notes("stop", &vm("stop", false)), BILLED);
    assert_eq!(text_update_notes("restart", &vm("restart", false)), "");
    assert_eq!(text_update_notes("restart", &vm("restart", true)), PENDING);
    assert_eq!(
        text_update_notes("stop", &vm_pending_for("stop", Some("host-unsupported"))),
        HOST_UNSUPPORTED,
        "a server without `stop` restarts the VM, so the stopped bill would contradict it"
    );
    // A pending policy without a reason is on its way, as `awaiting-ack` is.
    let no_reason = r#"{"on_guest_poweroff":"restart","on_guest_poweroff_pending":true}"#;
    assert_eq!(text_update_notes("restart", no_reason), PENDING);
    let server = Server::start(vec![route_of(
        &compute::VM_POWER_POLICY_UPDATE,
        [&VM],
        Reply::json(200, &vm("stop", true)),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&[
        "compute",
        "vms",
        "power-policy",
        "update",
        VM,
        "--on-guest-poweroff",
        "stop",
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(run.stderr(), "");
}

/// The read notes a pending policy as the update does, by its reason: a server that does not
/// support `stop` restarts the VM meanwhile, and `awaiting-ack` is on its way. An applied
/// policy gets no note.
#[test]
fn get_notes_a_pending_policy_by_its_reason() {
    for (body, note) in [
        (
            vm_pending_for("stop", Some("host-unsupported")),
            HOST_UNSUPPORTED,
        ),
        (vm_pending_for("stop", Some("awaiting-ack")), PENDING),
        (vm("stop", false), ""),
    ] {
        let server = Server::start(vec![route_of(&compute::VM, [&VM], Reply::json(200, &body))]);
        let run = Hipc::against(&Home::new(), &server).run(&[
            "--output",
            "text",
            "compute",
            "vms",
            "power-policy",
            "get",
            VM,
        ]);
        assert_eq!(run.code(), Some(0), "{}", run.stderr());
        assert_eq!(run.stderr(), note, "{body}");
    }
}

/// The dry run is the `PUT` with its body, needs no token, and sends nothing.
#[test]
fn update_dry_run_is_the_put() {
    let server = Server::start(vec![]);
    let doc = Hipc::tokenless(&Home::new(), &server)
        .run(&[
            "--dry-run",
            "compute",
            "vms",
            "power-policy",
            "update",
            VM,
            "--on-guest-poweroff",
            "restart",
        ])
        .ok();
    assert_eq!(draft(&doc)["method"], "PUT");
    assert_eq!(
        draft(&doc)["url"],
        format!("{}{}", server.url(), policy_path())
    );
    assert_eq!(
        draft(&doc)["body"],
        json!({ "on_guest_poweroff": "restart" })
    );
    assert_eq!(doc["requests"].as_array().map(Vec::len), Some(1));
    assert!(server.requests().is_empty());
}

/// A policy but `restart` or `stop`, a missing policy, or a bad VM id is `usage`, before
/// anything is sent.
#[test]
fn update_refuses_a_bad_policy_or_vm_id() {
    let server = Server::start(vec![]);
    for args in [
        &["update", VM, "--on-guest-poweroff", "shutdown"][..],
        &["update", VM, "--on-guest-poweroff", "Stop"],
        &["update", VM],
        &["update", "../ssh-keys", "--on-guest-poweroff", "stop"],
        &["get", "../ssh-keys"],
    ] {
        hipc(&server, args).fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

/// Swagger: a server that does not support `stop` yet answers 409
/// `power-policy-unsupported-on-host`; the server's words are quoted, and nothing was set.
#[test]
fn update_reports_a_host_without_stop() {
    let server = Server::start(vec![route_of(
        &compute::VM_POWER_POLICY_UPDATE,
        [&VM],
        Reply::json(409, r#"{"error":"power-policy-unsupported-on-host"}"#),
    )]);
    let error = hipc(&server, &["update", VM, "--on-guest-poweroff", "stop"]).fails(1, "conflict");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("power-policy-unsupported-on-host"),
        "{error}"
    );
    assert_eq!(error["side_effects"], "none");
    assert_eq!(
        error["details"],
        serde_json::from_str::<Value>(r#"{"error":"power-policy-unsupported-on-host"}"#).unwrap()
    );
}
