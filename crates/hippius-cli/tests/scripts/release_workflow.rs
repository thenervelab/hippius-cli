//! The release workflow's guards: `scripts/release-on-main.sh`, which refuses a tag on a
//! commit that is not on main, run with a stand-in `gh`; and the homebrew job's step for a
//! missing tap token and the `PyPI` job's `PYPI_PUBLISH` switch, run from the workflow file
//! itself.

use std::process::{Command, Output};

use crate::support::{TempDir, script};

const ON_MAIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/release-on-main.sh"
);

const RELEASE_WORKFLOW: &str = include_str!("../../../../.github/workflows/release.yml");

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

/// Runs the check with a `gh` that logs its arguments and runs `gh_body`.
fn on_main(dir: &TempDir, gh_body: &str, args: &[&str]) -> (Output, String) {
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).expect("create the tools directory");
    let log = dir.path().join("gh.log");
    script(
        &bin,
        "gh",
        &format!("echo \"$@\" >> '{}'\n{gh_body}", log.display()),
    );
    let out = Command::new("/bin/bash")
        .arg(ON_MAIN)
        .args(args)
        .env("PATH", &bin)
        .output()
        .expect("run the check");
    let calls = std::fs::read_to_string(&log).unwrap_or_default();
    (out, calls)
}

#[test]
fn a_commit_on_main_passes() {
    for status in ["identical", "behind"] {
        let dir = TempDir::new();

        let (out, calls) = on_main(
            &dir,
            &format!("echo {status}"),
            &["thenervelab/hippius-cli", COMMIT],
        );

        assert!(
            out.status.success(),
            "{status}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            calls,
            format!("api repos/thenervelab/hippius-cli/compare/main...{COMMIT} --jq .status\n")
        );
    }
}

#[test]
fn a_commit_off_main_or_a_failed_compare_is_refused() {
    for (gh_body, message) in [
        (
            "echo ahead",
            "is not on main: compared with main it is ahead",
        ),
        (
            "echo diverged",
            "is not on main: compared with main it is diverged",
        ),
        ("echo", "is not on main: compared with main it is unknown"),
        ("exit 1", "cannot compare"),
    ] {
        let dir = TempDir::new();

        let (out, _) = on_main(&dir, gh_body, &["thenervelab/hippius-cli", COMMIT]);

        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{gh_body}: {stderr}");
        assert!(stderr.contains(message), "{gh_body}: {stderr}");
    }
}

/// What goes into the API path is checked first, and `gh` is never called for it.
#[test]
fn a_bad_commit_or_repository_is_refused_before_gh_runs() {
    for (args, code, message) in [
        (
            &["thenervelab/hippius-cli", "main"][..],
            1,
            "not a commit SHA: main",
        ),
        (
            &["thenervelab/hippius-cli", &COMMIT.to_uppercase()],
            1,
            "not a commit SHA",
        ),
        (&["../x", COMMIT], 1, "not an owner/repo: ../x"),
        (&["thenervelab/hippius-cli"], 2, "usage:"),
    ] {
        let dir = TempDir::new();

        let (out, calls) = on_main(&dir, "echo identical", args);

        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(code), "{args:?}: {stderr}");
        assert!(stderr.contains(message), "{args:?}: {stderr}");
        assert_eq!(calls, "", "{args:?}");
    }
}

/// The jobs of the release workflow, each with the jobs its `needs` names.
fn jobs() -> Vec<(String, Vec<String>)> {
    let body = RELEASE_WORKFLOW
        .split("\njobs:\n")
        .nth(1)
        .expect("a jobs section");
    let mut jobs: Vec<(String, Vec<String>)> = Vec::new();
    for line in body.lines() {
        if let Some(name) = line
            .strip_prefix("  ")
            .and_then(|rest| rest.strip_suffix(':'))
            && !name.starts_with(' ')
            && !name.starts_with('#')
        {
            jobs.push((name.to_owned(), Vec::new()));
        } else if let Some(needs) = line.strip_prefix("    needs: ") {
            let names = needs.trim_matches(|c| c == '[' || c == ']');
            jobs.last_mut()
                .expect("needs inside a job")
                .1
                .extend(names.split(',').map(|name| name.trim().to_owned()));
        }
    }
    jobs
}

/// Every job waits for the check, through its `needs`, and `publish` runs it again before it
/// creates the release.
#[test]
fn the_release_workflow_runs_the_check_before_anything_else() {
    let jobs = jobs();
    assert!(jobs.len() > 5, "{jobs:?}");
    let reaches = |start: &str| {
        let mut todo = vec![start.to_owned()];
        let mut seen = Vec::new();
        while let Some(job) = todo.pop() {
            if job == "on-main" {
                return true;
            }
            if seen.contains(&job) {
                continue;
            }
            let needs = jobs
                .iter()
                .find(|(name, _)| *name == job)
                .map(|(_, needs)| needs.clone())
                .unwrap_or_default();
            todo.extend(needs);
            seen.push(job);
        }
        false
    };
    for (name, _) in &jobs {
        assert!(
            name == "on-main" || reaches(name),
            "{name} does not wait for on-main"
        );
    }
    let on_main = RELEASE_WORKFLOW
        .split("\n  on-main:\n")
        .nth(1)
        .and_then(|rest| rest.split("\n  ci:\n").next())
        .expect("an on-main job before ci");
    assert!(
        on_main.contains("run: scripts/release-on-main.sh \"${REPO}\" \"${BUILT}\""),
        "the on-main job runs the check"
    );
    let publish = RELEASE_WORKFLOW
        .split("\n  publish:\n")
        .nth(1)
        .expect("a publish job");
    let check = publish
        .find("\n          scripts/release-on-main.sh \"${REPO}\" \"${BUILT}\"\n")
        .expect("publish runs the check");
    let create = publish
        .find("\n          gh release create ")
        .expect("publish creates the release");
    assert!(check < create, "the check comes before the release");
}

/// The `run: |` block of the step named `name`, without its indentation.
fn run_block(name: &str) -> String {
    let lines: Vec<&str> = RELEASE_WORKFLOW.lines().collect();
    let step = lines
        .iter()
        .position(|line| line.trim() == format!("- name: {name}"))
        .unwrap_or_else(|| panic!("a step named {name}"));
    let run = (step..lines.len())
        .find(|&i| lines[i].trim() == "run: |")
        .expect("a run block");
    let indent = " ".repeat(lines[run].len() - lines[run].trim_start().len() + 2);
    lines[run + 1..]
        .iter()
        .take_while(|line| line.trim().is_empty() || line.starts_with(&indent))
        .map(|line| line.get(indent.len()..).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Without `HOMEBREW_TAP_TOKEN`, the homebrew job fails in the public repository, where the
/// tap would keep an older formula, and notes the skip in any other.
#[test]
fn a_missing_tap_token_fails_the_public_repository_only() {
    let block = run_block("Without a tap token, fail in the public repository and skip elsewhere");
    for (repo, code, message) in [
        (
            "thenervelab/hippius-cli",
            1,
            "::error::HOMEBREW_TAP_TOKEN is not set in the homebrew environment",
        ),
        (
            "someone/hippius-cli",
            0,
            "::notice::HOMEBREW_TAP_TOKEN is not set",
        ),
    ] {
        let out = Command::new("/bin/bash")
            .args(["-e", "-c", &block])
            .env("REPO", repo)
            .output()
            .expect("run the step");

        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(out.status.code(), Some(code), "{repo}: {stdout}");
        assert!(stdout.contains(message), "{repo}: {stdout}");
    }
}

/// The `PyPI` wheels are built only when `PYPI_PUBLISH` is `true` and the tag is the workspace
/// version; unset or `false` skips them, and any other value fails, so a misspelt switch is
/// not a silent skip.
#[test]
fn the_pypi_wheels_wait_for_pypi_publish() {
    let block = run_block("Build the wheels only when PYPI_PUBLISH is true");
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
    for (publish, tag, code, build) in [
        ("", tag.as_str(), 0, Some("build=false")),
        ("false", &tag, 0, Some("build=false")),
        ("true", &tag, 0, Some("build=true")),
        ("True", &tag, 1, None),
        ("1", &tag, 1, None),
        ("true", "v999.0.0", 1, None),
    ] {
        let dir = TempDir::new();
        let output = dir.path().join("output");
        std::fs::write(&output, "").expect("create the output file");
        let out = Command::new("/bin/bash")
            .args(["-e", "-c", &block])
            .current_dir(root)
            .env("PUBLISH", publish)
            .env("TAG", tag)
            .env("GITHUB_OUTPUT", &output)
            .output()
            .expect("run the step");

        let written = std::fs::read_to_string(&output).expect("read the output file");
        assert_eq!(
            out.status.code(),
            Some(code),
            "{publish:?} {tag}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            written.lines().next(),
            build,
            "{publish:?} {tag}: {written}"
        );
    }
    // The wheel and the upload are for the PyPI project `hippius`.
    assert!(RELEASE_WORKFLOW.contains("      url: https://pypi.org/project/hippius/\n"));
    assert!(RELEASE_WORKFLOW.contains("    if: needs.pypi-version.outputs.build == 'true'\n"));
    let pyproject = include_str!("../../../../packaging/pypi/pyproject.toml");
    assert!(pyproject.contains("\nname = \"hippius\"\n"), "{pyproject}");
}

/// Every workflow file, by name.
const WORKFLOWS: [(&str, &str); 4] = [
    (
        "ci.yml",
        include_str!("../../../../.github/workflows/ci.yml"),
    ),
    (
        "release-check.yml",
        include_str!("../../../../.github/workflows/release-check.yml"),
    ),
    ("release.yml", RELEASE_WORKFLOW),
    (
        "scheduled.yml",
        include_str!("../../../../.github/workflows/scheduled.yml"),
    ),
];

/// Every job that runs on a runner sets `timeout-minutes`, so a hung step fails in minutes,
/// not after GitHub's six hours. A job that calls a reusable workflow takes none; the called
/// workflow's jobs set their own.
#[test]
fn every_job_has_a_timeout() {
    for (file, workflow) in WORKFLOWS {
        let body = workflow.split("\njobs:\n").nth(1).expect("a jobs section");
        let mut jobs: Vec<(String, String)> = Vec::new();
        for line in body.lines() {
            match line
                .strip_prefix("  ")
                .and_then(|rest| rest.strip_suffix(':'))
            {
                Some(name) if !name.starts_with([' ', '#']) => {
                    jobs.push((name.to_owned(), String::new()));
                }
                _ => {
                    if let Some((_, text)) = jobs.last_mut() {
                        text.push_str(line);
                        text.push('\n');
                    }
                }
            }
        }
        assert!(!jobs.is_empty(), "{file}");
        for (name, text) in jobs {
            if text.contains("\n    uses: ") || text.starts_with("    uses: ") {
                continue;
            }
            let minutes = text
                .lines()
                .find_map(|line| line.strip_prefix("    timeout-minutes: "))
                .unwrap_or_else(|| panic!("{file}: the job {name} sets no timeout-minutes"));
            let minutes: u32 = minutes.parse().expect("a whole number of minutes");
            assert!((1..=90).contains(&minutes), "{file}: {name}: {minutes}");
        }
    }
}

/// The release builds the wheels with the maturin that packaging/pypi/pyproject.toml pins,
/// where Dependabot tracks it, so a bump there fails here until release.yml follows.
#[test]
fn the_wheels_are_built_with_the_maturin_pyproject_pins() {
    let pyproject = include_str!("../../../../packaging/pypi/pyproject.toml");
    let pinned = pyproject
        .lines()
        .find_map(|line| line.strip_prefix("requires = [\"maturin=="))
        .and_then(|rest| rest.strip_suffix("\"]"))
        .expect("pyproject.toml pins one exact maturin");
    let used: Vec<&str> = RELEASE_WORKFLOW
        .lines()
        .filter_map(|line| line.trim().strip_prefix("maturin-version: v"))
        .collect();
    assert_eq!(used, [pinned]);
    let dependabot = include_str!("../../../../.github/dependabot.yml");
    assert!(
        dependabot.contains("  - package-ecosystem: pip\n    directory: /packaging/pypi\n"),
        "{dependabot}"
    );
}

/// The release builds each tarball with `scripts/package.sh`, which `package.rs` checks, and
/// every release build, the tarballs' and the wheels', and each release check's, remaps the
/// runner's paths, so the same commit builds the same bytes.
#[test]
fn the_release_packs_with_the_package_script() {
    let remap = "      - name: Remap the build paths\n";
    assert_eq!(RELEASE_WORKFLOW.matches(remap).count(), 2);
    assert_eq!(WORKFLOWS[1].1.matches(remap).count(), 2);
    assert!(RELEASE_WORKFLOW.contains(
        "\n          scripts/package.sh \"target/${TARGET}/release/hipc\" \"dist/hipc-${TARGET}.tar.gz\"\n"
    ));
}
