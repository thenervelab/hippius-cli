//! The live release check, `scripts/release-check.sh`. Only its refusals run here: every
//! other check needs a real token and the live API.

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
