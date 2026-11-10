//! `hipc network create`, the `POST /api/network/provision/` that sets up the account's
//! private network, and the error every network command gives while it is not set up.

use serde_json::{Value, json};

use crate::common::{
    BILLING, Hipc, Home, MEMBER, PROJECT, Reply, Server, draft, one_request_dry_run, route,
    route_each,
};
use crate::network::{COMMANDS, SUMMARY};

const PROVISION: &str = "/api/network/provision/";

/// The live answer of a network read before the network exists (2026-11-03).
const NOT_SET_UP: &str = r#"{"detail":"You must call /api/network/provision/ first."}"#;

/// The hint of a network that is not set up, for a run by [`Hipc::against`] `server` given
/// `globals` (each followed by a space) before the origin it carries.
fn create_hint(server: &Server, globals: &str) -> String {
    format!(
        "run `hipc {globals}--api-url {} network create --yes` to create the account's \
         private network",
        server.url()
    )
}

#[test]
fn create_with_yes_sends_a_post_without_a_body_and_prints_the_summary() {
    let server = Server::start(vec![route("POST", PROVISION, Reply::json(200, SUMMARY))]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["network", "create", "--yes"])
        .ok();
    assert_eq!(doc, serde_json::from_str::<Value>(SUMMARY).unwrap());
    let request = server.only_request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, PROVISION);
    assert_eq!(request.body, b"");
    assert_eq!(request.header("x-hippius-account"), None);
}

/// The first call creates the network (201) and a repeat finds it (200); each prints the
/// summary the server answered, and both are the same POST.
#[test]
fn a_repeat_create_prints_the_existing_network() {
    let existing = SUMMARY.replace(r#""peers":2"#, r#""peers":5"#);
    let server = Server::start(vec![route_each(
        "POST",
        PROVISION,
        vec![Reply::json(201, SUMMARY), Reply::json(200, &existing)],
    )]);
    let home = Home::new();
    for (answer, peers) in [(SUMMARY, 2), (existing.as_str(), 5)] {
        let doc = Hipc::against(&home, &server)
            .run(&["network", "create", "--yes"])
            .ok();
        assert_eq!(doc, serde_json::from_str::<Value>(answer).unwrap());
        assert_eq!(doc["peers"], peers);
    }
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    for request in requests {
        assert_eq!(
            (request.method.as_str(), request.target.as_str()),
            ("POST", PROVISION)
        );
        assert_eq!(request.body, b"");
    }
}

#[test]
fn create_without_yes_is_refused_and_sends_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let error = Hipc::against(&home, &server)
        .run(&["network", "create"])
        .fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        "this would create the private network, and `--yes` was not passed"
    );
    assert_eq!(error["hint"], "pass `--yes` to confirm");
    // `--no-input` never confirms either.
    Hipc::against(&home, &server)
        .run(&["network", "create", "--no-input"])
        .fails(2, "confirmation_required");
    assert!(server.requests().is_empty());
}

/// Under a membership the question names the account whose network it creates, and the
/// request carries its header.
#[test]
fn a_membership_is_named_in_the_question_and_sent_as_the_header() {
    let server = Server::start(vec![route("POST", PROVISION, Reply::json(200, SUMMARY))]);
    let home = Home::new();
    let error = Hipc::against(&home, &server)
        .run(&["--account", MEMBER, "network", "create"])
        .fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!(
            "this would create the private network in the account `{MEMBER}`, and `--yes` \
             was not passed"
        )
    );
    assert!(server.requests().is_empty());
    Hipc::against(&home, &server)
        .run(&["--account", MEMBER, "network", "create", "--yes"])
        .ok();
    assert_eq!(
        server.only_request().header("x-hippius-account"),
        Some(MEMBER)
    );
}

/// A dry run needs no token and no `--yes`, and sends nothing.
#[test]
fn a_dry_run_needs_no_token_and_sends_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    for account in [None, Some(MEMBER)] {
        let mut args = vec!["--dry-run"];
        if let Some(account) = account {
            args.extend(["--account", account]);
        }
        args.extend(["network", "create"]);
        let doc = Hipc::tokenless(&home, &server)
            .env_remove("HIPPIUS_TOKEN")
            .run(&args)
            .dry_ok();
        assert_eq!(
            doc,
            one_request_dry_run(json!({
                "method": "POST",
                "url": format!("{}{PROVISION}", server.url()),
                "account_header": account,
                "project_header": null,
                "billing_header": null,
                "body": null,
            })),
            "{account:?}"
        );
    }
    assert!(server.requests().is_empty());
}

/// The create is a write: a project stored in the profile is refused, a dry run included,
/// rather than skipped as the reads skip it; a stored billing account is skipped, as on
/// every route that takes none.
#[test]
fn a_stored_project_is_refused_and_a_stored_billing_account_skipped() {
    let server = Server::start(vec![]);
    let home = Home::new();
    Hipc::new(&home)
        .run(&["account", "projects", "use", PROJECT])
        .ok();
    for extra in ["--yes", "--dry-run"] {
        Hipc::against(&home, &server)
            .run(&["network", "create", extra])
            .fails(2, "project_scope_unsupported");
    }
    let home = Home::new();
    Hipc::new(&home)
        .run(&["account", "billing-accounts", "use", BILLING])
        .ok();
    let doc = Hipc::against(&home, &server)
        .run(&["network", "create", "--dry-run"])
        .ok();
    assert!(!doc.to_string().contains(BILLING), "{doc}");
    assert!(server.requests().is_empty());
}

/// A 5xx after the POST was sent may have created the network: `partial`, not retryable.
/// A 4xx did nothing.
#[test]
fn a_failed_create_says_what_may_have_happened() {
    let home = Home::new();
    let server = Server::start(vec![route(
        "POST",
        PROVISION,
        Reply::json(502, r#"{"detail":"bad gateway"}"#),
    )]);
    let error = Hipc::against(&home, &server)
        .run(&["network", "create", "--yes"])
        .fails(1, "unavailable");
    assert_eq!(error["side_effects"], "partial");
    assert_eq!(error["retryable"], false);
    let suspended = Server::start(vec![route(
        "POST",
        PROVISION,
        Reply::json(403, r#"{"detail":"suspended"}"#),
    )]);
    let error = Hipc::against(&home, &suspended)
        .run(&["network", "create", "--yes"])
        .fails(1, "permission");
    assert_eq!(error["side_effects"], "none");
    assert_eq!(
        error["message"],
        "cannot create the private network: the API says `suspended`"
    );
}

/// Every network command that meets the API's "call provision first" 403 says the network
/// is not set up and names the command that sets it up. The code stays `permission`, the
/// code of a 403 without an account code, with the status and the server's body kept.
#[test]
fn every_command_on_a_network_not_set_up_names_the_create() {
    let home = Home::new();
    let server = Server::start(vec![
        route("GET", "/api/network/me/", Reply::json(403, NOT_SET_UP)),
        route("POST", PROVISION, Reply::json(403, NOT_SET_UP)),
        route("GET", "/api/network/peers/", Reply::json(403, NOT_SET_UP)),
        route(
            "GET",
            "/api/network/peers/12/",
            Reply::json(403, NOT_SET_UP),
        ),
        route(
            "PATCH",
            "/api/network/peers/12/",
            Reply::json(403, NOT_SET_UP),
        ),
        route(
            "DELETE",
            "/api/network/peers/12/",
            Reply::json(403, NOT_SET_UP),
        ),
        route("GET", "/api/network/groups/", Reply::json(403, NOT_SET_UP)),
        route(
            "GET",
            "/api/network/policies/",
            Reply::json(403, NOT_SET_UP),
        ),
        route(
            "GET",
            "/api/network/networks/",
            Reply::json(403, NOT_SET_UP),
        ),
        route(
            "GET",
            "/api/network/networks/3/resources/",
            Reply::json(403, NOT_SET_UP),
        ),
        route(
            "GET",
            "/api/network/networks/3/routers/",
            Reply::json(403, NOT_SET_UP),
        ),
    ]);
    let mapped: Vec<&[&str]> = COMMANDS
        .into_iter()
        .filter(|args| args[1] != "create")
        .collect();
    assert_eq!(mapped.len(), COMMANDS.len() - 1);
    for args in mapped {
        let error = Hipc::against(&home, &server)
            .run(args)
            .fails(1, "permission");
        let message = error["message"].as_str().unwrap();
        assert!(message.starts_with("cannot "), "{message}");
        assert!(
            message.ends_with(": the private network is not set up for this account"),
            "{message}"
        );
        assert!(!message.contains("the API says"), "{message}");
        assert_eq!(error["hint"], create_hint(&server, ""), "{args:?}");
        assert_eq!(error["retryable"], false, "{args:?}");
        assert_eq!(error["side_effects"], "none", "{args:?}");
        assert_eq!(error["http_status"], 403, "{args:?}");
        assert_eq!(
            error["details"],
            serde_json::from_str::<Value>(NOT_SET_UP).unwrap()
        );
    }
    // The create is the fix the hint names, so its own refusal keeps the server's words.
    let error = Hipc::against(&home, &server)
        .run(&["network", "create", "--yes"])
        .fails(1, "permission");
    assert_eq!(
        error["message"],
        "cannot create the private network: the API says `You must call \
         /api/network/provision/ first.`"
    );
    assert_eq!(error["hint"], Value::Null);
    assert_eq!(server.requests().len(), COMMANDS.len());
}

/// Only the server's sentence is cut from the message: an action that holds the same words,
/// such as a rename to a name that quotes them, keeps them.
#[test]
fn the_message_keeps_an_action_that_holds_the_servers_phrase() {
    let server = Server::start(vec![route(
        "PATCH",
        "/api/network/peers/12/",
        Reply::json(403, NOT_SET_UP),
    )]);
    let name = "a: the API says b";
    let error = Hipc::against(&Home::new(), &server)
        .run(&["network", "peers", "update", "12", "--name", name, "--yes"])
        .fails(1, "permission");
    assert_eq!(
        error["message"],
        format!(
            "cannot rename the network peer `12` to `{name}`: the private network is not set \
             up for this account"
        )
    );
}

/// The hint carries the account this run acted in, from `--account` or from
/// `HIPPIUS_ACCOUNT`, and the profile, so the network it creates is the one the read found
/// missing.
#[test]
fn the_hint_carries_the_runs_account_and_profile() {
    let server = Server::start(vec![route(
        "GET",
        "/api/network/peers/",
        Reply::json(403, NOT_SET_UP),
    )]);
    let home = Home::new();
    let by_flag = Hipc::against(&home, &server)
        .run(&["--account", MEMBER, "network", "peers", "list"])
        .fails(1, "permission");
    let by_variable = Hipc::against(&home, &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["network", "peers", "list"])
        .fails(1, "permission");
    for error in [by_flag, by_variable] {
        assert_eq!(
            error["hint"],
            format!(
                "run `hipc --api-url {} --account {MEMBER} network create --yes` to create the \
                 account's private network",
                server.url()
            )
        );
    }
    let error = Hipc::against(&home, &server)
        .run(&["--profile", "work", "network", "peers", "list"])
        .fails(1, "permission");
    assert_eq!(error["hint"], create_hint(&server, "--profile work "));
    // A profile name that is not one plain word cannot be repeated safely, so the hint
    // names the bare command.
    let spaced = Home::new();
    spaced.write_config("[profiles.\"my work\"]\n");
    let error = Hipc::against(&spaced, &server)
        .env("HIPPIUS_PROFILE", "my work")
        .run(&["network", "peers", "list"])
        .fails(1, "permission");
    assert_eq!(
        error["hint"],
        "run `hipc network create --yes` to create the account's private network"
    );
    for request in server.requests() {
        assert_eq!(request.path(), "/api/network/peers/");
    }
}

/// The lists name what they were doing before the plain words, in JSON and in text mode.
#[test]
fn a_list_on_a_network_not_set_up_says_so_in_both_modes() {
    let lists: [(&[&str], &str, &str); 4] = [
        (
            &["network", "peers", "list"],
            "/api/network/peers/",
            "list the network's peers",
        ),
        (
            &["network", "groups", "list"],
            "/api/network/groups/",
            "list the network's groups",
        ),
        (
            &["network", "policies", "list"],
            "/api/network/policies/",
            "list the network's policies",
        ),
        (
            &["network", "vpcs", "list"],
            "/api/network/networks/",
            "list the VPC networks",
        ),
    ];
    let home = Home::new();
    for (args, path, action) in lists {
        let server = Server::start(vec![route("GET", path, Reply::json(403, NOT_SET_UP))]);
        let message =
            format!("cannot {action}: the private network is not set up for this account");
        let error = Hipc::against(&home, &server)
            .run(args)
            .fails(1, "permission");
        assert_eq!(error["message"], message.as_str());
        let run = Hipc::against(&home, &server).run(&[&["--output", "text"], args].concat());
        assert_eq!(run.code(), Some(1));
        assert_eq!(run.stdout(), "");
        assert_eq!(
            run.stderr(),
            format!("error: {message}\nhint: {}\n", create_hint(&server, ""))
        );
    }
}

/// Only that 403 is mapped: another 403, one that names the route in another sentence, one
/// that carries an account code, one without a `detail`, a `detail` that is not text, or
/// another status keeps the server's words and has no create hint.
#[test]
fn any_other_refusal_keeps_the_servers_words() {
    let home = Home::new();
    for (status, body, code) in [
        (403, r#"{"detail":"suspended"}"#, "permission"),
        (
            403,
            r#"{"detail":"Only an owner may call /api/network/provision/."}"#,
            "permission",
        ),
        (
            403,
            r#"{"detail":"Missing grant to call /api/network/provision/."}"#,
            "permission",
        ),
        (
            403,
            r#"{"detail":"You must call /api/network/provision/ first. Ask the owner."}"#,
            "permission",
        ),
        (
            403,
            r#"{"detail":"Note: You must call /api/network/provision/ first."}"#,
            "permission",
        ),
        (
            403,
            r#"{"detail":"You must call /api/network/provision/ first.","code":"owner_only"}"#,
            "owner_only",
        ),
        (
            403,
            r#"{"detail":"You must call /api/network/provision/ first.","code":"not_a_member"}"#,
            "not_a_member",
        ),
        (
            403,
            concat!(
                r#"{"detail":"You must call /api/network/provision/ first.","#,
                r#""code":"insufficient_grant"}"#
            ),
            "insufficient_grant",
        ),
        (
            403,
            r#"{"error":"You must call /api/network/provision/ first."}"#,
            "permission",
        ),
        (
            403,
            r#"{"detail":["You must call /api/network/provision/ first."]}"#,
            "permission",
        ),
        (403, "[]", "permission"),
        (404, NOT_SET_UP, "not_found"),
        (409, NOT_SET_UP, "conflict"),
    ] {
        let server = Server::start(vec![route(
            "GET",
            "/api/network/peers/",
            Reply::json(status, body),
        )]);
        let error = Hipc::against(&home, &server)
            .run(&["network", "peers", "list"])
            .fails(1, code);
        let message = error["message"].as_str().unwrap();
        assert!(
            !message.contains("not set up"),
            "{status} {body}: {message}"
        );
        let hint = error["hint"].as_str().unwrap_or_default();
        assert!(!hint.contains("network create"), "{status} {body}: {hint}");
    }
}

/// `hipc api` sends the same POST only when confirmed, and refuses a stored project on it,
/// as on every write, while a stored billing account is left off it.
#[test]
fn hipc_api_confirms_the_provision_post_and_refuses_a_stored_project() {
    let server = Server::start(vec![route("POST", PROVISION, Reply::json(200, SUMMARY))]);
    let home = Home::new();
    for path in ["/network/provision/", "/network/provision", PROVISION] {
        Hipc::against(&home, &server)
            .run(&["api", "POST", path])
            .fails(2, "confirmation_required");
    }
    assert!(server.requests().is_empty());
    let doc = Hipc::against(&home, &server)
        .run(&["api", "POST", "/network/provision/", "--yes"])
        .ok();
    assert_eq!(doc["status"], "active");
    assert_eq!(server.only_request().body, b"");
    let home = Home::new();
    Hipc::new(&home)
        .run(&["account", "projects", "use", PROJECT])
        .ok();
    for extra in ["--yes", "--dry-run"] {
        Hipc::against(&home, &server)
            .run(&["api", "POST", "/network/provision/", extra])
            .fails(2, "project_scope_unsupported");
    }
    let home = Home::new();
    Hipc::new(&home)
        .run(&["account", "billing-accounts", "use", BILLING])
        .ok();
    let doc = Hipc::against(&home, &server)
        .run(&["api", "POST", "/network/provision/", "--dry-run"])
        .ok();
    // The dry run names the header the write would carry, `null` for none.
    assert_eq!(
        draft(&doc).get("billing_header"),
        Some(&serde_json::Value::Null),
        "{doc}"
    );
    assert_eq!(server.requests().len(), 1);
}
