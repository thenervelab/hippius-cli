//! `hipc status`: the state of each product, a top-level read of `GET /api/services/status/`.

use serde_json::json;

use crate::common::{Hipc, Home, MEMBER, Reply, Server, TOKEN, draft, route};

const STATUS: &str = concat!(
    r#"{"hub":{"state":"none"},"drive":{"state":"active"},"#,
    r#""s3":{"state":"active","buckets":2,"total_bytes":1024},"#,
    r#""compute":{"state":"active","vms":1,"running":1,"provisioning":0}}"#
);

fn server() -> Server {
    Server::start(vec![route(
        "GET",
        "/api/services/status/",
        Reply::json(200, STATUS),
    )])
}

/// The answer is printed as sent, and the request carries the token: swagger documents the
/// route for the signed-in user.
#[test]
fn status_prints_the_answer_and_sends_the_token() {
    let server = server();
    let doc = Hipc::against(&Home::new(), &server).run(&["status"]).ok();
    assert_eq!(
        doc["s3"],
        json!({"state": "active", "buckets": 2, "total_bytes": 1024})
    );
    let request = server.only_request();
    assert_eq!(request.target, "/api/services/status/");
    assert_eq!(
        request.header("authorization"),
        Some(format!("Token {TOKEN}").as_str())
    );
}

/// `--api-url` picks the origin, as for every command; `HIPPIUS_TOKEN` goes there when
/// `HIPPIUS_TOKEN_ORIGIN` names it, and a stored token is withheld from another one.
#[test]
fn status_follows_api_url_and_withholds_a_stored_token_from_another_origin() {
    let server = server();
    let home = Home::new();
    Hipc::new(&home)
        .with_env_token(server.url(), TOKEN)
        .run(&["--api-url", server.url(), "status"])
        .ok();
    assert_eq!(server.requests().len(), 1);
    home.write_config(&format!(
        "[profiles.default]\napi_url = \"https://api.hippius.com\"\ntoken = \"{TOKEN}\"\n"
    ));
    Hipc::new(&home)
        .run(&["--api-url", server.url(), "status"])
        .fails(1, "auth");
    // No token at all is `auth` too, with nothing sent.
    Hipc::new(&Home::new())
        .env("HIPPIUS_API_URL", server.url())
        .run(&["status"])
        .fails(1, "auth");
    assert_eq!(server.requests().len(), 1);
}

/// The route is in neither the account-scoped nor the personal list, so a membership is
/// refused rather than answered with the caller's own products; a dry run sends nothing.
#[test]
fn status_refuses_a_membership_and_a_dry_run_sends_nothing() {
    let server = server();
    Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["status"])
        .fails(2, "account_scope_unsupported");
    let doc = Hipc::new(&Home::new())
        .env("HIPPIUS_API_URL", server.url())
        .run(&["--dry-run", "status"])
        .ok();
    assert_eq!(draft(&doc)["method"], "GET");
    assert_eq!(
        draft(&doc)["url"],
        format!("{}/api/services/status/", server.url())
    );
    assert!(server.requests().is_empty());
}
