//! `hipc hub repositories` and `hipc hub artifacts`: the registry's repositories and the
//! artifacts in them.

use hippius::routes::hub;
use serde_json::{Value, json};

use crate::common::{
    Hipc, Home, MEMBER, Reply, Server, draft, hub_provision_hint, one_request_dry_run, route,
    route_of,
};

const REPOSITORY: &str = r#"{"id":4,"name":"acme/app","artifact_count":2,"pull_count":9}"#;

const ARTIFACT: &str = concat!(
    r#"{"id":8,"digest":"sha256:0a1b","size":1024,"#,
    r#""tags":[{"id":1,"name":"v1","immutable":false}]}"#
);

const NO_PROJECT: &str = r#"{"detail":"You must call /api/registry/provision/ first."}"#;

fn repositories(body: &str) -> Server {
    Server::start(vec![route_of(
        &hub::REPOSITORIES,
        [],
        Reply::json(200, body),
    )])
}

/// The console reads the list as a bare array or a DRF page, so `hipc` reads both; a page's
/// `count` stays beside `items`.
#[test]
fn repositories_list_is_an_envelope_of_either_shape() {
    let row: Value = serde_json::from_str(REPOSITORY).unwrap();
    for (body, envelope) in [
        (
            format!("[{REPOSITORY}]"),
            json!({"items": [row], "next_page_token": null}),
        ),
        (
            format!(r#"{{"count":1,"next":null,"previous":null,"results":[{REPOSITORY}]}}"#),
            json!({"count": 1, "items": [row], "next_page_token": null}),
        ),
    ] {
        let server = repositories(&body);
        let doc = Hipc::against(&Home::new(), &server)
            .run(&["hub", "repositories", "list"])
            .ok();
        assert_eq!(doc, envelope);
        assert_eq!(server.only_request().header("x-hippius-account"), None);
    }
}

/// Swagger lists no page parameter for either list, so `hipc` reads each whole and does not
/// print a short list as if it were complete.
#[test]
fn the_lists_refuse_a_second_page() {
    let page = |path: &str| {
        format!(r#"{{"count":2,"next":"https://x{path}?page=2","previous":null,"results":[]}}"#)
    };
    for (args, path) in [
        (
            &["hub", "repositories", "list"][..],
            "/api/registry/repositories/",
        ),
        (
            &["hub", "repositories", "get", "acme/app"],
            "/api/registry/repositories/",
        ),
        (
            &["hub", "artifacts", "list", "acme/app"],
            "/api/registry/repositories/acme/app/artifacts/",
        ),
    ] {
        let server = Server::start(vec![route("GET", path, Reply::json(200, &page(path)))]);
        let error = Hipc::against(&Home::new(), &server)
            .run(args)
            .fails(1, "internal");
        // The hint names the route, as `hipc api` takes it, so it runs as written.
        let route = path.strip_prefix("/api").unwrap();
        assert_eq!(
            error["hint"],
            format!("run `hipc api GET {route}` to see the answer and its `next` link"),
            "{args:?}"
        );
        assert_eq!(server.requests().len(), 1, "{args:?}");
    }
}

/// Swagger has no GET of one repository, so `get` reads the list and prints the row whose
/// `name` is the one given, namespace included.
#[test]
fn repositories_get_prints_the_row_with_that_name() {
    let other = r#"{"id":5,"name":"acme/app-two","artifact_count":0}"#;
    let server = repositories(&format!("[{other},{REPOSITORY}]"));
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "repositories", "get", "acme/app"])
        .ok();
    assert_eq!(doc, serde_json::from_str::<Value>(REPOSITORY).unwrap());
    assert_eq!(server.only_request().path(), "/api/registry/repositories/");
}

#[test]
fn repositories_get_of_a_name_the_list_lacks_is_not_found() {
    let server = repositories(&format!("[{REPOSITORY}]"));
    let error = Hipc::against(&Home::new(), &server)
        .run(&["hub", "repositories", "get", "acme/other"])
        .fails(1, "not_found");
    assert_eq!(
        error["message"],
        "the Hub registry has no repository `acme/other`"
    );
    assert_eq!(
        error["hint"],
        "run `hipc hub repositories list` to see each repository's name"
    );
}

/// A row without a string `name` cannot be the one asked for, so it is passed over.
#[test]
fn repositories_get_passes_over_a_row_without_a_name() {
    let server = repositories(&format!(r#"[{{"id":1}},{REPOSITORY}]"#));
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "repositories", "get", "acme/app"])
        .ok();
    assert_eq!(doc["id"], 4);
}

/// A repository name holds its namespace and a `/`, which stays a path separator, as the
/// console sends it.
#[test]
fn artifacts_list_reads_the_repository_path() {
    let server = Server::start(vec![route_of(
        &hub::ARTIFACTS,
        [&"acme/team/app"],
        Reply::json(200, &format!(r#"{{"count":1,"results":[{ARTIFACT}]}}"#)),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "artifacts", "list", "acme/team/app"])
        .ok();
    assert_eq!(doc["count"], 1);
    assert_eq!(doc["items"][0]["digest"], "sha256:0a1b");
    assert_eq!(doc["next_page_token"], Value::Null);
}

/// Swagger: "`reference` can be either a tag or a digest".
#[test]
fn artifacts_get_takes_a_tag_or_a_digest() {
    for reference in ["v1", "sha256:0a1b"] {
        let path = format!("/api/registry/repositories/acme/app/artifacts/{reference}/");
        let server = Server::start(vec![route("GET", &path, Reply::json(200, ARTIFACT))]);
        let doc = Hipc::against(&Home::new(), &server)
            .run(&["hub", "artifacts", "get", "acme/app", reference])
            .ok();
        assert_eq!(doc, serde_json::from_str::<Value>(ARTIFACT).unwrap());
    }
}

#[test]
fn an_artifact_the_server_lacks_hints_at_the_list() {
    let server = Server::start(vec![route_of(
        &hub::ARTIFACT,
        [&"acme/app", &"v9"],
        Reply::json(404, r#"{"detail":"Not found."}"#),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["hub", "artifacts", "get", "acme/app", "v9"])
        .fails(1, "not_found");
    assert!(
        error["hint"]
            .as_str()
            .unwrap()
            .contains("hipc hub artifacts list acme/app"),
        "{error}"
    );
}

#[test]
fn every_read_without_a_project_hints_at_namespace_create() {
    for (args, path) in [
        (
            &["hub", "repositories", "list"][..],
            "/api/registry/repositories/",
        ),
        // `get` reads the repository from the list.
        (
            &["hub", "repositories", "get", "acme/app"],
            "/api/registry/repositories/",
        ),
        (
            &["hub", "artifacts", "list", "acme/app"],
            "/api/registry/repositories/acme/app/artifacts/",
        ),
        (
            &["hub", "artifacts", "get", "acme/app", "v1"],
            "/api/registry/repositories/acme/app/artifacts/v1/",
        ),
    ] {
        let server = Server::start(vec![route("GET", path, Reply::json(403, NO_PROJECT))]);
        let error = Hipc::against(&Home::new(), &server)
            .run(args)
            .fails(1, "permission");
        assert_eq!(error["hint"], hub_provision_hint(&server), "{args:?}");
    }
}

/// The deletes: each argv, its DELETE path, and the document a 204 prints.
fn deletes() -> [(Vec<&'static str>, &'static str, Value); 3] {
    [
        (
            vec!["hub", "repositories", "delete", "acme/app"],
            "/api/registry/repositories/acme/app/",
            json!({"name": "acme/app", "deleted": true}),
        ),
        (
            vec!["hub", "artifacts", "delete", "acme/app", "v1"],
            "/api/registry/repositories/acme/app/artifacts/v1/",
            json!({"repository": "acme/app", "reference": "v1", "deleted": true}),
        ),
        (
            vec!["hub", "artifacts", "delete", "acme/app", "sha256:0a1b"],
            "/api/registry/repositories/acme/app/artifacts/sha256:0a1b/",
            json!({"repository": "acme/app", "reference": "sha256:0a1b", "deleted": true}),
        ),
    ]
}

/// `--yes` is the one confirmation of a Hub delete (tenet 5): the flag that once stood
/// beside it is gone, and passing it is a usage error that sends nothing.
#[test]
fn a_delete_takes_no_irreversible_flag() {
    for (args, _, _) in deletes() {
        let server = Server::start(vec![]);
        let mut argv = args.clone();
        argv.extend(["--irreversible", "--yes"]);
        Hipc::against(&Home::new(), &server)
            .run(&argv)
            .fails(2, "usage");
        assert!(server.requests().is_empty());
    }
}

/// Without `--yes` the refusal names what is lost.
#[test]
fn a_delete_without_yes_names_what_is_lost_and_sends_nothing() {
    let lost = [
        "this would delete the Hub repository `acme/app` with every artifact and tag in it, \
         and `--yes` was not passed",
        "this would delete the artifact `v1` from the Hub repository `acme/app`, with every \
         tag on it, and `--yes` was not passed",
        "this would delete the artifact `sha256:0a1b` from the Hub repository `acme/app`, \
         with every tag on it, and `--yes` was not passed",
    ];
    for ((args, _, _), message) in deletes().into_iter().zip(lost) {
        let server = Server::start(vec![]);
        let error = Hipc::against(&Home::new(), &server)
            .run(&args)
            .fails(2, "confirmation_required");
        assert_eq!(error["message"], message, "{args:?}");
        assert!(server.requests().is_empty());
    }
}

#[test]
fn a_confirmed_delete_sends_one_delete_and_prints_what_went() {
    for (args, path, done) in deletes() {
        let server = Server::start(vec![route("DELETE", path, Reply::json(204, ""))]);
        let mut argv = args.clone();
        argv.push("--yes");
        let doc = Hipc::against(&Home::new(), &server).run(&argv).ok();
        assert_eq!(doc, done, "{argv:?}");
        let request = server.only_request();
        assert_eq!(request.method, "DELETE");
        assert_eq!(request.path(), path);
        assert_eq!(request.header("x-hippius-account"), None);
    }
}

/// A delete refused with a 404 or a 409 changed nothing; one answered 502 or 503 may have
/// deleted the repository or the artifact, so it is `partial` and not retryable (tenet 4).
#[test]
fn a_failed_delete_says_whether_it_may_have_deleted() {
    for (args, path, _) in deletes() {
        for (status, code, effects) in [
            (404, "not_found", "none"),
            (409, "conflict", "none"),
            (502, "unavailable", "partial"),
            (503, "unavailable", "partial"),
        ] {
            let server = Server::start(vec![route(
                "DELETE",
                path,
                Reply::json(status, r#"{"detail":"x"}"#),
            )]);
            let argv = [&args[..], &["--yes"]].concat();
            let error = Hipc::against(&Home::new(), &server)
                .run(&argv)
                .fails(1, code);
            assert_eq!(error["http_status"], status, "{argv:?}");
            assert_eq!(error["side_effects"], effects, "{argv:?} {status}");
            assert_eq!(error["retryable"], false, "{argv:?} {status}");
            assert_eq!(server.requests().len(), 1);
        }
    }
}

/// A body on the 2xx is printed as sent, in place of the document `hipc` writes for a 204.
#[test]
fn a_delete_that_answers_a_body_prints_it() {
    let server = Server::start(vec![route_of(
        &hub::REPOSITORY_DELETE,
        [&"acme/app"],
        Reply::json(200, r#"{"deleted":"acme/app"}"#),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "repositories", "delete", "acme/app", "--yes"])
        .ok();
    assert_eq!(doc, json!({"deleted": "acme/app"}));
}

/// A dry run sends nothing, so it needs no `--yes` (tenet 5), and
/// lists its one request in `requests`.
#[test]
fn a_delete_dry_run_needs_no_flag_and_lists_the_delete() {
    for (args, path, _) in deletes() {
        let server = Server::start(vec![]);
        let mut argv = vec!["--dry-run"];
        argv.extend(&args);
        let doc = Hipc::against(&Home::new(), &server).run(&argv).dry_ok();
        assert_eq!(draft(&doc)["method"], "DELETE", "{argv:?}");
        assert_eq!(
            draft(&doc)["url"],
            format!("{}{path}", server.url()),
            "{argv:?}"
        );
        assert_eq!(doc, one_request_dry_run(draft(&doc).clone()), "{argv:?}");
        assert!(server.requests().is_empty());
    }
}

/// A read's dry run lists the one GET a real run sends; `repositories get` sends the list's.
#[test]
fn a_read_dry_run_lists_its_get_and_sends_nothing() {
    for (args, path) in [
        (
            &["hub", "repositories", "list"][..],
            "/api/registry/repositories/",
        ),
        (
            &["hub", "repositories", "get", "acme/app"],
            "/api/registry/repositories/",
        ),
        (
            &["hub", "artifacts", "list", "acme/app"],
            "/api/registry/repositories/acme/app/artifacts/",
        ),
        (
            &["hub", "artifacts", "get", "acme/app", "v1"],
            "/api/registry/repositories/acme/app/artifacts/v1/",
        ),
    ] {
        let server = Server::start(vec![]);
        let mut argv = args.to_vec();
        argv.push("--dry-run");
        let doc = Hipc::against(&Home::new(), &server).run(&argv).ok();
        assert_eq!(draft(&doc)["method"], "GET", "{argv:?}");
        assert_eq!(
            draft(&doc)["url"],
            format!("{}{path}", server.url()),
            "{argv:?}"
        );
        assert_eq!(doc["requests"].as_array().unwrap().len(), 1, "{argv:?}");
        assert!(server.requests().is_empty());
    }
}

/// Hub is hidden in another account, so every route under `/api/registry/` refuses a
/// membership before anything is sent, a dry run included (tenet 12).
#[test]
fn every_command_refuses_a_membership_before_sending() {
    let server = Server::start(vec![]);
    for args in [
        &["hub", "repositories", "list"][..],
        &["hub", "repositories", "get", "acme/app"],
        &["hub", "repositories", "delete", "acme/app", "--yes"],
        &["hub", "repositories", "delete", "acme/app", "--dry-run"],
        &["hub", "artifacts", "list", "acme/app"],
        &["hub", "artifacts", "get", "acme/app", "v1"],
        &["hub", "artifacts", "delete", "acme/app", "v1", "--yes"],
        &["hub", "artifacts", "delete", "acme/app", "v1", "--dry-run"],
    ] {
        Hipc::against(&Home::new(), &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(args)
            .fails(2, "account_scope_unsupported");
    }
    assert!(server.requests().is_empty());
}

/// The names go into the URL path, so anything that could change the route is refused
/// before anything is sent: a name without its namespace, an empty, `.`, or `..` part, a
/// character outside letters, digits, `.`, `_`, and `-`, and a part named `artifacts`, which
/// would also read as the artifact route.
#[test]
fn a_repository_name_that_could_change_the_route_is_usage() {
    let server = Server::start(vec![]);
    let long = format!("acme/{}", "a".repeat(251));
    for name in [
        "app",
        "",
        "acme/",
        "/app",
        "acme//app",
        "acme/../app",
        "acme/./app",
        "acme/a b",
        "acme/app?x=1",
        "acme/app#x",
        "acme/a%2fb",
        "acme/a\\b",
        "acme/artifacts/app",
        "acme/app/artifacts",
        &long,
    ] {
        for args in [
            &["hub", "repositories", "get", name][..],
            &["hub", "repositories", "delete", name, "--yes"],
            &["hub", "artifacts", "list", name],
        ] {
            let error = Hipc::against(&Home::new(), &server)
                .run(args)
                .fails(2, "usage");
            assert!(
                error["message"].as_str().unwrap().contains("repository"),
                "{args:?}: {error}"
            );
        }
    }
    assert!(server.requests().is_empty());
}

/// The longest name kept: 255 bytes, Docker's `RepositoryNameTotalLengthMax`; uppercase
/// letters, `.`, `_`, and `-` are left to the server to judge.
#[test]
fn a_repository_name_of_255_bytes_in_any_case_is_sent() {
    let name = format!("Acme/{}aa", "a._-".repeat(62));
    assert_eq!(name.len(), 255);
    let path = format!("/api/registry/repositories/{name}/artifacts/");
    let server = Server::start(vec![route("GET", &path, Reply::json(200, "[]"))]);
    Hipc::against(&Home::new(), &server)
        .run(&["hub", "artifacts", "list", &name])
        .ok();
    assert_eq!(server.only_request().path(), path);
}

/// A reference is a tag, as the OCI distribution spec writes one, or a digest,
/// `<algorithm>:<encoded>`; anything else could change the route, or names no artifact.
#[test]
fn a_reference_that_is_neither_a_tag_nor_a_digest_is_usage() {
    let server = Server::start(vec![]);
    let long = "v".repeat(129);
    for reference in [
        "",
        ".",
        "..",
        "-v1",
        ".v1",
        "v1/x",
        "v1?x",
        "v 1",
        "sha256:",
        ":0a1b",
        "SHA256:0a1b",
        "sha256+:0a1b",
        "sha256:0a/1b",
        "sha256:0a1b?x",
        &long,
    ] {
        for args in [
            // After `--`, so clap passes `-v1` to the parser instead of reading a flag.
            &["hub", "artifacts", "get", "acme/app", "--", reference][..],
            &[
                "hub",
                "artifacts",
                "delete",
                "--yes",
                "acme/app",
                "--",
                reference,
            ],
        ] {
            let error = Hipc::against(&Home::new(), &server)
                .run(args)
                .fails(2, "usage");
            assert!(
                error["message"].as_str().unwrap().contains("reference"),
                "{args:?}: {error}"
            );
        }
    }
    assert!(server.requests().is_empty());
}

#[test]
fn a_tag_of_128_characters_and_a_long_digest_are_sent() {
    let tag = format!("_{}abc", "v1.-".repeat(31));
    assert_eq!(tag.len(), 128);
    let digest = format!("sha512+x.y_z-w:{}", "0aZ=_-".repeat(22));
    for reference in [tag.as_str(), digest.as_str()] {
        let path = format!("/api/registry/repositories/acme/app/artifacts/{reference}/");
        let server = Server::start(vec![route("GET", &path, Reply::json(200, ARTIFACT))]);
        Hipc::against(&Home::new(), &server)
            .run(&["hub", "artifacts", "get", "acme/app", reference])
            .ok();
        assert_eq!(server.only_request().path(), path);
    }
}
