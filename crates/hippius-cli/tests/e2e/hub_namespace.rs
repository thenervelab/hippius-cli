//! `hipc hub namespace update` and `hipc hub namespace robot rotate`: opening or closing the
//! namespace, which needs `--public` to open it (tenet 5), and issuing a new docker robot
//! secret, which needs `--reveal` (tenet 9) and keeps every `docker login` command the server
//! sends off stdout. Both need `--yes`, and `/api/registry/` refuses a membership.

use serde_json::{Value, json};

use crate::common::{Hipc, Home, MEMBER, Reply, Server, draft, route};

const PUBLICITY: &str = "/api/registry/me/publicity/";

const ROTATE: &str = "/api/registry/robot/rotate/";

/// The rotate answer in the shape the `hippius-hub` client reads.
const ROTATED: &str = r#"{"robot_login":"robot$acme","robot_secret":"s3cr3t-rotated"}"#;

fn publicity(reply: Reply) -> Server {
    Server::start(vec![route("PATCH", PUBLICITY, reply)])
}

fn rotate(reply: Reply) -> Server {
    Server::start(vec![route("POST", ROTATE, reply)])
}

#[test]
fn update_public_sends_true_and_prints_the_answer() {
    let server = publicity(Reply::json(200, r#"{"public":true}"#));
    let doc = Hipc::against(&Home::new(), &server)
        .run(&[
            "hub",
            "namespace",
            "update",
            "--visibility",
            "public",
            "--public",
            "--yes",
        ])
        .ok();
    assert_eq!(doc, json!({"public": true}));
    let request = server.only_request();
    assert_eq!(request.method, "PATCH");
    assert_eq!(request.json(), json!({"public": true}));
}

/// Closing the namespace publishes nothing, so it needs `--yes` alone, and `--public` beside
/// it changes nothing.
#[test]
fn update_private_sends_false_and_needs_no_public() {
    for extra in [&[][..], &["--public"]] {
        let server = publicity(Reply::json(200, r#"{"public":false}"#));
        let mut args = vec![
            "hub",
            "namespace",
            "update",
            "--visibility",
            "private",
            "--yes",
        ];
        args.extend(extra);
        let doc = Hipc::against(&Home::new(), &server).run(&args).ok();
        assert_eq!(doc, json!({"public": false}));
        assert_eq!(server.only_request().json(), json!({"public": false}));
    }
}

/// Without `--public`, opening the namespace is refused before the token check and the
/// confirmation, `--yes` or not, and nothing is sent; the hint repeats the command with it.
#[test]
fn update_public_without_public_is_refused_and_sends_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    for args in [
        &[
            "hub",
            "namespace",
            "update",
            "--visibility",
            "public",
            "--yes",
        ][..],
        &["hub", "namespace", "update", "--visibility", "public"],
    ] {
        let error = Hipc::against(&home, &server)
            .run(args)
            .fails(2, "public_required");
        assert_eq!(
            error["message"],
            "making the Hub namespace public lets anyone pull its images and models without \
             logging in, and `--public` was not passed"
        );
        assert!(
            error["hint"].as_str().unwrap().contains("--public"),
            "{error}"
        );
        assert_eq!(error["side_effects"], "none");
        // Before the token check too.
        Hipc::new(&home)
            .env("HIPPIUS_API_URL", server.url())
            .run(args)
            .fails(2, "public_required");
    }
    assert!(server.requests().is_empty());
}

/// Without `--yes` on a pipe each way is refused with the action it names, and nothing is
/// sent; the token is checked before the question.
#[test]
fn update_without_yes_is_refused_and_names_the_change() {
    let server = Server::start(vec![]);
    let home = Home::new();
    for (args, action) in [
        (
            &[
                "hub",
                "namespace",
                "update",
                "--visibility",
                "public",
                "--public",
            ][..],
            "make the Hub namespace public",
        ),
        (
            &["hub", "namespace", "update", "--visibility", "private"],
            "make the Hub namespace private",
        ),
    ] {
        let error = Hipc::against(&home, &server)
            .run(args)
            .fails(2, "confirmation_required");
        assert!(
            error["message"].as_str().unwrap().contains(action),
            "{error}"
        );
        Hipc::new(&home)
            .env("HIPPIUS_API_URL", server.url())
            .run(args)
            .fails(1, "auth");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn update_needs_a_known_visibility() {
    let server = Server::start(vec![]);
    for args in [
        &["hub", "namespace", "update", "--yes"][..],
        &[
            "hub",
            "namespace",
            "update",
            "--visibility",
            "open",
            "--yes",
        ],
        &[
            "hub",
            "namespace",
            "update",
            "--visibility",
            "true",
            "--yes",
        ],
        &["hub", "namespace", "update", "--public", "--yes"],
    ] {
        Hipc::against(&Home::new(), &server)
            .run(args)
            .fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

/// A dry run needs neither `--public` nor `--yes` nor a token, and shows the PATCH.
#[test]
fn update_dry_run_shows_the_patch_and_sends_nothing() {
    let server = Server::start(vec![]);
    let base = server.url();
    for (visibility, public) in [("public", true), ("private", false)] {
        let doc = Hipc::new(&Home::new())
            .env("HIPPIUS_API_URL", base)
            .run(&[
                "--dry-run",
                "hub",
                "namespace",
                "update",
                "--visibility",
                visibility,
            ])
            .ok();
        assert_eq!(draft(&doc)["method"], "PATCH");
        assert_eq!(draft(&doc)["url"], format!("{base}{PUBLICITY}"));
        assert_eq!(draft(&doc)["body"], json!({ "public": public }));
        assert_eq!(doc["requests"].as_array().unwrap().len(), 1);
    }
    assert!(server.requests().is_empty());
}

#[test]
fn update_without_a_project_hints_at_provision() {
    let server = publicity(Reply::json(
        403,
        r#"{"detail":"You must call /api/registry/provision/ first."}"#,
    ));
    let error = Hipc::against(&Home::new(), &server)
        .run(&[
            "hub",
            "namespace",
            "update",
            "--visibility",
            "private",
            "--yes",
        ])
        .fails(1, "permission");
    assert!(
        error["hint"]
            .as_str()
            .unwrap()
            .contains("hub namespace create <namespace> --reveal --yes`"),
        "{error}"
    );
}

/// `hipc api` holds the same route to the same flag (tenet 5): a body that opens the
/// namespace needs `--public`, one that closes it does not.
#[test]
fn api_publicity_patch_needs_public_to_open_the_namespace() {
    let server = publicity(Reply::json(200, r#"{"public":true}"#));
    let home = Home::new();
    let open = home.file("open.json", br#"{"public": true}"#);
    let open = open.to_str().unwrap();
    let error = Hipc::against(&home, &server)
        .run(&[
            "api",
            "PATCH",
            "/registry/me/publicity/",
            "--body-file",
            open,
            "--yes",
        ])
        .fails(2, "public_required");
    assert!(
        error["hint"].as_str().unwrap().contains("--public"),
        "{error}"
    );
    // The refusal names the namespace, not a bucket.
    assert_eq!(
        error["message"],
        "this may make the Hub namespace public, so anyone can pull its images and models \
         without logging in, and `--public` was not passed"
    );
    // The route toggles, so a body that does not set `public` to `false` needs the flag too,
    // as does a PATCH with no body; slashless, the route is the same.
    let empty = home.file("empty.json", b"{}");
    for args in [
        &[
            "api",
            "PATCH",
            "/registry/me/publicity/",
            "--body-file",
            empty.to_str().unwrap(),
            "--yes",
        ][..],
        &["api", "PATCH", "/registry/me/publicity/", "--yes"],
        &["api", "PATCH", "/registry/me/publicity", "--yes"],
    ] {
        Hipc::against(&home, &server)
            .run(args)
            .fails(2, "public_required");
    }
    assert!(server.requests().is_empty());
    // A bucket route keeps the bucket sentence.
    let error = Hipc::against(&home, &server)
        .run(&["api", "POST", "/cdn/zones/", "--yes"])
        .fails(2, "public_required");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with("this may make a private bucket readable by anyone"),
        "{error}"
    );
    let doc = Hipc::against(&home, &server)
        .run(&[
            "api",
            "PATCH",
            "/registry/me/publicity/",
            "--body-file",
            open,
            "--public",
            "--yes",
        ])
        .ok();
    assert_eq!(doc, json!({"public": true}));
    let closed = home.file("closed.json", br#"{"public": false}"#);
    Hipc::against(&home, &server)
        .run(&[
            "api",
            "PATCH",
            "/registry/me/publicity/",
            "--body-file",
            closed.to_str().unwrap(),
            "--yes",
        ])
        .ok();
    let bodies: Vec<Value> = server
        .requests()
        .iter()
        .map(crate::common::Recorded::json)
        .collect();
    assert_eq!(bodies, [json!({"public": true}), json!({"public": false})]);
}

/// The new secret goes on stdout and nowhere else (`ok` checks stderr is empty), with the
/// `docker login` command `hipc` builds, which reads the secret from stdin.
#[test]
fn rotate_prints_the_new_secret_on_stdout_only() {
    let server = rotate(Reply::json(201, ROTATED));
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "namespace", "robot", "rotate", "--reveal", "--yes"])
        .ok();
    assert_eq!(
        doc,
        json!({
            "robot_login": "robot$acme",
            "robot_secret": "s3cr3t-rotated",
            "docker_login_command":
                "docker login registry.hippius.com -u 'robot$acme' --password-stdin",
        })
    );
    let request = server.only_request();
    assert_eq!(request.method, "POST");
    assert!(request.body.is_empty(), "{:?}", request.body);
    // In text mode too: the secret is on stdout, not stderr.
    let run = Hipc::against(&Home::new(), &server).run(&[
        "--output",
        "text",
        "hub",
        "namespace",
        "robot",
        "rotate",
        "--reveal",
        "--yes",
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(run.stdout().contains("s3cr3t-rotated"), "{}", run.stdout());
    assert!(!run.stderr().contains("s3cr3t-rotated"), "{}", run.stderr());
    // `--debug` names the request on stderr and never its answer.
    let run = Hipc::against(&Home::new(), &server).run(&[
        "--debug",
        "hub",
        "namespace",
        "robot",
        "rotate",
        "--reveal",
        "--yes",
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(run.stdout().contains("s3cr3t-rotated"), "{}", run.stdout());
    assert!(run.stderr().contains(ROTATE), "{}", run.stderr());
    assert!(!run.stderr().contains("s3cr3t-rotated"), "{}", run.stderr());
}

/// The login is named `robot_login`, `robot_name`, `username`, or `login`, in that order;
/// with none of them the command is null.
#[test]
fn rotate_builds_the_login_command_from_the_name_it_finds() {
    for (body, login) in [
        (
            json!({"robot_name": "robot$n", "username": "robot$a", "secret": "s"}),
            json!("docker login registry.hippius.com -u 'robot$n' --password-stdin"),
        ),
        (
            json!({"username": "robot$a", "login": "robot$b", "secret": "s"}),
            json!("docker login registry.hippius.com -u 'robot$a' --password-stdin"),
        ),
        (
            json!({"login": "robot$b", "secret": "s"}),
            json!("docker login registry.hippius.com -u 'robot$b' --password-stdin"),
        ),
        (json!({"secret": "s"}), Value::Null),
        (json!({"robot_login": 7, "secret": "s"}), Value::Null),
    ] {
        let server = rotate(Reply::json(201, &body.to_string()));
        let doc = Hipc::against(&Home::new(), &server)
            .run(&["hub", "namespace", "robot", "rotate", "--reveal", "--yes"])
            .ok();
        assert_eq!(doc["docker_login_command"], login, "{body}");
        assert_eq!(doc["secret"], "s");
    }
    // An answer that is not an object is printed as sent: there is nowhere to add the field.
    let server = rotate(Reply::json(201, r#""s3cr3t""#));
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "namespace", "robot", "rotate", "--reveal", "--yes"])
        .ok();
    assert_eq!(doc, json!("s3cr3t"));
}

/// A server `docker login` command under a command key is dropped whatever it holds, at any
/// depth and in any case; the secret keys stay, since printing the secret is the point.
#[test]
fn rotate_drops_every_command_field_the_server_sends() {
    for command in [
        json!("docker login registry.hippius.com -u robot -p s3cr3t-rotated"),
        json!("echo s3cr3t-rotated | docker login -u robot --password-stdin"),
        json!(["docker", "login", "-u", "robot", "-ps3cr3t-rotated"]),
        json!("ZG9ja2VyIGxvZ2lu"),
    ] {
        let body = json!({
            "robot_login": "robot$acme",
            "robot_secret": "s3cr3t-rotated",
            "docker_login": command,
            "Docker_Login_Cmd": command,
            "nested": {"login_cmd": command, "command": command},
        });
        let server = rotate(Reply::json(201, &body.to_string()));
        let doc = Hipc::against(&Home::new(), &server)
            .run(&["hub", "namespace", "robot", "rotate", "--reveal", "--yes"])
            .ok();
        assert_eq!(
            doc,
            json!({
                "robot_login": "robot$acme",
                "robot_secret": "s3cr3t-rotated",
                "nested": {},
                "docker_login_command":
                    "docker login registry.hippius.com -u 'robot$acme' --password-stdin",
            }),
            "{command}"
        );
    }
}

/// A `docker login` command anywhere else may pass the secret where it could be copied into
/// a shell history, so the whole answer is refused and nothing is printed. The rotate went
/// through, so the error is `partial` and says the old secret no longer works; the hint
/// names the rotate with the run's own flags. The secret is on neither stream.
#[test]
fn rotate_refuses_an_answer_with_any_other_docker_login_command() {
    for body in [
        json!({"robot_secret": "s3cr3t-rotated", "help": "docker login -p s3cr3t-rotated"}),
        json!({"robot_secret": "s3cr3t-rotated", "argv": ["docker", "login", "-u", "r"]}),
        json!({
            "robot_secret": "s3cr3t-rotated",
            "x": ["echo s3cr3t-rotated | docker\u{200B}login"],
        }),
        json!({"robot_secret": "s3cr3t-rotated", "secret": "DOCKER LOGIN"}),
    ] {
        let server = rotate(Reply::json(201, &body.to_string()));
        let run = Hipc::against(&Home::new(), &server)
            .env("HIPPIUS_PROFILE", "work")
            .run(&["hub", "namespace", "robot", "rotate", "--reveal", "--yes"]);
        assert_eq!(run.stdout(), "");
        assert!(!run.stderr().contains("s3cr3t-rotated"), "{}", run.stderr());
        let error = run.fails(1, "internal");
        assert_eq!(error["side_effects"], "partial", "{body}");
        assert_eq!(error["retryable"], false);
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("the old secret no longer works"),
            "{error}"
        );
        assert_eq!(
            error["hint"],
            format!(
                "run `hipc --profile work --api-url {} hub namespace robot rotate --reveal --yes` \
                 to issue a new secret, and if it is refused too, report the refused answer at \
                 `{}`",
                server.url(),
                "https://github.com/thenervelab/hippius-cli/issues"
            )
        );
    }
    // A profile name clap would read as a flag cannot be repeated, so the hint names no
    // command rather than one that fails.
    let body = json!({"robot_secret": "s3cr3t-rotated", "help": "docker login"});
    let server = rotate(Reply::json(201, &body.to_string()));
    let error = Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_PROFILE", "-work")
        .run(&["hub", "namespace", "robot", "rotate", "--reveal", "--yes"])
        .fails(1, "internal");
    assert_eq!(
        error["hint"],
        "report the refused answer at `https://github.com/thenervelab/hippius-cli/issues`"
    );
}

/// `--reveal` comes before the token check and the confirmation: without it the rotate is
/// refused even with `--yes`, and nothing is sent (tenet 9).
#[test]
fn rotate_without_reveal_is_refused_and_sends_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    for args in [
        &["hub", "namespace", "robot", "rotate", "--yes"][..],
        &["hub", "namespace", "robot", "rotate"],
    ] {
        let error = Hipc::against(&home, &server)
            .run(args)
            .fails(2, "reveal_required");
        assert!(
            error["hint"].as_str().unwrap().contains("--reveal"),
            "{error}"
        );
        Hipc::new(&home)
            .env("HIPPIUS_API_URL", server.url())
            .run(args)
            .fails(2, "reveal_required");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn rotate_without_yes_is_refused_and_names_the_rotate() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let args = ["hub", "namespace", "robot", "rotate", "--reveal"];
    let error = Hipc::against(&home, &server)
        .run(&args)
        .fails(2, "confirmation_required");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("rotate the Hub robot secret"),
        "{error}"
    );
    Hipc::new(&home)
        .env("HIPPIUS_API_URL", server.url())
        .run(&args)
        .fails(1, "auth");
    assert!(server.requests().is_empty());
}

#[test]
fn rotate_dry_run_needs_neither_reveal_nor_yes() {
    let server = Server::start(vec![]);
    let base = server.url();
    let doc = Hipc::new(&Home::new())
        .env("HIPPIUS_API_URL", base)
        .run(&["--dry-run", "hub", "namespace", "robot", "rotate"])
        .ok();
    assert_eq!(draft(&doc)["method"], "POST");
    assert_eq!(draft(&doc)["url"], format!("{base}{ROTATE}"));
    assert_eq!(draft(&doc)["body"], Value::Null);
    assert!(server.requests().is_empty());
}

/// A rotate the server refuses with a 404 names the robot read; one with no Hub namespace hints
/// at provisioning.
#[test]
fn rotate_failures_keep_their_codes_and_hints() {
    let server = rotate(Reply::not_found());
    let error = Hipc::against(&Home::new(), &server)
        .run(&["hub", "namespace", "robot", "rotate", "--reveal", "--yes"])
        .fails(1, "not_found");
    assert_eq!(
        error["hint"],
        "run `hipc hub namespace robot get` to see what exists"
    );
    let server = rotate(Reply::json(
        403,
        r#"{"detail":"You must call /api/registry/provision/ first."}"#,
    ));
    let error = Hipc::against(&Home::new(), &server)
        .run(&["hub", "namespace", "robot", "rotate", "--reveal", "--yes"])
        .fails(1, "permission");
    assert!(
        error["hint"]
            .as_str()
            .unwrap()
            .contains("hub namespace create <namespace> --reveal --yes`"),
        "{error}"
    );
}

#[test]
fn update_and_rotate_refuse_a_membership_before_sending() {
    let server = Server::start(vec![]);
    for args in [
        &[
            "hub",
            "namespace",
            "update",
            "--visibility",
            "public",
            "--public",
            "--yes",
        ][..],
        &[
            "hub",
            "namespace",
            "update",
            "--visibility",
            "private",
            "--dry-run",
        ],
        &["hub", "namespace", "robot", "rotate", "--reveal", "--yes"],
        &["hub", "namespace", "robot", "rotate", "--dry-run"],
    ] {
        Hipc::against(&Home::new(), &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(args)
            .fails(2, "account_scope_unsupported");
    }
    assert!(server.requests().is_empty());
}
