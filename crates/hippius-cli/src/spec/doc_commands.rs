//! The `hipc` commands the README and the getting-started and agent pages show, against
//! `hipc schema` and the parser `hipc` parses with, in-process, and the size of the agent
//! pages (`TENETS.md` tenet 16). The end-to-end suite keeps one of them, run by the binary
//! (`tests/e2e/contract.rs`).

use std::collections::BTreeSet;

use clap::error::ErrorKind;

use super::schema;
use crate::cli::parse_command;

/// Reads a page of the repository at compile time, so an edit to it rebuilds these tests.
macro_rules! page {
    ($path:literal) => {
        (
            $path,
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../", $path)),
        )
    };
}

/// The pages an agent reads, each with the most bytes it may hold: `docs/agents.md` is read
/// first, whole, and each page under `docs/agents/` only when a task needs it.
const AGENT_PAGES: [(&str, &str, usize); 5] = [
    sized(page!("docs/agents.md"), 10 * 1024),
    sized(page!("docs/agents/accounts.md"), 6 * 1024),
    sized(page!("docs/agents/api.md"), 6 * 1024),
    sized(page!("docs/agents/hub.md"), 6 * 1024),
    sized(page!("docs/agents/secrets.md"), 6 * 1024),
];

/// A page of [`AGENT_PAGES`] with its size.
const fn sized(
    (name, text): (&'static str, &'static str),
    budget: usize,
) -> (&'static str, &'static str, usize) {
    (name, text, budget)
}

/// The pages that show `hipc` commands. The user journeys show flags before the path, so they
/// are not here.
const COMMAND_PAGES: [(&str, &str); 7] = [
    (
        "README.md",
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md")),
    ),
    (
        "docs/getting-started.md",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/getting-started.md"
        )),
    ),
    page!("docs/agents.md"),
    page!("docs/agents/accounts.md"),
    page!("docs/agents/api.md"),
    page!("docs/agents/hub.md"),
    page!("docs/agents/secrets.md"),
];

/// Every command path `hipc schema` lists, each as its words joined by one space.
fn schema_paths() -> BTreeSet<String> {
    let doc = schema(&[]).expect("the whole schema has no path to refuse");
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

/// Every group of commands `hipc schema` lists, as its words joined by one space: each path
/// of a command with its last word left off, and its shorter prefixes, such as `s3` and
/// `s3 buckets`. A page may show a group only with `--help`, which lists what it holds.
fn group_paths(paths: &BTreeSet<String>) -> BTreeSet<String> {
    paths
        .iter()
        .flat_map(|path| {
            let words: Vec<&str> = path.split(' ').collect();
            (1..words.len()).map(move |n| words[..n].join(" "))
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

/// Every `hipc` command the README, `docs/getting-started.md`, and the agent pages show, in
/// a code block or an inline code span, starts with a command path of `hipc schema`, and `hipc`
/// parses it, so a rename that forgets a page fails here. The one other thing a page may show
/// is `--help` alone, on the top level or on a group, which is how a page teaches an agent to
/// walk the tree. The parse stops at `--help`, which
/// `hipc` runs without reading a file or the network, and fails on a word the command
/// does not take, such as the old `show` after `hipc billing stripe`. A stale word that the
/// command takes as a positional argument still parses; only the path and the flags are
/// checked. A page may not show a global flag before the path, or a placeholder in it.
#[test]
fn page_commands_start_with_a_schema_path_and_parse() {
    let paths = schema_paths();
    let groups = group_paths(&paths);
    let mut parser = parse_command();
    let mut found = BTreeSet::new();
    for (page, markdown) in COMMAND_PAGES {
        for (source, args) in readme_commands(markdown) {
            let source = source.trim();
            let line = args.join(" ");
            // The longest path, so `hipc compute vms list` is not read as a shorter one.
            let path = (1..=args.len())
                .rev()
                .map(|n| args[..n].join(" "))
                .find(|path| paths.contains(path))
                .or_else(|| match args.split_last() {
                    Some((&"--help", [])) => Some(String::new()),
                    Some((&"--help", group)) => {
                        Some(group.join(" ")).filter(|g| groups.contains(g))
                    }
                    _ => None,
                });
            assert!(
                path.is_some(),
                "{page} `{source}`: `hipc {line}` starts with no `hipc schema` path \
                 (a renamed command, or a flag before the path)"
            );
            found.insert((page, path.expect("a schema path was found, asserted above")));

            let argv = [&["hipc"], args.as_slice(), &["--help"]].concat();
            let stop = parser
                .try_get_matches_from_mut(argv)
                .expect_err("`--help` stops the parse with the help");
            assert_eq!(
                stop.kind(),
                ErrorKind::DisplayHelp,
                "{page} `{source}`: `hipc {line}` does not parse: {stop}"
            );
        }
    }

    // A scan that stopped finding a page's commands would pass with nothing to check.
    let anchors = [
        ("README.md", "auth login"),
        ("README.md", "auth status"),
        ("README.md", "billing balance"),
        ("README.md", "schema"),
        ("README.md", "completions"),
        ("README.md", "s3 buckets create"),
        ("docs/getting-started.md", "auth login"),
        ("docs/getting-started.md", "account use"),
        ("docs/getting-started.md", "completions"),
        ("docs/agents.md", "schema"),
        ("docs/agents.md", "auth login"),
        ("docs/agents.md", "api"),
        ("docs/agents.md", ""),
        ("docs/agents.md", "s3 buckets"),
        ("docs/agents/accounts.md", "account use"),
        ("docs/agents/api.md", "api"),
        ("docs/agents/hub.md", "hub keys create"),
        ("docs/agents/secrets.md", "s3 configure"),
    ];
    for (page, expected) in anchors {
        assert!(
            found.contains(&(page, expected.to_owned())),
            "{page} shows no `hipc {expected}`"
        );
    }
}

/// Each agent page stays within its size, so what an agent reads first stays small; a
/// paragraph that does not fit belongs in `--help`, in `hipc schema`, or in a page under
/// `docs/agents/`. Every page under `docs/agents/` is in [`AGENT_PAGES`], so a new one has a
/// size too.
#[test]
fn agent_pages_stay_within_their_size() {
    for (page, text, budget) in AGENT_PAGES {
        assert!(
            text.len() <= budget,
            "{page} holds {} bytes, more than its {budget}",
            text.len()
        );
    }
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/agents");
    let on_disk: BTreeSet<String> = std::fs::read_dir(dir)
        .expect("docs/agents is a directory")
        .map(|entry| {
            let name = entry.expect("a directory entry").file_name();
            format!("docs/agents/{}", name.to_string_lossy())
        })
        .collect();
    let listed: BTreeSet<String> = AGENT_PAGES[1..]
        .iter()
        .map(|(page, _, _)| (*page).to_owned())
        .collect();
    assert_eq!(
        on_disk, listed,
        "every page under docs/agents/ is in AGENT_PAGES"
    );
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
