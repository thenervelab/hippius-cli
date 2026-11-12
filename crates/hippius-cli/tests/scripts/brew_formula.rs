//! The Homebrew formula generator, `scripts/brew-formula.sh`, against a fixture
//! SHA256SUMS and the formula it must print.

use std::process::{Command, Output};

use crate::support::TempDir;
use crate::workflow;

const GENERATOR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/brew-formula.sh");

/// A SHA256SUMS with one distinct hash per release target, and the line for `install.sh`
/// that every release's SHA256SUMS also has.
const SUMS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/scripts/SHA256SUMS");

/// The formula the generator prints for version 1.2.3 and [`SUMS`]. Regenerate it with
/// `scripts/brew-formula.sh 1.2.3 crates/hippius-cli/tests/scripts/SHA256SUMS >
/// crates/hippius-cli/tests/scripts/hipc.rb`, and read the diff before committing it.
const FORMULA: &str = include_str!("hipc.rb");

/// The `hipc` crate's manifest, whose description the formula's must stay in step with.
const CRATE_MANIFEST: &str = include_str!("../../Cargo.toml");

fn generate(version: &str, sums: &str) -> Output {
    Command::new("/bin/bash")
        .arg(GENERATOR)
        .args([version, sums])
        .output()
        .expect("run the formula generator")
}

#[test]
fn the_formula_matches_the_checked_in_one() {
    let out = generate("1.2.3", SUMS);

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    let formula = String::from_utf8(out.stdout).expect("the formula is UTF-8");
    assert_eq!(
        formula, FORMULA,
        "the formula changed: regenerate tests/scripts/hipc.rb as its doc comment says"
    );
}

/// The formula is `hipc`, so `brew install thenervelab/tap/hipc` finds it in
/// `Formula/hipc.rb`, where it replaces the formula of the deprecated IPFS-era tool of that
/// name, and it installs the `hipc` binary. The caveats tell an upgraded user of the old
/// `hipc` what changed, and the description does not start with the formula name, which
/// Homebrew's audit refuses.
#[test]
fn the_formula_is_hipc_and_installs_hipc() {
    assert!(FORMULA.contains("\nclass Hipc < Formula\n"), "{FORMULA}");
    // The release job is what puts the formula at that path in the tap. The tap is shared,
    // so the job stages its own formula and removes only the two names this tool's releases
    // used before, and nothing else.
    let release = workflow::release();
    let push = workflow::run(workflow::named(
        workflow::steps(workflow::job(&release, "homebrew")),
        "Commit and push the formula",
    ));
    let lines: Vec<&str> = push.lines().map(str::trim).collect();
    for line in [
        r#"cp "${RUNNER_TEMP}/hipc.rb" Formula/hipc.rb"#,
        "git add Formula/hipc.rb",
    ] {
        assert!(
            lines.contains(&line),
            "the homebrew job no longer runs {line}"
        );
    }
    let removals: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| line.contains("git rm"))
        .collect();
    assert_eq!(
        removals,
        ["git rm --quiet --ignore-unmatch Formula/hip.rb Formula/hippius.rb"],
        "the homebrew job removes something other than this tool's earlier formulas"
    );
    let desc = FORMULA
        .lines()
        .find_map(|line| line.strip_prefix("  desc \""))
        .and_then(|rest| rest.strip_suffix('"'))
        .expect("a desc line");
    assert!(!desc.to_lowercase().starts_with("hipc"), "{desc}");
    // Homebrew's audit caps a description at 80 characters.
    assert!(desc.len() <= 80, "{desc}");
    // The rest of the audit's description rules: no leading article, a capital first
    // letter, no final period.
    let first = desc.split_whitespace().next().unwrap_or_default();
    assert!(
        !["a", "an", "the"].contains(&first.to_lowercase().as_str()),
        "{desc}"
    );
    assert!(desc.starts_with(|c: char| c.is_ascii_uppercase()), "{desc}");
    assert!(!desc.ends_with('.'), "{desc}");
    // The description is written in the generator, not read from the crate. Both name the
    // Hippius console, so a crate description that moves away from it fails here and the
    // formula's is reworded with it.
    assert!(desc.contains("Hippius console"), "{desc}");
    let manifest: toml::Table = CRATE_MANIFEST.parse().expect("Cargo.toml is TOML");
    let crate_desc = manifest["package"]["description"]
        .as_str()
        .expect("the crate has a description");
    assert!(
        crate_desc.starts_with("Hippius console "),
        "the crate description changed: reword the formula desc in scripts/brew-formula.sh"
    );
    assert!(FORMULA.contains("    bin.install \"hipc\"\n"), "{FORMULA}");
    assert!(
        FORMULA.contains(
            "  def caveats\n    \"This hipc is the Hippius console CLI; \
             the IPFS-era hipc commands are gone. Run `hipc --help` to start.\"\n"
        ),
        "{FORMULA}"
    );
}

/// Each platform block gets its own target's tarball and hash, so a swapped line (an
/// Intel URL under `on_arm`) fails here as well as in the golden comparison.
#[test]
fn each_platform_installs_its_own_target() {
    for (block, target, hash) in [
        ("on_macos", "aarch64-apple-darwin", 'a'),
        ("on_macos", "x86_64-apple-darwin", 'b'),
        ("on_linux", "x86_64-unknown-linux-musl", 'c'),
        ("on_linux", "aarch64-unknown-linux-musl", 'd'),
    ] {
        let start = FORMULA
            .find(&format!("  {block} do"))
            .expect("the platform block");
        let arch = if target.starts_with("aarch64") {
            "on_arm"
        } else {
            "on_intel"
        };
        let rest = &FORMULA[start..];
        let at = rest
            .find(&format!("    {arch} do\n"))
            .expect("the arch block");
        let mut lines = rest[at..].lines().skip(1);
        let url = format!(
            "      url \"https://github.com/thenervelab/hippius-cli/releases/download/v1.2.3/\
             hipc-{target}.tar.gz\""
        );
        assert_eq!(lines.next(), Some(url.as_str()));
        let sha = format!("      sha256 \"{}\"", hash.to_string().repeat(64));
        assert_eq!(lines.next(), Some(sha.as_str()));
    }
}

#[test]
fn a_bad_version_or_sums_file_prints_no_formula() {
    let dir = TempDir::new();
    let missing = dir.path().join("SHA256SUMS");
    let partial = dir.path().join("partial");
    // Three of the four targets and install.sh: the Linux aarch64 line is missing.
    let sums = std::fs::read_to_string(SUMS).expect("read the fixture");
    let partial_lines: Vec<&str> = sums
        .lines()
        .filter(|line| !line.ends_with("aarch64-unknown-linux-musl.tar.gz"))
        .collect();
    assert_eq!(partial_lines.len(), 4);
    std::fs::write(&partial, partial_lines.join("\n") + "\n").expect("write the partial sums");
    let missing = missing.to_str().expect("a UTF-8 path");
    let partial = partial.to_str().expect("a UTF-8 path");

    for (version, sums, message) in [
        ("v1.2.3", SUMS, "not a version: v1.2.3"),
        ("1.2", SUMS, "not a version: 1.2"),
        ("1.2.3", missing, "cannot read"),
        (
            "1.2.3",
            partial,
            "no single well-formed line for hipc-aarch64-unknown-linux-musl",
        ),
    ] {
        let out = generate(version, sums);

        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{version} {sums}: {stderr}");
        assert!(stderr.contains(message), "{stderr}");
        assert!(out.stdout.is_empty(), "{version} {sums}");
    }

    let usage = Command::new("/bin/bash")
        .arg(GENERATOR)
        .output()
        .expect("run the generator");
    assert_eq!(usage.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&usage.stderr).starts_with("usage: "));
}
