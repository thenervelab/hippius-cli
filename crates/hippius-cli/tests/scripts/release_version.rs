//! The release version check, `scripts/release-version.sh`: the tag must name the workspace
//! version, which every crate and the `hippius` wheel are built at.

use std::process::{Command, Output};

use crate::support::TempDir;

const CHECK: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/release-version.sh"
);

fn check(args: &[&str]) -> Output {
    Command::new("/bin/bash")
        .arg(CHECK)
        .args(args)
        .output()
        .expect("run the version check")
}

/// Runs the check of `tag` on a workspace manifest whose text is `manifest`.
fn check_manifest(dir: &TempDir, tag: &str, manifest: &str) -> Output {
    let path = dir.path().join("Cargo.toml");
    std::fs::write(&path, manifest).expect("write the manifest");
    check(&[tag, path.to_str().expect("a UTF-8 path")])
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
fn a_tag_naming_an_x_y_z_version_passes_and_the_version_is_printed() {
    let dir = TempDir::new();
    // A 0.x version passes: the check reads the version, and sets no floor.
    for version in ["0.15.0", "0.2.77", "1.0.0", "1.2.3", "12.0.0", "01.0.0"] {
        let out = check_manifest(&dir, &format!("v{version}"), &workspace(version));

        assert!(out.status.success(), "{version}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{version}\n"));
    }
}

#[test]
fn a_tag_naming_another_version_is_refused() {
    let dir = TempDir::new();
    for tag in ["v1.2.4", "1.2.3", "v1.2.3.0", "v1.2", ""] {
        let out = check_manifest(&dir, tag, &workspace("1.2.3"));

        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{tag}: {stderr}");
        assert!(
            stderr.contains(&format!(
                "tag {tag} does not name the workspace version 1.2.3"
            )),
            "{stderr}"
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout), "");
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
        let out = check_manifest(&dir, "v1.0.0", &manifest);

        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{manifest}: {stderr}");
        assert!(stderr.contains(message), "{stderr}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    }

    let missing = dir.path().join("missing.toml");
    let out = check(&["v1.0.0", missing.to_str().expect("a UTF-8 path")]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot read"));

    for args in [&[][..], &["a", "b", "c"]] {
        let out = check(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(String::from_utf8_lossy(&out.stderr).starts_with("usage: "));
    }
}

/// Without a manifest the check reads this repository's, and its workspace version is the
/// one cargo builds this crate at.
#[test]
fn the_default_manifest_is_the_workspace() {
    let version = env!("CARGO_PKG_VERSION");

    let out = check(&[&format!("v{version}")]);

    assert!(out.status.success(), "{version}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{version}\n"));
}
