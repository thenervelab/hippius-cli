//! The `PyPI` version check, `scripts/pypi-version-check.sh`: the workspace version the
//! `hippius` wheel is built at, which the release job compares with the tag.

use std::process::{Command, Output};

use crate::support::TempDir;

const CHECK: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/pypi-version-check.sh"
);

fn check(args: &[&str]) -> Output {
    Command::new("/bin/bash")
        .arg(CHECK)
        .args(args)
        .output()
        .expect("run the version check")
}

/// Runs the check on a workspace manifest whose text is `manifest`.
fn check_manifest(dir: &TempDir, manifest: &str) -> Output {
    let path = dir.path().join("Cargo.toml");
    std::fs::write(&path, manifest).expect("write the manifest");
    check(&[path.to_str().expect("a UTF-8 path")])
}

/// A workspace manifest with `version` in `[workspace.package]`, between other tables that
/// carry versions of their own.
fn workspace(version: &str) -> String {
    format!(
        "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.package]\nversion = \"{version}\"\n\
         edition = \"2024\"\n\n[workspace.dependencies]\nclap = {{ version = \"4.6\" }}\n"
    )
}

#[test]
fn an_x_y_z_version_passes_and_is_printed() {
    let dir = TempDir::new();
    // A 0.x version passes: every release of `hipc` is above the project's earlier ones.
    for version in ["0.15.0", "0.2.77", "1.0.0", "1.2.3", "12.0.0", "01.0.0"] {
        let out = check_manifest(&dir, &workspace(version));

        assert!(out.status.success(), "{version}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{version}\n"));
    }
}

#[test]
fn a_version_not_x_y_z_is_refused() {
    let dir = TempDir::new();
    for (manifest, message) in [
        (workspace("1.0.0-rc.1"), "1.0.0-rc.1 is not X.Y.Z"),
        (workspace("1.0"), "1.0 is not X.Y.Z"),
        (workspace("v1.0.0"), "v1.0.0 is not X.Y.Z"),
        // A crate's own version is not the workspace's.
        (
            "[package]\nname = \"x\"\nversion = \"1.0.0\"\n".to_owned(),
            "no version in [workspace.package]",
        ),
    ] {
        let out = check_manifest(&dir, &manifest);

        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{manifest}: {stderr}");
        assert!(stderr.contains(message), "{stderr}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    }

    let missing = dir.path().join("missing.toml");
    let out = check(&[missing.to_str().expect("a UTF-8 path")]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot read"));

    let out = check(&["a", "b"]);
    assert_eq!(out.status.code(), Some(2));
}

/// Without an argument the check reads this repository's workspace manifest.
#[test]
fn the_default_manifest_is_the_workspace() {
    let version = env!("CARGO_PKG_VERSION");

    let out = check(&[]);

    assert!(out.status.success(), "{version}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{version}\n"));
}
