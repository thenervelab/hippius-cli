//! `hipc billing gamma`: the subnet 75 gamma deposits, a hotkey link signed with the hotkey's
//! words, its unlink, and the conversions of gamma into credits.

use serde_json::json;

use crate::common::{Hipc, Home, MEMBER, PHRASES, Reply, Route, Server, route};

const OVERVIEW: &str = "/api/billing/gamma/";
const CHALLENGE: &str = "/api/billing/gamma/hotkeys/challenge/";
const LINK: &str = "/api/billing/gamma/hotkeys/";
const CONVERSIONS: &str = "/api/billing/gamma/conversions/";

/// The SS58 address of `PHRASES[0]`, the hotkey its words make.
const HOTKEY: &str = "5DfhGyQdFobKM8NsWvEeAKk5EQQgYe9AydgJ7rMB6E1EqRzV";
/// Another valid address, `PHRASES[1]`'s.
const OTHER: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
const NONCE: &str = "4f1c2b3a49d8e7f6";
const ID: &str = "6f0b2c1e-7b4d-4e8a-9c01-5d6e7f8a5f60";

fn message(hotkey: &str) -> String {
    format!("Hippius: pay with the gamma of {hotkey} for 41 nonce {NONCE}")
}

fn challenge_for(hotkey: &str) -> Route {
    let body = json!({
        "nonce": NONCE,
        "hotkey": hotkey,
        "message": message(hotkey),
        "expires_at": "2026-11-09T10:05:00+00:00",
    });
    route("POST", CHALLENGE, Reply::json(201, &body.to_string()))
}

fn linked() -> Route {
    let body = json!({"hotkey": HOTKEY, "linked_at": "2026-11-09T10:00:30+00:00"});
    route("POST", LINK, Reply::json(201, &body.to_string()))
}

fn conversion(status: &str) -> String {
    json!({
        "id": ID, "hotkey": HOTKEY, "amount": "10.000000000", "status": status,
        "reference": "0x9a", "transaction_id": null,
    })
    .to_string()
}

fn hotkey_file(home: &Home) -> String {
    let path = home.file("hotkey.txt", format!("{}\n", PHRASES[0]).as_bytes());
    path.to_str()
        .expect("the temporary path is UTF-8")
        .to_owned()
}

#[test]
fn get_prints_the_spender_and_the_deposits() {
    let body = json!({
        "enabled": true, "netuid": 75, "spender": OTHER, "min_amount": "0.100000000",
        "hotkeys": [{"hotkey": HOTKEY, "deposit": {"balance": "25.000000000",
                     "spendable": "25.000000000", "names_hippius": true}}],
    });
    let server = Server::start(vec![route(
        "GET",
        OVERVIEW,
        Reply::json(200, &body.to_string()),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["billing", "gamma", "get"])
        .ok();
    assert_eq!(doc, body);
    assert_eq!(server.only_request().target, OVERVIEW);
}

#[test]
fn a_hotkey_is_linked_with_its_signature_of_the_challenge() {
    let home = Home::new();
    let server = Server::start(vec![challenge_for(HOTKEY), linked()]);
    let file = hotkey_file(&home);
    let doc = Hipc::against(&home, &server)
        .run(&[
            "billing",
            "gamma",
            "hotkeys",
            "create",
            "--hotkey-file",
            &file,
        ])
        .ok();
    assert_eq!(doc["hotkey"], HOTKEY);
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path(), CHALLENGE);
    assert_eq!(requests[0].json(), json!({"hotkey": HOTKEY}));
    assert_eq!(requests[1].path(), LINK);
    let sent = requests[1].json();
    assert_eq!(sent["nonce"], NONCE);
    let signature = sent["signature"].as_str().unwrap();
    assert_eq!(signature.len(), 130, "{signature}");
    assert!(signature.starts_with("0x"));
}

#[test]
fn the_words_can_come_from_stdin() {
    let server = Server::start(vec![challenge_for(HOTKEY), linked()]);
    let doc = Hipc::against(&Home::new(), &server)
        .stdin(format!("{}\n", PHRASES[0]).as_bytes())
        .run(&[
            "billing",
            "gamma",
            "hotkeys",
            "create",
            "--hotkey-file",
            "-",
        ])
        .ok();
    assert_eq!(doc["hotkey"], HOTKEY);
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn a_challenge_for_another_hotkey_is_never_signed() {
    let home = Home::new();
    let server = Server::start(vec![challenge_for(OTHER), linked()]);
    let file = hotkey_file(&home);
    let error = Hipc::against(&home, &server)
        .run(&[
            "billing",
            "gamma",
            "hotkeys",
            "create",
            "--hotkey-file",
            &file,
        ])
        .fails(1, "internal");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("signed nothing"),
        "{error}"
    );
    // Only the challenge was asked for: nothing was signed or sent to the link.
    assert_eq!(server.only_request().path(), CHALLENGE);
}

#[test]
fn a_challenge_without_its_message_is_unreadable() {
    let home = Home::new();
    let server = Server::start(vec![route(
        "POST",
        CHALLENGE,
        Reply::json(201, &json!({"nonce": NONCE}).to_string()),
    )]);
    let file = hotkey_file(&home);
    Hipc::against(&home, &server)
        .run(&[
            "billing",
            "gamma",
            "hotkeys",
            "create",
            "--hotkey-file",
            &file,
        ])
        .fails(1, "internal");
    assert_eq!(server.only_request().path(), CHALLENGE);
}

#[test]
fn a_dry_run_link_shows_both_requests_and_sends_none() {
    let home = Home::new();
    let server = Server::start(vec![]);
    let file = hotkey_file(&home);
    let doc = Hipc::against(&home, &server)
        .run(&[
            "--dry-run",
            "billing",
            "gamma",
            "hotkeys",
            "create",
            "--hotkey-file",
            &file,
        ])
        .dry_ok();
    let requests = doc["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].to_string().contains(HOTKEY));
    assert!(server.requests().is_empty());
}

#[test]
fn a_missing_or_unprivate_hotkey_file_is_refused_without_its_words() {
    let home = Home::new();
    let missing = home.path().join("absent.txt");
    let error = Hipc::new(&home)
        .run(&[
            "billing",
            "gamma",
            "hotkeys",
            "create",
            "--hotkey-file",
            missing.to_str().unwrap(),
        ])
        .fails(2, "usage");
    let text: String = error["message"].as_str().unwrap().to_owned();
    assert!(text.contains("the hotkey file"), "{text}");
    let bad = home.file("bad.txt", b"not words at all");
    let error = Hipc::new(&home)
        .run(&[
            "billing",
            "gamma",
            "hotkeys",
            "create",
            "--hotkey-file",
            bad.to_str().unwrap(),
        ])
        .fails(2, "usage");
    let text = error["message"].as_str().unwrap();
    assert!(text.contains("the hotkey phrase"), "{text}");
    assert!(!text.contains("access key"), "{text}");
    // A file the group or others can read is refused before its words are read.
    let open = home.file("open.txt", format!("{}\n", PHRASES[0]).as_bytes());
    std::fs::set_permissions(&open, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    let error = Hipc::new(&home)
        .run(&[
            "billing",
            "gamma",
            "hotkeys",
            "create",
            "--hotkey-file",
            open.to_str().unwrap(),
        ])
        .fails(2, "usage");
    assert!(
        error["message"].as_str().unwrap().contains("hotkey file"),
        "{error}"
    );
}

#[test]
fn an_unlink_is_confirmed_and_prints_what_it_did() {
    let path = format!("{LINK}{HOTKEY}/");
    let server = Server::start(vec![route("DELETE", &path, Reply::json(204, ""))]);
    Hipc::against(&Home::new(), &server)
        .run(&["billing", "gamma", "hotkeys", "delete", HOTKEY])
        .fails(2, "confirmation_required");
    assert!(server.requests().is_empty());
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["billing", "gamma", "hotkeys", "delete", HOTKEY, "--yes"])
        .ok();
    assert_eq!(doc, json!({"hotkey": HOTKEY, "deleted": true}));
    assert_eq!(server.only_request().target, path);
}

#[test]
fn a_conversion_is_confirmed_and_names_the_read_that_follows_it() {
    let server = Server::start(vec![route(
        "POST",
        CONVERSIONS,
        Reply::json(202, &conversion("pending")),
    )]);
    let args = [
        "billing",
        "gamma",
        "conversions",
        "create",
        "--hotkey",
        HOTKEY,
        "--amount",
        "10",
    ];
    Hipc::against(&Home::new(), &server)
        .run(&args)
        .fails(2, "confirmation_required");
    assert!(server.requests().is_empty());
    let mut confirmed = vec!["--output", "text"];
    confirmed.extend_from_slice(&args);
    confirmed.push("--yes");
    let run = Hipc::against(&Home::new(), &server).run(&confirmed);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(
        run.stderr()
            .contains(&format!("hipc billing gamma conversions get {ID}")),
        "{}",
        run.stderr()
    );
    let sent = server.only_request().json();
    assert_eq!(sent, json!({"hotkey": HOTKEY, "amount": "10"}));
}

#[test]
fn a_conversion_answer_without_an_id_is_a_partial_failure_with_the_answer() {
    let server = Server::start(vec![route(
        "POST",
        CONVERSIONS,
        Reply::json(202, r#"{"status":"pending"}"#),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&[
        "billing",
        "gamma",
        "conversions",
        "create",
        "--hotkey",
        HOTKEY,
        "--amount",
        "0.1",
        "--yes",
    ]);
    assert_eq!(run.code(), Some(1), "{}", run.stderr());
    assert_eq!(run.json(), json!({"status": "pending"}));
    let error: serde_json::Value = serde_json::from_str(run.stderr()).unwrap();
    assert_eq!(error["error"]["code"], "partial_failure");
    assert_eq!(error["error"]["side_effects"], "partial");
    assert!(
        error["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("hipc billing gamma conversions list")
    );
}

#[test]
fn an_amount_or_a_hotkey_the_api_cannot_take_is_refused_before_sending() {
    for (hotkey, amount) in [
        (HOTKEY, "0.05"),
        (HOTKEY, "0"),
        (HOTKEY, "1.0000000001"),
        (HOTKEY, "ten"),
        (HOTKEY, "1."),
        (HOTKEY, ".5"),
        ("5FHn/../x", "1"),
        ("short", "1"),
        ("5DfhGyQdFobKM8NsWvEeAKk5EQQgYe9AydgJ7rMB6E1EqRz0", "1"),
    ] {
        Hipc::new(&Home::new())
            .run(&[
                "billing",
                "gamma",
                "conversions",
                "create",
                "--hotkey",
                hotkey,
                "--amount",
                amount,
                "--yes",
            ])
            .fails(2, "usage");
    }
    Hipc::new(&Home::new())
        .run(&["billing", "gamma", "conversions", "get", "not-a-uuid"])
        .fails(2, "usage");
    // A hotkey goes into the unlink's URL path, so no `/`, `?`, or `.` gets past.
    for hotkey in [
        "5DfhGyQdFobKM8NsWvEeAKk5EQQgYe9AydgJ7rMB6E/../x",
        "5DfhGyQdFobKM8NsWvEeAKk5EQQgYe9AydgJ7rMB6E1E?q=1",
        "5DfhGyQdFobKM8NsWvEeAKk5EQQgYe9AydgJ7rMB6E1Eq.z",
    ] {
        Hipc::new(&Home::new())
            .run(&["billing", "gamma", "hotkeys", "delete", hotkey, "--yes"])
            .fails(2, "usage");
    }
}

#[test]
fn the_conversions_are_read_one_or_all() {
    let one = format!("{CONVERSIONS}{ID}/");
    let list = format!(r#"{{"results":[{}]}}"#, conversion("spent"));
    let server = Server::start(vec![
        route("GET", CONVERSIONS, Reply::json(200, &list)),
        route("GET", &one, Reply::json(200, &conversion("spent"))),
    ]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["billing", "gamma", "conversions", "list"])
        .ok();
    assert_eq!(doc["results"][0]["status"], "spent");
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["billing", "gamma", "conversions", "get", ID])
        .ok();
    assert_eq!(doc["id"], ID);
    let targets: Vec<String> = server.requests().iter().map(|r| r.target.clone()).collect();
    assert_eq!(targets, vec![CONVERSIONS.to_owned(), one]);
}

#[test]
fn a_membership_is_refused_before_anything_is_sent() {
    let server = Server::start(vec![]);
    let id = ID;
    for args in [
        &["billing", "gamma", "get"][..],
        &["billing", "gamma", "conversions", "list"][..],
        &["billing", "gamma", "conversions", "get", id][..],
        &["billing", "gamma", "hotkeys", "delete", HOTKEY, "--yes"][..],
        &[
            "billing",
            "gamma",
            "conversions",
            "create",
            "--hotkey",
            HOTKEY,
            "--amount",
            "1",
            "--yes",
        ][..],
    ] {
        let mut full = vec!["--account", MEMBER];
        full.extend_from_slice(args);
        Hipc::against(&Home::new(), &server)
            .run(&full)
            .fails(2, "account_scope_unsupported");
    }
    assert!(server.requests().is_empty());
}
