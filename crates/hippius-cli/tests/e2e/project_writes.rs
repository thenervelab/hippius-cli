//! The project and billing-account writes under `hipc account`: create, rename, archive, a
//! project's budget and quota, and the project transfers. Each body is the console's, since
//! swagger types none; each confirmed write sends nothing without `--yes`; and the selectors
//! go where tenet 12 says, the transfers being personal.

use hippius::routes::account;
use serde_json::{Value, json};

use crate::common::{
    self, ACCOUNTS, BILLING, Hipc, Home, MEMBER, OWN_ACCOUNT, PROJECT, Reply, Run, Server, draft,
    no_server, one_request_dry_run, parsed, route, route_of,
};

const OTHER: &str = "acct_ffffffffffffffffffff";
/// A second project, beside `PROJECT`.
const SECOND: &str = "proj_ffffffffffffffffffff";
/// A second billing account, beside `BILLING`.
const SECOND_BILLING: &str = "bill_ffffffffffffffffffff";

const ONE_PROJECT: &str = concat!(
    r#"{"id":"proj_0a1b2c3d4e5f60718293","name":"web","is_default":false,"#,
    r#""billing":{"id":null,"name":"Default","is_default":true},"#,
    r#""created_by":null,"created_at":"2026-11-01T10:00:00Z"}"#
);

const ONE_BILLING: &str = concat!(
    r#"{"id":"bill_9f8e7d6c5b4a39281706","name":"ops","is_default":false,"projects":[],"#,
    r#""created_by":null,"created_at":"2026-11-01T10:00:00Z"}"#
);

const BUDGET: &str = concat!(
    r#"{"project":{"id":"proj_0a1b2c3d4e5f60718293","name":"web","is_default":false},"#,
    r#""monthly_limit":"100.00","thresholds":[50,90,100],"block_launch_over":false,"#,
    r#""month":"2026-11","spent":"3.25","alerted":[]}"#
);

const QUOTA: &str = concat!(
    r#"{"project":{"id":"proj_0a1b2c3d4e5f60718293","name":"web","is_default":false},"#,
    r#""limits":{"vms":5,"databases":null,"clusters":null,"vcpus":null,"memory_gb":null,"#,
    r#""disk_gb":null,"public_ips":null,"runners":null},"#,
    r#""used":{"vms":1,"databases":0,"clusters":0,"vcpus":2,"memory_gb":4,"disk_gb":40,"#,
    r#""public_ips":0,"runners":0},"#,
    r#""account_limits":{"vms":10,"databases":5,"clusters":2,"vcpus":40,"memory_gb":160,"#,
    r#""disk_gb":2000,"public_ips":5,"runners":10}}"#
);

const TRANSFER: &str = concat!(
    r#"{"id":5,"project":{"id":"proj_0a1b2c3d4e5f60718293","name":"web"},"#,
    r#""from_account":{"id":"acct_0123456789abcdef0123","name":"Ana"},"#,
    r#""to_account":{"id":"acct_ffffffffffffffffffff","name":"Bo"},"status":"pending","#,
    r#""created_at":"2026-11-01T10:00:00Z","expires_at":"2026-11-08T10:00:00Z","#,
    r#""decided_at":null}"#
);

fn hipc(server: &Server, args: &[&str]) -> Run {
    Hipc::against(&Home::new(), server).run(args)
}

fn with_yes<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut argv = args.to_vec();
    argv.push("--yes");
    argv
}

/// `hipc account projects <args>`.
fn projects<'a>(args: &[&'a str]) -> Vec<&'a str> {
    [&["account", "projects"][..], args].concat()
}

/// `hipc account project-transfers <args>`.
fn transfers<'a>(args: &[&'a str]) -> Vec<&'a str> {
    [&["account", "project-transfers"][..], args].concat()
}

/// Asserts that `args` sends nothing without `--yes` and names `action` in the refusal, then
/// sends exactly `method path` with `body` (`Value::Null` for none) once confirmed, and
/// prints `printed`.
fn confirmed(
    routes: Vec<crate::common::Route>,
    args: &[&str],
    action: &str,
    (method, path, body): (&str, &str, Value),
    printed: &Value,
) {
    let server = Server::start(routes);
    let error = hipc(&server, args).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!("this would {action}, and `--yes` was not passed"),
        "{args:?}"
    );
    assert_eq!(error["hint"], "pass `--yes` to confirm");
    assert!(server.requests().is_empty(), "{args:?}");
    assert_eq!(&hipc(&server, &with_yes(args)).ok(), printed, "{args:?}");
    let sent = server.only_request();
    assert_eq!((sent.method.as_str(), sent.target.as_str()), (method, path));
    if body.is_null() {
        assert!(sent.body.is_empty(), "{args:?}: {:?}", sent.body);
    } else {
        assert_eq!(sent.json(), body, "{args:?}");
    }
}

/// A project is free (the free-create rule): the create needs no `--yes`, sends the name
/// trimmed with the Default billing account, as the console does, and prints the project.
#[test]
fn projects_create_sends_the_trimmed_name_without_yes() {
    let server = Server::start(vec![route_of(
        &account::PROJECT_CREATE,
        [],
        Reply::json(201, ONE_PROJECT),
    )]);
    let doc = hipc(&server, &projects(&["create", "--name", "  web "])).ok();
    assert_eq!(doc, parsed(ONE_PROJECT));
    let sent = server.only_request();
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.json(), json!({"name": "web", "billing": null}));
}

/// A billing account is free too, and its create sends only the name, as the console does.
#[test]
fn billing_accounts_create_and_update_send_the_name_without_yes() {
    let server = Server::start(vec![
        route_of(
            &account::BILLING_ACCOUNT_CREATE,
            [],
            Reply::json(201, ONE_BILLING),
        ),
        route(
            "PATCH",
            &format!("/api/accounts/billing-accounts/{BILLING}/"),
            Reply::json(200, ONE_BILLING),
        ),
    ]);
    let create = ["account", "billing-accounts", "create", "--name", "ops"];
    assert_eq!(hipc(&server, &create).ok(), parsed(ONE_BILLING));
    let update = [
        "account",
        "billing-accounts",
        "update",
        BILLING,
        "--name",
        " ops-eu",
    ];
    assert_eq!(hipc(&server, &update).ok(), parsed(ONE_BILLING));
    let sent = server.requests();
    assert_eq!(sent[0].json(), json!({"name": "ops"}));
    assert_eq!(sent[1].method, "PATCH");
    assert_eq!(sent[1].json(), json!({"name": "ops-eu"}));
}

/// A rename is not confirmed, and sends only the new name.
#[test]
fn projects_update_renames_without_yes() {
    let path = format!("/api/accounts/projects/{PROJECT}/");
    let server = Server::start(vec![route("PATCH", &path, Reply::json(200, ONE_PROJECT))]);
    let doc = hipc(&server, &projects(&["update", PROJECT, "--name", "web"])).ok();
    assert_eq!(doc, parsed(ONE_PROJECT));
    let sent = server.only_request();
    assert_eq!(
        (sent.method.as_str(), sent.target.as_str()),
        ("PATCH", &*path)
    );
    assert_eq!(sent.json(), json!({"name": "web"}));
}

/// `cli::projects::tests` holds every refused name in each command that takes one; this pins
/// the `usage` a refusal becomes, and that 64 characters fit, counted as characters.
#[test]
fn a_name_outside_the_console_rule_is_refused() {
    let server = Server::start(vec![]);
    let error = hipc(&server, &projects(&["create", "--name", "two\nlines"])).fails(2, "usage");
    assert_eq!(
        error["message"],
        "invalid value `two\\nlines` for `--name <NAME>`: a name holds no control character or \
         line break, a tab or a newline included, since it is shown on one line"
    );
    assert!(server.requests().is_empty());
    let fits = "é".repeat(64);
    let doc = Hipc::new(&Home::new())
        .run(&[
            "--dry-run",
            "account",
            "projects",
            "create",
            "--name",
            &fits,
        ])
        .ok();
    assert_eq!(draft(&doc)["body"]["name"], fits);
}

/// `cli::projects::tests` holds every refused id in each command; this pins the `usage` each
/// id parser's refusal becomes, and that nothing is sent.
#[test]
fn a_malformed_or_default_id_is_refused_before_anything_is_sent() {
    let server = Server::start(vec![]);
    for (args, says) in [
        (
            projects(&["update", "default", "--name", "x"]),
            "a project id is `proj_` and 20 lowercase hex characters, from `hipc account \
             projects list`; the Default project has none",
        ),
        (
            vec![
                "account",
                "billing-accounts",
                "update",
                "bill_x",
                "--name",
                "x",
            ],
            "a billing account id is `bill_` and 20 lowercase hex characters, from `hipc \
             account billing-accounts list`; the Default billing account has none",
        ),
        (
            projects(&["transfer", PROJECT, "--to-account", "acct_x", "--yes"]),
            "an account id is `acct_` and 20 lowercase hex characters",
        ),
        (
            transfers(&["accept", "x", "--yes"]),
            "a transfer id is a whole number, from `hipc account project-transfers list`",
        ),
        (
            transfers(&["accept", "5", "--bill-to", "bill_x", "--yes"]),
            "a billing account is `default`, or `bill_` and 20 lowercase hex characters, from \
             `hipc account billing-accounts list`",
        ),
    ] {
        let error = hipc(&server, &args).fails(2, "usage");
        let message = error["message"].as_str().expect("the message is a string");
        assert!(message.ends_with(says), "{args:?}: {message}");
    }
    assert!(server.requests().is_empty());
}

/// Swagger: the delete archives an empty project. The question says so, and a 204 prints
/// the archived id.
#[test]
fn projects_delete_archives_with_yes() {
    let path = format!("/api/accounts/projects/{PROJECT}/");
    confirmed(
        vec![route("DELETE", &path, Reply::json(204, ""))],
        &projects(&["delete", PROJECT]),
        &format!("archive the project `{PROJECT}`, which must own nothing"),
        ("DELETE", &path, Value::Null),
        &json!({"id": PROJECT, "archived": true}),
    );
}

/// The server refuses a project that still owns resources with 409 `project_not_empty`;
/// that is `conflict`, with nothing done, and the message names what the project holds, as
/// the console's resource list gives it, so the caller knows what to remove. `details`
/// keeps the whole body.
#[test]
fn projects_delete_of_a_project_that_owns_something_names_what_it_holds() {
    let path = format!("/api/accounts/projects/{PROJECT}/");
    let refused = |body: &str| {
        let server = Server::start(vec![route("DELETE", &path, Reply::json(409, body))]);
        hipc(&server, &projects(&["delete", PROJECT, "--yes"])).fails(1, "conflict")
    };
    let body = concat!(
        r#"{"code":"project_not_empty","resources":[{"kind":"vm","id":"vm-1","name":"a"},"#,
        r#"{"kind":"bucket","id":7},{"kind":"widget","id":"w","name":"b"},"#,
        r#"{"kind":"vm"},{"id":"x"},{"kind":"vm","id":true}]}"#
    );
    let error = refused(body);
    assert_eq!(
        error["message"],
        format!(
            "cannot archive the project `{PROJECT}`: it still holds the VM `a` (`vm-1`), the \
             bucket `7`, the resource of kind `widget` `b` (`w`), and 3 more"
        )
    );
    assert_eq!(
        error["hint"],
        "delete or move what the project holds, then archive it again"
    );
    assert_eq!(error["side_effects"], "none");
    assert_eq!(error["details"], parsed(body));
    // Past five, the rest are counted; every kind the console names reads in words.
    let kinds = [
        "vm",
        "database",
        "kubernetes",
        "public_ip",
        "cdn_zone",
        "bucket",
        "s3_master_token",
        "s3_sub_token",
        "runner_installation",
        "runner_job",
    ];
    let words = [
        "VM",
        "database",
        "Kubernetes cluster",
        "public IP",
        "CDN zone",
        "bucket",
        "S3 master token",
        "S3 sub-token",
        "runner installation",
        "runner job",
    ];
    for (start, kinds) in [(0, &kinds[..5]), (5, &kinds[5..])] {
        let mut resources: Vec<Value> = kinds
            .iter()
            .map(|kind| json!({"kind": kind, "id": "i"}))
            .collect();
        resources.push(json!({"kind": "vm", "id": "extra"}));
        let body = json!({"code": "project_not_empty", "resources": resources});
        let error = refused(&body.to_string());
        let shown: Vec<String> = words[start..start + 5]
            .iter()
            .map(|word| format!("the {word} `i`"))
            .collect();
        assert_eq!(
            error["message"],
            format!(
                "cannot archive the project `{PROJECT}`: it still holds {}, and 1 more",
                shown.join(", ")
            )
        );
    }
    // Past five, every other resource is counted, one hipc cannot name included: seven
    // nameable and one without a kind leave three.
    let mut resources: Vec<Value> = (0..7).map(|i| json!({"kind": "vm", "id": i})).collect();
    resources.push(json!({"id": "x"}));
    let counted = json!({"code": "project_not_empty", "resources": resources});
    let error = refused(&counted.to_string());
    assert_eq!(
        error["message"],
        format!(
            "cannot archive the project `{PROJECT}`: it still holds the VM `0`, the VM `1`, the \
             VM `2`, the VM `3`, the VM `4`, and 3 more"
        )
    );
}

/// A refused archive that hipc cannot say more about keeps the server's words: a 409 with
/// no resource hipc can name, or another code, and any other status.
#[test]
fn projects_delete_keeps_the_servers_words_otherwise() {
    let path = format!("/api/accounts/projects/{PROJECT}/");
    let refused = |body: &str| {
        let server = Server::start(vec![route("DELETE", &path, Reply::json(409, body))]);
        hipc(&server, &projects(&["delete", PROJECT, "--yes"])).fails(1, "conflict")
    };
    // A 409 without a list of resources hipc can name, or with another code, keeps the
    // server's words.
    for body in [
        r#"{"code":"project_not_empty"}"#,
        r#"{"code":"project_not_empty","resources":[]}"#,
        r#"{"code":"project_not_empty","resources":[{"id":"x"}]}"#,
        r#"{"resources":[{"kind":"vm","id":"vm-1"}]}"#,
        r#"{"code":"other","resources":[{"kind":"vm","id":"vm-1"}]}"#,
    ] {
        let error = refused(body);
        assert_eq!(
            error["message"],
            format!(
                "cannot archive the project `{PROJECT}`, which must own nothing: the API \
                 answered 409"
            ),
            "{body}"
        );
        assert_eq!(error["hint"], Value::Null, "{body}");
    }
    // Another status is left as it is, a list of resources or not.
    let body = r#"{"code":"project_not_empty","resources":[{"kind":"vm","id":"vm-1"}]}"#;
    let server = Server::start(vec![route("DELETE", &path, Reply::json(404, body))]);
    let error = hipc(&server, &projects(&["delete", PROJECT, "--yes"])).fails(1, "not_found");
    assert!(
        !error["message"].as_str().unwrap().contains("still holds"),
        "{error}"
    );
}

/// The account a transfer is offered to in the tests: neither the token's own account
/// (`acct_ff...` in the accounts answer) nor the membership.
const TARGET: &str = "acct_eeeeeeeeeeeeeeeeeeee";

/// The projects answer: the Default project, then `ONE_PROJECT`.
fn projects_answer() -> String {
    format!(r#"{{"projects":[{{"id":null,"name":"Default","is_default":true}},{ONE_PROJECT}]}}"#)
}

/// Runs `args` (a transfer of `PROJECT`) against a server that answers the projects with
/// `projects` and the accounts with `accounts`: without `--yes` it must be refused with
/// `code`, naming `message`, having sent `reads` and nothing else; with `confirm`, it is
/// then run with `--yes` and must send the reads again and the offer.
fn offered(
    args: &[&str],
    (projects, accounts): (&str, &str),
    reads: &[&str],
    (code, message): (&str, &str),
    confirm: bool,
) {
    let path = format!("/api/accounts/projects/{PROJECT}/transfer/");
    let server = Server::start(vec![
        route_of(&account::PROJECTS, [], Reply::json(200, projects)),
        route_of(&account::ACCOUNTS, [], Reply::json(200, accounts)),
        route("POST", &path, Reply::json(201, TRANSFER)),
    ]);
    let argv = if code == "confirmation_required" {
        args.to_vec()
    } else {
        with_yes(args)
    };
    // `usage` and the confirmation are the command line's; the rest are what the server
    // answered or listed (tenet 4).
    let exit = if matches!(code, "usage" | "confirmation_required") {
        2
    } else {
        1
    };
    let error = hipc(&server, &argv).fails(exit, code);
    let expected = if code == "confirmation_required" {
        format!("this would {message}, and `--yes` was not passed")
    } else {
        message.to_owned()
    };
    assert_eq!(error["message"], expected, "{args:?}");
    let sent: Vec<String> = server.requests().iter().map(|r| r.target.clone()).collect();
    assert_eq!(sent, reads, "{args:?}");
    if confirm {
        assert_eq!(hipc(&server, &with_yes(args)).ok(), parsed(TRANSFER));
        let requests = server.requests();
        let offer = requests.last().expect("the offer");
        assert_eq!(
            (offer.method.as_str(), offer.target.as_str()),
            ("POST", path.as_str())
        );
        assert_eq!(offer.json(), json!({"to_account": TARGET}));
        assert_eq!(requests.len(), reads.len() * 2 + 1);
    }
}

/// The offer reads the projects first, so the question names the project, and, from the
/// token's own account, the accounts, so it names that account's id too; a membership is
/// named as every write names it. The body is the console's `{to_account}`.
#[test]
fn projects_transfer_names_the_project_and_both_accounts() {
    let args = projects(&["transfer", PROJECT, "--to-account", TARGET]);
    let both = ["/api/accounts/projects/", "/api/accounts/"];
    offered(
        &args,
        (&projects_answer(), ACCOUNTS),
        &both,
        (
            "confirmation_required",
            &format!(
                "offer the project `web` (`{PROJECT}`) to the account `{TARGET}` from the token's \
             own account `acct_ffffffffffffffffffff`"
            ),
        ),
        true,
    );
    let mut member = vec!["--account", MEMBER];
    member.extend(&args);
    offered(
        &member,
        (&projects_answer(), ACCOUNTS),
        &["/api/accounts/projects/"],
        (
            "confirmation_required",
            &format!(
                "offer the project `web` (`{PROJECT}`) to the account `{TARGET}` in the account \
             `{MEMBER}`"
            ),
        ),
        true,
    );
    // A project without a name is named by its id.
    let unnamed = format!(r#"{{"projects":[{{"id":"{PROJECT}"}}]}}"#);
    offered(
        &args,
        (&unnamed, ACCOUNTS),
        &both,
        (
            "confirmation_required",
            &format!(
                "offer the project `{PROJECT}` to the account `{TARGET}` from the token's own \
             account `acct_ffffffffffffffffffff`"
            ),
        ),
        false,
    );
    // The own account is the one marked `is_owner`, wherever it is listed: a membership
    // listed first is not the source.
    offered(
        &args,
        (&projects_answer(), MEMBER_FIRST),
        &both,
        (
            "confirmation_required",
            &format!(
                "offer the project `web` (`{PROJECT}`) to the account `{TARGET}` from the token's \
             own account `acct_ffffffffffffffffffff`"
            ),
        ),
        true,
    );
}

/// The accounts answer with a membership listed before the token's own account.
const MEMBER_FIRST: &str = r#"{"accounts":[
    {"id":"acct_0123456789abcdef0123","is_owner":false,"membership_id":7},
    {"id":"acct_ffffffffffffffffffff","is_owner":true,"membership_id":null}
]}"#;

/// Without one account marked `is_owner: true` with an `id`, the offer's source is unknown:
/// it is `internal` after the two reads, and nothing is offered or asked.
#[test]
fn projects_transfer_refuses_an_accounts_answer_without_one_own_account() {
    let args = projects(&["transfer", PROJECT, "--to-account", TARGET]);
    let none = "the API's accounts list does not mark exactly one account as the token's own \
                with `is_owner`";
    // Read as `hipc account list` reads it: an answer without an `accounts` list says so.
    for accounts in ["{}", r#"{"accounts":{}}"#, "[]"] {
        offered(
            &args,
            (&projects_answer(), accounts),
            &["/api/accounts/projects/", "/api/accounts/"],
            (
                "internal",
                "the API answered `GET /api/accounts/` without an `accounts` list",
            ),
            false,
        );
    }
    for accounts in [
        r#"{"accounts":[]}"#,
        // The first is no longer taken for the own account.
        r#"{"accounts":[{"id":"acct_ffffffffffffffffffff"}]}"#,
        r#"{"accounts":[{"id":"acct_ffffffffffffffffffff","is_owner":"true"}]}"#,
        r#"{"accounts":[{"id":"acct_ffffffffffffffffffff","is_owner":false}]}"#,
        r#"{"accounts":[{"id":"acct_ffffffffffffffffffff","is_owner":true},
            {"id":"acct_0123456789abcdef0123","is_owner":true}]}"#,
    ] {
        offered(
            &args,
            (&projects_answer(), accounts),
            &["/api/accounts/projects/", "/api/accounts/"],
            ("internal", none),
            false,
        );
    }
    for accounts in [
        r#"{"accounts":[{"is_owner":true}]}"#,
        r#"{"accounts":[{"id":7,"is_owner":true}]}"#,
    ] {
        offered(
            &args,
            (&projects_answer(), accounts),
            &["/api/accounts/projects/", "/api/accounts/"],
            (
                "internal",
                "the account the API marks as the token's own has no `id`",
            ),
            false,
        );
    }
}

/// An offer to the account the project is in is refused: for a membership before anything
/// is sent, and for the token's own account once the accounts read gives its id. A project
/// the account does not list is refused after the projects read. Nothing is offered.
#[test]
fn projects_transfer_refuses_the_same_account_and_an_unknown_project() {
    let same = |account: &str| format!("`--to-account {account}` is the account the project is in");
    let mut member = vec!["--account", MEMBER];
    member.extend(projects(&["transfer", PROJECT, "--to-account", MEMBER]));
    offered(
        &member,
        (&projects_answer(), ACCOUNTS),
        &[],
        ("usage", &same(MEMBER)),
        false,
    );
    let own = OWN_ACCOUNT;
    for accounts in [ACCOUNTS, MEMBER_FIRST] {
        offered(
            &projects(&["transfer", PROJECT, "--to-account", own]),
            (&projects_answer(), accounts),
            &["/api/accounts/projects/", "/api/accounts/"],
            ("conflict", &same(own)),
            false,
        );
    }
    // The membership listed first is not where the project is, so an offer to it is asked
    // about, from the own account.
    offered(
        &projects(&["transfer", PROJECT, "--to-account", MEMBER]),
        (&projects_answer(), MEMBER_FIRST),
        &["/api/accounts/projects/", "/api/accounts/"],
        (
            "confirmation_required",
            &format!(
                "offer the project `web` (`{PROJECT}`) to the account `{MEMBER}` from the token's \
             own account `acct_ffffffffffffffffffff`"
            ),
        ),
        false,
    );
    let other = r#"{"projects":[{"id":"proj_1111111111aaaaaaaaaa","name":"api"}]}"#;
    offered(
        &projects(&["transfer", PROJECT, "--to-account", TARGET]),
        (other, ACCOUNTS),
        &["/api/accounts/projects/"],
        (
            "not_found",
            &format!(
                "the project `{PROJECT}` is not one of the projects of the token's own account"
            ),
        ),
        false,
    );
    // A projects answer without the list is `internal`, and nothing is offered.
    offered(
        &projects(&["transfer", PROJECT, "--to-account", TARGET]),
        ("{}", ACCOUNTS),
        &["/api/accounts/projects/"],
        (
            "internal",
            "cannot list the projects: the API answered without a `projects` list",
        ),
        false,
    );
    let mut member = vec!["--account", MEMBER];
    member.extend(projects(&["transfer", PROJECT, "--to-account", TARGET]));
    offered(
        &member,
        (other, ACCOUNTS),
        &["/api/accounts/projects/"],
        (
            "not_found",
            &format!(
                "the project `{PROJECT}` is not one of the projects of the account `{MEMBER}`"
            ),
        ),
        false,
    );
}

/// From the token's own account the dry run of an offer lists both reads before it, and
/// sends nothing.
#[test]
fn projects_transfer_dry_run_from_the_own_account_lists_both_reads() {
    let origin = no_server();
    let mut argv = vec!["--dry-run", "--api-url", origin];
    argv.extend(projects(&["transfer", PROJECT, "--to-account", TARGET]));
    let doc = Hipc::new(&Home::new())
        .env_remove("HIPPIUS_TOKEN")
        .run(&argv)
        .ok();
    let urls: Vec<&str> = doc["requests"]
        .as_array()
        .expect("requests")
        .iter()
        .map(|request| {
            request["url"]
                .as_str()
                .expect("each recorded request has a url")
        })
        .collect();
    assert_eq!(
        urls,
        [
            format!("{origin}/api/accounts/projects/"),
            format!("{origin}/api/accounts/"),
            format!("{origin}/api/accounts/projects/{PROJECT}/transfer/"),
        ]
    );
}

#[test]
fn budget_and_quota_get_print_the_answer_as_sent() {
    let budget = format!("/api/accounts/projects/{PROJECT}/budget/");
    let quota = format!("/api/accounts/projects/{PROJECT}/quota/");
    let server = Server::start(vec![
        route("GET", &budget, Reply::json(200, BUDGET)),
        route("GET", &quota, Reply::json(200, QUOTA)),
    ]);
    assert_eq!(
        hipc(&server, &projects(&["budget", "get", PROJECT])).ok(),
        parsed(BUDGET)
    );
    assert_eq!(
        hipc(&server, &projects(&["quota", "get", PROJECT])).ok(),
        parsed(QUOTA)
    );
    let targets: Vec<String> = server.requests().into_iter().map(|r| r.target).collect();
    assert_eq!(targets, [budget.clone(), quota]);
    // A 404 names the list that shows the project ids.
    let server = Server::start(vec![route("GET", &budget, Reply::not_found())]);
    let error = hipc(&server, &projects(&["budget", "get", PROJECT])).fails(1, "not_found");
    assert_eq!(
        error["hint"],
        "run `hipc account projects list` to see what exists"
    );
}

/// The budget PUT replaces the whole budget, so every field is given and sent: the limit in
/// USD to the cent, the thresholds sorted, as the console sends them, and the launch block.
#[test]
fn budget_update_sends_the_whole_budget_with_yes() {
    let path = format!("/api/accounts/projects/{PROJECT}/budget/");
    let args = projects(&[
        "budget",
        "update",
        PROJECT,
        "--monthly-limit",
        "100.5",
        "--alert-at",
        "90",
        "--alert-at",
        "50",
        "--alert-at",
        "100",
        "--block-launches",
        "true",
    ]);
    confirmed(
        vec![route("PUT", &path, Reply::json(200, BUDGET))],
        &args,
        &format!(
            "set the monthly budget of the project `{PROJECT}` to `100.50` USD, mailing at \
             `50`, `90`, and `100` percent of it and refusing new launches over it"
        ),
        (
            "PUT",
            &path,
            json!({
                "monthly_limit": "100.50",
                "thresholds": [50, 90, 100],
                "block_launch_over": true,
            }),
        ),
        &parsed(BUDGET),
    );
    confirmed(
        vec![route("PUT", &path, Reply::json(200, BUDGET))],
        &projects(&[
            "budget",
            "update",
            PROJECT,
            "--monthly-limit",
            "1",
            "--alert-at",
            "1",
            "--block-launches",
            "false",
        ]),
        &format!(
            "set the monthly budget of the project `{PROJECT}` to `1.00` USD, mailing at `1` \
             percent of it and allowing new launches over it"
        ),
        (
            "PUT",
            &path,
            json!({"monthly_limit": "1.00", "thresholds": [1], "block_launch_over": false}),
        ),
        &parsed(BUDGET),
    );
}

/// `cli::projects::tests` holds the console's rules on a budget; this pins the `usage` a
/// validator's and a missing flag's refusal become.
#[test]
fn budget_update_refuses_what_the_console_refuses() {
    let server = Server::start(vec![]);
    for (flags, says) in [
        (
            "--monthly-limit 0 --alert-at 50 --block-launches true",
            "invalid value `0` for `--monthly-limit <USD>`: a monthly limit is more than `0`",
        ),
        (
            "--monthly-limit 1 --alert-at 50",
            "the following required arguments were not provided: `--block-launches <BOOL>`",
        ),
    ] {
        let args = budget_update_args(flags);
        let error = hipc(&server, &with_yes(&args)).fails(2, "usage");
        assert_eq!(error["message"], says, "{args:?}");
    }
    assert!(server.requests().is_empty());
}

/// `hipc account projects budget update <PROJECT>` and `flags`, split on spaces.
fn budget_update_args(flags: &str) -> Vec<&str> {
    let mut argv = vec!["account", "projects", "budget", "update", PROJECT];
    argv.extend(flags.split(' '));
    argv
}

/// The console's own rule on the thresholds: at most five, each once, sent sorted.
#[test]
fn budget_update_takes_at_most_five_distinct_thresholds() {
    let server = Server::start(vec![]);
    let six = budget_update_args(
        "--monthly-limit 1 --alert-at 10 --alert-at 20 --alert-at 30 --alert-at 40 \
         --alert-at 50 --alert-at 60 --block-launches true",
    );
    let error = hipc(&server, &with_yes(&six)).fails(2, "usage");
    assert_eq!(
        error["message"],
        "`--alert-at` is given 6 times, and a budget takes at most 5"
    );
    assert_eq!(error["hint"], "pass `--alert-at` at most 5 times");
    let twice = budget_update_args(
        "--monthly-limit 1 --alert-at 90 --alert-at 50 --alert-at 90 --block-launches true",
    );
    let error = hipc(&server, &with_yes(&twice)).fails(2, "usage");
    assert_eq!(error["message"], "`--alert-at 90` is given more than once");
    assert_eq!(error["hint"], "pass each `--alert-at` value once");
    assert!(server.requests().is_empty());
    // Five distinct thresholds are the most, and fit.
    let mut five = vec!["--dry-run"];
    five.extend(budget_update_args(
        "--monthly-limit 1 --alert-at 5 --alert-at 4 --alert-at 3 --alert-at 2 --alert-at 1 \
         --block-launches true",
    ));
    let doc = Hipc::new(&Home::new()).run(&five).ok();
    assert_eq!(draft(&doc)["body"]["thresholds"], json!([1, 2, 3, 4, 5]));
}

#[test]
fn budget_delete_removes_it_with_yes() {
    let path = format!("/api/accounts/projects/{PROJECT}/budget/");
    confirmed(
        vec![route("DELETE", &path, Reply::json(204, ""))],
        &projects(&["budget", "delete", PROJECT]),
        &format!(
            "remove the monthly budget of the project `{PROJECT}`, with its alerts and any \
             launch block"
        ),
        ("DELETE", &path, Value::Null),
        &json!({"project": PROJECT, "deleted": true}),
    );
}

/// The quota PATCH sends only the limits given, under `limits`, with the console's keys in
/// its order; `none` is `null`, the account's limit only.
#[test]
fn quota_update_sends_only_the_limits_given_with_yes() {
    let path = format!("/api/accounts/projects/{PROJECT}/quota/");
    confirmed(
        vec![route("PATCH", &path, Reply::json(200, QUOTA))],
        &projects(&["quota", "update", PROJECT, "--memory", "none", "--vms", "5"]),
        &format!(
            "set the compute limits of the project `{PROJECT}`: `vms` to `5` and `memory_gb` \
             to none of its own"
        ),
        (
            "PATCH",
            &path,
            json!({"limits": {"vms": 5, "memory_gb": null}}),
        ),
        &parsed(QUOTA),
    );
    let every = projects(&[
        "quota",
        "update",
        PROJECT,
        "--runners",
        "8",
        "--public-ips",
        "7",
        "--disk",
        "6",
        "--memory",
        "5",
        "--vcpus",
        "4",
        "--clusters",
        "3",
        "--databases",
        "2",
        "--vms",
        "0",
    ]);
    let server = Server::start(vec![route("PATCH", &path, Reply::json(200, QUOTA))]);
    hipc(&server, &with_yes(&every)).ok();
    let sent = server.only_request();
    assert_eq!(
        String::from_utf8(sent.body).unwrap(),
        concat!(
            r#"{"limits":{"vms":0,"databases":2,"clusters":3,"vcpus":4,"memory_gb":5,"#,
            r#""disk_gb":6,"public_ips":7,"runners":8}}"#
        )
    );
}

#[test]
fn quota_update_needs_a_limit_and_reads_each_as_a_count_or_none() {
    let server = Server::start(vec![]);
    let error = hipc(&server, &projects(&["quota", "update", PROJECT, "--yes"])).fails(2, "usage");
    assert!(
        error["message"].as_str().unwrap().contains("--vms"),
        "{error}"
    );
    // `cli::projects::tests` holds every refused limit.
    let error = hipc(
        &server,
        &projects(&["quota", "update", PROJECT, "--vcpus", "1.5", "--yes"]),
    )
    .fails(2, "usage");
    assert_eq!(
        error["message"],
        "invalid value `1.5` for `--vcpus <COUNT>`: a limit is a whole number, such as `5`, or \
         `none`"
    );
    assert!(server.requests().is_empty());
    let doc = Hipc::new(&Home::new())
        .run(&[
            "--dry-run",
            "account",
            "projects",
            "quota",
            "update",
            PROJECT,
            "--vms",
            "2147483647",
        ])
        .ok();
    assert_eq!(
        draft(&doc)["body"],
        json!({"limits": {"vms": 2_147_483_647}})
    );
}

/// The transfers answer `{incoming, outgoing}`: one envelope, the incoming ones first, each
/// with `hipc`'s own `direction`, and any other field beside the items; an item that is not
/// an object is kept as sent. The route is personal, so a membership sends no header.
#[test]
fn project_transfers_list_is_one_envelope_and_personal() {
    let outgoing = OUTGOING;
    let answer = format!(r#"{{"incoming":[{TRANSFER}],"outgoing":[{outgoing},7],"count":2}}"#);
    let server = Server::start(vec![route_of(
        &account::PROJECT_TRANSFERS,
        [],
        Reply::json(200, &answer),
    )]);
    let doc = hipc(&server, &transfers(&["list"])).ok();
    let mut incoming = parsed(TRANSFER);
    incoming["direction"] = json!("incoming");
    let mut sent = parsed(outgoing);
    sent["direction"] = json!("outgoing");
    assert_eq!(
        doc,
        json!({
            "items": [incoming, sent, 7],
            "count": 2,
            "next_page_token": null,
        })
    );
    let mut member = vec!["--account", MEMBER, "--project", PROJECT];
    member.extend(transfers(&["list"]));
    hipc(&server, &member).ok();
    for request in server.requests() {
        assert_eq!(request.header("x-hippius-account"), None);
        assert_eq!(request.header("x-hippius-project"), None);
    }
    // A `direction` the API sends is replaced by hipc's, which the list the transfer was in
    // decides.
    let sent_direction = Server::start(vec![route_of(
        &account::PROJECT_TRANSFERS,
        [],
        Reply::json(
            200,
            r#"{"incoming":[{"id":1,"direction":"outgoing"}],"outgoing":[]}"#,
        ),
    )]);
    assert_eq!(
        hipc(&sent_direction, &transfers(&["list"])).ok()["items"],
        json!([{"id": 1, "direction": "incoming"}])
    );
    let empty = Server::start(vec![route_of(
        &account::PROJECT_TRANSFERS,
        [],
        Reply::json(200, r#"{"incoming":[],"outgoing":[]}"#),
    )]);
    assert_eq!(
        hipc(&empty, &transfers(&["list"])).ok(),
        json!({"items": [], "next_page_token": null})
    );
}

/// Text mode shows each transfer's project and accounts by name and id, not as JSON
/// objects; one that names neither is shown as sent.
#[test]
fn project_transfers_list_names_the_project_and_accounts_in_text() {
    let odd = r#"{"id":8,"project":{"x":1},"from_account":null,"to_account":"acct_x"}"#;
    let answer = format!(r#"{{"incoming":[{TRANSFER}],"outgoing":[{odd}]}}"#);
    let server = Server::start(vec![route_of(
        &account::PROJECT_TRANSFERS,
        [],
        Reply::json(200, &answer),
    )]);
    let run = hipc(
        &server,
        &["--output", "text", "account", "project-transfers", "list"],
    );
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    let lines: Vec<&str> = run.stdout().lines().collect();
    assert_eq!(lines.len(), 3, "{}", run.stdout());
    let row = lines[1];
    for shown in [
        "web (proj_0a1b2c3d4e5f60718293)",
        "Ana (acct_0123456789abcdef0123)",
        "Bo (acct_ffffffffffffffffffff)",
        "incoming",
    ] {
        assert!(row.contains(shown), "{shown} in {row}");
    }
    assert!(!row.contains('{') && !row.contains('`'), "{row}");
    let odd_row = lines[2];
    assert!(odd_row.contains(r#"{"x":1}"#), "{odd_row}");
    assert!(odd_row.contains("acct_x"), "{odd_row}");
    assert!(odd_row.contains("outgoing"), "{odd_row}");
}

/// Fail closed: an answer without both lists is `internal`, never a list cut short.
#[test]
fn project_transfers_list_refuses_another_shape() {
    for answer in [
        r#"{"incoming":[]}"#,
        r#"{"outgoing":[]}"#,
        r#"{"incoming":{},"outgoing":[]}"#,
        r#"{"incoming":[],"outgoing":null}"#,
        "[]",
    ] {
        let server = Server::start(vec![route_of(
            &account::PROJECT_TRANSFERS,
            [],
            Reply::json(200, answer),
        )]);
        let error = hipc(&server, &transfers(&["list"])).fails(1, "internal");
        assert_eq!(
            error["message"],
            "cannot list the project transfers: the API answered without `incoming` and \
             `outgoing` lists",
            "{answer}"
        );
    }
}

/// The transfers answer with `TRANSFER` (id 5) offered to the token's own account and
/// `OUTGOING` (id 6) offered by it.
fn transfers_answer() -> String {
    format!(r#"{{"incoming":[{TRANSFER}],"outgoing":[{OUTGOING}]}}"#)
}

const OUTGOING: &str = concat!(
    r#"{"id":6,"project":{"id":"proj_1111111111aaaaaaaaaa","name":"api"},"#,
    r#""from_account":{"id":"acct_0123456789abcdef0123","name":"Ana"},"#,
    r#""to_account":{"id":"acct_eeeeeeeeeeeeeeeeeeee","name":"Cy"},"status":"pending","#,
    r#""created_at":"2026-11-01T10:00:00Z","expires_at":"2026-11-08T10:00:00Z","#,
    r#""decided_at":null}"#
);

/// Asserts that a decision on a transfer reads the transfers, then refuses without `--yes`
/// naming `action`, sending nothing else; and once confirmed reads them again and sends
/// `method path` with `body` (`Value::Null` for none), printing `printed`. No request takes
/// a header: the routes are personal.
fn decided(
    write: crate::common::Route,
    args: &[&str],
    action: &str,
    (method, path, body): (&str, &str, Value),
    printed: &Value,
) {
    let list = "/api/accounts/project-transfers/";
    let server = Server::start(vec![
        route("GET", list, Reply::json(200, &transfers_answer())),
        write,
    ]);
    let error = hipc(&server, args).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!("this would {action}, and `--yes` was not passed"),
        "{args:?}"
    );
    let sent: Vec<String> = server.requests().iter().map(|r| r.target.clone()).collect();
    assert_eq!(sent, [list], "{args:?}");
    assert_eq!(&hipc(&server, &with_yes(args)).ok(), printed, "{args:?}");
    let requests = server.requests();
    assert_eq!(requests.len(), 3, "{args:?}");
    assert_eq!(
        (requests[1].method.as_str(), requests[1].target.as_str()),
        ("GET", list)
    );
    let last = &requests[2];
    assert_eq!((last.method.as_str(), last.target.as_str()), (method, path));
    if body.is_null() {
        assert!(last.body.is_empty(), "{args:?}: {:?}", last.body);
    } else {
        assert_eq!(last.json(), body, "{args:?}");
    }
    for request in &requests {
        assert_eq!(request.header("x-hippius-account"), None);
        assert_eq!(request.header("x-hippius-project"), None);
        assert_eq!(request.header("x-hippius-billing"), None);
    }
}

/// An accept moves the project into the token's own account, whichever account is
/// selected, so the question says so and names the project and the account offering it;
/// the body names the billing account, `null` for the Default one, as the console sends it.
#[test]
fn project_transfers_accept_names_the_project_and_both_accounts() {
    let path = "/api/accounts/project-transfers/5/accept/";
    let answer = format!(r#"{{"project":{ONE_PROJECT}}}"#);
    let named = "`5` of the project `web` (`proj_0a1b2c3d4e5f60718293`) from the account `Ana` \
                 (`acct_0123456789abcdef0123`)";
    let default = format!(
        "accept the project transfer {named} into the token's own account, billed to the \
         Default billing account"
    );
    decided(
        route("POST", path, Reply::json(201, &answer)),
        &transfers(&["accept", "5"]),
        &default,
        ("POST", path, json!({"billing": null})),
        &parsed(&answer),
    );
    decided(
        route("POST", path, Reply::json(201, &answer)),
        &transfers(&["accept", "5", "--bill-to", "default"]),
        &default,
        ("POST", path, json!({"billing": null})),
        &parsed(&answer),
    );
    let mut member = vec!["--account", MEMBER];
    member.extend(transfers(&["accept", "5", "--bill-to", BILLING]));
    decided(
        route("POST", path, Reply::json(201, &answer)),
        &member,
        &format!(
            "accept the project transfer {named} into the token's own account, billed to the \
             billing account `{BILLING}`"
        ),
        ("POST", path, json!({"billing": BILLING})),
        &parsed(&answer),
    );
}

#[test]
fn project_transfers_decline_and_cancel_name_the_project_and_need_yes() {
    let decline = "/api/accounts/project-transfers/5/decline/";
    let declined = "decline the project transfer `5` of the project `web` \
                    (`proj_0a1b2c3d4e5f60718293`) from the account `Ana` \
                    (`acct_0123456789abcdef0123`)";
    decided(
        route("POST", decline, Reply::json(204, "")),
        &transfers(&["decline", "5"]),
        declined,
        ("POST", decline, Value::Null),
        &json!({"id": 5, "declined": true}),
    );
    let cancel = "/api/accounts/project-transfers/6/";
    decided(
        route("DELETE", cancel, Reply::json(204, "")),
        &transfers(&["cancel", "6"]),
        "cancel the project transfer `6` of the project `api` (`proj_1111111111aaaaaaaaaa`) to \
         the account `Cy` (`acct_eeeeeeeeeeeeeeeeeeee`)",
        ("DELETE", cancel, Value::Null),
        &json!({"id": 6, "cancelled": true}),
    );
    // A body, when the server sends one, is printed as sent.
    decided(
        route("POST", decline, Reply::json(201, TRANSFER)),
        &transfers(&["decline", "5"]),
        declined,
        ("POST", decline, Value::Null),
        &parsed(TRANSFER),
    );
}

/// A transfer that names its project or account only in part is named with what it has;
/// one that names neither is named by its id alone.
#[test]
fn a_transfer_is_named_with_what_the_list_gives() {
    let cases = [
        (
            concat!(
                r#"{"id":5,"project":{"id":"proj_0a1b2c3d4e5f60718293"},"#,
                r#""from_account":{"name":"Ana"}}"#
            ),
            "decline the project transfer `5` of the project `proj_0a1b2c3d4e5f60718293` from \
             the account `Ana`",
        ),
        (r#"{"id":5}"#, "decline the project transfer `5`"),
        (
            r#"{"id":5,"project":"web","from_account":null}"#,
            "decline the project transfer `5`",
        ),
    ];
    for (transfer, action) in cases {
        let answer = format!(r#"{{"incoming":[{transfer}],"outgoing":[]}}"#);
        let server = Server::start(vec![route_of(
            &account::PROJECT_TRANSFERS,
            [],
            Reply::json(200, &answer),
        )]);
        let error = hipc(&server, &transfers(&["decline", "5"])).fails(2, "confirmation_required");
        assert_eq!(
            error["message"],
            format!("this would {action}, and `--yes` was not passed"),
            "{transfer}"
        );
    }
}

/// An accept or a decline takes only a transfer offered to the token's own account, and a
/// cancel only one it offered: any other id is refused after the read, with nothing else
/// sent, even with `--yes`. A list of another shape is `internal`, also with nothing else
/// sent.
#[test]
fn a_transfer_not_on_its_side_is_refused_after_the_read() {
    let server = Server::start(vec![route_of(
        &account::PROJECT_TRANSFERS,
        [],
        Reply::json(200, &transfers_answer()),
    )]);
    let cases = [
        (
            "accept",
            "6",
            "is not one offered to the token's own account",
        ),
        (
            "decline",
            "6",
            "is not one offered to the token's own account",
        ),
        (
            "accept",
            "7",
            "is not one offered to the token's own account",
        ),
        ("cancel", "5", "is not one the token's own account offered"),
        ("cancel", "7", "is not one the token's own account offered"),
    ];
    for (verb, id, why) in cases {
        let error = hipc(&server, &transfers(&[verb, id, "--yes"])).fails(1, "not_found");
        assert_eq!(
            error["message"],
            format!("the project transfer `{id}` {why}"),
            "{verb} {id}"
        );
        assert_eq!(
            error["hint"],
            "run `hipc account project-transfers list` to see the transfer ids"
        );
        assert_eq!(error["side_effects"], "none");
    }
    let requests = server.requests();
    assert_eq!(requests.len(), cases.len());
    assert!(requests.iter().all(|r| r.method == "GET"));
    let broken = Server::start(vec![route_of(
        &account::PROJECT_TRANSFERS,
        [],
        Reply::json(200, r#"{"incoming":[]}"#),
    )]);
    hipc(&broken, &transfers(&["accept", "5", "--yes"])).fails(1, "internal");
    broken.only_request();
}

/// The project and billing-account routes are account-scoped: a membership sends the
/// account header, and the question names that account.
#[test]
fn a_membership_sends_the_account_header_and_is_named() {
    let path = format!("/api/accounts/projects/{PROJECT}/budget/");
    let server = Server::start(vec![
        route_of(&account::PROJECT_CREATE, [], Reply::json(201, ONE_PROJECT)),
        route("DELETE", &path, Reply::json(204, "")),
    ]);
    hipc(
        &server,
        &[
            "--account",
            MEMBER,
            "account",
            "projects",
            "create",
            "--name",
            "web",
        ],
    )
    .ok();
    let delete = [
        "--account",
        MEMBER,
        "account",
        "projects",
        "budget",
        "delete",
        PROJECT,
    ];
    let error = hipc(&server, &delete).fails(2, "confirmation_required");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains(&format!("in the account `{MEMBER}`")),
        "{error}"
    );
    hipc(&server, &with_yes(&delete)).ok();
    for request in server.requests() {
        assert_eq!(request.header("x-hippius-account"), Some(MEMBER));
    }
}

/// The accounts API takes neither a project nor a billing account (tenet 12): one given by
/// flag refuses every one of these routes, reads and dry runs included, while the personal
/// transfer routes take either and send neither. A project stored in the profile refuses a
/// create, which names no project, as it would otherwise act in the Default one.
#[test]
fn the_selectors_follow_the_route_classes() {
    let server = Server::start(vec![]);
    let scoped: [&[&str]; 9] = [
        &["account", "projects", "create", "--name", "web"],
        &["account", "projects", "update", PROJECT, "--name", "web"],
        &["account", "projects", "delete", PROJECT, "--yes"],
        &[
            "account",
            "projects",
            "transfer",
            PROJECT,
            "--to-account",
            OTHER,
            "--yes",
        ],
        &["account", "projects", "budget", "get", PROJECT],
        &[
            "account",
            "projects",
            "budget",
            "delete",
            PROJECT,
            "--dry-run",
        ],
        &[
            "account", "projects", "quota", "update", PROJECT, "--vms", "1", "--yes",
        ],
        &["account", "billing-accounts", "create", "--name", "ops"],
        &[
            "account",
            "billing-accounts",
            "update",
            BILLING,
            "--name",
            "ops",
        ],
    ];
    for args in scoped {
        let mut project = vec!["--project", PROJECT];
        project.extend(args);
        hipc(&server, &project).fails(2, "project_scope_unsupported");
        let mut billing = vec!["--billing", BILLING];
        billing.extend(args);
        hipc(&server, &billing).fails(2, "billing_scope_unsupported");
    }
    assert!(server.requests().is_empty());
    let home = Home::new();
    Hipc::new(&home)
        .run(&["account", "projects", "use", PROJECT])
        .ok();
    let error = Hipc::against(&home, &server)
        .run(&["account", "projects", "create", "--name", "web"])
        .fails(2, "project_scope_unsupported");
    assert_eq!(
        error["hint"],
        "pass `--project default` to act in the Default project"
    );
    assert!(server.requests().is_empty());
    let personal = Server::start(vec![
        route_of(
            &account::PROJECT_TRANSFERS,
            [],
            Reply::json(200, &transfers_answer()),
        ),
        route_of(
            &account::PROJECT_TRANSFER_DECLINE,
            [&5],
            Reply::json(204, ""),
        ),
    ]);
    hipc(
        &personal,
        &[
            "--billing",
            BILLING,
            "account",
            "project-transfers",
            "decline",
            "5",
            "--yes",
        ],
    )
    .ok();
    let sent = personal.requests();
    assert_eq!(sent.len(), 2);
    for request in sent {
        assert_eq!(request.header("x-hippius-billing"), None);
    }
}

/// A stored project is left off a read of these routes, which then reads as any read does,
/// with no project header; a stored billing account is left off a create, which takes none
/// (tenet 12).
#[test]
fn a_stored_selection_is_left_off_a_read_and_a_create() {
    let server = Server::start(vec![]);
    let home = Home::new();
    Hipc::new(&home)
        .run(&["account", "projects", "use", PROJECT])
        .ok();
    // A stored project is left off a read, which then reads the account's budget as any
    // read of these routes does, with no project header.
    let budget = format!("/api/accounts/projects/{PROJECT}/budget/");
    let reads = Server::start(vec![route("GET", &budget, Reply::json(200, BUDGET))]);
    let doc = Hipc::against(&home, &reads)
        .run(&["account", "projects", "budget", "get", PROJECT])
        .ok();
    assert_eq!(doc, parsed(BUDGET));
    assert_eq!(reads.only_request().header("x-hippius-project"), None);
    // A stored billing account is left off a create, which takes none.
    let billed = Home::new();
    Hipc::new(&billed)
        .run(&["account", "billing-accounts", "use", BILLING])
        .ok();
    let doc = Hipc::against(&billed, &server)
        .run(&[
            "account",
            "billing-accounts",
            "create",
            "--name",
            "ops",
            "--dry-run",
        ])
        .ok();
    assert_eq!(
        draft(&doc).get("billing_header"),
        Some(&serde_json::Value::Null),
        "{doc}"
    );
    assert!(server.requests().is_empty());
    // The create goes out, and no billing header is on the wire.
    let creates = Server::start(vec![route_of(
        &account::BILLING_ACCOUNT_CREATE,
        [],
        Reply::json(201, ONE_BILLING),
    )]);
    Hipc::against(&billed, &creates)
        .run(&["account", "billing-accounts", "create", "--name", "ops"])
        .ok();
    assert_eq!(creates.only_request().header("x-hippius-billing"), None);
}

/// A write to a route that names the project in its path acts on that project, so a stored
/// project is left off it and the write goes out with no project header (tenet 12): the
/// rename, the archive, the transfer, and the budget and quota writes. A stored billing
/// account is left off them too, since the path decides and the route takes no billing
/// account. The same project or billing account by flag still refuses each of them, sending
/// nothing.
#[test]
fn a_stored_project_is_left_off_a_write_that_names_the_project() {
    let home = Home::new();
    Hipc::new(&home)
        .run(&["account", "projects", "use", PROJECT])
        .ok();
    Hipc::new(&home)
        .run(&["account", "billing-accounts", "use", BILLING])
        .ok();
    let named = format!("/api/accounts/projects/{PROJECT}/");
    let budget = format!("{named}budget/");
    let quota = format!("{named}quota/");
    let transfer = format!("{named}transfer/");
    let server = Server::start(vec![
        route("PATCH", &named, Reply::json(200, ONE_PROJECT)),
        route("DELETE", &named, Reply::json(204, "")),
        route("PUT", &budget, Reply::json(200, BUDGET)),
        route("DELETE", &budget, Reply::json(204, "")),
        route("PATCH", &quota, Reply::json(200, QUOTA)),
        route_of(&account::PROJECTS, [], Reply::json(200, &projects_answer())),
        route_of(&account::ACCOUNTS, [], Reply::json(200, ACCOUNTS)),
        route("POST", &transfer, Reply::json(201, TRANSFER)),
    ]);
    let writes: [(&[&str], &str, &str); 6] = [
        (&["update", PROJECT, "--name", "web"], "PATCH", &named),
        (&["delete", PROJECT, "--yes"], "DELETE", &named),
        (
            &[
                "budget",
                "update",
                PROJECT,
                "--monthly-limit",
                "1",
                "--alert-at",
                "50",
                "--block-launches",
                "true",
                "--yes",
            ],
            "PUT",
            &budget,
        ),
        (&["budget", "delete", PROJECT, "--yes"], "DELETE", &budget),
        (
            &["quota", "update", PROJECT, "--vms", "5", "--yes"],
            "PATCH",
            &quota,
        ),
        (
            &["transfer", PROJECT, "--to-account", TARGET, "--yes"],
            "POST",
            &transfer,
        ),
    ];
    for (args, method, path) in writes {
        let argv = projects(args);
        Hipc::against(&home, &server).run(&argv).ok();
        let requests = server.requests();
        let write = requests.last().expect("the write");
        assert_eq!(
            (write.method.as_str(), write.target.as_str()),
            (method, path)
        );
        for request in &requests {
            assert_eq!(request.header("x-hippius-project"), None, "{argv:?}");
            assert_eq!(request.header("x-hippius-billing"), None, "{argv:?}");
        }
        let before = requests.len();
        let mut flagged = vec!["--project", PROJECT];
        flagged.extend(&argv);
        Hipc::against(&home, &server)
            .run(&flagged)
            .fails(2, "project_scope_unsupported");
        let mut billed = vec!["--billing", BILLING];
        billed.extend(&argv);
        Hipc::against(&home, &server)
            .run(&billed)
            .fails(2, "billing_scope_unsupported");
        assert_eq!(server.requests().len(), before, "{flagged:?}");
    }
}

/// A rename of a billing account names it in its path, so a stored billing account is left
/// off it and the rename goes out with no billing header (tenet 12), whichever billing
/// account the path names: the stored one or another. A stored project is left off it too,
/// as the route takes no project. The same billing account or project by flag or variable
/// still refuses it, sending nothing.
#[test]
fn a_stored_billing_account_is_left_off_a_rename_that_names_it() {
    let home = Home::new();
    Hipc::new(&home)
        .run(&["account", "billing-accounts", "use", BILLING])
        .ok();
    Hipc::new(&home)
        .run(&["account", "projects", "use", PROJECT])
        .ok();
    for id in [BILLING, SECOND_BILLING] {
        let path = format!("/api/accounts/billing-accounts/{id}/");
        let server = Server::start(vec![route("PATCH", &path, Reply::json(200, ONE_BILLING))]);
        let update = ["account", "billing-accounts", "update", id, "--name", "ops"];
        let doc = Hipc::against(&home, &server).run(&update).ok();
        assert_eq!(doc, parsed(ONE_BILLING));
        let sent = server.only_request();
        assert_eq!(
            (sent.method.as_str(), sent.target.as_str()),
            ("PATCH", &*path)
        );
        assert_eq!(sent.header("x-hippius-billing"), None);
        assert_eq!(sent.header("x-hippius-project"), None);
        assert_eq!(sent.json(), json!({"name": "ops"}));
        let mut flagged = vec!["--billing", BILLING];
        flagged.extend(update);
        Hipc::against(&home, &server)
            .run(&flagged)
            .fails(2, "billing_scope_unsupported");
        Hipc::against(&home, &server)
            .env("HIPPIUS_BILLING", BILLING)
            .run(&update)
            .fails(2, "billing_scope_unsupported");
        let mut projected = vec!["--project", PROJECT];
        projected.extend(update);
        Hipc::against(&home, &server)
            .run(&projected)
            .fails(2, "project_scope_unsupported");
        Hipc::against(&home, &server)
            .env("HIPPIUS_PROJECT", PROJECT)
            .run(&update)
            .fails(2, "project_scope_unsupported");
        assert_eq!(server.requests().len(), 1, "{id}");
    }
}

/// The path decides the project, not the stored one: with `PROJECT` stored, a write to
/// another project's quota goes out with no project header, to that project.
#[test]
fn a_stored_project_is_left_off_a_write_to_another_project() {
    let home = Home::new();
    Hipc::new(&home)
        .run(&["account", "projects", "use", PROJECT])
        .ok();
    let path = format!("/api/accounts/projects/{SECOND}/quota/");
    let server = Server::start(vec![route("PATCH", &path, Reply::json(200, QUOTA))]);
    Hipc::against(&home, &server)
        .run(&projects(&[
            "quota", "update", SECOND, "--vms", "5", "--yes",
        ]))
        .ok();
    let sent = server.only_request();
    assert_eq!(sent.target, path);
    assert_eq!(sent.header("x-hippius-project"), None);
}

/// Archiving the stored project clears the selection, as `projects use default` would, so
/// later commands do not act in a project that is gone; text mode says so in a note. JSON
/// mode clears it too, and prints the same document. Archiving another project, a dry run,
/// and a refused archive leave the selection as it was.
#[test]
fn archiving_the_stored_project_clears_the_selection() {
    let path = format!("/api/accounts/projects/{PROJECT}/");
    let stored = |home: &Home| {
        Hipc::new(home)
            .run(&["account", "projects", "use", PROJECT])
            .ok();
    };
    let selected = |home: &Home| home.config_text().contains(PROJECT);
    let server = Server::start(vec![route("DELETE", &path, Reply::json(204, ""))]);
    let home = Home::new();
    stored(&home);
    let run = Hipc::against(&home, &server).run(&[
        "--output", "text", "account", "projects", "delete", PROJECT, "--yes",
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    let note = format!(
        "note: the archived project `{PROJECT}` was the stored selection, so it is cleared: \
         commands now act in the Default project\n"
    );
    assert_eq!(run.stderr(), note);
    assert!(!selected(&home), "{}", home.config_text());
    let home = Home::new();
    stored(&home);
    let doc = Hipc::against(&home, &server)
        .run(&projects(&["delete", PROJECT, "--yes"]))
        .ok();
    assert_eq!(doc, json!({"id": PROJECT, "archived": true}));
    assert!(!selected(&home));
    // Another project, a dry run, and a refused archive keep it.
    let other = format!("/api/accounts/projects/{SECOND}/");
    let refused = r#"{"code":"project_not_empty","detail":"x","resources":[]}"#;
    let kept = Server::start(vec![
        route("DELETE", &other, Reply::json(204, "")),
        route("DELETE", &path, Reply::json(409, refused)),
    ]);
    let home = Home::new();
    stored(&home);
    Hipc::against(&home, &kept)
        .run(&projects(&["delete", SECOND, "--yes"]))
        .ok();
    Hipc::against(&home, &kept)
        .run(&projects(&["delete", PROJECT, "--yes", "--dry-run"]))
        .ok();
    Hipc::against(&home, &kept)
        .run(&projects(&["delete", PROJECT, "--yes"]))
        .fails(1, "conflict");
    assert!(selected(&home), "{}", home.config_text());
}

/// When the selection cannot be cleared, the archive still succeeded, so the command exits 0
/// and warns that the project is still selected, naming the command that clears it.
#[test]
fn a_selection_that_cannot_be_cleared_is_a_warning() {
    use std::os::unix::fs::PermissionsExt as _;
    if !common::permission_bits_apply() {
        return;
    }
    let path = format!("/api/accounts/projects/{PROJECT}/");
    let server = Server::start(vec![route("DELETE", &path, Reply::json(204, ""))]);
    let home = Home::new();
    Hipc::new(&home)
        .run(&["account", "projects", "use", PROJECT])
        .ok();
    let dir = home
        .config()
        .parent()
        .expect("the config directory")
        .to_owned();
    let mode = |bits| std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(bits));
    mode(0o500).expect("make the config directory read-only");
    let run = Hipc::against(&home, &server).run(&[
        "--output", "text", "account", "projects", "delete", PROJECT, "--yes",
    ]);
    mode(0o700).expect("restore the config directory");
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    let stderr = run.stderr();
    let warned = stderr.starts_with(&format!(
        "warning: the archived project `{PROJECT}` is still the stored selection, since "
    ));
    assert!(warned, "{stderr}");
    assert!(
        stderr.ends_with(": run `hipc account projects use default`\n"),
        "{stderr}"
    );
    assert!(home.config_text().contains(PROJECT));
}

/// A `projects use` stored while the archive was in flight is kept: the selection is compared
/// again under the file's lock, and only the archived project is cleared.
#[test]
fn archiving_keeps_a_selection_stored_meanwhile() {
    let path = format!("/api/accounts/projects/{PROJECT}/");
    let home = Home::new();
    Hipc::new(&home)
        .run(&["account", "projects", "use", PROJECT])
        .ok();
    let config = home.config();
    // Another run selects the second project after the first read the config.
    let reselect = move || {
        let text = std::fs::read_to_string(&config).expect("read the config");
        std::fs::write(&config, text.replace(PROJECT, SECOND)).expect("write the config");
    };
    let server = Server::start(vec![route(
        "DELETE",
        &path,
        Reply::json(204, "").before(reselect),
    )]);
    let run = Hipc::against(&home, &server).run(&[
        "--output", "text", "account", "projects", "delete", PROJECT, "--yes",
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(run.stderr(), "");
    let text = home.config_text();
    assert!(text.contains(SECOND) && !text.contains(PROJECT), "{text}");
}

/// Each write's dry run lists its requests, the reads that name its target first, needs no
/// token, and sends nothing; a membership shows on the scoped routes and never on the
/// personal ones.
#[test]
fn every_write_dry_run_lists_its_requests() {
    let project = format!("/api/accounts/projects/{PROJECT}/");
    let budget = "--monthly-limit 20 --alert-at 80 --block-launches false";
    let cases = [
        (
            "projects create --name web".to_owned(),
            "POST",
            "/api/accounts/projects/".to_owned(),
            json!({"name": "web", "billing": null}),
        ),
        (
            format!("projects update {PROJECT} --name web"),
            "PATCH",
            project.clone(),
            json!({"name": "web"}),
        ),
        (
            format!("projects delete {PROJECT}"),
            "DELETE",
            project.clone(),
            Value::Null,
        ),
        (
            format!("projects transfer {PROJECT} --to-account {OTHER}"),
            "POST",
            format!("{project}transfer/"),
            json!({"to_account": OTHER}),
        ),
        (
            format!("projects budget update {PROJECT} {budget}"),
            "PUT",
            format!("{project}budget/"),
            json!({"monthly_limit": "20.00", "thresholds": [80], "block_launch_over": false}),
        ),
        (
            format!("projects budget delete {PROJECT}"),
            "DELETE",
            format!("{project}budget/"),
            Value::Null,
        ),
        (
            format!("projects quota update {PROJECT} --disk none"),
            "PATCH",
            format!("{project}quota/"),
            json!({"limits": {"disk_gb": null}}),
        ),
        (
            "project-transfers list".to_owned(),
            "GET",
            "/api/accounts/project-transfers/".to_owned(),
            Value::Null,
        ),
        (
            "project-transfers accept 5".to_owned(),
            "POST",
            "/api/accounts/project-transfers/5/accept/".to_owned(),
            json!({"billing": null}),
        ),
        (
            "project-transfers decline 5".to_owned(),
            "POST",
            "/api/accounts/project-transfers/5/decline/".to_owned(),
            Value::Null,
        ),
        (
            "project-transfers cancel 5".to_owned(),
            "DELETE",
            "/api/accounts/project-transfers/5/".to_owned(),
            Value::Null,
        ),
        (
            "billing-accounts create --name ops".to_owned(),
            "POST",
            "/api/accounts/billing-accounts/".to_owned(),
            json!({"name": "ops"}),
        ),
        (
            format!("billing-accounts update {BILLING} --name ops"),
            "PATCH",
            format!("/api/accounts/billing-accounts/{BILLING}/"),
            json!({"name": "ops"}),
        ),
    ];
    let decisions = ["accept", "decline", "cancel"];
    for (words, method, path, body) in cases {
        let reads: &[&str] = if decisions.iter().any(|verb| words.contains(verb)) {
            &["/api/accounts/project-transfers/"]
        } else if words.contains("transfer ") {
            &["/api/accounts/projects/"]
        } else {
            &[]
        };
        assert_dry_run(&words, reads, method, &path, &body);
    }
}

/// Asserts that `hipc --account <MEMBER> account <words>` under `--dry-run`, with no token,
/// prints the request `method path` with `body`, after the `GET` of each of `reads` in
/// `requests`, and the account header on a scoped route only.
fn assert_dry_run(words: &str, reads: &[&str], method: &str, path: &str, body: &Value) {
    let origin = no_server();
    let mut argv = vec![
        "--dry-run",
        "--api-url",
        origin,
        "--account",
        MEMBER,
        "account",
    ];
    argv.extend(words.split(' '));
    let doc = Hipc::new(&Home::new())
        .env_remove("HIPPIUS_TOKEN")
        .run(&argv)
        .dry_ok();
    let describe = |method: &str, path: &str, body: &Value| {
        let personal = path.starts_with("/api/accounts/project-transfers/");
        json!({
            "method": method,
            "url": format!("{origin}{path}"),
            "account_header": if personal { Value::Null } else { json!(MEMBER) },
            "project_header": null,
            "billing_header": null,
            "body": body,
        })
    };
    let mut expected = one_request_dry_run(describe(method, path, body));
    let mut requests: Vec<Value> = reads
        .iter()
        .map(|read| describe("GET", read, &Value::Null))
        .collect();
    requests.push(describe(method, path, body));
    expected["requests"] = Value::Array(requests);
    assert_eq!(doc, expected, "{words}");
}
