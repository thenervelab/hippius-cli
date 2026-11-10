//! `hipc compute app-engine apps create`: the launch of an app, with its secrets read from a
//! private file. The order is argv, the compose file, the secrets file, the dry run, the token,
//! `POST validate/` with the compose alone, the local checks of its answer, the question, and
//! the launch. No secret value may reach argv, stdout, stderr, `--debug`, or an error, on any
//! path.

use std::os::unix::fs::PermissionsExt as _;

use serde_json::{Value, json};

use crate::common::{
    BILLING, Hipc, Home, MEMBER, PROJECT, Reply, Run, Server, draft, route, route_each,
};

const VALIDATE: &str = "/api/compute/app-engine/validate/";
const APPS: &str = "/api/compute/app-engine/apps/";
const APP: &str = "/api/compute/app-engine/apps/vm-1a2b/";

const COMPOSE: &str = "services:\n  web:\n    image: nginx\n";
/// `sha256sum` of [`COMPOSE`].
const COMPOSE_SHA256: &str = "5f6fad3dce5125e4e5595e34672d6d2bbe6149ab65095b42d9f77d66e3f8202c";

/// The secret values of the fixtures. No output may hold one, in any spelling.
const SENTINEL: &str = "app-secret-Vq93/never printed";
const OTHER_SECRET: &str = "second \"quoted\" value";

/// The spellings of [`SENTINEL`] a server might echo, each of which must be redacted.
const SENTINEL_SPELLINGS: [&str; 4] = [
    SENTINEL,
    "app-secret-Vq93%2Fnever%20printed",
    "app-secret-Vq93/never%20printed",
    "app-secret-Vq93%2fnever%20printed",
];

fn secrets_json() -> String {
    json!({ "DB_PASSWORD": SENTINEL, "API_KEY": OTHER_SECRET }).to_string()
}

/// Asserts that no secret of the fixtures is in `text`, in any spelling.
fn assert_clean(text: &str) {
    for secret in SENTINEL_SPELLINGS
        .iter()
        .chain(&[OTHER_SECRET, "second \\\"quoted"])
    {
        assert!(!text.contains(secret), "a secret value leaked: {text}");
    }
}

fn assert_run_clean(run: &Run) {
    assert_clean(run.stdout());
    assert_clean(run.stderr());
}

/// The validate answer, declaring `names`.
fn validated(names: &[&str]) -> String {
    json!({
        "grammar_version": 1,
        "compose_sha256": COMPOSE_SHA256,
        "services": [{"name": "web", "image": "nginx"}],
        "secret_names": names,
        "issues": [],
    })
    .to_string()
}

/// The 202 of a launch.
fn launched(status: &str) -> String {
    json!({
        "vm_id": "vm-1a2b",
        "name": "web",
        "status": status,
        "boot_phase": "pending",
        "compose_sha256": COMPOSE_SHA256,
        "secret_names": ["DB_PASSWORD", "API_KEY"],
    })
    .to_string()
}

/// A home with the compose file and a private secrets file, and their paths.
struct Files {
    home: Home,
    compose: String,
    secrets: String,
}

fn files() -> Files {
    let home = Home::new();
    let compose = path_text(&home.file("compose.yaml", COMPOSE.as_bytes()));
    let secrets = path_text(&home.file("secrets.json", secrets_json().as_bytes()));
    Files {
        home,
        compose,
        secrets,
    }
}

fn path_text(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `compute app-engine apps create --flavor small --compose-file <compose>`, then `extra`.
fn create_args<'a>(compose: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "compute",
        "app-engine",
        "apps",
        "create",
        "--flavor",
        "small",
        "--compose-file",
        compose,
    ];
    args.extend(extra);
    args
}

fn with_secrets<'a>(files: &'a Files, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = create_args(&files.compose, &["--secrets-file", &files.secrets]);
    args.extend(extra);
    args
}

/// A server that validates with both names declared and launches.
fn launching_server() -> Server {
    Server::start(vec![
        route(
            "POST",
            VALIDATE,
            Reply::json(200, &validated(&["DB_PASSWORD", "API_KEY"])),
        ),
        route("POST", APPS, Reply::json(202, &launched("provisioning"))),
    ])
}

/// The launch validates the compose alone, then sends the compose byte for byte with the
/// secrets as the file holds them, and prints the 202. No `public_ip` is sent.
#[test]
fn create_validates_the_compose_alone_then_launches_with_the_secrets() {
    let files = files();
    let server = launching_server();
    let run = Hipc::against(&files.home, &server).run(&with_secrets(&files, &["--yes"]));
    assert_run_clean(&run);
    let doc = run.ok();
    assert_eq!(doc["vm_id"], "vm-1a2b");
    let requests = server.requests();
    assert_eq!(requests.len(), 2, "a pipe gets no wait: {requests:#?}");
    assert_eq!(requests[0].path(), VALIDATE);
    assert_eq!(
        requests[0].json(),
        json!({ "compose": COMPOSE }),
        "validate never sends a secret"
    );
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].path(), APPS);
    assert_eq!(
        requests[1].json(),
        json!({
            "flavor": "small",
            "compose": COMPOSE,
            "secrets": { "DB_PASSWORD": SENTINEL, "API_KEY": OTHER_SECRET },
        })
    );
}

/// Each flag shared with `vms create` goes in the body by the launch route's names; without
/// `--secrets-file`, or with a file of `{}`, no `secrets` is sent at all.
#[test]
fn the_flags_go_in_the_body_and_an_empty_or_absent_file_sends_no_secrets() {
    let files = files();
    let empty = path_text(&files.home.file("empty.json", b"{}"));
    let server = Server::start(vec![
        route("POST", VALIDATE, Reply::json(200, &validated(&[]))),
        route("POST", APPS, Reply::json(202, &launched("provisioning"))),
    ]);
    let flags = [
        "--name",
        "web",
        "--region",
        "fr",
        "--os-disk-tier",
        "hdd",
        "--placement-group",
        "web-tier",
        "--on-guest-poweroff",
        "stop",
        "--backup-interval",
        "3600",
        "--failover",
        "auto",
        "--secrets-file",
        &empty,
        "--yes",
    ];
    Hipc::against(&files.home, &server)
        .run(&create_args(&files.compose, &flags))
        .ok();
    assert_eq!(
        server.requests()[1].json(),
        json!({
            "flavor": "small",
            "compose": COMPOSE,
            "name": "web",
            "region": "FR",
            "os_disk_tier": "hdd",
            "placement_group": "web-tier",
            "on_guest_poweroff": "stop",
            "failover_mode": "auto",
            "backup_interval_s": 3600,
        })
    );
    Hipc::against(&files.home, &server)
        .run(&create_args(&files.compose, &["--yes"]))
        .ok();
    assert_eq!(
        server.requests()[3].json(),
        json!({ "flavor": "small", "compose": COMPOSE })
    );
}

/// The grammar refuses what an app cannot take before anything is read: a failover without
/// backups, stdin as the secrets file, the VM launch's image, key, disk, and address flags,
/// and conflicting wait flags.
#[test]
fn the_grammar_refuses_what_an_app_does_not_take() {
    let files = files();
    let server = Server::start(vec![]);
    for extra in [
        &["--failover", "auto"][..],
        &["--secrets-file", "-"],
        &["--image", "ubuntu"],
        &["--ssh-key-id", "1"],
        &["--dedicated-disk", "2000:hdd"],
        &["--public-ip"],
        &["--wait", "--no-wait"],
        &["--no-wait", "--wait-timeout", "5"],
        &["--wait-timeout", "0"],
        &["--backup-interval", "60"],
        &["--region", "FRA"],
        &["--placement-group", "Web"],
    ] {
        let mut args = create_args(&files.compose, extra);
        args.push("--yes");
        let run = Hipc::against(&files.home, &server).run(&args);
        assert_eq!(run.code(), Some(2), "{extra:?}: {}", run.stderr());
    }
    let stdin = Hipc::against(&files.home, &server).run(&create_args(
        &files.compose,
        &["--secrets-file", "-", "--yes"],
    ));
    assert!(
        stdin.stderr().contains("never from stdin"),
        "{}",
        stdin.stderr()
    );
    assert!(server.requests().is_empty());
}

/// The bad secrets files, each with the words of its refusal.
fn bad_files(home: &Home) -> Vec<(String, &'static str)> {
    let file = |name: &str, text: &str| path_text(&home.file(name, text.as_bytes()));
    let chmod = |path: &std::path::Path, mode| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
    };
    let open = file("open.json", &secrets_json());
    chmod(open.as_ref(), 0o644);
    let group = file("group.json", &secrets_json());
    chmod(group.as_ref(), 0o640);
    let linked = home.path().join("linked.json");
    std::os::unix::fs::symlink(home.path().join("secrets.json"), &linked).expect("symlink");
    let large = file("large.json", &" ".repeat(1024 * 1024 + 1));
    let shared = home.path().join("shared");
    std::fs::create_dir(&shared).expect("create a directory");
    std::fs::write(shared.join("secrets.json"), secrets_json()).expect("write the file");
    chmod(&shared.join("secrets.json"), 0o600);
    chmod(&shared, 0o777);
    vec![
        (open, "has permission bits for the group or others"),
        (group, "has permission bits for the group or others"),
        (path_text(&linked), "is a symlink"),
        (large, "is larger than 1 MiB"),
        (
            path_text(&home.path().join("missing.json")),
            "does not exist",
        ),
        (
            path_text(&home.path().join("shared/secrets.json")),
            "can be written",
        ),
        (
            file("array.json", &format!("[\"{SENTINEL}\"]")),
            "is not a JSON object",
        ),
        (
            file("broken.json", &format!("{{\"A\": \"{SENTINEL}\"")),
            "is not a JSON object",
        ),
        (
            file("twice.json", &format!(r#"{{"A":"{SENTINEL}","A":"x"}}"#)),
            "names a secret twice",
        ),
        (
            file(
                "number.json",
                &format!(r#"{{"A":"{SENTINEL}","PORT":8080}}"#),
            ),
            "sets `PORT` to something other than a string",
        ),
        (
            file("pasted.json", &format!(r#"{{"{SENTINEL}":null}}"#)),
            "sets a secret whose name hipc does not print",
        ),
    ]
}

/// Every refused secrets file is `usage` before anything is sent, a dry run included, and
/// no message or hint quotes a value.
#[test]
fn a_bad_secrets_file_is_usage_with_zero_requests() {
    let files = files();
    let server = Server::start(vec![]);
    for (path, says) in bad_files(&files.home) {
        for extra in [&["--yes"][..], &["--dry-run"]] {
            let mut args = create_args(&files.compose, &["--secrets-file", &path]);
            args.extend(extra);
            let run = Hipc::against(&files.home, &server).run(&args);
            let error = run.fails(2, "usage");
            assert_run_clean(&run);
            let message = error["message"].as_str().unwrap();
            assert!(message.contains(says), "{path}: {error}");
            // A directory others can write is named for itself: it is what to fix.
            let names = if says == "can be written" {
                "the directory `"
            } else {
                "the secrets file `"
            };
            assert!(
                message.starts_with(names),
                "the message names the file: {error}"
            );
        }
    }
    let open = path_text(&files.home.path().join("open.json"));
    let error = Hipc::against(&files.home, &server)
        .run(&create_args(
            &files.compose,
            &["--secrets-file", &open, "--yes"],
        ))
        .fails(2, "usage");
    assert!(
        error["hint"]
            .as_str()
            .unwrap()
            .starts_with("run `chmod 600 "),
        "{error}"
    );
    assert!(server.requests().is_empty());
}

/// The compose is read before the secrets file, and both before the token; the dry run needs
/// no token.
#[test]
fn the_compose_then_the_secrets_file_then_the_token_are_checked_in_order() {
    let files = files();
    let server = Server::start(vec![]);
    let latin1 = path_text(&files.home.file("latin1.yaml", b"name: caf\xe9\n"));
    let open = path_text(&files.home.file("open.json", secrets_json().as_bytes()));
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o644)).unwrap();
    let no_token = || Hipc::tokenless(&files.home, &server);
    let error = no_token()
        .run(&create_args(&latin1, &["--secrets-file", &open, "--yes"]))
        .fails(2, "usage");
    assert!(
        error["message"].as_str().unwrap().contains("not UTF-8"),
        "{error}"
    );
    let error = no_token()
        .run(&create_args(
            &files.compose,
            &["--secrets-file", &open, "--yes"],
        ))
        .fails(2, "usage");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("permission bits"),
        "{error}"
    );
    let dry = no_token().run(&with_secrets(&files, &["--dry-run"]));
    assert_run_clean(&dry);
    assert_eq!(dry.ok()["dry_run"], true);
    no_token()
        .run(&with_secrets(&files, &["--yes"]))
        .fails(1, "auth");
    assert!(server.requests().is_empty());
}

/// The dry run sends nothing and lists both requests: validate with the compose alone, then
/// the launch with a placeholder for each value.
#[test]
fn the_dry_run_lists_both_requests_with_placeholders() {
    let files = files();
    let server = Server::start(vec![]);
    let run = Hipc::against(&files.home, &server).run(&with_secrets(&files, &["--dry-run"]));
    assert_run_clean(&run);
    let doc = run.ok();
    let launch = json!({
        "flavor": "small",
        "compose": COMPOSE,
        "secrets": {
            "DB_PASSWORD": "<the DB_PASSWORD from --secrets-file>",
            "API_KEY": "<the API_KEY from --secrets-file>",
        },
    });
    assert_eq!(draft(&doc)["method"], "POST");
    assert!(
        draft(&doc)["url"].as_str().unwrap().ends_with(APPS),
        "{doc}"
    );
    assert_eq!(draft(&doc)["body"], launch);
    let requests = doc["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[0]["url"].as_str().unwrap().ends_with(VALIDATE),
        "{doc}"
    );
    assert_eq!(requests[0]["body"], json!({ "compose": COMPOSE }));
    assert_eq!(requests[1]["body"], launch);
    assert!(server.requests().is_empty());
}

/// A declared secret with no value, and a value the compose does not declare, are refused
/// after validate and before the question, so nothing is launched and no value is sent.
#[test]
fn the_secrets_must_be_the_ones_the_compose_declares() {
    let files = files();
    let declares = |names: &[&str]| {
        Server::start(vec![route(
            "POST",
            VALIDATE,
            Reply::json(200, &validated(names)),
        )])
    };
    let server = declares(&["DB_PASSWORD", "API_KEY", "SMTP_TOKEN"]);
    let run = Hipc::against(&files.home, &server).run(&with_secrets(&files, &["--yes"]));
    assert_run_clean(&run);
    let error = run.fails(2, "usage");
    assert_eq!(
        error["message"],
        format!(
            "the compose file `{}` declares the secret `SMTP_TOKEN`, and `--secrets-file` gives \
             it no value",
            files.compose
        )
    );
    assert_eq!(
        error["hint"],
        "add `SMTP_TOKEN` and its value to the file `--secrets-file` names"
    );
    assert_eq!(server.only_request().path(), VALIDATE);

    let server = declares(&["DB_PASSWORD"]);
    let run = Hipc::against(&files.home, &server).run(&with_secrets(&files, &["--yes"]));
    assert_run_clean(&run);
    let error = run.fails(2, "usage");
    assert_eq!(
        error["message"],
        format!(
            "the secrets file gives 1 secret that the compose file `{}` does not declare",
            files.compose
        )
    );
    // The undeclared name is never quoted: it may be a value in the wrong place.
    assert_eq!(
        error["hint"],
        "remove from the secrets file every name the compose does not declare under \
         `x-hippius.secrets`; it declares `DB_PASSWORD`"
    );
    assert_eq!(server.only_request().path(), VALIDATE);
    let server = declares(&[]);
    let error = Hipc::against(&files.home, &server)
        .run(&with_secrets(&files, &["--yes"]))
        .fails(2, "usage");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("gives 2 secrets"),
        "{error}"
    );
    assert!(
        error["hint"]
            .as_str()
            .unwrap()
            .ends_with("it declares none")
    );

    // The check comes before the question: without `--yes` it is still this refusal.
    let server = declares(&["DB_PASSWORD"]);
    Hipc::against(&files.home, &server)
        .run(&with_secrets(&files, &[]))
        .fails(2, "usage");
    assert_eq!(server.only_request().path(), VALIDATE);

    // Without the flag, a declared secret has no value either.
    let server = declares(&["DB_PASSWORD"]);
    let error = Hipc::against(&files.home, &server)
        .run(&create_args(&files.compose, &["--yes"]))
        .fails(2, "usage");
    assert_eq!(
        error["hint"],
        "pass `--secrets-file` a mode-0600 JSON object with a value for each secret the compose \
         declares"
    );
    assert_eq!(server.only_request().path(), VALIDATE);
}

/// A validate answer whose `secret_names` is missing, null, or not a list of names is
/// internal, with or without a secrets file: nothing can be checked, so nothing is launched.
#[test]
fn a_validate_answer_without_its_secret_names_is_internal() {
    let files = files();
    for bare in [
        json!({ "compose_sha256": COMPOSE_SHA256 }),
        json!({ "compose_sha256": COMPOSE_SHA256, "secret_names": null }),
    ] {
        for args in [
            with_secrets(&files, &["--yes"]),
            create_args(&files.compose, &["--yes"]),
        ] {
            let server = Server::start(vec![route(
                "POST",
                VALIDATE,
                Reply::json(200, &bare.to_string()),
            )]);
            let error = Hipc::against(&files.home, &server)
                .run(&args)
                .fails(1, "internal");
            assert!(
                error["message"]
                    .as_str()
                    .unwrap()
                    .starts_with("cannot check the declared secrets"),
                "{error}"
            );
            assert_eq!(server.only_request().path(), VALIDATE);
        }
    }
    let odd = json!({ "compose_sha256": COMPOSE_SHA256, "secret_names": "DB_PASSWORD" });
    let server = Server::start(vec![route(
        "POST",
        VALIDATE,
        Reply::json(200, &odd.to_string()),
    )]);
    let error = Hipc::against(&files.home, &server)
        .run(&with_secrets(&files, &["--yes"]))
        .fails(1, "internal");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("`secret_names` that is not a list of names"),
        "{error}"
    );
    let numbers = json!({ "compose_sha256": COMPOSE_SHA256, "secret_names": [1] });
    let server = Server::start(vec![route(
        "POST",
        VALIDATE,
        Reply::json(200, &numbers.to_string()),
    )]);
    Hipc::against(&files.home, &server)
        .run(&with_secrets(&files, &["--yes"]))
        .fails(1, "internal");
}

/// A refused compose, an answer that hashed other bytes, and a 403 outside the feature all
/// stop at validate: the launch is never sent.
#[test]
fn a_refused_or_misread_compose_stops_before_the_launch() {
    let files = files();
    let refused = json!({
        "error": "app-compose-invalid",
        "detail": "the compose has 1 problem",
        "issues": [{"rule": "image-digest", "line": 3, "column": 12, "message": "pin it"}],
    });
    let server = Server::start(vec![route(
        "POST",
        VALIDATE,
        Reply::json(400, &refused.to_string()),
    )]);
    let error = Hipc::against(&files.home, &server)
        .run(&with_secrets(&files, &["--yes"]))
        .fails(1, "validation");
    assert_eq!(error["details"]["issues"][0]["rule"], "image-digest");
    assert_eq!(server.only_request().path(), VALIDATE);

    let other = json!({ "compose_sha256": "00".repeat(32), "secret_names": [] });
    let server = Server::start(vec![route(
        "POST",
        VALIDATE,
        Reply::json(200, &other.to_string()),
    )]);
    let error = Hipc::against(&files.home, &server)
        .run(&create_args(&files.compose, &["--yes"]))
        .fails(1, "internal");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("the API hashed other bytes than the compose file"),
        "{error}"
    );
    assert_eq!(server.only_request().path(), VALIDATE);

    let unavailable = r#"{"error":"app-engine-unavailable","detail":"not on this account"}"#;
    let server = Server::start(vec![route("POST", VALIDATE, Reply::json(403, unavailable))]);
    let error = Hipc::against(&files.home, &server)
        .run(&with_secrets(&files, &["--yes"]))
        .fails(1, "permission");
    assert_eq!(
        error["hint"],
        "run `hipc compute app-engine get` to see whether the account may use it"
    );
    assert_eq!(server.only_request().path(), VALIDATE);
}

/// Without `--yes` and with nobody to ask, the refusal comes after validate, names the spend
/// and, under a membership, the account, and the launch is not sent.
#[test]
fn without_yes_the_launch_is_refused_after_validate() {
    let files = files();
    let server = launching_server();
    let run = Hipc::against(&files.home, &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&with_secrets(
            &files,
            &["--region", "FR", "--backup-interval", "900"],
        ));
    assert_run_clean(&run);
    let error = run.fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!(
            "this would launch an app VM of flavor `small` with a public IPv4 in `FR`, with \
             backups every 900 seconds, stored backups billed per GB-hour at the price that \
             `hipc compute backups options` shows, billed per hour from the balance in the \
             account `{MEMBER}`, and `--yes` was not passed"
        )
    );
    assert_eq!(server.only_request().path(), VALIDATE);

    // A failover is named with the backups, and `auto` says it restarts from the latest
    // backup, losing what was written after it.
    for (mode, says) in [
        (
            "auto",
            " and failover `auto`, which restarts it on another server from its latest backup \
             if its server dies, losing what was written after that backup, stored backups \
             billed per GB-hour",
        ),
        (
            "manual",
            " and failover `manual`, stored backups billed per GB-hour",
        ),
    ] {
        let error = Hipc::against(&files.home, &launching_server())
            .run(&with_secrets(
                &files,
                &["--backup-interval", "900", "--failover", mode],
            ))
            .fails(2, "confirmation_required");
        let message = error["message"].as_str().unwrap();
        assert!(
            message.contains(&format!("backups every 900 seconds{says}")),
            "{message}"
        );
    }
    let error = Hipc::against(&files.home, &launching_server())
        .run(&with_secrets(&files, &["--on-guest-poweroff", "stop"]))
        .fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        "this would launch an app VM of flavor `small` with a public IPv4, with a power-off from \
         inside the VM leaving it stopped, and billed as a stopped VM until it is started or \
         deleted, billed per hour from the balance, and `--yes` was not passed"
    );
}

/// The launch sends the account and the project, a billing selection from a flag is refused
/// before anything is sent, and a stored billing account is left off this write.
#[test]
fn the_selectors_follow_the_compute_routes() {
    let files = files();
    let server = launching_server();
    Hipc::against(&files.home, &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&with_secrets(&files, &["--project", PROJECT, "--yes"]))
        .ok();
    for request in server.requests() {
        assert_eq!(request.header("x-hippius-account"), Some(MEMBER));
        assert_eq!(request.header("x-hippius-project"), Some(PROJECT));
        assert_eq!(request.header("x-hippius-billing"), None);
    }
    let fresh = Server::start(vec![]);
    for extra in [&["--yes"][..], &["--dry-run"]] {
        let mut args = vec!["--billing", BILLING];
        args.extend(with_secrets(&files, extra));
        Hipc::against(&files.home, &fresh)
            .run(&args)
            .fails(2, "billing_scope_unsupported");
    }
    assert!(fresh.requests().is_empty());
    // A stored billing account is left off the launch, which takes none.
    Hipc::new(&files.home)
        .run(&["account", "billing-accounts", "use", BILLING])
        .ok();
    let doc = Hipc::against(&files.home, &server)
        .run(&with_secrets(&files, &["--dry-run"]))
        .ok();
    assert!(!doc.to_string().contains(BILLING), "{doc}");
    let sent = server.requests().len();
    Hipc::against(&files.home, &server)
        .run(&with_secrets(&files, &["--yes"]))
        .ok();
    let requests = server.requests();
    assert!(requests.len() > sent);
    for request in &requests[sent..] {
        assert_eq!(request.header("x-hippius-billing"), None);
    }
}

/// The sentinel value reaches no output on any path: a launch, a dry run, a 400 that echoes
/// it in every spelling, a 409, a 500, and `--debug`, in JSON and in text mode.
#[test]
fn no_secret_value_reaches_any_output() {
    let files = files();
    let echoed = json!({
        "error": "app-secrets-invalid",
        "detail": format!(
            "secret DB_PASSWORD is {SENTINEL}, or {}, or {}",
            SENTINEL_SPELLINGS[1], SENTINEL_SPELLINGS[2]
        ),
        "line": 4,
        "source": format!("x-hippius: {SENTINEL}"),
        "value": SENTINEL,
        "issues": [{
            "rule": "secret-too-large",
            "line": 4,
            "column": 3,
            "message": format!("{OTHER_SECRET} is too large"),
            "hint": format!("shorten {}", SENTINEL_SPELLINGS[3]),
            "value": SENTINEL,
        }],
    });
    let answers = [
        (400, echoed.to_string()),
        (
            409,
            json!({"error": "no-free-public-ip", "detail": SENTINEL}).to_string(),
        ),
        (500, format!("<html>{SENTINEL}</html>")),
        (202, launched("provisioning").replace("pending", SENTINEL)),
    ];
    for (status, body) in &answers {
        for mode in ["json", "text"] {
            let server = Server::start(vec![
                route(
                    "POST",
                    VALIDATE,
                    Reply::json(200, &validated(&["DB_PASSWORD", "API_KEY"])),
                ),
                route("POST", APPS, Reply::json(*status, body)),
            ]);
            let mut args = vec!["--output", mode, "--debug"];
            args.extend(with_secrets(&files, &["--yes"]));
            let run = Hipc::against(&files.home, &server).run(&args);
            assert_run_clean(&run);
            assert!(
                run.stderr().contains("debug: POST "),
                "{status}: {}",
                run.stderr()
            );
            assert!(
                !run.stderr().contains("services:"),
                "--debug prints no body: {}",
                run.stderr()
            );
            assert_eq!(server.requests().len(), 2, "{status} {mode}");
        }
    }
    let dry = Hipc::against(&files.home, &Server::start(vec![]))
        .run(&with_secrets(&files, &["--debug", "--dry-run"]));
    assert_run_clean(&dry);
}

/// A 400 of a launch that carried secrets keeps the server's code, line, source, and each
/// issue's rule, line, column, message, and hint, every sent value redacted, and drops every
/// other field.
#[test]
fn a_refused_launch_keeps_what_to_fix_with_every_value_redacted() {
    let files = files();
    let refused = json!({
        "error": "app-secrets-invalid",
        "detail": format!("DB_PASSWORD is {SENTINEL}"),
        "line": 4,
        "source": "secrets",
        "value": SENTINEL,
        "issues": [{
            "rule": "secret-name",
            "line": 5,
            "column": 7,
            "message": format!("{OTHER_SECRET} is not allowed"),
            "hint": "rename it",
            "extra": 1,
        }],
    });
    let server = Server::start(vec![
        route(
            "POST",
            VALIDATE,
            Reply::json(200, &validated(&["DB_PASSWORD", "API_KEY"])),
        ),
        route("POST", APPS, Reply::json(400, &refused.to_string())),
    ]);
    let run = Hipc::against(&files.home, &server).run(&with_secrets(&files, &["--yes"]));
    assert_run_clean(&run);
    let error = run.fails(1, "validation");
    assert_eq!(
        error["details"],
        json!({
            "error": "app-secrets-invalid",
            "detail": "DB_PASSWORD is <redacted>",
            "line": 4,
            "source": "secrets",
            "issues": [{
                "rule": "secret-name",
                "line": 5,
                "column": 7,
                "message": "<redacted> is not allowed",
                "hint": "rename it",
            }],
        })
    );
    assert_eq!(
        error["message"],
        "cannot launch an app VM of flavor `small` with a public IPv4: the API says \
         `app-secrets-invalid`"
    );

    // A launch that carried no secret keeps the server's body as it came.
    let server = Server::start(vec![
        route("POST", VALIDATE, Reply::json(200, &validated(&[]))),
        route("POST", APPS, Reply::json(400, &refused.to_string())),
    ]);
    let error = Hipc::against(&files.home, &server)
        .run(&create_args(&files.compose, &["--yes"]))
        .fails(1, "validation");
    assert_eq!(error["details"]["value"], SENTINEL);

    // A 403 outside the feature gets the hint on the launch too.
    let server = Server::start(vec![
        route(
            "POST",
            VALIDATE,
            Reply::json(200, &validated(&["DB_PASSWORD", "API_KEY"])),
        ),
        route(
            "POST",
            APPS,
            Reply::json(403, r#"{"error":"app-engine-unavailable"}"#),
        ),
    ]);
    let error = Hipc::against(&files.home, &server)
        .run(&with_secrets(&files, &["--yes"]))
        .fails(1, "permission");
    assert!(
        error["hint"].as_str().unwrap().contains("app-engine get"),
        "{error}"
    );
    // The slug in `code`, as compute bodies may carry it, is kept with secrets sent too, so
    // the message names what the hint is about.
    let server = Server::start(vec![
        route(
            "POST",
            VALIDATE,
            Reply::json(200, &validated(&["DB_PASSWORD", "API_KEY"])),
        ),
        route(
            "POST",
            APPS,
            Reply::json(403, r#"{"code":"app-engine-unavailable"}"#),
        ),
    ]);
    let error = Hipc::against(&files.home, &server)
        .run(&with_secrets(&files, &["--yes"]))
        .fails(1, "permission");
    assert!(
        error["hint"].as_str().unwrap().contains("app-engine get"),
        "{error}"
    );
    assert_eq!(error["details"]["code"], "app-engine-unavailable");
    assert_eq!(
        error["message"],
        "cannot launch an app VM of flavor `small` with a public IPv4: the API says \
         `app-engine-unavailable`"
    );
}

/// A launch answer whose `compose_sha256` is not the file's is internal, with the app on
/// stdout so its id is not lost; one without the field is printed as it came.
#[test]
fn a_launch_answer_that_hashed_other_bytes_prints_the_app_and_fails() {
    let files = files();
    let other = launched("provisioning").replace(COMPOSE_SHA256, &"ab".repeat(32));
    let server = Server::start(vec![
        route(
            "POST",
            VALIDATE,
            Reply::json(200, &validated(&["DB_PASSWORD", "API_KEY"])),
        ),
        route("POST", APPS, Reply::json(202, &other)),
    ]);
    let run = Hipc::against(&files.home, &server).run(&with_secrets(&files, &["--yes"]));
    assert_run_clean(&run);
    assert_eq!(run.code(), Some(1));
    assert_eq!(run.json()["vm_id"], "vm-1a2b", "the app is printed");
    let error: Value = serde_json::from_str(run.stderr().lines().last().unwrap()).unwrap();
    assert_eq!(error["error"]["code"], "internal");
    assert_eq!(error["error"]["side_effects"], "partial");

    let bare = r#"{"vm_id":"vm-1a2b","status":"provisioning","compose_sha256":null}"#;
    let server = Server::start(vec![
        route("POST", VALIDATE, Reply::json(200, &validated(&[]))),
        route("POST", APPS, Reply::json(202, bare)),
    ]);
    let doc = Hipc::against(&files.home, &server)
        .run(&create_args(&files.compose, &["--yes"]))
        .ok();
    assert_eq!(doc["compose_sha256"], Value::Null);
}

fn waiting_server(polls: Vec<Reply>) -> Server {
    Server::start(vec![
        route("POST", VALIDATE, Reply::json(200, &validated(&[]))),
        route("POST", APPS, Reply::json(202, &launched("provisioning"))),
        route_each("GET", APP, polls),
    ])
}

fn app(status: &str, boot_phase: &str) -> Reply {
    let doc = json!({ "vm_id": "vm-1a2b", "status": status, "boot_phase": boot_phase });
    Reply::json(200, &doc.to_string())
}

/// `--wait` polls the app until its status and boot phase are both `running`, through a
/// passing outage, and prints the last document.
#[test]
fn wait_polls_until_the_app_runs() {
    let files = files();
    let server = waiting_server(vec![
        Reply::json(503, r#"{"detail":"busy"}"#),
        app("running", "running"),
    ]);
    let doc = Hipc::against(&files.home, &server)
        .run(&create_args(&files.compose, &["--wait", "--yes"]))
        .ok();
    assert_eq!(doc["boot_phase"], "running");
    assert_eq!(server.requests().len(), 4);

    // A text-mode run names the app before it waits.
    let server = waiting_server(vec![app("running", "running")]);
    let run = Hipc::against(&files.home, &server).run(&create_args(
        &files.compose,
        &["--output", "text", "--wait", "--yes"],
    ));
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        "note: launched the app `vm-1a2b`; waiting until it is running\n"
    );
}

/// A wait that ends before `running` prints the last app document on stdout and fails, not
/// retryable: on a timeout, a terminal status, a poll of another app, and a failed poll.
#[test]
fn a_wait_that_ends_early_prints_the_app() {
    let files = files();
    let cases = [
        (vec![app("running", "pulling")], "timeout", "1"),
        (vec![app("failed", "running")], "partial_failure", "30"),
        (
            vec![Reply::json(
                200,
                r#"{"vm_id":"vm-9z9z","status":"running"}"#,
            )],
            "internal",
            "30",
        ),
        (
            vec![Reply::json(200, r#"{"vm_id":"vm/9","status":"running"}"#)],
            "internal",
            "30",
        ),
        (
            vec![Reply::json(200, r#"{"status":"running"}"#)],
            "internal",
            "30",
        ),
        (
            vec![Reply::json(500, r#"{"detail":"boom"}"#)],
            "internal",
            "30",
        ),
    ];
    for (polls, code, timeout) in cases {
        let server = waiting_server(polls);
        let run = Hipc::against(&files.home, &server).run(&create_args(
            &files.compose,
            &["--wait-timeout", timeout, "--yes"],
        ));
        assert_eq!(run.code(), Some(1), "{code}: {}", run.stderr());
        assert_eq!(run.json()["vm_id"], "vm-1a2b", "{code}");
        let error: Value = serde_json::from_str(run.stderr().lines().last().unwrap()).unwrap();
        assert_eq!(error["error"]["code"], code, "{}", run.stderr());
        assert_eq!(error["error"]["retryable"], false);
        assert_eq!(error["error"]["side_effects"], "partial");
        assert_eq!(
            error["error"]["hint"],
            "run `hipc compute app-engine apps get vm-1a2b` to see where the launch stands"
        );
    }
}

/// A launch answer the wait cannot follow is internal with the answer on stdout, and one that
/// already runs is printed without a poll.
#[test]
fn a_launch_answer_the_wait_cannot_follow_is_printed() {
    let files = files();
    for (answer, says) in [
        (r#"{"status":"provisioning"}"#, "has no `vm_id`"),
        (
            r#"{"vm_id":"vm/1","status":"provisioning"}"#,
            "by an id `hipc` cannot poll",
        ),
    ] {
        let server = Server::start(vec![
            route("POST", VALIDATE, Reply::json(200, &validated(&[]))),
            route("POST", APPS, Reply::json(202, answer)),
        ]);
        let run = Hipc::against(&files.home, &server)
            .run(&create_args(&files.compose, &["--wait", "--yes"]));
        assert_eq!(run.code(), Some(1), "{}", run.stderr());
        assert_eq!(run.json(), serde_json::from_str::<Value>(answer).unwrap());
        assert!(run.stderr().contains(says), "{}", run.stderr());
        assert_eq!(server.requests().len(), 2);
    }
    let running = r#"{"vm_id":"vm/1","status":"running","boot_phase":"running"}"#;
    let server = Server::start(vec![
        route("POST", VALIDATE, Reply::json(200, &validated(&[]))),
        route("POST", APPS, Reply::json(202, running)),
    ]);
    let doc = Hipc::against(&files.home, &server)
        .run(&create_args(&files.compose, &["--wait", "--yes"]))
        .ok();
    assert_eq!(doc["vm_id"], "vm/1");
}

/// A `vm_id` that is a secret value sent, though it has an id's shape, is never printed: not
/// in the note, a poll's `--debug` line, the error, or the hint. The app is named "the app",
/// the id is not polled, and stdout holds the redacted answer. A poll answering with another
/// app's id that is a value sent does not name it either.
#[test]
fn a_vm_id_that_is_a_secret_value_is_never_printed() {
    const ID_SECRET: &str = "tok_ab12cd34ef56";
    let files = files();
    let secrets = path_text(&files.home.file(
        "id-secret.json",
        json!({ "API_KEY": ID_SECRET }).to_string().as_bytes(),
    ));
    let answer = |status: &str, vm_id: &str| {
        json!({"vm_id": vm_id, "status": status, "boot_phase": "pending"}).to_string()
    };
    let cases = [
        (answer("provisioning", ID_SECRET), None, "internal"),
        (answer("failed", ID_SECRET), None, "partial_failure"),
        (
            launched("provisioning"),
            Some(answer("running", ID_SECRET)),
            "internal",
        ),
    ];
    for (launch, polls, code) in &cases {
        for mode in ["json", "text"] {
            let mut routes = vec![
                route("POST", VALIDATE, Reply::json(200, &validated(&["API_KEY"]))),
                route("POST", APPS, Reply::json(202, launch)),
            ];
            if let Some(poll) = polls {
                routes.push(route("GET", APP, Reply::json(200, poll)));
            }
            let server = Server::start(routes);
            let mut args = vec!["--output", mode, "--debug"];
            args.extend(create_args(
                &files.compose,
                &["--secrets-file", &secrets, "--wait", "--yes"],
            ));
            let run = Hipc::against(&files.home, &server).run(&args);
            assert_eq!(run.code(), Some(1), "{code}: {}", run.stderr());
            assert!(!run.stdout().contains(ID_SECRET), "{}", run.stdout());
            assert!(!run.stderr().contains(ID_SECRET), "{}", run.stderr());
            let polled = server.requests().len() - 2;
            assert_eq!(polled, usize::from(polls.is_some()), "{code}: {mode}");
            if mode == "json" {
                let error: Value =
                    serde_json::from_str(run.stderr().lines().last().unwrap()).unwrap();
                assert_eq!(error["error"]["code"], *code, "{}", run.stderr());
                assert_eq!(error["error"]["side_effects"], "partial");
                if polls.is_none() {
                    assert_eq!(run.json()["vm_id"], "<redacted>");
                    assert_eq!(
                        error["error"]["hint"],
                        "run `hipc compute app-engine apps list` to find the new app"
                    );
                } else {
                    assert_eq!(
                        error["error"]["message"],
                        "the poll of the app `vm-1a2b` answered with the document of another app"
                    );
                }
            }
        }
    }
}

/// A launch answer without a `vm_id` names no app, so it is internal and partial whether or
/// not hipc waits, with the redacted answer on stdout.
#[test]
fn a_launch_answer_without_a_vm_id_fails_with_or_without_a_wait() {
    let files = files();
    let answer = json!({"status": "provisioning", "note": SENTINEL}).to_string();
    for wait in ["--wait", "--no-wait"] {
        let server = Server::start(vec![
            route(
                "POST",
                VALIDATE,
                Reply::json(200, &validated(&["DB_PASSWORD", "API_KEY"])),
            ),
            route("POST", APPS, Reply::json(202, &answer)),
        ]);
        let run = Hipc::against(&files.home, &server).run(&with_secrets(&files, &[wait, "--yes"]));
        assert_run_clean(&run);
        assert_eq!(run.code(), Some(1), "{wait}: {}", run.stderr());
        assert_eq!(
            run.json(),
            json!({"status": "provisioning", "note": "<redacted>"}),
            "{wait}"
        );
        let error: Value = serde_json::from_str(run.stderr().lines().last().unwrap()).unwrap();
        assert_eq!(error["error"]["code"], "internal", "{wait}");
        assert_eq!(error["error"]["side_effects"], "partial", "{wait}");
        assert_eq!(
            error["error"]["message"],
            "the launch answer has no `vm_id`"
        );
        assert_eq!(server.requests().len(), 2, "{wait}: no poll");
    }
}

/// `hipc api` refuses the three App Engine routes whose body sets `secrets`, a dry run
/// included, and sends them as usual without it.
#[test]
fn hipc_api_refuses_a_body_with_secrets() {
    let home = Home::new();
    let with = path_text(
        &home.file(
            "with.json",
            json!({"compose": COMPOSE, "secrets": {"A": SENTINEL}})
                .to_string()
                .as_bytes(),
        ),
    );
    let without = path_text(
        &home.file(
            "without.json",
            json!({"compose": COMPOSE, "flavor": "small"})
                .to_string()
                .as_bytes(),
        ),
    );
    let server = Server::start(vec![]);
    for (method, path, hint) in [
        (
            "POST",
            "/compute/app-engine/apps/",
            "apps create --secrets-file",
        ),
        (
            "PUT",
            "/compute/app-engine/apps/vm-1a2b/compose/",
            "not in this version",
        ),
        (
            "PATCH",
            "/compute/app-engine/apps/vm-1a2b/compose/",
            "not in this version",
        ),
        (
            "POST",
            "/compute/app-engine/validate",
            "app-engine validate`",
        ),
    ] {
        for extra in [&["--yes"][..], &["--dry-run"]] {
            let mut args = vec!["api", method, path, "--body-file", &with];
            args.extend(extra);
            let run = Hipc::against(&home, &server).run(&args);
            assert_run_clean(&run);
            let error = run.fails(2, "usage");
            assert!(
                error["message"]
                    .as_str()
                    .unwrap()
                    .contains("with `secrets`"),
                "{error}"
            );
            assert!(error["hint"].as_str().unwrap().contains(hint), "{error}");
        }
        let doc = Hipc::against(&home, &server)
            .run(&["--dry-run", "api", method, path, "--body-file", &without])
            .ok();
        assert_eq!(draft(&doc)["method"], method);
    }
    assert!(server.requests().is_empty());
}

/// A server that validates the three short-value names, launches with `launch`, and answers
/// each poll in turn.
fn short_value_server(launch: &str, polls: Vec<Reply>) -> Server {
    let mut routes = vec![
        route(
            "POST",
            VALIDATE,
            Reply::json(200, &validated(&["DB_PASSWORD", "API_KEY", "OTHER"])),
        ),
        route("POST", APPS, Reply::json(202, launch)),
    ];
    if !polls.is_empty() {
        routes.push(route_each("GET", APP, polls));
    }
    Server::start(routes)
}

/// Values as short as `-`, `n`, and `a` cannot change what hipc decides or the fields it
/// checked by shape: the id, the hash, and the status come out as they came, the wait reaches
/// `running` or a real terminal state, and only free text is redacted.
#[test]
fn a_short_secret_value_changes_no_id_hash_or_status() {
    let files = files();
    let short = path_text(&files.home.file(
        "short.json",
        br#"{"DB_PASSWORD":"-","API_KEY":"n","OTHER":"a"}"#,
    ));
    let args = |extra: &[&'static str]| {
        let mut args = vec![
            "compute",
            "app-engine",
            "apps",
            "create",
            "--flavor",
            "small",
            "--compose-file",
            files.compose.as_str(),
            "--secrets-file",
            short.as_str(),
        ];
        args.extend(extra);
        args
    };
    let launch = json!({
        "vm_id": "vm-1a2b",
        "name": "an app",
        "status": "provisioning",
        "boot_phase": "pending",
        "compose_sha256": COMPOSE_SHA256,
    })
    .to_string();
    let server = short_value_server(&launch, vec![]);
    let doc = Hipc::against(&files.home, &server)
        .run(&args(&["--no-wait", "--yes"]))
        .ok();
    assert_eq!(doc["vm_id"], "vm-1a2b");
    assert_eq!(doc["compose_sha256"], COMPOSE_SHA256);
    assert_eq!(doc["status"], "provisioning");
    assert_eq!(doc["boot_phase"], "pending");
    assert_eq!(doc["name"], "<redacted><redacted> <redacted>pp");

    let server = short_value_server(&launch, vec![app("running", "running")]);
    let doc = Hipc::against(&files.home, &server)
        .run(&args(&["--wait", "--yes"]))
        .ok();
    assert_eq!(
        (doc["vm_id"].as_str(), doc["status"].as_str()),
        (Some("vm-1a2b"), Some("running"))
    );

    let server = short_value_server(&launch, vec![app("failed", "running")]);
    let run = Hipc::against(&files.home, &server).run(&args(&["--wait", "--yes"]));
    assert_eq!(run.code(), Some(1), "{}", run.stderr());
    assert_eq!(run.json()["vm_id"], "vm-1a2b");
    let error: Value = serde_json::from_str(run.stderr().lines().last().unwrap()).unwrap();
    assert_eq!(error["error"]["code"], "partial_failure");
    assert_eq!(
        error["error"]["message"],
        "the app `vm-1a2b` reached `failed` instead of `running`"
    );

    // A refusal keeps its kebab code and the issue's rule whole; free text is redacted.
    let refused = json!({
        "error": "app-secret-invalid",
        "detail": "an a",
        "issues": [{"rule": "secret-name", "line": 1, "column": 2, "message": "a"}],
    });
    let server = Server::start(vec![
        route(
            "POST",
            VALIDATE,
            Reply::json(200, &validated(&["DB_PASSWORD", "API_KEY", "OTHER"])),
        ),
        route("POST", APPS, Reply::json(400, &refused.to_string())),
    ]);
    let error = Hipc::against(&files.home, &server)
        .run(&args(&["--yes"]))
        .fails(1, "validation");
    assert_eq!(error["details"]["error"], "app-secret-invalid");
    assert_eq!(error["details"]["issues"][0]["rule"], "secret-name");
    assert_eq!(
        error["details"]["detail"],
        "<redacted><redacted> <redacted>"
    );
    assert_eq!(
        error["message"],
        "cannot launch an app VM of flavor `small` with a public IPv4: the API says \
         `app-secret-invalid`"
    );
}

/// The message of a refused launch is built from the kept fields alone: a field that was
/// dropped (a field error, a `message`, a value changed in case) cannot reach it.
#[test]
fn a_dropped_field_never_reaches_the_message() {
    let files = files();
    let upper = SENTINEL.to_uppercase();
    for body in [
        json!({"secrets": {"DB_PASSWORD": [format!("{upper} is too long")]}}),
        json!({"message": format!("{upper} is not allowed")}),
        json!({"error": format!("{upper} bad"), "non_field_errors": [upper.clone()]}),
        json!({"detail": 7, "error": "Not A Code", "title": upper.clone()}),
    ] {
        let server = Server::start(vec![
            route(
                "POST",
                VALIDATE,
                Reply::json(200, &validated(&["DB_PASSWORD", "API_KEY"])),
            ),
            route("POST", APPS, Reply::json(400, &body.to_string())),
        ]);
        for mode in ["json", "text"] {
            let mut all = vec!["--output", mode];
            all.extend(with_secrets(&files, &["--yes"]));
            let run = Hipc::against(&files.home, &server).run(&all);
            assert_eq!(run.code(), Some(1), "{}", run.stderr());
            assert!(!run.stderr().contains("VQ93"), "{body}: {}", run.stderr());
        }
    }
    // With the code dropped, a kept `detail` speaks, redacted, and nothing else.
    let body = json!({"detail": format!("{SENTINEL} is bad"), "message": upper});
    let server = Server::start(vec![
        route(
            "POST",
            VALIDATE,
            Reply::json(200, &validated(&["DB_PASSWORD", "API_KEY"])),
        ),
        route("POST", APPS, Reply::json(400, &body.to_string())),
    ]);
    let error = Hipc::against(&files.home, &server)
        .run(&with_secrets(&files, &["--yes"]))
        .fails(1, "validation");
    assert_eq!(
        error["message"],
        "cannot launch an app VM of flavor `small` with a public IPv4: the API says \
         `<redacted> is bad`"
    );
    assert_eq!(error["details"], json!({"detail": "<redacted> is bad"}));
}

/// A value holding control characters is redacted in every spelling a message may carry,
/// the Rust escapes `phrase::quote` writes included, in JSON and text mode.
#[test]
fn a_value_with_control_characters_is_redacted_in_every_spelling() {
    let files = files();
    let secrets = path_text(&files.home.file(
        "controls.json",
        br#"{"DB_PASSWORD":"esc\u001bnul\u0000-Zq81"}"#,
    ));
    let value = "esc\u{1b}nul\u{0}-Zq81";
    let echoed = json!({
        "detail": format!("{value} is bad"),
        "source": format!("{value:?}"),
        "issues": [
            {"message": value, "hint": format!("`{value}`")},
            {"message": r"esc\x1bnul\x00-Zq81", "hint": r"esc\u001bnul\u0000-Zq81"},
        ],
    });
    let server = Server::start(vec![
        route(
            "POST",
            VALIDATE,
            Reply::json(200, &validated(&["DB_PASSWORD"])),
        ),
        route("POST", APPS, Reply::json(400, &echoed.to_string())),
    ]);
    for mode in ["json", "text"] {
        let run = Hipc::against(&files.home, &server).run(&create_args(
            &files.compose,
            &["--output", mode, "--secrets-file", &secrets, "--yes"],
        ));
        assert_eq!(run.code(), Some(1), "{}", run.stderr());
        assert!(!run.stderr().contains("Zq81"), "{}", run.stderr());
    }
}

/// A name the server would not take is described in the dry run, never printed.
#[test]
fn the_dry_run_describes_a_name_that_is_not_a_secret_name() {
    let files = files();
    let pasted = path_text(&files.home.file(
        "pasted.json",
        format!(r#"{{"DB_PASSWORD":"x","{SENTINEL}":"y"}}"#).as_bytes(),
    ));
    let run = Hipc::against(&files.home, &Server::start(vec![])).run(&create_args(
        &files.compose,
        &["--secrets-file", &pasted, "--dry-run"],
    ));
    assert_run_clean(&run);
    let doc = run.ok();
    assert_eq!(
        draft(&doc)["body"]["secrets"],
        json!({
            "DB_PASSWORD": "<the DB_PASSWORD from --secrets-file>",
            "<the name of secret 2 in --secrets-file, not printed>":
                "<the value of secret 2 in --secrets-file>",
        })
    );
}

/// The question names the OS disk tier, which is billed at its own price.
#[test]
fn the_question_names_the_os_disk_tier() {
    let files = files();
    let server = launching_server();
    let error = Hipc::against(&files.home, &server)
        .run(&with_secrets(&files, &["--os-disk-tier", "hdd"]))
        .fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        "this would launch an app VM of flavor `small` with a public IPv4, with an OS disk on \
         tier `hdd` or faster, billed at the `hdd` price that `hipc compute prices` shows \
         (`vm_by_os_disk_tier`), billed per hour from the balance, and `--yes` was not passed"
    );
}

/// A failed boot that stalled comes back by itself, as a VM's does, so the wait goes on.
#[test]
fn a_stalled_boot_is_waited_on() {
    let files = files();
    let stalled = r#"{"vm_id":"vm-1a2b","status":"failed","boot_stalled":true}"#;
    let server = waiting_server(vec![Reply::json(200, stalled), app("running", "running")]);
    let doc = Hipc::against(&files.home, &server)
        .run(&create_args(&files.compose, &["--wait", "--yes"]))
        .ok();
    assert_eq!(doc["status"], "running");
    assert_eq!(server.requests().len(), 4);
}

/// Runs a launch with the secrets `secrets` against a server that validates `names` and
/// answers the launch with `status` and `body` (and `header`, when given).
fn launch_with(secrets: &str, names: &[&str], reply: Reply) -> Run {
    let files = files();
    let path = path_text(&files.home.file("given.json", secrets.as_bytes()));
    let server = Server::start(vec![
        route("POST", VALIDATE, Reply::json(200, &validated(names))),
        route("POST", APPS, reply),
    ]);
    Hipc::against(&files.home, &server).run(&create_args(
        &files.compose,
        &["--secrets-file", &path, "--no-wait", "--yes"],
    ))
}

/// A value that fits a field's shape, such as a 40-digit hex key, is not printed in that
/// field: a shaped top-level field is printed as it came only when it holds no value, nothing
/// below the top level is taken for its shape, and the kebab `error` and the request id of a
/// refusal follow the same rule.
#[test]
fn a_value_that_fits_a_shape_is_not_printed() {
    let key = "9f2c4e7a1b3d5f60718293a4b5c6d7e8f9012345";
    let secrets = json!({ "API_KEY": key }).to_string();
    let answer = json!({
        "vm_id": "vm-1a2b",
        "status": key,
        "boot_phase": "pending",
        "compose_sha256": COMPOSE_SHA256,
        "secret_names": ["API_KEY"],
        "guest": {"error": key, "power_state": key, "vm_id": key},
        "note": format!("key {key}"),
    });
    let run = launch_with(
        &secrets,
        &["API_KEY"],
        Reply::json(202, &answer.to_string()),
    );
    assert!(!run.stdout().contains(key), "{}", run.stdout());
    let doc = run.ok();
    assert_eq!(doc["vm_id"], "vm-1a2b");
    assert_eq!(doc["status"], "<redacted>");
    assert_eq!(doc["guest"]["vm_id"], "<redacted>");

    let refused = json!({"error": key, "detail": format!("bad {key}")}).to_string();
    let reply = Reply::json(400, &refused).header("X-Request-Id", key);
    let run = launch_with(&secrets, &["API_KEY"], reply);
    assert!(!run.stderr().contains(key), "{}", run.stderr());
    let error = run.fails(1, "validation");
    assert_eq!(error["request_id"], Value::Null);
    assert_eq!(error["details"]["error"], "<redacted>");
}

/// A redirect's target is never shown: its message says only that it was a redirect.
#[test]
fn a_redirect_shows_no_target() {
    let key = "9f2c4e7a1b3d5f60718293a4b5c6d7e8f9012345";
    let secrets = json!({ "API_KEY": key }).to_string();
    let reply = Reply::json(302, "").header(
        "Location",
        &format!("https://elsewhere.example/x/{key}/y?k={key}"),
    );
    let run = launch_with(&secrets, &["API_KEY"], reply);
    assert!(!run.stderr().contains(key), "{}", run.stderr());
    assert!(!run.stderr().contains("elsewhere"), "{}", run.stderr());
    let error: Value = serde_json::from_str(run.stderr().lines().last().unwrap()).unwrap();
    assert!(
        error["error"]["message"].as_str().unwrap().ends_with(
            "the API answered with a redirect (status `302`); its target is not \
                        shown, since it may repeat a secret"
        ),
        "{error}"
    );
}

/// A number that spells an all-digit value is redacted; a short value leaves numbers alone.
#[test]
fn a_number_that_spells_a_value_is_redacted() {
    let secrets = json!({ "PIN": "12345678" }).to_string();
    let answer = json!({
        "vm_id": "vm-1",
        "status": "provisioning",
        "compose_sha256": COMPOSE_SHA256,
        "port": 12_345_678,
        "size": 9_123_456_789_u64,
        "count": 2,
        "note": "12345678",
    });
    let doc = launch_with(&secrets, &["PIN"], Reply::json(202, &answer.to_string())).ok();
    assert_eq!(doc["port"], "<redacted>");
    assert_eq!(doc["size"], "<redacted>");
    assert_eq!(doc["count"], 2);
    assert_eq!(doc["note"], "<redacted>");
}

/// A value Python's `repr` or `json.dumps` writes with escapes (a zero-width space, NEL) is
/// redacted in those spellings too.
#[test]
fn a_value_with_invisible_characters_is_redacted_in_its_escaped_spellings() {
    let secrets = "{\"K\":\"pa\u{200b}ss\\u0085word-Wt55\"}";
    for detail in [
        r"value 'pa\u200bss\x85word-Wt55'",
        r"value pa\u200bss\u0085word-Wt55",
        "value pa\u{200b}ss\u{85}word-Wt55",
    ] {
        let body = json!({ "detail": detail }).to_string();
        let run = launch_with(secrets, &["K"], Reply::json(400, &body));
        assert!(!run.stderr().contains("Wt55"), "{}", run.stderr());
        run.fails(1, "validation");
    }
}
