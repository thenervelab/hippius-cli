//! `--project` and `--billing`, with `HIPPIUS_PROJECT` and `HIPPIUS_BILLING` and the profile,
//! and the reads that list their ids: which routes get `X-Hippius-Project` and
//! `X-Hippius-Billing`, which go out without them, and which are refused with nothing sent.

use serde_json::{Value, json};

use crate::common::{Hipc, Home, MEMBER, Reply, Server, draft, route};

const PROJECT: &str = "proj_0a1b2c3d4e5f60718293";
const BILLING: &str = "bill_9f8e7d6c5b4a39281706";

const PROJECTS: &str = concat!(
    r#"{"projects":[{"id":null,"name":"Default","is_default":true,"#,
    r#""billing":{"id":null,"name":"Default","is_default":true}},"#,
    r#"{"id":"proj_0a1b2c3d4e5f60718293","name":"web","is_default":false,"#,
    r#""billing":{"id":null,"name":"Default","is_default":true}}]}"#,
);

const BILLING_ACCOUNTS: &str =
    r#"{"billing_accounts":[{"id":null,"name":"Default","is_default":true,"projects":[]}]}"#;

const COSTS: &str = concat!(
    r#"{"billing_account":{"id":null,"name":"Default","is_default":true},"#,
    r#""from":"2026-10-01","to":"2026-10-30","group":["project"],"currency":"credits","#,
    r#""decimals":18,"total":"0","rows":[]}"#,
);

/// A server that answers each read these tests send with an empty or minimal body.
fn reads() -> Server {
    Server::start(vec![
        route("GET", "/api/compute/vms/", Reply::json(200, "[]")),
        route("GET", "/api/ssh-keys/", Reply::json(200, "[]")),
        route(
            "GET",
            "/api/billing/credits/balance/",
            Reply::json(200, r#"{"balance":"1"}"#),
        ),
        route("GET", "/api/s3/plans/", Reply::json(200, "[]")),
        route("GET", "/api/accounts/projects/", Reply::json(200, PROJECTS)),
        route(
            "GET",
            "/api/accounts/billing-accounts/",
            Reply::json(200, BILLING_ACCOUNTS),
        ),
    ])
}

/// A route that takes a project gets it, from the flag or the variable; one that takes a
/// billing account gets that; a personal route gets neither, and still goes out.
#[test]
fn each_selector_goes_on_the_routes_that_take_it() {
    let server = reads();
    let home = Home::new();
    Hipc::against(&home, &server)
        .run(&["compute", "vms", "list", "--project", PROJECT])
        .ok();
    Hipc::against(&home, &server)
        .env("HIPPIUS_PROJECT", PROJECT)
        .run(&["s3", "plans"])
        .ok();
    Hipc::against(&home, &server)
        .env("HIPPIUS_BILLING", BILLING)
        .run(&["billing", "balance"])
        .ok();
    Hipc::against(&home, &server)
        .env("HIPPIUS_PROJECT", PROJECT)
        .run(&["compute", "ssh-keys", "list"])
        .ok();
    Hipc::against(&home, &server)
        .env("HIPPIUS_BILLING", BILLING)
        .run(&["compute", "ssh-keys", "list"])
        .ok();
    let sent: Vec<(String, Option<String>, Option<String>)> = server
        .requests()
        .iter()
        .map(|r| {
            (
                r.path().to_owned(),
                r.header("x-hippius-project").map(str::to_owned),
                r.header("x-hippius-billing").map(str::to_owned),
            )
        })
        .collect();
    let some = |id: &str| Some(id.to_owned());
    assert_eq!(
        sent,
        [
            ("/api/compute/vms/".to_owned(), some(PROJECT), None),
            ("/api/s3/plans/".to_owned(), some(PROJECT), None),
            (
                "/api/billing/credits/balance/".to_owned(),
                None,
                some(BILLING)
            ),
            ("/api/ssh-keys/".to_owned(), None, None),
            ("/api/ssh-keys/".to_owned(), None, None),
        ]
    );
    for request in server.requests() {
        assert_eq!(request.header("x-hippius-account"), None);
    }
}

/// A route that does not take the selected project or billing account is refused before
/// anything is sent, a dry run and a confirmed write included, with exit 2 and the hint
/// that names `default`.
#[test]
fn a_route_that_does_not_take_the_selector_is_refused_with_nothing_sent() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let cases: [(&[&str], &str, &str); 7] = [
        (
            &["--project", PROJECT, "drive", "subscription", "get"],
            "project_scope_unsupported",
            "`GET /api/drive/subscription/` does not take a project",
        ),
        (
            &["--project", PROJECT, "billing", "balance"],
            "project_scope_unsupported",
            "`GET /api/billing/credits/balance/` does not take a project",
        ),
        // The S3 plan takes a project on its reads only.
        (
            &[
                "--project",
                PROJECT,
                "s3",
                "subscription",
                "cancel",
                "--yes",
            ],
            "project_scope_unsupported",
            "`DELETE /api/s3/subscription/` does not take a project",
        ),
        (
            &[
                "--project",
                PROJECT,
                "account",
                "projects",
                "list",
                "--dry-run",
            ],
            "project_scope_unsupported",
            "`GET /api/accounts/projects/` does not take a project",
        ),
        (
            &["--billing", BILLING, "compute", "vms", "list"],
            "billing_scope_unsupported",
            "`GET /api/compute/vms/` does not take a billing account",
        ),
        (
            &[
                "--billing",
                BILLING,
                "compute",
                "vms",
                "delete",
                "vm-1",
                "--dry-run",
            ],
            "billing_scope_unsupported",
            "`DELETE /api/compute/vms/vm-1/` does not take a billing account",
        ),
        (
            &["--billing", BILLING, "billing", "transactions"],
            "billing_scope_unsupported",
            "`GET /api/billing/transactions/` does not take a billing account",
        ),
    ];
    for (args, code, message) in cases {
        let error = Hipc::against(&home, &server).run(args).fails(2, code);
        let text = error["message"].as_str().unwrap();
        assert!(text.starts_with(message), "{args:?}: {text}");
        let hint = if code == "project_scope_unsupported" {
            "pass `--project default` to act in the Default project"
        } else {
            "pass `--billing default` to use the Default billing account"
        };
        assert_eq!(error["hint"], hint, "{args:?}");
    }
    assert!(server.requests().is_empty());
}

/// `--project` wins over `HIPPIUS_PROJECT`, which wins over the profile; `default` in a
/// flag or a variable reads the Default project even with an id stored.
#[test]
fn the_flag_wins_over_the_variable_which_wins_over_the_profile() {
    let server = reads();
    let home = Home::new();
    let other = "proj_ffffffffffffffffffff";
    Hipc::new(&home)
        .run(&["account", "projects", "use", other])
        .ok();
    let vms = ["compute", "vms", "list"];
    Hipc::against(&home, &server).run(&vms).ok();
    Hipc::against(&home, &server)
        .env("HIPPIUS_PROJECT", PROJECT)
        .run(&vms)
        .ok();
    Hipc::against(&home, &server)
        .env("HIPPIUS_PROJECT", other)
        .run(&["compute", "vms", "list", "--project", PROJECT])
        .ok();
    Hipc::against(&home, &server)
        .env("HIPPIUS_PROJECT", PROJECT)
        .run(&["compute", "vms", "list", "--project", "default"])
        .ok();
    Hipc::against(&home, &server)
        .env("HIPPIUS_PROJECT", "default")
        .run(&vms)
        .ok();
    let sent: Vec<Option<&str>> = server
        .requests()
        .iter()
        .map(|r| r.header("x-hippius-project"))
        .map(|h| h.map(|h| if h == other { "other" } else { "flag" }))
        .collect();
    assert_eq!(
        sent,
        [Some("other"), Some("flag"), Some("flag"), None, None]
    );
}

/// A malformed id is usage with nothing sent, and the message names where it came from:
/// the flag, the variable, or the profile, each with its own fix.
#[test]
fn a_malformed_id_names_its_source_and_sends_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let error = Hipc::against(&home, &server)
        .run(&["compute", "vms", "list", "--project", "proj_x"])
        .fails(2, "usage");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with("`proj_x` from `--project` is not a project id"),
        "{error}"
    );
    let error = Hipc::against(&home, &server)
        .env("HIPPIUS_BILLING", "acct_0123456789abcdef0123")
        .run(&["billing", "balance"])
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        "`acct_0123456789abcdef0123` from `HIPPIUS_BILLING` is not a billing account id: a \
         billing account is `default`, or `bill_` and 20 lowercase hex characters"
    );
    assert_eq!(
        error["hint"],
        "unset `HIPPIUS_BILLING`, or set it to `default` or an id from `hipc account \
         billing-accounts list`"
    );
    home.write_config("[profiles.default]\nproject = \"proj_x\"\n");
    let error = Hipc::against(&home, &server)
        .run(&["compute", "vms", "list"])
        .fails(2, "usage");
    let message = error["message"].as_str().unwrap();
    assert!(
        message.starts_with("`proj_x` from the `project` of the profile `default` in ")
            && message.ends_with(
                " is not a project id: a project is `default`, or `proj_` and 20 lowercase hex \
                 characters"
            ),
        "{message}"
    );
    assert_eq!(
        error["hint"],
        "run `hipc account projects use default` to clear the stored project"
    );
    // The hint works: storing `default` does not read the bad selection first.
    Hipc::new(&home)
        .run(&["account", "projects", "use", "default"])
        .ok();
    assert!(!home.config_text().contains("proj_x"));
    for bad in ["", "Default", "proj_0A1B2C3D4E5F60718293", BILLING] {
        Hipc::new(&home)
            .run(&["account", "projects", "use", bad])
            .fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

/// `use` stores the selection in the profile and `default` removes it; a dry run writes
/// nothing; and `hipc account use` with another account removes both, since they belong to
/// the account they were picked in.
#[test]
fn use_stores_the_selection_and_a_new_account_clears_it() {
    let home = Home::new();
    let doc = Hipc::new(&home)
        .run(&["account", "projects", "use", PROJECT])
        .ok();
    assert_eq!(doc, json!({"profile": "default", "project": PROJECT}));
    let doc = Hipc::new(&home)
        .run(&["account", "billing-accounts", "use", BILLING])
        .ok();
    assert_eq!(doc, json!({"profile": "default", "billing": BILLING}));
    let text = home.config_text();
    assert!(text.contains(PROJECT) && text.contains(BILLING), "{text}");

    let doc = Hipc::new(&home)
        .run(&["account", "projects", "use", "default", "--dry-run"])
        .ok();
    assert_eq!(
        doc,
        json!({
            "dry_run": true,
            "action": "select_project",
            "profile": "default",
            "project": "default",
        })
    );
    assert!(home.config_text().contains(PROJECT));

    let doc = Hipc::new(&home)
        .run(&["account", "billing-accounts", "use", "default", "--dry-run"])
        .ok();
    assert_eq!(
        doc,
        json!({
            "dry_run": true,
            "action": "select_billing",
            "profile": "default",
            "billing": "default",
        })
    );
    assert!(home.config_text().contains(BILLING));
    let doc = Hipc::new(&home)
        .run(&["account", "billing-accounts", "use", "default"])
        .ok();
    assert_eq!(doc["billing"], "default");
    assert!(!home.config_text().contains(BILLING));

    Hipc::new(&home).run(&["account", "use", MEMBER]).ok();
    assert!(!home.config_text().contains(PROJECT));
}

/// `use` stores a project or billing account only for the account the profile acts in: an
/// `--account` or `HIPPIUS_ACCOUNT` that names another is refused with nothing written, a dry
/// run included, since the selection would sit beside an account that does not have it.
#[test]
fn use_refuses_a_selection_for_another_account() {
    let home = Home::new();
    Hipc::new(&home).run(&["account", "use", MEMBER]).ok();
    let before = home.config_text();
    let other = "acct_ffffffffffffffffffff";
    let message = format!(
        "selects the account `{other}`, and the profile `default` acts in the account \
         `{MEMBER}`: a project or billing account is stored for the profile's account"
    );
    for words in [
        ["account", "projects", "use", PROJECT],
        ["account", "billing-accounts", "use", BILLING],
    ] {
        for dry_run in [&[][..], &["--dry-run"][..]] {
            let args = [&["--account", other][..], &words, dry_run].concat();
            let error = Hipc::new(&home).run(&args).fails(2, "usage");
            assert_eq!(
                error["message"],
                format!("`--account` {message}"),
                "{args:?}"
            );
            assert_eq!(
                error["hint"],
                format!("run `hipc account use {other}` first")
            );
            let args = [&words[..], dry_run].concat();
            let error = Hipc::new(&home)
                .env("HIPPIUS_ACCOUNT", other)
                .run(&args)
                .fails(2, "usage");
            assert_eq!(error["message"], format!("`HIPPIUS_ACCOUNT` {message}"));
        }
    }
    assert_eq!(home.config_text(), before);
    // Naming the profile's own account is no change, so the selection is stored.
    Hipc::new(&home)
        .run(&["--account", MEMBER, "account", "projects", "use", PROJECT])
        .ok();
    Hipc::new(&home)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["account", "billing-accounts", "use", BILLING])
        .ok();
    let text = home.config_text();
    assert!(text.contains(PROJECT) && text.contains(BILLING), "{text}");
}

/// A project or billing account stored in the profile goes on the routes that take it, and
/// every other route goes out without it instead of being refused, the accounts API and the
/// personal routes included; the dry run shows `null` for a header left off.
#[test]
fn a_stored_selector_goes_only_on_the_routes_that_take_it() {
    let server = reads();
    let home = Home::new();
    Hipc::new(&home)
        .run(&["account", "projects", "use", PROJECT])
        .ok();
    Hipc::new(&home)
        .run(&["account", "billing-accounts", "use", BILLING])
        .ok();
    for args in [
        &["compute", "vms", "list"][..],
        &["billing", "balance"],
        &["compute", "ssh-keys", "list"],
        &["account", "projects", "list"],
    ] {
        Hipc::against(&home, &server).run(args).ok();
    }
    let sent: Vec<(String, Option<String>, Option<String>)> = server
        .requests()
        .iter()
        .map(|r| {
            (
                r.path().to_owned(),
                r.header("x-hippius-project").map(str::to_owned),
                r.header("x-hippius-billing").map(str::to_owned),
            )
        })
        .collect();
    let some = |id: &str| Some(id.to_owned());
    assert_eq!(
        sent,
        [
            ("/api/compute/vms/".to_owned(), some(PROJECT), None),
            (
                "/api/billing/credits/balance/".to_owned(),
                None,
                some(BILLING)
            ),
            ("/api/ssh-keys/".to_owned(), None, None),
            ("/api/accounts/projects/".to_owned(), None, None),
        ]
    );
    let doc = Hipc::new(&home)
        .run(&["--dry-run", "account", "projects", "list"])
        .ok();
    assert_eq!(draft(&doc).get("project_header"), Some(&Value::Null));
    assert_eq!(draft(&doc).get("billing_header"), Some(&Value::Null));
}

/// A stored project or billing account is left off a read of a route that cannot take it;
/// a stored project's write there is refused with nothing sent, a dry run included, and the
/// message names the profile, while a stored billing account is left off the write too; a
/// personal write goes out without either header.
#[test]
fn a_stored_selector_is_skipped_on_reads_and_a_stored_project_refused_on_writes() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/billing/credits/balance/",
            Reply::json(200, r#"{"balance":"1"}"#),
        ),
        route("GET", "/api/compute/vms/", Reply::json(200, "[]")),
        route(
            "POST",
            "/api/support/tickets/",
            Reply::json(201, r#"{"id":7}"#),
        ),
    ]);
    let cases = [
        (
            "projects",
            PROJECT,
            &["billing", "balance"][..],
            "project_scope_unsupported",
            "project",
            "pass `--project default` to act in the Default project",
        ),
        (
            "billing-accounts",
            BILLING,
            &["compute", "vms", "list"],
            "billing_scope_unsupported",
            "billing",
            "pass `--billing default` to use the Default billing account",
        ),
    ];
    for (group, id, read, code, field, hint) in cases {
        let home = Home::new();
        Hipc::new(&home).run(&["account", group, "use", id]).ok();
        Hipc::against(&home, &server).run(read).ok();
        for extra in ["--dry-run", "--yes"] {
            let subscribe = [
                "drive",
                "subscription",
                "subscribe",
                "--plan",
                "solo",
                extra,
            ];
            // A stored billing account is left off a write that takes none, as off a read.
            if group == "billing-accounts" {
                if extra == "--dry-run" {
                    let doc = Hipc::against(&home, &server).run(&subscribe).ok();
                    assert!(!doc.to_string().contains(BILLING), "{doc}");
                }
                continue;
            }
            let error = Hipc::against(&home, &server).run(&subscribe).fails(2, code);
            let message = error["message"].as_str().unwrap();
            assert!(
                message.starts_with("`POST /api/drive/subscription/` does not take a ")
                    && message.contains(&format!("from the `{field}` of the profile `default` in"))
                    && message
                        .ends_with(&format!("; `hipc account {group} use default` clears it")),
                "{message}"
            );
            assert_eq!(error["hint"], hint);
        }
        let file = home.file("message.txt", b"help");
        let file = file.to_str().unwrap();
        Hipc::against(&home, &server)
            .run(&[
                "support",
                "tickets",
                "create",
                "--subject",
                "Refund",
                "--body-file",
                file,
                "--yes",
            ])
            .ok();
    }
    let sent: Vec<(String, bool)> = server
        .requests()
        .iter()
        .map(|r| {
            let selector =
                r.header("x-hippius-project").is_some() || r.header("x-hippius-billing").is_some();
            (r.path().to_owned(), selector)
        })
        .collect();
    let tickets = ("/api/support/tickets/".to_owned(), false);
    assert_eq!(
        sent,
        [
            ("/api/billing/credits/balance/".to_owned(), false),
            tickets.clone(),
            ("/api/compute/vms/".to_owned(), false),
            tickets,
        ]
    );
}

/// A personal route inside a project prefix goes out without the project with or without its
/// trailing `/`: the slashless form is not read as the wider prefix.
#[test]
fn a_slashless_personal_route_goes_out_without_the_stored_project() {
    let paths = [
        "/api/compute/edge/",
        "/api/compute/edge",
        "/api/databases/12/members",
    ];
    let server = Server::start(
        paths
            .iter()
            .map(|path| route("GET", path, Reply::json(200, "[]")))
            .collect(),
    );
    let home = Home::new();
    Hipc::new(&home)
        .run(&["account", "projects", "use", PROJECT])
        .ok();
    for path in paths {
        let path = path.strip_prefix("/api").unwrap();
        Hipc::against(&home, &server)
            .run(&["api", "GET", path])
            .ok();
        Hipc::against(&home, &server)
            .run(&["--project", PROJECT, "api", "GET", path])
            .ok();
    }
    let requests = server.requests();
    assert_eq!(requests.len(), 6);
    for request in requests {
        assert_eq!(
            request.header("x-hippius-project"),
            None,
            "{}",
            request.path()
        );
    }
}

/// A selection from a flag or a variable is still refused on a route that does not take it,
/// and the message names which one it came from (tenet 15).
#[test]
fn a_refused_selection_names_the_flag_or_variable_it_came_from() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let error = Hipc::against(&home, &server)
        .run(&["--project", PROJECT, "billing", "balance"])
        .fails(2, "project_scope_unsupported");
    assert_eq!(
        error["message"],
        format!(
            "`GET /api/billing/credits/balance/` does not take a project, and the selected \
             project is `{PROJECT}` from `--project`"
        )
    );
    let error = Hipc::against(&home, &server)
        .env("HIPPIUS_BILLING", BILLING)
        .run(&["compute", "vms", "list"])
        .fails(2, "billing_scope_unsupported");
    assert_eq!(
        error["message"],
        format!(
            "`GET /api/compute/vms/` does not take a billing account, and the selected billing \
             account is `{BILLING}` from `HIPPIUS_BILLING`"
        )
    );
    // A flag wins over the stored value, so it is refused even with the same id stored.
    Hipc::new(&home)
        .run(&["account", "projects", "use", PROJECT])
        .ok();
    Hipc::against(&home, &server)
        .run(&["--project", PROJECT, "drive", "subscription", "get"])
        .fails(2, "project_scope_unsupported");
    assert!(server.requests().is_empty());
}

/// The dry run names the selector headers it would send, and `null` on a personal route or
/// without a selection, so all three fields are always there.
#[test]
fn the_dry_run_shows_the_selector_headers() {
    // Each header as `get` reads it, so a missing key (`None`) fails where `null` is expected.
    fn headers(doc: &Value) -> [Option<&Value>; 3] {
        ["account_header", "project_header", "billing_header"].map(|field| draft(doc).get(field))
    }
    let (null, project, billing) = (Value::Null, json!(PROJECT), json!(BILLING));
    let home = Home::new();
    let doc = Hipc::new(&home)
        .run(&[
            "--project",
            PROJECT,
            "--dry-run",
            "compute",
            "vms",
            "delete",
            "vm-1",
        ])
        .ok();
    assert_eq!(headers(&doc), [Some(&null), Some(&project), Some(&null)]);
    assert!(doc["requests"][0].get("dry_run").is_none(), "{doc}");
    let doc = Hipc::new(&home)
        .run(&[
            "--billing",
            BILLING,
            "--dry-run",
            "billing",
            "invoices",
            "list",
        ])
        .ok();
    assert_eq!(headers(&doc), [Some(&null), Some(&null), Some(&billing)]);
    // A personal route takes no project, so the selected one is not sent.
    let doc = Hipc::new(&home)
        .run(&[
            "--project",
            PROJECT,
            "--dry-run",
            "compute",
            "ssh-keys",
            "list",
        ])
        .ok();
    assert_eq!(headers(&doc), [Some(&null); 3]);
    let doc = Hipc::new(&home)
        .run(&["--dry-run", "compute", "vms", "list"])
        .ok();
    assert_eq!(headers(&doc), [Some(&null); 3]);
}

/// A member's top-up names a billing account, as the console sends one from a billing
/// account's page: with `--billing` both headers go out; without it the membership is still
/// refused.
#[test]
fn a_member_tops_up_a_named_billing_account() {
    let card = "/api/billing/stripe/create-checkout-session/";
    let server = Server::start(vec![route(
        "POST",
        card,
        Reply::json(200, r#"{"checkout_url":"https://checkout.example/c"}"#),
    )]);
    let top_up = [
        "billing", "top-up", "--amount", "50", "--method", "card", "--yes",
    ];
    Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .env("HIPPIUS_BILLING", BILLING)
        .run(&top_up)
        .ok();
    let sent = server.only_request();
    assert_eq!(sent.header("x-hippius-account"), Some(MEMBER));
    assert_eq!(sent.header("x-hippius-billing"), Some(BILLING));
    Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&top_up)
        .fails(2, "account_scope_unsupported");
    assert_eq!(server.requests().len(), 1);
    // A billing account stored in the membership's profile counts as one named for the run.
    let home = Home::new();
    Hipc::new(&home).run(&["account", "use", MEMBER]).ok();
    Hipc::new(&home)
        .run(&["account", "billing-accounts", "use", BILLING])
        .ok();
    Hipc::against(&home, &server).run(&top_up).ok();
    let sent = &server.requests()[1];
    assert_eq!(sent.header("x-hippius-account"), Some(MEMBER));
    assert_eq!(sent.header("x-hippius-billing"), Some(BILLING));
}

/// The legacy Stripe row acts in a membership beside a billing account only.
#[test]
fn the_stripe_row_takes_a_membership_beside_a_billing_account() {
    let server = Server::start(vec![route(
        "GET",
        "/api/billing/stripe/active-subscription/",
        Reply::json(200, r#"{"has_subscription":false}"#),
    )]);
    Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["billing", "stripe"])
        .fails(2, "account_scope_unsupported");
    Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["billing", "stripe", "--billing", BILLING])
        .ok();
    let sent = server.only_request();
    assert_eq!(sent.header("x-hippius-account"), Some(MEMBER));
    assert_eq!(sent.header("x-hippius-billing"), Some(BILLING));
}

/// The server's own refusal of a selector passes through with its code and exit 2.
#[test]
fn the_servers_selector_refusal_passes_through_as_exit_2() {
    let body = r#"{"code":"project_scope_unsupported","detail":"no project here"}"#;
    let server = Server::start(vec![route(
        "GET",
        "/api/compute/vms/",
        Reply::json(403, body),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["compute", "vms", "list", "--project", PROJECT])
        .fails(2, "project_scope_unsupported");
    assert_eq!(error["http_status"], 403);
    assert_eq!(
        error["hint"],
        "pass `--project default` to act in the Default project"
    );
}

/// The two lists are envelopes of the server's `projects` and `billing_accounts`, and a
/// membership names the account whose lists they are.
#[test]
fn the_lists_are_envelopes_and_take_the_account_header() {
    let server = reads();
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["account", "projects", "list"])
        .ok();
    assert_eq!(doc["next_page_token"], serde_json::Value::Null);
    assert_eq!(doc["items"][1]["id"], PROJECT);
    let doc = Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["account", "billing-accounts", "list"])
        .ok();
    assert_eq!(doc["items"][0]["name"], "Default");
    let requests = server.requests();
    assert_eq!(requests[0].header("x-hippius-account"), None);
    assert_eq!(requests[1].header("x-hippius-account"), Some(MEMBER));
}

/// The costs read sends only the query it was given, `group` once in the console's order
/// however the flags are ordered or repeated, and prints the server's body unchanged.
#[test]
fn costs_sends_the_range_and_groups_given() {
    let path = format!("/api/accounts/billing-accounts/{BILLING}/costs/");
    let server = Server::start(vec![route("GET", &path, Reply::json(200, COSTS))]);
    let run = Hipc::against(&Home::new(), &server).run(&[
        "account",
        "billing-accounts",
        "costs",
        BILLING,
        "--from",
        "2026-09-01",
        "--to",
        "2026-09-30",
        "--group-by",
        "day",
        "--group-by",
        "project",
        "--group-by",
        "day",
        "--group-by",
        "kind",
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(run.stdout(), format!("{COSTS}\n"));
    assert_eq!(
        server.only_request().target,
        format!("{path}?from=2026-09-01&to=2026-09-30&group=project%2Ckind%2Cday")
    );
    let server = Server::start(vec![route(
        "GET",
        "/api/accounts/billing-accounts/default/costs/",
        Reply::json(200, COSTS),
    )]);
    Hipc::against(&Home::new(), &server)
        .run(&["account", "billing-accounts", "costs", "default"])
        .ok();
    assert_eq!(
        server.only_request().target,
        "/api/accounts/billing-accounts/default/costs/"
    );
}

/// Text mode shows `total` and each row's `amount` in credits, as `billing summary` and
/// `usage` do, and says so; JSON keeps the base units, and a `decimals` other than 18 is
/// left as sent.
#[test]
fn costs_text_mode_shows_amounts_in_credits() {
    let body = concat!(
        r#"{"billing_account":{"id":null,"name":"Default","is_default":true},"#,
        r#""currency":"credits","decimals":18,"total":"2500000000000000000","#,
        r#""rows":[{"kind":"vm","amount":"1500000000000000000"}]}"#
    );
    let path = "/api/accounts/billing-accounts/default/costs/";
    let server = Server::start(vec![route("GET", path, Reply::json(200, body))]);
    let args = ["account", "billing-accounts", "costs", "default"];
    let run =
        Hipc::against(&Home::new(), &server).run(&[&["--output", "text"][..], &args].concat());
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    let total = run
        .stdout()
        .lines()
        .find(|line| line.starts_with("total"))
        .unwrap();
    assert_eq!(
        total.split_whitespace().last(),
        Some("2.5"),
        "{}",
        run.stdout()
    );
    assert!(
        run.stdout().contains(r#""amount":"1.5""#),
        "{}",
        run.stdout()
    );
    assert_eq!(
        run.stderr(),
        "note: `total` and `amount` are in credits here (1 credit = 1 USD); the JSON output \
         keeps base units (10^18 per credit)\n"
    );
    let doc = Hipc::against(&Home::new(), &server).run(&args).ok();
    assert_eq!(doc["total"], "2500000000000000000");
    let other = body.replace(r#""decimals":18"#, r#""decimals":6"#);
    let server = Server::start(vec![route("GET", path, Reply::json(200, &other))]);
    let run =
        Hipc::against(&Home::new(), &server).run(&[&["--output", "text"][..], &args].concat());
    assert!(
        run.stdout().contains("2500000000000000000"),
        "{}",
        run.stdout()
    );
    assert_eq!(run.stderr(), "");
}

/// A bad billing account or day is usage before anything is sent, and a 404 names the
/// list of ids.
#[test]
fn costs_checks_its_input_and_a_404_names_the_list() {
    let server = Server::start(vec![route(
        "GET",
        &format!("/api/accounts/billing-accounts/{BILLING}/costs/"),
        Reply::not_found(),
    )]);
    let home = Home::new();
    for args in [
        &["bill_x"][..],
        &["../x"],
        &[BILLING, "--from", "2026-9-1"],
        &[BILLING, "--to", "2026-13-01"],
        &[BILLING, "--to", "2026-09-32"],
        &[BILLING, "--to", "2026-09-00"],
        &[BILLING, "--group-by", "month"],
    ] {
        let words: Vec<&str> = ["account", "billing-accounts", "costs"]
            .iter()
            .chain(args)
            .copied()
            .collect();
        Hipc::against(&home, &server).run(&words).fails(2, "usage");
    }
    assert!(server.requests().is_empty());
    let error = Hipc::against(&home, &server)
        .run(&["account", "billing-accounts", "costs", BILLING])
        .fails(1, "not_found");
    assert_eq!(
        error["hint"],
        "run `hipc account billing-accounts list` to see what exists"
    );
}

/// A write that goes out without a stored billing account, because its route takes none,
/// says so in its question: outside a project its charge goes to the Default billing
/// account. A personal write, and a write of a route that names its billing account, say
/// nothing of it.
#[test]
fn a_question_names_the_stored_billing_account_it_skips() {
    let server = Server::start(vec![]);
    let home = Home::new();
    Hipc::new(&home)
        .run(&["account", "billing-accounts", "use", BILLING])
        .ok();
    let refused = |argv: &[&str]| {
        let error = Hipc::against(&home, &server)
            .run(argv)
            .fails(2, "confirmation_required");
        error["message"].as_str().unwrap().to_owned()
    };
    assert_eq!(
        refused(&["drive", "subscription", "subscribe", "--plan", "solo"]),
        format!(
            "this would buy the Drive plan `solo`, without the stored billing account \
             `{BILLING}`, which this route does not take, so any balance charge goes to the \
             Default billing account, and `--yes` was not passed"
        )
    );
    // In a project, the project's own billing decides, so only the skip is named.
    let vm = "0f8fad5b-d9cb-469f-a165-70867728950e";
    let stop = refused(&["compute", "vms", "stop", vm, "--project", PROJECT]);
    assert!(
        stop.ends_with(&format!(
            "in the project `{PROJECT}`, without the stored billing account `{BILLING}`, which \
             this route does not take, and `--yes` was not passed"
        )),
        "{stop}"
    );
    // A personal route acts as the caller, with no billing account to skip.
    let delete = refused(&["compute", "ssh-keys", "delete", "1"]);
    assert!(!delete.contains("billing"), "{delete}");
    // A route that names one project in its path acts on that one.
    let budget = refused(&["account", "projects", "budget", "delete", PROJECT]);
    assert!(!budget.contains("stored"), "{budget}");
    assert!(server.requests().is_empty());
}
