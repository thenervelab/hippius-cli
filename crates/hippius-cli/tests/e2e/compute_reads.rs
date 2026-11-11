//! The small compute reads: the ready-made apps, a launch's feasibility, the owner's
//! SSH keys, one stored SSH key, and a VM's failovers.

use hippius::routes::compute;
use serde_json::{Value, json};

use crate::common::{Hipc, Home, MEMBER, Reply, Run, Server, VM, draft, parsed, route, route_of};

/// Swagger's `{failovers}`: one done by itself, newest first.
const FAILOVERS: &str = concat!(
    r#"{"failovers":[{"id":"6f1c0a52-3c1e-4d7e-9a43-5e0f2b9b8c11","automatic":true,"#,
    r#""status":"done","point_taken_at":"2026-10-27T09:45:00Z","#,
    r#""started_at":"2026-10-27T10:00:00Z","committed_at":"2026-10-27T10:03:00Z","#,
    r#""finished_at":"2026-10-27T10:09:00Z","created_at":"2026-10-27T10:00:00Z","#,
    r#""error":null,"usable":true}]}"#
);

/// Swagger's feasibility object, for a region.
const FEASIBLE: &str =
    r#"{"flavor":"small","region":"FR","verdict":"yes","reason":null,"feasible":true}"#;

/// A verdict that is not `yes`, for the whole fleet: still a 200.
const NOT_NOW: &str = concat!(
    r#"{"flavor":"small","region":null,"verdict":"not-now","reason":"no-capacity","#,
    r#""feasible":false}"#
);

/// One app, with fields from the swagger description of `GET /api/compute/apps/`.
const APPS: &str = concat!(
    r#"[{"slug":"chat","image":"ubuntu-24.04","service_port":8080,"private_port":null,"#,
    r#""config_schema":[{"name":"api_key","secret":true}]}]"#
);

/// Swagger's `SSHKey`.
const KEY: &str = concat!(
    r#"{"id":3,"name":"laptop","public_key":"ssh-ed25519 AAAA me@laptop","#,
    r#""fingerprint":"SHA256:abc","created":"2026-10-27T10:00:00Z","last_used":null}"#
);

fn hipc(server: &Server, args: &[&str]) -> Run {
    let mut argv = vec!["compute"];
    argv.extend(args);
    Hipc::against(&Home::new(), server).run(&argv)
}

fn failovers_path() -> String {
    format!("/api/compute/vms/{VM}/failovers/")
}

/// The public price list, in the shape of swagger `compute_public-prices_list`: credits as
/// decimal strings.
const PUBLIC_PRICES: &str = concat!(
    r#"{"currency":"credits","units":"credits","hours_per_month":730,"os_disk_tiers":[{"#,
    r#""tier":"nvme","name":"NVMe","disk_gb":{"hourly":"0.0001","monthly":"0.07"},"#,
    r#""max_flavor":null,"flavors":[{"code":"small","name":"Small","vcpus":1,"#,
    r#""memory_mb":2048,"disk_gb":20,"available":true,"hourly":"0.0438","#,
    r#""monthly":"31.97"}]}],"dedicated_disks":null}"#
);

/// `public-prices` reads the public price list, which needs no token,
/// and prints it as sent. `prices` still reads the account's prices, which need one.
#[test]
fn public_prices_need_no_token() {
    let server = Server::start(vec![route_of(
        &compute::PUBLIC_PRICES,
        [],
        Reply::json(200, PUBLIC_PRICES),
    )]);
    let home = Home::new();
    let run = Hipc::tokenless(&home, &server).run(&["compute", "public-prices"]);
    assert_eq!(run.ok(), parsed(PUBLIC_PRICES));
    let request = server.only_request();
    assert_eq!(request.target, "/api/compute/public-prices/");
    assert_eq!(request.header("authorization"), None);
    Hipc::tokenless(&home, &server)
        .run(&["compute", "prices"])
        .fails(1, "auth");
    assert_eq!(server.requests().len(), 1);
    // A token that is configured still goes with it, as with `cdn prices`.
    Hipc::against(&home, &server)
        .run(&["compute", "public-prices"])
        .ok();
}

/// Text mode says on stderr that an empty list or object holds nothing, so a person
/// can tell it from a hang; stdout stays empty but for the fields beside the list. JSON mode
/// and `--quiet` print no note.
#[test]
fn text_mode_notes_an_empty_list_or_object_on_stderr() {
    let server = Server::start(vec![
        route_of(
            &compute::VMS,
            [],
            Reply::json(
                200,
                r#"{"count":0,"next":null,"previous":null,"results":[]}"#,
            ),
        ),
        route_of(&compute::VM, [&VM], Reply::json(200, "{}")),
    ]);
    let home = Home::new();
    let text = |args: &[&str]| {
        Hipc::against(&home, &server).run(&[&["--output", "text"][..], args].concat())
    };
    let list = text(&["compute", "vms", "list"]);
    assert_eq!(list.code(), Some(0));
    assert_eq!(list.stdout(), "count: 0\n");
    assert_eq!(list.stderr(), "note: no items\n");
    let get = text(&["compute", "vms", "get", VM]);
    assert_eq!((get.stdout(), get.stderr()), ("", "note: no fields\n"));
    let quiet = text(&["--quiet", "compute", "vms", "list"]);
    assert_eq!(quiet.stderr(), "");
    let json = Hipc::against(&home, &server).run(&["compute", "vms", "list"]);
    assert_eq!(json.ok()["items"], json!([]));
    assert_eq!(json.stderr(), "");
}

/// Text mode picks the list layout from the command, not from the answer's shape: a `get`
/// whose server object happens to hold `items` and `next_page_token` is one object, with no
/// table, no `next page: --page-token` hint (a flag `get` does not take), and no `no items`
/// note for an empty `items`.
#[test]
fn a_get_that_holds_items_and_a_page_token_is_not_a_list() {
    let paged = r#"{"items":[{"id":"a"}],"next_page_token":"2"}"#;
    let empty = r#"{"items":[],"next_page_token":null}"#;
    let server = Server::start(vec![
        route_of(&compute::VM, [&VM], Reply::json(200, paged)),
        route_of(&compute::VM, [&"vm-2"], Reply::json(200, empty)),
    ]);
    let home = Home::new();
    let text = |args: &[&str]| {
        Hipc::against(&home, &server).run(&[&["--output", "text"][..], args].concat())
    };
    let get = text(&["compute", "vms", "get", VM]);
    assert_eq!(get.code(), Some(0), "{}", get.stderr());
    assert_eq!(
        get.stdout(),
        "items            [{\"id\":\"a\"}]\nnext_page_token  2\n"
    );
    assert!(!get.stdout().contains("--page-token"), "{}", get.stdout());
    assert_eq!(get.stderr(), "");
    let get = text(&["compute", "vms", "get", "vm-2"]);
    assert_eq!(
        (get.stdout(), get.stderr()),
        ("items            []\nnext_page_token  -\n", "")
    );
    // JSON mode prints the answer as sent either way.
    let json = Hipc::against(&home, &server).run(&["compute", "vms", "get", VM]);
    assert_eq!(json.ok(), serde_json::from_str::<Value>(paged).unwrap());
}

#[test]
fn objects_print_as_sent_and_lists_are_envelopes() {
    let server = Server::start(vec![
        route_of(&compute::APPS, [], Reply::json(200, APPS)),
        route_of(
            &compute::ACCOUNT_SSH_KEYS,
            [],
            Reply::json(200, &format!("[{KEY}]")),
        ),
        route(
            "GET",
            "/api/compute/feasibility/?flavor=small&region=FR",
            Reply::json(200, FEASIBLE),
        ),
        route(
            "GET",
            "/api/compute/feasibility/?flavor=small",
            Reply::json(200, NOT_NOW),
        ),
        route_of(&compute::SSH_KEY, [&3], Reply::json(200, KEY)),
        route_of(&compute::VM_FAILOVERS, [&VM], Reply::json(200, FAILOVERS)),
    ]);
    let envelope = |items: Value| json!({ "items": items, "next_page_token": null });
    assert_eq!(hipc(&server, &["apps"]).ok(), envelope(parsed(APPS)));
    assert_eq!(
        hipc(&server, &["account-ssh-keys"]).ok(),
        envelope(json!([parsed(KEY)]))
    );
    assert_eq!(
        hipc(
            &server,
            &["feasibility", "--flavor", "small", "--region", "FR"]
        )
        .ok(),
        parsed(FEASIBLE)
    );
    assert_eq!(
        hipc(&server, &["feasibility", "--flavor", "small"]).ok(),
        parsed(NOT_NOW),
        "a verdict that is not yes is still the answer, exit 0"
    );
    assert_eq!(hipc(&server, &["ssh-keys", "get", "3"]).ok(), parsed(KEY));
    assert_eq!(
        hipc(&server, &["vms", "failovers", VM]).ok(),
        envelope(parsed(FAILOVERS)["failovers"].clone())
    );
    assert_eq!(server.requests().len(), 6);
}

#[test]
fn feasibility_sends_the_flavor_encoded_and_the_region_in_uppercase() {
    // One region parser for every command that takes a country code: two letters, sent in
    // uppercase, the ISO spelling.
    let server = Server::start(vec![
        route(
            "GET",
            "/api/compute/feasibility/?flavor=x%26region%3Dus",
            Reply::json(200, NOT_NOW),
        ),
        route(
            "GET",
            "/api/compute/feasibility/?flavor=small&region=FR",
            Reply::json(200, FEASIBLE),
        ),
    ]);
    hipc(&server, &["feasibility", "--flavor", "x&region=us"]).ok();
    hipc(
        &server,
        &["feasibility", "--flavor", "small", "--region", "fr"],
    )
    .ok();
    assert_eq!(server.requests().len(), 2);
}

/// `--os-disk-tier` sends `os_disk_tier` only when given, after the region, and
/// the answer echoes it (swagger). A tier off swagger's enum is usage with zero HTTP.
#[test]
fn feasibility_sends_the_os_disk_tier_only_when_given() {
    let echoed = concat!(
        r#"{"flavor":"small","region":"FR","verdict":"never","#,
        r#""reason":"disk-tier-not-offered","feasible":false,"os_disk_tier":"hdd"}"#
    );
    let server = Server::start(vec![
        route(
            "GET",
            "/api/compute/feasibility/?flavor=small&region=FR&os_disk_tier=hdd",
            Reply::json(200, echoed),
        ),
        route(
            "GET",
            "/api/compute/feasibility/?flavor=small&os_disk_tier=ssd",
            Reply::json(200, NOT_NOW),
        ),
    ]);
    let args = [
        "feasibility",
        "--flavor",
        "small",
        "--region",
        "FR",
        "--os-disk-tier",
        "hdd",
    ];
    assert_eq!(hipc(&server, &args).ok(), parsed(echoed));
    let args = ["feasibility", "--flavor", "small", "--os-disk-tier", "ssd"];
    hipc(&server, &args).ok();
    assert_eq!(server.requests().len(), 2);

    let quiet = Server::start(vec![]);
    for tier in ["sata", "HDD", ""] {
        let args = ["feasibility", "--flavor", "small", "--os-disk-tier", tier];
        hipc(&quiet, &args).fails(2, "usage");
    }
    assert!(quiet.requests().is_empty());
}

#[test]
fn feasibility_needs_a_flavor_and_a_two_letter_region_before_sending() {
    let server = Server::start(vec![]);
    hipc(&server, &["feasibility"]).fails(2, "usage");
    for region in ["FRA", "F", "", "F1", "../"] {
        hipc(
            &server,
            &["feasibility", "--flavor", "small", "--region", region],
        )
        .fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn feasibility_errors_map_to_their_codes() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/compute/feasibility/?flavor=huge",
            Reply::json(404, r#"{"error":"Unknown flavor"}"#),
        ),
        route(
            "GET",
            "/api/compute/feasibility/?flavor=small&region=ZZ",
            Reply::json(400, r#"{"error":"Malformed region"}"#),
        ),
        route(
            "GET",
            "/api/compute/feasibility/?flavor=small",
            Reply::json(503, r#"{"error":"Control plane unreachable"}"#),
        ),
    ]);
    let error = hipc(&server, &["feasibility", "--flavor", "huge"]).fails(1, "not_found");
    assert_eq!(
        error["hint"],
        "run `hipc compute flavors` to see what exists"
    );
    assert_eq!(
        error["message"],
        "cannot check whether the flavor `huge` can launch: the API says `Unknown flavor`"
    );
    hipc(
        &server,
        &["feasibility", "--flavor", "small", "--region", "ZZ"],
    )
    .fails(1, "validation");
    let error = hipc(&server, &["feasibility", "--flavor", "small"]).fails(1, "unavailable");
    assert_eq!(error["retryable"], true);
}

#[test]
fn failovers_errors_map_to_their_codes() {
    let server = Server::start(vec![route_of(
        &compute::VM_FAILOVERS,
        [&VM],
        Reply::not_found(),
    )]);
    let error = hipc(&server, &["vms", "failovers", VM]).fails(1, "not_found");
    assert_eq!(
        error["hint"],
        "run `hipc compute vms list` to see what exists"
    );
    let down = Server::start(vec![route_of(
        &compute::VM_FAILOVERS,
        [&VM],
        Reply::json(503, r#"{"error":"Not available"}"#),
    )]);
    hipc(&down, &["vms", "failovers", VM]).fails(1, "unavailable");
    let shapeless = Server::start(vec![route_of(
        &compute::VM_FAILOVERS,
        [&VM],
        Reply::json(200, r#"{"items":[]}"#),
    )]);
    let error = hipc(&shapeless, &["vms", "failovers", VM]).fails(1, "internal");
    assert_eq!(
        error["message"],
        format!(
            "cannot list the failovers of the VM `{VM}`: the API answered without a \
             `failovers` list"
        )
    );
}

#[test]
fn ssh_key_get_a_missing_key_hints_at_the_list() {
    let server = Server::start(vec![route_of(&compute::SSH_KEY, [&9], Reply::not_found())]);
    let error = hipc(&server, &["ssh-keys", "get", "9"]).fails(1, "not_found");
    assert_eq!(
        error["hint"],
        "run `hipc compute ssh-keys list` to see what exists"
    );
}

#[test]
fn catalog_reads_map_their_errors() {
    let server = Server::start(vec![
        route_of(&compute::APPS, [], Reply::not_found()),
        route_of(
            &compute::ACCOUNT_SSH_KEYS,
            [],
            Reply::json(403, r#"{"detail":"Compute is in private beta."}"#),
        ),
    ]);
    hipc(&server, &["apps"]).fails(1, "not_found");
    hipc(&server, &["account-ssh-keys"]).fails(1, "permission");
    let paged = Server::start(vec![route_of(
        &compute::APPS,
        [],
        Reply::json(
            200,
            r#"{"count":2,"next":"{base}/api/compute/apps/?page=2","previous":null,"results":[]}"#,
        ),
    )]);
    hipc(&paged, &["apps"]).fails(1, "internal");
}

#[test]
fn a_membership_sends_the_header_on_compute_reads_but_not_on_one_ssh_key() {
    let server = Server::start(vec![
        route_of(&compute::APPS, [], Reply::json(200, "[]")),
        route_of(&compute::ACCOUNT_SSH_KEYS, [], Reply::json(200, "[]")),
        route(
            "GET",
            "/api/compute/feasibility/?flavor=small",
            Reply::json(200, NOT_NOW),
        ),
        route_of(&compute::VM_FAILOVERS, [&VM], Reply::json(200, FAILOVERS)),
        route_of(&compute::SSH_KEY, [&3], Reply::json(200, KEY)),
    ]);
    let home = Home::new();
    for args in [
        &["apps"][..],
        &["account-ssh-keys"],
        &["feasibility", "--flavor", "small"],
        &["vms", "failovers", VM],
        &["ssh-keys", "get", "3"],
    ] {
        let mut argv = vec!["compute"];
        argv.extend(args);
        Hipc::against(&home, &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&argv)
            .ok();
    }
    let requests = server.requests();
    assert_eq!(requests.len(), 5);
    for scoped in &requests[..4] {
        assert_eq!(
            scoped.header("x-hippius-account"),
            Some(MEMBER),
            "{}",
            scoped.target
        );
    }
    assert_eq!(requests[4].header("x-hippius-account"), None);
}

/// A grant the member lacks is the server's code, with the hint to act in the token's own
/// account.
#[test]
fn a_member_without_the_grant_is_told_to_act_as_self() {
    let server = Server::start(vec![route_of(
        &compute::ACCOUNT_SSH_KEYS,
        [],
        Reply::json(
            403,
            r#"{"code":"insufficient_grant","detail":"vms admin required"}"#,
        ),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["compute", "account-ssh-keys"])
        .fails(1, "insufficient_grant");
    assert_eq!(
        error["hint"],
        "pass `--account self` to act in the token's own account"
    );
}

#[test]
fn debug_names_each_url_with_no_query_value() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/compute/feasibility/?flavor=small&region=FR",
            Reply::json(200, FEASIBLE),
        ),
        route_of(&compute::VM_FAILOVERS, [&VM], Reply::json(200, FAILOVERS)),
        route_of(&compute::SSH_KEY, [&3], Reply::json(200, KEY)),
        route_of(&compute::APPS, [], Reply::json(200, "[]")),
        route_of(&compute::ACCOUNT_SSH_KEYS, [], Reply::json(200, "[]")),
    ]);
    let url = server.url();
    for (args, target) in [
        (
            &["feasibility", "--flavor", "small", "--region", "FR"][..],
            "/api/compute/feasibility/?flavor=<redacted>&region=<redacted>".to_owned(),
        ),
        (&["vms", "failovers", VM], failovers_path()),
        (&["ssh-keys", "get", "3"], "/api/ssh-keys/3/".to_owned()),
        (&["apps"], "/api/compute/apps/".to_owned()),
        (
            &["account-ssh-keys"],
            "/api/compute/account-ssh-keys/".to_owned(),
        ),
    ] {
        let mut argv = args.to_vec();
        argv.push("--debug");
        let run = hipc(&server, &argv);
        assert_eq!(run.code(), Some(0), "{}", run.stderr());
        assert_eq!(
            run.stderr(),
            format!("debug: GET {url}{target}\ndebug: -> 200\n")
        );
    }
}

#[test]
fn an_id_that_could_change_the_route_is_usage_without_a_request() {
    let server = Server::start(vec![]);
    let long = "a".repeat(65);
    for vm_id in ["../ssh-keys", "a/b", "a?b", "", &long] {
        hipc(&server, &["vms", "failovers", vm_id]).fails(2, "usage");
    }
    for id in ["x", "-1", "1.5", "", "3/4", "18446744073709551616"] {
        hipc(&server, &["ssh-keys", "get", id]).fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

/// A dry run in JSON mode shows the request it would send, query values included
/// (tenet 9).
#[test]
fn a_feasibility_dry_run_shows_the_query_it_would_send() {
    let server = Server::start(vec![]);
    let doc = hipc(
        &server,
        &[
            "feasibility",
            "--flavor",
            "small",
            "--region",
            "FR",
            "--dry-run",
        ],
    )
    .ok();
    assert_eq!(draft(&doc)["method"], "GET");
    assert_eq!(
        draft(&doc)["url"],
        format!(
            "{}/api/compute/feasibility/?flavor=small&region=FR",
            server.url()
        )
    );
    assert!(server.requests().is_empty());
}
