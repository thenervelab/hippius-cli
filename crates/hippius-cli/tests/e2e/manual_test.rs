//! `scripts/manual_test.sh`, the guided walkthrough of `docs/user-journeys.md`, run with
//! `--offline` (a dead origin and a fake token) so no test reaches the network. It needs
//! `bash` and `jq`, as the script does.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write as _};
use std::os::fd::OwnedFd;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};

use crate::common::{Home, no_server};

const CATALOGUE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/user-journeys.md"
));

/// The fake token `--offline` logs in with, and the fake S3 secret it saves; neither may
/// reach the log.
const OFFLINE_TOKEN: &str = "hipc-manual-test-offline-token-5f3a9c71";
const OFFLINE_SECRET: &str = "hipc-manual-test-offline-s3-secret";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Runs the script with `HOME` and `TMPDIR` inside `home`, so nothing reads the developer's
/// own config and a test can see that the script's temporary directory is removed, and
/// without the variables that would change what it does (the token, the origin, CI and
/// agent detection).
fn manual_test(home: &Home, args: &[&str]) -> Output {
    manual_test_with(home, args, &[])
}

/// [`manual_test`] with the variables `env` set, after the others are removed. `--offline`
/// is aimed at [`no_server`], a port held bound and never listening, unless `env` names
/// another dead origin.
fn manual_test_with(home: &Home, args: &[&str], env: &[(&str, &str)]) -> Output {
    let tmp = home.path().join("tmp");
    std::fs::create_dir_all(&tmp).expect("create TMPDIR");
    let mut command = Command::new(repo().join("scripts/manual_test.sh"));
    for name in ["HIPPIUS_TOKEN", "HIPPIUS_API_URL", "CI", "AI_AGENT"] {
        command.env_remove(name);
    }
    // A run without `--offline` aims at the default origin; every request it might send goes
    // to a proxy that refuses it, so no test can reach the live API.
    for name in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "NO_PROXY",
        "no_proxy",
    ] {
        command.env_remove(name);
    }
    command
        .env("ALL_PROXY", no_server())
        .env("HOME", home.path())
        .env("TMPDIR", &tmp)
        .env("HIPC_MANUAL_TEST_DEAD_ORIGIN", no_server())
        .envs(env.iter().copied())
        .current_dir(home.path())
        .args(args);
    let log = args
        .iter()
        .position(|arg| *arg == "--log")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    bounded(&mut command, Stdio::null(), log.as_deref())
}

/// How long one run of the script may take. A whole offline run takes about a minute on a
/// loaded CI runner; a run past this is hung, and fails with its evidence rather than block
/// the job until CI cancels it.
const RUN_LIMIT: Duration = Duration::from_secs(300);

/// How long the script's output may stay open after it exits: only a process it left
/// behind keeps the pipes open, and that is a leak to report, not to wait for.
const DRAIN_LIMIT: Duration = Duration::from_secs(20);

/// Runs `command` (the script) with `stdin`, in its own process group, and returns its
/// output, as `Command::output` does, with two limits. A run past [`RUN_LIMIT`], or output
/// still held open [`DRAIN_LIMIT`] after the script exited, kills the whole group and fails
/// the test with the processes that were left and the end of the log at `log`.
fn bounded(command: &mut Command, stdin: Stdio, log: Option<&Path>) -> Output {
    bounded_within(command, stdin, log, RUN_LIMIT, DRAIN_LIMIT)
}

/// [`bounded`], with the two limits given.
fn bounded_within(
    command: &mut Command,
    stdin: Stdio,
    log: Option<&Path>,
    run_limit: Duration,
    drain_limit: Duration,
) -> Output {
    let mut child = command
        .process_group(0)
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run scripts/manual_test.sh");
    let group = child.id();
    let (sender, received) = mpsc::channel();
    for (index, mut pipe) in [
        Box::new(child.stdout.take().expect("stdout")) as Box<dyn Read + Send>,
        Box::new(child.stderr.take().expect("stderr")),
    ]
    .into_iter()
    .enumerate()
    {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            // A read error ends the stream early; what was read is still the evidence.
            pipe.read_to_end(&mut bytes).ok();
            // The receiver is gone only once the test has given up.
            sender.send((index, bytes)).ok();
        });
    }
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll the script") {
            break status;
        }
        if started.elapsed() > run_limit {
            let what = format!("is still running after {} seconds", run_limit.as_secs());
            give_up(group, &mut child, log, &what);
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let mut streams = [Vec::new(), Vec::new()];
    for _ in 0..2 {
        match received.recv_timeout(drain_limit) {
            Ok((index, bytes)) => streams[index] = bytes,
            Err(_) => give_up(
                group,
                &mut child,
                log,
                "exited, but a process it started still holds its output open",
            ),
        }
    }
    let [stdout, stderr] = streams;
    Output {
        status,
        stdout,
        stderr,
    }
}

/// Lists what is left of the script's process group, kills the group, and fails the test
/// with that list and the last lines of the log.
fn give_up(group: u32, child: &mut std::process::Child, log: Option<&Path>, what: &str) -> ! {
    let left = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "p=$(pgrep -g {group} | paste -sd , -); \
             [ -z \"$p\" ] || ps -o pid,ppid,pgid,stat,etime,command -p \"$p\""
        ))
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default();
    // The test fails below either way; a failed kill shows as the processes listed.
    Command::new("kill")
        .args(["-KILL", "--", &format!("-{group}")])
        .status()
        .ok();
    child.wait().ok();
    let tail = log
        .and_then(|log| std::fs::read_to_string(log).ok())
        .map_or_else(
            || "(no log)".to_owned(),
            |text| {
                let lines: Vec<&str> = text.lines().collect();
                lines[lines.len().saturating_sub(40)..].join("\n")
            },
        );
    panic!("scripts/manual_test.sh {what}\nprocesses left:\n{left}\nlast log lines:\n{tail}");
}

/// An offline run of `hipc`, with the log in `home`; returns the output and the log.
fn offline(home: &Home, hipc: &str, args: &[&str]) -> (Output, String) {
    let log = home.path().join("manual_test.log");
    let log_arg = log.to_str().expect("the temporary path is UTF-8");
    let mut all = vec!["--offline", "--hipc", hipc, "--log", log_arg];
    all.extend_from_slice(args);
    let output = manual_test(home, &all);
    let text = std::fs::read_to_string(&log).expect("the log is written");
    (output, text)
}

fn stdout(output: &Output) -> &str {
    std::str::from_utf8(&output.stdout).expect("the script prints UTF-8")
}

/// An executable shell script in `home`, standing in for `hipc`.
fn fake_hipc(home: &Home, body: &str) -> PathBuf {
    let fake = home.file("fake-hipc", format!("#!/bin/sh\n{body}\n").as_bytes());
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700))
        .expect("make the fake executable");
    fake
}

/// A stand-in for `hipc` that records every call in `calls`, runs `prelude` first, answers
/// the calls a case matches (each a `case` pattern of ` $* ` and the shell that answers it,
/// which ends with `exit`), and hands every other call, and every `--dry-run`, to the real
/// binary. Returns the stand-in and the calls file.
fn scripted_hipc(home: &Home, prelude: &str, cases: &[(&str, &str)]) -> (PathBuf, PathBuf) {
    let calls = home.path().join("calls");
    let mut body = format!(
        "printf '%s\\n' \"$*\" >>'{}'\n{prelude}\ncase \" $* \" in\n*\" --dry-run \"*) ;;\n",
        calls.display()
    );
    for (pattern, answer) in cases {
        body.push_str(&format!("{pattern}) {answer} ;;\n"));
    }
    body.push_str(&format!(
        "esac\nexec '{}' \"$@\"",
        env!("CARGO_BIN_EXE_hipc")
    ));
    (fake_hipc(home, &body), calls)
}

/// The calls a stand-in recorded that were sent for real: no `--dry-run`.
fn real_calls(calls: &Path) -> Vec<String> {
    std::fs::read_to_string(calls)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.split_whitespace().any(|word| word == "--dry-run"))
        .map(str::to_owned)
        .collect()
}

/// The titles of the run's failures, from the `failures:` list at the end of the log. A
/// stand-in that answers a call where `--offline` expects `unavailable` fails that step on
/// purpose, so a test checks that those are the only failures.
fn failed_titles(log: &str) -> Vec<String> {
    let Some((_, list)) = log.rsplit_once("\nfailures:\n") else {
        return Vec::new();
    };
    list.lines()
        .take_while(|line| line.starts_with('['))
        .filter_map(|line| line.split_once(": "))
        .map(|(_, what)| what.split(" (exit ").next().unwrap_or(what).to_owned())
        .collect()
}

/// A free Drive subscription, so the spend tier may ask to buy one.
const FREE_DRIVE: (&str, &str) = (
    "*\" drive subscription get \"*",
    "printf '%s\\n' '{\"plan_code\":\"free\"}'; exit 0",
);

/// The throwaway VM's exposure removed, and its firewall closed, so the public IP may be
/// attached.
const EXPOSURE_REMOVED: (&str, &str) = ("*\" exposures delete \"*", "exit 0");
const FIREWALL_CLOSED: (&str, &str) = ("*\" compute firewall update \"*", "exit 0");

/// The scenario names `--list` prints, in order, setup and teardown included.
fn listed_scenarios(listing: &str) -> Vec<String> {
    listing
        .lines()
        .filter_map(|line| line.strip_prefix("scenario ")?.split_once(':'))
        .map(|(name, _)| name.to_owned())
        .collect()
}

/// The log's steps, each from its `==== [n]` header to the next.
fn steps(log: &str) -> impl Iterator<Item = &str> {
    log.split("\n==== [").skip(1)
}

fn assert_clean_offline_run(output: &Output, log: &str, home: &Home) {
    assert!(output.status.success(), "{log}");
    // Everything printed is in the log, and nothing else.
    assert_eq!(stdout(output), log);
    assert!(!log.contains(OFFLINE_TOKEN), "{log}");
    assert!(!log.contains(OFFLINE_SECRET), "{log}");
    assert!(log.contains(", 0 failed, "), "{log}");
    assert!(
        log.contains("check: the log holds neither the token nor a revealed secret: PASS"),
        "{log}"
    );
    // Offline every snapshot read fails as expected, so neither comparison is a pass.
    assert!(
        log.contains("check: the account is as it was found: SKIP (not read: "),
        "{log}"
    );
    // The temporary directory, which held the token, is gone.
    let left = std::fs::read_dir(home.path().join("tmp"))
        .expect("TMPDIR exists")
        .count();
    assert_eq!(left, 0);
}

/// The catalogue's command blocks are the script's `--list --tier write` output, block for
/// block, so the two cannot drift apart.
#[test]
fn the_catalogue_lists_the_commands_the_script_runs() {
    let home = Home::new();
    let output = manual_test(&home, &["--list", "--tier", "write"]);
    assert!(output.status.success());
    let listing = stdout(&output);
    let expected: Vec<&str> = listing.trim_end().split("\n\n").collect();
    let blocks: Vec<&str> = CATALOGUE
        .split("```text\n")
        .skip(1)
        .map(|rest| rest.split("\n```").next().unwrap())
        .filter(|block| block.starts_with("scenario "))
        .collect();
    assert_eq!(blocks, expected);
    let names = listed_scenarios(listing);
    assert_eq!(names.first().map(String::as_str), Some("setup"));
    assert_eq!(names.last().map(String::as_str), Some("teardown"));
}

/// `--list` in the read tier shows each create as a dry run and no write or undo step.
#[test]
fn the_read_tier_lists_no_write() {
    let home = Home::new();
    let output = manual_test(&home, &["--list"]);
    let listing = stdout(&output);
    assert!(!listing.contains("[write]"), "{listing}");
    assert!(!listing.contains("[undo]"), "{listing}");
    assert!(listing.contains("[dry-run] hipc --output text s3 buckets create <bucket> --dry-run"));
}

/// The spend tier is opt-in: neither the default tier nor `write` lists a purchase or its
/// question, and `spend` lists each purchase after the question that guards it.
#[test]
fn only_the_spend_tier_lists_a_purchase() {
    let home = Home::new();
    for args in [&["--list"][..], &["--list", "--tier", "write"]] {
        let listing = stdout(&manual_test(&home, args)).to_owned();
        assert!(!listing.contains("[spend]"), "{args:?}: {listing}");
        assert!(!listing.contains("[ask]"), "{args:?}: {listing}");
    }
    let output = manual_test(&home, &["--list", "--tier", "spend"]);
    assert!(output.status.success());
    let listing = stdout(&output);
    let asks = listing.matches("  [ask] ").count();
    assert_eq!(asks, 5, "{listing}");
    for line in listing
        .lines()
        .filter(|line| line.starts_with("  [spend] "))
    {
        let before = &listing[..listing.find(line).expect("the line is in the listing")];
        let scenario_start = before.rfind("\nscenario ").expect("in a scenario");
        assert!(
            before[scenario_start..].contains("  [ask] "),
            "no question before {line}"
        );
    }
    assert!(
        listing.contains("[spend] hipc --json compute vms create"),
        "{listing}"
    );
    assert!(
        listing.contains("[check] the Drive plan this run bought is cancelled"),
        "{listing}"
    );
}

/// The write tier, offline: every scenario runs, every create and delete is sent to the
/// dead origin and fails with the expected `unavailable`, and the exit trap still works off
/// the ledger.
#[test]
fn an_offline_write_run_passes_and_works_off_the_ledger() {
    let home = Home::new();
    let (output, log) = offline(&home, env!("CARGO_BIN_EXE_hipc"), &["--tier", "write"]);
    assert_clean_offline_run(&output, &log, &home);
    let listing = stdout(&manual_test(&home, &["--list"])).to_owned();
    let scenarios: Vec<String> = listed_scenarios(&listing)
        .into_iter()
        .filter(|name| name != "setup" && name != "teardown")
        .collect();
    for name in &scenarios {
        assert!(
            log.contains(&format!("######## scenario {name}: ")),
            "{name}"
        );
        assert!(
            log.contains(&format!("######## end of scenario {name}\n")),
            "{name}"
        );
    }
    let writes: Vec<&str> = steps(&log)
        .filter(|step| {
            let header = step.lines().next().unwrap_or_default();
            header.ends_with(" (write)") || header.ends_with(" (undo)")
        })
        .collect();
    // Six creates, ten changes to what they made (a policy, two rotations, a rename, a
    // budget, a quota, and four edits of the WAF list), and seven undos (the budget's
    // included); and three changes to what the account has (the runner concurrency and spend
    // cap, a billing account's name), each put back.
    assert_eq!(writes.len(), 29, "{log}");
    for step in writes {
        assert!(
            step.contains("expected error code unavailable: PASS"),
            "{step}"
        );
    }
    // The write tier buys nothing and asks nothing.
    assert!(!log.contains(" (spend)\n"), "{log}");
    assert!(!log.contains(": spend: "), "{log}");
    for kind in ["ssh-key", "hub-key", "s3-token", "bucket", "project"] {
        assert!(
            log.contains(&format!("cleanup: {kind} hipc-manual-")),
            "{kind}: {log}"
        );
    }
    assert!(log.contains("cleanup: cdn-list hipc_manual_"), "{log}");
    for change in [
        "runner-max settings",
        "runner-cap limit",
        "billing-name bill_00000000000000000000",
    ] {
        assert!(
            log.contains(&format!(
                "cleanup: {change}: cannot read it (unavailable); offline, nothing was changed"
            )),
            "{change}: {log}"
        );
    }
    assert!(
        log.contains(": cannot list (unavailable); offline"),
        "{log}"
    );
    // Each scenario logs in and out from its own home, and the snapshot home does too.
    let homes = scenarios.len() + 1;
    assert_eq!(log.matches(": Log in (local)").count(), homes);
    assert_eq!(log.matches(": Log out (local)").count(), homes);
}

/// A create, and in the spend tier a purchase, is entered in the ledger before it is sent:
/// a wrapper around `hipc` copies the ledger each time it is asked to create or subscribe
/// for real, and every resource's line is already there, as `pending`.
#[test]
fn the_ledger_holds_a_resource_before_its_create_runs() {
    let home = Home::new();
    let seen = home.path().join("ledger-at-create");
    // A free Drive plan to buy, and an exposure that is removed and a firewall that is
    // closed, so the subscribe and the public IP are sent too.
    let (wrapper, _) = scripted_hipc(
        &home,
        &format!(
            concat!(
                "case \" $* \" in *\" --dry-run \"*) ;; ",
                "*\" create \"* | *\" subscribe \"*) ",
                "cat \"$TMPDIR\"/hipc-manual.*/ledger >>'{}' ;; esac"
            ),
            seen.display(),
        ),
        &[FREE_DRIVE, EXPOSURE_REMOVED, FIREWALL_CLOSED],
    );
    let (output, log) = offline(
        &home,
        wrapper.to_str().expect("UTF-8"),
        &[
            "--tier",
            "spend",
            "--yes-spend",
            "drive",
            "s3",
            "hub",
            "compute",
            "cdn",
            "team",
            "support",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{log}");
    assert!(
        failed_titles(&log).iter().all(|title| {
            title.starts_with("Read the Drive subscription")
                || title == "Remove the exposure"
                || title == "Close every inbound port"
        }),
        "{log}"
    );
    let seen = std::fs::read_to_string(seen).expect("the wrapper saw a create");
    let kinds = [
        ("drive-sub", "solo"),
        ("bucket", "hipc-manual-"),
        ("s3-token", "hipc-manual-"),
        ("hub-key", "hipc-manual-"),
        ("ssh-key", "hipc-manual-"),
        ("vm", "hipc-manual-"),
        ("public-ip", "hipc-manual-"),
        ("database", "hipc-manual-"),
        ("bucket", "hipc-manual-cdn-"),
        ("cdn-zone", "hipc-manual-"),
        ("cdn-list", "hipc_manual_"),
        ("project", "hipc-manual-"),
        ("ticket", "hipc-manual-"),
    ];
    for (kind, name) in kinds {
        assert!(
            seen.lines()
                .any(|line| line.starts_with(&format!("{kind} {name}"))
                    && line.ends_with(" - pending")),
            "{kind}: {seen}"
        );
    }
}

/// A change the write tier makes to what the account already has is in the ledger, with the
/// value to put back, before it is sent; when the scenario's own put-back fails, the exit
/// trap puts the value back and proves it by a read.
#[test]
fn a_change_the_scenario_could_not_put_back_is_put_back_by_the_trap() {
    let home = Home::new();
    let seen = home.path().join("ledger-at-change");
    let tries = home.path().join("put-back-tries");
    let lower = format!(
        "cat \"$TMPDIR\"/hipc-manual.*/ledger \"$TMPDIR\"/hipc-manual.*/restore/runner-max.settings \
         >>'{}'; exit 0",
        seen.display()
    );
    // The scenario's put-back fails; the trap's, the second, works.
    let put_back = format!(
        "echo x >>'{t}'; [ \"$(wc -l <'{t}')\" -ge 2 ] && exit 0; exit 1",
        t = tries.display()
    );
    let (wrapper, calls) = scripted_hipc(
        &home,
        "",
        &[
            (
                "*\" compute runners settings get \"*",
                "printf '%s\\n' '{\"max_concurrent\":3,\"quota_runners\":3}'; exit 0",
            ),
            (
                "*\" compute runners settings update --max-concurrent 2 \"*",
                &lower,
            ),
            (
                "*\" compute runners settings update --max-concurrent 3 \"*",
                &put_back,
            ),
        ],
    );
    let (output, log) = offline(
        &home,
        wrapper.to_str().expect("UTF-8"),
        &["--tier", "write", "compute"],
    );
    assert_eq!(output.status.code(), Some(1), "{log}");
    // The stand-in answers where offline expects `unavailable`, so those steps fail.
    assert!(
        failed_titles(&log).iter().all(|title| {
            title.starts_with("Read the runner settings")
                || title == "Lower the runner concurrency"
                || title == "Put the runner concurrency back"
        }),
        "{log}"
    );
    let seen = std::fs::read_to_string(seen).expect("the wrapper saw the change");
    assert!(
        seen.lines()
            .any(|line| line == "runner-max settings - pending"),
        "{seen}"
    );
    assert!(seen.lines().any(|line| line == "3"), "{seen}");
    let sent: Vec<String> = real_calls(&calls)
        .into_iter()
        .filter(|call| call.contains("compute runners settings update --max-concurrent 3 --yes"))
        .collect();
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(
        log.contains("cleanup: runner-max settings: put back\n"),
        "{log}"
    );
    assert!(!log.contains("cleanup: runner-max settings: FAIL"), "{log}");
}

/// The server refuses a `max_concurrent` above `quota_runners`, so a stored value above the
/// quota could not be put back: the write tier skips the change and sends no update. A
/// value at the quota is still lowered, and a settings answer without a quota skips too.
#[test]
fn the_runner_concurrency_change_is_skipped_when_it_could_not_be_put_back() {
    for (settings, reason) in [
        (
            r#"{"max_concurrent":5,"quota_runners":3}"#,
            Some(
                "SKIP: max_concurrent 5 is above quota_runners 3, a value the server refuses, so it could not be put back",
            ),
        ),
        (
            r#"{"max_concurrent":3}"#,
            Some(
                "SKIP: the settings show no quota_runners number, so the script cannot tell whether 3 could be put back",
            ),
        ),
        (
            r#"{"max_concurrent":3,"quota_runners":null}"#,
            Some(
                "SKIP: the settings show no quota_runners number, so the script cannot tell whether 3 could be put back",
            ),
        ),
        // At 1 or less there is no lower value, whatever the quota.
        (
            r#"{"max_concurrent":1}"#,
            Some("SKIP: max_concurrent is 1, so there is no lower value"),
        ),
        (r#"{"max_concurrent":3,"quota_runners":3}"#, None),
    ] {
        let home = Home::new();
        let get = format!("printf '%s\\n' '{settings}'; exit 0");
        let (wrapper, calls) = scripted_hipc(
            &home,
            "",
            &[
                ("*\" compute runners settings get \"*", &get),
                (
                    "*\" compute runners settings update --max-concurrent \"*",
                    "exit 0",
                ),
            ],
        );
        let (_, log) = offline(
            &home,
            wrapper.to_str().expect("UTF-8"),
            &["--tier", "write", "compute"],
        );
        let sent = real_calls(&calls);
        let updates = |flag: &str| {
            sent.iter()
                .filter(|call| call.contains(&format!("compute runners settings update {flag}")))
                .count()
        };
        if let Some(reason) = reason {
            assert!(log.contains(reason), "{settings}: {log}");
            assert_eq!(updates("--max-concurrent"), 0, "{settings}: {log}");
        } else {
            // The lower value once; the put-back, which the trap repeats offline.
            assert_eq!(updates("--max-concurrent 2 --yes"), 1, "{settings}: {log}");
            assert!(!log.contains("SKIP: max_concurrent"), "{settings}: {log}");
            assert!(
                !log.contains("SKIP: the settings show"),
                "{settings}: {log}"
            );
        }
    }
}

/// The spend cap is put back by sending the server's own `monthly_cap` with `hipc api PUT`,
/// and a billing account's old name with `billing-accounts update`, the first with an id, not
/// the Default; when the scenario's put-backs fail, the exit trap sends them again.
#[test]
fn the_trap_puts_back_the_spend_cap_and_a_billing_account_name() {
    let home = Home::new();
    let bodies = home.path().join("bodies");
    let tries = home.path().join("tries");
    // Fails the first time each kind is put back, then works.
    let twice = |kind: &str| {
        format!(
            "echo {kind} >>'{t}'; [ \"$(grep -c {kind} '{t}')\" -ge 2 ] && exit 0; exit 1",
            t = tries.display()
        )
    };
    let cap_back = format!(
        "while [ $# -gt 0 ]; do [ \"$1\" = --body-file ] && cat \"$2\" >>'{}'; shift; done; {}",
        bodies.display(),
        twice("cap")
    );
    let name_back = twice("name");
    let (wrapper, calls) = scripted_hipc(
        &home,
        "",
        &[
            (
                "*\" compute runners limit get \"*",
                "printf '%s\\n' '{\"monthly_cap\":\"20000000000000000000\",\
                 \"monthly_cap_credits\":\"20\"}'; exit 0",
            ),
            (
                "*\" compute runners limit update --monthly-cap 21 \"*",
                "exit 0",
            ),
            ("*\" api PUT /runners/limit/ \"*", &cap_back),
            (
                "*\" account billing-accounts list \"*",
                "printf '%s\\n' '{\"billing_accounts\":[{\"id\":\"default\",\"name\":\"Default\"},\
                 {\"id\":\"bill_0123456789abcdef0123\",\"name\":\"Ops team\"}]}'; exit 0",
            ),
            (
                "*\" account billing-accounts update bill_0123456789abcdef0123 --name hipc-manual-\"*",
                "exit 0",
            ),
            (
                "*\" account billing-accounts update bill_0123456789abcdef0123 --name Ops team \"*",
                &name_back,
            ),
        ],
    );
    let (output, log) = offline(
        &home,
        wrapper.to_str().expect("UTF-8"),
        &["--tier", "write", "compute", "team"],
    );
    assert_eq!(output.status.code(), Some(1), "{log}");
    assert!(
        failed_titles(&log).iter().all(|title| {
            title.starts_with("Read the runner limit")
                || title.starts_with("List the billing accounts")
                || title == "Raise the runner spend cap"
                || title == "Put the runner spend cap back"
                || title == "Rename a billing account"
                || title == "Rename the billing account back"
        }),
        "{log}"
    );
    let sent = real_calls(&calls);
    let count = |needle: &str| sent.iter().filter(|call| call.contains(needle)).count();
    assert_eq!(count("api PUT /runners/limit/ --body-file "), 2, "{sent:?}");
    assert_eq!(
        count("account billing-accounts update bill_0123456789abcdef0123 --name Ops team"),
        2,
        "{sent:?}"
    );
    let bodies = std::fs::read_to_string(bodies).expect("the cap was put back");
    assert_eq!(
        bodies,
        "{\"monthly_cap\":\"20000000000000000000\"}\n".repeat(2),
        "the server's own monthly_cap, sent back"
    );
    assert!(
        log.contains("cleanup: runner-cap limit: put back\n"),
        "{log}"
    );
    assert!(
        log.contains("cleanup: billing-name bill_0123456789abcdef0123: put back\n"),
        "{log}"
    );
}

/// No cap at all, whose meaning is not confirmed, is not raised; and a name hipc would trim
/// is not changed, since it could not be put back as it was.
#[test]
fn no_cap_and_a_name_hipc_would_trim_are_left_alone() {
    let home = Home::new();
    let (wrapper, calls) = scripted_hipc(
        &home,
        "",
        &[
            (
                "*\" compute runners limit get \"*",
                "printf '%s\\n' '{\"monthly_cap\":null,\"monthly_cap_credits\":null}'; exit 0",
            ),
            (
                "*\" account billing-accounts list \"*",
                "printf '%s\\n' '{\"billing_accounts\":[{\"id\":\"bill_0123456789abcdef0123\",\"name\":\"Ops \"}]}'; \
                 exit 0",
            ),
        ],
    );
    let (_, log) = offline(
        &home,
        wrapper.to_str().expect("UTF-8"),
        &["--tier", "write", "compute", "team"],
    );
    let sent = real_calls(&calls);
    assert!(
        !sent.iter().any(|call| call.contains("runners limit update")
            || call.contains("api PUT /runners/limit/")
            || call.contains("billing-accounts update")),
        "{sent:?}"
    );
    assert!(
        log.contains("SKIP: the runner limit shows no cap of a whole number of credits to raise"),
        "{log}"
    );
    assert!(
        log.contains("SKIP: hipc would not send the billing account's name back as it is"),
        "{log}"
    );
}

/// The spend tier, offline, with `--yes-spend`: every purchase is announced with its cost,
/// sent to the dead origin, and its undo is sent too; the exit trap works off what is left,
/// and the run passes.
#[test]
fn an_offline_spend_run_passes_and_announces_each_cost() {
    let home = Home::new();
    let (output, log) = offline(
        &home,
        env!("CARGO_BIN_EXE_hipc"),
        &["--tier", "spend", "--yes-spend"],
    );
    assert_clean_offline_run(&output, &log, &home);
    let spends: Vec<&str> = steps(&log)
        .filter(|step| {
            step.lines()
                .next()
                .unwrap_or_default()
                .ends_with(" (spend)")
        })
        .collect();
    // The Drive plan, the VM, its public IP, the database, the zone, and the ticket.
    assert_eq!(spends.len(), 6, "{log}");
    let sent: Vec<&&str> = spends
        .iter()
        .filter(|step| step.contains("expected error code unavailable: PASS"))
        .collect();
    // Offline the Drive subscription cannot be read, so nothing is bought, and the
    // exposure is never removed, so no public IP is attached; the other four are sent.
    assert_eq!(sent.len(), 4, "{log}");
    let step = |title: &str| {
        steps(&log)
            .find(|step| step.contains(title))
            .unwrap_or_else(|| panic!("no step {title}: {log}"))
            .to_owned()
    };
    assert!(
        step("Subscribe to Starter (spend)")
            .contains("SKIP: the Drive subscription could not be read, so nothing is bought"),
        "{log}"
    );
    assert!(
        step("Attach a public IP (spend)")
            .contains("SKIP: the exposure was not removed, so no public IP is attached"),
        "{log}"
    );
    // One question per purchase, answered by --yes-spend; the VM and its IP share one, and
    // the Drive plan is never asked about, so four.
    assert_eq!(log.matches("answer: y (--yes-spend)").count(), 4, "{log}");
    assert_eq!(log.matches("\ncost: ").count(), 4, "{log}");
    for undo in [
        "Delete the VM (undo)",
        "Delete the database (undo)",
        "Delete the zone (undo)",
        "Delete the zone's bucket (undo)",
    ] {
        assert!(log.contains(undo), "{undo}: {log}");
    }
    assert!(!log.contains("cleanup: drive-sub"), "{log}");
    assert!(log.contains("cleanup: ticket hipc-manual-"), "{log}");
}

/// Money fix: when the Drive subscription cannot be read, there is no telling whether a
/// paid plan exists, so the purchase is not even asked about, and no subscribe is sent.
#[test]
fn an_unreadable_drive_subscription_buys_nothing_and_asks_nothing() {
    let home = Home::new();
    let (wrapper, calls) = scripted_hipc(&home, "", &[]);
    let (output, log) = offline(
        &home,
        wrapper.to_str().expect("UTF-8"),
        &["--tier", "spend", "--yes-spend", "drive"],
    );
    assert_clean_offline_run(&output, &log, &home);
    assert!(!log.contains("\ncost: "), "{log}");
    let sent = real_calls(&calls);
    assert!(
        !sent
            .iter()
            .any(|call| call.contains("drive subscription subscribe")),
        "{sent:?}"
    );
    assert!(
        !sent
            .iter()
            .any(|call| call.contains("drive subscription cancel")),
        "{sent:?}"
    );
}

/// Money fix: a subscribe the server refuses (409) bought nothing, so neither the scenario
/// nor the exit trap cancels anything: the plan that exists is not this run's.
#[test]
fn a_refused_subscribe_is_never_cancelled() {
    let home = Home::new();
    let refused = concat!(
        "printf '%s\\n' '{\"error\":{\"code\":\"conflict\",\"message\":\"already subscribed\",",
        "\"http_status\":409}}' >&2; exit 1"
    );
    let (wrapper, calls) = scripted_hipc(
        &home,
        "",
        &[
            FREE_DRIVE,
            ("*\" drive subscription subscribe \"*", refused),
        ],
    );
    let (_, log) = offline(
        &home,
        wrapper.to_str().expect("UTF-8"),
        &["--tier", "spend", "--yes-spend", "drive"],
    );
    assert!(log.contains("answer: y (--yes-spend)"), "{log}");
    let sent = real_calls(&calls);
    assert!(
        sent.iter()
            .any(|call| call.contains("drive subscription subscribe --plan solo")),
        "{sent:?}"
    );
    assert!(
        !sent
            .iter()
            .any(|call| call.contains("drive subscription cancel")),
        "{sent:?}"
    );
    assert!(
        log.contains(concat!(
            "cleanup: drive-sub solo (pending): the subscribe did not exit 0, ",
            "so nothing is cancelled"
        )),
        "{log}"
    );
}

/// A subscribe that exits 0 is this run's purchase: the scenario cancels it, retrying a
/// 425 (`still_confirming`), and the exit trap cancels it again while it is in the ledger.
#[test]
fn a_bought_plan_is_cancelled_through_a_425() {
    let home = Home::new();
    let count = home.path().join("cancels");
    let confirming = format!(
        concat!(
            "n=$(cat '{0}' 2>/dev/null || echo 0); echo $((n + 1)) >'{0}'; ",
            "if [ \"$n\" -lt 2 ]; then printf '%s\\n' '{{\"error\":{{\"code\":\"conflict\",",
            "\"message\":\"still confirming\",\"http_status\":425,",
            "\"details\":{{\"code\":\"still_confirming\"}}}}}}' >&2; exit 1; fi"
        ),
        count.display()
    );
    let (wrapper, calls) = scripted_hipc(
        &home,
        "",
        &[
            FREE_DRIVE,
            (
                "*\" drive subscription subscribe \"*",
                "printf '%s\\n' '{}'; exit 0",
            ),
            ("*\" drive subscription cancel \"*", &confirming),
        ],
    );
    let mut command = Command::new(repo().join("scripts/manual_test.sh"));
    let tmp = home.path().join("tmp");
    std::fs::create_dir_all(&tmp).expect("create TMPDIR");
    let log_path = home.path().join("manual_test.log");
    command
        .env("HOME", home.path())
        .env("TMPDIR", &tmp)
        .env("HIPC_MANUAL_RETRY_SECONDS", "0")
        .current_dir(home.path())
        .args(["--offline", "--tier", "spend", "--yes-spend", "--hipc"])
        .arg(&wrapper)
        .arg("--log")
        .arg(&log_path)
        .arg("drive");
    let output = bounded(&mut command, Stdio::null(), Some(&log_path));
    let log = std::fs::read_to_string(&log_path).expect("the log is written");
    assert_eq!(output.status.code(), Some(1), "{log}");
    assert_eq!(
        log.matches("--- 425: still confirming; try ").count(),
        2,
        "{log}"
    );
    let cancels = real_calls(&calls)
        .into_iter()
        .filter(|call| call.contains("drive subscription cancel"))
        .count();
    // Three tries in the scenario (two 425s, then the dead origin), then the exit trap's.
    assert_eq!(cancels, 4, "{log}");
    assert!(
        log.contains("cleanup: $ hipc --json drive subscription cancel --yes"),
        "{log}"
    );
}

/// Money fix: a deleted VM or database is `decommissioning`, then `decommissioned`, and
/// stays listed; `revoked` counts it gone, so no step picks it and a list without a live
/// row proves the delete.
#[test]
fn a_decommissioned_vm_or_database_counts_as_gone() {
    let script = std::fs::read_to_string(repo().join("scripts/manual_test.sh")).expect("read");
    let defs = script
        .split_once("JQ_DEFS='")
        .and_then(|(_, rest)| rest.split_once("\n'\n"))
        .map(|(defs, _)| defs)
        .expect("the script defines JQ_DEFS");
    let list = r#"{"items":[
        {"vm_id":"a","name":"gone","status":"decommissioned"},
        {"vm_id":"b","name":"going","status":"decommissioning"},
        {"vm_id":"c","name":"live","status":"running"}],"next_page_token":null}"#;
    let mut jq = Command::new("jq")
        .arg("-c")
        .arg(format!(
            "{defs} [rows[] | select(revoked | not) | .vm_id], live_names"
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("run jq");
    jq.stdin
        .take()
        .expect("jq's stdin")
        .write_all(list.as_bytes())
        .expect("write the list");
    let output = jq.wait_with_output().expect("wait for jq");
    assert_eq!(stdout(&output), "[\"c\"]\n[\"live\"]\n");
}

/// Money fix: the throwaway VM's exposure is of a port nothing listens on, never SSH, and its
/// public IP is attached only after the exposure's delete exited 0 and a firewall with no
/// rule closed every inbound port.
#[test]
fn the_public_ip_waits_for_the_exposure_to_be_removed() {
    let listing = stdout(&manual_test(&Home::new(), &["--list", "--tier", "spend"])).to_owned();
    assert!(!listing.contains("--port 22"), "{listing}");
    let removed = listing
        .find("[undo] hipc --output text compute exposures delete")
        .expect("the exposure is removed");
    let attached = listing
        .find("[spend] hipc --json compute public-ip create")
        .expect("the IP is attached");
    assert!(removed < attached, "{listing}");
    let closed = listing
        .find("[write] hipc --output text compute firewall update")
        .expect("the firewall is closed");
    assert!(removed < closed && closed < attached, "{listing}");
    let home = Home::new();
    let (wrapper, calls) = scripted_hipc(&home, "", &[EXPOSURE_REMOVED, FIREWALL_CLOSED]);
    let (_, log) = offline(
        &home,
        wrapper.to_str().expect("UTF-8"),
        &["--tier", "spend", "--yes-spend", "compute"],
    );
    assert_eq!(
        failed_titles(&log),
        ["Remove the exposure", "Close every inbound port"],
        "{log}"
    );
    let sent = real_calls(&calls);
    assert!(
        sent.iter()
            .any(|call| call.contains("compute public-ip create")),
        "{sent:?}"
    );
    // With the exposure removed but the firewall change failed, no IP is attached.
    let home = Home::new();
    let (wrapper, calls) = scripted_hipc(&home, "", &[EXPOSURE_REMOVED]);
    let (_, log) = offline(
        &home,
        wrapper.to_str().expect("UTF-8"),
        &["--tier", "spend", "--yes-spend", "compute"],
    );
    assert!(
        log.contains("SKIP: the firewall was not closed, so no public IP is attached"),
        "{log}"
    );
    assert!(
        !real_calls(&calls)
            .iter()
            .any(|call| call.contains("compute public-ip create")),
        "{log}"
    );
}

/// Money fix: a launch whose create exits non-zero but prints the VM (a wait that ended
/// early) is in the ledger by that id, so the exit trap deletes it; so is a database. A
/// cleanup that cannot prove the delete keeps the ledger beside the log, without the lines
/// the trap cleaned.
#[test]
fn a_failed_create_that_answers_an_id_is_deleted_by_it() {
    let home = Home::new();
    let vm = "0f8fad5b-d9cb-469f-a165-424242424242";
    let vm_answer = format!("printf '%s\\n' '{{\"vm_id\":\"{vm}\"}}'; exit 1");
    let (wrapper, calls) = scripted_hipc(
        &home,
        "",
        &[
            ("*\" compute vms create \"*", &vm_answer),
            (
                "*\" compute databases create \"*",
                "printf '%s\\n' '{\"id\":4242}'; exit 1",
            ),
            // The database's cleanup works, so its line leaves the ledger; the VM's fails.
            ("*\" compute databases delete 4242 \"*", "exit 0"),
            (
                "*\" compute databases list \"*",
                "printf '%s\\n' '{\"items\":[],\"next_page_token\":null}'; exit 0",
            ),
        ],
    );
    let log_path = home.path().join("manual_test.log");
    let (output, log) = offline(
        &home,
        wrapper.to_str().expect("UTF-8"),
        &["--tier", "spend", "--yes-spend", "compute"],
    );
    assert_eq!(output.status.code(), Some(1), "{log}");
    let sent = real_calls(&calls);
    assert!(
        sent.iter()
            .any(|call| call.contains(&format!("compute vms delete {vm} --yes"))),
        "{sent:?}"
    );
    assert!(
        sent.iter()
            .any(|call| call.contains("compute databases delete 4242 --yes")),
        "{sent:?}"
    );
    let kept = std::fs::read_to_string(home.path().join("manual_test.log.ledger"))
        .expect("the ledger is kept beside the log");
    assert!(
        kept.lines()
            .any(|line| line.starts_with("vm ") && line.ends_with(&format!(" {vm} created"))),
        "{kept}"
    );
    // The trap removed the line it cleaned: the copy lists only what is left.
    assert!(log.contains("cleanup: database hipc-manual-"), "{log}");
    assert!(log.contains(": deleted\n"), "{log}");
    assert!(!kept.contains("database "), "{kept}");
    assert!(
        log.contains(&format!("ledger: {}.ledger", log_path.display())),
        "{log}"
    );
}

/// Money fix: the balance may move only after a purchase's create exited 0; an answer of y
/// alone, or a create that failed, spent nothing.
#[test]
fn the_balance_may_move_only_after_a_purchase_succeeds() {
    let moving = |home: &Home| {
        format!(
            concat!(
                "n=$(cat '{0}' 2>/dev/null || echo 0); echo $((n + 1)) >'{0}'; ",
                "printf '{{\"balance\":\"%s\"}}\\n' $((10 - n)); exit 0"
            ),
            home.path().join("balance-reads").display()
        )
    };
    // The answer is y and the subscribe is sent, but it fails against the dead origin.
    let home = Home::new();
    let balance = moving(&home);
    let (wrapper, _) = scripted_hipc(
        &home,
        "",
        &[("*\" billing balance \"*", &balance), FREE_DRIVE],
    );
    let (_, log) = offline(
        &home,
        wrapper.to_str().expect("UTF-8"),
        &["--tier", "spend", "--yes-spend", "drive"],
    );
    assert!(log.contains("answer: y (--yes-spend)"), "{log}");
    assert!(
        log.contains("check: the balance moved from \"10\" to \"9\": FAIL"),
        "{log}"
    );
    let home = Home::new();
    let balance = moving(&home);
    let (wrapper, _) = scripted_hipc(
        &home,
        "",
        &[
            ("*\" billing balance \"*", &balance),
            FREE_DRIVE,
            (
                "*\" drive subscription subscribe \"*",
                "printf '%s\\n' '{}'; exit 0",
            ),
        ],
    );
    let (_, log) = offline(
        &home,
        wrapper.to_str().expect("UTF-8"),
        &["--tier", "spend", "--yes-spend", "drive"],
    );
    assert!(
        log.contains("as the purchases this run made spend it: PASS"),
        "{log}"
    );
}

/// Money fix: a rename or a rotation edits the resource's ledger line in place, so at every
/// moment the ledger names it exactly once.
#[test]
fn a_rename_or_a_rotation_keeps_one_ledger_line() {
    let home = Home::new();
    let seen = home.path().join("ledgers");
    let prelude = format!(
        "cat \"$TMPDIR\"/hipc-manual.*/ledger >>'{}' 2>/dev/null; echo ---- >>'{}'",
        seen.display(),
        seen.display()
    );
    let (wrapper, _) = scripted_hipc(
        &home,
        &prelude,
        &[
            (
                "*\" account projects create \"*",
                "printf '%s\\n' '{\"id\":\"proj_aaaaaaaaaaaaaaaaaaaa\"}'; exit 0",
            ),
            (
                "*\" account projects update \"*",
                "printf '%s\\n' '{}'; exit 0",
            ),
            (
                "*\" s3 tokens create \"*",
                "printf '%s\\n' '{\"id\":\"tok-1\",\"secret\":\"first-secret-1\"}'; exit 0",
            ),
            (
                "*\" s3 tokens rotate \"*",
                "printf '%s\\n' '{\"id\":\"tok-2\",\"secret\":\"second-secret-2\"}'; exit 0",
            ),
        ],
    );
    let (_, log) = offline(
        &home,
        wrapper.to_str().expect("UTF-8"),
        &["--tier", "write", "s3", "team"],
    );
    let seen = std::fs::read_to_string(seen).expect("the wrapper saw the ledger");
    let dumps: Vec<&str> = seen.split("----\n").collect();
    for kind in ["project ", "s3-token "] {
        let first = dumps
            .iter()
            .position(|dump| dump.lines().any(|line| line.starts_with(kind)))
            .unwrap_or_else(|| panic!("no {kind}line: {seen}"));
        let last = dumps
            .iter()
            .rposition(|dump| dump.lines().any(|line| line.starts_with(kind)))
            .expect("a last line");
        for dump in &dumps[first..=last] {
            assert_eq!(
                dump.lines().filter(|line| line.starts_with(kind)).count(),
                1,
                "{kind}: {seen}"
            );
        }
    }
    assert!(
        seen.lines()
            .any(|line| line.ends_with("-renamed proj_aaaaaaaaaaaaaaaaaaaa created")),
        "{seen}"
    );
    assert!(
        seen.lines()
            .any(|line| line.starts_with("s3-token ") && line.ends_with(" tok-2 created")),
        "{seen}"
    );
    assert!(!log.contains("first-secret-1"), "{log}");
    assert!(!log.contains("second-secret-2"), "{log}");
}

/// A database's client key, a connection string, and a URI that holds a password are
/// masked, and so is a password shorter than 8 characters, since its field names it a
/// secret. A short string that is not in a secret field is not masked.
#[test]
fn a_client_key_a_connection_string_and_a_short_password_are_masked() {
    let home = Home::new();
    let out = concat!(
        r#"{"client_key":"ck-client-key-0001","connection_string":"host=h password=cs-0002","#,
        r#""uri":"postgres://app:uripw-0003@db.example/app","password":"pq7","note":"zz9"}"#
    );
    let fake = fake_hipc(&home, &format!("printf '%s\\n' '{out}'"));
    let (_, log) = offline(&home, fake.to_str().unwrap(), &["first-run"]);
    for secret in ["ck-client-key-0001", "cs-0002", "uripw-0003", "pq7"] {
        assert!(!log.contains(secret), "{secret}: {log}");
    }
    assert!(log.contains("zz9"), "{log}");
    assert!(
        log.contains("check: the log holds neither the token nor a revealed secret: PASS"),
        "{log}"
    );
}

/// A scenario named twice runs once: its resources have one name per run.
#[test]
fn a_scenario_named_twice_runs_once() {
    let home = Home::new();
    let (output, log) = offline(&home, env!("CARGO_BIN_EXE_hipc"), &["agent", "agent"]);
    assert_clean_offline_run(&output, &log, &home);
    assert_eq!(log.matches("######## scenario agent: ").count(), 1, "{log}");
}

/// An account whose private network was never set up answers each network read 403 with
/// "You must call /api/network/provision/ first."; the steps and the snapshot take that as
/// a state, with a note, and nothing sets the network up.
#[test]
fn an_unprovisioned_network_is_a_state_not_a_failure() {
    // Today's answer, with the detail; and the answer of a hipc with `network create`,
    // which says the network is not set up and names the command in its hint.
    let answers = [
        concat!(
            r#"{"error":{"code":"permission","message":"cannot list: the API says "#,
            r#"`You must call /api/network/provision/ first.`","http_status":403}}"#
        ),
        concat!(
            r#"{"error":{"code":"permission","message":"the private network is not set up "#,
            r#"for this account","hint":"run `hipc network create --yes`","http_status":403,"#,
            r#""details":{"detail":"You must call /api/network/provision/ first."}}}"#
        ),
    ];
    for answer in answers {
        let home = Home::new();
        let refused = format!("printf '%s\\n' '{answer}' >&2; exit 1");
        let (wrapper, calls) = scripted_hipc(
            &home,
            "",
            &[(
                "*\" network peers list \"* | *\" network groups list \"* | \
                 *\" network policies list \"* | *\" network vpcs list \"*",
                &refused,
            )],
        );
        let (output, log) = offline(
            &home,
            wrapper.to_str().expect("UTF-8"),
            &["network-kubernetes"],
        );
        assert_clean_offline_run(&output, &log, &home);
        assert_eq!(
            log.matches("note: this account's private network is not set up")
                .count(),
            4,
            "{log}"
        );
        assert!(
            !real_calls(&calls)
                .iter()
                .any(|call| call.contains("provision") || call.contains("network create")),
            "nothing sets the network up"
        );
    }
}

/// At a terminal, each purchase waits for an answer: a `y` buys it, anything else skips it
/// and every step that needs it, and makes no ledger line for it.
#[test]
fn a_purchase_runs_only_after_a_y_at_the_terminal() {
    let home = Home::new();
    // A readable, free Drive subscription, so the Drive plan is asked about.
    let (wrapper, _) = scripted_hipc(&home, "", &[FREE_DRIVE]);
    let (master, terminal) = pty();
    let log = home.path().join("manual_test.log");
    let mut command = Command::new(repo().join("scripts/manual_test.sh"));
    for name in ["HIPPIUS_TOKEN", "HIPPIUS_API_URL", "CI", "AI_AGENT"] {
        command.env_remove(name);
    }
    let tmp = home.path().join("tmp");
    std::fs::create_dir_all(&tmp).expect("create TMPDIR");
    command
        .env("HOME", home.path())
        .env("TMPDIR", &tmp)
        .current_dir(home.path())
        .args(["--offline", "--tier", "spend", "--hipc"])
        .arg(&wrapper)
        .arg("--log")
        .arg(&log)
        .args(["drive", "support"]);
    // Typed ahead: the terminal holds each line until the script reads it. The Drive plan
    // is declined, the ticket bought.
    let mut keyboard = File::from(master);
    keyboard.write_all(b"no\ny\n").expect("type the answers");
    let output = bounded(&mut command, Stdio::from(terminal), Some(&log));
    let log = std::fs::read_to_string(&log).expect("the log is written");
    // Only the stubbed reads, which answer where offline expects `unavailable`, fail.
    assert_eq!(output.status.code(), Some(1), "{log}");
    assert!(
        failed_titles(&log)
            .iter()
            .all(|title| title.starts_with("Read the Drive subscription")),
        "{log}"
    );
    assert!(
        log.contains("answer: no; the steps that need it are skipped"),
        "{log}"
    );
    assert!(log.contains("answer: y\n"), "{log}");
    let step = |title: &str| {
        steps(&log)
            .find(|step| step.contains(title))
            .unwrap_or_else(|| panic!("no step {title}: {log}"))
            .to_owned()
    };
    assert!(
        step("Subscribe to Starter (spend)").contains("SKIP: the purchase was declined"),
        "{log}"
    );
    assert!(
        step("Cancel the plan (undo)").contains("SKIP: the subscribe did not exit 0"),
        "{log}"
    );
    assert!(
        step("Open a ticket (spend)").contains("expected error code unavailable: PASS"),
        "{log}"
    );
    assert!(!log.contains("cleanup: drive-sub"), "{log}");
    assert!(log.contains("cleanup: ticket hipc-manual-"), "{log}");
}

/// A pseudo-terminal: the side the script sees as its stdin, and the side the test types
/// into.
fn pty() -> (OwnedFd, File) {
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).expect("open a pty");
    grantpt(&master).expect("grant the pty");
    unlockpt(&master).expect("unlock the pty");
    let name = ptsname(&master, Vec::new()).expect("the pty has a name");
    let terminal = OpenOptions::new()
        .read(true)
        .write(true)
        .open(name.to_str().expect("a pty name is UTF-8"))
        .expect("open the terminal side of the pty");
    (master, terminal)
}

/// The global flags that take a value, and those that do not; the guard checks both lists
/// against `hipc schema`, so a new global flag is stripped too.
const GLOBAL_VALUED: [&str; 7] = [
    "--output",
    "--profile",
    "--api-url",
    "--account",
    "--project",
    "--billing",
    "--request-timeout",
];
const GLOBAL_BARE: [&str; 6] = [
    "--json",
    "--yes",
    "--no-input",
    "--dry-run",
    "--debug",
    "--quiet",
];

/// The tags of the steps that run a command: `ext`, `setup`, and `check` lines run none,
/// and an `[ask]` line is a question.
const RUN_TAGS: [&str; 7] = [
    "[read]",
    "[write]",
    "[undo]",
    "[spend]",
    "[dry-run]",
    "[local]",
    "[error]",
];

/// The command path a `--list` line runs: the words after `hipc`, with every global flag
/// (and its value) taken out, up to the longest prefix that is a command of the schema.
/// None for a line that is not a step that runs one (another program, a check, a question,
/// `hipc --help`).
fn listed_path<'a>(line: &str, paths: &'a BTreeSet<String>) -> Option<&'a String> {
    let (tag, rest) = line.trim_start().split_once(' ')?;
    if !RUN_TAGS.contains(&tag) {
        return None;
    }
    let mut words = rest
        .split_whitespace()
        .skip_while(|word| word.contains('=') && !word.starts_with('-'));
    if words.next()? != "hipc" {
        return None;
    }
    let mut command = Vec::new();
    while let Some(word) = words.next() {
        let name = word.split_once('=').map_or(word, |(name, _)| name);
        if GLOBAL_VALUED.contains(&name) {
            if !word.contains('=') {
                words.next();
            }
        } else if !GLOBAL_BARE.contains(&word) {
            command.push(word);
        }
    }
    (1..=command.len())
        .rev()
        .find_map(|n| paths.get(&command[..n].join(" ")))
}

/// Every command of `hipc schema` is run by some tier of the script, as itself or as a
/// `--dry-run`, or is named in its EXCLUDED list with a reason; an excluded command is a
/// command of the schema, and no tier runs it. So a new command fails this test until the
/// script runs it or says why not.
#[test]
fn every_command_is_run_by_a_tier_or_excluded_with_a_reason() {
    let schema = Command::new(env!("CARGO_BIN_EXE_hipc"))
        .arg("schema")
        .output()
        .expect("run hipc schema");
    let schema: serde_json::Value = serde_json::from_slice(&schema.stdout).expect("JSON");
    let paths: BTreeSet<String> = schema["commands"]
        .as_array()
        .expect("the schema lists commands")
        .iter()
        .map(|command| {
            command["path"]
                .as_array()
                .expect("a path is an array")
                .iter()
                .map(|word| word.as_str().expect("a path word is a string"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    let globals = schema["global_flags"]
        .as_array()
        .expect("the schema lists the global flags");
    let names = |valued: bool| -> BTreeSet<&str> {
        globals
            .iter()
            .filter(|flag| flag["value_name"].is_null() != valued)
            .map(|flag| flag["name"].as_str().expect("a flag name"))
            .collect()
    };
    assert_eq!(names(true), GLOBAL_VALUED.into_iter().collect());
    assert_eq!(names(false), GLOBAL_BARE.into_iter().collect());
    let home = Home::new();
    let mut run = BTreeSet::new();
    for tier in ["read", "write", "spend"] {
        // With the binary named, the listing shows the steps that need a newer command.
        let output = manual_test(
            &home,
            &[
                "--list",
                "--tier",
                tier,
                "--hipc",
                env!("CARGO_BIN_EXE_hipc"),
            ],
        );
        assert!(output.status.success(), "{tier}");
        run.extend(
            stdout(&output)
                .lines()
                .filter_map(|line| listed_path(line, &paths))
                .cloned(),
        );
    }
    let excluded = manual_test(&home, &["--excluded"]);
    assert!(excluded.status.success());
    let mut reasons = BTreeSet::new();
    for line in stdout(&excluded).lines() {
        let (path, reason) = line
            .split_once(": ")
            .expect("an exclusion is `path: reason`");
        assert!(!reason.trim().is_empty(), "{path} has no reason");
        assert!(
            paths.contains(path),
            "{path} is excluded but not in the schema"
        );
        assert!(!run.contains(path), "{path} is excluded but a tier runs it");
        reasons.insert(path.to_owned());
    }
    let missing: Vec<&String> = paths
        .iter()
        .filter(|path| !run.contains(*path) && !reasons.contains(*path))
        .collect();
    assert!(
        missing.is_empty(),
        "scripts/manual_test.sh neither runs nor excludes: {missing:?}"
    );
}

/// The guard's parser reads a step line as the script prints it.
#[test]
fn a_listed_line_names_its_command_path() {
    let paths: BTreeSet<String> = ["compute vms list", "compute vms", "api", "status"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    for (line, path) in [
        (
            "  [read] hipc --output text compute vms list --all",
            Some("compute vms list"),
        ),
        (
            "  [read] AI_AGENT=1 hipc compute vms list",
            Some("compute vms list"),
        ),
        ("  [read] hipc --json --debug api GET /api/x/", Some("api")),
        ("  [read] hipc --output text status", Some("status")),
        (
            "  [dry-run] hipc --yes --account acct_1 --profile=p --quiet compute vms list",
            Some("compute vms list"),
        ),
        ("  [setup] hipc --output text status", None),
        ("  [ext] hipc status", None),
        ("  [ask] hipc status: a question", None),
        (
            "  [ext] aws --profile hipc-manual s3 ls s3://<bucket>",
            None,
        ),
        ("  [local] hipc --output text --help", None),
        ("  [check] the answer is JSON", None),
    ] {
        assert_eq!(
            listed_path(line, &paths).map(String::as_str),
            path,
            "{line}"
        );
    }
}

/// The read tier, offline: nothing is created, so the ledger stays empty.
#[test]
fn an_offline_read_run_writes_nothing() {
    let home = Home::new();
    let (output, log) = offline(&home, env!("CARGO_BIN_EXE_hipc"), &[]);
    assert_clean_offline_run(&output, &log, &home);
    assert!(!log.contains(" (write)\n"), "{log}");
    assert!(!log.contains(" (undo)\n"), "{log}");
    assert!(log.contains("cleanup: the ledger is empty"));
}

#[test]
fn a_scenario_filter_runs_only_those() {
    let home = Home::new();
    let (output, log) = offline(&home, env!("CARGO_BIN_EXE_hipc"), &["agent"]);
    assert_clean_offline_run(&output, &log, &home);
    assert_eq!(log.matches("######## scenario ").count(), 1);
    assert!(log.contains("######## scenario agent: "));
}

/// A binary that echoes stdin and exits 0 for everything: the refusals it should have
/// printed fail, the run exits 1 and lists them, and the token it was handed on stdin at
/// login is masked in the log.
#[test]
fn failed_steps_fail_the_run_and_the_mask_hides_an_echoed_token() {
    let home = Home::new();
    let fake = fake_hipc(&home, "cat");
    let (output, log) = offline(&home, fake.to_str().unwrap(), &["first-run"]);
    assert_eq!(output.status.code(), Some(1), "{log}");
    assert!(log.contains("\nfailures:\n"), "{log}");
    assert!(log.contains("expected error code auth: FAIL"), "{log}");
    assert!(log.contains("<token>"), "{log}");
    assert!(!log.contains(OFFLINE_TOKEN), "{log}");
}

/// A secret with a line break, a quote and a slash, as a JSON answer prints it (escaped)
/// on stdout, is masked in every form; so is a secret that only an error on stderr holds;
/// and the log scan finds nothing.
#[test]
fn a_secret_is_masked_in_its_json_form_and_line_by_line() {
    let home = Home::new();
    let out = r#"{"secret":"line-one-aaaa\nline-two-bbbb\"q/x"}"#;
    let err = r#"{"error":{"code":"internal","details":{"password":"stderr-only-cccc"}}}"#;
    // printf, not echo: some sh builtins' echo would turn the `\n` into a line break.
    let fake = fake_hipc(
        &home,
        &format!("printf '%s\\n' '{out}'; printf '%s\\n' '{err}' >&2"),
    );
    let (_, log) = offline(&home, fake.to_str().unwrap(), &["first-run"]);
    assert!(!log.contains("line-one-aaaa"), "{log}");
    assert!(!log.contains("line-two-bbbb"), "{log}");
    assert!(!log.contains("stderr-only-cccc"), "{log}");
    assert!(log.contains(r#"{"secret":"<redacted>"}"#), "{log}");
    assert!(
        log.contains("check: the log holds neither the token nor a revealed secret: PASS"),
        "{log}"
    );
}

#[test]
fn bad_arguments_exit_2_before_anything_runs() {
    let home = Home::new();
    for (args, phrase) in [
        (&["nosuch"][..], "unknown scenario nosuch"),
        (&["--tier", "subscribe"], "--tier is read, write or spend"),
        (&["--tier"], "--tier needs read, write or spend"),
        // `output()` gives the script no terminal on stdin, so it cannot ask.
        (
            &["--offline", "--tier", "spend"],
            "stdin is not a terminal; pass --yes-spend",
        ),
        (
            &["--offline", "--hipc", "/nonexistent/hipc"],
            "is not an executable file",
        ),
        (&["--bogus"], "unknown option --bogus"),
        (&["--offline"], "--offline needs --hipc <path>"),
        (&["--hipc", env!("CARGO_BIN_EXE_hipc")], "set HIPPIUS_TOKEN"),
    ] {
        let output = manual_test(&home, args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(phrase), "{args:?}: {stderr}");
    }
    assert!(!home.path().join("manual_test.log").exists());
}

/// Tracing would print the token, so the script refuses to start under it.
#[test]
fn xtrace_is_refused() {
    let home = Home::new();
    let mut command = Command::new(repo().join("scripts/manual_test.sh"));
    command
        .env("HOME", home.path())
        .env("SHELLOPTS", "xtrace")
        .env("HIPPIUS_TOKEN", OFFLINE_TOKEN)
        .args(["--offline", "--hipc", env!("CARGO_BIN_EXE_hipc")])
        .current_dir(home.path());
    let output = bounded(&mut command, Stdio::null(), None);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("refusing to run with xtrace"), "{stderr}");
    assert!(!stderr.contains(OFFLINE_TOKEN), "{stderr}");
}

/// The jq definitions the script keeps in the shell variable `name`, between its `'` quotes.
fn script_jq(name: &str) -> String {
    let script = std::fs::read_to_string(repo().join("scripts/manual_test.sh")).expect("read");
    script
        .split_once(&format!("\n{name}='"))
        .and_then(|(_, rest)| rest.split_once("\n'\n"))
        .map_or_else(
            || panic!("the script defines {name}"),
            |(defs, _)| defs.to_owned(),
        )
}

/// Runs `jq -c <filter>` on `input`, and returns what it printed.
fn jq(filter: &str, input: &str) -> String {
    let mut jq = Command::new("jq")
        .arg("-c")
        .arg(filter)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("run jq");
    jq.stdin
        .take()
        .expect("jq's stdin")
        .write_all(input.as_bytes())
        .expect("write the input");
    let output = jq.wait_with_output().expect("wait for jq");
    assert!(output.status.success(), "{filter}: {input}");
    stdout(&output).to_owned()
}

/// An offline run, as `offline`, with the 425 retries 0 seconds apart.
fn offline_fast_retry(home: &Home, hipc: &Path, args: &[&str]) -> (Output, String) {
    let tmp = home.path().join("tmp");
    std::fs::create_dir_all(&tmp).expect("create TMPDIR");
    let log_path = home.path().join("manual_test.log");
    let mut command = Command::new(repo().join("scripts/manual_test.sh"));
    command
        .env("HOME", home.path())
        .env("TMPDIR", &tmp)
        .env("HIPC_MANUAL_RETRY_SECONDS", "0")
        .env_remove("HIPPIUS_TOKEN")
        .env_remove("HIPPIUS_API_URL")
        .env_remove("CI")
        .env_remove("AI_AGENT")
        .current_dir(home.path())
        .arg("--offline")
        .arg("--hipc")
        .arg(hipc)
        .arg("--log")
        .arg(&log_path)
        .args(args);
    let output = bounded(&mut command, Stdio::null(), Some(&log_path));
    let log = std::fs::read_to_string(&log_path).expect("the log is written");
    (output, log)
}

/// The calls a stand-in recorded that hold `what`, sent for real.
fn count_calls(calls: &Path, what: &str) -> usize {
    real_calls(calls)
        .iter()
        .filter(|call| call.contains(what))
        .count()
}

/// Masking, round 2: the secret fields of every command that takes `--reveal` are masked,
/// among them a Kubernetes backup listing's `manifest.sealed_key` and signed download link,
/// a database's credentials, and the tokens and setup keys `hipc api --reveal` can print.
#[test]
fn every_reveal_secret_field_is_masked() {
    let home = Home::new();
    let out = concat!(
        r#"{"snapshots":[{"name":"20261027T120000Z-m0","#,
        r#""manifest":{"name":"20261027T120000Z-m0","#,
        r#""sealed_key":"c2VhbGVkLWtleS1vZi10aGUtc25hcHNob3Q=","sha256":"00ff"},"#,
        r#""download_url":"https://s3.hippius.com/b/e.enc?X-Amz-Expires=900&"#,
        r#"X-Amz-Signature=5e3c0ffee"}],"#,
        r#""credentials":{"password":"db-password-0011","#,
        r#""backup_cipher_passphrase":"db-pass-0012","#,
        r#""tls":{"client_key":"db-client-key-0013"}},"robot_secret":"robot-secret-0014","#,
        r#""access_token":"access-0015","refresh_token":"refresh-0016","setup_key":"setup-0017","#,
        r#""join_command":"netbird up --setup-key setup-0017","token":"master-0018"}"#
    );
    let fake = fake_hipc(&home, &format!("printf '%s\\n' '{out}'"));
    let (_, log) = offline(&home, fake.to_str().unwrap(), &["first-run"]);
    for secret in [
        "c2VhbGVkLWtleS1vZi10aGUtc25hcHNob3Q=",
        "5e3c0ffee",
        "db-password-0011",
        "db-pass-0012",
        "db-client-key-0013",
        "robot-secret-0014",
        "access-0015",
        "refresh-0016",
        "setup-0017",
        "master-0018",
    ] {
        assert!(!log.contains(secret), "{secret}: {log}");
    }
    // The snapshot's name and checksum are not secrets.
    assert!(log.contains(r#""sha256":"00ff""#), "{log}");
    assert!(
        log.contains("check: the log holds neither the token nor a revealed secret: PASS"),
        "{log}"
    );
}

/// Masking, round 2: the literal rules run longest first, so a password that is a prefix
/// of the passphrase and of the client key cannot replace its part of them first and leave
/// the rest in the log.
#[test]
fn a_password_that_starts_the_passphrase_and_the_key_does_not_shred_them() {
    let home = Home::new();
    let out = concat!(
        r#"{"password":"pq7","backup_cipher_passphrase":"pq7-passphrase-0004","#,
        r#""client_key":"pq7-client-key-0005"}"#
    );
    let fake = fake_hipc(&home, &format!("printf '%s\\n' '{out}'"));
    let (_, log) = offline(&home, fake.to_str().unwrap(), &["first-run"]);
    for part in ["pq7", "-passphrase-0004", "-client-key-0005"] {
        assert!(!log.contains(part), "{part}: {log}");
    }
    assert!(
        log.contains(concat!(
            r#"{"password":"<redacted>","backup_cipher_passphrase":"<redacted>","#,
            r#""client_key":"<redacted>"}"#
        )),
        "{log}"
    );
    assert!(
        log.contains("check: the log holds neither the token nor a revealed secret: PASS"),
        "{log}"
    );
}

/// Masking, round 2: the log scan also looks for the parts of a secret around a shorter
/// secret inside it, so a part left over by a short secret's rule fails the run, even when
/// neither whole secret is in the log.
#[test]
fn the_log_scan_finds_the_rest_of_a_secret_a_shorter_one_broke_up() {
    let home = Home::new();
    let out = r#"{"password":"pq7","backup_cipher_passphrase":"pq7-passphrase-0004"}"#;
    // Every call prints the secrets as JSON, and the `auth status` reads also print, in text,
    // what a mask that ran `pq7` first would have left of the passphrase.
    let fake = fake_hipc(
        &home,
        &format!(
            "printf '%s\\n' '{out}'\ncase \" $* \" in *\" auth status \"*) \
             printf '%s\\n' 'left: -passphrase-0004' ;; esac"
        ),
    );
    let (output, log) = offline(&home, fake.to_str().unwrap(), &["first-run"]);
    assert_eq!(output.status.code(), Some(1), "{log}");
    assert!(
        log.contains("check: the log holds the token or a revealed secret on "),
        "{log}"
    );
    // The report names the needle's source and the log lines, and never the needle.
    let report: Vec<&str> = log
        .lines()
        .filter(|line| line.starts_with("scan: needle "))
        .collect();
    assert!(
        report.iter().any(|line| line.contains("a part of needle")
            && line.contains("field backup_cipher_passphrase")
            && line.contains("16 bytes) is on ")),
        "{log}"
    );
    let at_least_one_line = log
        .lines()
        .position(|line| line == "left: -passphrase-0004")
        .expect("the leaked part is in the log")
        + 1;
    assert!(
        report
            .iter()
            .any(|line| line.contains(&format!(": {at_least_one_line},"))),
        "{report:?}"
    );
    for line in &report {
        assert!(
            !line.contains("passphrase-0004") && !line.contains("pq7"),
            "{line}"
        );
    }
}

/// Masking, round 2: a password or a passphrase in a URL's query is masked: the whole URL
/// under a `url` or `uri` key, and the value alone in any other line.
#[test]
fn a_password_in_a_query_string_is_masked() {
    let home = Home::new();
    let out = r#"{"url":"https://h.example/x?user=a&password=qp-url-0006"}"#;
    let text = concat!(
        "see https://h.example/y?passphrase=qp-text-0007&x=1 and ?PASSWORD=qp-text-0008",
        " &Pwd=qp-text-0009 ?SECRET=qp-text-0010 ;token=qp-text-0011 &pass=qp-text-0012",
        " &PassWord=qp-text-0013"
    );
    let fake = fake_hipc(
        &home,
        &format!("printf '%s\\n' '{out}'; printf '%s\\n' '{text}'"),
    );
    let (_, log) = offline(&home, fake.to_str().unwrap(), &["first-run"]);
    for secret in [
        "qp-url-0006",
        "qp-text-0007",
        "qp-text-0008",
        "qp-text-0009",
        "qp-text-0010",
        "qp-text-0011",
        "qp-text-0012",
        "qp-text-0013",
    ] {
        assert!(!log.contains(secret), "{secret}: {log}");
    }
    assert!(log.contains(r#"{"url":"<redacted>"}"#), "{log}");
    assert!(
        log.contains("see https://h.example/y?passphrase=<redacted>&x=1"),
        "{log}"
    );
    assert!(
        log.contains("check: the log holds neither the token nor a revealed secret: PASS"),
        "{log}"
    );
}

/// Drive, round 2: the Starter question is asked only on a free state the script
/// recognises; every other shape is taken for a paid plan.
#[test]
fn only_a_recognised_free_drive_state_may_be_bought_over() {
    let defs = script_jq("DRIVE_CANCELLED");
    let shapes = [
        (r#"{"plan":"free","active":true}"#, true),
        (r#"{"plan_code":"FREE"}"#, true),
        (r#"{"plan":"free","plan_code":"free","plan_id":null}"#, true),
        (r#"{"active":false}"#, true),
        (
            concat!(
                r#"{"active":false,"free_plan":{"available":false,"#,
                r#""unavailable_reason":"wallet_only_account"}}"#
            ),
            true,
        ),
        (r#"{"error":"not_found"}"#, true),
        (r#"{"plan":{"code":"free"}}"#, false),
        (
            r#"{"plan":null,"plan_name":"Starter","active":false}"#,
            false,
        ),
        (r#"{"plan":"free","plan_id":"p_solo"}"#, false),
        (r#"{"active":false,"plan_id":7}"#, false),
        (r#"{"active":false,"subscription_id":12}"#, false),
        (r#"{"plan":"solo","active":false}"#, false),
        (r#"{"plan":"free","plan_code":"solo"}"#, false),
        (
            r#"{"active":false,"status":null,"plan":null,"plan_name":null}"#,
            true,
        ),
        (r#"{"active":false,"status":"past_due"}"#, false),
        (
            r#"{"active":false,"current_period_end":"2026-12-01T00:00:00Z"}"#,
            false,
        ),
        (r#"{"active":false,"funding":"credits"}"#, false),
        (
            r#"{"active":false,"free_plan":{"available":true},"provider":"credits"}"#,
            false,
        ),
        ("{}", false),
        (r#"{"error":"unavailable"}"#, false),
        (r#""free""#, false),
    ];
    for (shape, free) in shapes {
        assert_eq!(
            jq(&format!("{defs} drive_free"), shape),
            format!("{free}\n"),
            "{shape}"
        );
    }
}

/// Drive, round 2: a subscription read that answers `not_found` (no subscription at all) is
/// a free state, so the plan is asked about and bought; a shape that is not recognised is
/// logged, and nothing is asked or bought.
#[test]
fn a_missing_drive_subscription_is_bought_over_and_an_unknown_one_is_not() {
    let home = Home::new();
    let missing = concat!(
        "printf '%s\\n' '{\"error\":{\"code\":\"not_found\",\"message\":\"no subscription\",",
        "\"http_status\":404}}' >&2; exit 1"
    );
    let (wrapper, calls) = scripted_hipc(&home, "", &[("*\" drive subscription get \"*", missing)]);
    let (_, log) = offline(
        &home,
        wrapper.to_str().expect("UTF-8"),
        &["--tier", "spend", "--yes-spend", "drive"],
    );
    assert!(log.contains("answer: y (--yes-spend)"), "{log}");
    assert_eq!(
        count_calls(&calls, "drive subscription subscribe"),
        1,
        "{log}"
    );

    let home = Home::new();
    let paid = "printf '%s\\n' '{\"plan\":null,\"plan_name\":\"Starter\",\"active\":true}'; exit 0";
    let (wrapper, calls) = scripted_hipc(&home, "", &[("*\" drive subscription get \"*", paid)]);
    let (_, log) = offline(
        &home,
        wrapper.to_str().expect("UTF-8"),
        &["--tier", "spend", "--yes-spend", "drive"],
    );
    assert!(
        log.contains(concat!(
            "drive: nothing is bought; the subscription read answered ",
            r#"{"plan":null,"plan_name":"Starter","active":true}"#
        )),
        "{log}"
    );
    assert!(!log.contains("\ncost: "), "{log}");
    assert_eq!(
        count_calls(&calls, "drive subscription subscribe"),
        0,
        "{log}"
    );
    assert_eq!(count_calls(&calls, "drive subscription cancel"), 0, "{log}");
}

/// Drive, round 2: only a real cancelled shape counts as cancelled; a cancel date on an
/// active, paid plan does not, nor does a status that merely holds the word.
#[test]
fn only_a_real_cancelled_drive_shape_counts_as_cancelled() {
    let defs = script_jq("DRIVE_CANCELLED");
    let shapes = [
        (r#"{"status":"cancelled","plan":"solo"}"#, true),
        (r#"{"status":"Canceled","plan":"solo"}"#, true),
        (r#"{"status":"cancel_submitted","plan":"solo"}"#, true),
        (
            r#"{"cancel_at_period_end":true,"plan":"solo","active":true}"#,
            true,
        ),
        (r#"{"plan":"free"}"#, true),
        (r#"{"error":"not_found"}"#, true),
        (r#"{"status":"not_cancelled","plan":"solo"}"#, false),
        (r#"{"status":"cancel_failed","plan":"solo"}"#, false),
        (
            r#"{"cancelled_at":"2026-11-01T00:00:00Z","active":true,"plan":"solo"}"#,
            false,
        ),
        (
            r#"{"canceled_at":"2026-11-01T00:00:00Z","plan_code":"solo"}"#,
            false,
        ),
        (
            r#"{"cancel_at_period_end":false,"status":"active","plan":"solo"}"#,
            false,
        ),
    ];
    for (shape, cancelled) in shapes {
        assert_eq!(
            jq(&format!("{defs} drive_cancelled"), shape),
            format!("{cancelled}\n"),
            "{shape}"
        );
    }
}

/// Drive, round 2: a cancel that exited 0 in the scenario marks its ledger line
/// `cancelled`, and the exit trap only reads the plan again: no second cancel is sent.
#[test]
fn a_cancel_that_exited_0_is_never_sent_again() {
    let home = Home::new();
    let (wrapper, calls) = scripted_hipc(
        &home,
        "",
        &[
            FREE_DRIVE,
            (
                "*\" drive subscription subscribe \"*",
                "printf '%s\\n' '{}'; exit 0",
            ),
            (
                "*\" drive subscription cancel \"*",
                "printf '%s\\n' '{}'; exit 0",
            ),
        ],
    );
    let (_, log) = offline_fast_retry(
        &home,
        &wrapper,
        &["--tier", "spend", "--yes-spend", "drive"],
    );
    assert_eq!(count_calls(&calls, "drive subscription cancel"), 1, "{log}");
    assert!(
        log.contains(
            "cleanup: drive-sub solo: the cancel exited 0 in the scenario; reading it again"
        ),
        "{log}"
    );
    assert!(log.contains("cleanup: drive-sub solo: undone"), "{log}");
}

/// Drive, round 2: the exit trap's cancel of a `created` line retries a 425
/// (`still_confirming`) as the scenario's does.
#[test]
fn the_trap_retries_a_425_cancel() {
    let home = Home::new();
    let count = home.path().join("cancels");
    // Nine 425s, then a cancel that works: the scenario's seven tries all see 425, and the
    // trap's third try works.
    let confirming = format!(
        concat!(
            "n=$(cat '{0}' 2>/dev/null || echo 0); echo $((n + 1)) >'{0}'; ",
            "if [ \"$n\" -lt 9 ]; then printf '%s\\n' '{{\"error\":{{\"code\":\"conflict\",",
            "\"message\":\"still confirming\",\"http_status\":425,",
            "\"details\":{{\"code\":\"still_confirming\"}}}}}}' >&2; exit 1; fi; ",
            "printf '%s\\n' '{{}}'; exit 0"
        ),
        count.display()
    );
    let (wrapper, calls) = scripted_hipc(
        &home,
        "",
        &[
            FREE_DRIVE,
            (
                "*\" drive subscription subscribe \"*",
                "printf '%s\\n' '{}'; exit 0",
            ),
            ("*\" drive subscription cancel \"*", &confirming),
        ],
    );
    let (_, log) = offline_fast_retry(
        &home,
        &wrapper,
        &["--tier", "spend", "--yes-spend", "drive"],
    );
    assert_eq!(
        count_calls(&calls, "drive subscription cancel"),
        10,
        "{log}"
    );
    assert_eq!(
        log.matches("cleanup: 425: still confirming; try ").count(),
        2,
        "{log}"
    );
    assert!(log.contains("cleanup: drive-sub solo: undone"), "{log}");
}

/// Matching, round 2: a 403 that is not the unprovisioned network stays a failure, even when
/// its hint names `network create` (a role that may not read the network).
#[test]
fn a_forbidden_network_read_whose_hint_names_the_create_is_a_failure() {
    let home = Home::new();
    let answer = concat!(
        r#"{"error":{"code":"permission","message":"cannot list the peers: "#,
        r#"the API says `You do not have permission to perform this action.`","#,
        r#""hint":"ask an owner of the account to run `hipc network create --yes`","#,
        r#""http_status":403,"details":{"detail":"You do not have permission to perform "#,
        r#"this action."}}}"#
    );
    let refused = format!("printf '%s\\n' '{answer}' >&2; exit 1");
    let (wrapper, _) = scripted_hipc(&home, "", &[("*\" network peers list \"*", &refused)]);
    let (output, log) = offline(
        &home,
        wrapper.to_str().expect("UTF-8"),
        &["network-kubernetes"],
    );
    assert_eq!(output.status.code(), Some(1), "{log}");
    // The stand-in's answer reached the step whole, so its code is what was judged.
    assert!(
        log.contains("\"hint\":\"ask an owner of the account to run `hipc network create --yes`\""),
        "{log}"
    );
    assert!(
        failed_titles(&log).contains(&"List the peers".to_owned()),
        "{log}"
    );
    assert!(
        !log.contains("note: this account's private network is not set up"),
        "{log}"
    );
}

/// Matching, round 2: `revoked` reads the status from its start, so `not_deleted` is live,
/// and a status that names a failure is never gone: the resource may still run and bill.
#[test]
fn a_failed_or_negated_status_is_not_gone() {
    let defs = script_jq("JQ_DEFS");
    let list = r#"{"items":[
        {"name":"a","status":"deleting"},
        {"name":"b","status":"Deleted"},
        {"name":"c","status":"terminated"},
        {"name":"d","status":"revoked"},
        {"name":"e","status":"archived"},
        {"name":"f","status":"decommissioned"},
        {"name":"g","status":"delete_failed"},
        {"name":"h","status":"termination_failed"},
        {"name":"i","status":"decommission_failed"},
        {"name":"j","status":"not_deleted"},
        {"name":"k","status":"undeleted"},
        {"name":"l","status":"running"}],"next_page_token":null}"#;
    assert_eq!(
        jq(&format!("{defs} live_names"), list),
        "[\"g\",\"h\",\"i\",\"j\",\"k\",\"l\"]\n"
    );
}

/// The App Engine launch is a dry run in every tier, with a 0600 secrets file whose value
/// the mask knows from the start: the dry run shows its placeholder, and a binary that
/// printed the value would fail both checks, with the value still masked in the log.
#[test]
fn the_app_launch_dry_run_shows_a_placeholder_and_never_the_secret() {
    for tier in ["read", "write"] {
        let listing = stdout(&manual_test(&Home::new(), &["--list", "--tier", tier])).to_owned();
        assert!(
            listing.contains(
                "[dry-run] hipc --json compute app-engine apps create --flavor small \
                 --compose-file ~/compose.yml --secrets-file ~/app-secrets.json \
                 --name hipc-manual --dry-run"
            ),
            "{listing}"
        );
    }
    let home = Home::new();
    let (output, log) = offline(&home, env!("CARGO_BIN_EXE_hipc"), &["compute"]);
    assert_clean_offline_run(&output, &log, &home);
    assert!(
        log.contains("check: the dry run shows a placeholder for the secret: PASS"),
        "{log}"
    );
    assert!(
        log.contains("check: the dry run does not show the secret: PASS"),
        "{log}"
    );
    assert!(!log.contains("hipc-manual-app-secret-"), "{log}");

    // A binary that prints the secrets file instead.
    let home = Home::new();
    let (wrapper, _) = scripted_hipc(
        &home,
        concat!(
            "case \" $* \" in *\" apps create \"*) for a; do case \"$a\" in ",
            "*app-secrets.json) cat \"$a\" ;; esac; done; exit 0 ;; esac"
        ),
        &[],
    );
    let (output, log) = offline(&home, wrapper.to_str().expect("UTF-8"), &["compute"]);
    assert_eq!(output.status.code(), Some(1), "{log}");
    assert_eq!(
        failed_titles(&log),
        [
            "check: the dry run shows a placeholder for the secret",
            "check: the dry run does not show the secret"
        ],
        "{log}"
    );
    assert!(log.contains(r#"{"DB_PASSWORD":"<app-secret>"}"#), "{log}");
    assert!(!log.contains("hipc-manual-app-secret-"), "{log}");
}

/// Drive, round 3: an inactive answer that still carries a status, a period end or a
/// funding source may be a plan the account pays for, so nothing is asked or bought, and
/// the answer is logged.
#[test]
fn an_inactive_drive_answer_with_more_to_it_is_not_bought_over() {
    for answer in [
        r#"{"active":false,"status":"past_due"}"#,
        r#"{"active":false,"current_period_end":"2026-12-01T00:00:00Z","funding":"credits"}"#,
    ] {
        let home = Home::new();
        let read = format!("printf '%s\\n' '{answer}'; exit 0");
        let (wrapper, calls) =
            scripted_hipc(&home, "", &[("*\" drive subscription get \"*", &read)]);
        let (_, log) = offline(
            &home,
            wrapper.to_str().expect("UTF-8"),
            &["--tier", "spend", "--yes-spend", "drive"],
        );
        assert!(
            log.contains(&format!(
                "drive: nothing is bought; the subscription read answered {answer}"
            )),
            "{log}"
        );
        assert!(!log.contains("\ncost: "), "{log}");
        assert_eq!(
            count_calls(&calls, "drive subscription subscribe"),
            0,
            "{log}"
        );
    }
}

/// Drive, round 3: the snapshot keeps the Drive subscription whole, so the teardown's
/// cancelled check reads every key `drive_free` reads, and the diff still compares only the
/// plan fields, `plan_name` among them.
#[test]
fn the_snapshot_keeps_the_drive_subscription_whole() {
    let defs = script_jq("JQ_DEFS");
    let script = std::fs::read_to_string(repo().join("scripts/manual_test.sh")).expect("read");
    // The filter's closing quote ends its last line.
    let filter = script
        .split_once("\nSNAPSHOT_FILTER='")
        .and_then(|(_, rest)| rest.split_once("'\n"))
        .map(|(filter, _)| filter)
        .expect("the script defines SNAPSHOT_FILTER");
    let answer = r#"{"active":false,"current_period_end":"2026-12-01","plan_name":"Starter"}"#;
    let kept = jq(
        &format!("{defs} \"drive-subscription\" as $name | {filter}"),
        answer,
    );
    assert_eq!(kept, format!("{answer}\n"));
    // Whole, the answer is not cancelled; reduced to the old fields it would have read free.
    let cancelled = format!("{} drive_cancelled", script_jq("DRIVE_CANCELLED"));
    assert_eq!(jq(&cancelled, answer), "false\n");
    assert_eq!(jq(&cancelled, r#"{"active":false}"#), "true\n");
    // The diff still compares plan_name.
    assert!(script.contains(r#""plan_id", "plan_code", "plan_name", "period""#));
}

/// Round 3: a kept ledger lists what may be left on the server first, and the lines that
/// name nothing to delete (a Drive subscribe that never exited 0, a ticket with no id) under
/// their own heading, commented out.
#[test]
fn the_kept_ledger_sets_apart_what_names_nothing_to_delete() {
    let home = Home::new();
    let vm = "0f8fad5b-d9cb-469f-a165-424242424242";
    let vm_answer = format!("printf '%s\\n' '{{\"vm_id\":\"{vm}\"}}'; exit 1");
    let refused = concat!(
        "printf '%s\\n' '{\"error\":{\"code\":\"conflict\",\"message\":\"no\",",
        "\"http_status\":409}}' >&2; exit 1"
    );
    let (wrapper, _) = scripted_hipc(
        &home,
        "",
        &[
            FREE_DRIVE,
            ("*\" drive subscription subscribe \"*", refused),
            ("*\" compute vms create \"*", &vm_answer),
        ],
    );
    let (output, log) = offline(
        &home,
        wrapper.to_str().expect("UTF-8"),
        &["--tier", "spend", "--yes-spend", "drive", "compute"],
    );
    assert_eq!(output.status.code(), Some(1), "{log}");
    let kept = std::fs::read_to_string(home.path().join("manual_test.log.ledger"))
        .expect("the ledger is kept beside the log");
    let (left, info) = kept
        .split_once("\n# nothing to delete, for information: ")
        .expect("the heading");
    assert!(left.contains(&format!(" {vm} created")), "{kept}");
    assert!(!left.contains("drive-sub"), "{kept}");
    assert!(info.contains("\n# drive-sub solo - pending"), "{kept}");
}

/// The harness's limits: a run that hangs, and a run that exits but leaves a process
/// holding its output open, each fail the test with what was left and the log's last lines,
/// and leave no process behind.
#[test]
fn a_hung_or_leaking_run_fails_fast_with_its_evidence() {
    let home = Home::new();
    let log = home.file("hung.log", b"step one\nstep two: the last line\n");
    let pid_file = home.path().join("pid");
    let cases = [
        ("exec sleep 60", "is still running after 1 seconds"),
        (
            "sleep 60 & echo $! >\"$0\"; exit 0",
            "exited, but a process it started still holds its output open",
        ),
    ];
    for (body, what) in cases {
        let mut command = Command::new("sh");
        command.args(["-c", body]).arg(&pid_file);
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            bounded_within(
                &mut command,
                Stdio::null(),
                Some(&log),
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
        }))
        .expect_err("the harness gives up");
        let message = panic.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(message.contains(what), "{message}");
        assert!(
            message.contains("sleep"),
            "the process left is named: {message}"
        );
        assert!(message.contains("step two: the last line"), "{message}");
    }
    // The leaked sleep was killed with its group: within a few seconds it is gone, or a
    // zombie its new parent has not reaped yet.
    let pid = std::fs::read_to_string(&pid_file).expect("the pid");
    let running = || {
        let state = Command::new("ps")
            .args(["-o", "stat=", "-p", pid.trim()])
            .output()
            .expect("run ps");
        let state = String::from_utf8_lossy(&state.stdout);
        !state.trim().is_empty() && !state.trim().starts_with('Z')
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while running() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!running(), "the leaked process is gone");
}

/// The final log scan stays fast on a big log: a run whose every answer is a 200 KB JSON
/// document with 20 secret fields writes a log of over 10 MB, and the scan of it takes a
/// few seconds. bash 3.2 (macOS) once spun for minutes in the scan's shell loops; on Linux
/// those loops took 12 seconds on this log, and the scan now takes about 1. The hosted macOS
/// runners of a public repository take up to 9 seconds for it, so macOS allows 30: still far
/// below the minutes of the old loops.
#[test]
fn the_log_scan_of_a_big_log_takes_seconds() {
    let home = Home::new();
    let mut answer = format!("{{\"blob\":\"{}\"", "a".repeat(200_000));
    for i in 0..20 {
        answer.push_str(&format!(
            ",\"password{i}\":\"perf-secret-{i:02}-0123456789\""
        ));
    }
    answer.push_str("}\n");
    let answer_file = home.file("answer.json", answer.as_bytes());
    let fake = fake_hipc(&home, &format!("cat '{}'", answer_file.display()));
    let (_, log) = offline(
        &home,
        fake.to_str().unwrap(),
        &["first-run", "account-billing", "hub"],
    );
    assert!(
        log.len() > 10_000_000,
        "the log is over 10 MB: {}",
        log.len()
    );
    for i in 0..20 {
        assert!(!log.contains(&format!("perf-secret-{i:02}-")), "{i}");
    }
    assert!(
        log.contains("check: the log holds neither the token nor a revealed secret: PASS"),
        "the scan ran and passed"
    );
    let seconds: u64 = log
        .split_once("scan: the log scan took ")
        .and_then(|(_, rest)| rest.split_once(" seconds"))
        .and_then(|(n, _)| n.parse().ok())
        .expect("the scan says how long it took");
    let limit = if cfg!(target_os = "macos") { 30 } else { 6 };
    assert!(
        seconds <= limit,
        "the scan took {seconds} seconds, over {limit}"
    );
}

/// Live write run, 2026-11-03: the S3 bucket list names its rows `bucket_name`, not `name`,
/// so a list that held the new bucket did not show it, and the delete's check passed for
/// any list. Both spellings are a row's name.
#[test]
fn a_bucket_list_names_its_rows_bucket_name() {
    let defs = script_jq("JQ_DEFS");
    let list = r#"{"count":3,"items":[{"bucket_id":"b","bucket_name":"hipc-manual-1"},
        {"id":"n","name":"other"},{"bucket_name":"gone","status":"deleted"}],
        "next_page_token":null}"#;
    assert_eq!(
        jq(&format!("{defs} live_names, complete"), list),
        "[\"hipc-manual-1\",\"other\"]\ntrue\n"
    );
    let script = std::fs::read_to_string(repo().join("scripts/manual_test.sh")).expect("read");
    assert!(
        script.contains(r#""bucket_id", "bucket_name","#),
        "the snapshot keeps them"
    );
}

/// Live write run: the server refuses a policy with no statement (`MalformedPolicy`), so the
/// write tier sets one Deny statement on a prefix the run never writes: valid, private, and
/// in the way of nothing. hipc's own checks accept it (its dry run passes in the read tier).
#[test]
fn the_bucket_policy_is_one_deny_statement_on_the_new_bucket() {
    let home = Home::new();
    let seen = home.path().join("policy-seen.json");
    let (wrapper, _) = scripted_hipc(
        &home,
        &format!(
            concat!(
                "case \" $* \" in *\" acl update \"*) ",
                "prev=; for a; do ",
                "[ \"$prev\" = --policy-file ] && cp \"$a\" '{}'; prev=$a; done ;; ",
                "esac"
            ),
            seen.display()
        ),
        &[],
    );
    let (output, log) = offline(
        &home,
        wrapper.to_str().expect("UTF-8"),
        &["--tier", "write", "s3"],
    );
    assert_clean_offline_run(&output, &log, &home);
    let policy: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&seen).expect("the policy was sent"))
            .expect("the policy is JSON");
    assert_eq!(policy["Version"], "2012-10-17");
    let statements = policy["Statement"].as_array().expect("statements");
    assert_eq!(statements.len(), 1, "{policy}");
    assert_eq!(statements[0]["Effect"], "Deny", "{policy}");
    assert_eq!(statements[0]["Principal"], "*", "{policy}");
    let resource = statements[0]["Resource"].as_str().expect("a resource");
    assert!(
        resource.starts_with("arn:aws:s3:::hipc-manual-")
            && resource.ends_with("/hipc-manual-denied/*"),
        "{resource}"
    );
}

/// Live write run: a 4-byte part threshold took ordinary text for a leak. A passphrase that
/// starts with the password leaves a short part (`-2026`) that a bucket name holds too, and
/// a key that ends with it a part (`2026-`) every timestamp holds; parts count from 8 bytes,
/// so that run passes.
#[test]
fn a_short_part_that_ordinary_text_holds_is_not_a_leak() {
    let home = Home::new();
    let out = concat!(
        r#"{"note":"made hipc-manual-20261103 at 2026-11-03","password":"zz1","#,
        r#""backup_cipher_passphrase":"zz1-2026","client_key":"2026-zz1"}"#
    );
    let fake = fake_hipc(&home, &format!("printf '%s\\n' '{out}'"));
    let (_, log) = offline(&home, fake.to_str().unwrap(), &["first-run"]);
    assert!(
        log.contains("made hipc-manual-20261103 at 2026-11-03"),
        "{log}"
    );
    assert!(!log.contains("zz1"), "{log}");
    assert!(
        log.contains("check: the log holds neither the token nor a revealed secret: PASS"),
        "{log}"
    );
}

/// Live write run: an S3 key's `secret_preview` is printed on purpose, in text tables too,
/// so it is not taken for a secret: a table that shows it before a JSON answer does is not a
/// leak.
#[test]
fn a_secret_preview_is_not_a_secret() {
    let home = Home::new();
    let fake = fake_hipc(
        &home,
        concat!(
            "case \" $* \" in *\" --output text \"*) ",
            "printf '%s\\n' 'kind  secret_preview' 'sub   pv-abcd1234' ;; ",
            "*) printf '%s\\n' '{\"items\":[{\"secret_preview\":\"pv-abcd1234\"}]}' ;; esac"
        ),
    );
    let (_, log) = offline(&home, fake.to_str().unwrap(), &["first-run"]);
    assert!(log.contains("sub   pv-abcd1234"), "{log}");
    assert!(
        log.contains("check: the log holds neither the token nor a revealed secret: PASS"),
        "{log}"
    );
}

/// The policy read-back passes only on the statement the run sent: a Deny of s3:GetObject on
/// the prefix the run never writes. The same Sid as an Allow, with a wider action, on another
/// resource, or beside a second statement fails it.
#[test]
fn the_policy_read_back_accepts_only_the_deny_statement() {
    let check = script_jq("POLICY_IS_DENY");
    let statement = |effect: &str, action: &str, resource: &str| {
        format!(r#"{{"Sid":"HipcManualDenyNothing","Effect":"{effect}","Principal":"*","#)
            + &format!(r#""Action":{action},"Resource":"{resource}"}}"#)
    };
    let deny = statement(
        "Deny",
        r#""s3:GetObject""#,
        "arn:aws:s3:::hipc-manual-1/hipc-manual-denied/*",
    );
    let doc = |statements: &str| {
        String::from(r#"{"bucket":"hipc-manual-1","policy":{"Version":"2012-10-17","#)
            + &format!(r#""Statement":[{statements}]}}}}"#)
    };
    let as_string = format!(
        r#"{{"policy":{}}}"#,
        serde_json::Value::String(format!(r#"{{"Statement":[{deny}]}}"#))
    );
    let cases = [
        (doc(&deny), true),
        (as_string, true),
        (
            doc(&statement(
                "Deny",
                r#"["s3:GetObject"]"#,
                "arn:aws:s3:::hipc-manual-1/hipc-manual-denied/*",
            )),
            true,
        ),
        (
            doc(&statement(
                "Allow",
                r#""s3:GetObject""#,
                "arn:aws:s3:::hipc-manual-1/hipc-manual-denied/*",
            )),
            false,
        ),
        (
            doc(&statement(
                "Deny",
                r#"["s3:GetObject","s3:PutObject"]"#,
                "arn:aws:s3:::hipc-manual-1/hipc-manual-denied/*",
            )),
            false,
        ),
        (
            doc(&statement(
                "Deny",
                r#""s3:*""#,
                "arn:aws:s3:::hipc-manual-1/*",
            )),
            false,
        ),
        (
            doc(&statement(
                "Deny",
                r#""s3:GetObject""#,
                "arn:aws:s3:::hipc-manual-1/*",
            )),
            false,
        ),
        (doc(&format!("{deny},{deny}")), false),
        (doc(""), false),
        (r#"{"policy":{}}"#.to_owned(), false),
    ];
    for (answer, accepted) in cases {
        assert_eq!(jq(&check, &answer), format!("{accepted}\n"), "{answer}");
    }
}

/// Live spend run: the WAF test takes a request with exactly ten keys, each given; the
/// request file holds exactly those.
#[test]
fn the_waf_test_request_has_exactly_the_ten_keys() {
    let home = Home::new();
    let seen = home.path().join("request-seen.json");
    let (wrapper, _) = scripted_hipc(
        &home,
        &format!(
            concat!(
                "case \" $* \" in *\" waf test \"*) prev=; for a; do ",
                "[ \"$prev\" = --request-file ] && cp \"$a\" '{}'; prev=$a; done ;; esac"
            ),
            seen.display()
        ),
        &[],
    );
    let (output, log) = offline(&home, wrapper.to_str().expect("UTF-8"), &["cdn"]);
    assert_clean_offline_run(&output, &log, &home);
    let request: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&seen).expect("the request was sent"))
            .expect("the request is JSON");
    let keys: BTreeSet<&str> = request
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    let expected: BTreeSet<&str> = [
        "ip", "country", "asn", "host", "method", "path", "query", "headers", "version", "ssl",
    ]
    .into_iter()
    .collect();
    assert_eq!(keys, expected);
    assert!(
        request["asn"].is_u64() && request["ssl"].is_boolean(),
        "{request}"
    );
    assert!(request["headers"].is_object(), "{request}");
}

/// Live spend run: a connection string or DSN is a secret only when it holds a password;
/// `postgresql://app@host:5432/app?sslmode=verify-ca` does not, and hipc prints it on
/// purpose. One with a password, in its user part or as `password=`, is still masked.
#[test]
fn a_connection_string_is_a_secret_only_with_a_password() {
    let home = Home::new();
    let out = concat!(
        r#"{"connection_string":"postgresql://app@db.example:5432/app?sslmode=verify-ca","#,
        r#""dsn":"host=db.example user=app dbname=app","#,
        r#""backup":{"connection_string":"postgresql://app:cs-pw-0021@db.example/app","#,
        r#""dsn":"host=db.example password=cs-pw-0022","#,
        r#""url":"postgres://db.example/app?pwd=cs-pw-0023","#,
        // An empty user, and the delimiters percent-encoded.
        r#""more":[{"connection_string":"postgresql://:cs-pw-0024@db.example/app"},"#,
        r#"{"connection_string":"postgresql://app:cs-pw-0025%40db.example/app"},"#,
        r#"{"connection_string":"postgresql://app%3Acs-pw-0026@db.example/app"},"#,
        r#"{"dsn":"postgresql://app@db.example/app?password%3Dcs-pw-0027"}]}}"#
    );
    let fake = fake_hipc(&home, &format!("printf '%s\\n' '{out}'"));
    let (_, log) = offline(&home, fake.to_str().unwrap(), &["first-run"]);
    assert!(
        log.contains("postgresql://app@db.example:5432/app?sslmode=verify-ca"),
        "{log}"
    );
    assert!(log.contains("host=db.example user=app dbname=app"), "{log}");
    for secret in [
        "cs-pw-0021",
        "cs-pw-0022",
        "cs-pw-0023",
        "cs-pw-0024",
        "cs-pw-0025",
        "cs-pw-0026",
        "cs-pw-0027",
    ] {
        assert!(!log.contains(secret), "{secret}: {log}");
    }
    assert!(
        log.contains("check: the log holds neither the token nor a revealed secret: PASS"),
        "{log}"
    );
}

/// Live spend run: the server makes a backup bucket and a writer token for a database and
/// keeps them after its delete; the run reports them as a check of their own and lists them
/// in the kept ledger, and the snapshot diff does not count them. The VM launch set up the
/// private network, so the peers read went from `network_unprovisioned` to no peers: allowed,
/// with a note.
#[test]
fn what_the_server_made_for_a_purchase_is_reported_apart() {
    let home = Home::new();
    let made = home.path().join("made");
    let vm_made = home.path().join("vm-made");
    let buckets = format!(
        concat!(
            "if [ -e '{0}' ]; then printf '%s\\n' '{{\"count\":2,\"items\":[",
            "{{\"bucket_id\":\"b1\",\"bucket_name\":\"mine\"}},",
            "{{\"bucket_id\":\"b2\",\"bucket_name\":\"hippius-db-224-backups\",",
            "\"is_managed\":true}}],\"next_page_token\":null}}'; else printf '%s\\n' ",
            "'{{\"count\":1,\"items\":[{{\"bucket_id\":\"b1\",\"bucket_name\":\"mine\"}}],",
            "\"next_page_token\":null}}'; fi; exit 0"
        ),
        made.display()
    );
    let tokens = format!(
        concat!(
            "if [ -e '{0}' ]; then printf '%s\\n' '{{\"items\":[{{\"id\":\"t2\",",
            "\"name\":\"db-224-backup-writer\"}}],\"next_page_token\":null}}'; ",
            "else printf '%s\\n' ",
            "'{{\"items\":[],\"next_page_token\":null}}'; fi; exit 0"
        ),
        made.display()
    );
    let peers = format!(
        concat!(
            "if [ -e '{0}' ]; then printf '%s\\n' '{{\"items\":[],\"next_page_token\":null}}'; ",
            "exit 0; fi; printf '%s\\n' '{{\"error\":{{\"code\":\"permission\",\"message\":",
            "\"cannot list: the private network is not set up for this account\",",
            "\"http_status\":403}}}}' >&2; exit 1"
        ),
        vm_made.display()
    );
    let database = format!(
        "touch '{}'; printf '%s\\n' '{{\"id\":224}}'; exit 0",
        made.display()
    );
    let vm = format!(
        concat!(
            "touch '{}'; printf '%s\\n' ",
            "'{{\"vm_id\":\"0f8fad5b-d9cb-469f-a165-707070707070\"}}'; exit 0"
        ),
        vm_made.display()
    );
    // Every other snapshot read answers an empty list, so each read gives a value and the
    // two snapshots are compared. Only the snapshot home's reads: the scenario steps and the
    // cleanup still meet the dead origin.
    let snapshot = concat!(
        "case \"$HOME\" in */snapshot/home) case \" $* \" in ",
        "*\" s3 buckets list \"* | *\" s3 tokens list \"* | *\" network peers list \"*) ;; ",
        "*\" --json \"*\" list \"* | *\" --json \"*\" get \"* | *\" billing balance \"*) ",
        "printf '%s\\n' '{\"items\":[],\"next_page_token\":null}'; exit 0 ;; esac ;; esac"
    );
    let (wrapper, _) = scripted_hipc(
        &home,
        snapshot,
        &[
            ("*\" s3 buckets list \"*", &buckets),
            ("*\" s3 tokens list \"*", &tokens),
            ("*\" network peers list \"*", &peers),
            ("*\" compute databases create \"*", &database),
            ("*\" compute vms create \"*", &vm),
        ],
    );
    let (output, log) = offline(
        &home,
        wrapper.to_str().expect("UTF-8"),
        &["--tier", "spend", "--yes-spend", "compute"],
    );
    assert_eq!(output.status.code(), Some(1), "{log}");
    assert!(
        log.contains(concat!(
            "check: the server left the database's backup bucket and its writer token after ",
            "the delete: FAIL; they are bucket hippius-db-224-backups,s3-token ",
            "db-224-backup-writer, listed in the kept ledger; the script deletes neither"
        )),
        "{log}"
    );
    assert!(
        log.contains("check: the account is as it was found: PASS"),
        "{log}"
    );
    assert!(
        log.contains("note: the VM launch set up the account's private network"),
        "{log}"
    );
    let kept = std::fs::read_to_string(home.path().join("manual_test.log.ledger"))
        .expect("the ledger is kept beside the log");
    let (_, left) = kept
        .split_once("# left by the server: ")
        .expect("the heading");
    assert!(
        left.contains("\nbucket hippius-db-224-backups b2 left"),
        "{kept}"
    );
    assert!(
        left.contains("\ns3-token db-224-backup-writer t2 left"),
        "{kept}"
    );
}

/// The login stores the token without checking it, so the run checks it with one
/// authenticated read before any scenario, and a refused token stops the run there with one
/// failure that says where the token came from; offline there is nothing to ask.
#[test]
fn a_refused_token_stops_the_run_before_any_scenario() {
    let home = Home::new();
    let refused = concat!(
        r#"printf '%s\n' '{"error":{"code":"auth","message":"the API refused the token","#,
        r#""http_status":401}}' >&2; exit 1"#
    );
    // Only the local calls reach the real binary: anything else would go to the live API,
    // so the stand-in refuses it, should the check ever let the run go on.
    let (wrapper, calls) = scripted_hipc(
        &home,
        "",
        &[
            ("*\" auth status \"*", refused),
            ("*\" version \"* | *\" schema \"* | *\" auth login \"*", ":"),
            ("*", "echo 'not sent: the live API' >&2; exit 1"),
        ],
    );
    let log = home.path().join("manual_test.log");
    let output = manual_test_with(
        &home,
        &[
            "--hipc",
            wrapper.to_str().expect("UTF-8"),
            "--log",
            log.to_str().expect("UTF-8"),
        ],
        &[("HIPPIUS_TOKEN", "hipc-manual-test-refused-token")],
    );
    let log = std::fs::read_to_string(&log).expect("the log is written");
    assert_eq!(output.status.code(), Some(1), "{log}");
    assert!(log.contains("token: from HIPPIUS_TOKEN\n"), "{log}");
    assert!(
        log.contains(
            "token: the token from HIPPIUS_TOKEN cannot be used: `hipc auth status` failed (auth)"
        ),
        "{log}"
    );
    assert_eq!(
        failed_titles(&log),
        ["the token from HIPPIUS_TOKEN cannot be used (auth)"],
        "{log}"
    );
    assert!(!log.contains("hipc-manual-test-refused-token"), "{log}");
    // Nothing past the check ran: no snapshot read and no scenario step.
    let calls = real_calls(&calls);
    assert_eq!(
        calls.last().map(String::as_str),
        Some("--json auth status"),
        "{calls:?}"
    );
    assert!(!log.contains("==== snapshot"), "{log}");
    // Offline the token is the stand-in, and the check says it asks no API.
    let home = Home::new();
    let (output, log) = offline(&home, env!("CARGO_BIN_EXE_hipc"), &["first-run"]);
    assert_clean_offline_run(&output, &log, &home);
    assert!(
        log.contains("token: the offline stand-in, not checked: there is no API to ask"),
        "{log}"
    );
}

/// `--offline` refuses a dead origin that something listens on: it would receive the fake
/// token and every request.
#[test]
fn offline_refuses_a_dead_origin_that_answers() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
    let origin = format!(
        "http://{}",
        listener.local_addr().expect("the bound address")
    );
    let home = Home::new();
    let output = manual_test_with(
        &home,
        &[
            "--offline",
            "--hipc",
            env!("CARGO_BIN_EXE_hipc"),
            "first-run",
        ],
        &[("HIPC_MANUAL_TEST_DEAD_ORIGIN", &origin)],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "manual_test: something listens on {}, the origin --offline needs closed",
            origin.trim_start_matches("http://")
        )),
        "{stderr}"
    );
}

/// Two snapshots that each failed a read compare equal and prove nothing: offline, where every
/// read fails, the account check is skipped and names the reads that gave nothing. A balance
/// read that failed, or answered no `balance`, is no pass either.
#[test]
fn failed_snapshot_reads_are_not_a_pass() {
    let home = Home::new();
    let (output, log) = offline(&home, env!("CARGO_BIN_EXE_hipc"), &["first-run"]);
    assert_clean_offline_run(&output, &log, &home);
    assert!(
        log.contains(
            "check: the balance is unchanged: SKIP (no balance was read: {\"error\":\"unavailable\"}, \
             then {\"error\":\"unavailable\"})"
        ),
        "{log}"
    );
    let unread = log
        .split("check: the account is as it was found: SKIP (not read: ")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .expect("the skip names the reads");
    assert!(
        unread.contains("balance") && unread.contains("s3-tokens"),
        "{unread}"
    );
    // A balance read that answers without `balance` gives null twice, which is not a pass.
    let home = Home::new();
    let (wrapper, _) = scripted_hipc(
        &home,
        "",
        &[("*\" billing balance \"*", "printf '%s\\n' '{}'; exit 0")],
    );
    let (output, log) = offline(&home, wrapper.to_str().expect("UTF-8"), &["first-run"]);
    assert_clean_offline_run(&output, &log, &home);
    assert!(
        log.contains(
            "check: the balance is unchanged: SKIP (no balance was read: null, then null)"
        ),
        "{log}"
    );
}

/// An account with no Hub namespace answers the namespace's reads, its keys and its
/// repositories 403 "You must call /api/registry/provision/ first."; the steps and the
/// snapshot take that as a state, with a note, and nothing creates a namespace.
#[test]
fn an_account_without_a_hub_namespace_is_a_state_not_a_failure() {
    let home = Home::new();
    let refused = concat!(
        r#"printf '%s\n' '{"error":{"code":"permission","message":"cannot list: the API says "#,
        r#"`You must call /api/registry/provision/ first.`","hint":"run `hipc hub namespace "#,
        r#"create <namespace> --reveal --yes` to create a Hub namespace","#,
        r#""http_status":403}}' >&2; exit 1"#
    );
    let (wrapper, calls) = scripted_hipc(
        &home,
        "",
        &[(
            "*\" hub namespace get \"* | *\" hub namespace robot get \"* | \
             *\" hub events list \"* | *\" hub usage \"* | *\" hub repositories list \"* | \
             *\" hub keys list \"*",
            refused,
        )],
    );
    let (output, log) = offline(&home, wrapper.to_str().expect("UTF-8"), &["hub"]);
    assert_clean_offline_run(&output, &log, &home);
    assert_eq!(
        log.matches("note: this account has no Hub namespace")
            .count(),
        6,
        "{log}"
    );
    // The snapshot takes the state as the value of its two Hub reads.
    let unread = log
        .split("check: the account is as it was found: SKIP (not read: ")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .expect("the skip names the reads");
    assert!(
        !unread.contains("hub-keys") && !unread.contains("hub-repositories"),
        "{unread}"
    );
    assert!(
        !real_calls(&calls)
            .iter()
            .any(|call| call.contains("provision") || call.contains("namespace create")),
        "nothing creates a namespace"
    );
    // A 403 that is not that sentence is still a failure, one naming the route too.
    for said in [
        "You do not have permission.",
        "Only an owner may call /api/registry/provision/.",
    ] {
        let home = Home::new();
        let forbidden = format!(
            concat!(
                r#"printf '%s\n' '{{"error":{{"code":"permission","message":"cannot list: "#,
                r#"the API says `{}`","http_status":403}}}}' >&2; exit 1"#
            ),
            said
        );
        let (wrapper, _) = scripted_hipc(&home, "", &[("*\" hub keys list \"*", &forbidden)]);
        let (_, log) = offline(&home, wrapper.to_str().expect("UTF-8"), &["hub"]);
        assert!(
            failed_titles(&log).contains(&"List the Hub keys".to_owned()),
            "{said}: {log}"
        );
    }
}
