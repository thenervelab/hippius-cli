//! `--secret-file`: the S3 token and Hub key creates and rotates, the Hub namespace create, and
//! the Hub robot rotate write their one-time secret to a new private file instead of stdout
//! (tenet 9), so it never reaches a log or an agent's transcript.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::common::{Hipc, Home, Reply, Run, Server, route};

const S3_SECRET: &str = "s3cr3t-QZ7-only-once";
const KEY_SECRET: &str = "hub_key_secret_value";
const ROBOT_SECRET: &str = "s3cr3t-rotated";
const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

fn s3_created() -> String {
    json!({
        "accessKeyId": "hip_NEW1",
        "secret": S3_SECRET,
        "id": ID,
        "name": "ci",
        "createdAt": "t",
    })
    .to_string()
}

const KEY_ISSUED: &str = concat!(
    r#"{"id":7,"name":"ci","role":"push","login":"robot$acme+ci","#,
    r#""secret":"hub_key_secret_value"}"#
);

const ROBOT_ROTATED: &str = r#"{"robot_login":"robot$acme","robot_secret":"s3cr3t-rotated"}"#;

const PROVISIONED: &str =
    r#"{"namespace":"acme","robot_name":"robot$acme+push","robot_secret":"s3cr3t-rotated"}"#;

/// One issuing command: its route, its answer, its argv before `--secret-file`, its secret,
/// and the field the document names the file in.
struct Issuer {
    path: String,
    answer: String,
    argv: Vec<&'static str>,
    secret: &'static str,
    field: &'static str,
}

fn issuers() -> Vec<Issuer> {
    vec![
        Issuer {
            path: "/api/objectstore/sub-tokens/".to_owned(),
            answer: s3_created(),
            argv: vec!["s3", "tokens", "create", "--kind", "sub", "--bucket", "b"],
            secret: S3_SECRET,
            field: "secret",
        },
        Issuer {
            path: format!("/api/objectstore/master-tokens/{ID}/rotate/"),
            answer: s3_created(),
            argv: vec!["s3", "tokens", "rotate", "--kind", "master", ID],
            secret: S3_SECRET,
            field: "secret",
        },
        Issuer {
            path: "/api/registry/keys/".to_owned(),
            answer: KEY_ISSUED.to_owned(),
            argv: vec!["hub", "keys", "create", "--name", "ci", "--role", "push"],
            secret: KEY_SECRET,
            field: "secret",
        },
        Issuer {
            path: "/api/registry/keys/7/rotate/".to_owned(),
            answer: KEY_ISSUED.to_owned(),
            argv: vec!["hub", "keys", "rotate", "7"],
            secret: KEY_SECRET,
            field: "secret",
        },
        Issuer {
            path: "/api/registry/robot/rotate/".to_owned(),
            answer: ROBOT_ROTATED.to_owned(),
            argv: vec!["hub", "namespace", "robot", "rotate"],
            secret: ROBOT_SECRET,
            field: "robot_secret",
        },
        Issuer {
            path: "/api/registry/provision/".to_owned(),
            answer: PROVISIONED.to_owned(),
            argv: vec!["hub", "namespace", "create", "acme"],
            secret: ROBOT_SECRET,
            field: "robot_secret",
        },
    ]
}

fn server_for(issuer: &Issuer, reply: Reply) -> Server {
    Server::start(vec![route("POST", &issuer.path, reply)])
}

/// `<argv> --secret-file <file> <extra>` in `home`.
fn run_into(home: &Home, server: &Server, issuer: &Issuer, file: &Path, extra: &[&str]) -> Run {
    let file = file.to_str().expect("the test path is UTF-8");
    let mut argv = issuer.argv.clone();
    argv.extend(["--secret-file", file]);
    argv.extend(extra);
    Hipc::against(home, server).run(&argv)
}

fn no_secret_in(run: &Run, secret: &str) {
    assert!(!run.stdout().contains(secret), "{}", run.stdout());
    assert!(!run.stderr().contains(secret), "{}", run.stderr());
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path)
        .expect("the file exists")
        .permissions()
        .mode()
        & 0o777
}

/// Each command writes the secret alone, with no newline, to a new file of mode 0600, names
/// the file in its place, and prints the secret nowhere. Naming the file is the reveal, so
/// `--reveal` is not needed beside it, and is still taken.
#[test]
fn each_issuer_writes_the_secret_only_to_the_file() {
    for issuer in issuers() {
        let what = issuer.argv.join(" ");
        let server = server_for(&issuer, Reply::json(201, &issuer.answer));
        let home = Home::new();
        let file = home.path().join("secret.txt");
        let run = run_into(&home, &server, &issuer, &file, &["--yes"]);
        let doc = run.ok();
        no_secret_in(&run, issuer.secret);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            issuer.secret,
            "{what}"
        );
        assert_eq!(mode(&file), 0o600, "{what}");
        assert!(doc.get(issuer.field).is_none(), "{what}: {doc}");
        assert_eq!(
            doc[format!("{}_file", issuer.field)],
            file.to_str().unwrap(),
            "{what}"
        );
        assert_eq!(server.requests().len(), 1, "{what}");

        // With `--reveal` too, as before the file alone counted, the same file is written.
        let server = server_for(&issuer, Reply::json(201, &issuer.answer));
        let both = home.path().join("both.txt");
        let run = run_into(&home, &server, &issuer, &both, &["--reveal", "--yes"]);
        let again = run.ok();
        no_secret_in(&run, issuer.secret);
        assert_eq!(
            std::fs::read_to_string(&both).unwrap(),
            issuer.secret,
            "{what}"
        );
        assert!(again.get(issuer.field).is_none(), "{what}: {again}");

        // Without either, nothing is sent: the secret would go to stdout.
        let server = server_for(&issuer, Reply::json(201, &issuer.answer));
        let mut argv = issuer.argv.clone();
        argv.push("--yes");
        let error = Hipc::against(&home, &server)
            .run(&argv)
            .fails(2, "reveal_required");
        assert!(
            error["hint"].as_str().expect("a hint").contains("--reveal"),
            "{what}: {error}"
        );
        assert!(server.requests().is_empty(), "{what}");
    }
}

/// The JSON documents, whole: the rest of the answer is printed as without the flag.
#[test]
fn the_documents_name_the_file_in_place_of_the_secret() {
    let issuers = issuers();
    let home = Home::new();
    let file = |name: &str| home.path().join(name);
    let s3 = &issuers[0];
    let server = server_for(s3, Reply::json(201, &s3.answer));
    let doc = run_into(&home, &server, s3, &file("s3"), &["--reveal", "--yes"]).ok();
    assert_eq!(
        doc,
        json!({
            "kind": "sub",
            "access_key_id": "hip_NEW1",
            "id": ID,
            "name": "ci",
            "created_at": "t",
            "secret_file": file("s3").to_str().unwrap(),
        })
    );
    let key = &issuers[2];
    let server = server_for(key, Reply::json(201, &key.answer));
    let doc = run_into(&home, &server, key, &file("key"), &["--reveal", "--yes"]).ok();
    assert_eq!(
        doc,
        json!({
            "id": 7,
            "name": "ci",
            "role": "push",
            "login": "robot$acme+ci",
            "docker_login_command":
                "docker login registry.hippius.com -u 'robot$acme+ci' --password-stdin",
            "secret_file": file("key").to_str().unwrap(),
        })
    );
}

/// Text mode names the file, and for an S3 token the `hipc s3 configure` that reads it.
#[test]
fn text_mode_notes_name_the_file() {
    let issuers = issuers();
    let home = Home::new();
    let file = home.path().join("s3.secret");
    let s3 = &issuers[0];
    let server = server_for(s3, Reply::json(201, &s3.answer));
    let run = run_into(
        &home,
        &server,
        s3,
        &file,
        &["--reveal", "--yes", "--output", "text"],
    );
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    no_secret_in(&run, S3_SECRET);
    let shown = file.display();
    assert!(
        run.stderr().contains(&format!(
            "note: the secret is in `{shown}`; run `hipc --api-url {} s3 configure \
             --access-key-id hip_NEW1 --secret-key-file {shown}` to write an AWS CLI profile \
             for it, then delete the file",
            server.url()
        )),
        "{}",
        run.stderr()
    );
    for (n, issuer) in issuers[2..].iter().enumerate() {
        let what = issuer.argv.join(" ");
        let server = server_for(issuer, Reply::json(201, &issuer.answer));
        let file = home.path().join(format!("{n}.secret"));
        let run = run_into(
            &home,
            &server,
            issuer,
            &file,
            &["--reveal", "--yes", "--output", "text"],
        );
        assert_eq!(run.code(), Some(0), "{what}: {}", run.stderr());
        no_secret_in(&run, issuer.secret);
        assert_eq!(
            run.stderr(),
            format!(
                "note: the secret is in `{}`, and not printed: give that file on stdin to the \
                 document's `docker_login_command`, and delete it once the secret is stored \
                 where it is used\n",
                file.display()
            ),
            "{what}"
        );
    }
}

/// The path is checked before anything is sent, in a dry run too: `-`, a directory, a path
/// that exists (a dangling symlink included), one in a missing directory, and one in a
/// directory others can write are usage, and nothing is written.
#[test]
fn a_bad_secret_file_is_usage_with_zero_requests() {
    let home = Home::new();
    let taken = home.file("taken", b"old");
    let dangling = home.path().join("dangling");
    std::os::unix::fs::symlink(home.path().join("nowhere"), &dangling).unwrap();
    let open = home.path().join("open");
    std::fs::create_dir(&open).unwrap();
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
    let bad: Vec<(PathBuf, &str)> = vec![
        (PathBuf::from("-"), "never to stdout"),
        (
            PathBuf::from(format!("{}/", home.path().display())),
            "names a directory",
        ),
        (taken.clone(), "already at that path"),
        (dangling, "already at that path"),
        (home.path().join("missing/secret"), "does not exist"),
        (open.join("secret"), "can be written by other users"),
    ];
    for issuer in issuers() {
        let server = Server::start(vec![]);
        for (path, words) in &bad {
            for extra in [&["--reveal", "--yes"][..], &["--dry-run"]] {
                let error = run_into(&home, &server, &issuer, path, extra).fails(2, "usage");
                let message = error["message"].as_str().unwrap();
                assert!(message.contains(words), "{}: {message}", path.display());
                assert!(message.contains("--secret-file"), "{message}");
            }
        }
        assert!(server.requests().is_empty());
    }
    assert_eq!(std::fs::read(&taken).unwrap(), b"old");
    assert!(std::fs::read_dir(&open).unwrap().next().is_none());
}

/// A dry run writes no file.
#[test]
fn a_dry_run_writes_no_file() {
    for issuer in issuers() {
        let server = Server::start(vec![]);
        let home = Home::new();
        let file = home.path().join("secret.txt");
        run_into(&home, &server, &issuer, &file, &["--dry-run"]).ok();
        assert!(std::fs::symlink_metadata(&file).is_err());
        assert!(server.requests().is_empty());
    }
}

/// A file made at the path after the check is not overwritten: the secret was issued, so
/// the whole answer goes on stdout, and the error is `internal` and `partial`.
#[test]
fn a_file_made_during_the_call_keeps_the_answer_on_stdout() {
    for issuer in issuers() {
        let what = issuer.argv.join(" ");
        let home = Home::new();
        let file = home.path().join("secret.txt");
        let at = file.clone();
        let server = server_for(
            &issuer,
            Reply::json(201, &issuer.answer)
                .before(move || drop(std::fs::write(&at, "someone else's"))),
        );
        let run = run_into(&home, &server, &issuer, &file, &["--reveal", "--yes"]);
        assert_eq!(run.code(), Some(1), "{what}: {}", run.stderr());
        assert_eq!(run.json()[issuer.field], issuer.secret, "{what}");
        assert!(!run.stderr().contains(issuer.secret), "{what}");
        let error: Value = serde_json::from_str(run.stderr()).expect("one JSON object");
        let error = &error["error"];
        assert_eq!(error["code"], "internal", "{what}");
        assert_eq!(error["side_effects"], "partial", "{what}");
        assert_eq!(error["retryable"], false, "{what}");
        let message = error["message"].as_str().unwrap();
        assert!(message.contains("File exists"), "{what}: {message}");
        assert!(
            message.ends_with("the secret is in the answer on stdout"),
            "{message}"
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "someone else's");
    }
}

/// An answer whose secret cannot be moved whole, two different secrets or one below the top
/// level, writes no file and prints the whole answer; a document printed without its secret
/// holds no other.
#[test]
fn a_secret_that_cannot_be_moved_whole_keeps_the_answer_on_stdout() {
    let rotate = issuers().remove(4);
    for (answer, words) in [
        (
            json!({"robot_login": "robot$acme", "robot_secret": "a-secret", "secret": "b-secret"}),
            "more than one secret",
        ),
        (
            json!({"robot_login": "robot$acme", "robot_secret": "a-secret", "robot": {"token": "b-secret"}}),
            "cannot move",
        ),
        (
            json!({"robot_login": "robot$acme", "robot_secret": 42}),
            "cannot move",
        ),
    ] {
        let server = server_for(&rotate, Reply::json(200, &answer.to_string()));
        let home = Home::new();
        let file = home.path().join("secret.txt");
        let run = run_into(&home, &server, &rotate, &file, &["--reveal", "--yes"]);
        assert_eq!(run.code(), Some(1), "{answer}: {}", run.stderr());
        // The answer as sent, since the secret is moved before `hipc` adds its login command.
        assert_eq!(run.json(), answer);
        let error = run.error();
        assert_eq!(error["code"], "internal");
        assert_eq!(error["side_effects"], "partial");
        let message = error["message"].as_str().unwrap();
        assert!(message.contains(words), "{message}");
        assert!(std::fs::symlink_metadata(&file).is_err(), "{answer}");
    }
    // Two fields with the one secret are both named by the file.
    let answer =
        json!({"robot_login": "r", "robot_secret": "same", "secret": "same", "password": null});
    let server = server_for(&rotate, Reply::json(200, &answer.to_string()));
    let home = Home::new();
    let file = home.path().join("secret.txt");
    let doc = run_into(&home, &server, &rotate, &file, &["--reveal", "--yes"]).ok();
    assert_eq!(doc["robot_secret_file"], file.to_str().unwrap());
    assert_eq!(doc["secret_file"], file.to_str().unwrap());
    assert_eq!(doc["password"], Value::Null);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "same");
}

/// A namespace create that issues no secret, since the namespace exists, writes no file and
/// says so in text mode.
#[test]
fn a_create_that_issues_no_secret_writes_no_file() {
    let create = issuers().remove(5);
    let server = server_for(
        &create,
        Reply::json(200, r#"{"namespace":"acme","status":"active"}"#),
    );
    let home = Home::new();
    let file = home.path().join("secret.txt");
    let run = run_into(
        &home,
        &server,
        &create,
        &file,
        &["--reveal", "--yes", "--output", "text"],
    );
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(std::fs::symlink_metadata(&file).is_err());
    assert!(
        run.stderr().starts_with(
            "note: no `robot_secret` was issued, because the namespace already exists"
        ),
        "{}",
        run.stderr()
    );
    let server = server_for(&create, Reply::json(200, r#"{"namespace":"acme"}"#));
    let doc = run_into(&home, &server, &create, &file, &["--reveal", "--yes"]).ok();
    assert_eq!(
        doc,
        json!({"namespace": "acme", "docker_login_command": null})
    );
}

/// An answer that is not an object holds no top-level secret: a secret inside it cannot be
/// moved whole, so the answer is printed, and one with none writes no file.
#[test]
fn an_answer_that_is_not_an_object_is_never_cut() {
    let s3 = issuers().remove(0);
    for (answer, writes_nothing_ok) in [(json!([{"secret": "x-secret"}]), false), (json!([]), true)]
    {
        let server = server_for(&s3, Reply::json(201, &answer.to_string()));
        let home = Home::new();
        let file = home.path().join("secret.txt");
        let run = run_into(&home, &server, &s3, &file, &["--reveal", "--yes"]);
        assert_eq!(run.json(), answer);
        assert_eq!(run.code() == Some(0), writes_nothing_ok, "{}", run.stderr());
        assert!(std::fs::symlink_metadata(&file).is_err());
    }
}

/// A profile name that is not one plain word cannot be repeated in a command, so the S3 note
/// is left out; the secret still goes only to the file.
#[test]
fn no_note_names_a_command_with_a_profile_that_is_not_a_plain_word() {
    let s3 = issuers().remove(0);
    let server = server_for(&s3, Reply::json(201, &s3.answer));
    let home = Home::new();
    home.write_config("[profiles.\"café\"]\n");
    let file = home.path().join("secret.txt");
    let mut argv = s3.argv.clone();
    let path = file.to_str().expect("the test path is UTF-8");
    argv.extend([
        "--secret-file",
        path,
        "--reveal",
        "--yes",
        "--output",
        "text",
    ]);
    let run = Hipc::against(&home, &server)
        .env("HIPPIUS_PROFILE", "café")
        .run(&argv);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(run.stderr(), "");
    assert_eq!(std::fs::read_to_string(&file).expect("written"), S3_SECRET);
}

/// A Hub answer `hipc` refuses to print, one with a `docker login` command left in it or a
/// login that reads as a flag, still loses no secret with `--secret-file`: the secret is in
/// the file before the refusal, which prints nothing and names the file.
#[test]
fn a_refused_hub_answer_keeps_the_secret_in_the_file() {
    let rotate = issuers().remove(4);
    for (answer, words) in [
        (
            json!({"robot_login": "robot$acme", "robot_secret": ROBOT_SECRET, "help": "docker login -p x"}),
            "holds a `docker login` command",
        ),
        (
            json!({"robot_login": "-x", "robot_secret": ROBOT_SECRET}),
            "starts with `-`",
        ),
    ] {
        let server = server_for(&rotate, Reply::json(200, &answer.to_string()));
        let home = Home::new();
        let file = home.path().join("secret.txt");
        let run = run_into(&home, &server, &rotate, &file, &["--reveal", "--yes"]);
        assert_eq!(run.code(), Some(1), "{}", run.stderr());
        assert_eq!(run.stdout(), "");
        no_secret_in(&run, ROBOT_SECRET);
        assert_eq!(
            std::fs::read_to_string(&file).expect("the secret was written"),
            ROBOT_SECRET
        );
        let error = run.error();
        assert_eq!(error["code"], "internal");
        assert_eq!(error["side_effects"], "partial");
        let message = error["message"].as_str().expect("a message");
        assert!(message.contains(words), "{message}");
        assert!(
            message.ends_with(&format!("; the secret is in `{}`", file.display())),
            "{message}"
        );
    }
}

/// A Hub answer that holds a `docker login` command under a key `hipc` does not drop, and
/// whose `--secret-file` cannot be written, prints only the secret: the whole answer, which a
/// file that cannot be written otherwise puts on stdout, would carry the command, and the
/// secret must not be lost.
#[test]
fn a_hub_answer_with_a_login_command_is_not_printed_when_the_file_fails() {
    let rotate = issuers().remove(4);
    let answer = json!({
        "robot_login": "robot$acme",
        "robot_secret": ROBOT_SECRET,
        "help": "docker login -u robot$acme -p s3cr3t-rotated hub.hippius.com",
    });
    let home = Home::new();
    let file = home.path().join("secret.txt");
    let at = file.clone();
    let server = server_for(
        &rotate,
        Reply::json(200, &answer.to_string())
            .before(move || drop(std::fs::write(&at, "someone else's"))),
    );
    let run = run_into(&home, &server, &rotate, &file, &["--reveal", "--yes"]);
    assert_eq!(run.code(), Some(1), "{}", run.stderr());
    assert_eq!(run.json(), json!({"robot_secret": ROBOT_SECRET}));
    assert!(!run.stdout().contains("docker"), "{}", run.stdout());
    assert!(!run.stderr().contains(ROBOT_SECRET), "{}", run.stderr());
    let error = run.error();
    assert_eq!(error["code"], "internal");
    assert_eq!(error["side_effects"], "partial");
    let message = error["message"].as_str().expect("a message");
    assert!(
        message.contains("holds a `docker login` command")
            && message.contains("printed only the secret on stdout"),
        "{message}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "someone else's");
}

/// An S3 token answer that names its secret another way (`secret_key`, `secretAccessKey`)
/// is treated as `secret`: `--secret-file` moves it, a list drops it, and a document printed
/// with `--reveal` names it `secret`.
#[test]
fn an_s3_secret_by_another_name_is_moved_and_dropped() {
    for name in ["secret_key", "secretAccessKey", "SECRET_ACCESS_KEY"] {
        let answer = json!({"accessKeyId": "hip_NEW1", name: S3_SECRET, "id": ID});
        let issuer = issuers().remove(0);
        let server = server_for(&issuer, Reply::json(201, &answer.to_string()));
        let home = Home::new();
        let file = home.path().join("secret.txt");
        let run = run_into(&home, &server, &issuer, &file, &["--yes"]);
        assert_eq!(run.code(), Some(0), "{name}: {}", run.stderr());
        no_secret_in(&run, S3_SECRET);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), S3_SECRET, "{name}");
        assert_eq!(
            run.json()["secret_file"],
            file.to_str().unwrap(),
            "{name}: {}",
            run.stdout()
        );
        let server = server_for(&issuer, Reply::json(201, &answer.to_string()));
        let mut argv = issuer.argv.clone();
        argv.extend(["--reveal", "--yes"]);
        let doc = Hipc::against(&home, &server).run(&argv).ok();
        assert_eq!(doc["secret"], S3_SECRET, "{name}");
        assert!(doc.get(name).is_none(), "{name}: {doc}");
        let listed = json!([answer]);
        let server = Server::start(vec![
            route(
                "GET",
                "/api/objectstore/master-tokens/",
                Reply::json(200, "[]"),
            ),
            route(
                "GET",
                "/api/objectstore/sub-tokens/",
                Reply::json(200, &listed.to_string()),
            ),
        ]);
        let run = Hipc::against(&home, &server).run(&["s3", "tokens", "list"]);
        assert_eq!(run.code(), Some(0), "{name}: {}", run.stderr());
        no_secret_in(&run, S3_SECRET);
        assert_eq!(run.json()["items"][0]["secret"], Value::Null, "{name}");
    }
    // Two spellings with two values: `--secret-file` cannot move one secret whole, so it
    // writes no file and the whole answer goes on stdout, as for any two secrets.
    let issuer = issuers().remove(0);
    let answer =
        json!({"accessKeyId": "hip_NEW1", "secret": S3_SECRET, "secretKey": "other", "id": ID});
    let server = server_for(&issuer, Reply::json(201, &answer.to_string()));
    let home = Home::new();
    let file = home.path().join("secret.txt");
    let run = run_into(&home, &server, &issuer, &file, &["--yes"]);
    assert_eq!(run.code(), Some(1), "{}", run.stderr());
    assert!(
        run.stderr().contains("more than one secret"),
        "{}",
        run.stderr()
    );
    assert!(!file.exists());
}

/// The document of an S3 token create with `--secret-file` names the file in `secret_file`,
/// and `s3 configure --from-token-file` reads the secret from there: a relative path from the
/// current directory, as the create wrote it, not from the document's directory.
#[test]
fn configure_follows_the_secret_file_of_a_token_document() {
    let issuer = issuers().remove(0);
    let server = server_for(&issuer, Reply::json(201, &issuer.answer));
    let home = Home::new();
    let mut argv = issuer.argv.clone();
    argv.extend(["--secret-file", "secret.txt", "--reveal", "--yes"]);
    let run = Hipc::against(&home, &server)
        .current_dir(home.path())
        .run(&argv);
    let doc = run.ok();
    assert_eq!(doc["secret_file"], "secret.txt", "{doc}");
    std::fs::create_dir(home.path().join("out")).unwrap();
    home.file("out/token.json", run.stdout().as_bytes());
    let run = Hipc::new(&home).current_dir(home.path()).run(&[
        "s3",
        "configure",
        "--from-token-file",
        "out/token.json",
    ]);
    assert_eq!(run.ok()["secret_written"], true);
    no_secret_in(&run, S3_SECRET);
    let credentials = std::fs::read_to_string(home.path().join(".aws/credentials")).unwrap();
    assert!(
        credentials.contains(&format!(
            "[hippius]\naws_access_key_id = hip_NEW1\naws_secret_access_key = {S3_SECRET}\n"
        )),
        "{credentials}"
    );
}

/// The file `secret_file` names is held to the checks of `--secret-key-file`, and anything but
/// a regular file is refused before it is opened: a symlink, a FIFO, a directory, a device, a
/// missing file, `-`, an empty path, the hipc config file, and the AWS credentials file are
/// each `usage` naming the document's `secret_file`, with a hint that names the other way in,
/// and nothing is written.
#[test]
fn configure_refuses_a_secret_file_that_is_not_a_regular_file() {
    let home = Home::new();
    home.write_config("");
    let target = home.file("real.txt", S3_SECRET.as_bytes());
    let link = home.path().join("link.txt");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let fifo = home.path().join("fifo.txt");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(made.success());
    let missing = home.path().join("missing.txt");
    let aws = home.path().join("aws-credentials");
    std::fs::write(&aws, S3_SECRET).unwrap();
    let config = home.config();
    let dir = home.path().display().to_string();
    for (file, says) in [
        (link.to_str().unwrap(), "a symlink"),
        (fifo.to_str().unwrap(), "not a regular file"),
        (dir.as_str(), "not a regular file"),
        ("/dev/null", "not a regular file"),
        (missing.to_str().unwrap(), "cannot be read"),
        ("-", "names no file"),
        ("", "names no file"),
        (config.to_str().unwrap(), "hipc config file"),
        (aws.to_str().unwrap(), "AWS"),
    ] {
        let document = json!({"kind": "sub", "access_key_id": "hip_NEW1", "secret_file": file});
        let doc = home.file("token.json", document.to_string().as_bytes());
        let run = Hipc::new(&home)
            .env("AWS_SHARED_CREDENTIALS_FILE", aws.to_str().unwrap())
            .stdin(S3_SECRET.as_bytes())
            .run(&[
                "s3",
                "configure",
                "--from-token-file",
                doc.to_str().unwrap(),
            ]);
        let error = run.fails(2, "usage");
        let message = error["message"].as_str().unwrap();
        assert!(message.contains(says), "{file:?}: {error}");
        assert_eq!(
            error["hint"],
            "run `hipc s3 configure` from the directory the token was created in, or pass \
             `--access-key-id` with `--secret-key-file` naming the secret file",
            "{file:?}"
        );
        no_secret_in(&run, S3_SECRET);
        if !file.contains("config") && !file.contains("aws") {
            assert!(message.contains("`secret_file`"), "{file:?}: {error}");
        }
    }
    assert_eq!(std::fs::read_to_string(&aws).unwrap(), S3_SECRET);
    assert!(!home.path().join(".aws").exists());
}

/// A `secret` in the document wins over a `secret_file`, which is then not read; and a
/// relative `secret_file` is read from the current directory, never beside the document.
#[test]
fn configure_prefers_the_secret_and_reads_a_relative_file_from_the_current_directory() {
    let home = Home::new();
    let both = json!({
        "access_key_id": "hip_NEW1",
        "secret": S3_SECRET,
        "secret_file": "/dev/null",
    });
    let doc = home.file("both.json", both.to_string().as_bytes());
    Hipc::new(&home)
        .run(&[
            "s3",
            "configure",
            "--from-token-file",
            doc.to_str().unwrap(),
        ])
        .ok();
    let credentials = home.path().join(".aws/credentials");
    assert!(
        std::fs::read_to_string(&credentials)
            .unwrap()
            .contains(&format!("aws_secret_access_key = {S3_SECRET}\n"))
    );
    std::fs::create_dir(home.path().join("out")).unwrap();
    home.file("out/secret.txt", b"beside-the-document");
    let relative = json!({"access_key_id": "hip_NEW1", "secret_file": "secret.txt"});
    home.file("out/token.json", relative.to_string().as_bytes());
    let run = Hipc::new(&home).current_dir(home.path()).run(&[
        "s3",
        "configure",
        "--from-token-file",
        "out/token.json",
        "--yes",
    ]);
    let error = run.fails(2, "usage");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("cannot be read"),
        "{error}"
    );
    assert!(!run.stderr().contains("beside-the-document"));
}
