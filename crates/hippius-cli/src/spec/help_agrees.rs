//! `hipc schema` against clap's own reading of the parser, in-process: the help of every
//! command, its usage line, and whether clap takes each flag twice (`TENETS.md` tenet 7).
//! The parser is built once and reused for every page and every parse, where a spawn of the
//! debug binary per page and per flag cost most of the end-to-end suite's time. The
//! end-to-end suite keeps one row of each check, which pins that the binary prints the help
//! these pages are rendered from (`tests/e2e/contract.rs`).

use std::collections::BTreeSet;

use clap::builder::ValueParser;
use clap::error::ErrorKind;
use clap::{CommandFactory as _, Parser as _};
use serde_json::Value;

use super::schema;
use crate::cli::{Cli, parse_command};

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

/// The `-h` text of the command at `path`, as `hipc <path> -h` prints it: clap renders the
/// help as the error of a parse that reaches `-h`, and `hipc` prints that rendering as it is.
fn help(parser: &mut clap::Command, path: &[&str]) -> String {
    let argv = [&["hipc"], path, &["-h"]].concat();
    let error = parser
        .try_get_matches_from_mut(argv)
        .expect_err("`-h` stops the parse with the help");
    assert_eq!(error.kind(), ErrorKind::DisplayHelp, "{path:?}: {error}");
    error.render().to_string()
}

/// Every command path of the schema and each path prefix of one, the root included.
fn pages(schema: &Value) -> BTreeSet<Vec<String>> {
    let mut pages = BTreeSet::new();
    for path in schema_paths(schema) {
        for len in 0..=path.len() {
            pages.insert(path[..len].to_vec());
        }
    }
    pages
}

/// The path of each command of the schema, in its order.
fn schema_paths(schema: &Value) -> Vec<Vec<String>> {
    schema["commands"]
        .as_array()
        .expect("the schema lists its commands")
        .iter()
        .map(|command| {
            command["path"]
                .as_array()
                .expect("a path is an array")
                .iter()
                .map(|word| word.as_str().expect("a path word is text").to_owned())
                .collect()
        })
        .collect()
}

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
    let mut parser = parse_command();
    for path in pages(&schema(&[]).unwrap()) {
        let path: Vec<&str> = path.iter().map(String::as_str).collect();
        let help = help(&mut parser, &path);
        let own = if path.is_empty() {
            assert!(
                help.contains("\nGlobal options:\n"),
                "the top-level help has no Global options heading:\n{help}"
            );
            let (own, global) = help
                .split_once("\nGlobal options:\n")
                .expect("the heading is in the help, asserted above");
            assert_eq!(option_flags(global), GLOBAL_FLAGS, "{path:?}");
            own
        } else {
            assert!(!help.contains("\nGlobal options:\n"), "{path:?}:\n{help}");
            assert!(
                help.ends_with("\n\nGlobal options: see hipc --help\n"),
                "{path:?}:\n{help}"
            );
            &help
        };
        let own = option_flags(own);
        assert!(own.contains(&"--help"), "{path:?}: {own:?}");
        assert!(
            own.iter().all(|flag| !GLOBAL_FLAGS.contains(flag)),
            "{path:?}: {own:?}"
        );
    }
}

/// Each command's `about` in `hipc schema` is the sentence its `-h` opens with, so the
/// schema alone maps a task to a command in the words the help uses.
#[test]
fn each_about_is_the_line_its_help_opens_with() {
    let mut parser = parse_command();
    let schema = schema(&[]).unwrap();
    let commands = schema["commands"].as_array().unwrap();
    let paths = schema_paths(&schema);
    assert_eq!(commands.len(), paths.len());
    for (command, path) in commands.iter().zip(paths) {
        let path: Vec<&str> = path.iter().map(String::as_str).collect();
        let help = help(&mut parser, &path);
        assert_eq!(help.lines().next(), command["about"].as_str(), "{path:?}");
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
    assert!(
        command[field].is_array(),
        "{field} is not an array: {command}"
    );
    command[field]
        .as_array()
        .expect("the field is an array, asserted above")
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
    let prefix = path.join(" ");
    assert!(
        line.starts_with(&prefix),
        "the usage of {path:?} does not start with its path: {line}"
    );
    let rest = line
        .strip_prefix(&prefix)
        .expect("the usage starts with the path, asserted above");
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

/// [`Cli`]'s parser with every value parser replaced by one that takes any text, so a flag
/// can be given twice whatever value it needs. Whether clap takes a flag twice is decided by
/// its action and its number of values, which stay as they are; a value that does not parse
/// would be refused before the second use and say nothing about it.
fn taking_any_value(command: clap::Command) -> clap::Command {
    command
        .mut_args(|arg| {
            if arg.get_action().takes_values() {
                arg.value_parser(ValueParser::os_string())
            } else {
                arg
            }
        })
        .mut_subcommands(taking_any_value)
}

/// Whether clap takes `flag` of the command at `path` twice, read from a parse of
/// `hipc <path> <flag> [x] <flag> [x] -h` by [`taking_any_value`]'s parser: clap refuses a
/// second use of a flag that does not repeat as it reads it, and stops at `-h` otherwise.
fn takes_twice(parser: &mut clap::Command, path: &[&str], flag: &Value) -> bool {
    let name = flag["name"].as_str().expect("a flag has a name");
    let once: &[&str] = if flag["value_name"].is_null() {
        &[name]
    } else {
        &[name, "x"]
    };
    let argv = [&["hipc"], path, once, once, &["-h"]].concat();
    let error = parser
        .try_get_matches_from_mut(argv)
        .expect_err("`-h` or the second use stops the parse");
    // Only these two stops are answers; any other error is a broken argv or parser.
    let refused = error.kind() == ErrorKind::ArgumentConflict
        && error.to_string().contains("cannot be used multiple times");
    assert!(
        refused || error.kind() == ErrorKind::DisplayHelp,
        "{path:?} {name}: {error}"
    );
    !refused
}

/// Every argument and flag the parser gives a command is in `hipc schema`, in the order its
/// `-h` lists them, with the value name, default, possible values, and help it shows; the
/// required flags, positional arguments, and required sets its usage line shows; and the
/// repeat bit clap applies when a flag is given twice. The global flags are listed once, at
/// the top level. The help and the parse are clap's own reading of the parser, so a
/// flag added, renamed, moved, or hidden without the schema following fails here.
#[test]
fn schema_lists_every_argument_and_flag_the_help_shows() {
    let schema = schema(&[]).unwrap();
    let mut parser = parse_command();
    let mut repeats_parser = taking_any_value(Cli::command());
    let globals = schema_lines(&schema, "global_flags", "Global options");
    let names: Vec<&str> = globals.iter().filter_map(|l| l.flag.as_deref()).collect();
    assert_eq!(names, GLOBAL_FLAGS);
    let root_globals: Vec<_> = help_lines(&help(&mut parser, &[]))
        .into_iter()
        .filter(|line| line.heading == "Global options")
        .collect();
    assert_eq!(
        root_globals, globals,
        "the root help lists every global flag"
    );
    for command in schema["commands"].as_array().unwrap() {
        let args: Vec<&str> = command["path"]
            .as_array()
            .unwrap()
            .iter()
            .map(|word| word.as_str().unwrap())
            .collect();
        let mut expected = schema_lines(command, "args", "Arguments");
        // A positional argument is named by its id, never by a flag spelling.
        assert!(expected.iter().all(|line| line.flag.is_none()), "{args:?}");
        expected.extend(schema_lines(command, "flags", "Options"));
        let help = help(&mut parser, &args);
        let (own, global): (Vec<_>, Vec<_>) = help_lines(&help)
            .into_iter()
            .partition(|line| line.heading != "Global options");
        // The global flags are on the root help alone, checked above.
        assert_eq!(global, [], "{args:?}");
        assert_eq!(own, expected, "{args:?}");
        let mut shown = usage(&help, &args);
        shown.flags.sort();
        assert_eq!(shown, schema_usage(command), "{args:?}");
        for flag in command["flags"].as_array().unwrap() {
            let repeats = takes_twice(&mut repeats_parser, &args, flag);
            assert_eq!(flag["repeats"], repeats, "{args:?} {flag}");
        }
    }
}

/// The repeat reader tells a flag clap takes twice from one it refuses twice, for a switch
/// and for a flag with a value, whatever value the flag's own parser needs.
#[test]
fn takes_twice_reads_both_answers() {
    let mut parser = taking_any_value(Cli::command());
    let path = ["account", "members", "update", "7"];
    let flag = |name: &str, value_name: Option<&str>| serde_json::json!({"name": name, "value_name": value_name});
    assert!(takes_twice(
        &mut parser,
        &path,
        &flag("--grant", Some("GRANT"))
    ));
    assert!(!takes_twice(
        &mut parser,
        &path,
        &flag("--billing-permission", Some("BOOL"))
    ));
    assert!(!takes_twice(&mut parser, &[], &flag("--yes", None)));
    // The parser `hipc` runs refuses the placeholder value before the second use.
    assert!(
        Cli::try_parse_from([
            "hipc",
            "account",
            "members",
            "update",
            "7",
            "--billing-permission",
            "x"
        ])
        .is_err()
    );
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
