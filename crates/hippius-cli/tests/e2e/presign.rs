//! `hipc s3 buckets objects presign`: a presigned download URL is a bearer credential for the
//! object, so it needs `--reveal`, goes only to stdout, and `--debug` never shows the query
//! values.

use hippius::routes::s3;

use crate::common::{Hipc, Home, Reply, Server, draft, route_of};

const PATH: &str = "/api/objectstore/buckets/photos/presigned-url/";

/// The signature in the answer's URL: no stderr line may hold it.
const SIGNATURE: &str = "sig0123456789presignedsecret";

fn answer() -> String {
    format!(
        concat!(
            r#"{{"url":"https://s3.hippius.com/photos/2026/cat.jpg?"#,
            r#"X-Amz-Expires=600&X-Amz-Signature={}"}}"#
        ),
        SIGNATURE
    )
}

fn server() -> Server {
    Server::start(vec![route_of(
        &s3::PRESIGNED_URL,
        [&"photos"],
        Reply::json(200, &answer()),
    )])
}

/// With `--reveal` the request carries the key and the expiry as query values, and the
/// answer is printed as sent, on stdout only.
#[test]
fn reveal_prints_the_url_on_stdout_and_nothing_on_stderr() {
    let server = server();
    let home = Home::new();
    let run = Hipc::against(&home, &server).run(&[
        "s3",
        "buckets",
        "objects",
        "presign",
        "photos",
        "2026/cat.jpg",
        "--expires-in",
        "600",
        "--reveal",
    ]);
    let doc = run.ok();
    assert!(doc["url"].as_str().unwrap().contains(SIGNATURE), "{doc}");
    assert_eq!(run.stderr(), "");
    let request = server.only_request();
    assert_eq!(request.path(), PATH);
    assert_eq!(
        request.target,
        format!("{PATH}?key=2026%2Fcat.jpg&expires_in=600")
    );

    // Text mode prints it on stdout too, and adds no note that could repeat it.
    let run = Hipc::against(&home, &server).run(&[
        "--output",
        "text",
        "s3",
        "buckets",
        "objects",
        "presign",
        "photos",
        "2026/cat.jpg",
        "--reveal",
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(run.stdout().contains(SIGNATURE), "{}", run.stdout());
    assert_eq!(run.stderr(), "");
    // Without `--expires-in` the server's default applies, and nothing is sent for it.
    assert_eq!(
        server.requests()[1].target,
        format!("{PATH}?key=2026%2Fcat.jpg")
    );
}

/// `--debug` names the query parameters and never their values, and the URL in the answer
/// reaches no debug line.
#[test]
fn debug_shows_query_names_and_no_values() {
    let server = server();
    let run = Hipc::against(&Home::new(), &server).run(&[
        "--debug",
        "s3",
        "buckets",
        "objects",
        "presign",
        "photos",
        "secret-key.txt",
        "--expires-in",
        "60",
        "--reveal",
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(run.stdout().contains(SIGNATURE), "{}", run.stdout());
    let stderr = run.stderr();
    assert!(
        stderr.contains("presigned-url/?key=<redacted>&expires_in=<redacted>"),
        "{stderr}"
    );
    assert!(!stderr.contains("secret-key"), "{stderr}");
    assert!(!stderr.contains(SIGNATURE), "{stderr}");
    assert!(!stderr.contains("s3.hippius.com"), "{stderr}");
}

/// Without `--reveal` it exits 2 `reveal_required` and sends nothing, `--yes` or not.
#[test]
fn without_reveal_nothing_is_sent() {
    let server = server();
    for extra in [&[][..], &["--yes"]] {
        let mut args = vec!["s3", "buckets", "objects", "presign", "photos", "a.txt"];
        args.extend(extra);
        let error = Hipc::against(&Home::new(), &server)
            .run(&args)
            .fails(2, "reveal_required");
        assert!(
            error["hint"].as_str().unwrap().contains("--reveal"),
            "{error}"
        );
    }
    assert!(server.requests().is_empty());
}

/// A dry run needs no `--reveal` and no token, and sends nothing.
#[test]
fn a_dry_run_needs_no_reveal_and_sends_nothing() {
    let server = server();
    let doc = Hipc::tokenless(&Home::new(), &server)
        .run(&[
            "--dry-run",
            "s3",
            "buckets",
            "objects",
            "presign",
            "photos",
            "a.txt",
        ])
        .ok();
    assert_eq!(doc["dry_run"], true);
    assert_eq!(draft(&doc)["method"], "GET");
    assert_eq!(
        draft(&doc)["url"],
        format!("{}{PATH}?key=a.txt", server.url())
    );
    assert!(server.requests().is_empty());
}

/// A bad bucket name, an empty key, or an expiry of zero is `usage` before `--reveal` is
/// checked, and nothing is sent.
#[test]
fn a_bad_argument_is_usage_before_reveal() {
    let server = server();
    for args in [
        &["s3", "buckets", "objects", "presign", "Bad_Name", "a.txt"][..],
        &["s3", "buckets", "objects", "presign", "photos", ""],
        &[
            "s3",
            "buckets",
            "objects",
            "presign",
            "photos",
            "a.txt",
            "--expires-in",
            "0",
        ],
        &[
            "s3",
            "buckets",
            "objects",
            "presign",
            "photos",
            "a.txt",
            "--expires-in",
            "soon",
        ],
    ] {
        Hipc::against(&Home::new(), &server)
            .run(args)
            .fails(2, "usage");
    }
    let error = Hipc::against(&Home::new(), &server)
        .run(&[
            "s3", "buckets", "objects", "presign", "photos", "", "--reveal",
        ])
        .fails(2, "usage");
    assert_eq!(error["message"], "the object key is empty");
    assert_eq!(
        error["hint"],
        "pass the object's key, such as `2026/cat.jpg`"
    );
    assert!(server.requests().is_empty());
}

/// An S3 presigned URL works for 7 days at most, so `--expires-in` stops at 604800 seconds:
/// one second more is `usage` with nothing sent, and the bound itself is sent as given.
#[test]
fn expires_in_is_at_most_seven_days() {
    let presign = |seconds: &'static str| {
        [
            "s3",
            "buckets",
            "objects",
            "presign",
            "photos",
            "2026/cat.jpg",
            "--expires-in",
            seconds,
            "--reveal",
        ]
    };
    let server = server();
    let error = Hipc::against(&Home::new(), &server)
        .run(&presign("604801"))
        .fails(2, "usage");
    assert!(
        error["message"].as_str().unwrap().contains("604801"),
        "{error}"
    );
    assert!(server.requests().is_empty());
    Hipc::against(&Home::new(), &server)
        .run(&presign("604800"))
        .ok();
    assert_eq!(
        server.only_request().target,
        format!("{PATH}?key=2026%2Fcat.jpg&expires_in=604800")
    );
}

/// `hipc api` holds the route to the same rule: `--reveal` or nothing is sent.
#[test]
fn hipc_api_needs_reveal_on_the_route() {
    let server = server();
    let target = "/objectstore/buckets/photos/presigned-url/?key=a.txt";
    Hipc::against(&Home::new(), &server)
        .run(&["api", "GET", target])
        .fails(2, "reveal_required");
    Hipc::against(&Home::new(), &server)
        .run(&[
            "api",
            "GET",
            "/objectstore/buckets/photos/presigned-url?key=a.txt",
        ])
        .fails(2, "reveal_required");
    assert!(server.requests().is_empty());
    let run = Hipc::against(&Home::new(), &server).run(&["api", "GET", target, "--reveal"]);
    assert!(run.ok()["url"].as_str().unwrap().contains(SIGNATURE));
    assert_eq!(server.requests().len(), 1);
}
