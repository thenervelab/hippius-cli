//! `hipc support tickets`: list and get, create with `--yes`, reply without it, list a
//! ticket's messages, and close one with `--yes`. The message comes from `--body-file`, and a
//! blank one is refused before anything is sent. The ticket routes act as the signed-in user,
//! so no selector header is sent.

use serde_json::{Value, json};

use crate::common::{
    BILLING, Hipc, Home, MEMBER, PROJECT, Reply, Route, Server, TOKEN, draft, route,
};

/// One ticket, in the shape of swagger's `TicketDetail`.
const TICKET: &str = concat!(
    r#"{"id":7,"subject":"VM will not boot","status":"open","priority":"normal","#,
    r#""category":"","resource_type":"","resource_id":"","created_by":3,"#,
    r#""creator_info":"me@example.com","assigned_to":null,"#,
    r#""created_at":"2026-10-29T10:00:00Z","updated_at":"2026-10-29T10:00:00Z","#,
    r#""closed_at":null,"messages":[]}"#
);

/// One message, in the shape of swagger's `TicketMessage`.
const MESSAGE: &str = concat!(
    r#"{"id":3,"author":3,"author_is_staff":false,"author_is_requester":true,"#,
    r#""author_display":"me","message_type":"public","body":"still down","#,
    r#""created_at":"2026-10-29T11:00:00Z","attachments":[],"seq":2,"#,
    r#""author_kind":"customer","client_msg_id":""}"#
);

/// The first DRF page of the ticket list, with a link to the second.
const PAGE_ONE: &str = concat!(
    r#"{"count":2,"next":"{base}/api/support/tickets/?page=2&page_size=1","#,
    r#""previous":null,"results":[{"id":7,"subject":"VM will not boot"}]}"#
);

/// The last page.
const PAGE_TWO: &str =
    r#"{"count":2,"next":null,"previous":null,"results":[{"id":6,"subject":"Refund"}]}"#;

/// The message every write sends, with the line break the file ends in.
const TEXT: &str = "the VM `vm_1` stays in `starting`\n";

/// The limit on a message file, in bytes: 1 MiB, arbitrary.
const LIMIT: usize = 1024 * 1024;

fn ticket_route(reply: Reply) -> Route {
    route("POST", "/api/support/tickets/", reply)
}

fn message_route(reply: Reply) -> Route {
    route("POST", "/api/support/tickets/7/messages/", reply)
}

/// The path of a private file in `home` that holds `text`.
fn body_file(home: &Home, text: &str) -> String {
    let path = home.file("message.txt", text.as_bytes());
    path.to_str().expect("a temporary path is UTF-8").to_owned()
}

#[test]
fn list_prints_one_page_with_its_token_and_sends_the_status_only_when_given() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/support/tickets/?status=open&page_size=1",
            Reply::json(200, PAGE_ONE),
        ),
        route(
            "GET",
            "/api/support/tickets/?page_size=100",
            Reply::json(200, PAGE_TWO),
        ),
    ]);
    let home = Home::new();
    let doc = Hipc::against(&home, &server)
        .run(&[
            "support", "tickets", "list", "--status", "open", "--limit", "1",
        ])
        .ok();
    assert_eq!(
        doc,
        json!({
            "count": 2,
            "items": [{"id": 7, "subject": "VM will not boot"}],
            "next_page_token": "2",
        })
    );
    let doc = Hipc::against(&home, &server)
        .run(&["support", "tickets", "list"])
        .ok();
    assert_eq!(doc["items"][0]["id"], 6);
    assert_eq!(doc["next_page_token"], Value::Null);
}

#[test]
fn list_all_follows_every_page_and_a_bare_array_is_one_page() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/support/tickets/?page_size=1",
            Reply::json(200, PAGE_ONE),
        ),
        route(
            "GET",
            "/api/support/tickets/?page_size=1&page=2",
            Reply::json(200, PAGE_TWO),
        ),
    ]);
    let home = Home::new();
    let doc = Hipc::against(&home, &server)
        .run(&["support", "tickets", "list", "--limit", "1", "--all"])
        .ok();
    assert_eq!(
        doc,
        json!([{"id": 7, "subject": "VM will not boot"}, {"id": 6, "subject": "Refund"}])
    );

    // Swagger documents the answer as a bare array.
    let bare = Server::start(vec![route(
        "GET",
        "/api/support/tickets/",
        Reply::json(200, r#"[{"id":7}]"#),
    )]);
    let doc = Hipc::against(&home, &bare)
        .run(&["support", "tickets", "list"])
        .ok();
    assert_eq!(doc, json!({"items": [{"id": 7}], "next_page_token": null}));
}

/// Each value of `--status` and `--priority` is sent as swagger spells it.
#[test]
fn every_status_and_priority_is_sent_as_swagger_spells_it() {
    let home = Home::new();
    let file = body_file(&home, TEXT);
    for status in ["open", "pending", "resolved", "closed"] {
        let doc = Hipc::new(&home)
            .run(&[
                "--dry-run",
                "support",
                "tickets",
                "list",
                "--status",
                status,
            ])
            .ok();
        let url = draft(&doc)["url"].as_str().unwrap();
        assert!(
            url.ends_with(&format!(
                "/api/support/tickets/?status={status}&page_size=100"
            )),
            "{url}"
        );
    }
    for priority in ["low", "normal", "high", "urgent"] {
        let doc = Hipc::new(&home)
            .run(&[
                "--dry-run",
                "support",
                "tickets",
                "create",
                "--subject",
                "Refund",
                "--priority",
                priority,
                "--body-file",
                &file,
            ])
            .ok();
        assert_eq!(draft(&doc)["body"]["priority"], priority);
    }
}

#[test]
fn list_refuses_a_status_and_a_page_token_the_api_does_not_have() {
    let home = Home::new();
    for args in [
        ["support", "tickets", "list", "--status", "waiting"],
        ["support", "tickets", "list", "--page-token", "x"],
    ] {
        Hipc::new(&home).run(&args).fails(2, "usage");
    }
}

#[test]
fn get_prints_the_ticket_and_a_missing_one_names_the_list() {
    let server = Server::start(vec![
        route("GET", "/api/support/tickets/7/", Reply::json(200, TICKET)),
        route("GET", "/api/support/tickets/8/", Reply::not_found()),
    ]);
    let home = Home::new();
    let doc = Hipc::against(&home, &server)
        .run(&["support", "tickets", "get", "7"])
        .ok();
    assert_eq!(doc, serde_json::from_str::<Value>(TICKET).unwrap());
    let error = Hipc::against(&home, &server)
        .run(&["support", "tickets", "get", "8"])
        .fails(1, "not_found");
    assert_eq!(
        error["hint"],
        "run `hipc support tickets list` to see what exists"
    );
    let error = Hipc::new(&home)
        .run(&["support", "tickets", "get", "../7"])
        .fails(2, "usage");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("a ticket id is a whole number"),
        "{error}"
    );
}

#[test]
fn create_with_yes_sends_the_subject_the_message_as_written_and_the_priority_given() {
    let server = Server::start(vec![ticket_route(Reply::json(201, TICKET))]);
    let home = Home::new();
    let file = body_file(&home, TEXT);
    let doc = Hipc::against(&home, &server)
        .run(&[
            "support",
            "tickets",
            "create",
            "--subject",
            "VM will not boot",
            "--priority",
            "high",
            "--body-file",
            &file,
            "--yes",
        ])
        .ok();
    assert_eq!(doc["id"], 7);
    assert_eq!(
        server.only_request().json(),
        json!({"subject": "VM will not boot", "description": TEXT, "priority": "high"})
    );
}

#[test]
fn create_reads_the_message_from_stdin_and_sends_no_priority_unless_given() {
    let server = Server::start(vec![ticket_route(Reply::json(201, TICKET))]);
    Hipc::against(&Home::new(), &server)
        .stdin(TEXT.as_bytes())
        .run(&[
            "support",
            "tickets",
            "create",
            "--subject",
            "Refund",
            "--body-file",
            "-",
            "--yes",
        ])
        .ok();
    assert_eq!(
        server.only_request().json(),
        json!({"subject": "Refund", "description": TEXT})
    );
}

/// Without `--yes` on a pipe the create is refused, and nothing is sent. The token is
/// checked before the question, so a missing one is `auth` even then (tenet 5).
#[test]
fn create_without_yes_is_refused_and_sends_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let file = body_file(&home, TEXT);
    let args = [
        "support",
        "tickets",
        "create",
        "--subject",
        "Refund",
        "--body-file",
        &file,
    ];
    let error = Hipc::against(&home, &server)
        .run(&args)
        .fails(2, "confirmation_required");
    assert_eq!(error["hint"], "pass `--yes` to confirm");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("open the support ticket `Refund`"),
        "{error}"
    );
    Hipc::tokenless(&home, &server).run(&args).fails(1, "auth");
    assert!(server.requests().is_empty());
}

/// A dry run needs no `--yes` and no token, and shows the body that would be sent.
#[test]
fn create_and_reply_dry_runs_show_the_body_and_send_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let file = body_file(&home, TEXT);
    let hipc = || Hipc::tokenless(&home, &server);
    let doc = hipc()
        .run(&[
            "--dry-run",
            "support",
            "tickets",
            "create",
            "--subject",
            "Refund",
            "--body-file",
            &file,
        ])
        .ok();
    assert_eq!(draft(&doc)["method"], "POST");
    assert_eq!(
        draft(&doc)["url"],
        format!("{}/api/support/tickets/", server.url())
    );
    assert_eq!(
        draft(&doc)["body"],
        json!({"subject": "Refund", "description": TEXT})
    );
    // Every dry run lists its requests: here the one POST.
    let requests = doc["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["url"], draft(&doc)["url"]);
    assert_eq!(requests[0]["body"], draft(&doc)["body"]);
    let doc = hipc()
        .run(&[
            "--dry-run",
            "support",
            "tickets",
            "reply",
            "7",
            "--body-file",
            &file,
        ])
        .ok();
    assert_eq!(
        draft(&doc)["url"],
        format!("{}/api/support/tickets/7/messages/", server.url())
    );
    assert_eq!(draft(&doc)["body"], json!({ "body": TEXT }));
    assert_eq!(doc["requests"].as_array().unwrap().len(), 1);
    assert_eq!(doc["requests"][0]["body"], draft(&doc)["body"]);
    assert!(server.requests().is_empty());
}

#[test]
fn reply_needs_no_yes_and_sends_only_the_body() {
    let server = Server::start(vec![message_route(Reply::json(201, MESSAGE))]);
    let home = Home::new();
    let file = body_file(&home, TEXT);
    let doc = Hipc::against(&home, &server)
        .run(&["support", "tickets", "reply", "7", "--body-file", &file])
        .ok();
    assert_eq!(doc, serde_json::from_str::<Value>(MESSAGE).unwrap());
    assert_eq!(server.only_request().json(), json!({ "body": TEXT }));
}

/// A person reads a ticket, so a subject or a message that holds an API token is refused
/// before anything is sent, a dry run and `--yes` included: `HIPPIUS_TOKEN`, or the token of
/// any profile in the config file, as it is, quoted, or percent-encoded. The refusal never
/// quotes the text.
#[test]
fn a_ticket_text_that_holds_a_token_is_usage_and_sends_nothing() {
    let server = Server::start(vec![
        ticket_route(Reply::json(201, TICKET)),
        message_route(Reply::json(201, MESSAGE)),
    ]);
    let home = Home::new();
    let stored = "tok/stored+0123`456789=";
    // A hand-edited token with spaces around it goes out trimmed, so it is found trimmed.
    home.write_config(&format!(
        "[profiles.other]\ntoken = \"{stored}\"\ntoken_origin = \"https://api.hippius.com\"\n\
         [profiles.padded]\ntoken = \"  tok_padded_0123456789  \"\n"
    ));
    let texts = [
        format!("my token is {TOKEN}, please check"),
        format!("config: token = {stored}"),
        "tok%2Fstored%2B0123%60456789%3D".to_owned(),
        "tok%2fstored%2b0123%60456789%3d".to_owned(),
        r"tok/stored+0123\u{60}456789=".to_owned(),
        "see tok_padded_0123456789".to_owned(),
    ];
    for text in &texts {
        let file = body_file(&home, text);
        for args in [
            vec!["support", "tickets", "reply", "7", "--body-file", &file],
            vec![
                "support",
                "tickets",
                "reply",
                "7",
                "--body-file",
                &file,
                "--dry-run",
            ],
            vec![
                "support",
                "tickets",
                "create",
                "--subject",
                "Help",
                "--body-file",
                &file,
                "--yes",
            ],
        ] {
            let run = Hipc::against(&home, &server).run(&args);
            let error = run.fails(2, "usage");
            assert_eq!(
                error["message"],
                "the ticket text holds an API token, which a person on the Hippius support team \
                 would read"
            );
            assert!(!error.to_string().contains("456789"), "{error}");
            assert!(!error.to_string().contains("padded"), "{error}");
        }
    }
    // The subject is checked too.
    let file = body_file(&home, TEXT);
    let subject = format!("token {TOKEN}");
    Hipc::against(&home, &server)
        .run(&[
            "support",
            "tickets",
            "create",
            "--subject",
            &subject,
            "--body-file",
            &file,
            "--yes",
        ])
        .fails(2, "usage");
    assert!(server.requests().is_empty());
    // An empty stored token matches no text.
    home.write_config("[profiles.other]\ntoken = \"\"\n");
    Hipc::against(&home, &server)
        .run(&["support", "tickets", "reply", "7", "--body-file", &file])
        .ok();
}

/// A text whose token whitespace splits, as a wrapped line or a pasted log column would,
/// is refused as the token is, and so is one split between the subject and the message; so is
/// one that holds an access key, with spaces, line breaks, numbering, commas, or a soft hyphen
/// between or inside its words. Prose of listed words is sent: fewer than 12 in a row, or 12
/// whose checksum fails. Neither refusal quotes the text.
#[test]
fn a_ticket_text_with_a_split_token_or_an_access_key_is_refused() {
    let server = Server::start(vec![message_route(Reply::json(201, MESSAGE))]);
    let home = Home::new();
    let stored = "tok_stored_0123456789abcdef";
    home.write_config(&format!(
        "[profiles.other]\ntoken = \"{stored}\"\ntoken_origin = \"https://api.hippius.com\"\n"
    ));
    let reply = |text: &str| {
        let file = body_file(&home, text);
        Hipc::against(&home, &server).run(&[
            "support",
            "tickets",
            "reply",
            "7",
            "--body-file",
            &file,
        ])
    };
    for text in [
        "token: tok_stored_01234\n56789abcdef\n",
        "token: tok_stored_ 0123456789abcdef",
        "tok_stored_0123456789\r\n\tabcdef",
    ] {
        let error = reply(text).fails(2, "usage");
        assert_eq!(
            error["message"],
            "the ticket text holds an API token, which a person on the Hippius support team \
             would read",
            "{text:?}"
        );
        assert!(!error.to_string().contains("6789"), "{error}");
    }
    let words: Vec<&str> = crate::common::PHRASES[0].split_whitespace().collect();
    let mut numbered = String::new();
    for (i, word) in words.iter().enumerate() {
        numbered.push_str(&format!("{}. {word}\n", i + 1));
    }
    for text in [
        format!("my key is {}", crate::common::PHRASES[0]),
        numbered,
        words.join(", "),
        words.join("\n").to_uppercase(),
        words.join(" ").replacen("drive", "dri\u{ad}ve", 1),
    ] {
        let error = reply(&text).fails(2, "usage");
        assert_eq!(
            error["message"],
            "the ticket text holds what looks like an access key, which a person on the Hippius \
             support team would read",
            "{text:?}"
        );
        assert_eq!(
            error["hint"],
            "remove the 12 words from the subject and the message; the support team never needs it"
        );
        assert!(!error.to_string().contains("curtain"), "{error}");
    }
    // Half the token in the subject, half in the message.
    let file = body_file(&home, "56789abcdef");
    let error = Hipc::against(&home, &server)
        .run(&[
            "support",
            "tickets",
            "create",
            "--subject",
            "tok_stored_01234",
            "--body-file",
            &file,
            "--yes",
        ])
        .fails(2, "usage");
    assert!(!error.to_string().contains("6789"), "{error}");
    assert!(server.requests().is_empty());
    // Eleven listed words in a row, then one that is not listed, then more: no run of 12.
    let eleven = "abandon ability able about above absent absorb abstract absurd abuse access";
    reply(&format!("{eleven} zzz {eleven}")).ok();
    // An everyday message, many of whose words are on the list, is sent, and so are twelve
    // listed words in a row whose checksum fails.
    reply("please help can you check this token error when you have that file").ok();
    reply(
        "Hello, my bucket upload failed again today after the network change; please check \
         the access log, the account balance, and the invoice for this month. Thank you!",
    )
    .ok();
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn a_reply_to_a_missing_ticket_names_the_list() {
    let server = Server::start(vec![route(
        "POST",
        "/api/support/tickets/8/messages/",
        Reply::not_found(),
    )]);
    let home = Home::new();
    let file = body_file(&home, TEXT);
    let error = Hipc::against(&home, &server)
        .run(&["support", "tickets", "reply", "8", "--body-file", &file])
        .fails(1, "not_found");
    assert_eq!(
        error["hint"],
        "run `hipc support tickets list` to see what exists"
    );
    assert_eq!(error["side_effects"], "none");
}

/// A blank message is refused before the token check and the question, for both writes:
/// an empty file, one of spaces and line breaks, and an empty stdin.
#[test]
fn a_blank_message_is_usage_and_sends_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    for text in ["", " \n\t\n"] {
        let file = body_file(&home, text);
        for command in [
            vec!["support", "tickets", "create", "--subject", "Refund"],
            vec!["support", "tickets", "reply", "7"],
        ] {
            let args = [command, vec!["--body-file", &file, "--yes"]].concat();
            let error = Hipc::tokenless(&home, &server).run(&args).fails(2, "usage");
            assert_eq!(
                error["message"],
                format!("the message from `{file}` is blank")
            );
            assert_eq!(
                error["hint"],
                "write the message into the file `--body-file` names"
            );
        }
    }
    let error = Hipc::against(&home, &server)
        .run(&["support", "tickets", "reply", "7", "--body-file", "-"])
        .fails(2, "usage");
    assert_eq!(error["message"], "the message from stdin is blank");
    assert!(server.requests().is_empty());
}

#[test]
fn a_message_that_cannot_be_read_is_usage_and_sends_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let missing = home.path().join("missing.txt");
    let binary = home.file("binary.txt", &[0xff, 0xfe, b'\n']);
    let large = home.file("large.txt", &vec![b'a'; LIMIT + 1]);
    for (path, words) in [
        (missing, "cannot read the message from"),
        (binary, "is not UTF-8 text"),
        (large, "is larger than 1048576 bytes"),
    ] {
        let error = Hipc::against(&home, &server)
            .run(&[
                "support",
                "tickets",
                "reply",
                "7",
                "--body-file",
                path.to_str().unwrap(),
            ])
            .fails(2, "usage");
        assert!(
            error["message"].as_str().unwrap().contains(words),
            "{error}"
        );
    }
    assert!(server.requests().is_empty());

    // Exactly at the limit is read and sent.
    let full = Server::start(vec![message_route(Reply::json(201, MESSAGE))]);
    let at_limit = home.file("full.txt", &vec![b'a'; LIMIT]);
    Hipc::against(&home, &full)
        .run(&[
            "support",
            "tickets",
            "reply",
            "7",
            "--body-file",
            at_limit.to_str().unwrap(),
        ])
        .ok();
    assert_eq!(
        full.only_request().json()["body"].as_str().unwrap().len(),
        LIMIT
    );
}

/// Swagger's subject is 1 to 255 characters, counted as characters, not bytes.
#[test]
fn a_subject_outside_one_line_of_255_characters_is_usage() {
    let home = Home::new();
    let file = body_file(&home, TEXT);
    let long = "a".repeat(256);
    for subject in ["", "   ", long.as_str(), "two\nlines"] {
        let error = Hipc::new(&home)
            .run(&[
                "support",
                "tickets",
                "create",
                "--subject",
                subject,
                "--body-file",
                &file,
                "--yes",
            ])
            .fails(2, "usage");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("a subject is one line of 1 to 255 characters"),
            "{error}"
        );
    }
    let server = Server::start(vec![ticket_route(Reply::json(201, TICKET))]);
    let widest = "é".repeat(255);
    Hipc::against(&home, &server)
        .run(&[
            "support",
            "tickets",
            "create",
            "--subject",
            &widest,
            "--body-file",
            &file,
            "--yes",
        ])
        .ok();
    assert_eq!(server.only_request().json()["subject"], widest);
}

/// The staff update of a ticket (`PUT` of one ticket) is not among the routes that act as the
/// signed-in user, so `hipc api` refuses it with a membership and sends nothing, with or
/// without the trailing `/`. The customer's close (`PATCH`) is, as the console sends it, so
/// it goes out without the account header.
#[test]
fn a_membership_refuses_the_staff_ticket_update_through_hipc_api_but_sends_the_close() {
    let server = Server::start(vec![route(
        "PATCH",
        "/api/support/tickets/7/",
        Reply::json(200, CLOSED),
    )]);
    let home = Home::new();
    let input = home.file("update.json", br#"{"status":"closed"}"#);
    let input = input.to_str().unwrap();
    for path in ["/api/support/tickets/7/", "/support/tickets/7"] {
        let error = Hipc::against(&home, &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&["api", "PUT", path, "--body-file", input, "--yes"])
            .fails(2, "account_scope_unsupported");
        assert_eq!(
            error["hint"],
            "pass `--account self` to act in the token's own account"
        );
    }
    assert!(server.requests().is_empty());
    Hipc::against(&home, &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&[
            "api",
            "PATCH",
            "/support/tickets/7/",
            "--body-file",
            input,
            "--yes",
        ])
        .ok();
    let request = server.only_request();
    assert_eq!(request.header("x-hippius-account"), None);
    assert_eq!(request.json(), json!({"status": "closed"}));
}

/// The `PATCH` of one ticket takes the staff fields too (swagger's `TicketUpdate`), and the
/// server is the guard on which of them a customer may change, so `hipc api` sends the body
/// the caller wrote, or none, with or without a membership, and never the account header.
#[test]
fn hipc_api_sends_a_ticket_patch_with_the_body_given() {
    let server = Server::start(vec![route(
        "PATCH",
        "/api/support/tickets/7/",
        Reply::json(200, CLOSED),
    )]);
    let home = Home::new();
    let bodies = [
        r#"{"status":"open"}"#,
        r#"{"status":"closed","priority":"urgent"}"#,
        r#"{"priority":"urgent","assigned_to":3}"#,
    ];
    let mut sent = Vec::new();
    for (i, body) in bodies.iter().enumerate() {
        let input = home.file(&format!("body{i}.json"), body.as_bytes());
        for member in [true, false] {
            let hipc = Hipc::against(&home, &server);
            let hipc = if member {
                hipc.env("HIPPIUS_ACCOUNT", MEMBER)
            } else {
                hipc
            };
            hipc.run(&[
                "api",
                "PATCH",
                "/support/tickets/7/",
                "--body-file",
                input.to_str().unwrap(),
                "--yes",
            ])
            .ok();
            sent.push(serde_json::from_str::<Value>(body).unwrap());
        }
    }
    Hipc::against(&home, &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["api", "PATCH", "/support/tickets/7/", "--yes"])
        .ok();
    let requests = server.requests();
    assert_eq!(requests.len(), sent.len() + 1);
    for (request, body) in requests.iter().zip(&sent) {
        assert_eq!(request.header("x-hippius-account"), None);
        assert_eq!(&request.json(), body);
    }
    let last = requests.last().unwrap();
    assert_eq!(last.header("x-hippius-account"), None);
    assert_eq!(last.body, b"");
}

/// The ticket routes act as the signed-in user (the console's header rules), so with a
/// membership, a project, or a billing account selected every command is sent, and none
/// carries a selector header.
#[test]
fn a_selection_sends_every_ticket_command_without_a_selector_header() {
    let server = Server::start(vec![
        route("GET", "/api/support/tickets/", Reply::json(200, "[]")),
        route("GET", "/api/support/tickets/7/", Reply::json(200, TICKET)),
        ticket_route(Reply::json(201, TICKET)),
        message_route(Reply::json(201, MESSAGE)),
        route(
            "GET",
            "/api/support/tickets/7/messages/?page_size=100",
            Reply::json(200, "[]"),
        ),
        route("PATCH", "/api/support/tickets/7/", Reply::json(200, CLOSED)),
    ]);
    let home = Home::new();
    let file = body_file(&home, TEXT);
    let commands: [&[&str]; 6] = [
        &["support", "tickets", "list"],
        &["support", "tickets", "get", "7"],
        &[
            "support",
            "tickets",
            "create",
            "--subject",
            "Refund",
            "--body-file",
            &file,
            "--yes",
        ],
        &["support", "tickets", "reply", "7", "--body-file", &file],
        &["support", "tickets", "messages", "list", "7"],
        &[
            "support", "tickets", "update", "7", "--status", "closed", "--yes",
        ],
    ];
    let selections = [
        ("--account", MEMBER),
        ("--project", PROJECT),
        ("--billing", BILLING),
    ];
    for (flag, value) in selections {
        for args in commands {
            Hipc::against(&home, &server)
                .run(&[&[flag, value], args].concat())
                .ok();
        }
    }
    let requests = server.requests();
    // The close reads the ticket first, so it sends two requests.
    assert_eq!(requests.len(), (commands.len() + 1) * selections.len());
    for request in requests {
        for name in [
            "x-hippius-account",
            "x-hippius-project",
            "x-hippius-billing",
        ] {
            assert_eq!(request.header(name), None, "{name} of {}", request.target);
        }
    }
}

/// A DRF page of a ticket's messages, with a link to the second.
const MESSAGES_ONE: &str = concat!(
    r#"{"count":2,"next":"{base}/api/support/tickets/7/messages/?page=2&page_size=1","#,
    r#""previous":null,"results":[{"id":3,"body":"still down"}]}"#
);

/// The last page of a ticket's messages.
const MESSAGES_TWO: &str =
    r#"{"count":2,"next":null,"previous":null,"results":[{"id":4,"body":"fixed"}]}"#;

#[test]
fn messages_list_prints_one_page_with_its_token_and_all_follows_every_page() {
    let server = Server::start(vec![
        route(
            "GET",
            "/api/support/tickets/7/messages/?page_size=1",
            Reply::json(200, MESSAGES_ONE),
        ),
        route(
            "GET",
            "/api/support/tickets/7/messages/?page_size=1&page=2",
            Reply::json(200, MESSAGES_TWO),
        ),
    ]);
    let home = Home::new();
    let doc = Hipc::against(&home, &server)
        .run(&[
            "support", "tickets", "messages", "list", "7", "--limit", "1",
        ])
        .ok();
    assert_eq!(
        doc,
        json!({"count": 2, "items": [{"id": 3, "body": "still down"}], "next_page_token": "2"})
    );
    let doc = Hipc::against(&home, &server)
        .run(&[
            "support",
            "tickets",
            "messages",
            "list",
            "7",
            "--limit",
            "1",
            "--page-token",
            "2",
        ])
        .ok();
    assert_eq!(doc["items"], json!([{"id": 4, "body": "fixed"}]));
    assert_eq!(doc["next_page_token"], Value::Null);
    let doc = Hipc::against(&home, &server)
        .run(&[
            "support", "tickets", "messages", "list", "7", "--limit", "1", "--all",
        ])
        .ok();
    assert_eq!(
        doc,
        json!([{"id": 3, "body": "still down"}, {"id": 4, "body": "fixed"}])
    );
}

/// Swagger documents the answer as a bare array of `TicketMessage`, read as one page.
#[test]
fn messages_list_reads_a_bare_array_as_one_page() {
    let server = Server::start(vec![route(
        "GET",
        "/api/support/tickets/7/messages/?page_size=100",
        Reply::json(200, &format!("[{MESSAGE}]")),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["support", "tickets", "messages", "list", "7"])
        .ok();
    assert_eq!(
        doc,
        json!({"items": [serde_json::from_str::<Value>(MESSAGE).unwrap()], "next_page_token": null})
    );
}

#[test]
fn messages_list_of_a_missing_ticket_names_the_list_and_a_bad_id_is_usage() {
    let server = Server::start(vec![route(
        "GET",
        "/api/support/tickets/8/messages/?page_size=100",
        Reply::not_found(),
    )]);
    let home = Home::new();
    let error = Hipc::against(&home, &server)
        .run(&["support", "tickets", "messages", "list", "8"])
        .fails(1, "not_found");
    assert_eq!(
        error["hint"],
        "run `hipc support tickets list` to see what exists"
    );
    for id in ["../7", "-1", "x"] {
        let error = Hipc::new(&home)
            .run(&["support", "tickets", "messages", "list", "--", id])
            .fails(2, "usage");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("a ticket id is a whole number"),
            "{error}"
        );
    }
    for args in [
        &["support", "tickets", "messages", "list"][..],
        &[
            "support",
            "tickets",
            "messages",
            "list",
            "7",
            "--page-token",
            "x",
        ],
        &[
            "support", "tickets", "messages", "list", "7", "--limit", "0",
        ],
    ] {
        Hipc::new(&home).run(args).fails(2, "usage");
    }
}

/// A message is someone's own text: text mode escapes a control character in it rather
/// than let it reach the terminal.
#[test]
fn messages_list_in_text_escapes_the_control_characters_of_a_body() {
    let server = Server::start(vec![route(
        "GET",
        "/api/support/tickets/7/messages/?page_size=100",
        Reply::json(200, r#"[{"id":3,"body":"see \u001b]0;owned\u0007 here"}]"#),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&[
        "--output", "text", "support", "tickets", "messages", "list", "7",
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(!run.stdout().contains('\u{1b}'), "{}", run.stdout());
    assert!(!run.stdout().contains('\u{7}'), "{}", run.stdout());
    assert!(run.stdout().contains("see "), "{}", run.stdout());
}

/// The ticket as swagger's `TicketDetail` answers a close.
const CLOSED: &str = r#"{"id":7,"subject":"VM will not boot","status":"closed"}"#;

/// The method and target of every request `server` received, in order.
fn sent(server: &Server) -> Vec<(String, String)> {
    server
        .requests()
        .into_iter()
        .map(|request| (request.method, request.target))
        .collect()
}

/// The read, then the close, as a close of an open ticket sends them.
fn close_routes(ticket: &str) -> Vec<Route> {
    vec![
        route("GET", "/api/support/tickets/7/", Reply::json(200, ticket)),
        route("PATCH", "/api/support/tickets/7/", Reply::json(200, CLOSED)),
    ]
}

fn read_then_patch() -> Vec<(String, String)> {
    vec![
        ("GET".to_owned(), "/api/support/tickets/7/".to_owned()),
        ("PATCH".to_owned(), "/api/support/tickets/7/".to_owned()),
    ]
}

#[test]
fn update_with_yes_reads_the_ticket_then_closes_it_sending_only_the_status() {
    let server = Server::start(close_routes(TICKET));
    let doc = Hipc::against(&Home::new(), &server)
        .run(&[
            "support", "tickets", "update", "7", "--status", "closed", "--yes",
        ])
        .ok();
    assert_eq!(doc, serde_json::from_str::<Value>(CLOSED).unwrap());
    assert_eq!(sent(&server), read_then_patch());
    assert_eq!(server.requests()[1].json(), json!({"status": "closed"}));
}

/// Only `closed` stops a close: a ticket the support team marked `resolved`, or one
/// `pending` a reply, is still sent the `PATCH`, as the console closes either.
#[test]
fn a_resolved_or_pending_ticket_is_still_closed() {
    for status in ["resolved", "pending"] {
        let ticket = TICKET.replace(r#""status":"open""#, &format!(r#""status":"{status}""#));
        assert_ne!(ticket, TICKET);
        let server = Server::start(close_routes(&ticket));
        let doc = Hipc::against(&Home::new(), &server)
            .run(&[
                "support", "tickets", "update", "7", "--status", "closed", "--yes",
            ])
            .ok();
        assert_eq!(doc["status"], "closed", "{status}");
        assert_eq!(sent(&server), read_then_patch(), "{status}");
        assert_eq!(server.requests()[1].json(), json!({"status": "closed"}));
    }
}

/// A close ends the support team's work on the ticket, so it is confirmed. The question
/// names the ticket's subject, read first: without `--yes` that read is all that is sent. A
/// dry run sends nothing and lists the read, then the `PATCH`, without a token.
#[test]
fn update_without_yes_names_the_subject_and_a_dry_run_lists_the_read_then_the_patch() {
    let server = Server::start(close_routes(TICKET));
    let home = Home::new();
    let args = ["support", "tickets", "update", "7", "--status", "closed"];
    let error = Hipc::against(&home, &server)
        .run(&args)
        .fails(2, "confirmation_required");
    assert_eq!(error["hint"], "pass `--yes` to confirm");
    assert_eq!(
        error["message"],
        "this would close the ticket `7`, `VM will not boot`, and `--yes` was not passed"
    );
    assert_eq!(sent(&server), read_then_patch()[..1]);
    let dry = Server::start(vec![]);
    let doc = Hipc::tokenless(&home, &dry)
        .run(&[&["--dry-run"], &args[..]].concat())
        .ok();
    assert_eq!(draft(&doc)["method"], "PATCH");
    assert_eq!(
        draft(&doc)["url"],
        format!("{}/api/support/tickets/7/", dry.url())
    );
    assert_eq!(draft(&doc)["body"], json!({"status": "closed"}));
    let listed: Vec<(&str, &str)> = doc["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|request| {
            (
                request["method"].as_str().unwrap(),
                request["url"].as_str().unwrap(),
            )
        })
        .collect();
    let url = format!("{}/api/support/tickets/7/", dry.url());
    assert_eq!(listed, [("GET", url.as_str()), ("PATCH", url.as_str())]);
    assert!(dry.requests().is_empty());
}

/// A subject is quoted as every string from the server is, and a ticket without one is
/// still closed, named by its id alone.
#[test]
fn the_question_quotes_the_subject_and_names_the_id_alone_without_one() {
    let odd = TICKET.replace("VM will not boot", "a `tick`\\u001b");
    let none = TICKET.replace(r#""subject":"VM will not boot","#, "");
    let cases = [
        (
            odd,
            "this would close the ticket `7`, `a \\u{60}tick\\u{60}\\u{1b}`, and `--yes` was not \
             passed",
        ),
        (
            none,
            "this would close the ticket `7`, and `--yes` was not passed",
        ),
    ];
    for (ticket, message) in cases {
        let server = Server::start(close_routes(&ticket));
        let error = Hipc::against(&Home::new(), &server)
            .run(&["support", "tickets", "update", "7", "--status", "closed"])
            .fails(2, "confirmation_required");
        assert_eq!(error["message"], message);
        assert_eq!(sent(&server), read_then_patch()[..1]);
    }
}

/// A ticket the read shows closed already is a state no flag changes: `conflict`, exit 1,
/// and the `PATCH` is not sent, with `--yes` or without.
#[test]
fn a_close_of_a_closed_ticket_is_a_conflict_and_sends_no_patch() {
    for yes in [true, false] {
        let server = Server::start(close_routes(CLOSED));
        let mut args = vec!["support", "tickets", "update", "7", "--status", "closed"];
        if yes {
            args.push("--yes");
        }
        let error = Hipc::against(&Home::new(), &server)
            .run(&args)
            .fails(1, "conflict");
        assert_eq!(error["message"], "the ticket `7` is already closed");
        assert_eq!(error["side_effects"], "none");
        assert_eq!(sent(&server), read_then_patch()[..1]);
    }
}

/// `closed` is the one status a customer sets: any other, or none, is usage before the
/// token check, and so is a bad id.
#[test]
fn update_takes_only_the_closed_status_and_a_whole_number_id() {
    let home = Home::new();
    for status in ["open", "pending", "resolved", "Closed", ""] {
        Hipc::new(&home)
            .run(&[
                "support", "tickets", "update", "7", "--status", status, "--yes",
            ])
            .fails(2, "usage");
    }
    Hipc::new(&home)
        .run(&["support", "tickets", "update", "7", "--yes"])
        .fails(2, "usage");
    let error = Hipc::new(&home)
        .run(&[
            "support", "tickets", "update", "7/", "--status", "closed", "--yes",
        ])
        .fails(2, "usage");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("a ticket id is a whole number"),
        "{error}"
    );
}

#[test]
fn a_close_of_a_missing_ticket_names_the_list_and_changed_nothing() {
    let server = Server::start(vec![
        route("GET", "/api/support/tickets/8/", Reply::not_found()),
        route("PATCH", "/api/support/tickets/8/", Reply::json(200, CLOSED)),
    ]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&[
            "support", "tickets", "update", "8", "--status", "closed", "--yes",
        ])
        .fails(1, "not_found");
    assert_eq!(
        error["hint"],
        "run `hipc support tickets list` to see what exists"
    );
    assert_eq!(error["side_effects"], "none");
    // The read found nothing, so the `PATCH` is never sent.
    assert_eq!(server.requests().len(), 1);
    assert_eq!(server.requests()[0].method, "GET");
}
