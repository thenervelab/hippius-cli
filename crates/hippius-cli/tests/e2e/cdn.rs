//! `hipc cdn`: prices, regions, the estimate, zones, the spend cap, hostnames, purges, rules,
//! and stats.

use hippius::routes::cdn;
use serde_json::{Value, json};

use crate::common::{Hipc, Home, MEMBER, Recorded, Reply, Route, Run, Server, draft, route_of};

const ZONE: &str = "zab12cd34ef56";

const ZONE_BODY: &str = concat!(
    r#"{"id":"zab12cd34ef56","name":"site","state":"active","#,
    r#""origin":{"kind":"s3","bucket":"web","prefix":"site/","private":false}}"#
);

fn hipc(server: &Server, args: &[&str]) -> Run {
    let mut argv = vec!["cdn"];
    argv.extend(args);
    Hipc::against(&Home::new(), server).run(&argv)
}

/// Runs `hipc cdn` in text mode, as a person at a terminal would see it.
fn text(server: &Server, args: &[&str]) -> Run {
    let mut argv = vec!["--output", "text", "cdn"];
    argv.extend(args);
    Hipc::against(&Home::new(), server).run(&argv)
}

#[test]
fn prices_need_no_token_and_print_as_sent() {
    let prices = concat!(
        r#"{"at":"2026-10-27T00:00:00Z","regions":["FR","AU"],"#,
        r#""requests":{"unit":"10000 requests","unit_price":"2000000000000000"}}"#
    );
    let server = Server::start(vec![route_of(&cdn::PRICES, [], Reply::json(200, prices))]);
    let run = Hipc::tokenless(&Home::new(), &server).run(&["cdn", "prices"]);
    assert_eq!(run.stdout().trim_end(), prices);
    assert_eq!(run.code(), Some(0));
    assert_eq!(server.only_request().header("authorization"), None);
}

#[test]
fn objects_print_as_the_server_sent_them() {
    let regions = r#"{"regions":[{"code":"FR","nodes_reporting":3,"status":"up"}],"domain":"c.x"}"#;
    let cap = r#"{"monthly_cap":"20000000000000000000","monthly_cap_credits":"20.000000"}"#;
    let server = Server::start(vec![
        route_of(&cdn::REGIONS, [], Reply::json(200, regions)),
        route_of(&cdn::ZONE, [&ZONE], Reply::json(200, ZONE_BODY)),
        route_of(&cdn::CAP, [&ZONE], Reply::json(200, cap)),
        route_of(&cdn::RULES, [&ZONE], Reply::json(200, r#"{"rules":[]}"#)),
        route_of(
            &cdn::PURGE,
            [&ZONE, &7],
            Reply::json(200, r#"{"id":7,"status":"done"}"#),
        ),
        route_of(
            &cdn::STATS,
            [&ZONE],
            Reply::json(200, r#"{"step_s":3600,"series":[]}"#),
        ),
    ]);
    assert_eq!(hipc(&server, &["regions"]).ok()["domain"], "c.x");
    assert_eq!(hipc(&server, &["zones", "get", ZONE]).ok()["id"], ZONE);
    // The cap is printed as sent, in base units and in credits: never converted.
    let shown = hipc(&server, &["cap", "get", ZONE]);
    assert_eq!(shown.stdout().trim_end(), cap);
    assert_eq!(
        hipc(&server, &["rules", "get", ZONE]).ok(),
        json!({"rules": []})
    );
    assert_eq!(
        hipc(&server, &["purges", "get", ZONE, "7"]).ok()["status"],
        "done"
    );
    assert_eq!(hipc(&server, &["stats", ZONE]).ok()["step_s"], 3600);
    let paths: Vec<String> = server.requests().iter().map(|r| r.target.clone()).collect();
    assert_eq!(
        paths,
        [
            "/api/cdn/regions/".to_owned(),
            format!("/api/cdn/zones/{ZONE}/"),
            format!("/api/cdn/zones/{ZONE}/cap/"),
            format!("/api/cdn/zones/{ZONE}/rules/"),
            format!("/api/cdn/zones/{ZONE}/purges/7/"),
            format!("/api/cdn/zones/{ZONE}/stats/"),
        ]
    );
    assert!(server.requests().iter().all(|r| r.method == "GET"));
}

#[test]
fn zones_list_keeps_beta_beside_the_items() {
    let body = format!(r#"{{"zones":[{ZONE_BODY}],"beta":false}}"#);
    let server = Server::start(vec![route_of(&cdn::ZONES, [], Reply::json(200, &body))]);
    let doc = hipc(&server, &["zones", "list"]).ok();
    assert_eq!(doc["items"][0]["id"], ZONE);
    assert_eq!(doc["beta"], false);
    assert_eq!(doc["next_page_token"], json!(null));
    assert!(doc.get("zones").is_none(), "{doc}");
}

#[test]
fn hostname_and_purge_lists_read_an_object_or_a_bare_array() {
    let server = Server::start(vec![
        route_of(
            &cdn::HOSTNAMES,
            [&ZONE],
            Reply::json(
                200,
                r#"{"hostnames":[{"hostname":"cdn.example.com"}],"count":1}"#,
            ),
        ),
        route_of(
            &cdn::PURGES,
            [&ZONE],
            Reply::json(200, r#"[{"id":7},{"id":8}]"#),
        ),
    ]);
    let hostnames = hipc(&server, &["hostnames", "list", ZONE]).ok();
    assert_eq!(
        hostnames,
        json!({
            "items": [{"hostname": "cdn.example.com"}],
            "count": 1,
            "next_page_token": null,
        })
    );
    let purges = hipc(&server, &["purges", "list", ZONE]).ok();
    assert_eq!(purges["items"][1]["id"], 8);
    assert_eq!(purges["next_page_token"], json!(null));
}

#[test]
fn a_list_object_without_its_key_is_internal() {
    let server = Server::start(vec![route_of(
        &cdn::PURGES,
        [&ZONE],
        Reply::json(200, r#"{"items":[]}"#),
    )]);
    let error = hipc(&server, &["purges", "list", ZONE]).fails(1, "internal");
    assert_eq!(
        error["message"],
        format!(
            "cannot list the purges of the CDN zone `{ZONE}`: the API answered without a \
             `purges` list"
        )
    );
}

#[test]
fn zones_create_needs_yes_and_sends_nothing_without_it() {
    let server = Server::start(vec![]);
    let error = hipc(
        &server,
        &[
            "zones", "create", "--name", "site", "--bucket", "web", "--public",
        ],
    )
    .fails(2, "confirmation_required");
    let message = error["message"].as_str().unwrap();
    assert!(
        message.contains("`web/`") && message.contains("public"),
        "{message}"
    );
    assert!(server.requests().is_empty());
}

#[test]
fn zones_create_dry_run_sends_only_the_flags_given() {
    let server = Server::start(vec![]);
    let doc = hipc(
        &server,
        &[
            "zones",
            "create",
            "--name",
            "site",
            "--bucket",
            "web",
            "--dry-run",
        ],
    )
    .ok();
    assert_eq!(draft(&doc)["method"], "POST");
    assert_eq!(
        draft(&doc)["body"],
        json!({"name": "site", "origin": {"kind": "s3", "bucket": "web"}})
    );
    let doc = hipc(
        &server,
        &[
            "zones",
            "create",
            "--name",
            "site",
            "--bucket",
            "web",
            "--prefix",
            "site/",
            "--shield-region",
            "AU",
            "--monthly-cap",
            "25.5",
            "--index-document",
            "index.html",
            "--spa-fallback",
            "true",
            "--spa-fallback-exclude",
            "/assets/",
            "--spa-fallback-exclude",
            "/img/",
            "--dry-run",
        ],
    )
    .ok();
    assert_eq!(
        draft(&doc)["body"],
        json!({
            "name": "site",
            "origin": {"kind": "s3", "bucket": "web", "prefix": "site/"},
            "shield_region": "AU",
            "index_document": "index.html",
            "spa_fallback": true,
            "spa_fallback_exclude": ["/assets/", "/img/"],
            "monthly_cap": "25.5",
        })
    );
    assert!(server.requests().is_empty());
}

#[test]
fn zones_create_says_in_text_mode_when_it_made_the_prefix_public() {
    let private = ZONE_BODY.replace(r#""private":false"#, r#""private":true"#);
    let public = Server::start(vec![route_of(
        &cdn::ZONE_CREATE,
        [],
        Reply::json(201, ZONE_BODY),
    )]);
    let made_public = Server::start(vec![route_of(
        &cdn::ZONE_CREATE,
        [],
        Reply::json(201, &private),
    )]);
    let create = [
        "zones", "create", "--name", "site", "--bucket", "web", "--prefix", "site/", "--public",
    ];
    let mut confirmed = create.to_vec();
    confirmed.push("--yes");

    let run = text(&made_public, &confirmed);
    assert_eq!(run.code(), Some(0));
    assert_eq!(
        run.stderr(),
        concat!(
            "warning: this zone makes the prefix `web/site/` public: anyone can read it ",
            "through the zone's hostnames\n"
        )
    );
    assert_eq!(
        made_public.only_request().json()["origin"]["prefix"],
        "site/"
    );
    // JSON mode prints the zone and no note; `origin.private` says the same thing.
    assert_eq!(
        hipc(&made_public, &confirmed).ok()["origin"]["private"],
        true
    );
    assert_eq!(text(&public, &confirmed).stderr(), "");
    let mut quiet = vec!["--quiet"];
    quiet.extend(&confirmed);
    assert_eq!(text(&made_public, &quiet).stderr(), "");
}

#[test]
fn zones_create_answered_with_an_empty_body_is_internal_and_partial() {
    let server = Server::start(vec![route_of(&cdn::ZONE_CREATE, [], Reply::json(201, ""))]);
    let error = hipc(
        &server,
        &[
            "zones", "create", "--name", "site", "--bucket", "web", "--public", "--yes",
        ],
    )
    .fails(1, "internal");
    assert_eq!(error["side_effects"], "partial", "the zone may exist");
}

#[test]
fn zones_update_sends_only_the_keys_passed() {
    let server = Server::start(vec![route_of(
        &cdn::ZONE_UPDATE,
        [&ZONE],
        Reply::json(200, ZONE_BODY),
    )]);
    let doc = hipc(
        &server,
        &["zones", "update", ZONE, "--name", "new", "--yes"],
    )
    .ok();
    assert_eq!(doc["id"], ZONE);
    assert_eq!(server.only_request().json(), json!({"name": "new"}));
    let doc = hipc(
        &server,
        &[
            "zones",
            "update",
            ZONE,
            "--redirect-https",
            "false",
            "--bucket",
            "other",
            "--spa-fallback",
            "false",
            "--dry-run",
        ],
    )
    .ok();
    assert_eq!(
        draft(&doc)["body"],
        json!({"redirect_https": false, "spa_fallback": false, "origin": {"bucket": "other"}})
    );
    assert_eq!(server.requests().len(), 1);
}

/// A zone name is one line of 1 to 64 characters, checked before anything is sent.
#[test]
fn a_zone_name_outside_the_label_rules_is_usage() {
    let server = Server::start(vec![]);
    let long = "n".repeat(65);
    for bad in ["", " ", "a\nb", long.as_str()] {
        for args in [
            vec![
                "zones",
                "create",
                "--name",
                bad,
                "--bucket",
                "web",
                "--public",
                "--dry-run",
            ],
            vec!["zones", "update", ZONE, "--name", bad, "--dry-run"],
        ] {
            let error = hipc(&server, &args).fails(2, "usage");
            assert!(
                error["message"]
                    .as_str()
                    .is_some_and(|m| m.contains(if bad.contains('\n') {
                        "a zone name holds no control character or line break"
                    } else {
                        "a zone name is one line of 1 to 64 characters"
                    })),
                "{bad:?}: {error}"
            );
        }
    }
    let longest = "n".repeat(64);
    hipc(
        &server,
        &["zones", "update", ZONE, "--name", &longest, "--dry-run"],
    )
    .ok();
    assert!(server.requests().is_empty());
}

/// `--prefix` without `--bucket` reads the zone and sends the whole origin with the new
/// prefix, which is right whether the PATCH merges `origin` or replaces it. The dry run lists
/// the read, then the PATCH with placeholders.
#[test]
fn zones_update_of_the_prefix_alone_sends_the_whole_origin() {
    let server = Server::start(vec![
        route_of(&cdn::ZONE, [&ZONE], Reply::json(200, ZONE_BODY)),
        route_of(&cdn::ZONE_UPDATE, [&ZONE], Reply::json(200, ZONE_BODY)),
    ]);
    let doc = hipc(
        &server,
        &[
            "zones",
            "update",
            ZONE,
            "--prefix",
            "",
            "--public",
            "--dry-run",
        ],
    )
    .ok();
    let requests = doc["requests"].as_array().expect("the requests");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["method"], "GET");
    assert_eq!(
        requests[1]["body"],
        json!({"origin": {
            "kind": "<the zone's origin kind>",
            "bucket": "<the zone's bucket>",
            "prefix": "",
        }})
    );
    assert!(server.requests().is_empty());
    hipc(
        &server,
        &[
            "zones", "update", ZONE, "--prefix", "www/", "--public", "--yes",
        ],
    )
    .ok();
    let sent = server.requests();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].method, "GET");
    assert_eq!(
        sent[1].json(),
        json!({"origin": {"kind": "s3", "bucket": "web", "prefix": "www/"}})
    );
    // A zone whose origin names no bucket is not sent a partial origin.
    let bare = Server::start(vec![route_of(
        &cdn::ZONE,
        [&ZONE],
        Reply::json(200, r#"{"id":"zab12cd34ef56","origin":{"kind":"s3"}}"#),
    )]);
    let error = hipc(
        &bare,
        &[
            "zones", "update", ZONE, "--prefix", "www/", "--public", "--yes",
        ],
    )
    .fails(1, "internal");
    assert_eq!(error["hint"], "pass `--bucket` with `--prefix`");
    assert_eq!(bare.only_request().method, "GET");
}

/// Swagger's example CORS policy for `cors` on `cdn_zones_create`.
const CORS: &str = concat!(
    r#"{"allow_origins":["https://app.example.com","https://*.example.org"],"#,
    r#""allow_methods":["GET","HEAD"],"allow_headers":["range"],"max_age":600}"#
);

/// `--cors-file` sends the file's policy as `cors`, the whole policy, on create and update; `null`
/// removes it. The question says the policy is replaced whole.
#[test]
fn zones_take_a_whole_cors_policy_from_a_file() {
    let server = Server::start(vec![route_of(
        &cdn::ZONE_UPDATE,
        [&ZONE],
        Reply::json(200, ZONE_BODY),
    )]);
    let home = Home::new();
    let cors = home.file("cors.json", CORS.as_bytes());
    let cors = cors.to_str().unwrap();
    let none = home.file("none.json", b" null\n");
    let none = none.to_str().unwrap();
    let policy: Value = serde_json::from_str(CORS).expect("a JSON fixture");
    let cdn = |args: &[&str]| Hipc::against(&home, &server).run(&[&["cdn"][..], args].concat());

    let create = [
        "zones", "create", "--name", "site", "--bucket", "web", "--public",
    ];
    let doc = cdn(&[&create[..], &["--cors-file", cors, "--dry-run"]].concat()).ok();
    assert_eq!(draft(&doc)["body"]["cors"], policy);
    let error =
        cdn(&[&create[..], &["--cors-file", cors]].concat()).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!(
            "this would create the CDN zone `site` serving `web/`, which makes that prefix \
             public if the bucket is private, with the whole CORS policy in `{cors}`, and \
             `--yes` was not passed"
        )
    );
    let error =
        cdn(&[&create[..], &["--cors-file", none]].concat()).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        "this would create the CDN zone `site` serving `web/`, which makes that prefix public if \
         the bucket is private, with no CORS policy, and `--yes` was not passed"
    );

    let update = ["zones", "update", ZONE];
    let error =
        cdn(&[&update[..], &["--cors-file", cors]].concat()).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!(
            "this would update the CDN zone `{ZONE}`: replace its whole CORS policy with the one \
             in `{cors}`, and `--yes` was not passed"
        )
    );
    let error = cdn(&[&update[..], &["--name", "n", "--cors-file", none]].concat())
        .fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!(
            "this would update the CDN zone `{ZONE}`: set `name` to `n`, and remove its CORS \
             policy, and `--yes` was not passed"
        )
    );
    assert!(server.requests().is_empty());

    cdn(&[&update[..], &["--cors-file", cors, "--yes"]].concat()).ok();
    cdn(&[&update[..], &["--cors-file", none, "--yes"]].concat()).ok();
    let requests = server.requests();
    assert_eq!(requests[0].json(), json!({ "cors": policy }));
    assert_eq!(requests[1].json(), json!({ "cors": null }));
}

/// A CORS file must hold a JSON object or `null`; anything else is usage before a request.
#[test]
fn a_cors_file_that_is_not_an_object_or_null_is_refused() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let huge = vec![b' '; 64 * 1024 + 1];
    for (name, contents) in [
        ("list.json", &b"[\"https://a.example\"]"[..]),
        ("text.json", b"\"*\""),
        ("bad.json", b"{allow_origins"),
        ("huge.json", &huge),
    ] {
        let path = home.file(name, contents);
        let error = Hipc::against(&home, &server)
            .run(&[
                "cdn",
                "zones",
                "update",
                ZONE,
                "--cors-file",
                path.to_str().unwrap(),
                "--yes",
            ])
            .fails(2, "usage");
        let hint = error["hint"].as_str().unwrap();
        assert!(hint.contains("`--cors-file`"), "{name}: {hint}");
    }
    let error = Hipc::against(&home, &server)
        .run(&[
            "cdn",
            "zones",
            "update",
            ZONE,
            "--cors-file",
            home.file("list.json", b"[]").to_str().unwrap(),
            "--yes",
        ])
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        format!(
            "the CORS policy from `{}` is not a JSON object or null",
            home.path().join("list.json").display()
        )
    );
    assert_eq!(
        error["hint"],
        "write the policy as a JSON object with `allow_origins`, or `null` to remove it, in the \
         file `--cors-file` names"
    );
    assert!(server.requests().is_empty());
}

#[test]
fn zones_update_with_nothing_to_change_is_usage_and_always_needs_yes() {
    let server = Server::start(vec![]);
    let error = hipc(&server, &["zones", "update", ZONE, "--yes"]).fails(2, "usage");
    assert_eq!(
        error["hint"],
        "pass at least one field to change, such as `--name`"
    );
    let error =
        hipc(&server, &["zones", "update", ZONE, "--name", "n"]).fails(2, "confirmation_required");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains(&format!("`{ZONE}`"))
    );
    let error = hipc(
        &server,
        &["zones", "update", ZONE, "--bucket", "b", "--public"],
    )
    .fails(2, "confirmation_required");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("purges the whole zone"),
        "{error}"
    );
    assert!(server.requests().is_empty());
}

/// The question names every field a zone update changes, and the bucket and prefix a move
/// would publish, so the refusal an agent reads says what `--yes` would do.
#[test]
fn zones_update_names_each_change_in_the_question() {
    let server = Server::start(vec![route_of(
        &cdn::ZONE,
        [&ZONE],
        Reply::json(200, ZONE_BODY),
    )]);
    let refused = |args: &[&str]| {
        let args = [&["zones", "update", ZONE][..], args].concat();
        hipc(&server, &args).fails(2, "confirmation_required")["message"]
            .as_str()
            .expect("a message")
            .to_owned()
    };
    assert_eq!(
        refused(&["--name", "n", "--redirect-https", "true"]),
        format!(
            "this would update the CDN zone `{ZONE}`: set `name` to `n` and `redirect_https` \
             to `true`, and `--yes` was not passed"
        )
    );
    assert_eq!(
        refused(&[
            "--index-document",
            "",
            "--spa-fallback-exclude",
            "/api/",
            "--spa-fallback-exclude",
            "/docs/",
        ]),
        format!(
            "this would update the CDN zone `{ZONE}`: set `index_document` to `\"\"` and \
             `spa_fallback_exclude` to `/api/` and `/docs/`, and `--yes` was not passed"
        )
    );
    assert_eq!(
        refused(&["--bucket", "site", "--prefix", "www/", "--public"]),
        format!(
            "this would move the CDN zone `{ZONE}` to the bucket `site` and the prefix `www/`, \
             which purges the whole zone and makes what it then serves public if the bucket is \
             private, and `--yes` was not passed"
        )
    );
    assert_eq!(
        refused(&["--prefix", "", "--name", "n", "--public"]),
        format!(
            "this would move the CDN zone `{ZONE}` to the bucket `web` and the prefix `\"\"`, \
             which purges the whole zone and makes what it then serves public if the bucket is \
             private, and set `name` to `n`, and `--yes` was not passed"
        )
    );
    // Only the prefix alone reads the zone, for its bucket.
    assert_eq!(server.only_request().method, "GET");
}

#[test]
fn zones_delete_prints_a_document_for_204_and_maps_404() {
    let server = Server::start(vec![route_of(
        &cdn::ZONE_DELETE,
        [&ZONE],
        Reply::json(204, ""),
    )]);
    assert_eq!(
        hipc(&server, &["zones", "delete", ZONE, "--yes"]).ok(),
        json!({"zone_id": ZONE, "deleted": true})
    );
    hipc(&server, &["zones", "delete", ZONE]).fails(2, "confirmation_required");
    assert_eq!(server.requests().len(), 1);
    let gone = Server::start(vec![route_of(
        &cdn::ZONE_DELETE,
        [&ZONE],
        Reply::json(404, r#"{"error":"zone not found","code":"not_found"}"#),
    )]);
    hipc(&gone, &["zones", "delete", ZONE, "--yes"]).fails(1, "not_found");
}

#[test]
fn cap_update_sends_the_cap_as_a_string_and_nothing_else() {
    let cap = r#"{"monthly_cap":"25000000000000000000","monthly_cap_credits":"25.000000"}"#;
    let in_force = r#"{"monthly_cap":"20000000000000000000","monthly_cap_credits":"20.000000"}"#;
    let server = Server::start(vec![
        route_of(&cdn::CAP, [&ZONE], Reply::json(200, in_force)),
        route_of(&cdn::CAP_UPDATE, [&ZONE], Reply::json(200, cap)),
    ]);
    let run = hipc(
        &server,
        &["cap", "update", ZONE, "--monthly-cap", "25", "--yes"],
    );
    assert_eq!(run.stdout().trim_end(), cap);
    let sent = server.requests();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].method, "GET");
    assert_eq!(sent[1].method, "PATCH");
    assert_eq!(sent[1].body, br#"{"monthly_cap":"25"}"#);
    // A raise over the cap the read shows asks, and names both amounts.
    let error = hipc(&server, &["cap", "update", ZONE, "--monthly-cap", "25"])
        .fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!(
            "this would raise the monthly spend cap of the CDN zone `{ZONE}` from \
             `20.000000` to `25` credits, and `--yes` was not passed"
        )
    );
    assert_eq!(server.requests().len(), 3, "the read, and no PATCH");
    for bad in [
        "",
        "-1",
        "2e3",
        "1.",
        ".5",
        "1.2.3",
        "twenty",
        "0",
        "0.5",
        "100000.5",
        "1.0000000000000000001",
        "20.0000000000000000001",
    ] {
        hipc(
            &server,
            &["cap", "update", ZONE, "--monthly-cap", bad, "--yes"],
        )
        .fails(2, "usage");
    }
    assert_eq!(server.requests().len(), 3);
}

/// A cut, or the same cap written another way, spends no more, so it is sent without
/// `--yes`, as the other spend caps are; a cap the read cannot compare asks. The exact
/// `monthly_cap` in base units decides, since `monthly_cap_credits` is rounded.
#[test]
fn cap_update_asks_only_for_a_raise() {
    let in_force = |base: &str, credits: &str| {
        format!(r#"{{"monthly_cap":{base},"monthly_cap_credits":{credits}}}"#)
    };
    let twenty = r#""20000000000000000000""#;
    for (base, credits, cap, sent_cap) in [
        (twenty, r#""20.000000""#, "10", "10"),
        (twenty, r#""20.000000""#, "20", "20"),
        // Sent in its normal form, as the runner limit is.
        (twenty, r#""20.000000""#, "020.0", "20"),
        (twenty, r#""20.000000""#, "20.0000000000000000000", "20"),
        (twenty, "20", "19.99", "19.99"),
        // Without base units, the credits decide.
        ("null", r#""20.000000""#, "10", "10"),
        ("null", "20", "20", "20"),
    ] {
        let server = Server::start(vec![
            route_of(
                &cdn::CAP,
                [&ZONE],
                Reply::json(200, &in_force(base, credits)),
            ),
            route_of(&cdn::CAP_UPDATE, [&ZONE], Reply::json(200, "{}")),
        ]);
        hipc(&server, &["cap", "update", ZONE, "--monthly-cap", cap]).ok();
        let sent = server.requests();
        assert_eq!(sent.len(), 2, "{base} {credits} {cap}");
        assert_eq!(sent[1].method, "PATCH");
        assert_eq!(
            sent[1].body,
            format!(r#"{{"monthly_cap":"{sent_cap}"}}"#).as_bytes()
        );
    }
    for (base, credits, cap, words) in [
        (
            twenty,
            r#""20.000000""#,
            "20.01",
            "from `20.000000` to `20.01` credits",
        ),
        // The credits read rounded: the base units show a raise.
        (
            r#""19999999600000000000""#,
            r#""20.000000""#,
            "19.9999999",
            "from `20.000000` to `19.9999999` credits",
        ),
        (twenty, "null", "20.01", "to `20.01` credits"),
        ("null", "null", "20.01", "where the cap in force is unknown"),
        (
            "null",
            r#""twenty""#,
            "20.01",
            "where `monthly_cap_credits` is `twenty`, which is not an amount",
        ),
    ] {
        let server = Server::start(vec![route_of(
            &cdn::CAP,
            [&ZONE],
            Reply::json(200, &in_force(base, credits)),
        )]);
        let error = hipc(&server, &["cap", "update", ZONE, "--monthly-cap", cap])
            .fails(2, "confirmation_required");
        let message = error["message"].as_str().unwrap();
        assert!(message.contains(words), "{base} {credits}: {message}");
        assert_eq!(server.only_request().method, "GET");
    }
    // A read without either field is unknown too.
    let server = Server::start(vec![route_of(&cdn::CAP, [&ZONE], Reply::json(200, "{}"))]);
    hipc(&server, &["cap", "update", ZONE, "--monthly-cap", "1"]).fails(2, "confirmation_required");
    // A failed read sends no PATCH.
    let server = Server::start(vec![route_of(&cdn::CAP, [&ZONE], Reply::not_found())]);
    hipc(
        &server,
        &["cap", "update", ZONE, "--monthly-cap", "1", "--yes"],
    )
    .fails(1, "not_found");
    assert_eq!(server.only_request().method, "GET");
}

#[test]
fn cap_update_dry_run_lists_the_read_and_the_patch() {
    let server = Server::start(vec![]);
    let doc = hipc(
        &server,
        &["cap", "update", ZONE, "--monthly-cap", "5", "--dry-run"],
    )
    .ok();
    let url = format!("{}/api/cdn/zones/{ZONE}/cap/", server.url());
    assert_eq!(doc["requests"][1]["body"], json!({ "monthly_cap": "5" }));
    assert_eq!(doc["requests"][0]["method"], "GET");
    assert_eq!(doc["requests"][0]["url"], url);
    assert_eq!(doc["requests"][1]["method"], "PATCH");
    assert_eq!(doc["requests"][1]["url"], url);
    assert!(server.requests().is_empty());
}

#[test]
fn purge_needs_exactly_one_target_and_no_confirmation() {
    let purge = r#"{"id":9,"kind":"all","status":"pending"}"#;
    let server = Server::start(vec![route_of(
        &cdn::PURGE_CREATE,
        [&ZONE],
        Reply::json(201, purge),
    )]);
    for args in [
        &["purges", "create", ZONE][..],
        &["purges", "create", ZONE, "--everything", "--path", "/a"],
        &["purges", "create", ZONE, "--path", "/a", "--prefix", "/b/"],
        &["purges", "create", ZONE, "--everything", "--prefix", "/b/"],
    ] {
        hipc(&server, args).fails(2, "usage");
    }
    assert!(server.requests().is_empty());
    assert_eq!(
        hipc(&server, &["purges", "create", ZONE, "--everything"]).ok()["id"],
        9
    );
    hipc(
        &server,
        &["purges", "create", ZONE, "--path", "/a", "--path", "/b"],
    )
    .ok();
    hipc(&server, &["purges", "create", ZONE, "--prefix", "/img/"]).ok();
    let bodies: Vec<_> = server.requests().iter().map(Recorded::json).collect();
    assert_eq!(
        bodies,
        [
            json!({"all": true}),
            json!({"paths": ["/a", "/b"]}),
            json!({"prefixes": ["/img/"]}),
        ]
    );
}

/// The parsed message of a usage refusal, for the checks below.
fn refused(server: &Server, args: &[&str]) -> String {
    let error = hipc(server, args).fails(2, "usage");
    error["message"]
        .as_str()
        .expect("the message is a string")
        .to_owned()
}

/// Swagger: purge lists hold 1 to 1000 percent-encoded paths starting with `/`, and a
/// prefix ends in `/`. Each is checked before anything is sent.
#[test]
fn purge_paths_and_prefixes_are_checked_without_a_request() {
    let server = Server::start(vec![route_of(
        &cdn::PURGE_CREATE,
        [&ZONE],
        Reply::json(201, "{}"),
    )]);
    let encoded_rule = "is percent-encoded, such as `/img/a%20b.jpg`: a space, `?`, `#`, `%` \
                        itself, and every byte that is not ASCII are written as `%XX`";
    let path_rule = format!("a purge path starts with `/` and {encoded_rule}");
    let prefix_rule = format!("a purge prefix starts and ends with `/` and {encoded_rule}");
    for path in [
        "a.jpg",
        "/a b.jpg",
        "/a?v=1",
        "/a#x",
        "/caf\u{e9}",
        "/a%2",
        "/a%zz",
        "",
    ] {
        let message = refused(&server, &["purges", "create", ZONE, "--path", path]);
        assert!(message.ends_with(&path_rule), "{path}: {message}");
    }
    for prefix in ["/img", "img/", "/a b/"] {
        let message = refused(&server, &["purges", "create", ZONE, "--prefix", prefix]);
        assert!(message.ends_with(&prefix_rule), "{prefix}: {message}");
    }
    let many = |flag: &'static str, value: &'static str, count: usize| {
        let mut args = vec!["purges", "create", ZONE];
        for _ in 0..count {
            args.extend([flag, value]);
        }
        args
    };
    for (flag, value) in [("--path", "/a"), ("--prefix", "/a/")] {
        let mut args = many(flag, value, 1001);
        args.push("--dry-run");
        assert_eq!(
            refused(&server, &args),
            format!("`{flag}` is given 1001 times, and a purge takes at most 1000")
        );
    }
    assert!(server.requests().is_empty());
    hipc(&server, &many("--path", "/a", 1000)).ok();
    hipc(&server, &many("--prefix", "/a/", 1000)).ok();
    let encoded = "/a%20b/c-d_e.f~g!$&'()*+,;=:@%C3%A9";
    hipc(&server, &["purges", "create", ZONE, "--path", encoded]).ok();
    hipc(&server, &["purges", "create", ZONE, "--prefix", "/"]).ok();
    let bodies: Vec<_> = server.requests().iter().map(Recorded::json).collect();
    assert_eq!(bodies[0]["paths"].as_array().map(Vec::len), Some(1000));
    assert_eq!(bodies[1]["prefixes"].as_array().map(Vec::len), Some(1000));
    assert_eq!(bodies[2], json!({ "paths": [encoded] }));
    assert_eq!(bodies[3], json!({ "prefixes": ["/"] }));
}

/// The zone prefix is empty or a folder ending in `/` (swagger and the help), and a SPA
/// fallback exclusion starts and ends with `/`, at most 16 of them.
#[test]
fn zone_prefixes_and_spa_exclusions_are_checked_without_a_request() {
    let server = Server::start(vec![]);
    let prefix_rule = "a zone prefix is empty, for the whole bucket, or a folder that ends with \
                       `/` and does not start with `/`, such as `site/`";
    let exclude_rule = "a SPA fallback exclusion starts and ends with `/`, such as `/assets/`";
    let create = |extra: &[&'static str]| {
        let mut args = vec![
            "zones",
            "create",
            "--name",
            "site",
            "--bucket",
            "web",
            "--dry-run",
        ];
        args.extend(extra);
        args
    };
    let update = |extra: &[&'static str]| {
        let mut args = vec!["zones", "update", ZONE, "--dry-run"];
        args.extend(extra);
        args
    };
    for prefix in ["site", "/site/", "/", "a/b"] {
        for args in [create(&["--prefix", prefix]), update(&["--prefix", prefix])] {
            let message = refused(&server, &args);
            assert!(message.ends_with(&prefix_rule), "{prefix}: {message}");
        }
    }
    hipc(&server, &create(&["--spa-fallback-exclude", "/"])).ok();
    for exclude in ["assets/", "/assets", ""] {
        let flags = ["--spa-fallback-exclude", exclude];
        for args in [create(&flags), update(&flags)] {
            let message = refused(&server, &args);
            assert!(message.ends_with(exclude_rule), "{exclude}: {message}");
        }
    }
    let excludes = |count: usize| {
        let mut flags = Vec::new();
        for _ in 0..count {
            flags.extend(["--spa-fallback-exclude", "/a/"]);
        }
        flags
    };
    for args in [create(&excludes(17)), update(&excludes(17))] {
        assert_eq!(
            refused(&server, &args),
            "`--spa-fallback-exclude` is given 17 times, and a zone keeps at most 16"
        );
    }
    let doc = hipc(&server, &create(&excludes(16))).ok();
    assert_eq!(
        draft(&doc)["body"]["spa_fallback_exclude"]
            .as_array()
            .map(Vec::len),
        Some(16)
    );
    let doc = hipc(&server, &update(&["--prefix", "a/b/"])).ok();
    assert_eq!(draft(&doc)["body"]["origin"]["prefix"], "a/b/");
    assert!(server.requests().is_empty());
}

/// Swagger: each GB value is 0 to 10^9, and `requests` and `custom_domains` at most 10^15.
#[test]
fn estimate_values_past_the_api_limits_are_usage_without_a_request() {
    let server = Server::start(vec![]);
    for bad in ["FR=1000000001", "FR=1e10", "FR=1000000000.5"] {
        let message = refused(&server, &["estimate", "--traffic", bad]);
        assert!(
            message.ends_with(
                "a `--traffic` value is `REGION=GB` with a number from 0 to 1000000000, such as \
                 `FR=100`"
            ),
            "{bad}: {message}"
        );
    }
    for flag in ["--requests", "--custom-hostnames"] {
        refused(&server, &["estimate", flag, "1000000000000001"]);
    }
    let doc = hipc(
        &server,
        &[
            "estimate",
            "--traffic",
            "FR=1000000000",
            "--traffic",
            "AU=0",
            "--requests",
            "1000000000000000",
            "--custom-hostnames",
            "1000000000000000",
            "--dry-run",
        ],
    )
    .ok();
    assert_eq!(
        draft(&doc)["body"]["gb"],
        json!({ "FR": 1_000_000_000, "AU": 0 })
    );
    assert!(server.requests().is_empty());
}

#[test]
fn rules_update_sends_the_file_bytes_unchanged() {
    let rules =
        b"{ \"rules\" : [ {\"match\":{\"glob\":\"*.jpg\"},\"actions\":{\"edge_ttl\":60}} ] }\n";
    let server = Server::start(vec![route_of(
        &cdn::RULES_UPDATE,
        [&ZONE],
        Reply::json(200, r#"{"rules":[]}"#),
    )]);
    let home = Home::new();
    let file = home.file("rules.json", rules);
    // `--remove-others` reads nothing first, so the PUT is the only request.
    let path = file.to_str().unwrap();
    let doc = Hipc::against(&home, &server)
        .run(&[
            "cdn",
            "rules",
            "update",
            ZONE,
            "--rules-file",
            path,
            "--yes",
            "--remove-others",
        ])
        .ok();
    assert_eq!(doc, json!({"rules": []}));
    assert_eq!(server.only_request().body, rules);
    assert_eq!(
        server.only_request().header("content-type"),
        Some("application/json")
    );
    Hipc::against(&home, &server)
        .stdin(b"[]")
        .run(&[
            "cdn",
            "rules",
            "update",
            ZONE,
            "--rules-file",
            "-",
            "--yes",
            "--remove-others",
        ])
        .ok();
    assert_eq!(server.requests()[1].body, b"[]");
    Hipc::against(&home, &server)
        .run(&[
            "cdn",
            "rules",
            "update",
            ZONE,
            "--rules-file",
            path,
            "--remove-others",
        ])
        .fails(2, "confirmation_required");
    assert_eq!(server.requests().len(), 2);
}

/// A key swagger does not list for a cache rule, its `match`, or its
/// `actions` is refused before anything is read or sent, under `--dry-run` too. Every key
/// it lists is accepted, and the bytes still go out unchanged.
#[test]
fn rules_update_refuses_a_key_swagger_does_not_list() {
    let server = Server::start(rules_routes("[]"));
    let rule_keys = "a rule has only `match` and `actions`";
    let match_keys = "a `match` has only `path_prefix`, `glob`, and `extensions`";
    let action_keys = "`actions` has only `edge_ttl`, `browser_ttl`, `query_string`, `bypass`, \
                       and `ignore_set_cookie`";
    for (rule, problem) in [
        (
            r#"{"match":{"glob":"*"},"action":{"bypass":true}}"#,
            format!("has the key `action`, which a cache rule does not have: {rule_keys}"),
        ),
        (
            r#"{"match":{"glob":"*"},"actions":{"bypass":true},"id":1}"#,
            format!("has the key `id`, which a cache rule does not have: {rule_keys}"),
        ),
        (
            r#"{"match":{"prefix":"/a/"},"actions":{"bypass":true}}"#,
            format!("has the key `prefix` in its `match`: {match_keys}"),
        ),
        (
            r#"{"match":{"glob":"*"},"actions":{"ttl":60}}"#,
            format!("has the key `ttl` in its `actions`: {action_keys}"),
        ),
    ] {
        let rules = format!(r#"{{"rules":[{{"match":{{"glob":"*.jpg"}}}},{rule}]}}"#);
        for flags in [
            &["--yes", "--remove-others"][..],
            &["--dry-run"],
            &["--yes"],
        ] {
            let error = rules_update(&server, &rules, flags).fails(2, "usage");
            assert_eq!(
                error["message"],
                format!("`rules[1]` from stdin {problem}"),
                "{rule}"
            );
            assert_eq!(
                error["hint"],
                "fix `rules[1]`; the zone keeps its rules until then"
            );
        }
        // A bare array is checked the same way.
        let bare = format!("[{rule}]");
        let error = rules_update(&server, &bare, &["--yes"]).fails(2, "usage");
        assert_eq!(error["message"], format!("`rules[0]` from stdin {problem}"));
    }
    assert!(server.requests().is_empty());
    let every_key = concat!(
        r#"[{"match":{"path_prefix":"/a/"},"actions":{"edge_ttl":"origin","browser_ttl":null,"#,
        r#""query_string":{"whitelist":["v"]},"bypass":false,"ignore_set_cookie":true}},"#,
        r#"{"match":{"glob":"*.css"},"actions":{"edge_ttl":60}},"#,
        r#"{"match":{"extensions":["js"]},"actions":{"bypass":true}}]"#
    );
    rules_update(&server, every_key, &["--yes", "--remove-others"]).ok();
    assert_eq!(server.only_request().body, every_key.as_bytes());
}

#[test]
fn rules_update_refuses_a_file_that_is_not_json_or_cannot_be_read() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let file = home.file("rules.json", b"rules: []");
    let error = Hipc::against(&home, &server)
        .run(&[
            "cdn",
            "rules",
            "update",
            ZONE,
            "--rules-file",
            file.to_str().unwrap(),
            "--yes",
        ])
        .fails(2, "usage");
    assert!(
        error["message"].as_str().unwrap().contains("is not JSON"),
        "{error}"
    );
    let missing = home.path().join("missing.json");
    Hipc::against(&home, &server)
        .run(&[
            "cdn",
            "rules",
            "update",
            ZONE,
            "--rules-file",
            missing.to_str().unwrap(),
        ])
        .fails(2, "usage");
    assert!(server.requests().is_empty());
}

#[test]
fn hostnames_create_verify_and_delete() {
    let host = r#"{"hostname":"cdn.example.com","status":"pending_dns","dns":{"records":[]}}"#;
    let server = Server::start(vec![
        route_of(&cdn::HOSTNAME_CREATE, [&ZONE], Reply::json(201, host)),
        route_of(
            &cdn::HOSTNAME_VERIFY,
            [&ZONE, &"cdn.example.com"],
            Reply::json(201, host),
        ),
        route_of(
            &cdn::HOSTNAME_DELETE,
            [&ZONE, &"cdn.example.com"],
            Reply::json(204, ""),
        ),
    ]);
    hipc(&server, &["hostnames", "create", ZONE, "cdn.example.com"])
        .fails(2, "confirmation_required");
    assert!(server.requests().is_empty());
    let created = hipc(
        &server,
        &["hostnames", "create", ZONE, "cdn.example.com", "--yes"],
    )
    .ok();
    assert_eq!(created["status"], "pending_dns");
    assert_eq!(
        server.only_request().json(),
        json!({"hostname": "cdn.example.com"})
    );
    // A verify checks the DNS records again: it needs no confirmation.
    let verified = hipc(&server, &["hostnames", "verify", ZONE, "cdn.example.com"]).ok();
    assert_eq!(verified["hostname"], "cdn.example.com");
    assert_eq!(server.requests()[1].body, b"");
    assert_eq!(
        hipc(
            &server,
            &["hostnames", "delete", ZONE, "cdn.example.com", "--yes"]
        )
        .ok(),
        json!({"zone_id": ZONE, "hostname": "cdn.example.com", "deleted": true})
    );
    let error = hipc(&server, &["hostnames", "delete", ZONE, "cdn.example.com"])
        .fails(2, "confirmation_required");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("`cdn.example.com`"),
        "{error}"
    );
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn a_zone_id_or_hostname_that_could_change_the_route_is_refused() {
    let server = Server::start(vec![]);
    let long_label = format!("{}.com", "a".repeat(64));
    let long_name = vec!["a".repeat(63); 4].join(".");
    for bad_zone in [
        "z/b12cd34ef56",
        "z?b12cd34ef56",
        "z..12cd34ef56",
        "zab12cd34ef5",
        "zab12cd34ef567",
        "yab12cd34ef56",
        "",
    ] {
        hipc(&server, &["zones", "get", bad_zone]).fails(2, "usage");
        hipc(&server, &["stats", bad_zone]).fails(2, "usage");
    }
    for bad_host in [
        "a/b.com",
        "a?b.com",
        "a..b",
        "..",
        ".a.com",
        "a.com.",
        "-a.com",
        "a-.com",
        "a_b.com",
        "",
        long_label.as_str(),
        long_name.as_str(),
    ] {
        hipc(&server, &["hostnames", "delete", ZONE, bad_host, "--yes"]).fails(2, "usage");
    }
    assert!(server.requests().is_empty());
    // Upper case and digits are DNS names too.
    let ok = vec!["a".repeat(63); 3].join(".") + ".abcdefghijklmnopqrstuvwxyz0123456789-a";
    assert!(ok.len() <= 253);
    hipc(
        &server,
        &[
            "hostnames",
            "verify",
            ZONE,
            "CDN-1.Example.com",
            "--dry-run",
        ],
    )
    .ok();
    hipc(&server, &["hostnames", "verify", ZONE, &ok, "--dry-run"]).ok();
    hipc(&server, &["zones", "get", "zAB12CD34EF56", "--dry-run"]).ok();
}

/// An estimate is a POST that changes nothing (`hipc schema`: `read_only`), so a 503 is a
/// read's failure: nothing changed, and a retry is safe. A hostname verify
/// checks the records again and stays a write.
#[test]
fn a_failed_estimate_is_a_read_that_changed_nothing() {
    let server = Server::start(vec![route_of(
        &cdn::ESTIMATE,
        [],
        Reply::json(503, r#"{"detail":"Busy"}"#),
    )]);
    let error = hipc(&server, &["estimate", "--requests", "5"]).fails(1, "unavailable");
    assert_eq!(error["side_effects"], "none");
    assert_eq!(error["retryable"], true);
}

#[test]
fn estimate_sends_only_the_keys_given_without_confirmation() {
    let server = Server::start(vec![route_of(
        &cdn::ESTIMATE,
        [],
        Reply::json(201, r#"{"total":"1000000000000000000"}"#),
    )]);
    let run = hipc(
        &server,
        &[
            "estimate",
            "--traffic",
            "FR=100",
            "--traffic",
            "AU=0.123456789012345678901",
            "--requests",
            "1000000",
            "--custom-hostnames",
            "2",
        ],
    );
    assert_eq!(
        run.stdout().trim_end(),
        r#"{"total":"1000000000000000000"}"#
    );
    // The GB are sent as written, digit for digit.
    assert_eq!(
        server.only_request().body,
        concat!(
            r#"{"gb":{"FR":100,"AU":0.123456789012345678901},"#,
            r#""requests":1000000,"custom_domains":2}"#
        )
        .as_bytes()
    );
    assert_eq!(
        hipc(&server, &["estimate", "--dry-run"]).ok()["requests"][0]["body"],
        json!({})
    );
    let doc = hipc(&server, &["estimate", "--requests", "5", "--dry-run"]).ok();
    assert_eq!(draft(&doc)["body"], json!({"requests": 5}));
    // A region is uppercased, so one written in two cases is one region given twice.
    for second in ["FR=2", "fr=2"] {
        let error = hipc(
            &server,
            &["estimate", "--traffic", "FR=1", "--traffic", second],
        )
        .fails(2, "usage");
        assert_eq!(
            error["message"],
            "`--traffic` names the region `FR` more than once"
        );
    }
    let doc = hipc(&server, &["estimate", "--traffic", "au=1", "--dry-run"]).ok();
    assert_eq!(draft(&doc)["body"], json!({"gb": {"AU": 1}}));
    for bad in ["FR", "FR=", "FR=-1", "=1", "F1=2", "FR=ten", "FRA=1", "F=1"] {
        hipc(&server, &["estimate", "--traffic", bad]).fails(2, "usage");
    }
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn stats_sends_only_the_query_parameters_given() {
    let server = Server::start(vec![route_of(&cdn::STATS, [&ZONE], Reply::json(200, "{}"))]);
    hipc(&server, &["stats", ZONE]).ok();
    hipc(
        &server,
        &[
            "stats",
            ZONE,
            "--from",
            "2026-10-01T00:00:00Z",
            "--to",
            "2026-10-02T00:00:00Z",
            "--step",
            "5m",
            "--region",
            "FR",
        ],
    )
    .ok();
    hipc(&server, &["stats", ZONE, "--step", "1d"]).ok();
    hipc(&server, &["stats", ZONE, "--step", "1h"]).ok();
    let targets: Vec<String> = server.requests().iter().map(|r| r.target.clone()).collect();
    let stats = format!("/api/cdn/zones/{ZONE}/stats/");
    assert_eq!(
        targets,
        [
            stats.clone(),
            format!(
                "{stats}?{}",
                concat!(
                    "from=2026-10-01T00%3A00%3A00Z&to=2026-10-02T00%3A00%3A00Z",
                    "&step=5m&region=FR"
                )
            ),
            format!("{stats}?step=1d"),
            format!("{stats}?step=1h"),
        ]
    );
    hipc(&server, &["stats", ZONE, "--step", "1w"]).fails(2, "usage");
    assert_eq!(server.requests().len(), 4);
}

#[test]
fn a_membership_sends_the_account_header_on_every_route() {
    let server = Server::start(vec![
        route_of(&cdn::PRICES, [], Reply::json(200, "{}")),
        route_of(&cdn::REGIONS, [], Reply::json(200, "{}")),
        route_of(&cdn::ESTIMATE, [], Reply::json(201, "{}")),
        route_of(
            &cdn::ZONES,
            [],
            Reply::json(200, r#"{"zones":[],"beta":true}"#),
        ),
        route_of(&cdn::ZONE_CREATE, [], Reply::json(201, ZONE_BODY)),
        route_of(&cdn::ZONE, [&ZONE], Reply::json(200, ZONE_BODY)),
        route_of(&cdn::ZONE_UPDATE, [&ZONE], Reply::json(200, ZONE_BODY)),
        route_of(&cdn::ZONE_DELETE, [&ZONE], Reply::json(204, "")),
        route_of(&cdn::CAP, [&ZONE], Reply::json(200, "{}")),
        route_of(&cdn::CAP_UPDATE, [&ZONE], Reply::json(200, "{}")),
        route_of(&cdn::HOSTNAMES, [&ZONE], Reply::json(200, "[]")),
        route_of(&cdn::HOSTNAME_CREATE, [&ZONE], Reply::json(201, "{}")),
        route_of(
            &cdn::HOSTNAME_VERIFY,
            [&ZONE, &"a.com"],
            Reply::json(201, "{}"),
        ),
        route_of(
            &cdn::HOSTNAME_DELETE,
            [&ZONE, &"a.com"],
            Reply::json(204, ""),
        ),
        route_of(&cdn::PURGE_CREATE, [&ZONE], Reply::json(201, "{}")),
        route_of(&cdn::PURGES, [&ZONE], Reply::json(200, "[]")),
        route_of(&cdn::PURGE, [&ZONE, &1], Reply::json(200, "{}")),
        route_of(&cdn::RULES, [&ZONE], Reply::json(200, r#"{"rules":[]}"#)),
        route_of(&cdn::RULES_UPDATE, [&ZONE], Reply::json(200, "{}")),
        route_of(&cdn::STATS, [&ZONE], Reply::json(200, "{}")),
    ]);
    let home = Home::new();
    let rules = home.file("rules.json", br#"{"rules":[]}"#);
    let commands: [&[&str]; 20] = [
        &["prices"],
        &["regions"],
        &["estimate"],
        &["zones", "list"],
        &[
            "zones", "create", "--name", "s", "--bucket", "b", "--public",
        ],
        &["zones", "get", ZONE],
        &["zones", "update", ZONE, "--name", "n"],
        &["zones", "delete", ZONE],
        &["cap", "get", ZONE],
        // It reads the cap in force first, so it sends two requests.
        &["cap", "update", ZONE, "--monthly-cap", "30"],
        &["hostnames", "list", ZONE],
        &["hostnames", "create", ZONE, "a.com"],
        &["hostnames", "verify", ZONE, "a.com"],
        &["hostnames", "delete", ZONE, "a.com"],
        &["purges", "create", ZONE, "--everything"],
        &["purges", "list", ZONE],
        &["purges", "get", ZONE, "1"],
        &["rules", "get", ZONE],
        // It reads the current rules first, so it sends two requests.
        &[
            "rules",
            "update",
            ZONE,
            "--rules-file",
            rules.to_str().unwrap(),
        ],
        &["stats", ZONE],
    ];
    for args in commands {
        let mut argv = vec!["cdn"];
        argv.extend(args);
        argv.push("--yes");
        Hipc::against(&home, &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&argv)
            .ok();
    }
    let requests = server.requests();
    assert_eq!(requests.len(), commands.len() + 2);
    for request in requests {
        assert_eq!(
            request.header("x-hippius-account"),
            Some(MEMBER),
            "{} {}",
            request.method,
            request.target
        );
    }
}

#[test]
fn a_server_refusal_passes_through_by_its_code() {
    let server = Server::start(vec![route_of(
        &cdn::ZONE_UPDATE,
        [&ZONE],
        Reply::json(
            403,
            r#"{"code":"insufficient_grant","error":"needs S3 admin"}"#,
        ),
    )]);
    let error = hipc(
        &server,
        &[
            "zones", "update", ZONE, "--bucket", "private", "--public", "--yes",
        ],
    )
    .fails(1, "insufficient_grant");
    assert_eq!(error["http_status"], 403);
    assert_eq!(error["side_effects"], "none");
}

#[test]
fn these_cdn_commands_dry_run_without_a_request() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let rules = home.file("rules.json", br#"{"rules":[]}"#);
    for args in [
        &["prices"][..],
        &["regions"],
        &["estimate", "--traffic", "FR=1"],
        &["zones", "list"],
        &["zones", "get", ZONE],
        &["zones", "create", "--name", "s", "--bucket", "b"],
        &["zones", "update", ZONE, "--name", "n"],
        &["zones", "delete", ZONE],
        &["cap", "get", ZONE],
        &["cap", "update", ZONE, "--monthly-cap", "30"],
        &["hostnames", "list", ZONE],
        &["hostnames", "create", ZONE, "a.com"],
        &["hostnames", "verify", ZONE, "a.com"],
        &["hostnames", "delete", ZONE, "a.com"],
        &["purges", "create", ZONE, "--everything"],
        &["purges", "list", ZONE],
        &["purges", "get", ZONE, "1"],
        &["rules", "get", ZONE],
        &[
            "rules",
            "update",
            ZONE,
            "--rules-file",
            rules.to_str().unwrap(),
        ],
        &["stats", ZONE, "--step", "1h"],
    ] {
        let mut argv = vec!["cdn"];
        argv.extend(args);
        argv.push("--dry-run");
        // No token: a dry run needs none (tenet 5).
        let doc = Hipc::tokenless(&home, &server).run(&argv).ok();
        assert_eq!(doc["dry_run"], true, "{args:?}");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn cdn_needs_a_token_except_for_prices() {
    let server = Server::start(vec![]);
    let no_token = |args: &[&str]| Hipc::tokenless(&Home::new(), &server).run(args);
    no_token(&["cdn", "zones", "list"]).fails(1, "auth");
    no_token(&["cdn", "zones", "delete", ZONE, "--yes"]).fails(1, "auth");
    assert!(server.requests().is_empty());
}
/// The zone's cache rules now: one by a glob, one by two extensions.
const CACHE_RULES: &str = concat!(
    r#"{"rules":[{"match":{"glob":"*.jpg"},"actions":{"edge_ttl":60}},"#,
    r#"{"match":{"extensions":["png",".gif"]},"actions":{"bypass":true}}]}"#
);

/// `cdn rules update` with `rules` on stdin, plus `extra` flags.
fn rules_update(server: &Server, rules: &str, extra: &[&str]) -> Run {
    let mut argv = vec!["cdn", "rules", "update", ZONE, "--rules-file", "-"];
    argv.extend(extra);
    Hipc::against(&Home::new(), server)
        .stdin(rules.as_bytes())
        .run(&argv)
}

/// The rules routes, with `current` as the zone's rules.
fn rules_routes(current: &str) -> Vec<Route> {
    vec![
        route_of(&cdn::RULES, [&ZONE], Reply::json(200, current)),
        route_of(&cdn::RULES_UPDATE, [&ZONE], Reply::json(200, current)),
    ]
}

/// The whole-list guard: a cache rule the zone has now that the file leaves out, or puts at
/// another place, is refused without `--remove-others`, after the read, and the PUT is
/// never sent.
#[test]
fn rules_update_that_drops_a_rule_is_refused_without_remove_others() {
    let one = r#"{"rules":[{"match":{"glob":"*.jpg"},"actions":{"edge_ttl":120}}]}"#;
    let second = r#"`{"position":1,"match":{"extensions":["gif","png"]}}`"#;
    // The rules as an object, and as the bare array swagger allows.
    let bare = concat!(
        r#"[{"match":{"glob":"*.jpg"},"actions":{"edge_ttl":60}},"#,
        r#"{"match":{"extensions":["png",".gif"]},"actions":{"bypass":true}}]"#
    );
    for current in [CACHE_RULES, bare] {
        let current = current.to_owned();
        let server = Server::start(rules_routes(&current));
        let error = rules_update(&server, one, &["--yes"]).fails(2, "remove_others_required");
        assert_eq!(
            error["message"],
            format!(
                "this would remove 1 cache rule that the CDN zone `{ZONE}` has now, and \
                 `--remove-others` was not passed"
            )
        );
        assert_eq!(
            error["hint"],
            format!(
                "run `hipc cdn rules update {ZONE} --rules-file - --yes --remove-others` to \
                 remove {second}"
            )
        );
        assert_eq!(
            server.targets(),
            [format!("GET /api/cdn/zones/{ZONE}/rules/")]
        );
        // Every rule gone names both, in the zone's order.
        let error = rules_update(&server, "[]", &["--yes"]).fails(2, "remove_others_required");
        let hint = error["hint"].as_str().expect("a hint");
        assert!(
            hint.ends_with(&format!(
                r#"`{{"position":0,"match":{{"glob":"*.jpg"}}}}` and {second}"#
            )),
            "{hint}"
        );
        let server = Server::start(rules_routes(&current));
        rules_update(&server, one, &["--yes", "--remove-others"]).ok();
        assert_eq!(
            server.targets(),
            [format!("PUT /api/cdn/zones/{ZONE}/rules/")]
        );
    }
}

/// The first matching rule wins, so a rule's place is part of it: a reorder, or a new rule
/// put ahead of one, changes which rule a path meets first, and is refused.
#[test]
fn rules_update_that_reorders_or_inserts_ahead_is_refused() {
    for moved in [
        concat!(
            r#"[{"match":{"extensions":["gif","png"]},"actions":{"bypass":true}},"#,
            r#"{"match":{"glob":"*.jpg"},"actions":{"edge_ttl":60}}]"#
        ),
        concat!(
            r#"[{"match":{"path_prefix":"/new/"},"actions":{"bypass":true}},"#,
            r#"{"match":{"glob":"*.jpg"},"actions":{"edge_ttl":60}},"#,
            r#"{"match":{"extensions":["gif","png"]},"actions":{"bypass":true}}]"#
        ),
    ] {
        let server = Server::start(rules_routes(CACHE_RULES));
        let error = rules_update(&server, moved, &["--yes"]).fails(2, "remove_others_required");
        assert!(
            error["message"]
                .as_str()
                .expect("a message")
                .starts_with("this would remove 2 cache rules that"),
            "{error}"
        );
        assert_eq!(server.requests().len(), 1, "{moved}");
    }
}

/// A rule kept at its place may change its `actions`; extensions compare without a leading
/// dot and in any order; a rule added at the end removes nothing.
#[test]
fn rules_update_that_keeps_every_rule_in_place_reads_then_puts() {
    let server = Server::start(rules_routes(CACHE_RULES));
    let kept = concat!(
        r#"[{"match":{"glob":"*.jpg"},"actions":{"bypass":true}},"#,
        r#"{"match":{"extensions":["gif",".png","png"]},"actions":{"edge_ttl":5}},"#,
        r#"{"match":{"path_prefix":"/new/"},"actions":{"bypass":true}}]"#
    );
    rules_update(&server, kept, &["--yes"]).ok();
    assert_eq!(
        server.targets(),
        [
            format!("GET /api/cdn/zones/{ZONE}/rules/"),
            format!("PUT /api/cdn/zones/{ZONE}/rules/"),
        ]
    );
    // An extension that is not text is compared as it is.
    let odd = r#"[{"match":{"extensions":[1]}},{"match":{"glob":"*"}}]"#;
    let server = Server::start(rules_routes(odd));
    rules_update(&server, odd, &["--yes"]).ok();
    let error = rules_update(&server, r#"[{"match":{"extensions":[1]}}]"#, &["--yes"])
        .fails(2, "remove_others_required");
    assert!(
        error["hint"]
            .as_str()
            .expect("a hint")
            .ends_with(r#"to remove `{"position":1,"match":{"glob":"*"}}`"#),
        "{error}"
    );
}

/// Of two rules that share a `match` only the first applies, so a rule is its place: a
/// second rule with the same `match` appended at the end removes nothing, while new
/// `actions` at the first rule's place stay an edit of it.
#[test]
fn rules_update_that_appends_a_rule_with_the_same_match_is_accepted() {
    let current = r#"[{"match":{"glob":"*"},"actions":{"edge_ttl":60}}]"#;
    let appended = concat!(
        r#"[{"match":{"glob":"*"},"actions":{"edge_ttl":5}},"#,
        r#"{"match":{"glob":"*"},"actions":{"bypass":true}}]"#
    );
    let server = Server::start(rules_routes(current));
    rules_update(&server, appended, &["--yes"]).ok();
    assert_eq!(
        server.targets(),
        [
            format!("GET /api/cdn/zones/{ZONE}/rules/"),
            format!("PUT /api/cdn/zones/{ZONE}/rules/"),
        ]
    );
}

#[test]
fn rules_update_dry_run_names_the_read_and_a_bad_shape_is_usage() {
    let server = Server::start(vec![]);
    let doc = rules_update(&server, "[]", &["--dry-run"]).dry_ok();
    let url = format!("{}/api/cdn/zones/{ZONE}/rules/", server.url());
    let request = |method: &str, body: Value| {
        json!({
            "method": method,
            "url": url,
            "account_header": null,
            "project_header": null,
            "billing_header": null,
            "body": body,
        })
    };
    // `requests` is the read, then the PUT, which the read leaves as it is.
    let read = request("GET", Value::Null);
    let put = request("PUT", json!([]));
    let mut expected = json!({ "dry_run": true });
    expected["reads_first"] = read.clone();
    expected["requests"] = json!([read, put]);
    assert_eq!(doc, expected);
    let doc = rules_update(&server, "[]", &["--dry-run", "--remove-others"]).dry_ok();
    assert!(doc.get("reads_first").is_none(), "{doc}");
    // With nothing read, `requests` is the PUT alone.
    assert_eq!(doc["requests"], json!([put]), "{doc}");
    // A rule without a `match` object is refused too: as a shared `null` key it would
    // match every other such rule.
    for shape in [
        r#"{"rule":[]}"#,
        r#"{"rules":{}}"#,
        r#""rules""#,
        "null",
        r#"[{"actions":{}}]"#,
        r#"[{"match":"*.jpg"}]"#,
    ] {
        let error = rules_update(&server, shape, &["--yes", "--remove-others"]).fails(2, "usage");
        assert_eq!(
            error["message"],
            "the cache rules from stdin are not a JSON object with a `rules` array, or an array, \
             of rules that each have a `match` object",
            "{shape}"
        );
        assert_eq!(
            error["hint"],
            "write the rules as `{\"rules\": [...]}`, the shape `hipc cdn rules get` prints"
        );
    }
    assert!(server.requests().is_empty());
}

#[test]
fn rules_update_whose_read_has_another_shape_is_internal_without_a_put() {
    for answer in ["{}", r#"{"rules":{}}"#, "7", r#"[{"actions":{}}]"#] {
        let server = Server::start(rules_routes(answer));
        let error = rules_update(&server, "[]", &["--yes"]).fails(1, "internal");
        assert_eq!(
            error["message"],
            format!(
                "cannot get the cache rules of the CDN zone `{ZONE}`: the API answered with a \
                 shape `hipc` does not read"
            )
        );
        assert_eq!(
            server.targets(),
            [format!("GET /api/cdn/zones/{ZONE}/rules/")]
        );
    }
}

/// The publishing guard: hipc cannot tell before the write whether the bucket is
/// private, so every create, and every update that moves the origin, needs `--public`. The
/// refusal sends nothing, comes before the token check, and a dry run does not need it.
#[test]
fn a_zone_that_may_publish_a_bucket_needs_public() {
    let server = Server::start(vec![
        route_of(&cdn::ZONE_CREATE, [], Reply::json(201, ZONE_BODY)),
        route_of(&cdn::ZONE, [&ZONE], Reply::json(200, ZONE_BODY)),
        route_of(&cdn::ZONE_UPDATE, [&ZONE], Reply::json(200, ZONE_BODY)),
    ]);
    let create = [
        "zones", "create", "--name", "site", "--bucket", "web", "--prefix", "site/", "--yes",
    ];
    let error = hipc(&server, &create).fails(2, "public_required");
    assert_eq!(
        error["message"],
        "the CDN zone would make `web/site/` readable by anyone if the bucket `web` is private, \
         and `--public` was not passed"
    );
    assert_eq!(
        error["hint"],
        "run `hipc cdn zones create --name site --bucket web --prefix site/ --yes --public`"
    );
    for moved in [&["--bucket", "other"][..], &["--prefix", "www/"]] {
        let mut update = vec!["zones", "update", ZONE, "--yes"];
        update.extend(moved);
        let error = hipc(&server, &update).fails(2, "public_required");
        assert_eq!(
            error["message"],
            format!(
                "the new origin of the CDN zone `{ZONE}` would be readable by anyone if its \
                 bucket is private, and `--public` was not passed"
            )
        );
    }
    let no_token = Hipc::tokenless(&Home::new(), &server).run(&[&["cdn"][..], &create].concat());
    no_token.fails(2, "public_required");
    assert!(server.requests().is_empty());

    let doc = hipc(&server, &[&create[..], &["--dry-run"]].concat()).ok();
    assert_eq!(draft(&doc)["method"], "POST");
    hipc(&server, &[&create[..], &["--public"]].concat()).ok();
    hipc(
        &server,
        &[
            "zones", "update", ZONE, "--prefix", "www/", "--public", "--yes",
        ],
    )
    .ok();
    // A change that keeps the origin publishes nothing new.
    hipc(&server, &["zones", "update", ZONE, "--name", "n", "--yes"]).ok();
    assert_eq!(
        server.targets(),
        [
            "POST /api/cdn/zones/".to_owned(),
            format!("GET /api/cdn/zones/{ZONE}/"),
            format!("PATCH /api/cdn/zones/{ZONE}/"),
            format!("PATCH /api/cdn/zones/{ZONE}/"),
        ]
    );
}
