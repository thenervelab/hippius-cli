//! Tenet 15, "Phrasing": a representative message of each kind, checked against the rules
//! a program can check.

use crate::common::{Hipc, Home, Reply, Run, Server, assert_voice, route};

/// Checks the `message` and the `hint` of a JSON error.
fn assert_error_voice(error: &serde_json::Value) {
    assert_voice(error["message"].as_str().expect("a message"));
    if let Some(hint) = error["hint"].as_str() {
        assert_voice(hint);
    }
}

#[test]
fn each_kind_of_error_follows_the_phrasing_rules() {
    let home = Home::new();
    let server = Server::start(vec![route(
        "GET",
        "/api/compute/quota/",
        Reply::json(200, "x"),
    )]);
    let errors = [
        // usage: a malformed flag value.
        Hipc::against(&home, &server)
            .run(&["--account", "nope", "account", "get"])
            .fails(2, "usage"),
        // confirmation_required: a write without `--yes` on a pipe.
        Hipc::against(&home, &server)
            .run(&["s3", "buckets", "create", "photos"])
            .fails(2, "confirmation_required"),
        // reveal_required: a one-time secret without `--reveal`.
        Hipc::against(&home, &server)
            .run(&["s3", "tokens", "create", "--kind", "master", "--yes"])
            .fails(2, "reveal_required"),
        // auth: no token at all.
        Hipc::tokenless(&home, &server)
            .run(&["compute", "quota"])
            .fails(1, "auth"),
        // internal: an answer that is not JSON.
        Hipc::against(&home, &server)
            .run(&["compute", "quota"])
            .fails(1, "internal"),
        // account_scope_unsupported: a membership on a personal-only route.
        Hipc::against(&home, &server)
            .run(&[
                "--account",
                crate::common::MEMBER,
                "drive",
                "subscription",
                "get",
            ])
            .fails(2, "account_scope_unsupported"),
    ];
    for error in &errors {
        assert_error_voice(error);
    }
}

/// A run that succeeded, with exactly one stderr line, which starts with `label`: the text
/// after it.
fn only_line(run: &Run, label: &str) -> String {
    assert_eq!(run.code(), Some(0), "stderr: {}", run.stderr());
    let lines: Vec<&str> = run.stderr().lines().collect();
    assert_eq!(lines.len(), 1, "{lines:?}");
    let text = lines[0].strip_prefix(label);
    text.unwrap_or_else(|| panic!("not a `{label}` line: {lines:?}"))
        .to_owned()
}

#[test]
fn notes_and_warnings_follow_the_phrasing_rules_and_carry_their_label() {
    let home = Home::new();
    let server = Server::start(vec![
        route(
            "POST",
            "/api/compute/vms/vm1/stop/",
            Reply::json(200, r#"{"vm_id":"vm1","status":"stopped"}"#),
        ),
        route(
            "POST",
            "/api/drive/subscription/",
            Reply::json(
                202,
                r#"{"checkout_url":"https://pay.example/x","intent_id":"pi_1"}"#,
            ),
        ),
    ]);
    let stop = Hipc::against(&home, &server)
        .run(&["--output", "text", "compute", "vms", "stop", "vm1", "--yes"]);
    assert_voice(&only_line(&stop, "warning: "));
    let checkout = Hipc::against(&home, &server).run(&[
        "--output",
        "text",
        "--yes",
        "drive",
        "subscription",
        "subscribe",
        "--plan",
        "solo",
        "--pay-from",
        "card",
    ]);
    assert_voice(&only_line(&checkout, "note: "));
}

/// `--quiet` drops notes and warnings, never a failure: in text mode an exit-2 refusal still
/// prints its `error:` and `hint:` lines (tenet 2).
#[test]
fn quiet_still_prints_the_error_and_the_hint_of_a_failure() {
    let run = Hipc::new(&Home::new()).run(&["--quiet", "--output", "text", "auth", "logout"]);
    assert_eq!(run.code(), Some(2));
    assert_eq!(run.stdout(), "");
    assert_eq!(
        run.stderr(),
        "error: this would remove the stored token from the profile `default`, and `--yes` \
         was not passed\nhint: pass `--yes` to confirm\n"
    );
    let run = Hipc::new(&Home::new()).run(&[
        "--quiet", "--output", "text", "compute", "vms", "get", "a/b",
    ]);
    assert_eq!(run.code(), Some(2));
    assert_eq!(
        run.stderr(),
        "error: invalid value `a/b` for `<VM_ID>`: a VM id is 1 to 64 letters, digits, `-`, \
         and `_`\nhint: run `hipc compute vms get --help`\n"
    );
}

/// The checker itself refuses what tenet 15 forbids.
#[test]
fn the_voice_checker_refuses_what_tenet_15_forbids() {
    for bad in [
        "No VM with that id.",
        "the VM is gone.",
        "unexpected argument '--bogus' found",
        "the token is \"wrong\"",
        "you used an expired token",
        "please pass `--yes`",
        "something went wrong",
        "an unclosed `literal",
        "it failed!",
        " the VM is gone",
        "no VM. try again",
        "pass'--yes'",
        "the users' VMs",
    ] {
        let refused = std::panic::catch_unwind(|| assert_voice(bad));
        assert!(refused.is_err(), "accepted: {bad}");
    }
    for good in [
        "no VM with id `vm_123`",
        "cannot delete the VM `vm_1`: the API says `VM is still starting.`",
        "the API's `next` link `x` has no page number",
        "write the rules as `{\"rules\": [...]}`",
        "`--account` is `nope`",
        "run `hipc api POST '/a b/' --reveal`",
        "invalid value `0` for `--limit <LIMIT>`: 0 is not in `1..=500`",
    ] {
        assert_voice(good);
    }
}
