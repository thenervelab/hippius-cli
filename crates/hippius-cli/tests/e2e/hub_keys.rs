//! `hipc hub keys`: the namespace's scoped CI keys. A create and a
//! rotate print a one-time secret, so they need `--reveal` before anything is sent; a list
//! never prints one. Every write needs `--yes`, and `/api/registry/` refuses a membership.

use hippius::routes::hub;
use serde_json::{Value, json};

use crate::common::{
    Hipc, Home, MEMBER, Reply, Server, draft, hub_provision_hint, route, route_of,
};

/// A key row as the list answers it, in the shape `hippius-hub registry keys list` reads.
const ROW: &str = concat!(
    r#"{"id":7,"name":"ci","role":"push","login":"robot$acme+ci","#,
    r#""expires_at":null,"last_used_at":null}"#
);

/// A create or rotate answer: the row, the one-time secret, and the login command.
const ISSUED: &str = concat!(
    r#"{"id":7,"name":"ci","role":"push","login":"robot$acme+ci","#,
    r#""secret":"hub_key_secret_value","#,
    r#""docker_login_cmd":"docker login registry.hippius.com -u robot$acme+ci"}"#
);

/// [`ISSUED`] as `hipc` prints it.
fn issued_as_printed() -> Value {
    json!({
        "id": 7,
        "name": "ci",
        "role": "push",
        "login": "robot$acme+ci",
        "secret": "hub_key_secret_value",
        "docker_login_command": "docker login registry.hippius.com -u 'robot$acme+ci' --password-stdin",
    })
}

fn keys_list(reply: Reply) -> Server {
    Server::start(vec![route_of(&hub::KEYS, [], reply)])
}

#[test]
fn list_prints_every_key_with_a_null_secret_and_no_secret_field() {
    let rows = concat!(
        r#"[{"id":7,"name":"ci","role":"push","secret":"leaked_secret_1","#,
        r#""password":"leaked_secret_2","robot":{"token":"leaked_secret_3"},"#,
        r#""robot_secret":"leaked_secret_4"},{"id":8,"name":"pull","role":"read"}]"#
    );
    let server = keys_list(Reply::json(200, rows));
    let run = Hipc::against(&Home::new(), &server).run(&["hub", "keys", "list"]);
    assert_eq!(
        run.ok(),
        json!({
            "items": [
                {"id": 7, "name": "ci", "role": "push", "robot": {}, "secret": null,
                 "docker_login_command": null},
                {"id": 8, "name": "pull", "role": "read", "secret": null,
                 "docker_login_command": null},
            ],
            "next_page_token": null,
        })
    );
    assert!(!run.stdout().contains("leaked_secret"), "{}", run.stdout());
}

/// The list is read whole: swagger lists no page parameter for it, and a DRF page without a
/// `next` link is the list.
#[test]
fn list_reads_a_last_page_and_refuses_a_next_link() {
    let page = format!(r#"{{"count":1,"next":null,"previous":null,"results":[{ROW}]}}"#);
    let server = keys_list(Reply::json(200, &page));
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "keys", "list"])
        .ok();
    assert_eq!(doc["count"], 1);
    assert_eq!(doc["items"][0]["login"], "robot$acme+ci");
    assert_eq!(doc["items"][0]["secret"], Value::Null);

    let paged =
        format!(r#"{{"count":2,"next":"http://x/api/registry/keys/?page=2","results":[{ROW}]}}"#);
    let server = keys_list(Reply::json(200, &paged));
    Hipc::against(&Home::new(), &server)
        .run(&["hub", "keys", "list"])
        .fails(1, "internal");
}

/// A key's own `name` and `login` are not read as a login command, so a key named like a
/// flag, made in the console or the Hub client, still lists.
#[test]
fn list_keeps_a_key_whose_name_reads_as_a_flag() {
    let rows = r#"[{"id":7,"name":"-push","login":"robot$acme+-push","role":"push"}]"#;
    let server = keys_list(Reply::json(200, rows));
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "keys", "list"])
        .ok();
    assert_eq!(doc["items"][0]["name"], "-push");
    assert_eq!(doc["items"][0]["secret"], Value::Null);
}

/// A row's `docker login` command under a command key is dropped whatever it holds, and the
/// row gets the command `hipc` builds for its `login`, in JSON and in text.
#[test]
fn list_drops_every_command_field_the_server_sends() {
    let built = "docker login registry.hippius.com -u 'robot$acme+ci' --password-stdin";
    for command in [
        json!("docker login registry.hippius.com -u robot -p leaked_secret"),
        json!("echo leaked_secret | docker login registry.hippius.com -u robot --password-stdin"),
        json!("DOCKER_PASSWORD=leaked_secret docker login registry.hippius.com -u robot"),
        json!(["docker", "login", "-u", "robot", "-pleaked_secret"]),
        json!("bGVha2VkX3NlY3JldA=="),
    ] {
        let rows = json!([{
            "id": 7,
            "login": "robot$acme+ci",
            "docker_login_cmd": command,
            "login_cmd": command,
            "command": command,
        }]);
        let server = keys_list(Reply::json(200, &rows.to_string()));
        let run = Hipc::against(&Home::new(), &server).run(&["hub", "keys", "list"]);
        assert!(!run.stdout().contains("leaked_secret"), "{}", run.stdout());
        assert_eq!(
            run.ok()["items"][0],
            json!({
                "id": 7,
                "login": "robot$acme+ci",
                "secret": null,
                "docker_login_command": built,
            }),
            "{command}"
        );
        let run =
            Hipc::against(&Home::new(), &server).run(&["--output", "text", "hub", "keys", "list"]);
        assert!(!run.stdout().contains("leaked_secret"), "{}", run.stdout());
        assert!(!run.stderr().contains("leaked_secret"), "{}", run.stderr());
        assert!(run.stdout().contains(built), "{}", run.stdout());
    }
}

/// A `docker login` command anywhere else in a row may pass the secret in a way no check
/// can list, so the whole list is refused and nothing is printed.
#[test]
fn list_refuses_a_row_with_any_other_docker_login_command() {
    for row in [
        json!({"id": 7, "argv": ["docker", "login", "-u", "robot", "-p", "leaked_secret"]}),
        json!({"id": 7, "argv": ["echo", "leaked_secret", "|", "docker", "login", "-u", "r"]}),
        json!({"id": 7, "argv": ["DOCKER_PASSWORD=leaked_secret", "docker", "login"]}),
        json!({"id": 7, "help": "docker\u{200D}login --password=leaked_secret"}),
        json!({"id": 7, "name": "echo leaked_secret | docker login"}),
        // As a shell reads them: quotes, backslashes, a continued line, a flag between the
        // words, a substitution, and a key.
        json!({"id": 7, "help": "\"docker\" \"login\" -u u -p leaked_secret"}),
        json!({"id": 7, "help": "docker \\\nlogin -p leaked_secret"}),
        json!({"id": 7, "help": "docke\\r log\\in -p leaked_secret"}),
        json!({"id": 7, "help": "docker $'login' -p leaked_secret"}),
        json!({"id": 7, "help": "/usr/bin/docker -D login -p leaked_secret"}),
        json!({"id": 7, "help": "$(printf docker) login -p leaked_secret"}),
        json!({"id": 7, "docker login -u r -p leaked_secret": "ok"}),
    ] {
        let rows = json!([row]).to_string();
        let server = keys_list(Reply::json(200, &rows));
        let run = Hipc::against(&Home::new(), &server).run(&["hub", "keys", "list"]);
        assert_eq!(run.stdout(), "");
        let error = run.fails(1, "internal");
        assert!(!run.stderr().contains("leaked_secret"), "{}", run.stderr());
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("holds a `docker login` command"),
            "{error}"
        );
    }
    // A password key in any case is dropped, and a word that looks like a flag prints.
    let rows = json!([{
        "id": 7,
        "env": {"Docker_Password": "leaked_secret"},
        "Token": "leaked_secret",
        "description": "-path -project",
    }]);
    let server = keys_list(Reply::json(200, &rows.to_string()));
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "keys", "list"])
        .ok();
    assert_eq!(
        doc["items"][0],
        json!({
            "id": 7,
            "env": {},
            "description": "-path -project",
            "secret": null,
            "docker_login_command": null,
        })
    );
}
#[test]
fn list_without_a_project_hints_at_namespace_create() {
    let server = keys_list(Reply::json(
        403,
        r#"{"detail":"You must call /api/registry/provision/ first."}"#,
    ));
    let error = Hipc::against(&Home::new(), &server)
        .run(&["hub", "keys", "list"])
        .fails(1, "permission");
    assert_eq!(error["hint"], hub_provision_hint(&server));
}

#[test]
fn create_sends_the_name_and_role_and_prints_the_secret_once() {
    let server = Server::start(vec![route_of(
        &hub::KEY_CREATE,
        [],
        Reply::json(201, ISSUED),
    )]);
    let run = Hipc::against(&Home::new(), &server).run(&[
        "hub", "keys", "create", "--name", "ci", "--role", "push", "--reveal", "--yes",
    ]);
    // The secret goes on stdout and nowhere else (tenet 9): `ok` checks stderr is empty.
    // The server's login command is replaced by the one `hipc` builds, which reads the
    // secret from stdin.
    assert_eq!(run.ok(), issued_as_printed());
    assert_eq!(
        server.only_request().json(),
        json!({"name": "ci", "role": "push"})
    );
}

#[test]
fn create_sends_the_expiry_in_days_as_a_number() {
    let server = Server::start(vec![route_of(
        &hub::KEY_CREATE,
        [],
        Reply::json(201, ISSUED),
    )]);
    Hipc::against(&Home::new(), &server)
        .run(&[
            "hub",
            "keys",
            "create",
            "--name",
            "ci",
            "--role",
            "read",
            "--expires-in",
            "30",
            "--reveal",
            "--yes",
        ])
        .ok();
    assert_eq!(
        server.only_request().json(),
        json!({"name": "ci", "role": "read", "expires_days": 30})
    );
}

/// Each `--role` is sent as `hippius-hub` spells it.
#[test]
fn every_role_is_sent_as_the_hub_client_spells_it() {
    let home = Home::new();
    for role in ["read", "push", "push-delete", "admin"] {
        let doc = Hipc::new(&home)
            .run(&[
                "--dry-run",
                "hub",
                "keys",
                "create",
                "--name",
                "ci",
                "--role",
                role,
            ])
            .ok();
        assert_eq!(draft(&doc)["body"], json!({"name": "ci", "role": role}));
    }
}

/// A role, an expiry, a name, or an id the API cannot take is usage before the token check,
/// and nothing is sent.
#[test]
fn bad_flags_are_usage_and_send_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let create = ["hub", "keys", "create", "--reveal", "--yes"];
    for (extra, words) in [
        (
            &["--name", "ci", "--role", "Write"][..],
            "a key role is a code",
        ),
        (&["--name", "ci"], "--role"),
        (&["--role", "read"], "--name"),
        (
            &["--name", "ci", "--role", "read", "--expires-in", "0"],
            "--expires-in",
        ),
        (
            &["--name", "ci", "--role", "read", "--expires-in=-1"],
            "--expires-in",
        ),
        (&["--name", "", "--role", "read"], "a key name is"),
        (&["--name", "ci job", "--role", "read"], "a key name is"),
        (&["--name", "ci\tjob", "--role", "read"], "a key name is"),
        (&["--name", "ci\u{1}job", "--role", "read"], "a key name is"),
        (&["--name=-push", "--role", "read"], "a key name is"),
    ] {
        let args = [&create[..], extra].concat();
        let error = Hipc::tokenless(&home, &server).run(&args).fails(2, "usage");
        assert!(
            error["message"].as_str().unwrap().contains(words),
            "{args:?}: {error}"
        );
    }
    for args in [
        &["hub", "keys", "rotate", "x", "--reveal", "--yes"][..],
        &["hub", "keys", "delete", "../7", "--yes"],
    ] {
        let error = Hipc::tokenless(&home, &server).run(args).fails(2, "usage");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("a key id is a whole number, from `hipc hub keys list`"),
            "{error}"
        );
    }
    assert!(server.requests().is_empty());
}

/// `--reveal` comes before the token check and the confirmation: without it a create or a
/// rotate is refused even with `--yes`, and nothing is sent (tenet 9).
#[test]
fn create_and_rotate_without_reveal_are_refused_and_send_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    for args in [
        &[
            "hub", "keys", "create", "--name", "ci", "--role", "read", "--yes",
        ][..],
        &["hub", "keys", "create", "--name", "ci", "--role", "read"],
        &["hub", "keys", "rotate", "7", "--yes"],
    ] {
        let error = Hipc::against(&home, &server)
            .run(args)
            .fails(2, "reveal_required");
        assert!(
            error["hint"].as_str().unwrap().contains("--reveal"),
            "{error}"
        );
        // Before the token check too.
        Hipc::tokenless(&home, &server)
            .run(args)
            .fails(2, "reveal_required");
    }
    assert!(server.requests().is_empty());
}

/// Without `--yes` on a pipe each write is refused with the action it names, and nothing is
/// sent. The token is checked before the question, so a missing one is `auth` (tenet 5).
#[test]
fn writes_without_yes_are_refused_and_name_the_key() {
    let server = Server::start(vec![]);
    let home = Home::new();
    for (args, action) in [
        (
            &[
                "hub", "keys", "create", "--name", "ci", "--role", "push", "--reveal",
            ][..],
            "create the Hub key `ci` with the role `push`",
        ),
        (
            &["hub", "keys", "rotate", "7", "--reveal"],
            "rotate the secret of the Hub key `7`",
        ),
        (&["hub", "keys", "delete", "7"], "delete the Hub key `7`"),
    ] {
        let error = Hipc::against(&home, &server)
            .run(args)
            .fails(2, "confirmation_required");
        assert_eq!(error["hint"], "pass `--yes` to confirm");
        assert!(
            error["message"].as_str().unwrap().contains(action),
            "{error}"
        );
        Hipc::tokenless(&home, &server).run(args).fails(1, "auth");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn rotate_posts_no_body_and_prints_the_new_secret() {
    let server = Server::start(vec![route_of(
        &hub::KEY_ROTATE,
        [&7],
        Reply::json(201, ISSUED),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "keys", "rotate", "7", "--reveal", "--yes"])
        .ok();
    assert_eq!(doc["secret"], "hub_key_secret_value");
    let request = server.only_request();
    assert_eq!(request.method, "POST");
    assert!(request.body.is_empty(), "{:?}", request.body);
}

/// A create or rotate answer that passes the secret in a `docker login` command, as
/// `-p SECRET` would, prints none of it: the secret could reach a shell history. The write
/// is done, so the error is `partial`, and its hint names the rotate that issues a new
/// secret.
#[test]
fn create_and_rotate_drop_the_server_login_command_and_refuse_any_other() {
    let cases = [
        (
            "/api/registry/keys/",
            vec![
                "hub", "keys", "create", "--name", "ci", "--role", "push", "--reveal", "--yes",
            ],
            "the Hub key create answer",
            "the key was created",
        ),
        (
            "/api/registry/keys/7/rotate/",
            vec!["hub", "keys", "rotate", "7", "--reveal", "--yes"],
            "the Hub key rotate answer",
            "the old secret no longer works",
        ),
    ];
    for (path, args, what, done) in cases {
        // Each server key that holds a login command is dropped, whatever it holds.
        let answer = json!({
            "id": 7,
            "login": "robot$acme+ci",
            "secret": "hub_key_secret_value",
            "docker_login_cmd": "docker login registry.hippius.com -u robot -p hub_key_secret_value",
            "docker_login": "docker login -p hub_key_secret_value",
        });
        let server = Server::start(vec![route(
            "POST",
            path,
            Reply::json(201, &answer.to_string()),
        )]);
        let doc = Hipc::against(&Home::new(), &server).run(&args).ok();
        let mut expected = issued_as_printed();
        let fields = expected.as_object_mut().unwrap();
        fields.shift_remove("name");
        fields.shift_remove("role");
        assert_eq!(doc, expected, "{path}");
        // Elsewhere in the answer, the command is refused whole.
        let answer = json!({
            "id": 7,
            "login": "robot$acme+ci",
            "secret": "hub_key_secret_value",
            "help": "docker login -u robot -p hub_key_secret_value",
        });
        let server = Server::start(vec![route(
            "POST",
            path,
            Reply::json(201, &answer.to_string()),
        )]);
        let run = Hipc::against(&Home::new(), &server).run(&args);
        assert_eq!(run.stdout(), "");
        assert!(
            !run.stderr().contains("hub_key_secret_value"),
            "{}",
            run.stderr()
        );
        let error = run.fails(1, "internal");
        assert_eq!(
            error["message"],
            format!(
                "{what} holds a `docker login` command, which may pass the secret where it could \
                 be copied into a shell history, so `hipc` printed none of it; {done}"
            )
        );
        assert_eq!(error["side_effects"], "partial");
        assert_eq!(
            error["hint"],
            format!(
                "run `hipc --api-url {} hub keys rotate 7 --reveal --yes` to issue a new secret, \
                 and if it is refused too, report the refused answer at \
                 `https://github.com/thenervelab/hippius-cli/issues`",
                server.url()
            )
        );
    }
    // A create answer without an id names no rotate.
    let answer = json!({"secret": "s", "help": "docker login -p s"});
    let server = Server::start(vec![route_of(
        &hub::KEY_CREATE,
        [],
        Reply::json(201, &answer.to_string()),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&[
            "hub", "keys", "create", "--name", "ci", "--role", "push", "--reveal", "--yes",
        ])
        .fails(1, "internal");
    assert_eq!(
        error["hint"],
        "report the refused answer at `https://github.com/thenervelab/hippius-cli/issues`"
    );
}

/// A login that starts with `-` would be read by `docker login -u` as a flag, so no
/// command is built for it: a read prints nothing, and a write that issued a secret is
/// `partial`.
#[test]
fn a_login_that_starts_with_a_dash_is_refused() {
    let row = r#"[{"id":7,"login":"--password=x"}]"#;
    let server = keys_list(Reply::json(200, row));
    let run = Hipc::against(&Home::new(), &server).run(&["hub", "keys", "list"]);
    assert_eq!(run.stdout(), "");
    assert!(!run.stderr().contains("password=x"), "{}", run.stderr());
    let error = run.fails(1, "internal");
    assert_eq!(
        error["message"],
        "the API answered a login that starts with `-`, which `docker login -u` would read as a \
         flag, so `hipc` printed none of the answer"
    );
    assert_eq!(error["side_effects"], "none");
    let server = Server::start(vec![route_of(
        &hub::KEY_ROTATE,
        [&7],
        Reply::json(
            201,
            r#"{"id":7,"login":"-x","secret":"hub_key_secret_value"}"#,
        ),
    )]);
    let run = Hipc::against(&Home::new(), &server)
        .run(&["hub", "keys", "rotate", "7", "--reveal", "--yes"]);
    assert_eq!(run.stdout(), "");
    let error = run.fails(1, "internal");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .ends_with("printed none of the answer; the old secret no longer works"),
        "{error}"
    );
    assert_eq!(error["side_effects"], "partial");
}

#[test]
fn a_missing_key_names_the_list() {
    let server = Server::start(vec![
        route_of(&hub::KEY_ROTATE, [&8], Reply::not_found()),
        route_of(&hub::KEY_DELETE, [&8], Reply::not_found()),
    ]);
    let home = Home::new();
    for args in [
        &["hub", "keys", "rotate", "8", "--reveal", "--yes"][..],
        &["hub", "keys", "delete", "8", "--yes"],
    ] {
        let error = Hipc::against(&home, &server)
            .run(args)
            .fails(1, "not_found");
        assert_eq!(error["hint"], "run `hipc hub keys list` to see what exists");
    }
}

#[test]
fn delete_answers_the_id_when_the_server_sends_no_body() {
    let server = Server::start(vec![route_of(
        &hub::KEY_DELETE,
        [&7],
        Reply::bytes(204, "application/json", b""),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "keys", "delete", "7", "--yes"])
        .ok();
    assert_eq!(doc, json!({"id": 7, "deleted": true}));
    assert_eq!(server.only_request().method, "DELETE");
}

/// A delete answers no secret; one the server sent anyway is not printed (tenet 9).
#[test]
fn delete_prints_a_body_without_its_secret() {
    let server = Server::start(vec![route_of(
        &hub::KEY_DELETE,
        [&7],
        Reply::json(200, r#"{"id":7,"revoked":true,"secret":"leaked_secret"}"#),
    )]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["hub", "keys", "delete", "7", "--yes"])
        .ok();
    assert_eq!(doc, json!({"id": 7, "revoked": true}));
    // The list's filter applies: a password key in any case and a command key are dropped,
    // and a body with any other `docker login` command gives way to `hipc`'s own document,
    // since the key is deleted either way.
    for (body, printed) in [
        (
            json!({"id": 7, "Password": "leaked_secret", "docker_login_cmd": "x"}),
            json!({"id": 7}),
        ),
        (
            json!({"id": 7, "argv": ["docker", "login", "-p", "leaked_secret"]}),
            json!({"id": 7, "deleted": true}),
        ),
    ] {
        let server = Server::start(vec![route_of(
            &hub::KEY_DELETE,
            [&7],
            Reply::json(200, &body.to_string()),
        )]);
        let run =
            Hipc::against(&Home::new(), &server).run(&["hub", "keys", "delete", "7", "--yes"]);
        assert!(!run.stderr().contains("leaked_secret"), "{}", run.stderr());
        assert_eq!(run.ok(), printed);
    }
}

/// A dry run needs no `--yes`, no `--reveal`, and no token, lists its one request, and
/// sends nothing.
#[test]
fn dry_runs_list_one_request_and_send_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let base = server.url();
    for (args, method, path, body) in [
        (
            &["hub", "keys", "list"][..],
            "GET",
            "/api/registry/keys/",
            Value::Null,
        ),
        (
            &[
                "hub",
                "keys",
                "create",
                "--name",
                "ci",
                "--role",
                "push-delete",
                "--expires-in",
                "7",
            ],
            "POST",
            "/api/registry/keys/",
            json!({"name": "ci", "role": "push-delete", "expires_days": 7}),
        ),
        (
            &["hub", "keys", "rotate", "7"],
            "POST",
            "/api/registry/keys/7/rotate/",
            Value::Null,
        ),
        (
            &["hub", "keys", "delete", "7"],
            "DELETE",
            "/api/registry/keys/7/",
            Value::Null,
        ),
    ] {
        let doc = Hipc::new(&home)
            .env("HIPPIUS_API_URL", base)
            .run(&[&["--dry-run"][..], args].concat())
            .ok();
        assert_eq!(doc["dry_run"], true, "{args:?}");
        assert_eq!(draft(&doc)["method"], method, "{args:?}");
        assert_eq!(draft(&doc)["url"], format!("{base}{path}"), "{args:?}");
        assert_eq!(draft(&doc)["body"], body, "{args:?}");
        let requests = doc["requests"].as_array().unwrap();
        assert_eq!(requests.len(), 1, "{args:?}");
        assert_eq!(requests[0]["url"], draft(&doc)["url"]);
        assert_eq!(requests[0]["body"], draft(&doc)["body"]);
    }
    assert!(server.requests().is_empty());
}

#[test]
fn every_key_command_refuses_a_membership_before_sending() {
    let server = Server::start(vec![]);
    for args in [
        &["hub", "keys", "list"][..],
        &["hub", "keys", "list", "--dry-run"],
        &[
            "hub", "keys", "create", "--name", "ci", "--role", "read", "--reveal", "--yes",
        ],
        &[
            "hub",
            "keys",
            "create",
            "--name",
            "ci",
            "--role",
            "read",
            "--dry-run",
        ],
        &["hub", "keys", "rotate", "7", "--reveal", "--yes"],
        &["hub", "keys", "delete", "7", "--yes"],
        &["hub", "keys", "delete", "7", "--dry-run"],
    ] {
        Hipc::against(&Home::new(), &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(args)
            .fails(2, "account_scope_unsupported");
    }
    assert!(server.requests().is_empty());
}

/// `get` prints the key as the list prints a row: a preview with `secret: null`, every
/// secret-like key and server command field dropped, and the command `hipc` builds.
#[test]
fn get_prints_a_preview_with_a_null_secret() {
    let body = concat!(
        r#"{"id":7,"name":"ci","role":"push","login":"robot$acme+ci","secret":"leaked_secret","#,
        r#""docker_login_cmd":"docker login -u robot$acme+ci -p leaked_secret","#,
        r#""robot":{"password":"leaked_secret"}}"#
    );
    let server = Server::start(vec![route_of(&hub::KEY, [&7], Reply::json(200, body))]);
    let run = Hipc::against(&Home::new(), &server).run(&["hub", "keys", "get", "7"]);
    assert_eq!(
        run.ok(),
        json!({
            "id": 7,
            "name": "ci",
            "role": "push",
            "login": "robot$acme+ci",
            "robot": {},
            "secret": null,
            "docker_login_command":
                "docker login registry.hippius.com -u 'robot$acme+ci' --password-stdin",
        })
    );
    assert!(!run.stdout().contains("leaked_secret"), "{}", run.stdout());
    assert_eq!(server.only_request().method, "GET");
}

/// A `docker login` command outside the dropped keys refuses the whole answer, naming the key.
#[test]
fn get_refuses_an_answer_with_any_other_docker_login_command() {
    let body = r#"{"id":7,"help":"docker login -u r -p leaked_secret"}"#;
    let server = Server::start(vec![route_of(&hub::KEY, [&7], Reply::json(200, body))]);
    let run = Hipc::against(&Home::new(), &server).run(&["hub", "keys", "get", "7"]);
    assert!(!run.stderr().contains("leaked_secret"), "{}", run.stderr());
    let error = run.fails(1, "internal");
    assert_eq!(
        error["message"],
        "the Hub key `7` holds a `docker login` command, which may pass the secret, and `hipc` \
         does not print a key secret"
    );
}

#[test]
fn get_of_a_missing_key_names_the_list_and_bad_ids_are_usage() {
    let server = Server::start(vec![route_of(&hub::KEY, [&8], Reply::not_found())]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["hub", "keys", "get", "8"])
        .fails(1, "not_found");
    assert_eq!(error["hint"], "run `hipc hub keys list` to see what exists");
    for id in ["x", "-1", "7/rotate", ""] {
        Hipc::against(&Home::new(), &server)
            .run(&["hub", "keys", "get", id])
            .fails(2, "usage");
    }
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn get_dry_runs_and_refuses_a_membership() {
    let server = Server::start(vec![]);
    let base = server.url();
    let doc = Hipc::new(&Home::new())
        .env("HIPPIUS_API_URL", base)
        .run(&["--dry-run", "hub", "keys", "get", "7"])
        .ok();
    assert_eq!(draft(&doc)["method"], "GET");
    assert_eq!(draft(&doc)["url"], format!("{base}/api/registry/keys/7/"));
    Hipc::against(&Home::new(), &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&["hub", "keys", "get", "7"])
        .fails(2, "account_scope_unsupported");
    assert!(server.requests().is_empty());
}
