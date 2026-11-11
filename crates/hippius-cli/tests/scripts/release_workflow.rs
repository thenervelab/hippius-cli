//! The release workflows and their guards: `scripts/release-on-main.sh`, which refuses a
//! tag on a commit that is not on main, run with a stand-in `gh`; the job graph, the steps
//! every release build shares through the release-build action, and the pins, read from the
//! parsed files; and the homebrew job's step for a missing tap token, the `PyPI` job's
//! `PYPI_PUBLISH` switch and the action's own steps, run from the files themselves.

use std::process::{Command, Output};

use yaml_rust2::Yaml;

use crate::support::{TempDir, script};
use crate::workflow;

const ON_MAIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/release-on-main.sh"
);

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

/// Every job waits for the check, through its `needs`; the check runs first in `on-main`,
/// with the tag's version check beside it, and `publish` runs it again before it creates the
/// release.
#[test]
fn the_release_workflow_runs_the_check_before_anything_else() {
    let release = workflow::release();
    let jobs = workflow::jobs(&release);
    assert!(jobs.len() > 5, "{jobs:?}");
    let reaches_on_main = |start: &str| {
        let mut todo = vec![start];
        let mut seen = Vec::new();
        while let Some(id) = todo.pop() {
            if id == "on-main" {
                return true;
            }
            if !seen.contains(&id) {
                seen.push(id);
                todo.extend(workflow::needs(workflow::job(&release, id)));
            }
        }
        false
    };
    for (id, _) in &jobs {
        assert!(
            *id == "on-main" || reaches_on_main(id),
            "{id} does not wait for on-main"
        );
    }

    let on_main = workflow::steps(workflow::job(&release, "on-main"));
    let check = workflow::named(on_main, "Check that the tagged commit is on main");
    assert_eq!(
        workflow::run(check),
        r#"scripts/release-on-main.sh "${REPO}" "${BUILT}""#
    );
    let version = workflow::named(on_main, "Check that the tag names the workspace version");
    assert_eq!(
        workflow::run(version),
        r#"scripts/release-version.sh "${TAG}""#
    );
    assert_eq!(
        workflow::text(&version["env"]["TAG"], "the tag"),
        "${{ github.ref_name }}"
    );

    let publish = workflow::steps(workflow::job(&release, "publish"));
    let create = workflow::run(workflow::named(publish, "Create the release"));
    let lines: Vec<&str> = create.lines().map(str::trim).collect();
    let check = lines
        .iter()
        .position(|line| *line == r#"scripts/release-on-main.sh "${REPO}" "${BUILT}""#)
        .expect("publish runs the check");
    let create = lines
        .iter()
        .position(|line| line.starts_with("gh release create "))
        .expect("publish creates the release");
    assert!(check < create, "the check comes before the release");
}

/// The tag is compared with the workspace version once, in `on-main`: no build job, and no
/// wheel job, carries a check of its own that could disagree with it.
#[test]
fn the_version_is_checked_once() {
    let release = workflow::release();
    let checks: Vec<&str> = workflow::jobs(&release)
        .into_iter()
        .flat_map(|(id, job)| workflow::steps(job).iter().map(move |step| (id, step)))
        .filter(|(_, step)| {
            step["run"].as_str().is_some_and(|run| {
                run.contains("release-version.sh") || run.contains("cargo metadata")
            })
        })
        .map(|(id, _)| id)
        .collect();
    assert_eq!(checks, ["on-main"]);
}

/// Runs the step `name` of the release workflow with bash, as a runner does (`bash -e`), in
/// the repository root, with `env`.
fn run_step(name: &str, env: &[(&str, &std::ffi::OsStr)]) -> Output {
    let release = workflow::release();
    let step = workflow::named(workflow::every_step(&release), name);
    Command::new("/bin/bash")
        .args(["-e", "-c", workflow::run(step)])
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .envs(env.iter().copied())
        .output()
        .expect("run the step")
}

/// Without `HOMEBREW_TAP_TOKEN`, the homebrew job fails in the public repository, where the
/// tap would keep an older formula, and notes the skip in any other.
#[test]
fn a_missing_tap_token_fails_the_public_repository_only() {
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
        let out = run_step(
            "Without a tap token, fail in the public repository and skip elsewhere",
            &[("REPO", repo.as_ref())],
        );

        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(out.status.code(), Some(code), "{repo}: {stdout}");
        assert!(stdout.contains(message), "{repo}: {stdout}");
    }
}

/// The `PyPI` wheels are built only when `PYPI_PUBLISH` is `true`; unset or `false` skips
/// them, and any other value fails, so a misspelt switch is not a silent skip.
#[test]
fn the_pypi_wheels_wait_for_pypi_publish() {
    for (publish, code, build) in [
        ("", 0, Some("build=false")),
        ("false", 0, Some("build=false")),
        ("true", 0, Some("build=true")),
        ("True", 1, None),
        ("1", 1, None),
    ] {
        let dir = TempDir::new();
        let output = dir.path().join("output");
        std::fs::write(&output, "").expect("create the output file");

        let out = run_step(
            "Build the wheels only when PYPI_PUBLISH is true",
            &[
                ("PUBLISH", publish.as_ref()),
                ("GITHUB_OUTPUT", output.as_os_str()),
            ],
        );

        let written = std::fs::read_to_string(&output).expect("read the output file");
        assert_eq!(
            out.status.code(),
            Some(code),
            "{publish:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(written.lines().next(), build, "{publish:?}: {written}");
    }

    let release = workflow::release();
    assert_eq!(
        workflow::text(
            &workflow::job(&release, "pypi-wheels")["if"],
            "the wheels' condition"
        ),
        "needs.pypi-version.outputs.build == 'true'"
    );
    // The upload goes to the PyPI project packaging/pypi/pyproject.toml names.
    let name = pyproject()["project"]["name"]
        .as_str()
        .expect("pyproject.toml names the project")
        .to_owned();
    assert_eq!(name, "hippius");
    assert_eq!(
        workflow::text(
            &workflow::job(&release, "pypi-publish")["environment"]["url"],
            "the environment's url"
        ),
        format!("https://pypi.org/project/{name}/")
    );
}

/// packaging/pypi/pyproject.toml, parsed.
fn pyproject() -> toml::Table {
    include_str!("../../../../packaging/pypi/pyproject.toml")
        .parse()
        .expect("pyproject.toml is TOML")
}

/// Every job that runs on a runner sets `timeout-minutes`, so a hung step fails in minutes,
/// not after GitHub's six hours, and at most 90: the slowest job, the macOS test suite, takes
/// about 15. A job that calls a reusable workflow takes none; the called workflow's jobs set
/// their own.
#[test]
fn every_job_has_a_timeout() {
    for (file, workflow) in workflow::every_workflow() {
        let jobs = workflow::jobs(&workflow);
        assert!(!jobs.is_empty(), "{file}");
        for (id, job) in jobs {
            if job["uses"].as_str().is_some() {
                continue;
            }
            let minutes = job["timeout-minutes"]
                .as_i64()
                .unwrap_or_else(|| panic!("{file}: the job {id} sets no timeout-minutes"));
            assert!((1..=90).contains(&minutes), "{file}: {id}: {minutes}");
        }
    }
}

/// Every action a workflow or the release action uses is pinned to a full commit SHA, so a
/// moved tag cannot change what runs; only this repository's own files go by path.
#[test]
fn every_action_is_pinned_to_a_commit() {
    let workflows = workflow::every_workflow();
    let action = workflow::release_build_action();
    let mut used: Vec<(&str, &str)> = workflow::steps(&action["runs"])
        .iter()
        .filter_map(workflow::uses)
        .map(|used| ("release-build/action.yml", used))
        .collect();
    for (file, yaml) in &workflows {
        // A job that calls a reusable workflow names it in `uses`, as a step names an action.
        let jobs = workflow::jobs(yaml).into_iter().map(|(_, job)| job);
        let steps = workflow::every_step(yaml);
        used.extend(
            jobs.chain(steps)
                .filter_map(workflow::uses)
                .map(|u| (*file, u)),
        );
    }
    assert!(used.len() > 20, "{used:?}");
    for (file, used) in used {
        if used.starts_with("./") {
            continue;
        }
        let Some((_, sha)) = used.split_once('@') else {
            panic!("{file}: {used} names no version");
        };
        assert!(
            sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
            "{file}: {used} is not pinned to a commit"
        );
    }
}

/// The release builds the wheels with the maturin that packaging/pypi/pyproject.toml pins,
/// where Dependabot tracks it, so a bump there fails here until the action follows.
#[test]
fn the_wheels_are_built_with_the_maturin_pyproject_pins() {
    let requires = pyproject()["build-system"]["requires"].clone();
    let pinned = requires
        .as_array()
        .and_then(|requires| match requires.as_slice() {
            [one] => one.as_str()?.strip_prefix("maturin=="),
            _ => None,
        })
        .expect("pyproject.toml requires one exact maturin");
    let action = workflow::release_build_action();
    let used: Vec<&str> = workflow::steps(&action["runs"])
        .iter()
        .filter(|step| workflow::uses(step).is_some_and(|u| u.starts_with("PyO3/maturin-action@")))
        .map(|step| workflow::text(&step["with"]["maturin-version"], "the maturin version"))
        .collect();
    assert_eq!(used, [format!("v{pinned}")]);

    let dependabot = workflow::load(".github/dependabot.yml");
    let tracked = dependabot["updates"]
        .as_vec()
        .expect("a list of updates")
        .iter()
        .any(|update| {
            update["package-ecosystem"].as_str() == Some("pip")
                && update["directory"].as_str() == Some("/packaging/pypi")
        });
    assert!(tracked, "Dependabot tracks packaging/pypi");
}

/// The jobs that build a release artifact: each with the workflow it is in and the package
/// it builds.
const BUILDS: [(&str, &str, &str); 4] = [
    ("release.yml", "build", "tarball"),
    ("release.yml", "pypi-wheels", "wheel"),
    ("release-check.yml", "release-build", "tarball"),
    ("release-check.yml", "wheels", "wheel"),
];

/// Every release build, the tarballs', the wheels' and each rehearsal's, goes through the
/// release-build action, for the same four targets on the same runner images, so the
/// rehearsal builds what a release builds.
#[test]
fn every_release_build_goes_through_the_action() {
    let release = workflow::release();
    let release_check = workflow::release_check();
    let mut matrices = Vec::new();
    for (file, id, package) in BUILDS {
        let yaml = if file == "release.yml" {
            &release
        } else {
            &release_check
        };
        let job = workflow::job(yaml, id);
        let builds: Vec<&Yaml> = workflow::steps(job)
            .iter()
            .filter(|step| workflow::uses(step) == Some("./.github/actions/release-build"))
            .collect();
        let [build] = builds.as_slice() else {
            panic!("{file}: {id} uses the release-build action once");
        };
        assert_eq!(
            workflow::text(&build["with"]["target"], "the target"),
            "${{ matrix.target }}",
            "{file}: {id}"
        );
        assert_eq!(
            workflow::text(&build["with"]["package"], "the package"),
            package,
            "{file}: {id}"
        );
        assert_eq!(
            workflow::text(&job["runs-on"], "the runner"),
            "${{ matrix.runner }}",
            "{file}: {id}"
        );
        matrices.push((file, id, job["strategy"]["matrix"].clone()));
    }
    let (_, _, first) = &matrices[0];
    let targets: Vec<&str> = first["include"]
        .as_vec()
        .expect("a list of targets")
        .iter()
        .map(|entry| workflow::text(&entry["target"], "a target"))
        .collect();
    assert_eq!(
        targets,
        [
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
            "x86_64-unknown-linux-musl",
            "aarch64-unknown-linux-musl"
        ]
    );
    for (file, id, matrix) in &matrices {
        assert_eq!(
            matrix, first,
            "{file}: {id} builds other targets or runners"
        );
    }

    // No other step builds a release binary without the action's settings before it.
    for (file, yaml) in [
        ("release.yml", &release),
        ("release-check.yml", &release_check),
    ] {
        for (id, job) in workflow::jobs(yaml) {
            let steps = workflow::steps(job);
            let action = steps
                .iter()
                .position(|step| workflow::uses(step) == Some("./.github/actions/release-build"));
            for (at, step) in steps.iter().enumerate() {
                let builds = step["run"]
                    .as_str()
                    .is_some_and(|run| run.contains("cargo build --release"));
                assert!(
                    !builds || action.is_some_and(|action| action < at),
                    "{file}: {id} builds a release binary outside the action"
                );
            }
        }
    }
}

/// The action refuses a package it does not know, so a misspelt `wheels` fails the job
/// instead of skipping every build step.
#[test]
fn the_action_refuses_an_unknown_package() {
    let action = workflow::release_build_action();
    let check = workflow::run(workflow::named(
        workflow::steps(&action["runs"]),
        "Check the inputs",
    ));
    for (package, code) in [("tarball", 0), ("wheel", 0), ("wheels", 1), ("", 1)] {
        let out = Command::new("/bin/bash")
            .args(["-e", "-c", check])
            .env("PACKAGE", package)
            .output()
            .expect("run the step");

        assert_eq!(out.status.code(), Some(code), "{package:?}");
    }
}

/// The action maps the checkout and the cargo home to fixed names and builds ring's C for
/// musl with musl-gcc, for the build steps after it and for the rest of the job, so the same
/// commit builds the same bytes on any runner.
#[test]
fn the_action_remaps_the_build_paths_for_the_rest_of_the_job() {
    let action = workflow::release_build_action();
    let remap = workflow::run(workflow::named(
        workflow::steps(&action["runs"]),
        "Remap the build paths",
    ));
    for (cargo_home, cargo) in [
        (Some("/runner/cargo"), "/runner/cargo"),
        (None, "/home/runner/.cargo"),
    ] {
        let dir = TempDir::new();
        let env_file = dir.path().join("env");
        let mut command = Command::new("/bin/bash");
        command
            .args(["-e", "-c", remap])
            .env_clear()
            .env("GITHUB_ENV", &env_file)
            .env("GITHUB_WORKSPACE", "/runner/work/hipc")
            .env("HOME", "/home/runner");
        if let Some(cargo_home) = cargo_home {
            command.env("CARGO_HOME", cargo_home);
        }

        let out = command.output().expect("run the step");

        assert!(out.status.success(), "{cargo_home:?}");
        assert_eq!(
            std::fs::read_to_string(&env_file).expect("the step writes GITHUB_ENV"),
            format!(
                "RUSTFLAGS=--remap-path-prefix=/runner/work/hipc=/build \
                 --remap-path-prefix={cargo}=/cargo\n\
                 CC_x86_64_unknown_linux_musl=musl-gcc\n\
                 CC_aarch64_unknown_linux_musl=musl-gcc\n"
            ),
            "{cargo_home:?}"
        );
    }
}

/// The release packs each tarball with `scripts/package.sh`, which `package.rs` checks.
#[test]
fn the_release_packs_with_the_package_script() {
    let release = workflow::release();
    let package = workflow::run(workflow::named(
        workflow::steps(workflow::job(&release, "build")),
        "Package",
    ));
    assert!(
        package
            .lines()
            .any(|line| line.trim()
                == r#"scripts/package.sh "target/${TARGET}/release/hipc" "dist/hipc-${TARGET}.tar.gz""#),
        "{package}"
    );
}

/// The action fails a release binary that holds `HIPC_TEST_POLL_INTERVAL_MS`, the test suite's
/// poll interval, which only a build with debug assertions compiles in. The debug `hipc` this
/// suite runs holds it, so the check is shown to find the name in a real build; a binary
/// without it passes, and a missing binary fails. The release profile leaves debug assertions
/// off, so the build the action makes does not hold it.
#[test]
fn the_action_refuses_a_release_binary_that_reads_the_test_poll_interval() {
    let action = workflow::release_build_action();
    let check = workflow::run(workflow::named(
        workflow::steps(&action["runs"]),
        "No test seam",
    ));
    for (binary, code) in [
        (Some(env!("CARGO_BIN_EXE_hipc")), 1),
        (Some("/bin/sh"), 0),
        (None, 1),
    ] {
        let dir = TempDir::new();
        let release = dir.path().join("target/x86_64-unknown-linux-musl/release");
        std::fs::create_dir_all(&release).expect("create the release directory");
        if let Some(binary) = binary {
            std::os::unix::fs::symlink(binary, release.join("hipc")).expect("link the binary");
        }

        let out = Command::new("/bin/bash")
            .args(["-e", "-c", check])
            .current_dir(dir.path())
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("TARGET", "x86_64-unknown-linux-musl")
            .output()
            .expect("run the step");

        assert_eq!(out.status.code(), Some(code), "{binary:?}");
    }

    let manifest: toml::Table = include_str!("../../../../Cargo.toml")
        .parse()
        .expect("Cargo.toml is TOML");
    let release = &manifest["profile"]["release"];
    assert!(
        release
            .get("debug-assertions")
            .is_none_or(|on| on.as_bool() == Some(false)),
        "the release profile turns debug assertions on: {release}"
    );
}

/// Every `run:` block of the release-build action passes shellcheck, as a script under
/// `scripts/` and a workflow's `run:` block do: actionlint reads only `.github/workflows`, so
/// the action's shell would otherwise merge unlinted. Each block runs under `bash`, the shell
/// every step names. Linux only: the coverage job and the lint job run there, and the Ubuntu
/// runner image ships shellcheck, which the macOS one need not.
#[cfg(target_os = "linux")]
#[test]
fn every_run_block_of_the_release_action_passes_shellcheck() {
    let action = workflow::release_build_action();
    let blocks: Vec<&str> = workflow::steps(&action["runs"])
        .iter()
        .filter(|step| !step["run"].is_badvalue())
        .map(workflow::run)
        .collect();
    assert!(
        blocks.len() >= 5,
        "the action's steps were read: {blocks:?}"
    );
    let dir = TempDir::new();
    for (at, block) in blocks.iter().enumerate() {
        let file = dir.path().join(format!("step-{at}.sh"));
        std::fs::write(&file, format!("#!/usr/bin/env bash\n{block}\n")).expect("write a step");
        let out = Command::new(crate::support::tool("shellcheck"))
            .arg(&file)
            .output()
            .expect("run shellcheck");
        assert!(
            out.status.success(),
            "step {at}:\n{block}\n{}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
}
