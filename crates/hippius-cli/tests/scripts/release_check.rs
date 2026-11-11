//! The live release check, `scripts/release-check.sh`. Its checks need a real token and the
//! live API, so here it runs only to its refusals, against a stand-in `hipc` that reaches no
//! server, or a function at a time.

use std::process::Command;

use crate::support::{TempDir, script};

const CHECK: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/release-check.sh"
);

/// A stand-in token; the refusal must not print it.
const TOKEN: &str = "hipc-release-check-test-token-9c1e";

/// Tracing would print the token the moment it is read, so the script refuses to run under
/// `bash -x` or `SHELLOPTS=xtrace`, before it reads the token. The binary it is given does
/// not exist, so a script without the refusal stops at that check and reaches no server.
#[test]
fn xtrace_is_refused_before_the_token_is_read() {
    for (flag, shellopts) in [(Some("-x"), None), (None, Some("xtrace"))] {
        let mut command = Command::new("/bin/bash");
        command.args(flag).arg(CHECK);
        command
            .args(["/nonexistent/hipc", "0.0.0", "x86_64-unknown-linux-musl"])
            .env("HIPPIUS_TOKEN", TOKEN);
        if let Some(value) = shellopts {
            command.env("SHELLOPTS", value);
        }

        let out = command.output().expect("run the release check");

        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{flag:?}: {stderr}");
        assert!(out.stdout.is_empty(), "{flag:?}");
        assert!(
            stderr.contains("release-check: refusing to run with xtrace"),
            "{flag:?}: {stderr}"
        );
        assert!(!stderr.contains(TOKEN), "{flag:?}: {stderr}");
    }
}

/// A run reports every group of checks, whatever fails: here a `hipc` that answers only
/// `schema` and fails every other command, so the checks fail and files `hipc` should have
/// written are missing. The run still reaches the last group and the summary, exits 1, and
/// prints the token nowhere. The stand-in reaches no server: it never connects.
#[test]
fn a_run_where_everything_fails_still_reports_every_group() {
    let dir = TempDir::new();
    let hipc = dir.path().join("hipc");
    script(
        dir.path(),
        "hipc",
        r#"case "$*" in
schema) echo '{"schema_version":4,"commands":[{"path":["version"],"effect":"read_only","args":[],"flags":[]}],"global_flags":[]}' ;;
*) exit 1 ;;
esac"#,
    );
    let tmp = dir.path().join("tmp");
    std::fs::create_dir(&tmp).expect("create the temp directory");

    let out = Command::new("/bin/bash")
        .arg(CHECK)
        .arg(&hipc)
        .args(["0.0.0", "x86_64-unknown-linux-musl"])
        .env("HIPPIUS_TOKEN", TOKEN)
        .env("TMPDIR", &tmp)
        .env_remove("RUNNER_TEMP")
        .output()
        .expect("run the release check");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stdout}\n{stderr}");
    assert!(
        stdout.contains("\n== every command, and the output ==\n"),
        "the last group did not run:\n{stdout}"
    );
    let summary = stdout.lines().last().unwrap_or_default();
    assert!(
        summary.starts_with("== ") && summary.contains(" failed, "),
        "{summary}"
    );
    assert!(!stdout.contains(TOKEN) && !stderr.contains(TOKEN));
    // The temporary directory, which held the stored token, is gone.
    let left: Vec<_> = std::fs::read_dir(&tmp)
        .expect("read the temp directory")
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

/// The definition of the shell function `name` in the release check, from its `name() {`
/// line, or its one-line form, to its closing `}`.
fn function(name: &str) -> String {
    let script = std::fs::read_to_string(CHECK).expect("read the release check");
    let start = format!("{name}() {{");
    let mut lines = script.lines().skip_while(|line| !line.starts_with(&start));
    let first = lines
        .next()
        .expect("the release check defines the function");
    if first.ends_with('}') {
        return first.to_owned();
    }
    let mut body = vec![first];
    body.extend(lines.by_ref().take_while(|line| *line != "}"));
    body.push("}");
    body.join("\n")
}

/// `script`, run by bash under the release check's `set -euo pipefail`, after `functions`.
fn run_strict(functions: &str, script: &str, env: &[(&str, &str)]) -> std::process::Output {
    Command::new("/bin/bash")
        .arg("-c")
        .arg(format!("set -euo pipefail\n{functions}\n{script}"))
        .envs(env.iter().copied())
        .output()
        .expect("run bash")
}

/// A file that is missing reads as the mode `missing`, so a check that records it as a
/// failure runs, rather than the whole run ending under `set -e` before its summary.
#[test]
fn the_mode_of_a_missing_file_does_not_end_the_run() {
    let out = run_strict(
        &function("mode_of"),
        "file=\"$(mode_of /nonexistent/hipc-file)\"\necho \"mode $file\"",
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "mode missing\n");
}

/// The invoice download runs with the real token only when its dry run lists reads alone, so
/// the dry run's methods are read from the document, with the output file moved to a path
/// that does not exist. That dry run itself runs with the fake token against the dead origin,
/// as every offline call does: a `hipc` that ignored `--dry-run` would send nothing real.
#[test]
fn a_local_write_with_the_token_is_held_to_its_dry_run() {
    let temp = TempDir::new();
    let dir = temp.path();
    // A stand-in hipc that prints, as its dry run, the requests the test sets, and its argv.
    script(
        dir,
        "hipc",
        "printf '%s %s %s\\n' \"$HIPPIUS_TOKEN\" \"$HIPPIUS_TOKEN_ORIGIN\" \"$*\" >\"$WORK/argv\"\n\
         printf '%s' \"$DRY\"",
    );
    let fake = dir.join("hipc");
    let work = dir.to_str().expect("a UTF-8 temp path");
    let methods = |dry: &str| {
        let out = run_strict(
            &function("dry_run_methods"),
            "dry_run_methods billing invoices get 7 --output-file /x/invoice.pdf",
            &[
                ("HIPC", fake.to_str().expect("a UTF-8 path")),
                ("WORK", work),
                ("DRY", dry),
                ("FAKE_TOKEN", "fake-token"),
                ("DEAD_ORIGIN", "http://127.0.0.1:9"),
                ("HIPPIUS_TOKEN", "the-real-token"),
            ],
        );
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    assert_eq!(
        methods(r#"{"requests":[{"method":"GET"},{"method":"GET"}]}"#),
        "GET"
    );
    assert_eq!(
        methods(r#"{"requests":[{"method":"GET"},{"method":"POST"}]}"#),
        "GET,POST"
    );
    assert_eq!(methods("not json"), "");
    let argv = std::fs::read_to_string(dir.join("argv")).expect("the stand-in ran");
    assert_eq!(
        argv,
        format!(
            "fake-token http://127.0.0.1:9 --api-url http://127.0.0.1:9 --output json --dry-run \
             billing invoices get 7 --output-file {work}/dry-run-never-written\n"
        )
    );
}

/// A `--debug` URL check passes only on the whole line of the one route the schema gives: an
/// empty lookup, or one that gives two routes, fails rather than matching every `GET` line, and
/// a line for a longer path does not pass for its prefix.
#[test]
fn a_debug_url_check_needs_one_route_and_the_whole_line() {
    let temp = TempDir::new();
    let dir = temp.path();
    std::fs::write(
        dir.join("1.err"),
        "debug: GET https://api.hippius.com/api/compute/vms/abc/\n\
         debug: GET https://api.hippius.com/api/billing/usage/?month=<redacted>\n",
    )
    .expect("write the stderr");
    let stubs = "result() { echo \"$1 $2\"; }\nredacted() { echo redacted; }\n";
    let check = |args: &str| {
        let out = run_strict(
            &format!("{stubs}{}", function("debug_line")),
            &format!("n=1\ndebug_line {args}"),
            &[
                ("OUT", dir.to_str().expect("a UTF-8 temp path")),
                ("ORIGIN", "https://api.hippius.com"),
            ],
        );
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    assert_eq!(check("vms GET /api/compute/vms/abc/"), "PASS vms\n");
    assert_eq!(
        check("usage GET /api/billing/usage/ '?month=<redacted>'"),
        "PASS usage\n"
    );
    for args in [
        "vms GET /api/compute/vms/",
        "vms GET ''",
        "vms GET $'/api/compute/vms/abc/\\n/api/compute/vms/'",
        "usage GET /api/billing/usage/",
    ] {
        assert_eq!(
            check(args),
            format!("FAIL {}\n", args.split(' ').next().unwrap_or_default())
        );
    }
}
