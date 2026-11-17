//! `hipc support tickets messages attach`: one file, sent as `multipart/form-data` with
//! `--yes`, after local checks of its size, its type, and the secrets it may hold, each of
//! which sends nothing, the dry run included. The dry run shows the file's name, type, and
//! size, never its bytes.

use std::path::Path;

use hippius::routes::support;
use serde_json::{Value, json};

use crate::common::{
    Hipc, Home, MEMBER, Recorded, Reply, Route, SEED_PHRASES, Server, TOKEN, draft, route_of,
};

/// The answer, in the shape of swagger's `TicketAttachment`.
const ATTACHMENT: &str = concat!(
    r#"{"id":9,"filename":"shot.png","#,
    r#""file":"https://api.hippius.com/media/support/9/shot.png","#,
    r#""uploaded_at":"2026-11-17T10:00:00Z"}"#
);

/// A PNG file's signature and a few bytes, one of them not UTF-8.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\xffIHDR";

/// The largest file attached, in bytes: 5 MB.
const LIMIT: usize = 5_000_000;

fn attach_route(reply: Reply) -> Route {
    route_of(&support::ATTACHMENT_CREATE, [&42, &3], reply)
}

/// The arguments of an attach of `path` to the message 3 of the ticket 42, then `more`.
fn attach<'a>(path: &'a str, more: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "support", "tickets", "messages", "attach", "42", "3", "--file", path,
    ];
    args.extend_from_slice(more);
    args
}

fn shown(path: &Path) -> &str {
    path.to_str().expect("a temporary path is UTF-8")
}

/// The boundary the request's `Content-Type` names, after checking the type is a form.
fn boundary(request: &Recorded) -> String {
    let content_type = request.header("content-type").expect("a content type");
    content_type
        .strip_prefix("multipart/form-data; boundary=")
        .unwrap_or_else(|| panic!("not a form: {content_type}"))
        .to_owned()
}

/// The body a form of one file sends, framed by `boundary`.
fn form(boundary: &str, filename: &str, kind: &str, contents: &[u8]) -> Vec<u8> {
    [
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"{filename}\"\r\nContent-Type: {kind}\r\n\r\n"
        )
        .as_bytes(),
        contents,
        format!("\r\n--{boundary}--\r\n").as_bytes(),
    ]
    .concat()
}

#[test]
fn attach_sends_the_file_as_a_form_and_prints_the_answer() {
    let server = Server::start(vec![attach_route(Reply::json(201, ATTACHMENT))]);
    let home = Home::new();
    let file = home.file("shot.png", PNG);
    let doc = Hipc::against(&home, &server)
        .run(&attach(shown(&file), &["--yes"]))
        .ok();
    assert_eq!(doc, serde_json::from_str::<Value>(ATTACHMENT).unwrap());
    let request = server.only_request();
    let boundary = boundary(&request);
    assert!(boundary.starts_with("hipc-"), "{boundary}");
    assert_eq!(boundary.len(), "hipc-".len() + 36, "{boundary}");
    assert_eq!(request.body, form(&boundary, "shot.png", "image/png", PNG));
    assert_eq!(
        request.header("content-length"),
        Some(request.body.len().to_string().as_str())
    );
    // The ticket routes act as the signed-in user: no selector header, whatever is selected.
    let server = Server::start(vec![attach_route(Reply::json(201, ATTACHMENT))]);
    Hipc::against(&home, &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&attach(shown(&file), &["--yes"]))
        .ok();
    assert_eq!(server.only_request().header("x-hippius-account"), None);
}

/// Each type is sent as its first bytes say, whatever the file is called; text is
/// `text/plain`.
#[test]
fn each_type_is_sent_as_its_first_bytes_say() {
    let home = Home::new();
    for (name, contents, kind) in [
        ("a.bin", &b"\xff\xd8\xff\xe0JFIF"[..], "image/jpeg"),
        ("a.png", b"GIF89a\x01\x00", "image/gif"),
        ("a", b"RIFF\x04\x00\x00\x00WEBPVP8 ", "image/webp"),
        ("invoice.pdf", b"%PDF-1.7\n%\xe2\xe3", "application/pdf"),
        (
            "vm.log",
            "boot failed at 10:00 \u{2014} again\n".as_bytes(),
            "text/plain",
        ),
    ] {
        let server = Server::start(vec![attach_route(Reply::json(201, ATTACHMENT))]);
        let file = home.file(name, contents);
        Hipc::against(&home, &server)
            .run(&attach(shown(&file), &["--yes"]))
            .ok();
        let request = server.only_request();
        assert_eq!(
            request.body,
            form(&boundary(&request), name, kind, contents),
            "{name}"
        );
    }
}

/// A quote or a backslash in the name would end the quoted string of the part's header, so
/// each is sent as `_`.
#[test]
fn a_quote_or_backslash_in_the_name_is_sent_as_an_underscore() {
    let server = Server::start(vec![attach_route(Reply::json(201, ATTACHMENT))]);
    let home = Home::new();
    let file = home.file("say \"hi\\there\".txt", b"hello\n");
    Hipc::against(&home, &server)
        .run(&attach(shown(&file), &["--yes"]))
        .ok();
    let request = server.only_request();
    assert_eq!(
        request.body,
        form(
            &boundary(&request),
            "say _hi_there_.txt",
            "text/plain",
            b"hello\n"
        )
    );
}

#[test]
fn text_mode_prints_the_answer() {
    let server = Server::start(vec![attach_route(Reply::json(201, ATTACHMENT))]);
    let home = Home::new();
    let file = home.file("shot.png", PNG);
    let run = Hipc::against(&home, &server)
        .run(&[&["--output", "text"][..], &attach(shown(&file), &["--yes"])].concat());
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(run.stdout().contains("shot.png"), "{}", run.stdout());
    assert!(run.stdout().contains("uploaded_at"), "{}", run.stdout());
}

/// Without `--yes` on a pipe the attach is refused with a question that names the file, its
/// size and type, the message, and the ticket, and nothing is sent. The token is checked
/// before the question, so a missing one is `auth` even then (tenet 5).
#[test]
fn attach_without_yes_names_the_file_and_the_ticket_and_sends_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let file = home.file("shot.png", PNG);
    let args = attach(shown(&file), &[]);
    let error = Hipc::against(&home, &server)
        .run(&args)
        .fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        "this would attach `shot.png` (14 bytes, image/png) to the message `3` of the ticket \
         `42`, and `--yes` was not passed"
    );
    assert_eq!(error["hint"], "pass `--yes` to confirm");
    Hipc::tokenless(&home, &server).run(&args).fails(1, "auth");
    assert!(server.requests().is_empty());
}

/// A dry run needs no `--yes` and no token, and shows the file's name, type, and size in
/// place of its bytes.
#[test]
fn the_dry_run_shows_the_file_not_its_bytes_and_sends_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let file = home.file("vm.log", b"the VM s3cr3t-looking line\n");
    let run = Hipc::tokenless(&home, &server).run(&attach(shown(&file), &["--dry-run"]));
    assert!(!run.stdout().contains("s3cr3t"), "{}", run.stdout());
    let doc = run.ok();
    assert_eq!(doc["confirmation"], true);
    assert_eq!(doc["requests"].as_array().unwrap().len(), 1);
    assert_eq!(
        draft(&doc),
        &json!({
            "method": "POST",
            "url": format!("{}/api/support/tickets/42/messages/3/attachments/", server.url()),
            "account_header": null,
            "project_header": null,
            "billing_header": null,
            "content_type": "multipart/form-data",
            "body": {"file": {"filename": "vm.log", "content_type": "text/plain", "bytes": 27}},
        })
    );
    // Text mode shows the same, and no bytes of the file either.
    let run = Hipc::tokenless(&home, &server).run(
        &[
            &["--output", "text", "--dry-run"][..],
            &attach(shown(&file), &[]),
        ]
        .concat(),
    );
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(run.stdout().contains("vm.log"), "{}", run.stdout());
    assert!(!run.stdout().contains("s3cr3t"), "{}", run.stdout());
    assert!(server.requests().is_empty());
}

/// A file of exactly 5 MB is attached; one byte more is refused before it is sent.
#[test]
fn a_file_over_5_mb_is_refused_and_one_of_5_mb_is_not() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let file = home.file("big.log", &vec![b'a'; LIMIT]);
    let doc = Hipc::tokenless(&home, &server)
        .run(&attach(shown(&file), &["--dry-run"]))
        .ok();
    assert_eq!(draft(&doc)["body"]["file"]["bytes"], LIMIT);
    let file = home.file("bigger.log", &vec![b'a'; LIMIT + 1]);
    for more in [&["--yes"][..], &["--dry-run"]] {
        let error = Hipc::against(&home, &server)
            .run(&attach(shown(&file), more))
            .fails(2, "usage");
        assert_eq!(
            error["message"],
            format!(
                "`{}` is larger than 5000000 bytes, the most hipc reads for the attachment",
                shown(&file)
            )
        );
    }
    assert!(server.requests().is_empty());
}

/// Each local refusal is `usage`, sends nothing, and comes before the dry run and the token
/// check: an empty file, a type hipc does not attach, a missing file, a symlink, a
/// directory, and stdin.
#[test]
fn each_refused_file_is_usage_and_sends_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let empty = home.file("empty.txt", b"");
    let binary = home.file("a.zip", b"PK\x03\x04\x14\x00\x00\x00");
    let nul = home.file("nul.txt", b"text\0with a NUL");
    let target = home.file("real.png", PNG);
    let link = home.path().join("link.png");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let missing = home.path().join("missing.png");
    let not_a_type = |path: &Path| {
        format!(
            "`{}` is not a PNG, JPEG, GIF, WebP, or PDF file, nor UTF-8 text, the types hipc \
             attaches",
            shown(path)
        )
    };
    let cases = [
        (
            shown(&empty).to_owned(),
            format!("`{}` is empty", shown(&empty)),
            "pass `--file` the file that holds what to attach",
        ),
        (
            shown(&binary).to_owned(),
            not_a_type(&binary),
            "save it as one of those, such as a screenshot as PNG or a log as plain text",
        ),
        (
            shown(&nul).to_owned(),
            not_a_type(&nul),
            "save it as one of those, such as a screenshot as PNG or a log as plain text",
        ),
        (
            shown(&link).to_owned(),
            format!(
                "`{}` is a symlink, and a symlink is never followed",
                shown(&link)
            ),
            "pass a regular file to `--file`, or `-` for stdin",
        ),
        (
            shown(home.path()).to_owned(),
            format!("`{}` is not a regular file", shown(home.path())),
            "pass a regular file to `--file`, or `-` for stdin",
        ),
        (
            shown(&missing).to_owned(),
            format!(
                "cannot read the attachment from `{}`: it does not exist",
                shown(&missing)
            ),
            "pass a file that can be read to `--file`",
        ),
    ];
    for (path, message, hint) in &cases {
        for more in [&["--yes"][..], &["--dry-run"]] {
            let error = Hipc::tokenless(&home, &server)
                .run(&attach(path, more))
                .fails(2, "usage");
            assert_eq!(error["message"], *message, "{path}");
            assert_eq!(error["hint"], *hint, "{path}");
        }
    }
    let error = Hipc::new(&home)
        .run(&attach("-", &["--yes"]))
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        "invalid value `-` for `--file <PATH>`: an attachment is read from a file, not stdin; \
         pass its path"
    );
    assert!(server.requests().is_empty());
}

/// A person reads the attachment, so a file that holds the API token or an access key is
/// refused as a ticket's text is, a dry run and `--yes` included, and the refusal never
/// quotes it: in a text file, and in a picture's or a PDF's bytes read as text.
#[test]
fn a_file_that_holds_a_token_or_an_access_key_is_refused() {
    let server = Server::start(vec![attach_route(Reply::json(201, ATTACHMENT))]);
    let home = Home::new();
    let stored = "tok_stored_0123456789abcdef";
    home.write_config(&format!(
        "[profiles.other]\ntoken = \"{stored}\"\ntoken_origin = \"https://api.hippius.com\"\n"
    ));
    let png_with = |text: &str| [PNG, b"tEXtComment\0", text.as_bytes(), b"\xff"].concat();
    let token_files = [
        (
            "token.log",
            format!("Authorization: Token {TOKEN}\n").into_bytes(),
        ),
        ("config.txt", format!("token = \"{stored}\"\n").into_bytes()),
        ("wrapped.txt", b"tok_stored_01234\n56789abcdef\n".to_vec()),
        ("meta.png", png_with(TOKEN)),
        (
            "doc.pdf",
            [b"%PDF-1.4\n(", stored.as_bytes(), b") Tj\n\xe2"].concat(),
        ),
    ];
    for (name, contents) in &token_files {
        let file = home.file(name, contents);
        for more in [&["--yes"][..], &["--dry-run"]] {
            let error = Hipc::against(&home, &server)
                .run(&attach(shown(&file), more))
                .fails(2, "usage");
            assert_eq!(
                error["message"],
                format!(
                    "`{}` holds an API token, which a person on the Hippius support team would \
                     read",
                    shown(&file)
                )
            );
            assert_eq!(
                error["hint"],
                "remove the token from the file; the support team never needs it"
            );
            assert!(!error.to_string().contains("6789"), "{error}");
        }
    }
    for (name, contents) in [
        (
            "key.txt",
            format!("my key: {}\n", SEED_PHRASES[0]).into_bytes(),
        ),
        ("key.png", png_with(SEED_PHRASES[0])),
    ] {
        let file = home.file(name, &contents);
        let error = Hipc::against(&home, &server)
            .run(&attach(shown(&file), &["--yes"]))
            .fails(2, "usage");
        assert_eq!(
            error["message"],
            format!(
                "`{}` holds what looks like an access key, which a person on the Hippius \
                 support team would read",
                shown(&file)
            )
        );
        assert_eq!(
            error["hint"],
            "remove the 12 words from the file; the support team never needs it"
        );
    }
    assert!(server.requests().is_empty());
    // A file without one is sent.
    let file = home.file("clean.png", &png_with("made with a screenshot tool"));
    Hipc::against(&home, &server)
        .run(&attach(shown(&file), &["--yes"]))
        .ok();
    assert_eq!(server.requests().len(), 1);
}

/// The server's refusal of the file, or a missing ticket or message, is its error; a 502
/// after the upload was sent may have attached it, so it is `partial`.
#[test]
fn the_servers_answers_keep_their_codes() {
    let home = Home::new();
    let file = home.file("shot.png", PNG);
    for (reply, code, side_effects, hint) in [
        (
            Reply::json(400, r#"{"file":["Unsupported file type."]}"#),
            "validation",
            "none",
            Value::Null,
        ),
        (
            Reply::json(404, r#"{"detail":"Not found."}"#),
            "not_found",
            "none",
            json!("run `hipc support tickets messages list 42` to see what exists"),
        ),
        (
            Reply::json(429, r#"{"detail":"slow down"}"#).header("Retry-After", "7"),
            "rate_limited",
            "none",
            Value::Null,
        ),
        (
            Reply::json(502, r#"{"error":"x"}"#),
            "unavailable",
            "partial",
            Value::Null,
        ),
    ] {
        let server = Server::start(vec![attach_route(reply)]);
        let error = Hipc::against(&home, &server)
            .run(&attach(shown(&file), &["--yes"]))
            .fails(1, code);
        assert_eq!(error["side_effects"], side_effects, "{code}");
        if !hint.is_null() {
            assert_eq!(error["hint"], hint, "{code}");
        }
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .starts_with("cannot attach `shot.png`"),
            "{error}"
        );
    }
}
