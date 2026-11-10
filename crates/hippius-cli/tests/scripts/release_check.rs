//! The live release check, `scripts/release-check.sh`. Its checks need a real token and the
//! live API, so here it runs only to its refusals, or against a stand-in `hipc` that reaches
//! no server.

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
schema) echo '{"schema_version":3,"commands":[{"path":["version"],"effect":"read_only","args":[],"flags":[]}],"global_flags":[]}' ;;
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
