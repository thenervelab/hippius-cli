//! The live release check, `scripts/release-check.sh`. Only its refusals and a few of its
//! functions run here: every other check needs a real token and the live API.

use std::process::Command;

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
/// that does not exist.
#[test]
fn a_local_write_with_the_token_is_held_to_its_dry_run() {
    let dir = std::env::temp_dir().join(format!("hipc-release-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("make a temp dir");
    let fake = dir.join("hipc");
    // A stand-in hipc that prints, as its dry run, the requests the test sets, and its argv.
    std::fs::write(
        &fake,
        "#!/bin/bash\nprintf '%s\\n' \"$*\" >\"$WORK/argv\"\nprintf '%s' \"$DRY\"\n",
    )
    .expect("write the stand-in");
    let mut permissions = std::fs::metadata(&fake).expect("stat").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&fake, permissions).expect("chmod the stand-in");
    let work = dir.to_str().expect("a UTF-8 temp path");
    let methods = |dry: &str| {
        let out = run_strict(
            &function("dry_run_methods"),
            "dry_run_methods billing invoices get 7 --output-file /x/invoice.pdf",
            &[
                ("HIPC", fake.to_str().expect("a UTF-8 path")),
                ("WORK", work),
                ("DRY", dry),
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
            "--output json --dry-run billing invoices get 7 --output-file \
             {work}/dry-run-never-written\n"
        )
    );
    std::fs::remove_dir_all(&dir).expect("remove the temp dir");
}
