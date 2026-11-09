//! The paging contract of every curated list (`TENETS.md` tenets 3 and 6): only a list the
//! server pages takes `--limit`, `--page-token`, and `--all`, and a list read whole refuses
//! a `next` link rather than print one page as the whole list.

use serde_json::{Value, json};

use crate::common::{Hipc, Home, MEMBER, Reply, Run, Server, route, route_each};

const ZONE: &str = "zab12cd34ef56";
const VPC: &str = "11111111-1111-1111-1111-111111111111";

/// Every list that prints the envelope and reads its route whole, because swagger
/// (2026-11-02) documents no page parameter for it: the arguments, the route, and the field
/// that holds the items (`results` for a DRF page or a bare array, a named field for a list
/// answered inside an object).
const READ_WHOLE: &[(&[&str], &str, &str)] = &[
    (&["account", "list"], "/api/accounts/", "accounts"),
    (
        &["account", "referrals", "list"],
        "/api/referral-codes/",
        "results",
    ),
    (
        &["account", "members", "list"],
        "/api/accounts/members/",
        "members",
    ),
    (
        &["account", "invites", "list"],
        "/api/accounts/invites/",
        "invites",
    ),
    (
        &["account", "projects", "list"],
        "/api/accounts/projects/",
        "projects",
    ),
    (
        &["account", "billing-accounts", "list"],
        "/api/accounts/billing-accounts/",
        "billing_accounts",
    ),
    (&["drive", "plans"], "/api/drive/plans/", "results"),
    (
        &["s3", "buckets", "list"],
        "/api/objectstore/buckets/",
        "buckets",
    ),
    (
        &["s3", "buckets", "acl", "list"],
        "/api/objectstore/acl/",
        "results",
    ),
    (&["hub", "plans"], "/api/registry/plans/", "results"),
    (
        &["hub", "subscription", "get"],
        "/api/registry/subscriptions/",
        "results",
    ),
    (&["hub", "keys", "list"], "/api/registry/keys/", "results"),
    (
        &["hub", "events", "list"],
        "/api/registry/events/",
        "results",
    ),
    (
        &["hub", "usage", "--repositories"],
        "/api/registry/usage/repositories/",
        "results",
    ),
    (
        &["hub", "repositories", "list"],
        "/api/registry/repositories/",
        "results",
    ),
    (
        &["hub", "artifacts", "list", "my-team/app"],
        "/api/registry/repositories/my-team/app/artifacts/",
        "results",
    ),
    (&["compute", "flavors"], "/api/compute/flavors/", "results"),
    (
        &["compute", "app-engine", "apps", "list"],
        "/api/compute/app-engine/apps/",
        "results",
    ),
    (&["compute", "apps"], "/api/compute/apps/", "results"),
    (&["compute", "regions"], "/api/compute/regions/", "results"),
    (&["compute", "vms", "list"], "/api/compute/vms/", "results"),
    (
        &["compute", "vms", "failovers", "vm_1"],
        "/api/compute/vms/vm_1/failovers/",
        "failovers",
    ),
    (
        &["compute", "exposures", "list", "vm_1"],
        "/api/compute/vms/vm_1/expose/",
        "results",
    ),
    (
        &["compute", "runners", "installations", "list"],
        "/api/runners/installations/",
        "results",
    ),
    (
        &["compute", "account-ssh-keys"],
        "/api/compute/account-ssh-keys/",
        "results",
    ),
    (
        &["compute", "databases", "list"],
        "/api/databases/",
        "results",
    ),
    (
        &["compute", "kubernetes", "clusters", "list"],
        "/api/kubernetes/clusters/",
        "results",
    ),
    (
        &["compute", "kubernetes", "clusters", "events", "1"],
        "/api/kubernetes/clusters/1/events/",
        "results",
    ),
    (
        &["compute", "kubernetes", "clusters", "refusals"],
        "/api/kubernetes/clusters/refusals/",
        "results",
    ),
    (
        &["compute", "kubernetes", "node-pools", "list", "1"],
        "/api/kubernetes/clusters/1/node-pools/",
        "results",
    ),
    (
        &["compute", "kubernetes", "operations", "list", "1"],
        "/api/kubernetes/clusters/1/operations/",
        "results",
    ),
    (&["cdn", "zones", "list"], "/api/cdn/zones/", "zones"),
    (
        &["cdn", "hostnames", "list", ZONE],
        "/api/cdn/zones/zab12cd34ef56/hostnames/",
        "hostnames",
    ),
    (
        &["cdn", "purges", "list", ZONE],
        "/api/cdn/zones/zab12cd34ef56/purges/",
        "purges",
    ),
    (
        &["cdn", "waf", "versions", "list", ZONE],
        "/api/cdn/zones/zab12cd34ef56/waf/versions/",
        "versions",
    ),
    (&["cdn", "lists", "list"], "/api/cdn/lists/", "lists"),
    (
        &["network", "peers", "list"],
        "/api/network/peers/",
        "results",
    ),
    (
        &["network", "groups", "list"],
        "/api/network/groups/",
        "results",
    ),
    (
        &["network", "policies", "list"],
        "/api/network/policies/",
        "results",
    ),
    (
        &["network", "vpcs", "list"],
        "/api/network/networks/",
        "results",
    ),
    (
        &["network", "vpcs", "resources", "list", VPC],
        "/api/network/networks/11111111-1111-1111-1111-111111111111/resources/",
        "results",
    ),
    (
        &["network", "vpcs", "routers", "list", VPC],
        "/api/network/networks/11111111-1111-1111-1111-111111111111/routers/",
        "results",
    ),
];

/// The `... list` commands that are neither paged nor in [`READ_WHOLE`], each with why.
const NOT_READ_WHOLE: &[(&str, &str)] = &[
    (
        "account project-transfers list",
        "two lists in one answer; `project_transfers_refuse_a_next_link`",
    ),
    (
        "s3 tokens list",
        "two routes merged, every page followed within the caps",
    ),
    (
        "compute backups list",
        "the server's object printed as sent, so a `next` link reaches stdout",
    ),
    (
        "compute kubernetes backups list",
        "the server's object printed as sent, so a `next` link reaches stdout",
    ),
];

/// The lists swagger pages, and so the only ones that take the page flags: by `page` and
/// `page_size` (`/billing/transactions/`, `/billing/invoices/`, `/drive/subscription/history/`,
/// `/s3/subscription/history/`, `/ssh-keys/`, `/support/tickets/`,
/// `/support/tickets/{id}/messages/`), by
/// `continuation_token` (`/objectstore/buckets/{name}/objects/`), by the `before` cursor at
/// a fixed size (`/runners/jobs/`, no `--limit`), and by `page` and `page_size` with the next
/// page worked out from `total` (`/models/` and a model's versions, `hub_models.rs`).
const PAGED: &[(&str, &[&str])] = &[
    (
        "billing transactions",
        &["--limit", "--page-token", "--all"],
    ),
    (
        "billing invoices list",
        &["--limit", "--page-token", "--all"],
    ),
    (
        "drive subscription history",
        &["--limit", "--page-token", "--all"],
    ),
    (
        "s3 subscription history",
        &["--limit", "--page-token", "--all"],
    ),
    (
        "s3 buckets objects list",
        &["--limit", "--page-token", "--all"],
    ),
    ("hub models list", &["--limit", "--page-token", "--all"]),
    ("hub models get", &["--limit", "--page-token", "--all"]),
    ("compute runners jobs", &["--page-token", "--all"]),
    (
        "compute ssh-keys list",
        &["--limit", "--page-token", "--all"],
    ),
    (
        "support tickets list",
        &["--limit", "--page-token", "--all"],
    ),
    (
        "support tickets messages list",
        &["--limit", "--page-token", "--all"],
    ),
];

/// The lists paged by `page` and `page_size`, with their routes: the ones `--page-token`
/// asks for a page number on.
const PAGED_BY_NUMBER: &[(&[&str], &str)] = &[
    (&["billing", "transactions"], "/api/billing/transactions/"),
    (&["billing", "invoices", "list"], "/api/billing/invoices/"),
    (
        &["drive", "subscription", "history"],
        "/api/drive/subscription/history/",
    ),
    (
        &["s3", "subscription", "history"],
        "/api/s3/subscription/history/",
    ),
    (&["compute", "ssh-keys", "list"], "/api/ssh-keys/"),
    (&["support", "tickets", "list"], "/api/support/tickets/"),
    (
        &["support", "tickets", "messages", "list", "42"],
        "/api/support/tickets/42/messages/",
    ),
];

fn run(server: &Server, args: &[&str]) -> Run {
    let mut argv = vec!["--json"];
    argv.extend_from_slice(args);
    Hipc::against(&Home::new(), server).run(&argv)
}

/// A page with a `next` link, under `key`, as each route shapes its list.
fn paged_body(route: &str, key: &str) -> String {
    json!({
        key: [{"id": 1}],
        "count": 2,
        "next": format!("{{base}}{route}?page=2"),
        "previous": null,
    })
    .to_string()
}

/// One page of a list read whole is never printed as the whole list: a `next` link is
/// `internal`, after one request, with stdout empty.
#[test]
fn every_list_read_whole_refuses_a_next_link() {
    for (args, path, key) in READ_WHOLE {
        let server = Server::start(vec![route(
            "GET",
            path,
            Reply::json(200, &paged_body(path, key)),
        )]);
        let run = run(&server, args);
        assert_eq!(run.stdout(), "", "{args:?}");
        let error = run.fails(1, "internal");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("the API split a list into pages"),
            "{args:?}: {error}"
        );
        // The hint names the route read, as `hipc api` takes it, so it runs as written.
        let hint = error["hint"].as_str().unwrap();
        assert_eq!(
            hint,
            format!(
                "run `hipc api GET {}` to see the answer and its `next` link",
                path.strip_prefix("/api").unwrap()
            ),
            "{args:?}"
        );
        assert_eq!(server.requests().len(), 1, "{args:?}");
        // The hint's command, run exactly as written, prints the answer with its `next`.
        let shown = run_hint(&server, hint);
        assert!(
            shown["next"].as_str().unwrap().contains("page=2"),
            "{args:?}: {shown}"
        );
    }
}

/// Runs the command in backticks in `hint`, word for word without the leading `hipc`,
/// against `server`, and returns what it printed.
fn run_hint(server: &Server, hint: &str) -> Value {
    let command = hint.split('`').nth(1).expect("a command in backticks");
    let argv: Vec<&str> = command.split_whitespace().collect();
    assert_eq!(argv[0], "hipc", "{hint}");
    Hipc::against(&Home::new(), server).run(&argv[1..]).ok()
}

/// Every `... list` in `hipc schema` that does not page is in [`READ_WHOLE`], or in
/// [`NOT_READ_WHOLE`] with its reason, so a new list read whole cannot miss these tests.
/// Only the `... list` commands: the schema does not say which commands print a list, so a
/// list under another name, such as `drive plans`, is in [`READ_WHOLE`] by hand.
#[test]
fn every_list_that_does_not_page_is_audited() {
    let schema = Hipc::new(&Home::new()).run(&["schema"]).ok();
    for command in schema["commands"].as_array().unwrap() {
        let path: Vec<&str> = command["path"]
            .as_array()
            .unwrap()
            .iter()
            .map(|part| part.as_str().unwrap())
            .collect();
        if path.last() != Some(&"list") || command["pagination"] == true {
            continue;
        }
        let read_whole = READ_WHOLE
            .iter()
            .any(|(args, _, _)| args.len() >= path.len() && args[..path.len()] == path[..]);
        let exempt = NOT_READ_WHOLE
            .iter()
            .any(|(exempt, _)| *exempt == path.join(" "));
        assert!(
            read_whole != exempt,
            "{path:?}: read whole {read_whole}, exempt {exempt}"
        );
    }
}

/// The last page of a list read whole is the whole list: printed as the envelope, its
/// `next` and `previous` links dropped (the null token says no more exist), its other fields
/// kept.
#[test]
fn every_list_read_whole_prints_a_last_page_as_the_envelope() {
    for (args, path, key) in READ_WHOLE {
        let body = json!({*key: [{"id": 1}], "count": 1, "next": null, "previous": null});
        let server = Server::start(vec![route(
            "GET",
            path,
            Reply::json(200, &body.to_string()),
        )]);
        let mut doc = run(&server, args).ok();
        // Only the envelope is checked: a command may add fields to an item, as `hub keys
        // list` adds a null `secret`.
        doc["items"][0] = json!({"id": doc["items"][0]["id"]});
        assert_eq!(
            doc,
            json!({"count": 1, "items": [{"id": 1}], "next_page_token": null}),
            "{args:?}"
        );
    }
}

/// A `next` that is not a link is not read as the last page.
#[test]
fn a_list_inside_an_object_refuses_a_next_that_is_not_a_link() {
    let server = Server::start(vec![route(
        "GET",
        "/api/cdn/zones/",
        Reply::json(200, r#"{"zones":[],"next":2}"#),
    )]);
    run(&server, &["cdn", "zones", "list"]).fails(1, "internal");
}

/// `hipc account get` finds the selected account in the list of accounts: one on the page
/// read is the answer whatever comes after it, and one missing from a page with a `next`
/// link is `internal`, not `not_a_member`, as it may be on a later page.
#[test]
fn account_get_reads_the_page_and_refuses_a_miss_on_a_page_split() {
    let accounts = |next: bool| {
        json!({
            "accounts": [
                {"id": "acct_ffffffffffffffffffff", "is_owner": true},
                {"id": MEMBER, "is_owner": false},
            ],
            "next": next.then_some("{base}/api/accounts/?page=2"),
        })
        .to_string()
    };
    let get = |body: &str, account: &str| {
        let server = Server::start(vec![route("GET", "/api/accounts/", Reply::json(200, body))]);
        let mut hipc = Hipc::against(&Home::new(), &server);
        if account != "self" {
            hipc = hipc.env("HIPPIUS_ACCOUNT", account);
        }
        hipc.run(&["--json", "account", "get"])
    };
    // On the page read: the account itself, and the token's own account, marked `is_owner`.
    assert_eq!(
        get(&accounts(true), MEMBER).ok(),
        json!({"id": MEMBER, "is_owner": false, "selected_project": null,
               "selected_project_source": null, "selected_billing": null,
               "selected_billing_source": null})
    );
    let own = |id: &str| {
        json!({"id": id, "is_owner": true, "selected_project": null,
               "selected_project_source": null, "selected_billing": null,
               "selected_billing_source": null})
    };
    assert_eq!(
        get(&accounts(true), "self").ok(),
        own("acct_ffffffffffffffffffff")
    );
    let missing = "acct_99999999999999999999";
    let error = get(&accounts(true), missing).fails(1, "internal");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("the API split a list into pages"),
        "{error}"
    );
    assert_eq!(
        error["hint"],
        "run `hipc api GET /accounts/` to see the answer and its `next` link"
    );
    get(&accounts(false), missing).fails(1, "not_a_member");
}

/// `hipc account members update` reads the members to guard the grants it would drop: a
/// member on the page read is guarded as on a whole list, and one missing from a page with a
/// `next` link is `internal`, not `not_found`, with no PATCH.
#[test]
fn members_update_reads_the_page_and_refuses_a_miss_on_a_page_split() {
    let members = json!({
        "owner": {"id": "u1"},
        "members": [{"id": 7, "grants": [{"scope": "vms", "level": "viewer"}]}],
        "next": "{base}/api/accounts/members/?page=2",
    })
    .to_string();
    let update = |id: &str| {
        let server = Server::start(vec![
            route("GET", "/api/accounts/members/", Reply::json(200, &members)),
            route(
                "PATCH",
                &format!("/api/accounts/members/{id}/"),
                Reply::json(200, r#"{"id":7}"#),
            ),
        ]);
        let argv = [
            "--json",
            "account",
            "members",
            "update",
            id,
            "--grant",
            "vms=viewer",
            "--yes",
        ];
        let run = Hipc::against(&Home::new(), &server).run(&argv);
        let methods: Vec<String> = server.requests().into_iter().map(|r| r.method).collect();
        (run, methods)
    };
    let (run, methods) = update("7");
    run.ok();
    assert_eq!(methods, ["GET", "PATCH"]);
    let (run, methods) = update("8");
    let error = run.fails(1, "internal");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("the API split a list into pages"),
        "{error}"
    );
    assert_eq!(
        error["hint"],
        "run `hipc api GET /accounts/members/` to see the answer and its `next` link"
    );
    assert_eq!(methods, ["GET"]);
}

/// A list read whole takes none of the page flags, so none of them can suggest it pages.
#[test]
fn no_list_read_whole_takes_a_page_flag() {
    let home = Home::new();
    for (args, _, _) in READ_WHOLE {
        for flag in [&["--limit", "2"][..], &["--page-token", "2"], &["--all"]] {
            let mut argv = vec!["--json"];
            argv.extend_from_slice(args);
            argv.extend_from_slice(flag);
            Hipc::new(&home).run(&argv).fails(2, "usage");
        }
    }
}

/// `hipc schema` names exactly the lists the server pages as paginated, each with the flags
/// its server takes, so a flag added or dropped goes through this audit again.
#[test]
fn the_lists_that_page_are_the_ones_the_server_pages() {
    let schema = Hipc::new(&Home::new()).run(&["schema"]).ok();
    let paged: Vec<(String, Vec<String>)> = schema["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|command| command["pagination"] == true)
        .map(|command| {
            let path: Vec<&str> = command["path"]
                .as_array()
                .unwrap()
                .iter()
                .map(|part| part.as_str().unwrap())
                .collect();
            let flags = command["flags"]
                .as_array()
                .unwrap()
                .iter()
                .map(|flag| flag["name"].as_str().unwrap().to_owned())
                .filter(|name| ["--limit", "--page-token", "--all"].contains(&name.as_str()))
                .collect();
            (path.join(" "), flags)
        })
        // `hipc api` pages any route with `--all`, which this audit does not cover.
        .filter(|(path, _)| path != "api")
        .collect();
    let expected: Vec<(String, Vec<String>)> = PAGED
        .iter()
        .map(|(path, flags)| {
            (
                (*path).to_owned(),
                flags.iter().map(|flag| (*flag).to_owned()).collect(),
            )
        })
        .collect();
    assert_eq!(paged, expected);
}

/// `args` with `extra` after them.
fn with<'a>(args: &[&'a str], extra: &[&'a str]) -> Vec<&'a str> {
    let mut argv = args.to_vec();
    argv.extend_from_slice(extra);
    argv
}

/// `--page-token` asks for that page, and the envelope names the next one.
#[test]
fn every_list_paged_by_number_fetches_the_page_its_token_names() {
    for (args, path) in PAGED_BY_NUMBER {
        let body = json!({
            "results": [{"id": 3}],
            "count": 5,
            "next": format!("{{base}}{path}?page=3&page_size=2"),
            "previous": format!("{{base}}{path}?page_size=2"),
        });
        let server = Server::start(vec![route(
            "GET",
            path,
            Reply::json(200, &body.to_string()),
        )]);
        let doc = run(&server, &with(args, &["--limit", "2", "--page-token", "2"])).ok();
        assert_eq!(doc["items"], json!([{"id": 3}]), "{args:?}");
        assert_eq!(doc["next_page_token"], "3", "{args:?}");
        assert_eq!(
            server.only_request().target,
            format!("{path}?page_size=2&page=2"),
            "{args:?}"
        );
    }
}

/// `--all` follows the `next` links to the last page and prints one array.
#[test]
fn every_list_paged_by_number_follows_every_page_with_all() {
    for (args, path) in PAGED_BY_NUMBER {
        let first = json!({"results": [{"id": 1}], "next": format!("{{base}}{path}?page=2")});
        let last = json!({"results": [{"id": 2}], "next": null});
        let server = Server::start(vec![route_each(
            "GET",
            path,
            vec![
                Reply::json(200, &first.to_string()),
                Reply::json(200, &last.to_string()),
            ],
        )]);
        let doc = run(&server, &with(args, &["--all"])).ok();
        assert_eq!(doc, json!([{"id": 1}, {"id": 2}]), "{args:?}");
        let targets: Vec<String> = server.requests().into_iter().map(|r| r.target).collect();
        assert_eq!(
            targets,
            [
                format!("{path}?page_size=100"),
                format!("{path}?page_size=100&page=2")
            ],
            "{args:?}"
        );
    }
}

/// The hint of a page other than the one a `--page-token` asked for.
const CHANGED_SINCE_TOKEN: &str =
    "run again without `--page-token`, as the list may have changed while it was read";

/// A server that answers another page than the one asked for, as one answering page 1 to
/// `page=2` would, is refused rather than read: its `next` would lead to page 2 forever.
/// The hint says to start over when the list may have changed: since the token was
/// printed, or while `--all` read it. A first page asked for without a token has no such
/// excuse, so it keeps the bug-report hint.
#[test]
fn a_list_paged_by_number_refuses_another_page_than_the_one_asked_for() {
    for (args, path) in PAGED_BY_NUMBER {
        let next = format!("{{base}}{path}?page=2");
        let page = |at: Value| json!({"results": [{"id": 1}], "page": at, "next": next});
        for (asked, replies, message, hint) in [
            (
                &["--page-token", "2"][..],
                vec![page(json!(1))],
                "the API answered page `1` to a request for page `2`",
                CHANGED_SINCE_TOKEN,
            ),
            (
                &[][..],
                vec![page(json!(2))],
                "the API answered page `2` to a request for page `1`",
                "--debug",
            ),
            (
                &["--all"][..],
                vec![page(json!(1)), page(json!(1))],
                "the API answered page `1` to a request for page `2`",
                "run the command again, as the list may have changed while it was read",
            ),
        ] {
            let sent = replies.len();
            let replies = replies
                .iter()
                .map(|body| Reply::json(200, &body.to_string()))
                .collect();
            let server = Server::start(vec![route_each("GET", path, replies)]);
            let run = run(&server, &with(args, asked));
            assert_eq!(run.stdout(), "", "{args:?} {asked:?}");
            let error = run.fails(1, "internal");
            assert!(
                error["message"].as_str().unwrap().contains(message),
                "{args:?} {asked:?}: {error}"
            );
            assert!(
                error["hint"].as_str().unwrap().contains(hint),
                "{args:?} {asked:?}: {error}"
            );
            assert_eq!(server.requests().len(), sent, "{args:?} {asked:?}");
        }
    }
}

/// A `page` that is not a whole number cannot say which page it is, so it is refused, with
/// the server's text quoted, rather than compared with the page asked for.
#[test]
fn a_list_paged_by_number_refuses_a_page_that_is_not_a_whole_number() {
    for (args, path) in PAGED_BY_NUMBER {
        for (at, shown) in [
            (json!("x`y"), r"`x\u{60}y`"),
            (json!(1.5), "`1.5`"),
            (json!(-1), "`-1`"),
            (json!("+1"), "`+1`"),
            (json!(""), "``"),
            (json!(true), "`true`"),
        ] {
            let body = json!({"results": [], "page": at, "next": null});
            let server = Server::start(vec![route(
                "GET",
                path,
                Reply::json(200, &body.to_string()),
            )]);
            let error = run(&server, args).fails(1, "internal");
            assert_eq!(
                error["message"],
                format!(
                    "the API answered a `page` that is not a whole number, {shown}, so the list \
                     cannot be read in order"
                ),
                "{args:?} {at}"
            );
            assert!(
                error["hint"].as_str().unwrap().contains("--debug"),
                "{error}"
            );
        }
        // A whole number past `u64::MAX` is no page number either, and says so.
        for (at, shown) in [
            (
                json!("99999999999999999999999"),
                "`99999999999999999999999`",
            ),
            (json!(1e20), "`1e+20`"),
        ] {
            let body = json!({"results": [], "page": at, "next": null});
            let server = Server::start(vec![route(
                "GET",
                path,
                Reply::json(200, &body.to_string()),
            )]);
            let error = run(&server, args).fails(1, "internal");
            assert_eq!(
                error["message"],
                format!(
                    "the API answered a `page` that is too large to be a page number, {shown}, \
                     so the list cannot be read in order"
                ),
                "{args:?} {at}"
            );
        }
    }
}

/// A page that names itself as the page asked for is read as any other, whether it names
/// the number as a number, a float, or a decimal string, and whatever `page_size` it names:
/// the next page comes from the `next` link, so an empty list's size of 0 or a size sent as
/// a string lists as sent.
#[test]
fn a_list_paged_by_number_reads_a_page_that_names_itself() {
    for (args, path) in PAGED_BY_NUMBER {
        for (at, size) in [
            (json!(2), json!(5)),
            (json!(2.0), json!(0)),
            (json!("2"), json!("100")),
            (json!("02"), json!(null)),
            // A `page` of null names no page, as a DRF page that names none.
            (json!(null), json!(5)),
        ] {
            let body = json!({"count": 0, "results": [], "page": at, "page_size": size,
                              "next": null, "previous": null});
            let server = Server::start(vec![route(
                "GET",
                path,
                Reply::json(200, &body.to_string()),
            )]);
            let doc = run(&server, &with(args, &["--page-token", "2"])).ok();
            assert_eq!(
                doc,
                json!({"count": 0, "items": [], "page": at, "page_size": size,
                       "next_page_token": null}),
                "{args:?} {at} {size}"
            );
        }
        // An empty first page that names a size of 0, as an empty mailbox does.
        let empty = json!({"count": 0, "next": null, "previous": null, "results": [],
                           "page_size": 0});
        let server = Server::start(vec![route(
            "GET",
            path,
            Reply::json(200, &empty.to_string()),
        )]);
        assert_eq!(
            run(&server, &with(args, &["--all"])).ok(),
            json!([]),
            "{args:?}"
        );
    }
}

/// `--all` follows a list whose every page names its own `page`, as each is checked.
#[test]
fn every_list_paged_by_number_follows_pages_that_name_themselves() {
    for (args, path) in PAGED_BY_NUMBER {
        let first = json!({"results": [{"id": 1}], "page": 1,
                           "next": format!("{{base}}{path}?page=2")});
        let last = json!({"results": [{"id": 2}], "page": 2, "next": null});
        let server = Server::start(vec![route_each(
            "GET",
            path,
            vec![
                Reply::json(200, &first.to_string()),
                Reply::json(200, &last.to_string()),
            ],
        )]);
        let doc = run(&server, &with(args, &["--all"])).ok();
        assert_eq!(doc, json!([{"id": 1}, {"id": 2}]), "{args:?}");
        assert_eq!(server.requests().len(), 2, "{args:?}");
    }
}

/// Pages count from 1, so a `--page-token` of 0 is no token an answer gave: a usage error,
/// with nothing sent.
#[test]
fn a_page_token_of_zero_is_a_usage_error() {
    let home = Home::new();
    let lists = PAGED_BY_NUMBER
        .iter()
        .map(|(args, _)| *args)
        .chain([&["hub", "models", "list"][..]]);
    for args in lists {
        for token in ["0", "00"] {
            let error = Hipc::new(&home)
                .run(&with(&with(&["--json"], args), &["--page-token", token]))
                .fails(2, "usage");
            assert!(
                error["message"]
                    .as_str()
                    .unwrap()
                    .contains("a page number from 1"),
                "{args:?} {token}: {error}"
            );
        }
    }
}

/// The project transfers are two lists in one answer, `{incoming, outgoing}`, read whole:
/// a `next` link is refused by the list and by the read before a decision, which would
/// otherwise miss a transfer on a later page, and a null one is not printed.
#[test]
fn project_transfers_refuse_a_next_link() {
    let path = "/api/accounts/project-transfers/";
    let paged = json!({
        "incoming": [{"id": 5}],
        "outgoing": [],
        "next": format!("{{base}}{path}?page=2"),
    });
    for args in [
        &["account", "project-transfers", "list"][..],
        &["account", "project-transfers", "decline", "6", "--yes"],
    ] {
        let server = Server::start(vec![route(
            "GET",
            path,
            Reply::json(200, &paged.to_string()),
        )]);
        let run = run(&server, args);
        assert_eq!(run.stdout(), "", "{args:?}");
        let error = run.fails(1, "internal");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("the API split a list into pages"),
            "{args:?}: {error}"
        );
        assert_eq!(server.requests().len(), 1, "{args:?}");
    }
    // A transfer on the page read is decided on, whatever comes after it.
    let decline = "/api/accounts/project-transfers/5/decline/";
    let server = Server::start(vec![
        route("GET", path, Reply::json(200, &paged.to_string())),
        route("POST", decline, Reply::json(204, "")),
    ]);
    run(
        &server,
        &["account", "project-transfers", "decline", "5", "--yes"],
    )
    .ok();
    let targets: Vec<String> = server.requests().into_iter().map(|r| r.target).collect();
    assert_eq!(targets, [path, decline]);
    let last = json!({"incoming": [], "outgoing": [], "next": null, "previous": null});
    let server = Server::start(vec![route(
        "GET",
        path,
        Reply::json(200, &last.to_string()),
    )]);
    assert_eq!(
        run(&server, &["account", "project-transfers", "list"]).ok(),
        json!({"items": [], "next_page_token": null})
    );
}
