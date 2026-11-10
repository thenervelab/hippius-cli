//! `hipc account referrals`: the signed-in user's referral links (list, create, rename) and
//! their analytics. The routes act as the signed-in user, so no selector header is sent,
//! and no command is confirmed, since none spends.

use serde_json::{Value, json};

use crate::common::{BILLING, Hipc, Home, MEMBER, PROJECT, Reply, Server, draft, route};

/// One link, in the shape swagger gives an entry of `GET /api/referral-codes/`.
const LINK: &str = concat!(
    r#"{"id":4,"name":"telegram","slug":"tg1","code":"HIPREF","#,
    r#""link":"https://console.hippius.com/r/HIPREF?src=tg1","#,
    r#""created_at":"2026-11-01T10:00:00Z"}"#
);

/// A second link of the same code.
const OTHER_LINK: &str =
    r#"{"id":5,"name":"discord","slug":"dc1","code":"HIPREF","link":null,"created_at":"x"}"#;

/// Analytics have no shape in swagger; the answer is printed as sent.
const ANALYTICS: &str = r#"{"signups":3,"converted":1,"channels":[{"slug":"tg1","signups":3}]}"#;

#[test]
fn list_prints_the_links_as_the_envelope() {
    let server = Server::start(vec![route(
        "GET",
        "/api/referral-codes/",
        Reply::json(200, &format!("[{LINK},{OTHER_LINK}]")),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["account", "referrals", "list"])
        .ok();
    assert_eq!(
        doc,
        json!({
            "items": [
                serde_json::from_str::<Value>(LINK).unwrap(),
                serde_json::from_str::<Value>(OTHER_LINK).unwrap(),
            ],
            "next_page_token": null,
        })
    );
    // No links yet is an empty list.
    let empty = Server::start(vec![route(
        "GET",
        "/api/referral-codes/",
        Reply::json(200, "[]"),
    )]);
    let doc = Hipc::against(&Home::new(), &empty)
        .run(&["account", "referrals", "list"])
        .ok();
    assert_eq!(doc, json!({"items": [], "next_page_token": null}));
}

/// Every link shares the user's one code, so links of two codes are the server's
/// inconsistency: the list is printed whole, with a warning in text mode and none in JSON
/// mode (tenet 3), so the user is never locked out of their own links.
#[test]
fn a_list_of_more_than_one_code_prints_with_a_warning_in_text_mode() {
    let foreign = LINK.replace("HIPREF", "OTHER");
    let body = format!("[{LINK},{foreign}]");
    let server = Server::start(vec![route(
        "GET",
        "/api/referral-codes/",
        Reply::json(200, &body),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["account", "referrals", "list"])
        .ok();
    assert_eq!(doc["items"].as_array().unwrap().len(), 2);
    let run = Hipc::against(&Home::new(), &server).run(&[
        "--output",
        "text",
        "account",
        "referrals",
        "list",
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(run.stdout().contains("OTHER"), "{}", run.stdout());
    assert_eq!(
        run.stderr(),
        "warning: these links carry more than one referral `code`, though a user has only one: \
         check each link's `code` before sharing it\n"
    );
}

/// A link with an empty or missing `code` (one made before the code was minted) beside the
/// current one is not a second code: no warning.
#[test]
fn an_empty_or_missing_code_beside_the_current_one_is_not_a_second_code() {
    let empty = LINK.replace(r#""code":"HIPREF""#, r#""code":"""#);
    let missing = LINK.replace(r#""code":"HIPREF","#, "");
    let null = LINK.replace(r#""code":"HIPREF""#, r#""code":null"#);
    for other in [empty, missing, null] {
        for body in [format!("[{other},{LINK}]"), format!("[{LINK},{other}]")] {
            let server = Server::start(vec![route(
                "GET",
                "/api/referral-codes/",
                Reply::json(200, &body),
            )]);
            let run = Hipc::against(&Home::new(), &server).run(&[
                "--output",
                "text",
                "account",
                "referrals",
                "list",
            ]);
            assert_eq!(run.code(), Some(0), "{}", run.stderr());
            assert_eq!(run.stderr(), "", "{body}");
        }
    }
}

/// A create is free, so it needs no `--yes`, and sends only the name.
#[test]
fn create_needs_no_yes_and_sends_only_the_name() {
    let server = Server::start(vec![route(
        "POST",
        "/api/referral-codes/",
        Reply::json(201, LINK),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["account", "referrals", "create", "--name", "telegram"])
        .ok();
    assert_eq!(doc, serde_json::from_str::<Value>(LINK).unwrap());
    assert_eq!(server.only_request().json(), json!({"name": "telegram"}));
}

/// A name already in use is the server's 409, passed on as `conflict`; nothing was made.
#[test]
fn a_name_in_use_is_a_conflict() {
    let server = Server::start(vec![route(
        "POST",
        "/api/referral-codes/",
        Reply::json(
            409,
            r#"{"detail":"You already have an entry with this name"}"#,
        ),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["account", "referrals", "create", "--name", "telegram"])
        .fails(1, "conflict");
    assert_eq!(error["side_effects"], "none");
}

/// Swagger's name is required and at most 64 characters, counted as characters: a blank
/// one, a longer one, or one with a control character is usage before the token check.
#[test]
fn a_name_outside_one_line_of_64_characters_is_usage() {
    let home = Home::new();
    let long = "a".repeat(65);
    for name in ["", "   ", long.as_str(), "two\nlines", "bell\u{7}"] {
        for command in [
            vec!["account", "referrals", "create"],
            vec!["account", "referrals", "update", "4"],
        ] {
            let args = [command, vec!["--name", name]].concat();
            let error = Hipc::new(&home).run(&args).fails(2, "usage");
            assert!(
                error["message"]
                    .as_str()
                    .unwrap()
                    .contains("a referral name is one line of 1 to 64 characters"),
                "{error}"
            );
        }
    }
    let server = Server::start(vec![route(
        "POST",
        "/api/referral-codes/",
        Reply::json(201, LINK),
    )]);
    let widest = "é".repeat(64);
    Hipc::against(&home, &server)
        .run(&["account", "referrals", "create", "--name", &widest])
        .ok();
    assert_eq!(server.only_request().json()["name"], widest);
}

#[test]
fn update_renames_the_link_and_sends_only_the_name() {
    let server = Server::start(vec![route(
        "PATCH",
        "/api/referral-codes/4/",
        Reply::json(200, LINK),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["account", "referrals", "update", "4", "--name", "discord"])
        .ok();
    assert_eq!(doc["id"], 4);
    assert_eq!(server.only_request().json(), json!({"name": "discord"}));
}

#[test]
fn update_of_a_missing_link_names_the_list_and_a_bad_id_is_usage() {
    let server = Server::start(vec![route(
        "PATCH",
        "/api/referral-codes/9/",
        Reply::not_found(),
    )]);
    let home = Home::new();
    let error = Hipc::against(&home, &server)
        .run(&["account", "referrals", "update", "9", "--name", "discord"])
        .fails(1, "not_found");
    assert_eq!(
        error["hint"],
        "run `hipc account referrals list` to see what exists"
    );
    assert_eq!(error["side_effects"], "none");
    for id in ["../4", "x", "-1"] {
        let error = Hipc::new(&home)
            .run(&["account", "referrals", "update", "--name", "d", "--", id])
            .fails(2, "usage");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("a referral link id is a whole number"),
            "{error}"
        );
    }
    Hipc::new(&home)
        .run(&["account", "referrals", "update", "4"])
        .fails(2, "usage");
}

/// A dry run of either write needs no token and shows the body; nothing is sent.
#[test]
fn create_and_update_dry_runs_show_the_body_and_send_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let hipc = || Hipc::tokenless(&home, &server);
    let doc = hipc()
        .run(&["--dry-run", "account", "referrals", "create", "--name", "x"])
        .ok();
    assert_eq!(draft(&doc)["method"], "POST");
    assert_eq!(
        draft(&doc)["url"],
        format!("{}/api/referral-codes/", server.url())
    );
    assert_eq!(draft(&doc)["body"], json!({"name": "x"}));
    let doc = hipc()
        .run(&[
            "--dry-run",
            "account",
            "referrals",
            "update",
            "4",
            "--name",
            "y",
        ])
        .ok();
    assert_eq!(draft(&doc)["method"], "PATCH");
    assert_eq!(
        draft(&doc)["url"],
        format!("{}/api/referral-codes/4/", server.url())
    );
    assert_eq!(draft(&doc)["body"], json!({"name": "y"}));
    assert!(server.requests().is_empty());
}

/// The analytics are printed as sent; `--refresh` asks the server to count them again,
/// and without it no query is sent.
#[test]
fn analytics_prints_the_answer_and_refresh_adds_the_query() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/referral/analytics/",
            Reply::json(200, ANALYTICS),
        ),
        route(
            "GET",
            "/api/referral/analytics/?refresh=true",
            Reply::json(200, ANALYTICS),
        ),
    ]);
    let home = Home::new();
    let doc = Hipc::against(&home, &server)
        .run(&["account", "referrals", "analytics"])
        .ok();
    assert_eq!(doc, serde_json::from_str::<Value>(ANALYTICS).unwrap());
    Hipc::against(&home, &server)
        .run(&["account", "referrals", "analytics", "--refresh"])
        .ok();
    let targets: Vec<String> = server.requests().into_iter().map(|r| r.target).collect();
    assert_eq!(
        targets,
        [
            "/api/referral/analytics/",
            "/api/referral/analytics/?refresh=true"
        ]
    );
}

/// The routes act as the signed-in user: with a membership, a project, or a billing
/// account selected, every command is sent, and none carries a selector header, so a member
/// never reaches the owner's code.
#[test]
fn a_selection_sends_every_referral_command_without_a_selector_header() {
    let server = Server::start(vec![
        route("GET", "/api/referral-codes/", Reply::json(200, "[]")),
        route("POST", "/api/referral-codes/", Reply::json(201, LINK)),
        route("PATCH", "/api/referral-codes/4/", Reply::json(200, LINK)),
        route(
            "GET",
            "/api/referral/analytics/",
            Reply::json(200, ANALYTICS),
        ),
    ]);
    let home = Home::new();
    let commands: [&[&str]; 4] = [
        &["account", "referrals", "list"],
        &["account", "referrals", "create", "--name", "x"],
        &["account", "referrals", "update", "4", "--name", "y"],
        &["account", "referrals", "analytics"],
    ];
    let selections = [
        ("--account", MEMBER),
        ("--project", PROJECT),
        ("--billing", BILLING),
    ];
    for (flag, value) in selections {
        for args in commands {
            Hipc::against(&home, &server)
                .run(&[&[flag, value], args].concat())
                .ok();
        }
    }
    let requests = server.requests();
    assert_eq!(requests.len(), commands.len() * selections.len());
    for request in requests {
        for name in [
            "x-hippius-account",
            "x-hippius-project",
            "x-hippius-billing",
        ] {
            assert_eq!(request.header(name), None, "{name} of {}", request.target);
        }
    }
}
