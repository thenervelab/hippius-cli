//! `scripts/package.sh`, which packs a release binary into the tarball a release publishes:
//! the same bytes from the same binary, whatever the files' times, modes and owner.

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::{Command, Output};

use crate::support::{TempDir, tool};

const PACKAGE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/package.sh");

/// The commit time the tests pack with.
const EPOCH: &str = "1700000000";

/// Whether GNU tar is installed, as `gtar` or `tar`: a stock Mac has neither, and the test
/// then skips and says so.
fn gnu_tar() -> bool {
    let found = ["gtar", "tar"].iter().any(|name| {
        Command::new(name)
            .arg("--version")
            .output()
            .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).contains("GNU tar"))
    });
    if !found {
        // Shown with --nocapture; a skipped test otherwise reads as a pass.
        drop(writeln!(
            std::io::stderr(),
            "skipped: GNU tar is not installed"
        ));
    }
    found
}

fn package(binary: &Path, tarball: &Path, epoch: Option<&str>) -> Output {
    let mut command = Command::new(PACKAGE);
    command.arg(binary).arg(tarball);
    match epoch {
        Some(epoch) => command.env("SOURCE_DATE_EPOCH", epoch),
        None => command.env_remove("SOURCE_DATE_EPOCH"),
    };
    command.output().expect("run the package script")
}

/// Two packings of the same binary are the same bytes, though the binary's time and mode
/// changed in between; a different binary is not.
#[test]
fn the_same_binary_packs_to_the_same_bytes() {
    if !gnu_tar() {
        return;
    }
    let dir = TempDir::new();
    let binary = dir.path().join("hipc");
    std::fs::write(&binary, b"#!/bin/sh\necho hipc\n").expect("write a stand-in binary");
    let first = dir.path().join("first.tar.gz");
    let out = package(&binary, &first, Some(EPOCH));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))
        .expect("change the mode");
    Command::new(tool("touch"))
        .args(["-t", "203001010000"])
        .arg(&binary)
        .status()
        .expect("change the time");
    let second = dir.path().join("second.tar.gz");
    assert!(package(&binary, &second, Some(EPOCH)).status.success());
    let bytes = |path: &Path| std::fs::read(path).expect("read the tarball");
    assert_eq!(bytes(&first), bytes(&second));
    std::fs::write(&binary, b"#!/bin/sh\necho other\n").expect("change the binary");
    let third = dir.path().join("third.tar.gz");
    assert!(package(&binary, &third, Some(EPOCH)).status.success());
    assert_ne!(bytes(&first), bytes(&third));
}

/// The members are `hipc` and the two license files at the top of the archive, in that
/// order, owned by 0:0, with modes 0755 and 0644 and the commit's time.
#[test]
fn the_members_are_fixed_in_name_order_owner_mode_and_time() {
    if !gnu_tar() {
        return;
    }
    let dir = TempDir::new();
    let binary = dir.path().join("built");
    std::fs::write(&binary, b"binary").expect("write a stand-in binary");
    let tarball = dir.path().join("hipc.tar.gz");
    assert!(package(&binary, &tarball, Some(EPOCH)).status.success());
    let gnutar = if Command::new("gtar").arg("--version").output().is_ok() {
        "gtar"
    } else {
        "tar"
    };
    let listing = Command::new(gnutar)
        .args([
            "--list",
            "--verbose",
            "--numeric-owner",
            "--full-time",
            "--utc",
            "--file",
        ])
        .arg(&tarball)
        .output()
        .expect("list the tarball");
    let listing = String::from_utf8(listing.stdout).expect("the listing is UTF-8");
    let rows: Vec<Vec<&str>> = listing
        .lines()
        .map(|line| line.split_whitespace().collect())
        .collect();
    let names: Vec<&str> = rows.iter().map(|row| row[5]).collect();
    assert_eq!(
        names,
        ["hipc", "LICENSE-MIT", "LICENSE-APACHE"],
        "{listing}"
    );
    for row in &rows {
        let mode = if row[5] == "hipc" {
            "-rwxr-xr-x"
        } else {
            "-rw-r--r--"
        };
        assert_eq!(row[0], mode, "{listing}");
        assert_eq!(row[1], "0/0", "{listing}");
        assert_eq!(
            format!("{} {}", row[3], row[4]),
            "2023-11-14 22:13:20",
            "{listing}"
        );
    }
    // gzip stores no name and no time: bytes 4 to 7 of its header are the time.
    let bytes = std::fs::read(&tarball).expect("read the tarball");
    assert_eq!(&bytes[4..8], &[0, 0, 0, 0]);
}

/// Without `SOURCE_DATE_EPOCH` the time is the commit's; a time that is not a number of seconds,
/// a missing binary, and a wrong argument count are refused.
#[test]
fn bad_input_is_refused() {
    let dir = TempDir::new();
    let tarball = dir.path().join("hipc.tar.gz");
    let out = package(&dir.path().join("missing"), &tarball, Some(EPOCH));
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("package: no binary at "));
    let binary = dir.path().join("hipc");
    std::fs::write(&binary, b"binary").expect("write a stand-in binary");
    let out = package(&binary, &tarball, Some("yesterday"));
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("package: SOURCE_DATE_EPOCH is not a number of seconds")
    );
    let out = Command::new(PACKAGE).arg("one").output().expect("run");
    assert_eq!(out.status.code(), Some(2));
    if gnu_tar() {
        assert!(package(&binary, &tarball, None).status.success());
    }
}
