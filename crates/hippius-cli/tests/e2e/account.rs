//! `hipc account list`, `get`, `notifications get`, and `use`.

use serde_json::json;

use crate::common::{ACCOUNTS, Hipc, Home, MEMBER, Reply, Server, draft, route};

fn accounts_server() -> Server {
    Server::start(vec![route(
        "GET",
        "/api/accounts/",
        Reply::json(200, ACCOUNTS),
    )])
}

#[test]
fn list_is_an_envelope_and_never_sends_the_account_header() {
    let server = accounts_server();
    let home = Home::new();
    let doc = Hipc::against(&home, &server)
        .run(&["account", "list", "--account", MEMBER])
        .ok();
    assert_eq!(doc["next_page_token"], serde_json::Value::Null);
    let items = doc["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["is_owner"], true);
    assert_eq!(
        doc["sharing_enabled"], true,
        "the answer's other fields are kept"
    );
    assert_eq!(server.only_request().header("x-hippius-account"), None);
}

#[test]
fn a_server_field_named_like_the_envelope_does_not_replace_it() {
    let server = Server::start(vec![route(
        "GET",
        "/api/accounts/",
        Reply::json(
            200,
            r#"{"accounts":[{"id":"a"}],"items":"x","next_page_token":"9"}"#,
        ),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["account", "list"])
        .ok();
    assert_eq!(doc["items"], serde_json::json!([{"id": "a"}]));
    assert_eq!(doc["next_page_token"], serde_json::Value::Null);
}

#[test]
fn an_unexpected_accounts_shape_is_internal() {
    for body in ["[]", r#"{"accounts":{}}"#, r#"{"other":[]}"#, "<html>"] {
        let server = Server::start(vec![route("GET", "/api/accounts/", Reply::json(200, body))]);
        Hipc::against(&Home::new(), &server)
            .run(&["account", "list"])
            .fails(1, "internal");
    }
}

#[test]
fn get_picks_the_own_account_for_self_and_the_membership_by_id() {
    let server = accounts_server();
    let home = Home::new();
    let own = Hipc::against(&home, &server).run(&["account", "get"]).ok();
    assert_eq!(own["id"], "acct_ffffffffffffffffffff");
    let member = Hipc::against(&home, &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["account", "get"])
        .ok();
    assert_eq!(member["id"], MEMBER);
}

const PROJECT: &str = "proj_0a1b2c3d4e5f60718293";
const OTHER_PROJECT: &str = "proj_99999999999999999999";
const BILLING: &str = "bill_0a1b2c3d4e5f60718293";

/// The four fields `hipc auth status` and `hipc account get` add, from `doc`.
fn selected(doc: &serde_json::Value) -> serde_json::Value {
    json!({
        "project": doc["selected_project"],
        "project_source": doc["selected_project_source"],
        "billing": doc["selected_billing"],
        "billing_source": doc["selected_billing_source"],
    })
}

/// `hipc account get` and `hipc auth status` say which project and billing account the run
/// acts in, and where each was selected: nothing, the profile, a flag over the profile, a
/// variable, and `default` from a flag, which picks the Default project over a stored one.
#[test]
fn get_and_status_name_the_selected_project_and_billing_account_and_their_source() {
    let home = Home::new();
    let get = |env: &[(&str, &str)], flags: &[&str], command: &[&str]| {
        let server = Server::start(vec![
            route("GET", "/api/accounts/", Reply::json(200, ACCOUNTS)),
            route("GET", "/api/users/me/", Reply::json(200, crate::common::ME)),
        ]);
        let mut hipc = Hipc::against(&home, &server);
        for (name, value) in env {
            hipc = hipc.env(name, value);
        }
        selected(&hipc.run(&[flags, command].concat()).ok())
    };
    for command in [&["account", "get"][..], &["auth", "status"]] {
        home.write_config("");
        assert_eq!(
            get(&[], &[], command),
            json!({"project": null, "project_source": null, "billing": null, "billing_source": null}),
            "{command:?}"
        );
        home.write_config(&format!(
            "[profiles.default]\nproject = \"{PROJECT}\"\nbilling = \"{BILLING}\"\n"
        ));
        assert_eq!(
            get(&[], &[], command),
            json!({"project": PROJECT, "project_source": "profile", "billing": BILLING,
                   "billing_source": "profile"}),
            "{command:?}"
        );
        home.write_config(&format!("[profiles.default]\nproject = \"{PROJECT}\"\n"));
        assert_eq!(
            get(
                &[("HIPPIUS_BILLING", BILLING)],
                &["--project", OTHER_PROJECT],
                command
            ),
            json!({"project": OTHER_PROJECT, "project_source": "flag", "billing": BILLING,
                   "billing_source": "environment"}),
            "{command:?}"
        );
        assert_eq!(
            get(&[("HIPPIUS_PROJECT", OTHER_PROJECT)], &[], command),
            json!({"project": OTHER_PROJECT, "project_source": "environment", "billing": null,
                   "billing_source": null}),
            "{command:?}"
        );
        // A stored project belongs to the profile's account: another account does not read it.
        assert_eq!(
            get(&[], &["--account", MEMBER], command),
            json!({"project": null, "project_source": null, "billing": null,
                   "billing_source": null}),
            "{command:?}"
        );
        assert_eq!(
            get(&[], &["--project", "default"], command),
            json!({"project": null, "project_source": "flag", "billing": null,
                   "billing_source": null}),
            "{command:?}"
        );
    }
}

/// A confirmation names the project and the billing account the write carries, as it names
/// a selected membership's account.
#[test]
fn a_confirmation_names_the_selected_project_and_billing_account() {
    let home = Home::new();
    let error = Hipc::new(&home)
        .env("HIPPIUS_TOKEN", "a-token")
        .run(&["--project", PROJECT, "s3", "buckets", "delete", "my-bucket"])
        .fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!(
            "this would delete the bucket `my-bucket` in the project `{PROJECT}`, and `--yes` \
             was not passed"
        )
    );
    let error = Hipc::new(&home)
        .env("HIPPIUS_TOKEN", "a-token")
        .run(&[
            "--account",
            MEMBER,
            "--billing",
            BILLING,
            "billing",
            "top-up",
            "--amount",
            "15",
            "--method",
            "card",
        ])
        .fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!(
            "this would start a card top-up of `15.00` USD in the account `{MEMBER}` and the \
             billing account `{BILLING}`, and `--yes` was not passed"
        )
    );
}

#[test]
fn get_of_an_account_the_token_cannot_reach_is_not_a_member() {
    let server = accounts_server();
    let error = Hipc::against(&Home::new(), &server)
        .run(&["account", "get", "--account", "acct_99999999999999999999"])
        .fails(1, "not_a_member");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("acct_99999999999999999999")
    );
}

/// The token's own account is the one entry marked `is_owner: true`, wherever the list puts
/// it. A list that marks none, or more than one, is `internal`: guessing would show a
/// membership as the token's own.
#[test]
fn get_of_self_is_the_account_marked_is_owner() {
    let answer =
        |body: &str| Server::start(vec![route("GET", "/api/accounts/", Reply::json(200, body))]);
    let member_first = answer(
        r#"{"accounts":[{"id":"acct_0123456789abcdef0123","is_owner":false},
            {"id":"acct_ffffffffffffffffffff","is_owner":true}]}"#,
    );
    let own = Hipc::against(&Home::new(), &member_first)
        .run(&["account", "get"])
        .ok();
    assert_eq!(own["id"], "acct_ffffffffffffffffffff");
    for body in [
        r#"{"accounts":[]}"#,
        r#"{"accounts":[{"id":"acct_ffffffffffffffffffff"}]}"#,
        r#"{"accounts":[{"id":"acct_ffffffffffffffffffff","is_owner":1}]}"#,
        r#"{"accounts":[{"id":"acct_ffffffffffffffffffff","is_owner":true},
            {"id":"acct_0123456789abcdef0123","is_owner":true}]}"#,
    ] {
        let error = Hipc::against(&Home::new(), &answer(body))
            .run(&["account", "get"])
            .fails(1, "internal");
        assert_eq!(
            error["message"],
            "the API's accounts list does not mark exactly one account as the token's own \
             with `is_owner`",
            "{body}"
        );
    }
}

#[test]
fn use_stores_the_selection_and_self_clears_it() {
    let server = accounts_server();
    let home = Home::new();
    let doc = Hipc::new(&home).run(&["account", "use", MEMBER]).ok();
    assert_eq!(doc, json!({"profile": "default", "account": MEMBER}));
    assert!(home.config_text().contains(MEMBER));
    let shown = Hipc::against(&home, &server).run(&["account", "get"]).ok();
    assert_eq!(shown["id"], MEMBER);
    let doc = Hipc::new(&home).run(&["account", "use", "self"]).ok();
    assert_eq!(doc["account"], "self");
    assert!(!home.config_text().contains(MEMBER));
}

#[test]
fn use_rejects_anything_but_self_or_an_id_without_a_request() {
    let server = Server::start(vec![]);
    let home = Home::new();
    for bad in [
        "",
        "SELF",
        "acct_123",
        "acct_0123456789ABCDEF0123",
        "user_1",
    ] {
        Hipc::against(&home, &server)
            .run(&["account", "use", bad])
            .fails(2, "usage");
    }
    assert!(server.requests().is_empty());
    assert!(!home.config().exists());
}

#[test]
fn use_dry_run_writes_nothing() {
    let home = Home::new();
    let doc = Hipc::new(&home)
        .run(&["account", "use", MEMBER, "--dry-run"])
        .ok();
    let fields: Vec<&str> = doc
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(fields, ["dry_run", "action", "profile", "account"]);
    assert_eq!(doc["account"], MEMBER);
    assert_eq!(doc["dry_run"], true);
    assert!(!home.config().exists());
}

#[test]
fn a_bad_selected_account_is_usage_for_every_command_that_reads_it() {
    let server = accounts_server();
    for args in [
        &["account", "list"][..],
        &["auth", "status"],
        &["account", "get"],
    ] {
        Hipc::against(&Home::new(), &server)
            .env("HIPPIUS_ACCOUNT", "nope")
            .run(args)
            .fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn the_flag_beats_the_environment() {
    let server = accounts_server();
    let doc = Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["account", "get", "--account", "self"])
        .ok();
    assert_eq!(doc["id"], "acct_ffffffffffffffffffff");
}

/// `hipc account notifications get` prints the signed-in user's `NotificationPreferences` as
/// sent. The route is personal: a membership sends no header and is not refused.
#[test]
fn notifications_reads_the_users_own_settings_without_the_header() {
    let settings = concat!(
        r#"{"email_enabled":true,"low_credit_alerts":true,"zero_balance_alerts":false,"#,
        r#""marketing_emails":false,"future_field":1}"#
    );
    let server = Server::start(vec![route(
        "GET",
        "/api/notifications/settings/",
        Reply::json(200, settings),
    )]);
    for account in ["self", MEMBER] {
        let doc = Hipc::against(&Home::new(), &server)
            .run(&["account", "notifications", "get", "--account", account])
            .ok();
        assert_eq!(
            doc,
            serde_json::from_str::<serde_json::Value>(settings).unwrap()
        );
    }
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    for request in requests {
        assert_eq!(request.method, "GET");
        assert_eq!(request.header("x-hippius-account"), None);
    }
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["account", "notifications", "get", "--dry-run"])
        .ok();
    assert_eq!(
        draft(&doc)["url"],
        format!("{}/api/notifications/settings/", server.url())
    );
    assert_eq!(server.requests().len(), 2, "a dry run sends nothing");
}

/// A failed read is the server's error, and no token sends nothing.
#[test]
fn notifications_fails_like_any_read() {
    let server = Server::start(vec![route(
        "GET",
        "/api/notifications/settings/",
        Reply::json(401, r#"{"detail":"Invalid token."}"#),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["account", "notifications", "get"])
        .fails(1, "auth");
    assert_eq!(
        error["message"],
        "cannot read the notification settings: the API says `Invalid token.`"
    );
    let empty = Server::start(vec![]);
    Hipc::new(&Home::new())
        .env("HIPPIUS_API_URL", empty.url())
        .run(&["account", "notifications", "get"])
        .fails(1, "auth");
    assert!(empty.requests().is_empty());
}

#[test]
fn list_and_get_dry_run_describe_the_accounts_request() {
    let server = Server::start(vec![]);
    for args in [
        &["account", "list", "--dry-run"][..],
        &["account", "get", "--dry-run"],
    ] {
        let doc = Hipc::against(&Home::new(), &server).run(args).ok();
        assert_eq!(
            draft(&doc)["url"],
            format!("{}/api/accounts/", server.url())
        );
    }
    assert!(server.requests().is_empty());
}
