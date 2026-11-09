//! The command reference, `docs/commands.md`, rendered from the built binary by
//! `scripts/docs-reference.sh` (`TENETS.md`, tenet 16). The committed page must be the
//! script's output for this binary, so a help change ships with its regenerated page.

use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt as _;
use std::process::{Command, Output};

use serde_json::Value;

use crate::support::TempDir;

const SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/docs-reference.sh"
);
const PAGE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/commands.md");
const HIP: &str = env!("CARGO_BIN_EXE_hipc");

fn render(args: &[&str]) -> Output {
    Command::new("/bin/bash")
        .arg(SCRIPT)
        .args(args)
        .output()
        .expect("run the reference generator")
}

/// The page the script renders for the built binary.
fn rendered() -> String {
    let out = render(&[HIP]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    String::from_utf8(out.stdout).expect("the page is UTF-8")
}

/// Every command path `hipc schema` lists, as `hipc` followed by its words.
fn schema_titles() -> BTreeSet<String> {
    let out = Command::new(HIP)
        .arg("schema")
        .output()
        .expect("the hipc binary built by cargo runs");
    let doc: Value = serde_json::from_slice(&out.stdout).expect("the schema is JSON");
    doc["commands"]
        .as_array()
        .expect("the schema has commands")
        .iter()
        .map(|command| {
            let words: Vec<&str> = command["path"]
                .as_array()
                .expect("a path is an array")
                .iter()
                .map(|w| w.as_str().expect("a path word is a string"))
                .collect();
            format!("hipc {}", words.join(" "))
        })
        .collect()
}

/// The titles of the page's `##` and `###` headings, in order, without the contents list.
fn headings(page: &str) -> Vec<&str> {
    page.lines()
        .filter_map(|line| {
            line.strip_prefix("### ")
                .or_else(|| line.strip_prefix("## "))
        })
        .filter(|title| *title != "Contents")
        .collect()
}

/// The 1-based number of the first line where `committed` and `rendered` differ, with both
/// lines; a page that is a prefix of the other differs at its end.
fn first_difference<'a>(committed: &'a str, rendered: &'a str) -> (usize, &'a str, &'a str) {
    let mut ours = committed.lines();
    let mut theirs = rendered.lines();
    let mut number = 0;
    loop {
        number += 1;
        match (ours.next(), theirs.next()) {
            (Some(a), Some(b)) if a == b => {}
            (a, b) => {
                return (
                    number,
                    a.unwrap_or("<end of file>"),
                    b.unwrap_or("<end of file>"),
                );
            }
        }
    }
}

#[test]
fn the_committed_page_is_the_script_output_for_this_binary() {
    let committed = std::fs::read_to_string(PAGE).expect("docs/commands.md exists");
    let rendered = rendered();
    if committed != rendered {
        let (number, ours, theirs) = first_difference(&committed, &rendered);
        panic!(
            "docs/commands.md is not the help of the built binary; first difference at line \
             {number}:\n  committed: {ours}\n  rendered:  {theirs}\nrun \
             `scripts/docs-reference.sh target/debug/hipc > docs/commands.md` and commit it"
        );
    }
}

#[test]
fn the_first_difference_names_the_line_and_the_end_of_a_shorter_page() {
    assert_eq!(first_difference("a\nb\nc\n", "a\nx\nc\n"), (2, "b", "x"));
    assert_eq!(
        first_difference("a\nb\n", "a\nb\nc\n"),
        (3, "<end of file>", "c")
    );
    assert_eq!(
        first_difference("a\n", "a\n"),
        (2, "<end of file>", "<end of file>")
    );
}

#[test]
fn every_schema_command_has_a_heading_and_a_contents_entry() {
    let page = rendered();
    let headings: BTreeSet<&str> = headings(&page).into_iter().collect();
    assert!(headings.contains("hipc"), "{headings:?}");
    for title in schema_titles() {
        assert!(
            headings.contains(title.as_str()),
            "no heading for `{title}`"
        );
        let anchor = title.replace(' ', "-");
        let entry = format!("- [`{title}`](#{anchor})");
        assert!(page.contains(&entry), "no contents entry {entry}");
    }
}

#[test]
fn the_global_options_are_printed_once_and_every_page_keeps_its_examples() {
    let page = rendered();
    assert_eq!(page.matches("\nGlobal options:\n").count(), 1);
    // One fenced block per heading, each ending with the page's examples, which every
    // help page has; cutting the global options must not take them along.
    let fences = page.matches("\n```text\n").count();
    assert_eq!(fences, headings(&page).len());
    assert_eq!(page.matches("\nExamples:\n").count(), fences);
    // A heading at depth one is a top-level command or group; deeper pages are `###`.
    assert!(page.contains("\n## hipc version\n"));
    assert!(page.contains("\n## hipc auth\n"));
    assert!(page.contains("\n### hipc auth login\n"));
    assert!(page.contains("\n  hipc auth login --token-file token.txt\n"));
}

#[test]
fn the_page_says_how_it_is_made_and_where_the_schema_is() {
    let page = rendered();
    assert!(page.starts_with("# Command reference\n"));
    assert!(page.contains("scripts/docs-reference.sh target/debug/hipc > docs/commands.md"));
    assert!(page.contains("`hipc schema`"));
    assert!(page.ends_with("\n```\n"), "one line break ends the file");
    assert!(!page.ends_with("\n\n"), "no blank line ends the file");
    // A blank line separates each fence from the next heading.
    assert!(!page.contains("```\n##"), "a heading right after a fence");
}

#[test]
fn no_argument_two_arguments_and_a_file_that_is_not_executable_are_refused() {
    for args in [&[][..], &["a", "b"][..]] {
        let out = render(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("usage:"));
        assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    }

    let dir = TempDir::new();
    let plain = dir.path().join("hipc");
    std::fs::write(&plain, "not a binary").expect("write a plain file");
    std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o644))
        .expect("make the file plain");
    for path in [plain.to_str().expect("a UTF-8 path"), "/nonexistent/hipc"] {
        let out = render(&[path]);
        assert_eq!(out.status.code(), Some(1), "{path}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("is not an executable file"));
        assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    }
}
