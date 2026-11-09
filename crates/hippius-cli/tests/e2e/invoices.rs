//! `hipc billing invoices get <invoice_id> --output-file <path>`: the invoice PDF goes to a new
//! file the caller names, of mode 0600, and never to stdout.

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::json;

use crate::common::{Hipc, Home, MEMBER, Reply, Server, command, draft, route};

const PDF_PATH: &str = "/api/billing/invoices/12/pdf/";

/// A PDF header followed by bytes that are not UTF-8, so a test sees whether they are
/// written as they came.
const PDF: &[u8] = b"%PDF-1.7\n\x00\xff\xfe binary \x1b]52;c;x\x07 %%EOF\n";

fn pdf_server() -> Server {
    Server::start(vec![route(
        "GET",
        PDF_PATH,
        Reply::bytes(200, "application/pdf", PDF),
    )])
}

fn get(home: &Home, server: &Server, output: &Path) -> crate::common::Run {
    Hipc::against(home, server).run(&[
        "billing",
        "invoices",
        "get",
        "12",
        "--output-file",
        output.to_str().expect("a test home path is UTF-8"),
    ])
}

#[test]
fn get_writes_the_pdf_to_a_new_private_file_and_prints_only_json() {
    let server = pdf_server();
    let home = Home::new();
    let output = home.path().join("invoice.pdf");
    let doc = get(&home, &server, &output).ok();
    assert_eq!(
        doc,
        json!({"id": 12, "output_file": output.to_str().unwrap(), "bytes": PDF.len()})
    );
    assert_eq!(std::fs::read(&output).unwrap(), PDF);
    let mode = std::fs::metadata(&output).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let request = server.only_request();
    assert_eq!(request.target, PDF_PATH);
    // The console asks for the PDF the same way; every other request asks for JSON.
    assert_eq!(request.header("accept"), Some("application/pdf"));
    assert_eq!(request.header("x-hippius-account"), None);
}

#[test]
fn text_mode_prints_the_document_and_never_the_pdf() {
    let server = pdf_server();
    let home = Home::new();
    let output = home.path().join("invoice.pdf");
    let run = Hipc::against(&home, &server).run(&[
        "--output",
        "text",
        "billing",
        "invoices",
        "get",
        "12",
        "--output-file",
        output.to_str().unwrap(),
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(!run.stdout_bytes().windows(4).any(|w| w == b"%PDF"));
    assert!(run.stdout().contains("invoice.pdf"), "{}", run.stdout());
    assert_eq!(std::fs::read(&output).unwrap(), PDF);
}

#[test]
fn an_existing_file_a_symlink_or_stdout_is_refused_before_anything_is_sent() {
    let server = pdf_server();
    let home = Home::new();
    let existing = home.file("kept.pdf", b"kept");
    let link = home.path().join("link.pdf");
    std::os::unix::fs::symlink(home.path().join("nowhere.pdf"), &link).unwrap();
    for (path, says) in [
        (existing.to_str().unwrap(), "never overwrites"),
        (link.to_str().unwrap(), "never overwrites"),
        (home.path().to_str().unwrap(), "never overwrites"),
        ("-", "never to stdout"),
    ] {
        for dry_run in [false, true] {
            let mut args = vec!["billing", "invoices", "get", "12", "--output-file", path];
            if dry_run {
                args.insert(0, "--dry-run");
            }
            let error = Hipc::against(&home, &server).run(&args).fails(2, "usage");
            assert!(
                error["message"].as_str().unwrap().contains(says),
                "{path}: {error}"
            );
        }
    }
    assert_eq!(std::fs::read(&existing).unwrap(), b"kept");
    assert!(!home.path().join("nowhere.pdf").exists());
    assert!(server.requests().is_empty());
}

/// A dangling symlink named with a trailing `/` (or `/.`) is resolved through the link by
/// the system, so it would be created at the link's target; it is refused as a directory
/// path, with nothing sent and nothing created.
#[test]
fn a_dangling_symlink_with_a_trailing_slash_is_refused_and_its_target_is_not_created() {
    let server = pdf_server();
    let home = Home::new();
    let sub = home.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    let link = sub.join("invoice.pdf");
    std::os::unix::fs::symlink("../stolen.pdf", &link).unwrap();
    let link = link.to_str().unwrap();
    for path in [
        format!("{link}/"),
        format!("{link}/."),
        format!("{link}//"),
        format!("{link}/.."),
        "/".to_owned(),
        ".".to_owned(),
    ] {
        let error = Hipc::against(&home, &server)
            .run(&["billing", "invoices", "get", "12", "--output-file", &path])
            .fails(2, "usage");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("names a directory"),
            "{path}: {error}"
        );
    }
    assert!(!home.path().join("stolen.pdf").exists());
    assert!(server.requests().is_empty());
}

/// A dangling symlink put at the path after the parse-time check, while the PDF is on its
/// way, is refused when the file is created: the link stays, and its target is not
/// created. This is the race the exclusive create closes.
#[test]
fn a_dangling_symlink_made_during_the_download_is_refused_and_its_target_is_not_created() {
    let home = Home::new();
    let output = home.path().join("invoice.pdf");
    let target = home.path().join("stolen.pdf");
    let (link, to) = (output.clone(), target.clone());
    let server = Server::start(vec![route(
        "GET",
        PDF_PATH,
        // The server thread must not panic, so a failure to make the link is left to the
        // assertions below.
        Reply::bytes(200, "application/pdf", PDF)
            .before(move || drop(std::os::unix::fs::symlink(&to, &link))),
    )]);
    let error = get(&home, &server, &output).fails(2, "usage");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("invoice.pdf`: the system says `File exists"),
        "{error}"
    );
    assert!(output.symlink_metadata().unwrap().is_symlink());
    assert!(!target.exists());
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn the_path_is_checked_before_the_token() {
    let home = Home::new();
    let existing = home.file("kept.pdf", b"kept");
    Hipc::new(&home)
        .run(&[
            "billing",
            "invoices",
            "get",
            "12",
            "--output-file",
            existing.to_str().unwrap(),
        ])
        .fails(2, "usage");
}

#[test]
fn an_id_that_is_not_a_number_and_a_missing_flag_are_usage() {
    let server = pdf_server();
    let home = Home::new();
    let output = home.path().join("invoice.pdf");
    let output = output.to_str().unwrap();
    for args in [
        &[
            "billing",
            "invoices",
            "get",
            "../12",
            "--output-file",
            output,
        ][..],
        &["billing", "invoices", "get", "-1", "--output-file", output],
        &["billing", "invoices", "get", "12"],
    ] {
        Hipc::against(&home, &server).run(args).fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn a_failed_download_leaves_no_file() {
    let home = Home::new();
    let output = home.path().join("invoice.pdf");
    for (reply, code) in [
        (Reply::not_found(), "not_found"),
        (
            Reply::json(403, r#"{"code":"billing_required","error":"no"}"#),
            "billing_required",
        ),
        (Reply::json(503, "{}"), "unavailable"),
    ] {
        let server = Server::start(vec![route("GET", PDF_PATH, reply)]);
        let error = get(&home, &server, &output).fails(1, code);
        if code == "not_found" {
            assert_eq!(
                error["hint"],
                "run `hipc billing invoices list` to see what exists"
            );
        }
        assert!(!output.exists(), "{code}");
    }
}

#[test]
fn an_answer_that_is_not_a_pdf_is_internal_and_writes_nothing() {
    let home = Home::new();
    let output = home.path().join("invoice.pdf");
    for body in [r#"{"detail":"ok"}"#, ""] {
        let server = Server::start(vec![route("GET", PDF_PATH, Reply::json(200, body))]);
        let error = get(&home, &server, &output).fails(1, "internal");
        assert_eq!(
            error["message"],
            "cannot download the invoice `12`: the API answered 200 with a body that is not a \
             PDF"
        );
        assert!(!output.exists());
    }
}

#[test]
fn a_path_in_a_missing_directory_is_usage_before_anything_is_sent() {
    let server = pdf_server();
    let home = Home::new();
    let file = home.file("file", b"");
    for output in [
        home.path().join("missing").join("invoice.pdf"),
        file.join("invoice.pdf"),
    ] {
        let error = get(&home, &server, &output).fails(2, "usage");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("the directory of that path does not exist"),
            "{error}"
        );
    }
    assert!(server.requests().is_empty());
}

/// A directory that exists but cannot be written is found when the file is created, after
/// the download; it is still usage, since the caller must name another path.
#[test]
fn a_directory_that_cannot_be_written_is_usage_and_leaves_no_file() {
    let server = pdf_server();
    let home = Home::new();
    let locked = home.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
    let output = locked.join("invoice.pdf");
    let error = get(&home, &server, &output).fails(2, "usage");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .ends_with("invoice.pdf`: permission is denied"),
        "{error}"
    );
    assert!(!output.exists());
    assert_eq!(server.requests().len(), 1);
}

/// Runs `hipc billing invoices get 12 --output-file <output>` against `server` from a shell
/// that runs `setup` first, with the environment [`command`] gives every test.
fn get_under_sh(home: &Home, server: &Server, setup: &str, output: &Path) -> Output {
    let hipc = command(home);
    let mut shell = Command::new("/bin/sh");
    for (name, value) in hipc.get_envs() {
        match value {
            Some(value) => shell.env(name, value),
            None => shell.env_remove(name),
        };
    }
    shell
        .env("HIPPIUS_API_URL", server.url())
        .env("HIPPIUS_TOKEN", crate::common::TOKEN)
        .env("HIPPIUS_TOKEN_ORIGIN", server.url())
        .args(["-c", &format!("{setup}; exec \"$0\" \"$@\"")])
        .arg(hipc.get_program())
        .args(["billing", "invoices", "get", "12", "--output-file"])
        .arg(output)
        .output()
        .expect("run hipc under sh")
}

/// The mode is exactly 0600 whatever the umask, which masks the mode `open` is given.
#[test]
fn the_file_is_0600_under_a_strict_umask() {
    let server = pdf_server();
    let home = Home::new();
    let output = home.path().join("invoice.pdf");
    let run = get_under_sh(&home, &server, "umask 0277", &output);
    assert_eq!(run.status.code(), Some(0), "{run:?}");
    let mode = std::fs::metadata(&output).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(std::fs::read(&output).unwrap(), PDF);
}

/// A write that fails removes the file it created, so a retry finds the path free.
///
/// The shell limits the size of any file hipc writes and ignores the signal the limit
/// sends, so a write past it fails with `EFBIG`; stdout and stderr are pipes, which the
/// limit does not cover. The limit is 2048 blocks: 1 MiB where `ulimit -f` counts 512-byte
/// blocks (POSIX), 2 MiB where it counts 1 KiB ones (bash). The PDF is 3 MiB, past both,
/// and the coverage profile an instrumented hipc writes at exit (about 100 KiB) fits.
#[test]
fn a_write_that_fails_removes_the_file() {
    let mut pdf = PDF.to_vec();
    pdf.resize(3 * 1024 * 1024, b' ');
    let server = Server::start(vec![route(
        "GET",
        PDF_PATH,
        Reply::bytes(200, "application/pdf", &pdf),
    )]);
    let home = Home::new();
    let output = home.path().join("invoice.pdf");
    let run = get_under_sh(&home, &server, "trap '' XFSZ; ulimit -f 2048", &output);
    assert_eq!(run.status.code(), Some(1));
    assert_eq!(run.stdout, b"");
    let error: serde_json::Value = serde_json::from_slice(&run.stderr).unwrap();
    assert_eq!(error["error"]["code"], "internal");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("so it was removed"),
        "{error}"
    );
    assert!(!output.exists());
}

#[test]
fn a_membership_sends_the_account_header() {
    let server = pdf_server();
    let home = Home::new();
    let output = home.path().join("invoice.pdf");
    Hipc::against(&home, &server)
        .env("HIPPIUS_ACCOUNT", MEMBER)
        .run(&[
            "billing",
            "invoices",
            "get",
            "12",
            "--output-file",
            output.to_str().unwrap(),
        ])
        .ok();
    assert_eq!(
        server.only_request().header("x-hippius-account"),
        Some(MEMBER)
    );
}

#[test]
fn dry_run_describes_the_get_and_writes_nothing() {
    let server = pdf_server();
    let home = Home::new();
    let output = home.path().join("invoice.pdf");
    let doc = Hipc::against(&home, &server)
        .run(&[
            "--dry-run",
            "billing",
            "invoices",
            "get",
            "12",
            "--output-file",
            output.to_str().unwrap(),
        ])
        .ok();
    assert_eq!(draft(&doc)["method"], "GET");
    assert_eq!(draft(&doc)["url"], format!("{}{PDF_PATH}", server.url()));
    assert!(!output.exists());
    assert!(server.requests().is_empty());
}

#[test]
fn debug_prints_two_lines_and_never_the_pdf() {
    let server = pdf_server();
    let home = Home::new();
    let output = home.path().join("invoice.pdf");
    let run = Hipc::against(&home, &server).run(&[
        "--debug",
        "billing",
        "invoices",
        "get",
        "12",
        "--output-file",
        output.to_str().unwrap(),
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        format!("debug: GET {}{PDF_PATH}\ndebug: -> 200\n", server.url())
    );
    assert_eq!(run.json()["bytes"], PDF.len());
}
