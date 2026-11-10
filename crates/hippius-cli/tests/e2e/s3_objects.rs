//! `hipc s3 buckets objects list`: a bucket's objects, read from the control plane, one page
//! at a time or every page with `--all`. No object bytes move.

use serde_json::{Value, json};

use crate::common::{
    BILLING, Hipc, Home, MEMBER, PROJECT, Reply, Route, Run, Server, draft, route,
};

const PATH: &str = "/api/objectstore/buckets/photos/objects/";

/// An object as swagger's `Contents` types it (values made up).
const OBJECT: &str = concat!(
    r#"{"Key":"2026/a.jpg","LastModified":"2026-11-01T10:00:00Z","ETag":"\"e1\"","#,
    r#""Size":99000000000000000000,"StorageClass":"STANDARD"}"#
);

fn objects(args: &[&str]) -> Vec<String> {
    let mut argv: Vec<String> = ["s3", "buckets", "objects", "list"]
        .iter()
        .map(|word| (*word).to_owned())
        .collect();
    argv.extend(args.iter().map(|arg| (*arg).to_owned()));
    argv
}

fn hipc(server: &Server, args: &[&str]) -> Run {
    let argv = objects(args);
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    Hipc::against(&Home::new(), server).run(&argv)
}

/// The objects route answering `query` (with its `?`). One without a query answers every
/// query, so the test server, which takes the first route that matches, lists it last.
fn page(query: &str, body: &str) -> Route {
    route("GET", &format!("{PATH}{query}"), Reply::json(200, body))
}

/// A `ListObjectsV2` page holding `keys`, with `next` as its continuation token.
fn listing(keys: &[&str], next: Option<&str>) -> String {
    let contents: Vec<Value> = keys.iter().map(|key| json!({"Key": key})).collect();
    let mut answer = json!({
        "Name": "photos",
        "Prefix": "",
        "KeyCount": keys.len(),
        "MaxKeys": 100,
        "IsTruncated": next.is_some(),
        "Contents": contents,
    });
    if let Some(next) = next {
        answer["NextContinuationToken"] = next.into();
    }
    answer.to_string()
}

fn targets(server: &Server) -> Vec<String> {
    server.requests().into_iter().map(|r| r.target).collect()
}

#[test]
fn one_page_is_the_envelope_of_contents_with_the_token_and_the_other_fields() {
    let first = format!(
        concat!(
            r#"{{"Name":"photos","Prefix":"","KeyCount":1,"MaxKeys":100,"IsTruncated":true,"#,
            r#""Contents":[{OBJECT}],"CommonPrefixes":[],"NextContinuationToken":"1Abc+/="}}"#
        ),
        OBJECT = OBJECT
    );
    let server = Server::start(vec![page("", &first)]);
    let run = hipc(&server, &["photos"]);
    let doc = run.ok();
    let object: Value = serde_json::from_str(OBJECT).unwrap();
    assert_eq!(doc["items"], json!([object]));
    assert_eq!(doc["next_page_token"], "1Abc+/=");
    assert_eq!(doc["Name"], "photos");
    assert_eq!(doc["KeyCount"], 1);
    assert_eq!(doc["IsTruncated"], true);
    assert_eq!(doc["CommonPrefixes"], json!([]));
    assert_eq!(doc.get("Contents"), None, "the objects are the items");
    assert_eq!(
        doc.get("NextContinuationToken"),
        None,
        "the token is next_page_token"
    );
    // A server number is not rounded (tenet 14).
    assert!(
        run.stdout().contains("99000000000000000000"),
        "{}",
        run.stdout()
    );
    assert_eq!(targets(&server), [format!("{PATH}?max_keys=100")]);
}

#[test]
fn the_flags_become_max_keys_prefix_and_the_continuation_token() {
    let server = Server::start(vec![page("", &listing(&["2026/b"], None))]);
    let doc = hipc(
        &server,
        &[
            "photos",
            "--prefix",
            "2026/ b",
            "--limit",
            "1000",
            "--page-token",
            "1Abc+/=",
        ],
    )
    .ok();
    assert_eq!(doc["items"], json!([{"Key": "2026/b"}]));
    assert_eq!(doc["next_page_token"], Value::Null);
    hipc(&server, &["photos", "--limit", "1"]).ok();
    assert_eq!(
        targets(&server),
        [
            format!("{PATH}?max_keys=1000&prefix=2026%2F%20b&continuation_token=1Abc%2B%2F%3D"),
            format!("{PATH}?max_keys=1"),
        ]
    );
}

#[test]
fn text_mode_prints_the_objects() {
    let server = Server::start(vec![page("", &listing(&["2026/a.jpg"], None))]);
    let argv = objects(&["photos"]);
    let mut full = vec!["--output", "text"];
    full.extend(argv.iter().map(String::as_str));
    let run = Hipc::against(&Home::new(), &server).run(&full);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(run.stdout().contains("2026/a.jpg"), "{}", run.stdout());
}

#[test]
fn an_empty_bucket_is_an_empty_page_and_an_empty_token_is_the_last_page() {
    // boto3 leaves `Contents` out of an empty page.
    for body in [
        r#"{"Name":"photos","KeyCount":0,"IsTruncated":false}"#,
        r#"{"Contents":null,"NextContinuationToken":null}"#,
        r#"{"Contents":[],"IsTruncated":false,"NextContinuationToken":""}"#,
    ] {
        let server = Server::start(vec![page("", body)]);
        let doc = hipc(&server, &["photos"]).ok();
        assert_eq!(doc["items"], json!([]), "{body}");
        assert_eq!(doc["next_page_token"], Value::Null, "{body}");
        let all = hipc(&server, &["photos", "--all"]).ok();
        assert_eq!(all, json!([]), "{body}");
        assert_eq!(server.requests().len(), 2, "{body}");
    }
}

#[test]
fn all_follows_the_tokens_until_none_is_left_and_prints_one_array() {
    let server = Server::start(vec![
        page(
            "?max_keys=2&continuation_token=t1",
            &listing(&["c", "d"], Some("t2")),
        ),
        page("?max_keys=2&continuation_token=t2", &listing(&["e"], None)),
        page("?max_keys=2", &listing(&["a", "b"], Some("t1"))),
    ]);
    let all = hipc(&server, &["photos", "--limit", "2", "--all"]).ok();
    assert_eq!(
        all,
        json!([{"Key": "a"}, {"Key": "b"}, {"Key": "c"}, {"Key": "d"}, {"Key": "e"}])
    );
    assert_eq!(
        targets(&server),
        [
            format!("{PATH}?max_keys=2"),
            format!("{PATH}?max_keys=2&continuation_token=t1"),
            format!("{PATH}?max_keys=2&continuation_token=t2"),
        ]
    );
    // The prefix goes on every page.
    let prefixed = Server::start(vec![
        page(
            "?max_keys=100&prefix=p%2F&continuation_token=t1",
            &listing(&["p/b"], None),
        ),
        page("?max_keys=100&prefix=p%2F", &listing(&["p/a"], Some("t1"))),
    ]);
    let all = hipc(&prefixed, &["photos", "--prefix", "p/", "--all"]).ok();
    assert_eq!(all, json!([{"Key": "p/a"}, {"Key": "p/b"}]));
}

#[test]
fn all_stops_on_a_token_that_leads_back() {
    let server = Server::start(vec![
        page(
            "?max_keys=100&continuation_token=t1",
            &listing(&[], Some("t2")),
        ),
        page(
            "?max_keys=100&continuation_token=t2",
            &listing(&[], Some("t1")),
        ),
        page("?max_keys=100", &listing(&[], Some("t1"))),
    ]);
    let error = hipc(&server, &["photos", "--all"]).fails(1, "internal");
    assert_eq!(
        error["message"],
        "the API's next page leads back to `t1`, which was already read"
    );
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn all_stops_after_a_thousand_pages() {
    let server = crate::common::endless_pages(PATH, |served| {
        format!(
            r#"{{"Contents":[],"IsTruncated":true,"NextContinuationToken":"t{}"}}"#,
            served + 1
        )
    });
    let argv = objects(&["photos", "--all"]);
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    let error = Hipc::against(&Home::new(), &server)
        .run(&argv)
        .fails(1, "internal");
    assert!(
        error["message"].as_str().unwrap().contains("1000 pages"),
        "{error}"
    );
    assert_eq!(server.requests().len(), 1000);
}

#[test]
fn all_stops_once_the_pages_hold_more_than_64_mib() {
    // Each page is under the 32 MiB answer cap; the third takes the list past 64 MiB.
    let padded = |next: &str| {
        format!(
            r#"{{"Name":"{}","Contents":[],"IsTruncated":true,"NextContinuationToken":"{next}"}}"#,
            "x".repeat(22 * 1024 * 1024)
        )
    };
    let server = Server::start(vec![
        page("?max_keys=100&continuation_token=t1", &padded("t2")),
        page("?max_keys=100&continuation_token=t2", &padded("t3")),
        page("?max_keys=100", &padded("t1")),
    ]);
    let error = hipc(&server, &["photos", "--all"]).fails(1, "internal");
    assert_eq!(
        error["message"],
        "the list is larger than 64 MiB across its pages, so reading stopped"
    );
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn an_answer_that_is_not_a_listing_is_internal() {
    for (body, says) in [
        ("[]", "something other than a list"),
        (r#""photos""#, "something other than a list"),
        (r#"{"Contents":{"Key":"a"}}"#, "something other than a list"),
        (
            r#"{"Contents":[],"NextContinuationToken":7}"#,
            "is not text",
        ),
        (
            r#"{"Contents":[],"IsTruncated":true}"#,
            "gives no `NextContinuationToken`",
        ),
        (
            r#"{"Contents":[],"IsTruncated":true,"NextContinuationToken":""}"#,
            "gives no `NextContinuationToken`",
        ),
        // An `IsTruncated` that is not a boolean says nothing either way, with a token or
        // without one.
        (
            r#"{"Contents":[],"IsTruncated":"true"}"#,
            "`IsTruncated` is not `true` or `false`",
        ),
        (
            r#"{"Contents":[],"IsTruncated":"false","NextContinuationToken":"t1"}"#,
            "`IsTruncated` is not `true` or `false`",
        ),
        (
            r#"{"Contents":[],"IsTruncated":1}"#,
            "`IsTruncated` is not `true` or `false`",
        ),
        (
            r#"{"Contents":[],"IsTruncated":0}"#,
            "`IsTruncated` is not `true` or `false`",
        ),
        (
            r#"{"Contents":[],"IsTruncated":[true]}"#,
            "`IsTruncated` is not `true` or `false`",
        ),
    ] {
        let server = Server::start(vec![page("", body)]);
        let error = hipc(&server, &["photos"]).fails(1, "internal");
        assert!(
            error["message"].as_str().unwrap().contains(says),
            "{body}: {error}"
        );
        hipc(&server, &["photos", "--all"]).fails(1, "internal");
    }
}

#[test]
fn a_server_token_that_page_token_would_refuse_is_internal() {
    let too_long = "a".repeat(16 * 1024 + 1);
    for token in ["a b", "a\\u0001b", "a\\tb", too_long.as_str()] {
        for truncated in [r#","IsTruncated":true"#, ""] {
            let body = format!(r#"{{"Contents":[]{truncated},"NextContinuationToken":"{token}"}}"#);
            let server = Server::start(vec![page("", &body)]);
            let error = hipc(&server, &["photos"]).fails(1, "internal");
            assert!(
                error["message"]
                    .as_str()
                    .unwrap()
                    .contains("is not one `--page-token` takes"),
                "{error}"
            );
            assert!(
                !error["message"].as_str().unwrap().contains("aaaa"),
                "the token is not quoted"
            );
            hipc(&server, &["photos", "--all"]).fails(1, "internal");
        }
    }
}

#[test]
fn every_printed_token_is_one_page_token_takes() {
    // The longest token taken, with the base64 characters S3 tokens hold.
    let token = format!("1+/={}", "A".repeat(16 * 1024 - 4));
    let encoded = format!("1%2B%2F%3D{}", "A".repeat(16 * 1024 - 4));
    let server = Server::start(vec![
        page(
            &format!("?max_keys=100&continuation_token={encoded}"),
            &listing(&["b"], None),
        ),
        page("?max_keys=100", &listing(&["a"], Some(&token))),
    ]);
    let first = hipc(&server, &["photos"]).ok();
    let printed = first["next_page_token"].as_str().unwrap().to_owned();
    assert_eq!(printed, token);
    let next = hipc(&server, &["photos", "--page-token", &printed]).ok();
    assert_eq!(next["items"], json!([{"Key": "b"}]));
    assert_eq!(next["next_page_token"], Value::Null);
}

#[test]
fn a_page_that_is_not_truncated_is_the_last_whatever_token_it_carries() {
    let body = r#"{"Contents":[{"Key":"a"}],"IsTruncated":false,"NextContinuationToken":"t1"}"#;
    let server = Server::start(vec![page("", body)]);
    let doc = hipc(&server, &["photos"]).ok();
    assert_eq!(doc["items"], json!([{"Key": "a"}]));
    assert_eq!(doc["next_page_token"], Value::Null);
    let all = hipc(&server, &["photos", "--all"]).ok();
    assert_eq!(all, json!([{"Key": "a"}]));
    assert_eq!(server.requests().len(), 2);
    // Not even a token `--page-token` would refuse is read on the last page.
    let odd = Server::start(vec![page(
        "",
        r#"{"Contents":[],"IsTruncated":false,"NextContinuationToken":"a b"}"#,
    )]);
    assert_eq!(hipc(&odd, &["photos"]).ok()["next_page_token"], Value::Null);
}

#[test]
fn a_repeated_token_is_quoted_whole() {
    let server = Server::start(vec![
        page(
            "?max_keys=100&continuation_token=a%3Fb%3Dc%23d",
            &listing(&[], Some("a?b=c#d")),
        ),
        page("?max_keys=100", &listing(&[], Some("a?b=c#d"))),
    ]);
    let error = hipc(&server, &["photos", "--all"]).fails(1, "internal");
    assert_eq!(
        error["message"],
        "the API's next page leads back to `a?b=c#d`, which was already read"
    );
}

#[test]
fn bad_flags_are_usage_before_any_request_or_token() {
    let server = Server::start(vec![]);
    let long = "a".repeat(16 * 1024 + 1);
    for args in [
        &["Photos"][..],
        &["a/b"],
        &["photos", "--limit", "0"],
        &["photos", "--limit", "1001"],
        &["photos", "--page-token", ""],
        &["photos", "--page-token", "a b"],
        &["photos", "--page-token", "a\u{1}b"],
        &["photos", "--page-token", &long],
        &["photos", "--page-token", "t1", "--all"],
        &[],
    ] {
        let argv = objects(args);
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        Hipc::tokenless(&Home::new(), &server)
            .run(&argv)
            .fails(2, "usage");
    }
    let error = hipc(&server, &["photos", "--page-token", "a b"]).fails(2, "usage");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("a continuation token"),
        "{error}"
    );
    // The longest token is taken.
    let longest = "a".repeat(16 * 1024);
    hipc(&server, &["photos", "--page-token", &longest, "--dry-run"]).ok();
    assert!(server.requests().is_empty());
    let argv = objects(&["photos"]);
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    Hipc::tokenless(&Home::new(), &server)
        .run(&argv)
        .fails(1, "auth");
    assert!(server.requests().is_empty());
}

#[test]
fn a_missing_bucket_is_not_found_with_a_hint_to_list_the_buckets() {
    let server = Server::start(vec![route(
        "GET",
        PATH,
        Reply::json(404, r#"{"detail":"Bucket not found"}"#),
    )]);
    let error = hipc(&server, &["photos"]).fails(1, "not_found");
    assert_eq!(
        error["message"],
        "cannot list the objects of the bucket `photos`: the API says `Bucket not found`"
    );
    assert!(
        error["hint"]
            .as_str()
            .unwrap()
            .contains("hipc s3 buckets list"),
        "{error}"
    );
    assert_eq!(error["side_effects"], "none");
}

#[test]
fn a_membership_and_a_project_go_on_the_request_and_a_billing_account_is_refused() {
    let server = Server::start(vec![page("", &listing(&[], None))]);
    let argv = objects(&["photos"]);
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .env("HIPPIUS_PROJECT", PROJECT)
        .run(&argv)
        .ok();
    let mut flagged = vec!["--project", PROJECT];
    flagged.extend(&argv);
    Hipc::against(&Home::new(), &server).run(&flagged).ok();
    hipc(&server, &["photos"]).ok();
    let sent: Vec<(Option<String>, Option<String>)> = server
        .requests()
        .iter()
        .map(|r| {
            (
                r.header("x-hippius-account").map(str::to_owned),
                r.header("x-hippius-project").map(str::to_owned),
            )
        })
        .collect();
    let some = |id: &str| Some(id.to_owned());
    assert_eq!(
        sent,
        [
            (some(MEMBER), some(PROJECT)),
            (None, some(PROJECT)),
            (None, None)
        ]
    );
    let mut billed = vec!["--billing", BILLING];
    billed.extend(&argv);
    Hipc::against(&Home::new(), &server)
        .run(&billed)
        .fails(2, "billing_scope_unsupported");
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn a_dry_run_names_the_first_request_without_a_token() {
    let server = Server::start(vec![]);
    for (args, query) in [
        (&["photos"][..], "?max_keys=100"),
        (&["photos", "--all"], "?max_keys=100"),
        (
            &[
                "photos",
                "--prefix",
                "a/",
                "--limit",
                "5",
                "--page-token",
                "t9",
            ],
            "?max_keys=5&prefix=a%2F&continuation_token=t9",
        ),
    ] {
        let mut argv = objects(args);
        argv.push("--dry-run".to_owned());
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        let doc = Hipc::tokenless(&Home::new(), &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&argv)
            .ok();
        assert_eq!(doc["dry_run"], true, "{args:?}");
        assert_eq!(draft(&doc)["method"], "GET", "{args:?}");
        assert_eq!(
            draft(&doc)["url"],
            format!("{}{PATH}{query}", server.url()),
            "{args:?}"
        );
        assert_eq!(doc["requests"].as_array().unwrap().len(), 1, "{args:?}");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn the_help_points_at_the_aws_cli_with_the_profile_s3_configure_writes() {
    let run = Hipc::new(&Home::new()).run(&["s3", "buckets", "objects", "list", "--help"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(
        run.stdout()
            .contains("aws --profile hippius s3 cp s3://photos/<key> ."),
        "{}",
        run.stdout()
    );
    // The profile named is the one `hipc s3 configure` writes by default.
    let configure = Hipc::new(&Home::new()).run(&["s3", "configure", "--help"]);
    assert!(
        configure.stdout().contains("[default: hippius]"),
        "{}",
        configure.stdout()
    );
}
