//! `hipc cdn hostnames dns-setup` and `domain-connect`: the two ways to publish a custom
//! hostname's DNS records. The DNS setup sends a provider credential read from a private
//! file; these tests hold that it reaches no argv, output, `--debug` line, or error message.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::common::{Hipc, Home, MEMBER, Reply, Run, Server, TOKEN, draft, route};

const ZONE: &str = "zab12cd34ef56";
const HOST: &str = "cdn.example.com";
const PATH: &str = "/api/cdn/zones/zab12cd34ef56/hostnames/cdn.example.com/dns-setup/";

/// The provider secrets of the fixtures. No output may hold one.
const CF_TOKEN: &str = "cf-token-Qx81-never-printed";
const AWS_SECRET: &str = "aws-secret-Zr42-never-printed";
const AWS_SESSION: &str = "aws-session-Lp07-never-printed";

fn cloudflare() -> String {
    format!(r#"{{"api_token":"{CF_TOKEN}","zone_id":"cf-zone-1"}}"#)
}

fn route53() -> String {
    format!(
        concat!(
            r#"{{"access_key_id":"AKIAEXAMPLE","secret_access_key":"{}","#,
            r#""session_token":"{}","hosted_zone_id":"Z0EXAMPLE"}}"#
        ),
        AWS_SECRET, AWS_SESSION
    )
}

/// The access key id of the Route 53 fixture: half of the pair, so it is redacted too.
const AWS_KEY_ID: &str = "AKIAEXAMPLE";

/// Asserts that no secret field of the fixtures is in `text`. The ids are not secrets, and
/// are not redacted.
fn assert_clean(text: &str) {
    for secret in [CF_TOKEN, AWS_KEY_ID, AWS_SECRET, AWS_SESSION] {
        assert!(!text.contains(secret), "a credential leaked: {text}");
    }
}

fn assert_run_clean(run: &Run) {
    assert_clean(run.stdout());
    assert_clean(run.stderr());
}

fn setup_args<'a>(provider: &'a str, file: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "cdn",
        "hostnames",
        "dns-setup",
        ZONE,
        HOST,
        "--provider",
        provider,
        "--credentials-file",
        file,
    ];
    args.extend(extra);
    args
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The credential goes in the body as the file holds it, the answer's documented fields are
/// printed, and neither stdout nor stderr holds a secret, in JSON or text mode.
#[test]
fn the_credential_is_sent_in_the_body_and_printed_nowhere() {
    let answer = r#"{"provider":"cloudflare","mode":"all","replace":false}"#;
    let server = Server::start(vec![route("POST", PATH, Reply::json(201, answer))]);
    let home = Home::new();
    let file = path_text(&home.file("cf.json", cloudflare().as_bytes()));
    let run = Hipc::against(&home, &server).run(&setup_args("cloudflare", &file, &["--yes"]));
    assert_eq!(run.ok(), serde_json::from_str::<Value>(answer).unwrap());
    assert_run_clean(&run);
    let sent = server.only_request().json();
    assert_eq!(
        sent,
        json!({
            "provider": "cloudflare",
            "credentials": {"api_token": CF_TOKEN, "zone_id": "cf-zone-1"},
        })
    );

    let run = Hipc::against(&home, &server).run(
        &[
            &["--output", "text"][..],
            &setup_args("cloudflare", &file, &["--yes"]),
        ]
        .concat(),
    );
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(run.stdout().contains("cloudflare"), "{}", run.stdout());
    assert_run_clean(&run);
}

/// Route 53 takes the access key, its secret, and the optional session token and hosted
/// zone; `--debug` shows the request line and the status, never the body.
#[test]
fn route53_sends_its_fields_and_debug_shows_no_body() {
    let server = Server::start(vec![route("POST", PATH, Reply::json(201, "{}"))]);
    let home = Home::new();
    let file = path_text(&home.file("aws.json", route53().as_bytes()));
    let run =
        Hipc::against(&home, &server).run(&setup_args("route53", &file, &["--yes", "--debug"]));
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_run_clean(&run);
    assert_eq!(
        run.stderr(),
        format!("debug: POST {}{PATH}\ndebug: -> 201\n", server.url())
    );
    assert_eq!(
        server.only_request().json()["credentials"],
        json!({
            "access_key_id": "AKIAEXAMPLE",
            "secret_access_key": AWS_SECRET,
            "session_token": AWS_SESSION,
            "hosted_zone_id": "Z0EXAMPLE",
        })
    );
}

/// The answer keeps only what hipc can check: the provider it sent, a `mode` swagger lists,
/// and a boolean `replace`. Text the server chose (a `confirm`, another provider, an unknown
/// mode, a string `replace`, `credentials`, any other key) is dropped, and an answer that is
/// not an object prints as an empty one.
#[test]
fn the_answer_keeps_only_values_hip_can_check() {
    let home = Home::new();
    let file = path_text(&home.file("cf.json", cloudflare().as_bytes()));
    let run_with = |answer: &str| {
        let server = Server::start(vec![route("POST", PATH, Reply::json(201, answer))]);
        let run = Hipc::against(&home, &server).run(&setup_args("cloudflare", &file, &["--yes"]));
        assert_run_clean(&run);
        run.ok()
    };
    let echo = format!(
        concat!(
            r#"{{"provider":"cloudflare","credentials":{},"note":"used {}","#,
            r#""mode":"verification","confirm":true,"records":[1],"replace":true}}"#
        ),
        cloudflare(),
        CF_TOKEN
    );
    assert_eq!(
        run_with(&echo),
        json!({"provider": "cloudflare", "mode": "verification", "replace": true})
    );
    let odd = format!(
        r#"{{"provider":"route53","mode":"{CF_TOKEN}","replace":"{CF_TOKEN}","confirm":"x"}}"#
    );
    assert_eq!(run_with(&odd), json!({}));
    assert_eq!(run_with(r#"{"mode":{"all":true}}"#), json!({}));
    assert_eq!(run_with(&format!(r#""{CF_TOKEN}""#)), json!({}));
}

/// A failed setup says only its status: the message is hipc's own sentence, `details` is
/// dropped, and the code, the hint, and `side_effects` are as the status sets them, in JSON
/// and text mode.
#[test]
fn an_error_says_only_its_status() {
    let refusal = format!(r#"{{"detail":"Cloudflare refused the token {CF_TOKEN}"}}"#);
    let server = Server::start(vec![
        route(
            "POST",
            PATH,
            Reply::json(400, &refusal).header("X-Request-Id", &format!("req-{CF_TOKEN}")),
        ),
        route(
            "POST",
            "/api/cdn/zones/zab12cd34ef56/hostnames/www.example.com/dns-setup/",
            Reply::json(404, r#"{"detail":"Not found."}"#),
        ),
        route(
            "POST",
            "/api/cdn/zones/zab12cd34ef56/hostnames/api.example.com/dns-setup/",
            Reply::json(502, "{}"),
        ),
    ]);
    let home = Home::new();
    let file = path_text(&home.file("cf.json", cloudflare().as_bytes()));
    let run = Hipc::against(&home, &server).run(&setup_args("cloudflare", &file, &["--yes"]));
    let error = run.fails(1, "validation");
    assert_run_clean(&run);
    assert_eq!(
        error["message"],
        "cannot write the DNS records of the custom hostname `cdn.example.com` at \
         `cloudflare`: the API answered with status `400`; its words are not shown, since they \
         may repeat the credential"
    );
    assert_eq!(error["details"], serde_json::Value::Null, "{error}");
    assert_eq!(error["http_status"], 400);
    // The request id is a header the server chose, so it is kept only redacted.
    assert_eq!(error["request_id"], "req-<redacted>");
    assert_eq!(error["side_effects"], "none");

    let mut args = setup_args("cloudflare", &file, &["--yes"]);
    args[4] = "www.example.com";
    let error = Hipc::against(&home, &server)
        .run(&args)
        .fails(1, "not_found");
    assert_eq!(
        error["hint"],
        format!("run `hipc cdn hostnames list {ZONE}` to see what exists")
    );
    args[4] = "api.example.com";
    let error = Hipc::against(&home, &server)
        .run(&args)
        .fails(1, "unavailable");
    assert_eq!(error["side_effects"], "partial");
    assert_eq!(error["details"], serde_json::Value::Null, "{error}");

    let run = Hipc::against(&home, &server).run(
        &[
            &["--output", "text"][..],
            &setup_args("cloudflare", &file, &["--yes"]),
        ]
        .concat(),
    );
    assert_eq!(run.code(), Some(1));
    assert_run_clean(&run);
    assert!(run.stderr().contains("status `400`"), "{}", run.stderr());

    // An error with no status holds no server text, and keeps hipc's own message.
    let error = Hipc::new(&home)
        .env("HIPPIUS_TOKEN", TOKEN)
        .run(&setup_args("cloudflare", &file, &["--yes"]))
        .fails(1, "unavailable");
    assert_eq!(error["http_status"], Value::Null);
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("the connection to `127.0.0.1:9`"),
        "{error}"
    );
}

/// Without `--yes` it exits 2 `confirmation_required` and sends nothing; a dry run needs no
/// token and shows a placeholder where the credential goes.
#[test]
fn it_needs_yes_and_a_dry_run_shows_a_placeholder() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let file = path_text(&home.file("cf.json", cloudflare().as_bytes()));
    let run = Hipc::against(&home, &server).run(&setup_args("cloudflare", &file, &[]));
    let error = run.fails(2, "confirmation_required");
    assert_run_clean(&run);
    assert!(error["hint"].as_str().unwrap().contains("--yes"));

    let run = Hipc::new(&home)
        .env("HIPPIUS_API_URL", server.url())
        .run(&[&["--dry-run"][..], &setup_args("cloudflare", &file, &[])].concat());
    let doc = run.ok();
    assert_run_clean(&run);
    assert_eq!(draft(&doc)["method"], "POST");
    assert_eq!(draft(&doc)["url"], format!("{}{PATH}", server.url()));
    assert_eq!(
        draft(&doc)["body"],
        json!({
            "provider": "cloudflare",
            "credentials": {
                "api_token": "<the api_token from --credentials-file>",
                "zone_id": "<the zone_id from --credentials-file>",
            },
        })
    );
    assert!(server.requests().is_empty());
}

/// The credential has no flag of its own: the command takes a path, and `hipc schema` lists
/// no other way in.
#[test]
fn the_credential_has_no_argv_form() {
    let schema: Value =
        serde_json::from_str(Hipc::new(&Home::new()).run(&["schema"]).stdout()).unwrap();
    let command = schema["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["path"] == json!(["cdn", "hostnames", "dns-setup"]))
        .expect("dns-setup is in the schema");
    let names = |key: &str| -> Vec<String> {
        command[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(names("args"), ["zone_id", "hostname"]);
    assert_eq!(names("flags"), ["--provider", "--credentials-file"]);
    assert_eq!(command["flags"][1]["value_name"], "PATH");
    assert_eq!(command["confirmation"], true);
    assert_eq!(command["effect"], "idempotent");
}

/// Credential files `hipc` refuses, written in `home`, each with what its refusal says.
fn bad_files(home: &Home) -> Vec<(PathBuf, &'static str, &'static str)> {
    let bad = |name: &str, text: &[u8], mode: u32| -> PathBuf {
        let path = home.file(name, text);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
            .expect("chmod a fixture");
        path
    };
    let link = home.path().join("link.json");
    std::os::unix::fs::symlink(bad("target.json", cloudflare().as_bytes(), 0o600), &link)
        .expect("link a fixture");
    let dir = home.path().join("dir.json");
    std::fs::create_dir(&dir).expect("make a directory fixture");
    let mut large = cloudflare().into_bytes();
    large.resize((1 << 20) + 1, b' ');
    let other = format!(r#"{{"api_token":"{CF_TOKEN}","{AWS_SECRET}":"x"}}"#);
    vec![
        (
            home.path().join("missing.json"),
            "cloudflare",
            "does not exist",
        ),
        (link, "cloudflare", "is a symlink"),
        (dir, "cloudflare", "is not a regular file"),
        (
            bad("open.json", cloudflare().as_bytes(), 0o644),
            "cloudflare",
            "has permission bits for the group or others",
        ),
        (
            bad("group.json", cloudflare().as_bytes(), 0o640),
            "cloudflare",
            "has permission bits for the group or others",
        ),
        (
            bad("large.json", &large, 0o600),
            "cloudflare",
            "larger than",
        ),
        (
            bad(
                "ini.json",
                format!("[default]\naws_secret_access_key = {AWS_SECRET}\n").as_bytes(),
                0o600,
            ),
            "route53",
            "is not a JSON object",
        ),
        (
            bad("array.json", format!(r#"["{CF_TOKEN}"]"#).as_bytes(), 0o600),
            "cloudflare",
            "is not a JSON object",
        ),
        // The Cloudflare file given for Route 53: its fields are the wrong ones.
        (
            bad("cf.json", cloudflare().as_bytes(), 0o600),
            "route53",
            "has no `access_key_id`, which `--provider route53` needs",
        ),
        (
            bad("number.json", br#"{"api_token":42}"#, 0o600),
            "cloudflare",
            "sets `api_token` to something other than a non-empty string",
        ),
        (
            bad("empty.json", br#"{"api_token":""}"#, 0o600),
            "cloudflare",
            "sets `api_token` to something other than a non-empty string",
        ),
        (
            bad(
                "zone.json",
                format!(r#"{{"api_token":"{CF_TOKEN}","zone_id":7}}"#).as_bytes(),
                0o600,
            ),
            "cloudflare",
            "sets `zone_id` to something other than a non-empty string",
        ),
        // A field given twice: a parser would keep one of them silently.
        (
            bad(
                "twice.json",
                format!(r#"{{"api_token":"{CF_TOKEN}","api_token":"other"}}"#).as_bytes(),
                0o600,
            ),
            "cloudflare",
            "names a field twice",
        ),
        // A field it does not take is refused without naming it, since the name may be a
        // secret pasted in the wrong place.
        (
            bad("extra.json", other.as_bytes(), 0o600),
            "cloudflare",
            "has a field that `--provider cloudflare` does not take",
        ),
    ]
}

/// Each refused file is `usage` before anything is sent, a dry run included, and no message
/// or hint quotes what the file holds.
#[test]
fn a_bad_credentials_file_is_usage_with_zero_requests() {
    let server = Server::start(vec![]);
    let home = Home::new();
    for (path, provider, says) in bad_files(&home) {
        let file = path_text(&path);
        for extra in [&["--yes"][..], &["--dry-run"]] {
            let run = Hipc::against(&home, &server).run(&setup_args(provider, &file, extra));
            let error = run.fails(2, "usage");
            assert_run_clean(&run);
            let message = error["message"].as_str().unwrap();
            assert!(message.contains(says), "{file}: {error}");
            assert!(
                message.starts_with("the DNS credentials file `"),
                "the message names the file: {error}"
            );
        }
    }
    let open = path_text(&home.path().join("open.json"));
    let error = Hipc::against(&home, &server)
        .run(&setup_args("cloudflare", &open, &["--yes"]))
        .fails(2, "usage");
    assert!(
        error["hint"]
            .as_str()
            .unwrap()
            .starts_with("run `chmod 600 "),
        "{error}"
    );
    let missing = path_text(&home.path().join("missing.json"));
    let error = Hipc::against(&home, &server)
        .run(&setup_args("route53", &missing, &["--yes"]))
        .fails(2, "usage");
    assert_eq!(
        error["hint"],
        "pass `--credentials-file` a mode-0600 JSON file with `access_key_id` and \
         `secret_access_key`, and optionally `session_token` and `hosted_zone_id`"
    );
    assert!(server.requests().is_empty());
}

/// The file is checked before the config file is read and before the token: a refused file
/// is `usage` even with no token configured, and a good one then fails on the token.
#[test]
fn the_file_is_checked_before_the_token() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let open = home.file("open.json", cloudflare().as_bytes());
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o644)).unwrap();
    let good = path_text(&home.file("cf.json", cloudflare().as_bytes()));
    let hipc = || Hipc::new(&home).env("HIPPIUS_API_URL", server.url());
    hipc()
        .run(&setup_args("cloudflare", &path_text(&open), &["--yes"]))
        .fails(2, "usage");
    hipc()
        .run(&setup_args("cloudflare", &good, &["--yes"]))
        .fails(1, "auth");
    assert!(server.requests().is_empty());
}

const LINK: &str =
    r#"{"url":"https://dns.example.net/v2/domainTemplates/providers/hippius?sig=abc"}"#;

/// The link is printed as sent with `next_step`, the read that shows the hostname's status;
/// text mode notes it without repeating the URL.
#[test]
fn domain_connect_prints_the_link_and_the_next_step() {
    let path = "/api/cdn/zones/zab12cd34ef56/hostnames/cdn.example.com/domain-connect/";
    let server = Server::start(vec![route("GET", path, Reply::json(200, LINK))]);
    let home = Home::new();
    let args = ["cdn", "hostnames", "domain-connect", ZONE, HOST];
    let doc = Hipc::against(&home, &server).run(&args).ok();
    assert_eq!(
        doc["url"],
        serde_json::from_str::<Value>(LINK).unwrap()["url"]
    );
    // `HIPPIUS_API_URL` names the origin, so the next command repeats it.
    let next = format!("hipc --api-url {} cdn hostnames list {ZONE}", server.url());
    assert_eq!(doc["next_step"], next);
    assert_eq!(server.only_request().target, path);

    let run = Hipc::against(&home, &server).run(&[&["--output", "text"][..], &args].concat());
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        format!(
            "note: open the `url` in the output to publish the records; `{next}` shows the \
             hostname's status\n"
        )
    );
    assert!(run.stdout().contains("dns.example.net"), "{}", run.stdout());
}

/// `next_step` and the note carry the run's origin and selectors, so the command runs where
/// this one did; when one of them is not a plain word, both leave the command out.
#[test]
fn domain_connect_next_step_carries_the_selectors_of_the_run() {
    const PROJECT: &str = "proj_0a1b2c3d4e5f60718293";
    let path = "/api/cdn/zones/zab12cd34ef56/hostnames/cdn.example.com/domain-connect/";
    let server = Server::start(vec![route("GET", path, Reply::json(200, LINK))]);
    let home = Home::new();
    let args = [
        "--api-url",
        server.url(),
        "--account",
        MEMBER,
        "--project",
        PROJECT,
        "cdn",
        "hostnames",
        "domain-connect",
        ZONE,
        HOST,
    ];
    let next = format!(
        "hipc --api-url {} --account {MEMBER} --project {PROJECT} cdn hostnames list {ZONE}",
        server.url()
    );
    let hipc = || Hipc::new(&home).with_env_token(server.url(), TOKEN);
    assert_eq!(hipc().run(&args).ok()["next_step"], next);
    let run = hipc().run(&[&["--output", "text"][..], &args].concat());
    assert_eq!(
        run.stderr(),
        format!(
            "note: open the `url` in the output to publish the records; `{next}` shows the \
             hostname's status\n"
        )
    );
    let request = &server.requests()[0];
    assert_eq!(request.header("x-hippius-account"), Some(MEMBER));
    assert_eq!(request.header("x-hippius-project"), Some(PROJECT));

    // A profile name with a space is not one plain word: no command is named.
    home.write_config("[profiles.\"my work\"]\n");
    let run = hipc()
        .env("HIPPIUS_PROFILE", "my work")
        .run(&[&["--output", "text"][..], &args].concat());
    assert_eq!(
        run.stderr(),
        "note: open the `url` in the output to publish the records\n"
    );
    let doc = hipc().env("HIPPIUS_PROFILE", "my work").run(&args).ok();
    assert_eq!(doc.get("next_step"), None, "{doc}");
}

/// An answer without a string `url` is printed as sent, with no `next_step` and no note; a
/// 404, which the server gives while Domain Connect is off or the provider has no template,
/// is `not_found` with a hint that says so.
#[test]
fn domain_connect_without_a_url_or_a_template() {
    let path = |host: &str| format!("/api/cdn/zones/{ZONE}/hostnames/{host}/domain-connect/");
    let server = Server::start(vec![
        route(
            "GET",
            &path("a.example.com"),
            Reply::json(200, r#""https://dns.example.net/x""#),
        ),
        route(
            "GET",
            &path("b.example.com"),
            Reply::json(200, r#"{"available":false}"#),
        ),
        route(
            "GET",
            &path("c.example.com"),
            Reply::json(200, r#"{"url":7}"#),
        ),
        route(
            "GET",
            &path("d.example.com"),
            Reply::json(403, r#"{"detail":"No."}"#),
        ),
        route(
            "GET",
            &path("www.example.com"),
            Reply::json(404, r#"{"detail":"Not found."}"#),
        ),
    ]);
    let home = Home::new();
    for (host, answer) in [
        ("a.example.com", json!("https://dns.example.net/x")),
        ("b.example.com", json!({"available": false})),
        ("c.example.com", json!({"url": 7})),
    ] {
        let args = ["cdn", "hostnames", "domain-connect", ZONE, host];
        assert_eq!(Hipc::against(&home, &server).run(&args).ok(), answer);
        let run = Hipc::against(&home, &server).run(&[&["--output", "text"][..], &args].concat());
        assert_eq!(run.code(), Some(0), "{}", run.stderr());
        assert_eq!(run.stderr(), "", "{host}");
    }
    let error = Hipc::against(&home, &server)
        .run(&[
            "cdn",
            "hostnames",
            "domain-connect",
            ZONE,
            "www.example.com",
        ])
        .fails(1, "not_found");
    assert_eq!(
        error["hint"],
        format!(
            "check the hostname with `hipc cdn hostnames list {ZONE}`; if it is there, its DNS \
             provider has no Domain Connect template, or Domain Connect is off on this server"
        )
    );
    // Any other error keeps its own hint.
    let error = Hipc::against(&home, &server)
        .run(&["cdn", "hostnames", "domain-connect", ZONE, "d.example.com"])
        .fails(1, "permission");
    assert!(
        !error["hint"]
            .as_str()
            .unwrap_or("")
            .contains("Domain Connect"),
        "{error}"
    );
    Hipc::against(&home, &server)
        .run(&["cdn", "hostnames", "domain-connect", "bad", "a.example.com"])
        .fails(2, "usage");
    assert_eq!(server.requests().len(), 8);
}

/// A secret echoed in forms no spelling list foresees (base64, a Python repr, ASCII-only
/// JSON escapes, a truncated copy) reaches no output, from the answer or from an error.
#[test]
fn echoes_in_unforeseen_forms_reach_no_output() {
    // A `'`, a `"`, a `/`, a `+`, and a letter outside ASCII, so each form differs from the raw
    // secret.
    let secret = "s3'cr/t\"K+ey\u{fc}-never-printed";
    let file = format!(
        r#"{{"access_key_id":"AKIAEXAMPLE","secret_access_key":{}}}"#,
        json!(secret)
    );
    let forms = [
        "czMnY3IvdCJLK2V5w7wtbmV2ZXItcHJpbnRlZA==".to_owned(),
        r#"'s3\'cr/t"K+ey\u{fc}-never-printed'"#.replace("\\u{fc}", "\u{fc}"),
        r#"s3'cr/t\"K+ey\u00fc-never-printed"#.to_owned(),
        "s3'cr/t\"K+e".to_owned(),
    ];
    assert_eq!(forms[0], base64(secret.as_bytes()), "the base64 fixture");
    let echo = json!({
        "provider": forms[0],
        "mode": forms[1],
        "confirm": forms[2],
        "replace": forms[3],
        "note": forms.join(" "),
    });
    let server = Server::start(vec![
        route("POST", PATH, Reply::json(201, &echo.to_string())),
        route(
            "POST",
            "/api/cdn/zones/zab12cd34ef56/hostnames/www.example.com/dns-setup/",
            Reply::json(400, &echo.to_string()),
        ),
    ]);
    let home = Home::new();
    let path = path_text(&home.file("aws.json", file.as_bytes()));
    let mut args = setup_args("route53", &path, &["--yes"]);
    let mut runs = vec![Hipc::against(&home, &server).run(&args)];
    assert_eq!(runs[0].ok(), json!({}));
    args[4] = "www.example.com";
    for mode in ["json", "text"] {
        let run = Hipc::against(&home, &server).run(&[&["--output", mode][..], &args].concat());
        assert_eq!(run.code(), Some(1), "{}", run.stderr());
        runs.push(run);
    }
    for run in &runs {
        for form in forms
            .iter()
            .map(String::as_str)
            .chain([secret, "never-printed"])
        {
            assert!(!run.stdout().contains(form), "{form}: {}", run.stdout());
            assert!(!run.stderr().contains(form), "{form}: {}", run.stderr());
        }
    }
}

/// Standard base64 of `bytes`, for the echo fixture.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> (18 - 6 * i)) as usize & 63]));
            } else {
                out.push('=');
            }
        }
    }
    out
}
