//! `hipc network`: the summary, the peers with their rename and remove, the groups, the
//! policies, and the VPC networks with their resources and routers. Every route acts in
//! the selected account, so a membership sends `X-Hippius-Account`, and the network is
//! account-wide, so a project or a billing account is refused. A list is read whole.

use hippius::routes::network;
use serde_json::{Value, json};

use crate::common::{
    BILLING, Hipc, Home, MEMBER, PROJECT, Reply, Server, one_request_dry_run, route, route_of,
};

/// A peer; swagger documents no answer body, and types the peer id as a string.
const PEER: &str = r#"{"id":"12","name":"laptop","ip":"100.64.0.12","connected":true}"#;

/// A rename answer without a `name`, as a server that ignored the body might send: it
/// cannot confirm the rename.
const NAMELESS: &str = r#"{"id":"12","connected":true}"#;

/// A tenant summary; swagger says "status + counts".
pub(crate) const SUMMARY: &str = concat!(
    r#"{"status":"active","group_id":"d1a2b3","groups":3,"networks":1,"peers":2,"#,
    r#""policies":1,"created_at":"2026-10-29T10:00:00Z"}"#
);

/// Every command, with `--yes` on those that are confirmed, so each one reaches the server
/// or a local refusal rather than the question.
pub(crate) const COMMANDS: [&[&str]; 11] = [
    &["network", "get", "--yes"],
    &["network", "create", "--yes"],
    &["network", "peers", "list"],
    &["network", "peers", "get", "12"],
    &["network", "peers", "update", "12", "--name", "b", "--yes"],
    &["network", "peers", "delete", "12", "--yes"],
    &["network", "groups", "list"],
    &["network", "policies", "list"],
    &["network", "vpcs", "list"],
    &["network", "vpcs", "resources", "list", "3"],
    &["network", "vpcs", "routers", "list", "3"],
];

#[test]
fn get_with_yes_prints_the_summary() {
    let server = Server::start(vec![route_of(&network::ME, [], Reply::json(200, SUMMARY))]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["network", "get", "--yes"])
        .ok();
    assert_eq!(doc, serde_json::from_str::<Value>(SUMMARY).unwrap());
    assert_eq!(server.only_request().header("x-hippius-account"), None);
}

/// The first `get` for an account creates its network, and no call can tell beforehand
/// whether it is the first, so every `get` is confirmed.
#[test]
fn get_without_yes_is_refused_and_sends_nothing() {
    let server = Server::start(vec![]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["network", "get"])
        .fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        "this would get the private network, creating it if the account has none, and \
         `--yes` was not passed"
    );
    assert!(server.requests().is_empty());
}

/// `get` counts as a write: a 5xx after it was sent may have created the network, so it
/// is `partial` and not retryable; a 4xx is `none`.
#[test]
fn a_failed_get_says_it_may_have_created_the_network() {
    let server = Server::start(vec![route_of(
        &network::ME,
        [],
        Reply::json(502, r#"{"detail":"bad gateway"}"#),
    )]);
    let home = Home::new();
    let error = Hipc::against(&home, &server)
        .run(&["network", "get", "--yes"])
        .fails(1, "unavailable");
    assert_eq!(error["side_effects"], "partial");
    assert_eq!(error["retryable"], false);
    let suspended = Server::start(vec![route_of(
        &network::ME,
        [],
        Reply::json(403, r#"{"detail":"suspended"}"#),
    )]);
    let error = Hipc::against(&home, &suspended)
        .run(&["network", "get", "--yes"])
        .fails(1, "permission");
    assert_eq!(error["side_effects"], "none");
}

/// A project or billing account stored in the profile is left off the reads, which cannot
/// take one, but `get` may create the network, so a stored project is refused there, a dry
/// run included, and nothing is sent; a stored billing account is left off it too.
#[test]
fn a_stored_selection_is_skipped_on_reads_and_a_stored_project_refused_on_get() {
    let server = Server::start(vec![
        route_of(&network::PEERS, [], Reply::json(200, "[]")),
        route_of(&network::ME, [], Reply::json(200, SUMMARY)),
    ]);
    for (group, id) in [("projects", PROJECT), ("billing-accounts", BILLING)] {
        let home = Home::new();
        Hipc::new(&home).run(&["account", group, "use", id]).ok();
        Hipc::against(&home, &server)
            .run(&["network", "peers", "list"])
            .ok();
        for extra in ["--yes", "--dry-run"] {
            let run = Hipc::against(&home, &server).run(&["network", "get", extra]);
            if group == "projects" {
                run.fails(2, "project_scope_unsupported");
            } else {
                let doc = run.ok();
                assert!(!doc.to_string().contains(BILLING), "{doc}");
            }
        }
    }
    let calls = server.calls();
    assert_eq!(
        calls,
        [
            "GET /api/network/peers/",
            "GET /api/network/peers/",
            "GET /api/network/me/"
        ]
    );
    for request in server.requests() {
        assert_eq!(request.header("x-hippius-project"), None);
        assert_eq!(request.header("x-hippius-billing"), None);
    }
}

/// Each list is read whole, as a bare array or a page with no `next` link, and printed as
/// the envelope with the page's other fields beside the items.
#[test]
fn every_list_prints_the_envelope() {
    let lists: [(&[&str], &str); 6] = [
        (&["network", "peers", "list"], "/api/network/peers/"),
        (&["network", "groups", "list"], "/api/network/groups/"),
        (&["network", "policies", "list"], "/api/network/policies/"),
        (&["network", "vpcs", "list"], "/api/network/networks/"),
        (
            &["network", "vpcs", "resources", "list", "3"],
            "/api/network/networks/3/resources/",
        ),
        (
            &["network", "vpcs", "routers", "list", "net-a_1"],
            "/api/network/networks/net-a_1/routers/",
        ),
    ];
    let home = Home::new();
    for (args, path) in lists {
        let server = Server::start(vec![route("GET", path, Reply::json(200, r#"[{"id":1}]"#))]);
        let doc = Hipc::against(&home, &server).run(args).ok();
        assert_eq!(
            doc,
            json!({"items": [{"id": 1}], "next_page_token": null}),
            "{args:?}"
        );
        let paged = Server::start(vec![route(
            "GET",
            path,
            Reply::json(200, r#"{"count":1,"next":null,"results":[{"id":2}]}"#),
        )]);
        let doc = Hipc::against(&home, &paged).run(args).ok();
        assert_eq!(
            doc,
            json!({"count": 1, "items": [{"id": 2}], "next_page_token": null}),
            "{args:?}"
        );
    }
}

/// Swagger documents no page parameters, so a list takes no `--limit`, `--page-token`, or
/// `--all`, and an answer split into pages is `internal` rather than printed short.
#[test]
fn a_list_split_into_pages_is_internal_and_no_list_takes_page_flags() {
    let server = Server::start(vec![route_of(
        &network::PEERS,
        [],
        Reply::json(
            200,
            r#"{"count":2,"next":"{base}/api/network/peers/?page=2","results":[{"id":1}]}"#,
        ),
    )]);
    let home = Home::new();
    let error = Hipc::against(&home, &server)
        .run(&["network", "peers", "list"])
        .fails(1, "internal");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("the API split a list into pages"),
        "{error}"
    );
    for flag in ["--limit", "--page-token", "--all"] {
        Hipc::new(&home)
            .run(&["network", "peers", "list", flag, "2"])
            .fails(2, "usage");
    }
}

#[test]
fn an_answer_that_is_not_a_list_is_internal() {
    let server = Server::start(vec![route_of(
        &network::GROUPS,
        [],
        Reply::json(200, r#"{"groups":[]}"#),
    )]);
    Hipc::against(&Home::new(), &server)
        .run(&["network", "groups", "list"])
        .fails(1, "internal");
}

#[test]
fn peers_get_prints_the_peer_and_a_missing_one_names_the_list() {
    let server = Server::start(vec![
        route_of(&network::PEER, [&12], Reply::json(200, PEER)),
        route_of(&network::PEER, [&13], Reply::not_found()),
    ]);
    let home = Home::new();
    let doc = Hipc::against(&home, &server)
        .run(&["network", "peers", "get", "12"])
        .ok();
    assert_eq!(doc, serde_json::from_str::<Value>(PEER).unwrap());
    let error = Hipc::against(&home, &server)
        .run(&["network", "peers", "get", "13"])
        .fails(1, "not_found");
    assert_eq!(
        error["hint"],
        "run `hipc network peers list` to see what exists"
    );
}

#[test]
fn a_missing_vpc_network_names_the_list() {
    let server = Server::start(vec![route_of(
        &network::VPC_ROUTERS,
        [&9],
        Reply::not_found(),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["network", "vpcs", "routers", "list", "9"])
        .fails(1, "not_found");
    assert_eq!(
        error["hint"],
        "run `hipc network vpcs list` to see what exists"
    );
}

/// A peer or VPC id goes into the path, so one that could change the route is refused
/// before anything is sent, and the message names the list that prints the ids.
#[test]
fn an_id_that_could_change_the_route_is_usage() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let long = "a".repeat(65);
    for id in ["../12", "12/", "1?x=1", "", "a.b", long.as_str()] {
        for (args, words) in [
            (
                vec!["network", "peers", "get", id],
                "a peer id is 1 to 64 letters, digits, `-`, and `_`, from `hipc network peers list`",
            ),
            (
                vec!["network", "peers", "delete", id, "--yes"],
                "a peer id is 1 to 64",
            ),
            (
                vec!["network", "peers", "update", id, "--name", "x", "--yes"],
                "a peer id is 1 to 64",
            ),
            (
                vec!["network", "vpcs", "resources", "list", id],
                "a VPC network id is 1 to 64 letters, digits, `-`, and `_`, from \
                 `hipc network vpcs list`",
            ),
            (
                vec!["network", "vpcs", "routers", "list", id],
                "a VPC network id is 1 to 64",
            ),
        ] {
            let error = Hipc::against(&home, &server).run(&args).fails(2, "usage");
            assert!(
                error["message"].as_str().unwrap().contains(words),
                "{args:?}: {error}"
            );
        }
    }
    // The longest id is accepted.
    let widest = "a".repeat(64);
    let ok = Server::start(vec![route_of(
        &network::PEER,
        [&widest],
        Reply::json(200, PEER),
    )]);
    Hipc::against(&home, &ok)
        .run(&["network", "peers", "get", &widest])
        .ok();
    assert!(server.requests().is_empty());
}

/// The rename sends `name`, trimmed, and prints the peer the server answers with, renamed:
/// a server that trims answers what was sent, so a padded `--name` is not a false
/// `partial`. The case is kept as given.
#[test]
fn update_with_yes_sends_the_name_trimmed() {
    let renamed = r#"{"id":"12","name":"build Box","ip":"100.64.0.12","connected":true}"#;
    let server = Server::start(vec![route_of(
        &network::PEER_UPDATE,
        [&12],
        Reply::json(200, renamed),
    )]);
    let home = Home::new();
    for name in ["build Box", "  build Box ", "\tbuild Box\n"] {
        let doc = Hipc::against(&home, &server)
            .run(&["network", "peers", "update", "12", "--name", name, "--yes"])
            .ok();
        assert_eq!(
            doc,
            serde_json::from_str::<Value>(renamed).unwrap(),
            "{name:?}"
        );
    }
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    for request in requests {
        assert_eq!(request.json(), json!({"name": "build Box"}));
    }
    // A string or a number `id` is compared as text; an answer without one, or with one of
    // another type, is judged by its name alone.
    for reply in [
        r#"{"name":"build Box"}"#,
        r#"{"id":12,"name":"build Box"}"#,
        r#"{"id":[12],"name":"build Box"}"#,
    ] {
        let unidentified = Server::start(vec![route_of(
            &network::PEER_UPDATE,
            [&12],
            Reply::json(200, reply),
        )]);
        let doc = Hipc::against(&home, &unidentified)
            .run(&[
                "network",
                "peers",
                "update",
                "12",
                "--name",
                "build Box",
                "--yes",
            ])
            .ok();
        assert_eq!(doc, serde_json::from_str::<Value>(reply).unwrap());
    }
    let lowered = Server::start(vec![route_of(
        &network::PEER_UPDATE,
        [&12],
        Reply::json(200, r#"{"id":"12","name":"build box"}"#),
    )]);
    Hipc::against(&home, &lowered)
        .run(&[
            "network",
            "peers",
            "update",
            "12",
            "--name",
            "build Box",
            "--yes",
        ])
        .fails(1, "internal");
}

/// A 2xx whose `name` is not the requested one leaves the rename unconfirmed: the server
/// took the request, and an ignored field or a name it normalised both look like this, so
/// it is `internal`, `partial`, and nothing on stdout. A 2xx without a top-level `name`,
/// a nested peer included, with a `name` that is not a string, or about another peer's
/// `id`, is the same failure, with a message that quotes what the API said; the PATCH was
/// sent in each. The error carries
/// the answer's status and body, so the refusal can be traced. An empty 204 is refused
/// before the name is looked at, with the default `internal` hint.
#[test]
fn a_rename_answer_without_the_new_name_is_internal_and_partial() {
    let home = Home::new();
    let args = [
        "network", "peers", "update", "12", "--name", "build", "--yes",
    ];
    // Not the default `internal` hint on a body without the name: running the rename
    // again would send a PATCH whose effect is unknown, so the hint reads the peer instead.
    let check = "run `hipc network peers get 12` to see the peer's name, and report \
                 what it prints at `https://github.com/thenervelab/hippius-cli/issues`";
    // A rename answered 2xx was done, so running it again is not the fix.
    let report = hippius::CHECK_FIRST_HINT;
    for (status, reply, why, hint) in [
        (200, PEER, "the API says `laptop`", check),
        (200, NAMELESS, "the API answered without `name`", check),
        (200, r#"{"id":"12","name":7}"#, "the API says `7`", check),
        (
            200,
            r#"{"peer":{"id":"12","name":"build"}}"#,
            "the API answered without `name`",
            check,
        ),
        (
            200,
            r#"{"id":"99","name":"build"}"#,
            "the API answered about another peer, `99`",
            check,
        ),
        (
            200,
            r#"{"id":99,"name":"build"}"#,
            "the API answered about another peer, `99`",
            check,
        ),
        (204, "", "the API answered 204 with an empty body", report),
    ] {
        let server = Server::start(vec![route_of(
            &network::PEER_UPDATE,
            [&12],
            Reply::json(status, reply),
        )]);
        let run = Hipc::against(&home, &server).run(&args);
        let error = run.fails(1, "internal");
        assert_eq!(
            error["message"],
            format!("cannot rename the network peer `12` to `build`: {why}"),
            "{status} {reply}"
        );
        assert_eq!(error["hint"], hint, "{status} {reply}");
        assert_eq!(error["side_effects"], "partial");
        assert_eq!(error["retryable"], false);
        assert_eq!(error["http_status"], status);
        let details = serde_json::from_str::<Value>(reply).unwrap_or(Value::Null);
        assert_eq!(error["details"], details, "{status} {reply}");
        assert_eq!(run.stdout(), "", "{status} {reply}");
        let request = server.only_request();
        assert_eq!(request.method, "PATCH");
        assert_eq!(request.json(), json!({"name": "build"}));
    }
}

/// A name that is blank, more than one line, or longer than 255 characters is refused
/// before the question; 255 characters, counted as characters, is sent.
#[test]
fn a_peer_name_outside_one_line_of_255_characters_is_usage() {
    let widest = "é".repeat(255);
    let renamed = json!({"id": "12", "name": widest}).to_string();
    let server = Server::start(vec![route_of(
        &network::PEER_UPDATE,
        [&12],
        Reply::json(200, &renamed),
    )]);
    let home = Home::new();
    let long = "a".repeat(256);
    for name in ["", "   ", "two\nlines", "tab\there", long.as_str()] {
        let error = Hipc::against(&home, &server)
            .run(&["network", "peers", "update", "12", "--name", name, "--yes"])
            .fails(2, "usage");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("a peer name is one line of 1 to 255 characters"),
            "{error}"
        );
    }
    assert!(server.requests().is_empty());
    // The limit counts the trimmed value, which is what is sent.
    let padded = format!(" {widest}\n");
    for name in [widest.as_str(), padded.as_str()] {
        Hipc::against(&home, &server)
            .run(&["network", "peers", "update", "12", "--name", name, "--yes"])
            .ok();
    }
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    for request in requests {
        assert_eq!(request.json()["name"], widest);
    }
}

/// The delete answers 204, so hipc prints its own document.
#[test]
fn delete_with_yes_prints_what_was_removed() {
    let server = Server::start(vec![route_of(
        &network::PEER_DELETE,
        [&12],
        Reply::json(204, ""),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["network", "peers", "delete", "12", "--yes"])
        .ok();
    assert_eq!(doc, json!({"id": "12", "deleted": true}));
    assert_eq!(server.only_request().method, "DELETE");
}

/// Without `--yes` on a pipe a rename and a remove are refused, the refusal names the
/// peer, and nothing is sent.
#[test]
fn update_and_delete_without_yes_are_refused_and_send_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    for (args, action) in [
        (
            vec!["network", "peers", "update", "12", "--name", "build"],
            "this would rename the network peer `12` to `build`, and `--yes` was not passed",
        ),
        (
            vec!["network", "peers", "delete", "12"],
            "this would remove the peer `12` from the private network, and `--yes` was not \
             passed",
        ),
    ] {
        let error = Hipc::against(&home, &server)
            .run(&args)
            .fails(2, "confirmation_required");
        assert_eq!(error["message"], action);
        assert_eq!(error["hint"], "pass `--yes` to confirm");
    }
    assert!(server.requests().is_empty());
}

/// A dry run needs no `--yes` and no token, and lists the exact request.
#[test]
fn dry_runs_list_the_exact_request_and_send_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let url = |path: &str| format!("{}{path}", server.url());
    let cases: [(&[&str], Value); 4] = [
        (
            &["network", "peers", "update", "12", "--name", "build"],
            json!({
                "method": "PATCH",
                "url": url("/api/network/peers/12/"),
                "account_header": null,
                "project_header": null,
                "billing_header": null,
                "body": {"name": "build"},
            }),
        ),
        (
            &["network", "peers", "delete", "12"],
            json!({
                "method": "DELETE",
                "url": url("/api/network/peers/12/"),
                "account_header": null,
                "project_header": null,
                "billing_header": null,
                "body": null,
            }),
        ),
        (
            &["network", "get"],
            json!({
                "method": "GET",
                "url": url("/api/network/me/"),
                "account_header": null,
                "project_header": null,
                "billing_header": null,
                "body": null,
            }),
        ),
        (
            &["network", "vpcs", "resources", "list", "3"],
            json!({
                "method": "GET",
                "url": url("/api/network/networks/3/resources/"),
                "account_header": null,
                "project_header": null,
                "billing_header": null,
                "body": null,
            }),
        ),
    ];
    for (args, request) in cases {
        let doc = Hipc::tokenless(&home, &server)
            .env_remove("HIPPIUS_TOKEN")
            .run(&[&["--dry-run"], args].concat())
            .dry_ok();
        assert_eq!(doc, one_request_dry_run(request), "{args:?}");
    }
    assert!(server.requests().is_empty());
}

/// With a membership selected every route sends `X-Hippius-Account`, reads and writes.
#[test]
fn a_membership_sends_the_account_header_everywhere() {
    let server = Server::start(vec![
        route_of(&network::ME, [], Reply::json(200, SUMMARY)),
        route_of(&network::PROVISION, [], Reply::json(200, SUMMARY)),
        route_of(&network::PEERS, [], Reply::json(200, "[]")),
        route_of(&network::PEER, [&12], Reply::json(200, PEER)),
        route_of(
            &network::PEER_UPDATE,
            [&12],
            Reply::json(200, r#"{"id":"12","name":"b"}"#),
        ),
        route_of(&network::PEER_DELETE, [&12], Reply::json(204, "")),
        route_of(&network::GROUPS, [], Reply::json(200, "[]")),
        route_of(&network::POLICIES, [], Reply::json(200, "[]")),
        route_of(&network::VPCS, [], Reply::json(200, "[]")),
        route_of(&network::VPC_RESOURCES, [&3], Reply::json(200, "[]")),
        route_of(&network::VPC_ROUTERS, [&3], Reply::json(200, "[]")),
    ]);
    let home = Home::new();
    for args in COMMANDS {
        Hipc::against(&home, &server)
            .run(&[&["--account", MEMBER], args].concat())
            .ok();
    }
    let requests = server.requests();
    assert_eq!(requests.len(), COMMANDS.len());
    for request in requests {
        assert_eq!(
            request.header("x-hippius-account"),
            Some(MEMBER),
            "{} {}",
            request.method,
            request.target
        );
    }
}

/// The network is account-wide: a project or a billing account from a flag is refused on
/// every route, read or write, and nothing is sent.
#[test]
fn a_project_or_a_billing_account_is_refused() {
    let server = Server::start(vec![]);
    let home = Home::new();
    for (flag, id, code) in [
        ("--project", PROJECT, "project_scope_unsupported"),
        ("--billing", BILLING, "billing_scope_unsupported"),
    ] {
        for args in COMMANDS {
            Hipc::against(&home, &server)
                .run(&[&[flag, id], args].concat())
                .fails(2, code);
        }
    }
    assert!(server.requests().is_empty());
}

/// A remove the server rejected changed nothing, so it is `none`; a 5xx after the write
/// was sent is `partial`.
#[test]
fn a_failed_delete_says_what_may_have_happened() {
    let server = Server::start(vec![
        route_of(
            &network::PEER_DELETE,
            [&12],
            Reply::json(403, r#"{"detail":"suspended"}"#),
        ),
        route_of(
            &network::PEER_DELETE,
            [&13],
            Reply::json(502, r#"{"detail":"bad gateway"}"#),
        ),
    ]);
    let home = Home::new();
    let error = Hipc::against(&home, &server)
        .run(&["network", "peers", "delete", "12", "--yes"])
        .fails(1, "permission");
    assert_eq!(error["side_effects"], "none");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with("cannot remove the peer `12` from the private network"),
        "{error}"
    );
    let error = Hipc::against(&home, &server)
        .run(&["network", "peers", "delete", "13", "--yes"])
        .fails(1, "unavailable");
    assert_eq!(error["side_effects"], "partial");
}
