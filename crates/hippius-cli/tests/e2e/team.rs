//! `hipc account members` and `hipc account invites`: the team of the selected account.

use serde_json::{Value, json};

use crate::common::{Hipc, Home, MEMBER, Recorded, Reply, Route, Run, Server, draft, route};

const MEMBERS: &str = concat!(
    r#"{"owner":{"id":"u1","email":"owner@example.com","username":"owner"},"#,
    r#""members":[{"id":7,"email":"ana@example.com","#,
    r#""grants":[{"scope":"vms","level":"viewer"}],"#,
    r#""can_manage_team":false,"can_billing":false}]}"#
);

const ONE_MEMBER: &str = concat!(
    r#"{"id":7,"email":"ana@example.com","#,
    r#""grants":[{"scope":"vms","level":"operator"}],"#,
    r#""can_manage_team":true,"can_billing":false}"#
);

const INVITES: &str = concat!(
    r#"{"invites":[{"id":3,"email":"bo@example.com","grants":[],"#,
    r#""can_manage_team":false,"can_billing":false,"expires_at":"2026-11-03T00:00:00Z"}]}"#
);

const ONE_INVITE: &str = concat!(
    r#"{"id":3,"email":"bo@example.com","grants":[{"scope":"s3","level":"admin"}],"#,
    r#""can_manage_team":false,"can_billing":true,"email_sent":true}"#
);

/// Every team route, answering as the console documents it.
fn team_routes() -> Vec<Route> {
    vec![
        route("GET", "/api/accounts/members/", Reply::json(200, MEMBERS)),
        route(
            "PATCH",
            "/api/accounts/members/7/",
            Reply::json(200, ONE_MEMBER),
        ),
        route("DELETE", "/api/accounts/members/7/", Reply::json(204, "")),
        route("GET", "/api/accounts/invites/", Reply::json(200, INVITES)),
        route(
            "POST",
            "/api/accounts/invites/",
            Reply::json(201, ONE_INVITE),
        ),
        route("DELETE", "/api/accounts/invites/3/", Reply::json(204, "")),
    ]
}

fn hipc(server: &Server, args: &[&str]) -> Run {
    let mut argv = vec!["account"];
    argv.extend(args);
    Hipc::against(&Home::new(), server).run(&argv)
}

/// Runs `hipc account` in text mode, as a person at a terminal would see it.
fn text(server: &Server, args: &[&str]) -> Run {
    let mut argv = vec!["--output", "text", "account"];
    argv.extend(args);
    Hipc::against(&Home::new(), server).run(&argv)
}

/// One invocation of every command, each confirmed where it needs it.
const EVERY_COMMAND: [&[&str]; 6] = [
    &["members", "list"],
    &[
        "members",
        "update",
        "7",
        "--grant",
        "vms=operator",
        "--remove-others",
    ],
    &["members", "delete", "7"],
    &["invites", "list"],
    &["invites", "create", "--email", "bo@example.com"],
    &["invites", "revoke", "3"],
];

#[test]
fn members_list_keeps_the_owner_beside_the_items() {
    let server = Server::start(team_routes());
    let doc = hipc(&server, &["members", "list"]).ok();
    assert_eq!(doc["items"][0]["id"], 7);
    assert_eq!(doc["owner"]["username"], "owner");
    assert_eq!(doc["next_page_token"], Value::Null);
    assert!(doc.get("members").is_none(), "{doc}");
    let shown = text(&server, &["members", "list"]);
    assert_eq!(shown.code(), Some(0), "{}", shown.stderr());
    assert!(
        shown.stdout().contains("ana@example.com"),
        "{}",
        shown.stdout()
    );
    assert!(server.requests().iter().all(|r| r.method == "GET"));
}

#[test]
fn invites_list_is_an_envelope() {
    let server = Server::start(team_routes());
    let doc = hipc(&server, &["invites", "list"]).ok();
    assert_eq!(doc["items"][0]["id"], 3);
    assert_eq!(doc["next_page_token"], Value::Null);
    let shown = text(&server, &["invites", "list"]);
    assert_eq!(shown.code(), Some(0), "{}", shown.stderr());
    assert!(
        shown.stdout().contains("bo@example.com"),
        "{}",
        shown.stdout()
    );
}

#[test]
fn a_list_answer_without_its_key_is_internal() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/accounts/members/",
            Reply::json(200, r#"{"owner":{}}"#),
        ),
        route(
            "GET",
            "/api/accounts/invites/",
            Reply::json(200, r#"{"items":[]}"#),
        ),
    ]);
    hipc(&server, &["members", "list"]).fails(1, "internal");
    hipc(&server, &["invites", "list"]).fails(1, "internal");
}

#[test]
fn update_with_remove_others_sends_exactly_the_grants_given_and_reads_nothing_first() {
    let server = Server::start(team_routes());
    let doc = hipc(
        &server,
        &[
            "members",
            "update",
            "7",
            "--grant",
            "vms=operator",
            "--grant",
            "s3=viewer",
            "--remove-others",
            "--yes",
        ],
    )
    .ok();
    assert_eq!(doc, serde_json::from_str::<Value>(ONE_MEMBER).unwrap());
    let request = server.only_request();
    assert_eq!(request.method, "PATCH");
    assert_eq!(request.target, "/api/accounts/members/7/");
    assert_eq!(
        std::str::from_utf8(&request.body).unwrap(),
        concat!(
            r#"{"grants":[{"scope":"vms","level":"operator"},"#,
            r#"{"scope":"s3","level":"viewer"}]}"#
        )
    );
}

#[test]
fn update_in_text_mode_prints_the_member() {
    let server = Server::start(team_routes());
    let shown = text(
        &server,
        &[
            "members",
            "update",
            "7",
            "--billing-permission",
            "true",
            "--yes",
        ],
    );
    assert_eq!(shown.code(), Some(0), "{}", shown.stderr());
    assert!(
        shown.stdout().contains("ana@example.com"),
        "{}",
        shown.stdout()
    );
}

#[test]
fn every_scope_group_and_level_is_sent_by_its_name() {
    let server = Server::start(team_routes());
    let grants = [
        "vms=viewer",
        "databases=operator",
        "kubernetes=admin",
        "runners=viewer",
        "s3=operator",
        "cdn=admin",
        "network=viewer",
        "compute:*=operator",
        "storage:*=admin",
    ];
    let mut argv = vec!["members", "update", "7", "--yes", "--remove-others"];
    for grant in grants {
        argv.extend(["--grant", grant]);
    }
    hipc(&server, &argv).ok();
    let sent: Vec<String> = server.only_request().json()["grants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| {
            format!(
                "{}={}",
                g["scope"].as_str().unwrap(),
                g["level"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(sent, grants);
}

#[test]
fn a_permission_alone_sends_no_grants_key() {
    let server = Server::start(team_routes());
    hipc(
        &server,
        &["members", "update", "7", "--manage-team", "true", "--yes"],
    )
    .ok();
    hipc(
        &server,
        &[
            "members",
            "update",
            "7",
            "--billing-permission",
            "false",
            "--yes",
        ],
    )
    .ok();
    let both = [
        "members",
        "update",
        "7",
        "--manage-team",
        "false",
        "--billing-permission",
        "true",
        "--yes",
    ];
    hipc(&server, &both).ok();
    let bodies: Vec<Value> = server.requests().iter().map(Recorded::json).collect();
    assert_eq!(
        bodies,
        [
            json!({"can_manage_team": true}),
            json!({"can_billing": false}),
            json!({"can_manage_team": false, "can_billing": true}),
        ]
    );
}

#[test]
fn a_permission_takes_only_true_or_false() {
    let server = Server::start(team_routes());
    for value in ["yes", "1", "", "TRUE"] {
        hipc(
            &server,
            &[
                "members",
                "update",
                "7",
                "--billing-permission",
                value,
                "--yes",
            ],
        )
        .fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn grant_none_sends_an_empty_list() {
    let server = Server::start(team_routes());
    hipc(
        &server,
        &[
            "members",
            "update",
            "7",
            "--grant",
            "none",
            "--yes",
            "--remove-others",
        ],
    )
    .ok();
    assert_eq!(server.only_request().json(), json!({"grants": []}));
}

#[test]
fn grant_none_beside_another_grant_is_refused_without_a_request() {
    let server = Server::start(team_routes());
    for grants in [
        &["--grant", "none", "--grant", "vms=admin"][..],
        &["--grant", "vms=admin", "--grant", "none"],
        &["--grant", "none", "--grant", "none"],
    ] {
        for command in [
            &["members", "update", "7"][..],
            &["invites", "create", "--email", "a@b.co"],
        ] {
            let mut argv = command.to_vec();
            argv.extend(grants);
            argv.push("--yes");
            let error = hipc(&server, &argv).fails(2, "usage");
            assert_eq!(
                error["message"],
                "`--grant none` cannot be combined with another `--grant`"
            );
            assert_eq!(
                error["hint"],
                "pass `--grant none` alone to remove every grant"
            );
        }
    }
    assert!(server.requests().is_empty());
}

#[test]
fn a_scope_given_twice_is_refused_without_a_request() {
    let server = Server::start(team_routes());
    let argv = [
        "members",
        "update",
        "7",
        "--grant",
        "vms=admin",
        "--grant",
        "vms=viewer",
        "--yes",
    ];
    let error = hipc(&server, &argv).fails(2, "usage");
    assert_eq!(
        error["message"],
        "`--grant` names the scope `vms` more than once"
    );
    assert_eq!(error["hint"], "pass each scope once");
    assert!(server.requests().is_empty());
}

#[test]
fn an_update_with_nothing_to_change_is_usage_without_a_request() {
    let server = Server::start(team_routes());
    let error = hipc(&server, &["members", "update", "7", "--yes"]).fails(2, "usage");
    assert_eq!(
        error["message"],
        "`hipc account members update` was given nothing to change"
    );
    assert_eq!(
        error["hint"],
        "pass `--grant`, `--manage-team`, or `--billing-permission`"
    );
    assert!(server.requests().is_empty());
}

#[test]
fn a_bad_grant_is_usage_without_a_request_and_lists_the_valid_values() {
    let server = Server::start(team_routes());
    let scopes = concat!(
        "use `vms`, `databases`, `kubernetes`, `runners`, `s3`, `cdn`, `network`, ",
        "`compute:*`, `storage:*`, or `billing`"
    );
    let shape = "a grant is `SCOPE=LEVEL`, such as `vms=operator`, `SCOPE=LEVEL@ID`, or `none`";
    let project_id = concat!(
        "`proj_x` is not a project id: a grant on `vms` names a project, `proj_` and 20 ",
        "lowercase hex characters, from `hipc account projects list`"
    );
    let cases = [
        ("vms", shape),
        ("", shape),
        ("NONE", shape),
        ("drive=admin", "`drive` is not a grant scope"),
        ("project=admin", "`project` is not a grant scope"),
        ("Billing=viewer", "`Billing` is not a grant scope"),
        (
            "billing=operator",
            "`operator` is not a billing level: use `viewer` or `admin`",
        ),
        ("vms=admin@proj_x", project_id),
        ("vms=admin@", "`` is not a project id"),
        ("vms=admin@bill_9f8e7d6c5b4a39281706", "is not a project id"),
        ("vms=admin@default", "`default` is not a project id"),
        (
            "billing=admin@proj_0a1b2c3d4e5f60718293",
            "is not a billing account id: a grant on `billing` names a billing account",
        ),
        (
            "network=admin@proj_0a1b2c3d4e5f60718293",
            "a grant on `network` is account-wide, so it names no project or billing account",
        ),
        ("VMS=admin", "`VMS` is not a grant scope"),
        ("compute=admin", "`compute` is not a grant scope"),
        (
            "vms=owner",
            "`owner` is not a grant level: use `viewer`, `operator`, or `admin`",
        ),
        ("vms=Admin", "`Admin` is not a grant level"),
        ("vms=", "`` is not a grant level"),
    ];
    for (grant, expected) in cases {
        for command in [
            &["members", "update", "7"][..],
            &["invites", "create", "--email", "a@b.co"],
        ] {
            let mut argv = command.to_vec();
            argv.extend(["--grant", grant, "--yes"]);
            let error = hipc(&server, &argv).fails(2, "usage");
            let message = error["message"].as_str().unwrap();
            assert!(message.contains(expected), "{grant}: {message}");
            if expected.ends_with("scope") {
                assert!(message.contains(scopes), "{grant}: {message}");
            }
        }
    }
    assert!(server.requests().is_empty());
}

#[test]
fn a_bad_id_is_usage_without_a_request() {
    let server = Server::start(team_routes());
    for id in ["x", "-1", "7/", "1.5", "", "7?x=1", "../8"] {
        for command in [
            &["members", "update", id, "--billing-permission", "true"][..],
            &["members", "delete", id],
            &["invites", "revoke", id],
        ] {
            let mut argv = command.to_vec();
            argv.push("--yes");
            hipc(&server, &argv).fails(2, "usage");
        }
    }
    assert!(server.requests().is_empty());
}

#[test]
fn a_bad_email_is_usage_without_a_request() {
    let server = Server::start(team_routes());
    let long = format!("{}@example.com", "a".repeat(243));
    for email in [
        "",
        "ana",
        "@example.com",
        "ana@",
        "ana@example",
        "ana@.com",
        "ana@example.",
        "ana@b@example.com",
        "ana bo@example.com",
        "ana@exa\tmple.com",
        "ana@example.com\n",
        &long,
    ] {
        let argv = ["invites", "create", "--email", email, "--yes"];
        let error = hipc(&server, &argv).fails(2, "usage");
        let message = error["message"].as_str().unwrap();
        assert!(
            message.contains("an email address is `name@domain`"),
            "{message}"
        );
    }
    assert!(server.requests().is_empty());
    let longest = format!("{}@example.com", "a".repeat(242));
    hipc(
        &server,
        &["invites", "create", "--email", &longest, "--yes"],
    )
    .ok();
    assert_eq!(server.only_request().json()["email"], longest);
}

#[test]
fn every_write_needs_yes_and_sends_nothing_without_it() {
    let server = Server::start(team_routes());
    let cases: [(&[&str], &str); 6] = [
        (
            &[
                "members",
                "update",
                "7",
                "--grant",
                "vms=operator",
                "--grant",
                "s3=viewer",
                "--remove-others",
            ],
            "update the member `7`: replace every grant with `vms=operator` and `s3=viewer`",
        ),
        (
            &[
                "members",
                "update",
                "7",
                "--grant",
                "none",
                "--manage-team",
                "true",
                "--remove-others",
            ],
            "update the member `7`: remove every grant and set `can_manage_team` to `true`",
        ),
        (
            &["members", "delete", "7"],
            "remove the member `7` from the team",
        ),
        (
            &["invites", "create", "--email", "bo@example.com"],
            "email an invite to `bo@example.com` that grants nothing",
        ),
        (
            &[
                "invites",
                "create",
                "--email",
                "bo@example.com",
                "--grant",
                "s3=admin",
                "--billing-permission",
                "true",
                "--manage-team",
                "true",
            ],
            concat!(
                "email an invite to `bo@example.com` that grants `s3=admin`, ",
                "`can_manage_team`, and `can_billing`"
            ),
        ),
        (&["invites", "revoke", "3"], "revoke the invite `3`"),
    ];
    for (args, what) in cases {
        let error = hipc(&server, args).fails(2, "confirmation_required");
        assert_eq!(
            error["message"],
            format!("this would {what}, and `--yes` was not passed"),
            "{args:?}"
        );
    }
    assert!(server.requests().is_empty());
}

#[test]
fn a_membership_names_the_account_in_the_confirmation() {
    let server = Server::start(team_routes());
    let error = Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["account", "members", "delete", "7"])
        .fails(2, "confirmation_required");
    let message = error["message"].as_str().unwrap();
    assert!(message.contains(MEMBER), "{message}");
    assert!(server.requests().is_empty());
}

#[test]
fn dry_run_shows_each_request_and_sends_nothing() {
    let server = Server::start(vec![]);
    let dry = |args: &[&str]| {
        let mut argv = vec!["account"];
        argv.extend(args);
        argv.push("--dry-run");
        Hipc::against(&Home::new(), &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&argv)
            .dry_ok()
    };
    let described = |method: &str, path: &str, body: Value| {
        json!({
            "method": method,
            "url": format!("{}/api/accounts/{path}", server.url()),
            "account_header": MEMBER,
            "project_header": null,
            "billing_header": null,
            "body": body,
        })
    };
    // A command that sends one request lists it alone in `requests`.
    let request = |method: &str, path: &str, body: Value| {
        crate::common::one_request_dry_run(described(method, path, body))
    };
    assert_eq!(
        dry(&[
            "members",
            "update",
            "7",
            "--grant",
            "cdn=admin",
            "--billing-permission",
            "true"
        ]),
        {
            // A grant update reads the member's grants first, to refuse one it would remove.
            let patch = described(
                "PATCH",
                "members/7/",
                json!({"grants": [{"scope": "cdn", "level": "admin"}], "can_billing": true}),
            );
            let read = described("GET", "members/", Value::Null);
            json!({ "dry_run": true, "reads_first": read, "requests": [read, patch] })
        }
    );
    assert_eq!(
        dry(&["invites", "create", "--email", "bo@example.com"]),
        request(
            "POST",
            "invites/",
            json!({
                "email": "bo@example.com",
                "grants": [],
                "can_manage_team": false,
                "can_billing": false,
            })
        )
    );
    assert_eq!(
        dry(&[
            "invites",
            "create",
            "--email",
            "bo@example.com",
            "--grant",
            "storage:*=viewer",
            "--manage-team",
            "true",
            "--billing-permission",
            "false",
        ]),
        request(
            "POST",
            "invites/",
            json!({
                "email": "bo@example.com",
                "grants": [{"scope": "storage:*", "level": "viewer"}],
                "can_manage_team": true,
                "can_billing": false,
            })
        )
    );
    assert_eq!(
        dry(&["members", "delete", "7"]),
        request("DELETE", "members/7/", Value::Null)
    );
    assert_eq!(
        dry(&["invites", "revoke", "3"]),
        request("DELETE", "invites/3/", Value::Null)
    );
    assert!(server.requests().is_empty());
}

#[test]
fn these_team_commands_dry_run_without_a_token_or_a_request() {
    let server = Server::start(vec![]);
    for args in EVERY_COMMAND {
        let mut argv = vec!["account"];
        argv.extend(args);
        argv.push("--dry-run");
        let doc = Hipc::new(&Home::new())
            .env("HIPPIUS_API_URL", server.url())
            .run(&argv)
            .ok();
        assert_eq!(doc["dry_run"], true, "{args:?}");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn a_membership_sends_the_account_header_on_every_team_route() {
    let server = Server::start(team_routes());
    for args in EVERY_COMMAND {
        let mut argv = vec!["account"];
        argv.extend(args);
        argv.push("--yes");
        Hipc::against(&Home::new(), &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&argv)
            .ok();
    }
    let requests = server.requests();
    assert_eq!(requests.len(), EVERY_COMMAND.len());
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

#[test]
fn the_own_account_sends_no_account_header() {
    let server = Server::start(team_routes());
    for args in EVERY_COMMAND {
        let mut argv = vec!["account"];
        argv.extend(args);
        argv.extend(["--yes", "--account", "self"]);
        Hipc::against(&Home::new(), &server).run(&argv).ok();
    }
    let requests = server.requests();
    assert_eq!(requests.len(), EVERY_COMMAND.len());
    assert!(
        requests
            .iter()
            .all(|r| r.header("x-hippius-account").is_none())
    );
}

#[test]
fn a_204_prints_the_document_the_plan_names() {
    let server = Server::start(team_routes());
    assert_eq!(
        hipc(&server, &["members", "delete", "7", "--yes"]).ok(),
        json!({"id": 7, "deleted": true})
    );
    assert_eq!(
        hipc(&server, &["invites", "revoke", "3", "--yes"]).ok(),
        json!({"id": 3, "revoked": true})
    );
    let shown = text(&server, &["invites", "revoke", "3", "--yes"]);
    assert_eq!(shown.code(), Some(0), "{}", shown.stderr());
    assert!(shown.stdout().contains("revoked"), "{}", shown.stdout());
    let methods: Vec<String> = server.requests().iter().map(|r| r.method.clone()).collect();
    assert_eq!(methods, ["DELETE", "DELETE", "DELETE"]);
}

#[test]
fn a_delete_that_answers_a_body_prints_it() {
    let server = Server::start(vec![route(
        "DELETE",
        "/api/accounts/members/7/",
        Reply::json(200, r#"{"detail":"removed"}"#),
    )]);
    assert_eq!(
        hipc(&server, &["members", "delete", "7", "--yes"]).ok(),
        json!({"detail": "removed"})
    );
}

#[test]
fn invites_create_prints_the_invite() {
    let server = Server::start(team_routes());
    let argv = [
        "invites",
        "create",
        "--email",
        "bo@example.com",
        "--grant",
        "s3=admin",
        "--billing-permission",
        "true",
        "--yes",
    ];
    let doc = hipc(&server, &argv).ok();
    assert_eq!(doc["email_sent"], true);
    assert_eq!(
        server.only_request().json(),
        json!({
            "email": "bo@example.com",
            "grants": [{"scope": "s3", "level": "admin"}],
            "can_manage_team": false,
            "can_billing": true,
        })
    );
    let shown = text(&server, &argv);
    assert_eq!(shown.code(), Some(0), "{}", shown.stderr());
    assert!(shown.stdout().contains("email_sent"), "{}", shown.stdout());
}

#[test]
fn a_team_refusal_passes_through_by_its_code() {
    let refusal = |code: &str| {
        Reply::json(
            403,
            &format!(r#"{{"code":"{code}","error":"not allowed"}}"#),
        )
    };
    let server = Server::start(vec![
        route(
            "PATCH",
            "/api/accounts/members/7/",
            refusal("team_management_required"),
        ),
        route("DELETE", "/api/accounts/members/7/", refusal("owner_only")),
    ]);
    let error = hipc(
        &server,
        &[
            "members",
            "update",
            "7",
            "--grant",
            "none",
            "--yes",
            "--remove-others",
        ],
    )
    .fails(1, "team_management_required");
    assert_eq!(error["http_status"], 403);
    assert_eq!(error["side_effects"], "none");
    hipc(&server, &["members", "delete", "7", "--yes"]).fails(1, "owner_only");
}

#[test]
fn an_invite_refusal_keeps_the_server_body_in_details() {
    for (status, code) in [(400, "validation"), (409, "conflict")] {
        let body = r#"{"error":"the team is full","detail":{"limit":50}}"#;
        let server = Server::start(vec![route(
            "POST",
            "/api/accounts/invites/",
            Reply::json(status, body),
        )]);
        let error = hipc(
            &server,
            &["invites", "create", "--email", "bo@example.com", "--yes"],
        )
        .fails(1, code);
        assert_eq!(
            error["details"],
            serde_json::from_str::<Value>(body).unwrap()
        );
        assert_eq!(error["side_effects"], "none");
    }
}

#[test]
fn team_commands_need_a_token() {
    let server = Server::start(vec![]);
    for args in EVERY_COMMAND {
        let mut argv = vec!["account"];
        argv.extend(args);
        argv.push("--yes");
        Hipc::new(&Home::new())
            .env("HIPPIUS_API_URL", server.url())
            .run(&argv)
            .fails(1, "auth");
    }
    assert!(server.requests().is_empty());
}
/// The method and path of every request the server received.
fn targets(server: &Server) -> Vec<String> {
    server
        .requests()
        .iter()
        .map(|r| format!("{} {}", r.method, r.target))
        .collect()
}

/// The whole-list guard: the member has `vms=viewer`, so `--grant`s that leave it out, or give
/// `vms` another level, are refused without `--remove-others` after the read, and the
/// PATCH is never sent.
#[test]
fn update_that_drops_a_grant_is_refused_without_remove_others() {
    for grants in [
        &["--grant", "s3=admin"][..],
        &["--grant", "vms=admin"],
        &["--grant", "none"],
    ] {
        let server = Server::start(team_routes());
        let mut argv = vec!["members", "update", "7"];
        argv.extend(grants);
        argv.push("--yes");
        let error = hipc(&server, &argv).fails(2, "remove_others_required");
        assert_eq!(
            error["message"],
            "this would remove 1 grant that the member `7` has now, and `--remove-others` was \
             not passed"
        );
        assert_eq!(
            error["hint"],
            format!(
                "run `hipc account {} --remove-others` to remove `vms=viewer`",
                argv.join(" ")
            )
        );
        assert_eq!(targets(&server), ["GET /api/accounts/members/"]);
        argv.push("--remove-others");
        hipc(&server, &argv).ok();
        assert_eq!(server.requests().len(), 2, "{grants:?}");
        assert_eq!(server.requests()[1].method, "PATCH");
    }
}

/// Grants that keep `vms=viewer` read the member, then send the PATCH; both carry the
/// membership header.
#[test]
fn update_that_keeps_every_grant_reads_then_patches() {
    let server = Server::start(team_routes());
    let argv = [
        "account",
        "members",
        "update",
        "7",
        "--grant",
        "s3=admin",
        "--grant",
        "vms=viewer",
        "--yes",
    ];
    Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&argv)
        .ok();
    assert_eq!(
        targets(&server),
        [
            "GET /api/accounts/members/",
            "PATCH /api/accounts/members/7/"
        ]
    );
    for request in server.requests() {
        assert_eq!(request.header("x-hippius-account"), Some(MEMBER));
    }
}

/// A member the list does not hold is `not_found` with no PATCH: the guard fails closed, so
/// a list that keys its members another way than the route cannot turn it off.
#[test]
fn update_of_a_member_not_in_the_list_is_not_found_without_a_patch() {
    let mut routes = team_routes();
    routes.push(route(
        "PATCH",
        "/api/accounts/members/8/",
        Reply::json(200, ONE_MEMBER),
    ));
    let server = Server::start(routes);
    let error = hipc(
        &server,
        &["members", "update", "8", "--grant", "vms=viewer", "--yes"],
    )
    .fails(1, "not_found");
    assert_eq!(error["message"], "no member with id `8`");
    assert_eq!(
        error["hint"],
        "run `hipc account members list` to see what exists"
    );
    assert_eq!(targets(&server), ["GET /api/accounts/members/"]);
}

/// A members answer without the shape the guard reads ends the update before the PATCH.
#[test]
fn update_whose_read_has_another_shape_is_internal_without_a_patch() {
    for answer in [
        r#"{"owner":{}}"#,
        r#"{"members":{}}"#,
        r#"{"members":[{"id":7}]}"#,
        r#"{"members":[{"id":7,"grants":{}}]}"#,
        r#"{"members":[{"id":7,"grants":[{"scope":"vms"}]}]}"#,
        r#"{"members":[{"id":7,"grants":[{"level":"viewer"}]}]}"#,
        r#"{"members":[{"id":7,"grants":[{"scope":"vms","level":1}]}]}"#,
        // A grant on a project the guard cannot read would pass as account-wide, and a new
        // list could then widen it without `--remove-others`.
        r#"{"members":[{"id":7,"grants":[{"scope":"vms","level":"viewer","project":12}]}]}"#,
        r#"{"members":[{"id":7,"grants":[{"scope":"vms","level":"viewer","billing":true}]}]}"#,
        r#"{"members":[{"id":7,"grants":[{"scope":"vms","level":"viewer","project_id":"p"}]}]}"#,
        r#"{"members":[{"id":7,"grants":["vms=viewer"]}]}"#,
        r#"{"members":[{"id":7,"grants":[{"scope":"vms","level":"viewer","project":"p","billing":"b"}]}]}"#,
    ] {
        let server = Server::start(vec![
            route("GET", "/api/accounts/members/", Reply::json(200, answer)),
            route(
                "PATCH",
                "/api/accounts/members/7/",
                Reply::json(200, ONE_MEMBER),
            ),
        ]);
        let error = hipc(
            &server,
            &["members", "update", "7", "--grant", "vms=viewer", "--yes"],
        )
        .fails(1, "internal");
        assert_eq!(
            error["message"],
            "cannot list the team members: the API answered with a shape `hipc` does not read",
            "{answer}"
        );
        assert_eq!(targets(&server), ["GET /api/accounts/members/"], "{answer}");
    }
}

#[test]
fn remove_others_needs_a_grant_and_its_dry_run_reads_nothing() {
    let server = Server::start(team_routes());
    hipc(
        &server,
        &[
            "members",
            "update",
            "7",
            "--billing-permission",
            "true",
            "--remove-others",
        ],
    )
    .fails(2, "usage");
    let doc = hipc(
        &server,
        &[
            "members",
            "update",
            "7",
            "--grant",
            "none",
            "--remove-others",
            "--dry-run",
        ],
    )
    .ok();
    assert_eq!(draft(&doc)["method"], "PATCH");
    assert!(doc.get("reads_first").is_none(), "{doc}");
    // One request goes out, so `requests` is that request alone.
    assert_eq!(doc["requests"].as_array().map(Vec::len), Some(1), "{doc}");
    assert_eq!(doc["requests"][0]["method"], "PATCH", "{doc}");
    // A permission alone sends no grants, so its dry run names no read either.
    let doc = hipc(
        &server,
        &[
            "members",
            "update",
            "7",
            "--billing-permission",
            "true",
            "--dry-run",
        ],
    )
    .ok();
    assert!(doc.get("reads_first").is_none(), "{doc}");
    assert_eq!(doc["requests"].as_array().map(Vec::len), Some(1), "{doc}");
    assert!(server.requests().is_empty());
}
const PROJECT: &str = "proj_0a1b2c3d4e5f60718293";
const BILLING: &str = "bill_9f8e7d6c5b4a39281706";

/// A grant names one project or billing account with `@ID`, and goes out with the key the
/// console sends; one scope may be given once per project.
#[test]
fn a_grant_on_one_project_or_billing_account_sends_its_id() {
    let server = Server::start(team_routes());
    let other = "proj_ffffffffffffffffffff";
    let grants = [
        format!("vms=operator@{PROJECT}"),
        format!("vms=viewer@{other}"),
        "vms=viewer".to_owned(),
        format!("billing=admin@{BILLING}"),
        "billing=viewer".to_owned(),
    ];
    let mut argv = vec!["invites", "create", "--email", "a@b.co", "--yes"];
    for grant in &grants {
        argv.extend(["--grant", grant]);
    }
    hipc(&server, &argv).ok();
    assert_eq!(
        server.only_request().json()["grants"],
        json!([
            {"scope": "vms", "level": "operator", "project": PROJECT},
            {"scope": "vms", "level": "viewer", "project": other},
            {"scope": "vms", "level": "viewer"},
            {"scope": "billing", "level": "admin", "billing": BILLING},
            {"scope": "billing", "level": "viewer"},
        ])
    );
    let twice = format!("vms=admin@{PROJECT}");
    let error = hipc(
        &server,
        &[
            "members", "update", "7", "--grant", &twice, "--grant", &twice, "--yes",
        ],
    )
    .fails(2, "usage");
    assert_eq!(
        error["message"],
        format!("`--grant` names the scope `vms` on `{PROJECT}` more than once")
    );
    assert_eq!(server.requests().len(), 1);
}

/// The guard compares a grant with the project it names: replacing a grant limited to a
/// project with the same level account-wide removes the limited one, so it is refused
/// without `--remove-others`, and the confirmation names each grant with its id. A null
/// `billing` beside the project is read as no billing account.
#[test]
fn the_guard_tells_a_grant_on_a_project_from_one_on_the_account() {
    let members = format!(
        concat!(
            r#"{{"owner":{{"id":"u1"}},"members":[{{"id":7,"grants":["#,
            r#"{{"scope":"vms","level":"operator","project":"{}","billing":null}}]}}]}}"#
        ),
        PROJECT
    );
    let server = Server::start(vec![
        route("GET", "/api/accounts/members/", Reply::json(200, &members)),
        route(
            "PATCH",
            "/api/accounts/members/7/",
            Reply::json(200, ONE_MEMBER),
        ),
    ]);
    let error = hipc(
        &server,
        &["members", "update", "7", "--grant", "vms=operator", "--yes"],
    )
    .fails(2, "remove_others_required");
    assert!(
        error["hint"]
            .as_str()
            .unwrap()
            .ends_with(&format!("to remove `vms=operator@{PROJECT}`")),
        "{error}"
    );
    assert_eq!(targets(&server), ["GET /api/accounts/members/"]);
    let kept = format!("vms=operator@{PROJECT}");
    let doc = hipc(
        &server,
        &["members", "update", "7", "--grant", &kept, "--dry-run"],
    )
    .ok();
    assert_eq!(
        draft(&doc)["body"]["grants"],
        json!([{"scope": "vms", "level": "operator", "project": PROJECT}])
    );
    hipc(
        &server,
        &["members", "update", "7", "--grant", &kept, "--yes"],
    )
    .ok();
    assert_eq!(server.requests().len(), 3);
}
