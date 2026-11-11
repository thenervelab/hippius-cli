//! `hipc s3 plans`, `subscription`, `buckets`, `tokens`, and `configure`.

use std::os::unix::fs::PermissionsExt as _;

use hippius::routes::s3;
use serde_json::json;

use crate::common::{
    Hipc, Home, MEMBER, PROJECT, Reply, Route, Server, command, draft, route, route_of,
};

/// A secret the server returns once. Tests assert where it may and may not appear.
const SECRET: &str = "s3cr3t-QZ7-only-once";
const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

/// The S3 plan catalog a subscribe or a plan change reads after its question.
fn plans() -> Route {
    route_of(
        &s3::PLANS,
        [],
        Reply::json(
            200,
            r#"{"plans":[{"code":"pro"},{"code":"business"},{"code":"enterprise"}]}"#,
        ),
    )
}

fn created() -> String {
    format!(
        concat!(
            r#"{{"accessKeyId":"hip_NEW1","secret":"{SECRET}","id":"{ID}","name":"ci","#,
            r#""status":"active","createdAt":"t","expiresAt":null}}"#
        ),
        SECRET = SECRET,
        ID = ID
    )
}

#[test]
fn plans_are_the_catalog_object_and_need_no_token() {
    let body = r#"{"plans":[{"code":"pro"}],"pay_as_you_go":{"price_per_tb_month":6}}"#;
    let server = Server::start(vec![route_of(&s3::PLANS, [], Reply::json(200, body))]);
    let doc = Hipc::tokenless(&Home::new(), &server)
        .run(&["s3", "plans"])
        .ok();
    assert_eq!(doc["pay_as_you_go"]["price_per_tb_month"], 6);
    assert_eq!(doc.get("items"), None, "an object answer is not wrapped");
    assert_eq!(server.only_request().header("authorization"), None);
}

#[test]
fn subscription_routes_send_the_account_header_for_a_membership() {
    let server = Server::start(vec![
        plans(),
        route_of(
            &s3::SUBSCRIPTION,
            [],
            Reply::json(200, r#"{"active":false,"mode":"pay_as_you_go"}"#),
        ),
        route_of(
            &s3::SUBSCRIBE,
            [],
            Reply::json(
                403,
                r#"{"code":"owner_only","error":"Only the owner can do this."}"#,
            ),
        ),
    ]);
    let home = Home::new();
    Hipc::against(&home, &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["s3", "subscription", "get"])
        .ok();
    let error = Hipc::against(&home, &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["s3", "subscription", "subscribe", "--plan", "pro", "--yes"])
        .fails(1, "owner_only");
    assert_eq!(
        error["message"],
        "cannot buy the S3 plan `pro`: the API says `Only the owner can do this.`"
    );
    for request in server.requests() {
        assert_eq!(request.header("x-hippius-account"), Some(MEMBER));
    }
}

#[test]
fn subscribe_sends_plan_and_rail_and_change_sends_plan_only() {
    let server = Server::start(vec![
        plans(),
        route_of(&s3::SUBSCRIBE, [], Reply::json(201, r#"{"active":true}"#)),
        route_of(
            &s3::SUBSCRIPTION_CHANGE,
            [],
            Reply::json(200, r#"{"plan":"business"}"#),
        ),
    ]);
    let home = Home::new();
    Hipc::against(&home, &server)
        .run(&[
            "s3",
            "subscription",
            "subscribe",
            "--plan",
            "enterprise",
            "--yes",
        ])
        .ok();
    Hipc::against(&home, &server)
        .run(&[
            "s3",
            "subscription",
            "change",
            "--plan",
            "business",
            "--yes",
        ])
        .ok();
    let requests = server.requests();
    let sent: Vec<(&str, &str)> = requests
        .iter()
        .map(|r| (r.method.as_str(), r.target.as_str()))
        .collect();
    assert_eq!(
        sent,
        [
            ("GET", "/api/s3/plans/"),
            ("POST", "/api/s3/subscription/"),
            ("GET", "/api/s3/plans/"),
            ("PATCH", "/api/s3/subscription/"),
        ]
    );
    assert_eq!(
        requests[1].json(),
        json!({"plan": "enterprise", "rail": "credits"})
    );
    assert_eq!(requests[3].json(), json!({"plan": "business"}));
}

/// The plans are the server's (owner decision D5): a code its catalog lists is bought even
/// when this hipc does not know it, and one it does not list is refused after the question and
/// the read, before anything is bought. A dry run lists the read and reads nothing.
#[test]
fn a_plan_is_checked_against_the_server_catalog() {
    let server = Server::start(vec![
        route_of(
            &s3::PLANS,
            [],
            Reply::json(200, r#"{"plans":[{"code":"pro"},{"code":"team"}]}"#),
        ),
        route_of(&s3::SUBSCRIBE, [], Reply::json(201, r#"{"active":true}"#)),
    ]);
    let home = Home::new();
    let subscribe = |plan: &str| {
        Hipc::against(&home, &server).run(&[
            "s3",
            "subscription",
            "subscribe",
            "--plan",
            plan,
            "--yes",
        ])
    };
    subscribe("team").ok();
    assert_eq!(server.requests()[1].json()["plan"], "team");
    let error = subscribe("business").fails(2, "usage");
    assert_eq!(
        error["message"],
        "an S3 plan `business` is not one the server offers: the server offers `pro` and `team`"
    );
    assert_eq!(error["hint"], "run `hipc s3 plans` to see them");
    let error = Hipc::against(&home, &server)
        .run(&["s3", "subscription", "change", "--plan", "gold", "--yes"])
        .fails(2, "usage");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with("an S3 plan `gold` is not one")
    );
    let methods: Vec<String> = server.requests().iter().map(|r| r.method.clone()).collect();
    assert_eq!(
        methods,
        ["GET", "POST", "GET", "GET"],
        "nothing bought after a refusal"
    );

    let doc = Hipc::against(&home, &server)
        .run(&[
            "s3",
            "subscription",
            "subscribe",
            "--plan",
            "gold",
            "--dry-run",
        ])
        .ok();
    let sent: Vec<&str> = doc["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["method"].as_str().unwrap())
        .collect();
    assert_eq!(sent, ["GET", "POST"]);
    assert_eq!(server.requests().len(), 4);
    let error = subscribe("Pro").fails(2, "usage");
    assert!(
        error["message"].as_str().unwrap().contains(
            "an S3 plan is a code of lowercase letters, digits, `.`, and `-`, such as `pro`, \
             `business`, or `enterprise`"
        ),
        "{error}"
    );
}

/// Swagger `s3_subscription_partial_update`: a PATCH is a plan change, a funding switch,
/// or an overage cap, one at a time, each confirmed.
#[test]
fn a_card_checkout_and_a_cancel_note_only_in_text_mode() {
    let server = Server::start(vec![
        plans(),
        route_of(
            &s3::SUBSCRIBE,
            [],
            Reply::json(
                202,
                r#"{"checkout_url":"https://checkout.example/s3","intent_id":"i-2"}"#,
            ),
        ),
        route_of(
            &s3::SUBSCRIPTION_CANCEL,
            [],
            Reply::json(202, r#"{"status":"cancel_submitted"}"#),
        ),
    ]);
    let home = Home::new();
    let card = [
        "s3",
        "subscription",
        "subscribe",
        "--plan",
        "pro",
        "--pay-from",
        "card",
        "--yes",
    ];
    let json_run = Hipc::against(&home, &server).run(&card);
    assert_eq!(json_run.stderr(), "");
    // The one field hipc adds: the command that goes on with the flow.
    assert_eq!(
        json_run.json(),
        json!({
            "checkout_url": "https://checkout.example/s3",
            "intent_id": "i-2",
            "next_step": format!("hipc --api-url {} s3 subscription checkout i-2", server.url())
        })
    );
    assert_eq!(server.requests()[1].json()["rail"], "card");
    let mut text = vec!["--output", "text"];
    text.extend(card);
    let text_run = Hipc::against(&home, &server).run(&text);
    assert!(
        text_run.stderr().contains("`checkout_url` in the output")
            && text_run.stderr().contains(&format!(
                "`hipc --api-url {} s3 subscription checkout i-2`",
                server.url()
            )),
        "{}",
        text_run.stderr()
    );
    let cancel = Hipc::against(&home, &server).run(&[
        "--output",
        "text",
        "s3",
        "subscription",
        "cancel",
        "--yes",
    ]);
    assert_eq!(cancel.code(), Some(0));
    assert!(
        cancel.stderr().contains(
            "note: once the cancel is confirmed, the plan stays until the end of the month paid \
             for, with no refund, and then the account is on pay as you go"
        ),
        "{}",
        cancel.stderr()
    );
    let quiet = Hipc::against(&home, &server).run(&[
        "--output",
        "text",
        "--quiet",
        "s3",
        "subscription",
        "cancel",
        "--yes",
    ]);
    assert_eq!(quiet.stderr(), "");
}

#[test]
fn a_bodyless_checkout_says_nothing_was_bought() {
    let server = Server::start(vec![
        plans(),
        route_of(&s3::SUBSCRIBE, [], Reply::json(202, "{}")),
    ]);
    let run = Hipc::against(&Home::new(), &server).run(&[
        "--output",
        "text",
        "s3",
        "subscription",
        "subscribe",
        "--plan",
        "pro",
        "--pay-from",
        "card",
        "--yes",
    ]);
    assert!(
        run.stderr()
            .starts_with("note: nothing has been bought yet")
    );
}

#[test]
fn subscription_writes_need_yes_and_send_nothing_without_it() {
    let server = Server::start(vec![]);
    for args in [
        &["s3", "subscription", "subscribe", "--plan", "pro"][..],
        &["s3", "subscription", "change", "--plan", "pro"],
        &["s3", "subscription", "cancel"],
    ] {
        Hipc::against(&Home::new(), &server)
            .run(args)
            .fails(2, "confirmation_required");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn cancel_maps_the_chain_errors() {
    for (status, code) in [
        (404, "not_found"),
        (422, "validation"),
        (425, "unavailable"),
        (503, "unavailable"),
    ] {
        let server = Server::start(vec![route_of(
            &s3::SUBSCRIPTION_CANCEL,
            [],
            Reply::json(status, "{}"),
        )]);
        Hipc::against(&Home::new(), &server)
            .run(&["s3", "subscription", "cancel", "--yes"])
            .fails(1, code);
    }
}

#[test]
fn buckets_list_create_and_delete() {
    let server = Server::start(vec![
        route_of(&s3::BUCKETS, [], Reply::json(200, r#"[{"name":"photos"}]"#)),
        route_of(
            &s3::BUCKET_CREATE,
            [],
            Reply::json(201, r#"{"name":"logs","region":"auto"}"#),
        ),
        route_of(&s3::BUCKET_DELETE, [&"logs"], Reply::json(204, "")),
    ]);
    let home = Home::new();
    let list = Hipc::against(&home, &server)
        .run(&["s3", "buckets", "list"])
        .ok();
    assert_eq!(
        list,
        json!({"items": [{"name": "photos"}], "next_page_token": null})
    );
    Hipc::against(&home, &server)
        .run(&["s3", "buckets", "create", "logs", "--yes"])
        .ok();
    Hipc::against(&home, &server)
        .run(&["s3", "buckets", "create", "logs", "--region", "eu", "--yes"])
        .ok();
    let deleted = Hipc::against(&home, &server)
        .run(&["s3", "buckets", "delete", "logs", "--yes"])
        .ok();
    assert_eq!(deleted, json!({"name": "logs", "deleted": true}));
    let requests = server.requests();
    assert_eq!(requests[1].json(), json!({"name": "logs"}));
    assert_eq!(requests[2].json(), json!({"name": "logs", "region": "eu"}));
    assert_eq!(requests[3].method, "DELETE");
}

/// Swagger's `{bucket, policy}`, with an S3 bucket policy that lets anyone read.
const ACL: &str = concat!(
    r#"{"bucket":"photos","policy":{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":"*","Action":["s3:GetObject"],"Resource":["arn:aws:s3:::photos/*"]}]}}"#
);

#[test]
fn bucket_acl_prints_the_policy_as_sent_and_names_its_url_under_debug() {
    let server = Server::start(vec![route_of(
        &s3::BUCKET_ACL,
        [&"photos"],
        Reply::json(200, ACL),
    )]);
    let home = Home::new();
    let acl = Hipc::against(&home, &server)
        .run(&["s3", "buckets", "acl", "get", "photos"])
        .ok();
    assert_eq!(
        acl,
        serde_json::from_str::<serde_json::Value>(ACL).expect("a JSON fixture")
    );
    let run =
        Hipc::against(&home, &server).run(&["s3", "buckets", "acl", "get", "photos", "--debug"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        format!(
            "debug: GET {}/api/objectstore/buckets/photos/acl/\ndebug: -> 200\n",
            server.url()
        )
    );
    let dry = Hipc::against(&home, &server)
        .run(&["s3", "buckets", "acl", "get", "photos", "--dry-run"])
        .ok();
    assert_eq!(draft(&dry)["method"], "GET");
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn bucket_acl_refuses_a_bad_name_and_maps_its_errors() {
    let server = Server::start(vec![
        route_of(&s3::BUCKET_ACL, [&"gone"], Reply::not_found()),
        route_of(
            &s3::BUCKET_ACL,
            [&"odd"],
            Reply::json(400, r#"{"error":"NoSuchBucketPolicy"}"#),
        ),
    ]);
    let home = Home::new();
    for name in ["Logs", "a", "../x", "a..b", "a/b", "a?b"] {
        Hipc::against(&home, &server)
            .run(&["s3", "buckets", "acl", "get", name])
            .fails(2, "usage");
    }
    // The message names every rule, the `..` one included.
    let error = Hipc::against(&home, &server)
        .run(&["s3", "buckets", "acl", "get", "a..b"])
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        "invalid value `a..b` for `<BUCKET_NAME>`: a bucket name is 3 to 63 lowercase \
         letters, digits, `.`, and `-`, starts and ends with a letter or a digit, and has no \
         `..`, such as `my-bucket.2`"
    );
    assert!(server.requests().is_empty());
    let error = Hipc::against(&home, &server)
        .run(&["s3", "buckets", "acl", "get", "gone"])
        .fails(1, "not_found");
    assert_eq!(
        error["hint"],
        "run `hipc s3 buckets list` to see what exists"
    );
    Hipc::against(&home, &server)
        .run(&["s3", "buckets", "acl", "get", "odd"])
        .fails(1, "validation");
}

#[test]
fn bucket_acl_with_a_membership_sends_the_account_header() {
    let server = Server::start(vec![route_of(
        &s3::BUCKET_ACL,
        [&"photos"],
        Reply::json(200, ACL),
    )]);
    Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["s3", "buckets", "acl", "get", "photos"])
        .ok();
    assert_eq!(
        server.only_request().header("x-hippius-account"),
        Some(MEMBER)
    );
}

/// Swagger `objectstore_acl_list`: per bucket, the sub-token grants that apply, as a bare
/// array, printed as the list envelope.
#[test]
fn bucket_acl_list_is_an_envelope_of_the_account_view() {
    let view = concat!(
        r#"[{"bucket":"photos","grants":[{"token_access_key_id":"hip_A1B2","#,
        r#""actions":["read"],"prefixes":["public/"]}]}]"#
    );
    let server = Server::start(vec![route_of(&s3::ACLS, [], Reply::json(200, view))]);
    let doc = Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["s3", "buckets", "acl", "list"])
        .ok();
    let items: serde_json::Value = serde_json::from_str(view).expect("a JSON fixture");
    assert_eq!(doc, json!({"items": items, "next_page_token": null}));
    assert_eq!(
        server.only_request().header("x-hippius-account"),
        Some(MEMBER)
    );
}

/// A bucket policy S3 reads: anyone may get the objects under `public/`.
const POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","#,
    r#""Action":["s3:GetObject"],"Resource":["arn:aws:s3:::photos/public/*"]}]}"#
);

/// `acl update` replaces the bucket policy, which can publish the bucket, so it needs
/// `--public`, then `--remove-others`, then `--yes`, as `hipc api` does on the same route. A dry
/// run needs none of them.
#[test]
fn bucket_acl_update_needs_public_then_remove_others_then_yes() {
    let server = Server::start(vec![route_of(
        &s3::BUCKET_ACL_SET,
        [&"photos"],
        Reply::json(200, r#"{"bucket":"photos","updated":true}"#),
    )]);
    let home = Home::new();
    let policy = home.file("policy.json", POLICY.as_bytes());
    let policy = policy.to_str().unwrap();
    let hipc = |extra: &[&str]| {
        let update = [
            "s3",
            "buckets",
            "acl",
            "update",
            "photos",
            "--policy-file",
            policy,
        ];
        Hipc::against(&home, &server).run(&[&update[..], extra].concat())
    };
    let sent = json!({
        "policy": serde_json::from_str::<serde_json::Value>(POLICY).expect("a JSON fixture")
    });

    let error = hipc(&["--yes"]).fails(2, "public_required");
    assert_eq!(
        error["message"],
        "the policy may make the bucket `photos` readable by anyone, and `--public` was not \
         passed"
    );
    hipc(&["--yes", "--remove-others"]).fails(2, "public_required");
    let error = hipc(&["--yes", "--public"]).fails(2, "remove_others_required");
    assert_eq!(
        error["message"],
        "the policy replaces the whole policy of the bucket `photos`, removing every statement \
         it leaves out, and `--remove-others` was not passed"
    );
    let hint = error["hint"].as_str().unwrap();
    assert!(
        hint.ends_with("--yes --public --remove-others` to replace it whole"),
        "{hint}"
    );
    let error = hipc(&["--public", "--remove-others"]).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!(
            "this would replace the whole policy of the bucket `photos` with the one in \
             `{policy}`, which can make it readable by anyone, and `--yes` was not passed"
        )
    );
    let dry = hipc(&["--dry-run"]).ok();
    assert_eq!(draft(&dry)["method"], "PUT");
    assert_eq!(draft(&dry)["body"], sent);
    // The write is the one request: the current policy is not read first.
    assert_eq!(dry["requests"].as_array().map(Vec::len), Some(1));
    assert!(server.requests().is_empty());

    let doc = hipc(&["--public", "--remove-others", "--yes"]).ok();
    assert_eq!(doc, json!({"bucket": "photos", "updated": true}));
    assert_eq!(server.only_request().json(), sent);
}

/// Swagger documents the answer as "Updated" with no body, so an empty answer prints what
/// was done.
#[test]
fn bucket_acl_update_answered_with_no_body_prints_what_it_did() {
    let server = Server::start(vec![route_of(
        &s3::BUCKET_ACL_SET,
        [&"photos"],
        Reply::bytes(200, "application/json", b""),
    )]);
    let home = Home::new();
    let policy = home.file("policy.json", POLICY.as_bytes());
    let doc = Hipc::against(&home, &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&[
            "s3",
            "buckets",
            "acl",
            "update",
            "photos",
            "--policy-file",
            policy.to_str().unwrap(),
            "--public",
            "--remove-others",
            "--yes",
        ])
        .ok();
    assert_eq!(doc, json!({"bucket": "photos", "updated": true}));
    assert_eq!(
        server.only_request().header("x-hippius-account"),
        Some(MEMBER)
    );
}

#[test]
fn bucket_acl_update_refuses_a_bad_name_or_a_policy_that_is_not_an_object() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let good = home.file("policy.json", POLICY.as_bytes());
    let good = good.to_str().unwrap();
    let update = |name: &str, file: &str| {
        Hipc::against(&home, &server).run(&[
            "s3",
            "buckets",
            "acl",
            "update",
            name,
            "--policy-file",
            file,
            "--public",
            "--remove-others",
            "--yes",
        ])
    };
    update("Bad_Name", good).fails(2, "usage");
    let huge = vec![b' '; 1024 * 1024 + 1];
    for (file, contents) in [
        ("null.json", &b"null"[..]),
        ("list.json", b"[]"),
        ("bad.json", b"{Statement"),
        ("huge.json", &huge),
        ("gone.json", b""),
    ] {
        let path = if file == "gone.json" {
            home.path().join(file)
        } else {
            home.file(file, contents)
        };
        let error = update("photos", path.to_str().unwrap()).fails(2, "usage");
        let hint = error["hint"].as_str().unwrap();
        assert!(hint.contains("`--policy-file`"), "{file}: {hint}");
    }
    let null = home.path().join("null.json");
    let error = update("photos", null.to_str().unwrap()).fails(2, "usage");
    assert_eq!(
        error["message"],
        format!(
            "the bucket policy from `{}` is not a JSON object",
            null.display()
        )
    );
    assert!(server.requests().is_empty());
}

#[test]
fn bucket_writes_need_yes_and_valid_names() {
    let server = Server::start(vec![]);
    let home = Home::new();
    Hipc::against(&home, &server)
        .run(&["s3", "buckets", "create", "logs"])
        .fails(2, "confirmation_required");
    Hipc::against(&home, &server)
        .run(&["s3", "buckets", "delete", "logs"])
        .fails(2, "confirmation_required");
    for name in ["Logs", "a", "../x", "a..b", "a/b"] {
        Hipc::against(&home, &server)
            .run(&["s3", "buckets", "delete", name, "--yes"])
            .fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn bucket_delete_errors_map_to_their_codes() {
    let server = Server::start(vec![
        route_of(
            &s3::BUCKET_DELETE,
            [&"busy"],
            Reply::json(409, r#"{"code":"bucket_served_by_cdn","zones":["z"]}"#),
        ),
        route_of(
            &s3::BUCKET_DELETE,
            [&"full"],
            Reply::json(400, r#"{"error":"not empty"}"#),
        ),
        route_of(&s3::BUCKET_DELETE, [&"gone"], Reply::not_found()),
    ]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["s3", "buckets", "delete", "busy", "--yes"])
        .fails(1, "conflict");
    assert_eq!(error["details"]["zones"], json!(["z"]));
    Hipc::against(&Home::new(), &server)
        .run(&["s3", "buckets", "delete", "full", "--yes"])
        .fails(1, "validation");
    Hipc::against(&Home::new(), &server)
        .run(&["s3", "buckets", "delete", "gone", "--yes"])
        .fails(1, "not_found");
}

#[test]
fn tokens_list_both_kinds_with_a_preview_and_no_secret() {
    let server = Server::start(vec![
        route_of(
            &s3::MASTER_TOKENS,
            [],
            Reply::json(
                200,
                // A server that leaks a secret into a listing: hipc must not print it.
                &format!(
                    concat!(
                        r#"[{{"id":"m","access_key_id":"hip_M","last4":"wxyz","#,
                        r#""status":"active","secret":"{SECRET}"}}]"#
                    ),
                    SECRET = SECRET
                ),
            ),
        ),
        route_of(
            &s3::SUB_TOKENS,
            [],
            Reply::json(
                200,
                // A nested secret is removed too.
                &format!(
                    concat!(
                        r#"[{{"id":"s","access_key_id":"hip_S","last4":"abcd","#,
                        r#""scope_type":"global","grant":{{"secret":"{SECRET}"}}}}]"#
                    ),
                    SECRET = SECRET
                ),
            ),
        ),
    ]);
    let run = Hipc::against(&Home::new(), &server).run(&["s3", "tokens", "list"]);
    assert!(!run.stdout().contains(SECRET));
    let doc = run.ok();
    assert_eq!(
        doc["items"][0],
        json!({
            "kind": "master",
            "id": "m",
            "access_key_id": "hip_M",
            "secret_preview": "wxyz",
            "status": "active",
            "secret": null
        })
    );
    assert_eq!(doc["items"][1]["kind"], "sub");
    assert_eq!(doc["items"][1]["secret_preview"], "abcd");
    assert_eq!(doc["items"][1]["secret"], serde_json::Value::Null);
    assert_eq!(doc["items"][1]["grant"], json!({}));
    for request in server.requests() {
        assert!(
            !request.target.contains('?'),
            "no pagination parameters: {}",
            request.target
        );
    }
}

#[test]
fn tokens_list_dry_run_describes_both_collections() {
    let server = Server::start(vec![]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["s3", "tokens", "list", "--dry-run"])
        .ok();
    assert_eq!(doc["requests"].as_array().unwrap().len(), 2);
    assert!(server.requests().is_empty());
}

#[test]
fn create_without_reveal_sends_nothing_even_with_yes() {
    let server = Server::start(vec![]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["s3", "tokens", "create", "--kind", "master", "--yes"])
        .fails(2, "reveal_required");
    assert!(error["hint"].as_str().unwrap().contains("--reveal"));
    assert!(server.requests().is_empty());
}

#[test]
fn reveal_is_checked_before_the_confirmation() {
    let server = Server::start(vec![]);
    Hipc::against(&Home::new(), &server)
        .run(&["s3", "tokens", "create", "--kind", "master"])
        .fails(2, "reveal_required");
    Hipc::against(&Home::new(), &server)
        .run(&["s3", "tokens", "create", "--kind", "master", "--reveal"])
        .fails(2, "confirmation_required");
    assert!(server.requests().is_empty());
}

#[test]
fn create_with_reveal_prints_the_secret_on_stdout_only_in_snake_case() {
    let server = Server::start(vec![route_of(
        &s3::MASTER_TOKEN_CREATE,
        [],
        Reply::json(201, &created()),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&[
        "s3", "tokens", "create", "--kind", "master", "--name", "ci", "--reveal", "--yes",
    ]);
    let doc = run.ok();
    assert_eq!(doc["kind"], "master");
    assert_eq!(doc["access_key_id"], "hip_NEW1");
    assert_eq!(doc["secret"], SECRET);
    assert_eq!(doc["created_at"], "t");
    assert!(doc.get("accessKeyId").is_none() && doc.get("createdAt").is_none());
    assert!(!run.stderr().contains(SECRET));
    assert_eq!(server.only_request().json(), json!({"name": "ci"}));
}

/// A master token is account-wide, so its route takes no project: `--project` on its
/// create is refused before anything is sent, a dry run included (tenet 12's own example).
#[test]
fn a_master_token_create_with_a_project_is_refused_without_a_request() {
    let server = Server::start(vec![]);
    for extra in [&["--reveal", "--yes"][..], &["--dry-run"]] {
        let argv = [
            &[
                "--project",
                PROJECT,
                "s3",
                "tokens",
                "create",
                "--kind",
                "master",
            ][..],
            extra,
        ]
        .concat();
        Hipc::against(&Home::new(), &server)
            .run(&argv)
            .fails(2, "project_scope_unsupported");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn create_dry_run_needs_neither_reveal_nor_yes() {
    let server = Server::start(vec![]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&[
            "s3",
            "tokens",
            "create",
            "--kind",
            "sub",
            "--bucket",
            "a",
            "--dry-run",
        ])
        .ok();
    assert_eq!(
        draft(&doc)["url"],
        format!("{}/api/objectstore/sub-tokens/", server.url())
    );
    assert_eq!(draft(&doc)["body"], json!({"bucket_names": ["a"]}));
    assert!(server.requests().is_empty());
}

#[test]
fn a_sub_key_sends_every_flag_given_and_nothing_else() {
    let server = Server::start(vec![route_of(
        &s3::SUB_TOKEN_CREATE,
        [],
        Reply::json(201, &created()),
    )]);
    Hipc::against(&Home::new(), &server)
        .run(&[
            "s3",
            "tokens",
            "create",
            "--kind",
            "sub",
            "--scope-type",
            "multi-bucket",
            "--bucket",
            "a",
            "--bucket",
            "b",
            "--action",
            "s3:GetObject",
            "--prefix",
            "logs/",
            "--ip",
            "10.0.0.0/8",
            "--not-before",
            "2026-11-01T00:00:00Z",
            "--expires-at",
            "2027-01-01T00:00:00Z",
            "--reveal",
            "--yes",
        ])
        .ok();
    assert_eq!(
        server.only_request().json(),
        json!({
            "expires_at": "2027-01-01T00:00:00Z",
            "scope_type": "multi_bucket",
            "bucket_names": ["a", "b"],
            "actions": ["s3:GetObject"],
            "allowed_prefixes": ["logs/"],
            "ip_allowlist": ["10.0.0.0/8"],
            "not_before": "2026-11-01T00:00:00Z"
        })
    );
}

#[test]
fn a_sub_key_flag_on_a_master_key_is_usage() {
    let server = Server::start(vec![]);
    for flags in [
        &["--bucket", "a"][..],
        &["--scope-type", "global"],
        &["--action", "x"],
        &["--prefix", "p"],
        &["--ip", "1.2.3.4"],
        &["--not-before", "t"],
    ] {
        let mut args = vec![
            "s3", "tokens", "create", "--kind", "master", "--reveal", "--yes",
        ];
        args.extend_from_slice(flags);
        Hipc::against(&Home::new(), &server)
            .run(&args)
            .fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn a_rejected_sub_key_is_validation() {
    let server = Server::start(vec![route_of(
        &s3::SUB_TOKEN_CREATE,
        [],
        Reply::json(400, r#"{"bucket_names":["This field is required."]}"#),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&[
            "s3", "tokens", "create", "--kind", "sub", "--reveal", "--yes",
        ])
        .fails(1, "validation");
    assert_eq!(
        error["details"]["bucket_names"][0],
        "This field is required."
    );
}

#[test]
fn revoke_posts_to_the_kinds_collection_with_an_empty_body() {
    let path = format!("/api/objectstore/sub-tokens/{ID}/revoke/");
    let server = Server::start(vec![route(
        "POST",
        &path,
        Reply::json(
            200,
            r#"{"status":"revoked","secret":"s3cr3t","old":{"secret":"s3cr3t"}}"#,
        ),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["s3", "tokens", "revoke", "--kind", "sub", ID, "--yes"])
        .ok();
    assert_eq!(doc, json!({"id": ID, "status": "revoked", "old": {}}));
    let request = server.only_request();
    assert_eq!(request.target, path);
    assert_eq!(request.header("content-length"), Some("0"));
}

#[test]
fn revoke_with_an_empty_answer_still_names_the_id() {
    let path = format!("/api/objectstore/master-tokens/{ID}/revoke/");
    let server = Server::start(vec![route("POST", &path, Reply::json(200, ""))]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["s3", "tokens", "revoke", "--kind", "master", ID, "--yes"])
        .ok();
    assert_eq!(doc, json!({"id": ID}));
}

#[test]
fn token_ids_must_be_uuids_and_revoke_needs_yes() {
    let server = Server::start(vec![]);
    for id in [
        "abc",
        "../../accounts",
        "0f8fad5b-d9cb-469f-a165-70867728950",
    ] {
        Hipc::against(&Home::new(), &server)
            .run(&["s3", "tokens", "revoke", "--kind", "sub", id, "--yes"])
            .fails(2, "usage");
        Hipc::against(&Home::new(), &server)
            .run(&[
                "s3", "tokens", "rotate", "--kind", "sub", id, "--reveal", "--yes",
            ])
            .fails(2, "usage");
    }
    Hipc::against(&Home::new(), &server)
        .run(&["s3", "tokens", "revoke", "--kind", "sub", ID])
        .fails(2, "confirmation_required");
    assert!(server.requests().is_empty());
}

/// The refusal of each token write names a token, the noun of `hipc s3 tokens` (tenet 15),
/// and a bad id is not a token id.
#[test]
fn token_writes_name_a_token() {
    let server = Server::start(vec![]);
    for (args, message) in [
        (
            &["s3", "tokens", "create", "--kind", "master", "--reveal"][..],
            "this would create an S3 `master` token, and `--yes` was not passed",
        ),
        (
            &["s3", "tokens", "revoke", "--kind", "sub", ID],
            &*format!("this would revoke the S3 `sub` token `{ID}`, and `--yes` was not passed"),
        ),
        (
            &["s3", "tokens", "rotate", "--kind", "sub", ID, "--reveal"],
            &*format!("this would rotate the S3 `sub` token `{ID}`, and `--yes` was not passed"),
        ),
    ] {
        let error = Hipc::against(&Home::new(), &server)
            .run(args)
            .fails(2, "confirmation_required");
        assert_eq!(error["message"], message);
    }
    let error = Hipc::against(&Home::new(), &server)
        .run(&["s3", "tokens", "revoke", "--kind", "sub", "abc", "--yes"])
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        "invalid value `abc` for `<TOKEN_ID>`: a token id is a UUID, from `hipc s3 tokens list`"
    );
    let error = Hipc::against(&Home::new(), &server)
        .run(&[
            "s3", "tokens", "create", "--kind", "master", "--ip", "1.2.3.4",
        ])
        .fails(2, "usage");
    assert_eq!(
        error["hint"],
        "pass `--kind sub` to create a token with those limits"
    );
    assert!(server.requests().is_empty());
}

#[test]
fn rotate_needs_reveal_and_prints_the_new_token() {
    let path = format!("/api/objectstore/master-tokens/{ID}/rotate/");
    let server = Server::start(vec![route("POST", &path, Reply::json(201, &created()))]);
    Hipc::against(&Home::new(), &server)
        .run(&["s3", "tokens", "rotate", "--kind", "master", ID, "--yes"])
        .fails(2, "reveal_required");
    assert!(server.requests().is_empty());
    let run = Hipc::against(&Home::new(), &server).run(&[
        "s3", "tokens", "rotate", "--kind", "master", ID, "--reveal", "--yes",
    ]);
    let doc = run.ok();
    assert_eq!(doc["secret"], SECRET);
    assert_eq!(doc["access_key_id"], "hip_NEW1");
    let revoked = Server::start(vec![route(
        "POST",
        &format!("/api/objectstore/sub-tokens/{ID}/rotate/"),
        Reply::json(409, r#"{"error":"The token is revoked"}"#),
    )]);
    Hipc::against(&Home::new(), &revoked)
        .run(&[
            "s3", "tokens", "rotate", "--kind", "sub", ID, "--reveal", "--yes",
        ])
        .fails(1, "conflict");
}

#[test]
fn s3_account_routes_need_a_token() {
    let server = Server::start(vec![]);
    for args in [
        &["s3", "buckets", "list"][..],
        &["s3", "tokens", "list"],
        &["s3", "subscription", "get"],
    ] {
        Hipc::tokenless(&Home::new(), &server)
            .run(args)
            .fails(1, "auth");
    }
    assert!(server.requests().is_empty());
}

fn mode(path: &std::path::Path) -> u32 {
    std::fs::metadata(path)
        .expect("the file exists")
        .permissions()
        .mode()
        & 0o777
}

#[test]
fn configure_writes_a_profile_for_s3_hippius_com_and_keeps_other_profiles() {
    let home = Home::new();
    let aws = home.path().join(".aws");
    std::fs::create_dir_all(&aws).expect("mkdir .aws");
    // Explicit, so that a host umask of 0002 does not leave the directory open to the group.
    std::fs::set_permissions(&aws, std::fs::Permissions::from_mode(0o700)).expect("chmod");
    home.file(
        ".aws/credentials",
        b"[work]\naws_access_key_id = AKIAWORK\n",
    );
    let secret = home.file("secret", format!("{SECRET}\n").as_bytes());
    let run = Hipc::new(&home).run(&[
        "s3",
        "configure",
        "--access-key-id",
        "hip_KEY1",
        "--secret-key-file",
        secret.to_str().unwrap(),
    ]);
    let doc = run.ok();
    assert_eq!(doc["aws_profile"], "hippius");
    assert_eq!(doc["secret_written"], true);
    assert!(!run.stdout().contains(SECRET));
    let config = std::fs::read_to_string(aws.join("config")).unwrap();
    assert!(config.contains(
        "[profile hippius]\nendpoint_url = https://s3.hippius.com\nregion = decentralized\n"
    ));
    assert!(config.contains("addressing_style = path"));
    let credentials = std::fs::read_to_string(aws.join("credentials")).unwrap();
    assert!(credentials.starts_with("[work]\naws_access_key_id = AKIAWORK\n"));
    assert!(credentials.contains(&format!(
        "[hippius]\naws_access_key_id = hip_KEY1\naws_secret_access_key = {SECRET}\n"
    )));
    assert_eq!(mode(&aws.join("credentials")), 0o600);
}

/// `hipc s3 configure`, run by `/bin/sh` after `setup`, with the secret key in `secret`.
fn configure_under_sh(home: &Home, setup: &str, secret: &std::path::Path) -> std::process::Output {
    let hipc = command(home);
    let mut shell = std::process::Command::new("/bin/sh");
    for (name, value) in hipc.get_envs() {
        match value {
            Some(value) => shell.env(name, value),
            None => shell.env_remove(name),
        };
    }
    shell
        .args(["-c", &format!("{setup}; exec \"$0\" \"$@\"")])
        .arg(hipc.get_program())
        .args([
            "--json",
            "s3",
            "configure",
            "--yes",
            "--access-key-id",
            "hip_KEY1",
        ])
        .arg("--secret-key-file")
        .arg(secret)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    crate::common::finished(&mut shell)
}

/// A umask that clears the owner's write bit does not leave the new `.aws` directory or files
/// read-only: they are `0700` and `0600`, and a second run can replace the profile.
#[test]
fn configure_under_a_strict_umask_makes_files_it_can_write_again() {
    let home = Home::new();
    let secret = home.file("secret", SECRET.as_bytes());
    let aws = home.path().join(".aws");
    let run = configure_under_sh(&home, "umask 0277", &secret);
    assert_eq!(run.status.code(), Some(0), "{run:?}");
    assert_eq!(mode(&aws), 0o700);
    assert_eq!(mode(&aws.join("config")), 0o600);
    assert_eq!(mode(&aws.join("credentials")), 0o600);
    let other = home.file("other", b"other-secret");
    let run = configure_under_sh(&home, "umask 0277", &other);
    assert_eq!(run.status.code(), Some(0), "{run:?}");
    let credentials = std::fs::read_to_string(aws.join("credentials")).unwrap();
    assert!(credentials.contains("aws_secret_access_key = other-secret\n"));
}

/// A run that waits for another one's lock of an AWS file says so in text mode, then does its
/// work once the lock is free; in JSON mode it waits without a word (tenet 3).
#[test]
fn configure_says_when_it_waits_for_another_runs_lock() {
    use std::io::BufRead as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    for output in ["text", "json"] {
        let home = Home::new();
        let aws = home.path().join(".aws");
        std::fs::create_dir_all(&aws).expect("mkdir .aws");
        std::fs::set_permissions(&aws, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(aws.join(".credentials.lock"))
            .expect("open the lock file");
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive).expect("flock");
        let secret = home.file("secret", SECRET.as_bytes());
        let mut child = command(&home)
            .args([
                "--output",
                output,
                "s3",
                "configure",
                "--access-key-id",
                "hip_KEY1",
            ])
            .arg("--secret-key-file")
            .arg(&secret)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn hipc");
        let stderr = child.stderr.take().expect("a piped stderr");
        let (told, first_line) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut stderr = std::io::BufReader::new(stderr);
            let mut line = String::new();
            stderr.read_line(&mut line).expect("read stderr");
            drop(told.send(line.clone()));
            let mut rest = String::new();
            std::io::Read::read_to_string(&mut stderr, &mut rest).expect("read stderr");
            line + &rest
        });
        let wait = std::time::Duration::from_millis(if output == "text" { 30_000 } else { 500 });
        let line = first_line.recv_timeout(wait);
        assert!(
            !aws.join("credentials").exists(),
            "{output}: written while it waits"
        );
        drop(lock);
        let run = crate::common::finish(child, None, &["s3", "configure"]);
        let stderr = reader.join().expect("the stderr reader");
        assert_eq!(run.status.code(), Some(0), "{output}: {stderr}");
        let credentials = std::fs::read_to_string(aws.join("credentials")).unwrap();
        assert!(credentials.contains("[hippius]"), "{output}");
        if output == "text" {
            assert_eq!(
                line.expect("a note before the wait"),
                format!(
                    "note: waiting for another run of hipc to finish with the AWS credentials \
                     file `{}`\n",
                    aws.join("credentials").display()
                )
            );
        } else {
            assert!(line.is_err(), "{line:?}");
            assert_eq!(stderr, "");
        }
    }
}

#[test]
fn configure_over_an_existing_profile_needs_yes_and_then_replaces_the_secret() {
    let home = Home::new();
    let first = home.file("first", b"first-secret");
    let second = home.file("second", b"second-secret");
    let run = |secret: &std::path::Path, yes: bool| {
        let mut args = vec![
            "s3",
            "configure",
            "--access-key-id",
            "hip_KEY1",
            "--secret-key-file",
            secret.to_str().expect("a UTF-8 path"),
        ];
        if yes {
            args.push("--yes");
        }
        Hipc::new(&home).run(&args)
    };
    run(&first, false).ok();
    run(&second, false).fails(2, "confirmation_required");
    // The lock file the first run created stays beside the two files, and the refusal adds
    // nothing else.
    let mut names: Vec<String> = std::fs::read_dir(home.path().join(".aws"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [".config.lock", ".credentials.lock", "config", "credentials"]
    );
    let credentials = home.path().join(".aws").join("credentials");
    assert!(
        std::fs::read_to_string(&credentials)
            .unwrap()
            .contains("first-secret")
    );
    run(&second, true).ok();
    let text = std::fs::read_to_string(&credentials).unwrap();
    assert!(
        text.contains("second-secret") && !text.contains("first-secret"),
        "{text}"
    );
    assert_eq!(text.matches("[hippius]").count(), 1);
}

/// The AWS CLI expands `$VAR` and then a leading `~` in both variables (botocore 1.35.0,
/// `configloader.py` `raw_config_parse`), and `hipc` does not, so such a path is refused
/// rather than written under the working directory as `./~/...`. Nothing is
/// written, in the working directory or at the expanded path.
#[test]
fn configure_refuses_an_aws_path_the_aws_cli_would_expand() {
    let literal = |home: &Home| home.path().join("lit").display().to_string();
    for (variable, value) in [
        ("AWS_CONFIG_FILE", "~/.aws/config"),
        ("AWS_CONFIG_FILE", "~"),
        ("AWS_CONFIG_FILE", "~root/.aws/config"),
        ("AWS_SHARED_CREDENTIALS_FILE", "$HOME/.aws/credentials"),
        ("AWS_SHARED_CREDENTIALS_FILE", "${HOME}/.aws/credentials"),
        ("AWS_SHARED_CREDENTIALS_FILE", "/tmp/$USER/credentials"),
        // Set but empty: botocore reads `""` as the path, not as unset, so the default
        // `~/.aws/` that hipc would write is not where the AWS CLI looks.
        ("AWS_CONFIG_FILE", ""),
        ("AWS_SHARED_CREDENTIALS_FILE", ""),
    ] {
        let home = Home::new();
        let other = if variable == "AWS_CONFIG_FILE" {
            "AWS_SHARED_CREDENTIALS_FILE"
        } else {
            "AWS_CONFIG_FILE"
        };
        let error = Hipc::new(&home)
            .current_dir(home.path())
            .env(variable, value)
            .env(other, &literal(&home))
            .stdin(SECRET.as_bytes())
            .run(&[
                "s3",
                "configure",
                "--access-key-id",
                "hip_K",
                "--secret-key-file",
                "-",
                "--yes",
            ])
            .fails(2, "usage");
        let (message, hint) = if value.is_empty() {
            (
                format!(
                    "`{variable}` is set but empty, which the AWS CLI reads as a path to no \
                     file, so `hipc` would write a file the AWS CLI does not read"
                ),
                format!("unset `{variable}`, or set it to the file's path"),
            )
        } else {
            (
                format!(
                    "`{variable}` is `{value}`, which the AWS CLI expands and `hipc` does not, \
                     so `hipc` would write a file the AWS CLI does not read"
                ),
                format!("set `{variable}` to the path with `~` and every `$` variable written out"),
            )
        };
        assert_eq!(error["message"], message, "{variable}={value}");
        assert_eq!(error["hint"], hint, "{variable}={value}");
        let left: Vec<_> = std::fs::read_dir(home.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(
            left.iter()
                .all(|name| name != "~" && name != "$HOME" && name != ".aws"),
            "{value}: {left:?}"
        );
        assert!(!home.path().join("lit").exists(), "{value}");
    }
    // A `~` that does not start the path is a plain character to the AWS CLI too.
    let home = Home::new();
    let config = home.path().join("a~b");
    Hipc::new(&home)
        .env("AWS_CONFIG_FILE", config.to_str().unwrap())
        .env("AWS_SHARED_CREDENTIALS_FILE", &literal(&home))
        .stdin(SECRET.as_bytes())
        .run(&[
            "s3",
            "configure",
            "--access-key-id",
            "hip_K",
            "--secret-key-file",
            "-",
            "--yes",
        ])
        .ok();
    assert!(config.exists());
}

#[test]
fn configure_reads_the_secret_from_stdin_and_honors_the_aws_variables() {
    let home = Home::new();
    let config = home.path().join("alt-config");
    let credentials = home.path().join("alt-credentials");
    let doc = Hipc::new(&home)
        .env("AWS_CONFIG_FILE", config.to_str().unwrap())
        .env("AWS_SHARED_CREDENTIALS_FILE", credentials.to_str().unwrap())
        .stdin(b"from-stdin\n")
        .run(&[
            "s3",
            "configure",
            "--aws-profile",
            "default",
            "--access-key-id",
            "hip_K",
            "--secret-key-file",
            "-",
        ])
        .ok();
    assert_eq!(doc["config_path"], config.display().to_string());
    assert!(
        std::fs::read_to_string(&config)
            .unwrap()
            .starts_with("[default]\n")
    );
    assert!(
        std::fs::read_to_string(&credentials)
            .unwrap()
            .contains("aws_secret_access_key = from-stdin\n")
    );
}

#[test]
fn configure_refuses_bad_input_and_writes_nothing() {
    let home = Home::new();
    let secret = home.file("secret", b"ok");
    let spaced = home.file("spaced", b"a b");
    let path = secret.to_str().unwrap();
    for args in [
        &[
            "s3",
            "configure",
            "--access-key-id",
            "AKIA1",
            "--secret-key-file",
            path,
        ][..],
        &[
            "s3",
            "configure",
            "--access-key-id",
            "hip_K",
            "--aws-profile",
            "a]b",
            "--secret-key-file",
            path,
        ],
        &[
            "s3",
            "configure",
            "--access-key-id",
            "hip_K",
            "--secret-key-file",
            spaced.to_str().unwrap(),
        ],
        &[
            "s3",
            "configure",
            "--access-key-id",
            "hip_K",
            "--secret-key-file",
            "/nope/secret",
        ],
        &["s3", "configure", "--access-key-id", "hip_K"],
    ] {
        Hipc::new(&home).run(args).fails(2, "usage");
    }
    assert!(!home.path().join(".aws").exists());
}

/// The document `s3 tokens create --reveal` prints, saved to a file, goes straight into `s3
/// configure`, so the secret never passes through argv or a shell variable.
#[test]
fn configure_takes_the_key_pair_from_a_token_document() {
    let created =
        format!(r#"{{"kind":"master","access_key_id":"hip_DOC","secret":"{SECRET}","id":"i"}}"#);
    let home = Home::new();
    let file = home.file("created.json", created.as_bytes());
    let args = [
        "s3",
        "configure",
        "--from-token-file",
        file.to_str().unwrap(),
    ];
    let run = Hipc::new(&home).run(&args);
    assert_eq!(run.ok()["secret_written"], true);
    assert!(!run.stdout().contains(SECRET) && !run.stderr().contains(SECRET));
    let credentials = home.path().join(".aws").join("credentials");
    assert!(
        std::fs::read_to_string(&credentials)
            .unwrap()
            .contains(&format!(
                "[hippius]\naws_access_key_id = hip_DOC\naws_secret_access_key = {SECRET}\n"
            ))
    );
    // The profile exists now, so it needs `--yes`; the file still holds the secret.
    Hipc::new(&home)
        .run(&args)
        .fails(2, "confirmation_required");
    Hipc::new(&home).run(&[&args[..], &["--yes"]].concat()).ok();
}

/// `--from-token-file` takes a file, never stdin: a one-time secret piped from a create would
/// be lost whenever `s3 configure` then writes nothing, as in a dry run, over a profile that
/// exists without `--yes`, or with a key it refuses. Each is refused before stdin is read.
#[test]
fn configure_refuses_a_token_document_on_stdin() {
    let piped = format!(r#"{{"access_key_id":"hip_P","secret":"{SECRET}"}}"#);
    let home = Home::new();
    let secret = home.file("secret", b"ok");
    // An existing profile, which would need `--yes`.
    Hipc::new(&home)
        .run(&[
            "s3",
            "configure",
            "--access-key-id",
            "hip_K",
            "--secret-key-file",
            secret.to_str().unwrap(),
        ])
        .ok();
    let bad_key = format!(r#"{{"access_key_id":"AKIA1","secret":"{SECRET}"}}"#);
    for (stdin, extra) in [
        (&piped, &["--dry-run"][..]),
        (&piped, &[][..]),
        (&bad_key, &["--yes"][..]),
    ] {
        let mut args = vec!["s3", "configure", "--from-token-file", "-"];
        args.extend(extra);
        let run = Hipc::new(&home).stdin(stdin.as_bytes()).run(&args);
        let error = run.fails(2, "usage");
        assert!(
            error["message"]
                .as_str()
                .expect("a message")
                .contains("reads a file, not stdin"),
            "{error}"
        );
        assert!(!run.stderr().contains(SECRET), "{}", run.stderr());
    }
}

/// A document without a usable key pair is `usage`, names what is missing, never quotes the
/// file, and writes nothing; the flag stands alone, apart from the two it replaces.
#[test]
fn configure_refuses_a_token_document_without_a_key_pair() {
    let home = Home::new();
    let listed = r#"{"kind":"master","access_key_id":"hip_L","secret":null}"#;
    let cases = [
        (listed, "has no `secret`"),
        (
            r#"{"secret":"s3cr3t-QZ7-only-once"}"#,
            "has no `access_key_id`",
        ),
        (
            r#"{"access_key_id":7,"secret":"x"}"#,
            "has no `access_key_id`",
        ),
        (
            r#"[{"secret":"s3cr3t-QZ7-only-once"}]"#,
            "is not a JSON object",
        ),
        ("s3cr3t-QZ7-only-once", "is not a JSON object"),
        // What text mode prints for a create is not JSON either.
        (
            "kind           master\naccess_key_id  hip_A\nsecret         s3cr3t-QZ7-only-once\n",
            "is not a JSON object",
        ),
    ];
    for (document, problem) in cases {
        let file = home.file("doc.json", document.as_bytes());
        let run = Hipc::new(&home).run(&[
            "s3",
            "configure",
            "--from-token-file",
            file.to_str().unwrap(),
        ]);
        let error = run.fails(2, "usage");
        let message = error["message"].as_str().expect("a message");
        assert!(message.contains(problem), "{message}");
        assert!(!run.stderr().contains(SECRET), "{}", run.stderr());
        let hint = error["hint"].as_str().expect("a hint");
        if document == listed {
            // Only a create or a rotate shows the secret.
            assert!(hint.contains("--reveal"), "{error}");
        } else {
            // The hint names the JSON output, which is what the flag reads.
            assert!(hint.contains("hipc --json s3 tokens"), "{error}");
        }
    }
    let secret = home.file("secret", b"ok");
    let doc = home.file("doc.json", b"{}");
    let doc = doc.to_str().unwrap();
    for args in [
        &["s3", "configure"][..],
        &[
            "s3",
            "configure",
            "--from-token-file",
            doc,
            "--access-key-id",
            "hip_K",
        ],
        &[
            "s3",
            "configure",
            "--from-token-file",
            doc,
            "--secret-key-file",
            secret.to_str().unwrap(),
        ],
    ] {
        Hipc::new(&home).run(args).fails(2, "usage");
    }
    assert!(!home.path().join(".aws").exists());
}

#[test]
fn configure_dry_run_writes_nothing() {
    let home = Home::new();
    let secret = home.file("secret", b"ok");
    let doc = Hipc::new(&home)
        .run(&[
            "s3",
            "configure",
            "--access-key-id",
            "hip_K",
            "--secret-key-file",
            secret.to_str().unwrap(),
            "--dry-run",
        ])
        .ok();
    let fields: Vec<&str> = doc
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        fields,
        [
            "dry_run",
            "action",
            "aws_profile",
            "config_path",
            "credentials_path",
            "secret_written"
        ]
    );
    assert_eq!(doc["dry_run"], true);
    assert_eq!(doc["action"], "write_aws_profile");
    assert_eq!(doc["aws_profile"], "hippius");
    assert_eq!(doc["secret_written"], false);
    assert!(!home.path().join(".aws").exists());
}

/// A dry run refuses each AWS file a real run refuses, with the same error, before it
/// writes or asks anything (tenet 5).
#[test]
fn configure_dry_run_refuses_what_the_real_run_refuses() {
    use std::os::unix::fs::PermissionsExt as _;

    type Setup = fn(&Home, &std::path::Path) -> Vec<(&'static str, String)>;

    let private = |path: &std::path::Path, mode: u32| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
    };
    let aws = |home: &Home| {
        let dir = home.path().join(".aws");
        std::fs::create_dir(&dir).expect("mkdir .aws");
        private(&dir, 0o700);
        dir
    };
    let cases: [(&str, Setup); 6] = [
        ("is a symlink", |home, dir| {
            let real = home.file("real-credentials", b"");
            std::os::unix::fs::symlink(real, dir.join("credentials")).expect("symlink");
            Vec::new()
        }),
        ("has permission bits for the group or others", |_, dir| {
            std::fs::write(dir.join("credentials"), b"").expect("write");
            std::fs::set_permissions(
                dir.join("credentials"),
                std::fs::Permissions::from_mode(0o644),
            )
            .expect("chmod 644");
            Vec::new()
        }),
        ("is not UTF-8 text", |_, dir| {
            std::fs::write(dir.join("config"), b"\xff").expect("write");
            std::fs::set_permissions(dir.join("config"), std::fs::Permissions::from_mode(0o600))
                .expect("chmod 600");
            Vec::new()
        }),
        ("can be written by other users", |_, dir| {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o777))
                .expect("chmod 777");
            Vec::new()
        }),
        ("both name", |_, dir| {
            let one = dir.join("one");
            std::fs::write(&one, b"").expect("write");
            std::fs::set_permissions(&one, std::fs::Permissions::from_mode(0o600))
                .expect("chmod 600");
            let one = one.to_str().unwrap().to_owned();
            vec![
                ("AWS_CONFIG_FILE", one.clone()),
                ("AWS_SHARED_CREDENTIALS_FILE", one),
            ]
        }),
        // Two missing names that differ only in case: one file on a case-insensitive volume
        // once created, so the dry run refuses them as the real run does.
        ("both name", |_, dir| {
            vec![
                (
                    "AWS_CONFIG_FILE",
                    dir.join("aws").to_str().unwrap().to_owned(),
                ),
                (
                    "AWS_SHARED_CREDENTIALS_FILE",
                    dir.join("AWS").to_str().unwrap().to_owned(),
                ),
            ]
        }),
    ];
    for (refusal, setup) in cases {
        let home = Home::new();
        let secret = home.file("secret", b"ok");
        let dir = aws(&home);
        let variables = setup(&home, &dir);
        let run = |extra: &[&str]| {
            let mut hipc = Hipc::new(&home);
            for (name, value) in &variables {
                hipc = hipc.env(name, value);
            }
            let mut args = vec![
                "s3",
                "configure",
                "--access-key-id",
                "hip_K",
                "--secret-key-file",
                secret.to_str().unwrap(),
            ];
            args.extend(extra);
            hipc.run(&args).fails(2, "usage")
        };
        let dry = run(&["--dry-run"]);
        assert!(
            dry["message"].as_str().unwrap().contains(refusal),
            "{refusal}: {dry}"
        );
        // The dry run locks nothing; the real run takes the lock before it reads the files.
        assert!(!dir.join(".credentials.lock").exists(), "{refusal}");
        assert_eq!(dry, run(&["--yes"]), "{refusal}");
    }
}

#[test]
fn these_s3_commands_have_a_dry_run_that_sends_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let cases: [(&[&str], &str, &str); 8] = [
        (
            &["s3", "subscription", "get"],
            "GET",
            "/api/s3/subscription/",
        ),
        (
            &["s3", "subscription", "subscribe", "--plan", "pro"],
            "POST",
            "/api/s3/subscription/",
        ),
        (
            &["s3", "subscription", "cancel"],
            "DELETE",
            "/api/s3/subscription/",
        ),
        (
            &["s3", "buckets", "list"],
            "GET",
            "/api/objectstore/buckets/",
        ),
        (
            &["s3", "buckets", "delete", "logs"],
            "DELETE",
            "/api/objectstore/buckets/logs/",
        ),
        (&["s3", "plans"], "GET", "/api/s3/plans/"),
        (
            &["s3", "tokens", "revoke", "--kind", "master", ID],
            "POST",
            "/api/objectstore/master-tokens/0f8fad5b-d9cb-469f-a165-70867728950e/revoke/",
        ),
        (
            &["s3", "tokens", "rotate", "--kind", "sub", ID],
            "POST",
            "/api/objectstore/sub-tokens/0f8fad5b-d9cb-469f-a165-70867728950e/rotate/",
        ),
    ];
    for (args, method, path) in cases {
        let mut full = args.to_vec();
        full.push("--dry-run");
        let doc = Hipc::against(&home, &server).run(&full).ok();
        assert_eq!(draft(&doc)["method"], method, "{args:?}");
        assert_eq!(
            draft(&doc)["url"],
            format!("{}{path}", server.url()),
            "{args:?}"
        );
    }
    assert!(server.requests().is_empty());
}

#[test]
fn every_scope_type_has_its_wire_name() {
    let server = Server::start(vec![]);
    for (flag, wire) in [
        ("single-bucket", "single_bucket"),
        ("multi-bucket", "multi_bucket"),
        ("global", "global"),
    ] {
        let doc = Hipc::against(&Home::new(), &server)
            .run(&[
                "s3",
                "tokens",
                "create",
                "--kind",
                "sub",
                "--scope-type",
                flag,
                "--dry-run",
            ])
            .ok();
        assert_eq!(draft(&doc)["body"]["scope_type"], wire);
    }
}

#[test]
fn configure_without_home_or_with_a_file_for_a_directory_is_refused() {
    let home = Home::new();
    let secret = home.file("secret", b"ok");
    let path = secret.to_str().unwrap();
    Hipc::new(&home)
        .env_remove("HOME")
        .run(&[
            "s3",
            "configure",
            "--access-key-id",
            "hip_K",
            "--secret-key-file",
            path,
        ])
        .fails(2, "usage");
    let not_a_dir = home.file("plain", b"");
    let inside = not_a_dir.join("config");
    let error = Hipc::new(&home)
        .env("AWS_CONFIG_FILE", inside.to_str().unwrap())
        .run(&[
            "s3",
            "configure",
            "--access-key-id",
            "hip_K",
            "--secret-key-file",
            path,
        ])
        .fails(2, "usage");
    assert_eq!(
        error["hint"],
        "set `AWS_CONFIG_FILE` to a file in a directory"
    );
    assert!(!home.path().join(".aws").join("credentials").exists());
}

#[test]
fn configure_refuses_a_symlinked_aws_directory_and_writes_nothing() {
    let home = Home::new();
    let secret = home.file("secret", b"ok");
    let real = home.path().join("dotfiles");
    std::fs::create_dir(&real).expect("mkdir dotfiles");
    std::os::unix::fs::symlink(&real, home.path().join(".aws")).expect("symlink .aws");
    let error = Hipc::new(&home)
        .run(&[
            "s3",
            "configure",
            "--access-key-id",
            "hip_K",
            "--secret-key-file",
            secret.to_str().unwrap(),
        ])
        .fails(2, "usage");
    assert_eq!(
        error["hint"],
        "set `AWS_CONFIG_FILE` to a file in the directory the symlink points to"
    );
    assert_eq!(std::fs::read_dir(&real).expect("list").count(), 0);
}

#[test]
fn buckets_list_takes_the_live_object_shape_and_keeps_its_totals() {
    // The shape api.hippius.com answers, which its swagger documents as a bare array.
    let body = concat!(
        r#"{"buckets":[{"name":"a"},{"name":"b"}],"count":2,"total_objects":9,"#,
        r#""total_size_bytes":99000000000000000000,"main_account_id":"m"}"#
    );
    let server = Server::start(vec![route_of(&s3::BUCKETS, [], Reply::json(200, body))]);
    let run = Hipc::against(&Home::new(), &server).run(&["s3", "buckets", "list"]);
    let doc = run.ok();
    assert_eq!(doc["items"], json!([{"name": "a"}, {"name": "b"}]));
    assert_eq!(doc["next_page_token"], serde_json::Value::Null);
    assert_eq!(doc["count"], 2);
    assert_eq!(doc["main_account_id"], "m");
    assert!(run.stdout().contains("99000000000000000000"));
    let odd = Server::start(vec![route_of(
        &s3::BUCKETS,
        [],
        Reply::json(200, r#"{"count":0}"#),
    )]);
    Hipc::against(&Home::new(), &odd)
        .run(&["s3", "buckets", "list"])
        .fails(1, "internal");
}

#[test]
fn buckets_list_with_a_membership_sends_the_account_header() {
    let server = Server::start(vec![route_of(&s3::BUCKETS, [], Reply::json(200, "[]"))]);
    Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["s3", "buckets", "list"])
        .ok();
    assert_eq!(
        server.only_request().header("x-hippius-account"),
        Some(MEMBER)
    );
}

#[test]
fn subscribe_404_and_409_map_to_their_codes() {
    for (status, code) in [(404, "not_found"), (409, "conflict")] {
        let server = Server::start(vec![
            plans(),
            route_of(&s3::SUBSCRIBE, [], Reply::json(status, r#"{"error":"no"}"#)),
        ]);
        let error = Hipc::against(&Home::new(), &server)
            .run(&["s3", "subscription", "subscribe", "--plan", "pro", "--yes"])
            .fails(1, code);
        assert_eq!(error["side_effects"], "none");
    }
}

#[test]
fn tokens_list_follows_every_page_of_the_sub_keys() {
    let server = Server::start(vec![
        route_of(&s3::MASTER_TOKENS, [], Reply::json(200, "[]")),
        // The query-exact route comes first: the plain one ignores the query.
        route(
            "GET",
            "/api/objectstore/sub-tokens/?page=2",
            Reply::json(
                200,
                r#"{"count":2,"next":null,"previous":null,"results":[{"id":"b"}]}"#,
            ),
        ),
        route_of(
            &s3::SUB_TOKENS,
            [],
            Reply::json(
                200,
                concat!(
                    r#"{"count":2,"next":"{base}/api/objectstore/sub-tokens/?page=2","#,
                    r#""previous":null,"results":[{"id":"a"}]}"#
                ),
            ),
        ),
    ]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["s3", "tokens", "list"])
        .ok();
    let ids: Vec<&str> = doc["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["a", "b"]);
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn tokens_list_refuses_a_next_link_that_loops() {
    let server = Server::start(vec![
        route_of(&s3::MASTER_TOKENS, [], Reply::json(200, "[]")),
        route_of(
            &s3::SUB_TOKENS,
            [],
            Reply::json(
                200,
                concat!(
                    r#"{"count":2,"next":"{base}/api/objectstore/sub-tokens/?page=1","#,
                    r#""previous":null,"results":[]}"#
                ),
            ),
        ),
    ]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["s3", "tokens", "list"])
        .fails(1, "internal");
    assert!(
        error["message"].as_str().unwrap().contains("leads back to"),
        "{error}"
    );
}

/// A server that ignores `page` answers page 1 to every request; when it names the page it
/// sent, a rising `next` cannot repeat the list page after page.
#[test]
fn tokens_list_refuses_a_page_that_is_not_the_one_asked() {
    let server = Server::start(vec![
        route_of(&s3::MASTER_TOKENS, [], Reply::json(200, "[]")),
        route(
            "GET",
            "/api/objectstore/sub-tokens/?page=2",
            Reply::json(
                200,
                concat!(
                    r#"{"page":1,"next":"{base}/api/objectstore/sub-tokens/?page=3","#,
                    r#""results":[{"id":"a"}]}"#
                ),
            ),
        ),
        route_of(
            &s3::SUB_TOKENS,
            [],
            Reply::json(
                200,
                concat!(
                    r#"{"page":1,"next":"{base}/api/objectstore/sub-tokens/?page=2","#,
                    r#""results":[{"id":"a"}]}"#
                ),
            ),
        ),
    ]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["s3", "tokens", "list"])
        .fails(1, "internal");
    assert_eq!(
        error["message"],
        "the API answered page `1` to a request for page `2`, so the list cannot be read in order"
    );
    assert_eq!(
        error["hint"],
        "run the command again, as the list may have changed while it was read"
    );
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn an_s3_subscribe_503_with_retry_after_did_nothing() {
    let server = Server::start(vec![
        plans(),
        route_of(
            &s3::SUBSCRIBE,
            [],
            Reply::json(503, r#"{"error":"Chain unreachable"}"#).header("Retry-After", "2"),
        ),
    ]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["s3", "subscription", "subscribe", "--plan", "pro", "--yes"])
        .fails(1, "unavailable");
    assert_eq!(error["side_effects"], "none");
    assert_eq!(error["retryable"], true);
}

/// A subscribe answered 502, a 503 without a numeric `Retry-After`, or no answer at all may have
/// bought the plan, so each is `partial` and not retryable.
#[test]
fn an_s3_subscribe_502_503_or_timeout_may_have_bought_the_plan() {
    let subscribe = [
        "s3",
        "subscription",
        "subscribe",
        "--plan",
        "pro",
        "--yes",
        "--request-timeout",
        "1",
    ];
    for (reply, code) in [
        (Reply::json(502, r#"{"error":"x"}"#), "unavailable"),
        (Reply::json(503, r#"{"error":"x"}"#), "unavailable"),
        (
            Reply::json(503, r#"{"error":"x"}"#).header("Retry-After", "soon"),
            "unavailable",
        ),
        (Reply::never(), "timeout"),
    ] {
        let server = Server::start(vec![plans(), route_of(&s3::SUBSCRIBE, [], reply)]);
        let error = Hipc::against(&Home::new(), &server)
            .run(&subscribe)
            .fails(1, code);
        assert_eq!(error["side_effects"], "partial", "{error}");
        assert_eq!(error["retryable"], false, "{error}");
        assert_eq!(server.requests().len(), 2);
    }
}

#[test]
fn checkout_shows_a_card_checkout_and_sends_the_membership_header() {
    let server = Server::start(vec![route_of(
        &s3::CHECKOUT,
        [&"pi_1"],
        Reply::json(200, r#"{"intent_id":"pi_1","status":"fulfilling"}"#),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["s3", "subscription", "checkout", "pi_1"])
        .ok();
    assert_eq!(doc["status"], "fulfilling");
    assert_eq!(
        server.only_request().header("x-hippius-account"),
        Some(MEMBER)
    );
    let dry = Hipc::against(&Home::new(), &server)
        .run(&["s3", "subscription", "checkout", "pi_1", "--dry-run"])
        .ok();
    assert_eq!(draft(&dry)["method"], "GET");
    assert_eq!(server.requests().len(), 1);
}
