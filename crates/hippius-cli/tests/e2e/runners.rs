//! `hipc compute runners`: GitHub Actions runner settings, installations, jobs, and the
//! connect URL.

use serde_json::{Value, json};

use crate::common::{
    Hipc, Home, MEMBER, PROJECT, Reply, Route, Run, Server, api, draft, parsed, route,
};

/// The settings, shaped as the live answer was on 2026-10-27 (values made up).
const SETTINGS: &str = concat!(
    r#"{"enabled":false,"max_concurrent":2,"effective_max_concurrent":2,"#,
    r#""allowed_sizes":["small"],"default_region":"","allow_public_repos":false,"#,
    r#""quota_runners":2,"active":0,"sizes":{"small":"runner-small","medium":null,"#,
    r#""large":null},"size_flavors":{"small":{"code":"runner-small","vcpus":2,"#,
    r#""price_credits_monthly":12.5,"available":true},"medium":null,"large":null}}"#
);

/// An installation, shaped as the console types it.
const INSTALLATION: &str = concat!(
    r#"{"id":7,"installation_id":51234567,"account_login":"acme","#,
    r#""account_type":"Organization","repository_selection":"all","status":"active","#,
    r#""linked_at":"2026-10-27T10:00:00Z","#,
    r#""settings_url":"https://github.com/organizations/acme/settings/installations/1"}"#
);

/// A job the server refused, with the refusal on the row as the console types it.
const REFUSED_JOB: &str = concat!(
    r#"{"id":900,"repo":"acme/app","job_name":"test","labels":["hippius-small"],"#,
    r#""size":"small","status":"refused","refusal":{"code":"compute_runway","#,
    r#""message":"Balance too low","runway_hours":24},"billed_seconds":0,"#,
    r#""estimated_cost_credits":null}"#
);

const CONNECT: &str = concat!(
    r#"{"url":"https://github.com/apps/hippius-runners/installations/new?state=s1.sig","#,
    r#""state":"s1.sig","expires_in":600}"#
);

fn hipc(server: &Server, args: &[&str]) -> Run {
    Hipc::against(&Home::new(), server).run(&runners(args))
}

fn text(server: &Server, args: &[&str]) -> Run {
    let mut argv = vec!["--output", "text"];
    argv.extend(runners(args));
    Hipc::against(&Home::new(), server).run(&argv)
}

fn runners<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut argv = vec!["compute", "runners"];
    argv.extend(args);
    argv
}

/// A jobs route. One without a query answers every query, so the test server, which takes
/// the first route that matches, lists it after the routes with one.
fn jobs(query: &str, body: &str) -> Route {
    route(
        "GET",
        &format!("/api/runners/jobs/{query}"),
        Reply::json(200, body),
    )
}

#[test]
fn settings_get_prints_the_settings_as_sent() {
    let server = Server::start(vec![route(
        "GET",
        "/api/runners/settings/",
        Reply::json(200, SETTINGS),
    )]);
    assert_eq!(hipc(&server, &["settings", "get"]).ok(), parsed(SETTINGS));
    let request = server.only_request();
    assert_eq!(request.method, "GET");
    assert_eq!(request.target, "/api/runners/settings/");
    let run = text(&server, &["settings", "get"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(run.stdout().contains("runner-small"), "{}", run.stdout());
    assert!(!run.stdout().trim_start().starts_with('{'));
}

#[test]
fn settings_update_sends_only_the_flags_given() {
    let server = Server::start(vec![
        route("GET", "/api/runners/settings/", Reply::json(200, SETTINGS)),
        route("PUT", "/api/runners/settings/", Reply::json(200, SETTINGS)),
    ]);
    let cases: [(&[&str], Value); 6] = [
        (&["--max-concurrent", "4"], json!({"max_concurrent": 4})),
        (
            &["--allowed-size", "small", "--allowed-size", "large"],
            json!({"allowed_sizes": ["small", "large"]}),
        ),
        (&["--default-region", "fr"], json!({"default_region": "FR"})),
        (
            &["--default-region", "anywhere"],
            json!({"default_region": ""}),
        ),
        (&["--enabled", "false"], json!({"enabled": false})),
        (
            &[
                "--enabled",
                "true",
                "--max-concurrent",
                "0",
                "--allowed-size",
                "medium",
                "--default-region",
                "DE",
                "--allow-public-repos",
                "true",
            ],
            json!({
                "enabled": true,
                "max_concurrent": 0,
                "allowed_sizes": ["medium"],
                "default_region": "DE",
                "allow_public_repos": true,
            }),
        ),
    ];
    for (flags, body) in cases {
        let mut argv = vec!["settings", "update", "--yes"];
        argv.extend(flags);
        assert_eq!(hipc(&server, &argv).ok(), parsed(SETTINGS), "{flags:?}");
        let request = server.requests().pop().expect("one request per case");
        assert_eq!(request.method, "PUT");
        assert_eq!(request.target, "/api/runners/settings/");
        assert_eq!(request.json(), body, "{flags:?}");
    }
    // Each update reads the settings first, then sends the PUT.
    let sent = server.requests();
    assert_eq!(sent.len(), 12);
    assert!(sent.iter().step_by(2).all(|r| r.method == "GET"));
    assert_eq!(
        String::from_utf8(sent[1].body.clone()).unwrap(),
        r#"{"max_concurrent":4}"#
    );
}

#[test]
fn settings_update_needs_yes_and_its_question_names_every_change() {
    // The settings in force: off, at most 2 at once, `small` only, anywhere, no public repos.
    let server = Server::start(vec![route(
        "GET",
        "/api/runners/settings/",
        Reply::json(200, SETTINGS),
    )]);
    let cases: [(&[&str], &str); 5] = [
        (
            &["--max-concurrent", "4"],
            "set the runner settings `max_concurrent` to `4`",
        ),
        (
            &["--default-region", "fr", "--allowed-size", "medium"],
            "set the runner settings `allowed_sizes` to `medium` and `default_region` to `FR`",
        ),
        (
            &["--allowed-size", "small", "--allowed-size", "large"],
            "set the runner settings `allowed_sizes` to `small` and `large`",
        ),
        (
            &["--enabled", "true"],
            "set the runner settings `enabled` to `true`",
        ),
        (
            &["--allow-public-repos", "true", "--enabled", "false"],
            "set the runner settings `enabled` to `false` and `allow_public_repos` to `true`, \
             which lets a pull request from anyone start a job on a public repository, billed to \
             the balance",
        ),
    ];
    for (flags, what) in cases {
        let mut argv = vec!["settings", "update"];
        argv.extend(flags);
        let error = hipc(&server, &argv).fails(2, "confirmation_required");
        assert_eq!(
            error["message"],
            format!("this would {what}, and `--yes` was not passed"),
            "{flags:?}"
        );
    }
    // Each one read the settings to compare, and sent no PUT.
    let sent = server.requests();
    assert_eq!(sent.len(), 5);
    assert!(sent.iter().all(|r| r.method == "GET"));
}

/// A change that cannot let runners spend more, a pure cut or the settings in force again,
/// is sent without `--yes`, as every spend cap's cut is.
#[test]
fn settings_update_sends_a_cut_unasked() {
    let on = r#"{"enabled":true,"max_concurrent":4,"allowed_sizes":["small","large"],"default_region":"FR","allow_public_repos":true}"#;
    let server = Server::start(vec![
        route("GET", "/api/runners/settings/", Reply::json(200, on)),
        route("PUT", "/api/runners/settings/", Reply::json(200, on)),
    ]);
    for flags in [
        &["--enabled", "false"][..],
        &["--enabled", "true"],
        &["--max-concurrent", "1"],
        &["--max-concurrent", "4"],
        &["--allowed-size", "large"],
        &["--default-region", "FR"],
        &["--allow-public-repos", "false"],
        &["--allow-public-repos", "true"],
        &[
            "--enabled",
            "false",
            "--max-concurrent",
            "1",
            "--allowed-size",
            "small",
            "--allow-public-repos",
            "false",
        ],
    ] {
        let mut argv = vec!["settings", "update"];
        argv.extend(flags);
        hipc(&server, &argv).ok();
        let put = server.requests().pop().expect("the PUT");
        assert_eq!(put.method, "PUT", "{flags:?}");
    }
    assert_eq!(server.requests().len(), 18);
    // `0` at once and a new default region spend no more, so neither asks.
    for flags in [
        &["--max-concurrent", "0"][..],
        &["--default-region", "de"],
        &["--default-region", "anywhere"],
    ] {
        let mut argv = vec!["settings", "update"];
        argv.extend(flags);
        hipc(&server, &argv).ok();
        assert_eq!(server.requests().pop().unwrap().method, "PUT", "{flags:?}");
    }
    assert_eq!(server.requests().len(), 24);
    // A setting the read does not hold as hipc compares it may be a raise, so it asks.
    let odd = r#"{"enabled":"yes","max_concurrent":"4","allowed_sizes":"small","default_region":null,"allow_public_repos":null}"#;
    let server = Server::start(vec![route(
        "GET",
        "/api/runners/settings/",
        Reply::json(200, odd),
    )]);
    for flags in [
        &["--enabled", "true"][..],
        &["--max-concurrent", "1"],
        &["--allowed-size", "small"],
        &["--allow-public-repos", "true"],
    ] {
        let mut argv = vec!["settings", "update"];
        argv.extend(flags);
        hipc(&server, &argv).fails(2, "confirmation_required");
    }
    // Turning runners off, closing public repositories, `0` at once, or a region never asks,
    // whatever the read holds.
    let server = Server::start(vec![
        route("GET", "/api/runners/settings/", Reply::json(200, odd)),
        route("PUT", "/api/runners/settings/", Reply::json(200, on)),
    ]);
    hipc(
        &server,
        &[
            "settings",
            "update",
            "--enabled",
            "false",
            "--allow-public-repos",
            "false",
            "--max-concurrent",
            "0",
            "--default-region",
            "FR",
        ],
    )
    .ok();
    // A failed read sends no PUT.
    let server = Server::start(vec![route(
        "GET",
        "/api/runners/settings/",
        Reply::json(502, r#"{"error":"x"}"#),
    )]);
    let error =
        hipc(&server, &["settings", "update", "--enabled", "false"]).fails(1, "unavailable");
    assert_eq!(error["side_effects"], "none");
    assert_eq!(server.only_request().method, "GET");
}

#[test]
fn settings_update_refuses_bad_flags_before_any_request_or_token() {
    let server = Server::start(vec![]);
    let error = Hipc::tokenless(&Home::new(), &server)
        .run(&runners(&["settings", "update", "--yes"]))
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        "`hipc compute runners settings update` was given nothing to change"
    );
    assert_eq!(
        error["hint"],
        "pass `--enabled`, `--max-concurrent`, `--allowed-size`, `--default-region`, or \
         `--allow-public-repos`"
    );
    let error = hipc(
        &server,
        &[
            "settings",
            "update",
            "--allowed-size",
            "small",
            "--allowed-size",
            "small",
            "--yes",
        ],
    )
    .fails(2, "usage");
    assert_eq!(
        error["message"],
        "`--allowed-size` names `small` more than once"
    );
    for flags in [
        &["--allowed-size", "xlarge"][..],
        &["--allowed-size", "Small"],
        &["--default-region", "FRA"],
        &["--default-region", ""],
        &["--default-region", "f1"],
        &["--enabled", "yes"],
        &["--max-concurrent", "-1"],
        &["--max-concurrent", "4294967296"],
    ] {
        let mut argv = vec!["settings", "update", "--yes"];
        argv.extend(flags);
        hipc(&server, &argv).fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn settings_update_errors_map_to_their_codes() {
    for (status, body, code, effects) in [
        (
            400,
            r#"{"error":"max_concurrent above quota"}"#,
            "validation",
            "none",
        ),
        (
            403,
            r#"{"code":"insufficient_grant","error":"needs runners admin"}"#,
            "insufficient_grant",
            "none",
        ),
        (502, r#"{"error":"x"}"#, "unavailable", "partial"),
    ] {
        let server = Server::start(vec![
            route("GET", "/api/runners/settings/", Reply::json(200, SETTINGS)),
            route("PUT", "/api/runners/settings/", Reply::json(status, body)),
        ]);
        let error = hipc(
            &server,
            &["settings", "update", "--enabled", "true", "--yes"],
        )
        .fails(1, code);
        assert_eq!(error["http_status"], status);
        assert_eq!(error["side_effects"], effects, "{status}");
        assert_eq!(error["retryable"], false, "{status}");
    }
}

#[test]
fn installations_list_prints_the_envelope_from_a_bare_array() {
    let server = Server::start(vec![route(
        "GET",
        "/api/runners/installations/",
        Reply::json(200, &format!("[{INSTALLATION}]")),
    )]);
    let doc = hipc(&server, &["installations", "list"]).ok();
    assert_eq!(
        doc,
        json!({"items": [parsed(INSTALLATION)], "next_page_token": null})
    );
    let run = text(&server, &["installations", "list"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(run.stdout().contains("acme"), "{}", run.stdout());
    assert!(!run.stdout().trim_start().starts_with('{'));
    let odd = Server::start(vec![route(
        "GET",
        "/api/runners/installations/",
        Reply::json(200, r#"{"installations":[]}"#),
    )]);
    hipc(&odd, &["installations", "list"]).fails(1, "internal");
}

#[test]
fn installations_delete_needs_yes_and_prints_a_document_on_204() {
    let server = Server::start(vec![route(
        "DELETE",
        "/api/runners/installations/7/",
        Reply::json(204, ""),
    )]);
    let error = hipc(&server, &["installations", "delete", "7"]).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        "this would unlink the GitHub App installation `7` from the account, and `--yes` was \
         not passed"
    );
    assert!(server.requests().is_empty());
    let doc = hipc(&server, &["installations", "delete", "7", "--yes"]).ok();
    assert_eq!(doc, json!({"id": 7, "deleted": true}));
    let request = server.only_request();
    assert_eq!(request.method, "DELETE");
    assert_eq!(request.target, "/api/runners/installations/7/");
    let run = text(&server, &["installations", "delete", "7", "--yes"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(!run.stdout().trim_start().starts_with('{'));
}

#[test]
fn installations_delete_prints_a_body_and_maps_a_missing_one_to_not_found() {
    let server = Server::start(vec![route(
        "DELETE",
        "/api/runners/installations/7/",
        Reply::json(200, r#"{"id":7,"deleted":true,"note":"x"}"#),
    )]);
    let doc = hipc(&server, &["installations", "delete", "7", "--yes"]).ok();
    assert_eq!(doc["note"], "x");
    let missing = Server::start(vec![route(
        "DELETE",
        "/api/runners/installations/8/",
        Reply::not_found(),
    )]);
    let error = hipc(&missing, &["installations", "delete", "8", "--yes"]).fails(1, "not_found");
    assert_eq!(error["http_status"], 404);
    for bad in ["x", "-1", "7/../8", ""] {
        hipc(&missing, &["installations", "delete", bad, "--yes"]).fails(2, "usage");
    }
    assert_eq!(missing.requests().len(), 1);
}

#[test]
fn jobs_reads_the_cursor_into_the_page_token() {
    let first = format!(r#"{{"results":[{REFUSED_JOB}],"next_before":812}}"#);
    let server = Server::start(vec![
        jobs(
            "?before=812",
            r#"{"results":[{"id":5}],"next_before":null}"#,
        ),
        jobs("", &first),
    ]);
    let doc = hipc(&server, &["jobs"]).ok();
    assert_eq!(
        doc,
        json!({"items": [parsed(REFUSED_JOB)], "next_page_token": "812"})
    );
    // The refusal is on the row, printed as sent: not an error of the command.
    assert_eq!(doc["items"][0]["refusal"]["code"], "compute_runway");
    assert_eq!(server.only_request().target, "/api/runners/jobs/");
    let next = hipc(&server, &["jobs", "--page-token", "812"]).ok();
    assert_eq!(next, json!({"items": [{"id": 5}], "next_page_token": null}));
    // A token with leading zeros is the same cursor.
    hipc(&server, &["jobs", "--page-token", "0812"]).ok();
    let targets: Vec<String> = server.requests().into_iter().map(|r| r.target).collect();
    assert_eq!(
        targets,
        [
            "/api/runners/jobs/",
            "/api/runners/jobs/?before=812",
            "/api/runners/jobs/?before=812",
        ]
    );
    let run = text(&server, &["jobs"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(run.stdout().contains("compute_runway"), "{}", run.stdout());
}

#[test]
fn jobs_all_follows_the_cursor_until_it_is_null() {
    let server = Server::start(vec![
        jobs("?before=3", r#"{"results":[{"id":2}],"next_before":2}"#),
        jobs("?before=2", r#"{"results":[{"id":1}],"next_before":null}"#),
        jobs("", r#"{"results":[{"id":3}],"next_before":3}"#),
    ]);
    let all = hipc(&server, &["jobs", "--all"]).ok();
    assert_eq!(all, json!([{"id": 3}, {"id": 2}, {"id": 1}]));
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn jobs_all_stops_on_a_repeated_cursor() {
    let server = Server::start(vec![
        jobs("?before=9", r#"{"results":[],"next_before":8}"#),
        jobs("?before=8", r#"{"results":[],"next_before":9}"#),
        jobs("", r#"{"results":[],"next_before":9}"#),
    ]);
    let error = hipc(&server, &["jobs", "--all"]).fails(1, "internal");
    assert_eq!(
        error["message"],
        "the API's next page leads back to `9`, which was already read"
    );
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn jobs_all_stops_after_a_thousand_pages() {
    let server = crate::common::endless_pages(&api("/runners/jobs/"), |served| {
        format!(r#"{{"results":[],"next_before":{}}}"#, served + 1)
    });
    let error = Hipc::against(&Home::new(), &server)
        .run(&runners(&["jobs", "--all"]))
        .fails(1, "internal");
    assert!(
        error["message"].as_str().unwrap().contains("1000 pages"),
        "{error}"
    );
    assert_eq!(server.requests().len(), 1000);
}

#[test]
fn jobs_refuses_a_limit_a_bad_token_and_a_page_that_is_not_a_cursor_page() {
    let server = Server::start(vec![]);
    for args in [
        &["jobs", "--limit", "5"][..],
        &["jobs", "--page-token", "abc"],
        &["jobs", "--page-token", "-1"],
        &["jobs", "--page-token", ""],
        &["jobs", "--page-token", "99999999999999999999"],
        &["jobs", "--page-token", "3", "--all"],
    ] {
        hipc(&server, args).fails(2, "usage");
    }
    let error = Hipc::tokenless(&Home::new(), &server)
        .run(&runners(&["jobs", "--page-token", "x"]))
        .fails(2, "usage");
    assert!(
        error["message"].as_str().unwrap().contains("a cursor"),
        "{error}"
    );
    assert!(server.requests().is_empty());
    for body in [
        r#"{"results":[],"next_before":"812"}"#,
        r#"{"results":[]}"#,
        r#"{"count":0,"next":null,"previous":null,"results":[]}"#,
        "[]",
    ] {
        let odd = Server::start(vec![jobs("", body)]);
        hipc(&odd, &["jobs"]).fails(1, "internal");
    }
}

#[test]
fn connect_prints_the_url_document_and_says_the_console_finishes_the_link() {
    let server = Server::start(vec![route(
        "POST",
        "/api/runners/github/connect/",
        Reply::json(201, CONNECT),
    )]);
    assert_eq!(hipc(&server, &["connect"]).ok(), parsed(CONNECT));
    let request = server.only_request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/api/runners/github/connect/");
    assert_eq!(request.body, b"");
    let run = text(&server, &["connect"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(run.stdout().contains("hippius-runners"), "{}", run.stdout());
    assert_eq!(
        run.stderr(),
        "note: open the `url` in the output to install the Hippius Runners GitHub App; nothing is \
         linked until the App is installed and the console finishes the link\n"
    );
    let quiet = text(&server, &["connect", "--quiet"]);
    assert_eq!(quiet.code(), Some(0), "{}", quiet.stderr());
    assert_eq!(quiet.stderr(), "");
}

#[test]
fn connect_errors_map_to_their_codes() {
    for (status, code, effects) in [(403, "permission", "none"), (503, "unavailable", "partial")] {
        let server = Server::start(vec![route(
            "POST",
            "/api/runners/github/connect/",
            Reply::json(status, r#"{"error":"x"}"#),
        )]);
        let run = text(&server, &["connect"]);
        assert_eq!(run.code(), Some(1), "{}", run.stderr());
        assert!(!run.stderr().contains("open the"), "{}", run.stderr());
        let error = hipc(&server, &["connect"]).fails(1, code);
        assert_eq!(error["side_effects"], effects, "{status}");
        assert_eq!(error["retryable"], false, "{status}");
    }
}

#[test]
fn a_membership_sends_the_account_header_on_every_route() {
    let server = Server::start(vec![
        route("GET", "/api/runners/settings/", Reply::json(200, SETTINGS)),
        route("PUT", "/api/runners/settings/", Reply::json(200, SETTINGS)),
        route("GET", "/api/runners/installations/", Reply::json(200, "[]")),
        route(
            "DELETE",
            "/api/runners/installations/7/",
            Reply::json(204, ""),
        ),
        jobs("", r#"{"results":[],"next_before":null}"#),
        route(
            "POST",
            "/api/runners/github/connect/",
            Reply::json(201, CONNECT),
        ),
    ]);
    for args in [
        &["settings", "get"][..],
        &["settings", "update", "--enabled", "true", "--yes"],
        &["installations", "list"],
        &["installations", "delete", "7", "--yes"],
        &["jobs"],
        &["connect"],
    ] {
        Hipc::against(&Home::new(), &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&runners(args))
            .ok();
        let request = server.requests().pop().expect("a request per command");
        assert_eq!(
            request.header("x-hippius-account"),
            Some(MEMBER),
            "{args:?}"
        );
    }
    // The settings update reads the settings first.
    assert_eq!(server.requests().len(), 7);
    let own = Server::start(vec![jobs("", r#"{"results":[],"next_before":null}"#)]);
    hipc(&own, &["jobs"]).ok();
    assert_eq!(own.only_request().header("x-hippius-account"), None);
}

#[test]
fn these_runners_commands_dry_run_without_a_token_or_a_request() {
    let server = Server::start(vec![]);
    for (args, method, path) in [
        (&["settings", "get"][..], "GET", "/api/runners/settings/"),
        (
            &["settings", "update", "--max-concurrent", "4"],
            "PUT",
            "/api/runners/settings/",
        ),
        (
            &["installations", "list"],
            "GET",
            "/api/runners/installations/",
        ),
        (
            &["installations", "delete", "7"],
            "DELETE",
            "/api/runners/installations/7/",
        ),
        (&["jobs"], "GET", "/api/runners/jobs/"),
        (&["jobs", "--all"], "GET", "/api/runners/jobs/"),
        (
            &["jobs", "--page-token", "812"],
            "GET",
            "/api/runners/jobs/?before=812",
        ),
        (&["connect"], "POST", "/api/runners/github/connect/"),
    ] {
        let mut argv = runners(args);
        argv.push("--dry-run");
        let doc = Hipc::tokenless(&Home::new(), &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&argv)
            .ok();
        assert_eq!(doc["dry_run"], true, "{args:?}");
        assert_eq!(draft(&doc)["method"], method, "{args:?}");
        assert_eq!(
            draft(&doc)["url"],
            format!("{}{path}", server.url()),
            "{args:?}"
        );
        assert_eq!(draft(&doc)["account_header"], MEMBER, "{args:?}");
    }
    let doc = hipc(
        &server,
        &["settings", "update", "--max-concurrent", "4", "--dry-run"],
    )
    .ok();
    assert_eq!(draft(&doc)["body"], json!({"max_concurrent": 4}));
    let run = text(&server, &["connect", "--dry-run"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(run.stderr(), "");
    assert!(server.requests().is_empty());
}

/// The runner spend over a period (values made up; swagger types no body).
const USAGE: &str = r#"{"period":"2026-11","total_credits":"3.25","jobs":4}"#;

/// 20 credits in base units, as the CDN cap writes `monthly_cap`.
const BASE_UNITS: &str = "20000000000000000000";

/// A limit answer shaped as the CDN cap's: `monthly_cap` in base units and the credits in
/// `monthly_cap_credits`, given as a JSON value; `null` makes both `null`. hipc never reads the
/// base units, so they stay the same whatever the credits say.
fn limit(credits: &str) -> String {
    let base = if credits == "null" {
        "null".to_owned()
    } else {
        format!("\"{BASE_UNITS}\"")
    };
    format!(r#"{{"monthly_cap":{base},"monthly_cap_credits":{credits},"spent_this_month":"1.50"}}"#)
}

/// A server whose limit reads `answer` and that answers a PUT with `put`.
fn limit_server(answer: &str, put: &str) -> Server {
    Server::start(vec![
        route("GET", "/api/runners/limit/", Reply::json(200, answer)),
        route("PUT", "/api/runners/limit/", Reply::json(200, put)),
    ])
}

/// The limit a server that applied the `PUT` of `body` answers with.
fn applied(body: &Value) -> String {
    match &body["monthly_cap"] {
        Value::String(credits) => limit(&format!("\"{credits}\"")),
        _ => limit("null"),
    }
}

fn update(cap: &str) -> Vec<&str> {
    vec!["limit", "update", "--monthly-cap", cap]
}

fn with_yes<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut argv = args.to_vec();
    argv.push("--yes");
    argv
}

fn methods(server: &Server) -> Vec<String> {
    server.requests().into_iter().map(|r| r.method).collect()
}

const NOT_ENABLED: &str = "the runner spend limit is not enabled on this server; run `hipc compute \
                           runners settings get`, then lower `--max-concurrent` or \
                           `--allowed-size` with `hipc compute runners settings update`";

const SET_UNKNOWN: &str = "set the monthly runner spend cap to `50` credits, where the current \
                           cap is unknown: the limit has no amount in `monthly_cap_credits`";

const REMOVE_UNKNOWN: &str = "remove the monthly runner spend cap, whose current value is \
                              unknown: the limit has no amount in `monthly_cap_credits`";

/// Asserts that `--monthly-cap <cap>` over `answer` asks `what` after the read and sends
/// nothing, then sends `body` with `--yes`.
fn asks(answer: &str, cap: &str, what: &str, body: &Value) {
    let server = limit_server(answer, &applied(body));
    let error = hipc(&server, &update(cap)).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!("this would {what}, and `--yes` was not passed"),
        "{answer} {cap}"
    );
    assert_eq!(methods(&server), ["GET"], "{answer} {cap}");
    let doc = hipc(&server, &with_yes(&update(cap))).ok();
    assert_eq!(doc, parsed(&applied(body)), "{answer} {cap}");
    let sent = server.requests();
    assert_eq!(methods(&server), ["GET", "GET", "PUT"], "{answer} {cap}");
    assert_eq!(sent[2].target, "/api/runners/limit/");
    assert_eq!(&sent[2].json(), body, "{answer} {cap}");
}

#[test]
fn usage_prints_the_spend_as_sent() {
    let server = Server::start(vec![route(
        "GET",
        "/api/runners/usage/",
        Reply::json(200, USAGE),
    )]);
    assert_eq!(hipc(&server, &["usage"]).ok(), parsed(USAGE));
    let request = server.only_request();
    assert_eq!(request.method, "GET");
    assert_eq!(request.target, "/api/runners/usage/");
    let run = text(&server, &["usage"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(run.stdout().contains("3.25"), "{}", run.stdout());
    let failing = Server::start(vec![route(
        "GET",
        "/api/runners/usage/",
        Reply::json(403, r#"{"error":"x"}"#),
    )]);
    let error = hipc(&failing, &["usage"]).fails(1, "permission");
    assert_eq!(error["side_effects"], "none");
}

#[test]
fn limit_get_prints_the_limit_as_sent() {
    let body = limit("\"50.000000\"");
    let server = Server::start(vec![route(
        "GET",
        "/api/runners/limit/",
        Reply::json(200, &body),
    )]);
    assert_eq!(hipc(&server, &["limit", "get"]).ok(), parsed(&body));
    let request = server.only_request();
    assert_eq!(request.method, "GET");
    assert_eq!(request.target, "/api/runners/limit/");
    let run = text(&server, &["limit", "get"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(run.stdout().contains("50.000000"), "{}", run.stdout());
}

/// Swagger: the limit answers 404 while the server's runner usage limits are off. Only a
/// read of the limit can tell that; a 404 on the write after a good read is plain.
#[test]
fn a_404_on_reading_the_limit_is_not_found_with_a_hint_that_it_is_not_enabled() {
    let off = Server::start(vec![route(
        "GET",
        "/api/runners/limit/",
        Reply::not_found(),
    )]);
    let error = hipc(&off, &["limit", "get"]).fails(1, "not_found");
    assert_eq!(error["http_status"], 404);
    assert_eq!(error["hint"], NOT_ENABLED);
    let run = text(&off, &["limit", "get"]);
    assert_eq!(run.code(), Some(1));
    assert!(
        run.stderr().contains(&format!("hint: {NOT_ENABLED}\n")),
        "{}",
        run.stderr()
    );
    // The update's read meets the same 404, and nothing is written.
    let error = hipc(&off, &with_yes(&update("50"))).fails(1, "not_found");
    assert_eq!(error["hint"], NOT_ENABLED);
    assert_eq!(error["side_effects"], "none");
    assert!(methods(&off).iter().all(|m| m == "GET"));
    let late = Server::start(vec![
        route(
            "GET",
            "/api/runners/limit/",
            Reply::json(200, &limit("\"20\"")),
        ),
        route("PUT", "/api/runners/limit/", Reply::not_found()),
    ]);
    let error = hipc(&late, &update("5")).fails(1, "not_found");
    assert_eq!(error["http_status"], 404);
    let hint = error["hint"].as_str().unwrap_or_default();
    assert!(!hint.contains("not enabled"), "{error}");
    assert_eq!(methods(&late), ["GET", "PUT"]);
    // Another error on the limit keeps its own hint.
    let denied = Server::start(vec![route(
        "GET",
        "/api/runners/limit/",
        Reply::json(403, r#"{"code":"insufficient_grant","error":"x"}"#),
    )]);
    let error = hipc(&denied, &["limit", "get"]).fails(1, "insufficient_grant");
    assert_ne!(error["hint"], NOT_ENABLED);
}

/// A raise, or a removal of a cap in force, allows more spend and needs `--yes`; the
/// question names the cap read from `monthly_cap_credits` and the new one. The base units in
/// `monthly_cap` are a far larger number, so a comparison with them would call each raise a
/// cut.
#[test]
fn limit_update_needs_yes_when_it_raises_or_removes_the_cap() {
    let cases: [(&str, &str, &str, Value); 8] = [
        (
            "\"20.000000\"",
            "50",
            "raise the monthly runner spend cap from `20.000000` to `50` credits",
            json!({"monthly_cap": "50"}),
        ),
        (
            "\"20\"",
            "none",
            "remove the monthly runner spend cap of `20` credits",
            json!({"monthly_cap": null}),
        ),
        // Exact decimals: no float decides a raise.
        (
            "\"0.45\"",
            "0.5",
            "raise the monthly runner spend cap from `0.45` to `0.5` credits",
            json!({"monthly_cap": "0.5"}),
        ),
        (
            "9",
            "10",
            "raise the monthly runner spend cap from `9` to `10` credits",
            json!({"monthly_cap": "10"}),
        ),
        // A leading zero does not make a cap longer.
        (
            "\"050\"",
            "60",
            "raise the monthly runner spend cap from `050` to `60` credits",
            json!({"monthly_cap": "60"}),
        ),
        (
            "\"0\"",
            "0.01",
            "raise the monthly runner spend cap from `0` to `0.01` credits",
            json!({"monthly_cap": "0.01"}),
        ),
        // The amount sent and shown is the normal form of the one given.
        (
            "\"20\"",
            "050.500",
            "raise the monthly runner spend cap from `20` to `50.5` credits",
            json!({"monthly_cap": "50.5"}),
        ),
        // A cap that is not an amount still asks before it is removed, quoted as the
        // server's words.
        (
            "\"lots`\"",
            "none",
            "remove the monthly runner spend cap, where `monthly_cap_credits` is \
             `lots\\u{60}`, which is not an amount of credits",
            json!({"monthly_cap": null}),
        ),
    ];
    for (credits, cap, what, body) in cases {
        asks(&limit(credits), cap, what, &body);
    }
}

/// `null` is unconfirmed to mean no cap, and may be a server default, so a cap set over it
/// asks. Without an amount in `monthly_cap_credits` the cap in force is unknown, whatever
/// `monthly_cap` holds, so every change asks, `none` included.
#[test]
fn limit_update_needs_yes_when_the_cap_in_force_is_null_or_unknown() {
    let base_only = format!(r#"{{"monthly_cap":"{BASE_UNITS}"}}"#);
    let base_and_null = format!(r#"{{"monthly_cap":"{BASE_UNITS}","monthly_cap_credits":null}}"#);
    asks(
        &limit("null"),
        "50",
        "set the monthly runner spend cap to `50` credits, where the limit shows `null`, unset \
         or a server default",
        &json!({"monthly_cap": "50"}),
    );
    // The CDN shape without the credits: base units alone must not read as 20 quintillion
    // credits, under which `50` would be a cut.
    for answer in [
        base_only.as_str(),
        &base_and_null,
        r#"{"spent_this_month":"1.50"}"#,
        r#"{"monthly_cap_credits":null}"#,
        // A missing `monthly_cap_credits` is not `null`: the cap in force is unknown.
        r#"{"monthly_cap":null}"#,
        // A `null` beside credits hipc cannot read is no proof that no cap is set.
        r#"{"monthly_cap":null,"monthly_cap_credits":true}"#,
        "[]",
    ] {
        asks(answer, "50", SET_UNKNOWN, &json!({"monthly_cap": "50"}));
        asks(
            answer,
            "none",
            REMOVE_UNKNOWN,
            &json!({"monthly_cap": null}),
        );
    }
    // A `monthly_cap_credits` that is not a decimal amount is no cap to compare with, and
    // the question names it.
    for (credits, shown) in [
        ("\"-5\"", "`-5`"),
        ("-5", "`-5`"),
        ("\"5.\"", "`5.`"),
        ("\".5\"", "`.5`"),
        ("\"5.x\"", "`5.x`"),
        ("\"5e1\"", "`5e1`"),
        ("1e300", "`1e+300`"),
        ("\"\"", "``"),
    ] {
        let bad =
            format!("where `monthly_cap_credits` is {shown}, which is not an amount of credits");
        let set = format!("set the monthly runner spend cap to `50` credits, {bad}");
        asks(&limit(credits), "50", &set, &json!({"monthly_cap": "50"}));
        let remove = format!("remove the monthly runner spend cap, {bad}");
        asks(
            &limit(credits),
            "none",
            &remove,
            &json!({"monthly_cap": null}),
        );
    }
    // One that is not a string or a number is no amount at all.
    for credits in ["true", r#"{"credits":"5"}"#, "[5]"] {
        asks(
            &limit(credits),
            "50",
            SET_UNKNOWN,
            &json!({"monthly_cap": "50"}),
        );
    }
}

/// A lower cap, the same cap, or `none` over a `null` cap allows no more spend, so it is
/// sent without `--yes`, after the read, in its normal form.
#[test]
fn limit_update_needs_no_yes_when_it_lowers_or_keeps_the_cap() {
    let cases: [(String, &str, Value); 12] = [
        (limit("\"20.000000\""), "5", json!({"monthly_cap": "5"})),
        (limit("\"10\""), "9.99", json!({"monthly_cap": "9.99"})),
        (limit("20.5"), "20.50", json!({"monthly_cap": "20.5"})),
        (limit("\"050\""), "50.0", json!({"monthly_cap": "50"})),
        (limit("\"50\""), "050.00", json!({"monthly_cap": "50"})),
        (limit("\"50\""), "50", json!({"monthly_cap": "50"})),
        (limit("\"0.5\""), "0.45", json!({"monthly_cap": "0.45"})),
        (limit("\"1\""), "000.000", json!({"monthly_cap": "0"})),
        (limit("\"1\""), "0.10", json!({"monthly_cap": "0.1"})),
        (limit("null"), "none", json!({"monthly_cap": null})),
        // 18 decimals is a credit's smallest part; trailing zeros past it are dropped.
        (
            limit("\"20\""),
            "0.000000000000000001",
            json!({"monthly_cap": "0.000000000000000001"}),
        ),
        (
            limit("\"20\""),
            "5.000000000000000000000",
            json!({"monthly_cap": "5"}),
        ),
    ];
    for (answer, cap, body) in cases {
        let server = limit_server(&answer, &applied(&body));
        let doc = hipc(&server, &update(cap)).ok();
        assert_eq!(doc, parsed(&applied(&body)), "{answer} {cap}");
        let sent = server.requests();
        assert_eq!(methods(&server), ["GET", "PUT"], "{answer} {cap}");
        assert_eq!(sent[1].json(), body, "{answer} {cap}");
    }
    let server = limit_server(&limit("\"20\""), &limit("\"5\""));
    let run = text(&server, &update("5"));
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(run.stdout().contains("1.50"), "{}", run.stdout());
}

/// A 2xx to the `PUT` that does not show the cap sent leaves the change unconfirmed: a server
/// that ignores `monthly_cap` answers 200 with the cap it kept. It is `internal`, `partial`,
/// and not retryable, with nothing on stdout, the answer in `details`, and a hint that reads
/// the limit instead of sending the `PUT` again. The answer's cap is compared as an exact
/// decimal, so `5.000000` and the number `5` show the cap `5`.
#[test]
fn limit_update_refuses_an_answer_that_does_not_show_the_cap_sent() {
    let set = "set the monthly runner spend cap to `5` credits";
    let removed = "remove the monthly runner spend cap of `20` credits";
    let hint = "run `hipc compute runners limit get` to see the cap in force, and report what \
                it prints at `https://github.com/thenervelab/hippius-cli/issues`";
    let without = "the API answered without an amount in `monthly_cap_credits`";
    let base_and_null = format!(r#"{{"monthly_cap":"{BASE_UNITS}","monthly_cap_credits":null}}"#);
    for (cap, put, action, why) in [
        (
            "5",
            limit("\"20\""),
            set,
            "the API answered `monthly_cap_credits` `20`",
        ),
        (
            "5",
            limit("5.5"),
            set,
            "the API answered `monthly_cap_credits` `5.5`",
        ),
        (
            "5",
            limit("\"5 credits\""),
            set,
            "the API answered `monthly_cap_credits` `5 credits`",
        ),
        ("5", limit("null"), set, "the API answered a cap of `null`"),
        ("5", r#"{"monthly_cap":"5"}"#.to_owned(), set, without),
        ("5", "[]".to_owned(), set, without),
        (
            "none",
            limit("\"20\""),
            removed,
            "the API answered `monthly_cap_credits` `20`",
        ),
        // `null` credits beside a cap in base units is not a removal, and neither is an
        // answer that leaves either field out.
        ("none", base_and_null.clone(), removed, without),
        (
            "none",
            r#"{"monthly_cap":null}"#.to_owned(),
            removed,
            without,
        ),
        (
            "none",
            r#"{"monthly_cap_credits":null}"#.to_owned(),
            removed,
            without,
        ),
    ] {
        let server = limit_server(&limit("\"20\""), &put);
        let run = Hipc::against(&Home::new(), &server).run(&runners(&with_yes(&update(cap))));
        let error = run.fails(1, "internal");
        assert_eq!(
            error["message"],
            format!("cannot {action}: {why}"),
            "{cap} {put}"
        );
        assert_eq!(error["hint"], hint, "{cap} {put}");
        assert_eq!(error["side_effects"], "partial", "{cap} {put}");
        assert_eq!(error["retryable"], false, "{cap} {put}");
        assert_eq!(error["http_status"], 200, "{cap} {put}");
        assert_eq!(error["details"], parsed(&put), "{cap} {put}");
        assert_eq!(run.stdout(), "", "{cap} {put}");
        assert_eq!(methods(&server), ["GET", "PUT"], "{cap} {put}");
    }
    // An empty 2xx is refused before the cap is looked at, with the default hint.
    let empty = limit_server(&limit("\"20\""), "");
    let error = hipc(&empty, &update("5")).fails(1, "internal");
    assert_eq!(error["side_effects"], "partial");
    assert_ne!(error["hint"], hint);
    // The amount the answer shows, in any exact spelling of the one sent, is the change.
    for (cap, put) in [
        ("5", limit("5")),
        ("5", limit("\"5.000000\"")),
        ("5", limit("\"005\"")),
        ("0.1", limit("\"0.10\"")),
        ("none", limit("null")),
    ] {
        let server = limit_server(&limit("\"20\""), &put);
        let doc = hipc(&server, &with_yes(&update(cap))).ok();
        assert_eq!(doc, parsed(&put), "{cap} {put}");
    }
}

#[test]
fn limit_update_refuses_a_bad_cap_before_any_request_or_token() {
    let server = Server::start(vec![]);
    for cap in [
        "", "-1", "1e3", "5.", ".5", "NONE", "None", "50 ", "5,0", "x",
    ] {
        // `=` keeps a value that starts with `-` from reading as a flag.
        let flag = format!("--monthly-cap={cap}");
        let error = Hipc::tokenless(&Home::new(), &server)
            .run(&runners(&["limit", "update", &flag, "--yes"]))
            .fails(2, "usage");
        assert!(
            error["message"].as_str().unwrap().contains(
                "a cap is a decimal number of credits, such as `50` or `20.5`, or `none`"
            ),
            "{cap}: {error}"
        );
    }
    // A credit has 18 decimals, so a finer cap is refused too, whatever trailing zeros follow.
    for cap in [
        "0.0000000000000000001",
        "1.12345678901234567890",
        "5.0000000000000000001000",
    ] {
        let flag = format!("--monthly-cap={cap}");
        let error = hipc(&server, &["limit", "update", &flag, "--yes"]).fails(2, "usage");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("a cap has at most 18 digits after the point"),
            "{cap}: {error}"
        );
    }
    hipc(&server, &["limit", "update", "--yes"]).fails(2, "usage");
    assert!(server.requests().is_empty());
}

#[test]
fn limit_update_errors_map_to_their_codes() {
    for (status, code, effects) in [
        (400, "validation", "none"),
        (403, "permission", "none"),
        (502, "unavailable", "partial"),
    ] {
        let server = Server::start(vec![
            route(
                "GET",
                "/api/runners/limit/",
                Reply::json(200, &limit("\"20\"")),
            ),
            route(
                "PUT",
                "/api/runners/limit/",
                Reply::json(status, r#"{"error":"x"}"#),
            ),
        ]);
        let error = hipc(&server, &update("5")).fails(1, code);
        assert_eq!(error["http_status"], status);
        assert_eq!(error["side_effects"], effects, "{status}");
        assert_ne!(error["hint"], NOT_ENABLED, "{status}");
    }
}

/// The limit and usage routes are account-scoped and take a project, as the other runner
/// routes do (tenet 12): both go on the read before the update's question too, and the
/// question names the membership.
#[test]
fn the_limit_and_usage_routes_take_a_membership_and_a_project() {
    let server = Server::start(vec![
        route("GET", "/api/runners/usage/", Reply::json(200, USAGE)),
        route(
            "GET",
            "/api/runners/limit/",
            Reply::json(200, &limit("\"20\"")),
        ),
        route(
            "PUT",
            "/api/runners/limit/",
            Reply::json(200, &limit("\"5\"")),
        ),
    ]);
    let home = Home::new();
    let scoped = |args: &[&str]| {
        Hipc::against(&home, &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .env("HIPPIUS_PROJECT", PROJECT)
            .run(&runners(args))
    };
    scoped(&["usage"]).ok();
    scoped(&["limit", "get"]).ok();
    scoped(&update("5")).ok();
    // The PUT answers the cap `5`, so the raise to `50` is not shown as made; its requests
    // still carry both headers.
    scoped(&with_yes(&update("50"))).fails(1, "internal");
    let error = scoped(&update("50")).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!(
            "this would raise the monthly runner spend cap from `20` to `50` credits in the \
             account `{MEMBER}` and the project `{PROJECT}`, and `--yes` was not passed"
        )
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 7);
    for request in &requests {
        assert_eq!(
            request.header("x-hippius-account"),
            Some(MEMBER),
            "{}",
            request.target
        );
        assert_eq!(
            request.header("x-hippius-project"),
            Some(PROJECT),
            "{}",
            request.target
        );
    }
}

#[test]
fn the_limit_and_usage_commands_dry_run_without_a_token_or_a_request() {
    let server = Server::start(vec![]);
    let dry = |args: &[&str]| {
        let mut argv = runners(args);
        argv.push("--dry-run");
        Hipc::tokenless(&Home::new(), &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&argv)
            .ok()
    };
    let url = format!("{}/api/runners/limit/", server.url());
    for (args, path) in [
        (&["usage"][..], "/api/runners/usage/"),
        (&["limit", "get"], "/api/runners/limit/"),
    ] {
        let doc = dry(args);
        assert_eq!(doc["dry_run"], true, "{args:?}");
        assert_eq!(draft(&doc)["method"], "GET", "{args:?}");
        assert_eq!(
            draft(&doc)["url"],
            format!("{}{path}", server.url()),
            "{args:?}"
        );
        assert_eq!(draft(&doc)["account_header"], MEMBER, "{args:?}");
    }
    // The update lists the read that decides whether it asks, then the write, and needs no
    // `--yes` to print that.
    for (cap, body) in [
        ("50", json!({"monthly_cap": "50"})),
        ("none", json!({"monthly_cap": null})),
    ] {
        let doc = dry(&update(cap));
        assert_eq!(draft(&doc)["method"], "PUT", "{cap}");
        assert_eq!(draft(&doc)["url"], url, "{cap}");
        assert_eq!(draft(&doc)["body"], body, "{cap}");
        assert_eq!(draft(&doc)["account_header"], MEMBER, "{cap}");
        let requests = doc["requests"].as_array().unwrap();
        assert_eq!(requests.len(), 2, "{cap}");
        assert_eq!(requests[0]["method"], "GET");
        assert_eq!(requests[0]["url"], url);
        assert_eq!(requests[0]["body"], Value::Null);
        assert_eq!(requests[1]["method"], "PUT");
        assert_eq!(requests[1]["body"], body);
    }
    assert!(server.requests().is_empty());
}
