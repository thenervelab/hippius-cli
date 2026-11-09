//! Every file flag reads a regular file, or stdin for `-`: a symlink is not followed and a
//! FIFO is refused, so a planted link cannot send the config file and a read cannot wait for
//! ever. The hipc config file and the AWS credentials file are refused whatever names them,
//! a hard link or stdin included: a file flag sends what it reads, to the API, to support
//! staff, or into a published page.

use std::path::Path;

use crate::common::{Hipc, Home, Server, command, draft};

const ZONE: &str = "zab12cd34ef56";

/// A dry run of each command that sends a file it reads, with `path` as its file.
fn commands(path: &str) -> Vec<(&'static str, Vec<String>)> {
    let words = |words: &[&str]| words.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
    vec![
        (
            "--body-file",
            words(&[
                "--dry-run",
                "support",
                "tickets",
                "create",
                "--subject",
                "x",
                "--body-file",
                path,
            ]),
        ),
        (
            "--block-body-file",
            words(&[
                "--dry-run",
                "cdn",
                "waf",
                "update",
                ZONE,
                "--block-body-file",
                path,
            ]),
        ),
        (
            "--body-file",
            words(&[
                "--dry-run",
                "api",
                "POST",
                "/support/tickets/",
                "--body-file",
                path,
            ]),
        ),
        (
            "--token-file",
            words(&["auth", "login", "--token-file", path]),
        ),
    ]
}

/// Runs every command of [`commands`] on `path`, and checks each is refused with `usage`
/// and the message `message` gives for its flag, and that nothing is sent.
fn refused(home: &Home, path: &Path, message: impl Fn(&str, &str) -> String) {
    let server = Server::start(vec![]);
    let shown = path.to_str().expect("a test path is UTF-8");
    for (flag, args) in commands(shown) {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let run = Hipc::against(home, &server).run(&args);
        assert!(!run.stdout().contains("s3cr3t"), "{}", run.stdout());
        assert!(!run.stderr().contains("s3cr3t"), "{}", run.stderr());
        let error = run.fails(2, "usage");
        assert_eq!(error["message"], message(shown, flag), "{args:?}");
    }
    assert!(server.requests().is_empty());
}

fn home_with_config() -> Home {
    let home = Home::new();
    home.write_config("[profiles.default]\ntoken = \"hip_s3cr3t_token\"\n");
    home
}

#[test]
fn a_symlink_to_the_config_file_is_not_followed() {
    let home = home_with_config();
    let link = home.path().join("blocked.html");
    std::os::unix::fs::symlink(home.config(), &link).unwrap();
    refused(&home, &link, |path, _| {
        format!("`{path}` is a symlink, and a symlink is never followed")
    });
}

#[test]
fn the_config_file_is_refused_by_any_name_or_link() {
    let home = home_with_config();
    let config = |path: &str, flag: &str| {
        format!(
            "`{path}` is the hipc config file, which holds the token, and `{flag}` never reads it"
        )
    };
    refused(&home, &home.config(), config);
    let hard = home.path().join("notes.txt");
    std::fs::hard_link(home.config(), &hard).unwrap();
    refused(&home, &hard, config);
    // Another spelling of the path is the same file.
    let dotted = home.path().join(".hippius/../.hippius/config.toml");
    refused(&home, &dotted, config);
}

#[test]
fn the_default_config_file_is_refused_when_hippius_config_names_another() {
    let home = home_with_config();
    let server = Server::start(vec![]);
    let other = home.file("other.toml", b"[profiles.default]\n");
    let path = home.config();
    let shown = path.to_str().unwrap();
    let error = Hipc::against(&home, &server)
        .env("HIPPIUS_CONFIG", other.to_str().unwrap())
        .run(&[
            "--dry-run",
            "support",
            "tickets",
            "create",
            "--subject",
            "x",
            "--body-file",
            shown,
        ])
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        format!(
            "`{shown}` is the hipc config file, which holds the token, and `--body-file` never \
             reads it"
        )
    );
}

#[test]
fn the_aws_files_are_refused_at_their_default_paths_and_where_the_variables_point() {
    let home = Home::new();
    let aws = home.path().join(".aws");
    std::fs::create_dir(&aws).unwrap();
    let default = aws.join("credentials");
    std::fs::write(&default, "[default]\naws_secret_access_key = s3cr3t\n").unwrap();
    let message = |path: &str, flag: &str| {
        format!("`{path}` is an AWS CLI file, which can hold S3 keys, and `{flag}` never reads it")
    };
    refused(&home, &default, message);
    // The AWS config file can hold keys too.
    let config = aws.join("config");
    std::fs::write(&config, "[default]\naws_secret_access_key = s3cr3t\n").unwrap();
    refused(&home, &config, message);
    let moved = home.file("keys.ini", b"[default]\naws_secret_access_key = s3cr3t\n");
    let server = Server::start(vec![]);
    let shown = moved.to_str().unwrap();
    let error = Hipc::against(&home, &server)
        .env("AWS_SHARED_CREDENTIALS_FILE", shown)
        .run(&[
            "--dry-run",
            "support",
            "tickets",
            "create",
            "--subject",
            "x",
            "--body-file",
            shown,
        ])
        .fails(2, "usage");
    assert_eq!(error["message"], message(shown, "--body-file"));
    let error = Hipc::against(&home, &server)
        .env("AWS_CONFIG_FILE", shown)
        .run(&[
            "--dry-run",
            "support",
            "tickets",
            "create",
            "--subject",
            "x",
            "--body-file",
            shown,
        ])
        .fails(2, "usage");
    assert_eq!(error["message"], message(shown, "--body-file"));
}

#[test]
fn a_fifo_is_refused_without_waiting() {
    let home = Home::new();
    let fifo = home.path().join("message.txt");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(made.success());
    refused(&home, &fifo, |path, _| {
        format!("`{path}` is not a regular file")
    });
}

/// Stdin is read for `-`, unless it is redirected from one of the two files.
#[test]
fn stdin_is_read_unless_it_is_the_config_file() {
    let home = home_with_config();
    let args = [
        "--dry-run",
        "support",
        "tickets",
        "create",
        "--subject",
        "x",
        "--body-file",
        "-",
    ];
    let doc = Hipc::new(&home)
        .env("HIPPIUS_TOKEN", "hip_test_token")
        .stdin(b"the VM will not boot\n")
        .run(&args)
        .ok();
    assert_eq!(draft(&doc)["body"]["description"], "the VM will not boot\n");
    let output = command(&home)
        .env("HIPPIUS_TOKEN", "hip_test_token")
        .arg("--json")
        .args(args)
        .stdin(std::fs::File::open(home.config()).unwrap())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stdout, b"");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains("s3cr3t"), "{stderr}");
    assert!(
        stderr.contains(
            "stdin is the hipc config file, which holds the token, and `--body-file` never reads it"
        ),
        "{stderr}"
    );
}

/// A public key file is the one input that follows a symlink, as dotfile managers link
/// `~/.ssh/*.pub`; its target is still held to the other checks.
#[test]
fn a_public_key_link_is_followed_to_a_regular_file_that_is_not_the_config() {
    let home = home_with_config();
    let server = Server::start(vec![]);
    let key = |path: &Path| {
        let path = path.to_str().expect("a test path is UTF-8").to_owned();
        Hipc::against(&home, &server).run(&[
            "--dry-run",
            "compute",
            "ssh-keys",
            "create",
            "--name",
            "k",
            "--public-key-file",
            &path,
        ])
    };
    let public = home.file(
        "id.pub",
        b"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA me@laptop\n",
    );
    let linked = home.path().join("linked.pub");
    std::os::unix::fs::symlink(&public, &linked).unwrap();
    key(&linked).ok();
    let to_config = home.path().join("config.pub");
    std::os::unix::fs::symlink(home.config(), &to_config).unwrap();
    let error = key(&to_config).fails(2, "usage");
    assert_eq!(
        error["message"],
        format!(
            "`{}` is the hipc config file, which holds the token, and `--public-key-file` never \
             reads it",
            to_config.display()
        )
    );
    let fifo = home.path().join("fifo");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let to_fifo = home.path().join("fifo.pub");
    std::os::unix::fs::symlink(&fifo, &to_fifo).unwrap();
    let error = key(&to_fifo).fails(2, "usage");
    assert_eq!(
        error["message"],
        format!("`{}` is not a regular file", to_fifo.display())
    );
    assert!(server.requests().is_empty());
}
