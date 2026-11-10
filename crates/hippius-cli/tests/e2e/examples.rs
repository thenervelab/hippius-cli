//! The `Examples:` lines of every help page, run as a person would type them, against a
//! local listener that answers every request with a 404. An example that the command
//! refuses as typed exits 2 and fails here; the `hipc cdn` example without `--public` did. A dry
//! run skips that check (tenet 5), so the real run is the one that counts, and the 404 keeps every
//! example that spends or changes state away from a real server. A dry run of each example must
//! also send nothing.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::common::{self, CLUSTER_KEY, Hipc, Home, Reply, Run, Server, TOKEN, any_request};

/// The confirmation a command asks for only because the test listener is not the default
/// origin (`hipc schema` `confirmation_when`). A person running the example against
/// `https://api.hippius.com` is not asked, so the test confirms it for them.
const NOT_THE_DEFAULT_ORIGIN: &str = "the API origin is not `https://api.hippius.com`";

/// Each `hipc schema` command, as its words joined by one space, and its `confirmation_when`.
type Commands = BTreeMap<String, Option<String>>;

/// Runs `hipc args` in a clean home with no server, and returns stdout.
fn stdout_of(args: &[&str]) -> String {
    let home = Home::new();
    let out = common::command(&home)
        .args(args)
        .output()
        .expect("the hipc binary built by cargo runs");
    assert_eq!(out.status.code(), Some(0), "hipc {args:?}");
    String::from_utf8(out.stdout).expect("output is UTF-8")
}

/// Each schema path's `confirmation` and `confirmation_when`, as the schema prints them.
type Confirmations = BTreeMap<String, (Value, Value)>;

fn schema_confirmations() -> Confirmations {
    let schema: Value =
        serde_json::from_str(&stdout_of(&["schema"])).expect("the schema is one JSON document");
    schema["commands"]
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
            (
                words.join(" "),
                (
                    command["confirmation"].clone(),
                    command["confirmation_when"].clone(),
                ),
            )
        })
        .collect()
}

fn schema_commands() -> Commands {
    let schema: Value =
        serde_json::from_str(&stdout_of(&["schema"])).expect("the schema is one JSON document");
    let commands = schema["commands"]
        .as_array()
        .expect("the schema has commands");
    commands
        .iter()
        .map(|command| {
            let words: Vec<&str> = command["path"]
                .as_array()
                .expect("a path is an array")
                .iter()
                .map(|w| w.as_str().expect("a path word is a string"))
                .collect();
            let when = command["confirmation_when"].as_str().map(str::to_owned);
            (words.join(" "), when)
        })
        .collect()
}

/// The lines under `Examples:` in one help text, without their indent.
fn examples_in(help: &str) -> Vec<String> {
    let Some((_, rest)) = help.split_once("\nExamples:\n") else {
        return Vec::new();
    };
    rest.lines()
        .map_while(|line| line.strip_prefix("  "))
        .map(str::to_owned)
        .collect()
}

/// Every example of every help page: the top level, each group, and each command.
fn every_example(commands: &Commands) -> BTreeSet<String> {
    let mut pages = BTreeSet::new();
    for path in commands.keys() {
        let words: Vec<&str> = path.split(' ').collect();
        for len in 0..=words.len() {
            pages.insert(words[..len].to_vec());
        }
    }
    let mut examples = BTreeSet::new();
    for page in pages {
        let help = stdout_of(&[page.as_slice(), &["--help"]].concat());
        examples.extend(examples_in(&help));
    }
    examples
}

/// Runs `args` in a fresh home holding the files the examples name, against a listener
/// that answers every request with a 404. Returns the run and the number of requests the
/// listener received.
fn run_against_a_listener(args: &[&str]) -> (Run, usize) {
    let server = Server::start(vec![any_request(Reply::not_found())]);
    let run = run_against(&server, args);
    (run, server.requests().len())
}

/// Runs `args` in a fresh home holding the files the examples name, against `server`.
fn run_against(server: &Server, args: &[&str]) -> Run {
    let home = Home::new();
    home.file("token.txt", format!("{TOKEN}\n").as_bytes());
    home.file("secret.txt", b"example-secret\n");
    home.file(
        "token.json",
        br#"{"kind":"master","access_key_id":"hip_A1B2","secret":"example-secret"}"#,
    );
    home.file("bucket.json", br#"{"name":"photos"}"#);
    home.file("message.txt", b"the VM does not boot\n");
    home.file("items.json", br#"{"items":["203.0.113.7"]}"#);
    home.file(
        "cors.json",
        br#"{"allow_origins":["https://app.example.com"]}"#,
    );
    home.file("policy.json", br#"{"Version":"2012-10-17","Statement":[]}"#);
    home.file("invite.txt", b"inv_example\n");
    home.file(
        "access-key.txt",
        format!("{}\n", crate::common::PHRASES[1]).as_bytes(),
    );
    home.file(
        "hotkey.txt",
        format!("{}\n", crate::common::PHRASES[0]).as_bytes(),
    );
    home.file(
        "id_ed25519.pub",
        b"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGV4YW1wbGU ana@laptop\n",
    );
    home.file(
        "rules.json",
        br#"{"rules":[{"protocol":"tcp","ports":["22"],"source_cidrs":["203.0.113.0/24"]}]}"#,
    );
    home.file(
        "cdn-rules.json",
        br#"{"rules":[{"match":{"path_prefix":"/assets/"},"actions":{"edge_ttl":86400}}]}"#,
    );
    home.file(
        "waf-rules.json",
        br#"{"rules":[{"expression":"ip.src in $blocked_ips","action":"block"}]}"#,
    );
    home.file(
        "rate-limits.json",
        concat!(
            r#"{"rules":[{"expression":"true","action":"block","key":"ip","period":60,"#,
            r#""requests":600,"timeout":300}]}"#
        )
        .as_bytes(),
    );
    home.file(
        "waf-request.json",
        br#"{"ip":"203.0.113.7","path":"/admin"}"#,
    );
    home.file("items.txt", b"203.0.113.7\n198.51.100.0/24\n");
    home.file("block.html", b"<p>Blocked: {ref}</p>\n");
    home.file("hippius-cluster-prod.key", CLUSTER_KEY.as_bytes());
    home.file("cloudflare.json", br#"{"api_token":"cf-example-token"}"#);
    home.file("compose.yaml", b"services:\n  web:\n    image: nginx\n");
    home.file("secrets.json", br#"{"DB_PASSWORD":"example"}"#);
    Hipc::against(&home, server)
        .current_dir(home.path())
        .run(args)
}

/// The `hipc schema` path that starts `args`, the longest one, so `compute vms list` is not
/// read as a shorter path. A leading flag that takes no value, such as `--dry-run`, is skipped.
fn path_of<'c>(args: &[&str], commands: &'c Commands) -> &'c str {
    let args: Vec<&str> = args
        .iter()
        .copied()
        .skip_while(|a| a.starts_with("--"))
        .collect();
    (1..=args.len())
        .rev()
        .find_map(|n| commands.get_key_value(&args[..n].join(" ")))
        .map_or_else(
            || panic!("`hipc {}` starts with no schema path", args.join(" ")),
            |(path, _)| path.as_str(),
        )
}

/// Every example runs as typed without exit 2, and its dry run exits 0 and sends nothing.
#[test]
fn every_help_example_runs_as_typed_and_its_dry_run_sends_nothing() {
    let commands = schema_commands();
    let confirmations = schema_confirmations();
    let examples = every_example(&commands);
    // A scan that stopped finding the examples would pass with nothing to check.
    assert!(
        examples.contains("hipc cdn zones create --name site --bucket photos --public --yes"),
        "{examples:#?}"
    );

    for example in &examples {
        assert!(
            !example.contains(['\'', '"']),
            "the scan splits on spaces, so it cannot run a quoted example: {example}"
        );
        let mut args: Vec<&str> = example.split(' ').collect();
        assert_eq!(
            args.remove(0),
            "hipc",
            "an example starts with hipc: {example}"
        );
        let when = commands[path_of(&args, &commands)].as_deref();
        if when.is_some_and(|when| when.starts_with(NOT_THE_DEFAULT_ORIGIN))
            && !args.contains(&"--yes")
        {
            args.push("--yes");
        }

        let (run, _) = run_against_a_listener(&args);
        assert_ne!(
            run.code(),
            Some(2),
            "`{example}` is refused as typed: {}",
            run.stderr()
        );

        if !args.contains(&"--dry-run") {
            args.insert(0, "--dry-run");
        }
        let (run, sent) = run_against_a_listener(&args);
        assert_eq!(run.code(), Some(0), "`{example}` dry run: {}", run.stderr());
        assert_eq!(sent, 0, "the dry run of `{example}` sent a request");
        // Help, schema, version, and completions never touch the network, and print their
        // own output under `--dry-run`, not a dry-run document.
        let path = path_of(&args, &commands);
        if !["completions", "schema", "version"].contains(&path) {
            assert_one_dry_run_shape(&run.json(), example, path, &confirmations);
        }
    }
}

/// The commands `hipc schema` marks as always confirmed that stop before the question when
/// every read answers `{}`: the read names the target in the question, or decides it, and
/// `{}` does not hold what it needs, so the run exits 1 having sent only reads. Each still
/// sends no write, which the sweep checks.
const STOP_AT_A_READ: &[&str] = &[
    // The list a replace compares, or the member list a grant replace reads.
    "account members update",
    "cdn lists items update",
    "cdn rules update",
    "compute firewall update",
    // The transfer, the project, or the accounts the question names.
    "account project-transfers accept",
    "account project-transfers cancel",
    "account project-transfers decline",
    "account projects transfer",
    // The hAlpha quote and purchases the buy is checked against.
    "billing top-up",
    // The validate answer the launch is checked against.
    "compute app-engine apps create",
    // The VM, the cluster, the node pool, or the operation the question names.
    "compute backups restore",
    "compute kubernetes node-pools scale",
    "compute kubernetes nodes replace",
    "compute kubernetes operations cancel",
    "compute kubernetes operations resume",
];

/// Every command `hipc schema` marks as always confirmed (`confirmation` true, no
/// `confirmation_when`) refuses, run as each of its help examples without `--yes`, with exit
/// 2 `confirmation_required`, and sends no write: the listener answers every request 200
/// `{}`, so a call site that sent its write without asking would exit 0 and fail here,
/// whatever the schema and the dry run say. Reads may run first (tenet 5); a command whose
/// read gives it too little to ask is in [`STOP_AT_A_READ`], and must still send no write.
#[test]
fn every_confirmed_command_refuses_without_yes_and_sends_no_write() {
    let commands = schema_commands();
    let confirmations = schema_confirmations();
    let always: BTreeSet<&str> = confirmations
        .iter()
        .filter(|(_, (confirmation, when))| *confirmation == true && when.is_null())
        .map(|(path, _)| path.as_str())
        .collect();
    assert!(always.contains("compute vms delete"), "{always:?}");
    let mut swept = BTreeSet::new();
    let mut stopped = BTreeSet::new();
    let mut report = Vec::new();
    for example in every_example(&commands) {
        let mut args: Vec<&str> = example.split(' ').skip(1).collect();
        let path = path_of(&args, &commands);
        if !always.contains(path) || args.contains(&"--dry-run") {
            continue;
        }
        args.retain(|arg| *arg != "--yes");
        let server = Server::start(vec![any_request(Reply::json(200, "{}"))]);
        let run = run_against(&server, &args);
        let writes: Vec<String> = server
            .requests()
            .iter()
            .filter(|request| !["GET", "HEAD"].contains(&request.method.as_str()))
            .filter(|request| !request.target.ends_with("/validate/"))
            .map(|request| format!("{} {}", request.method, request.target))
            .collect();
        assert!(
            writes.is_empty(),
            "`{example}` without `--yes` sent {writes:?}"
        );
        let code = serde_json::from_str::<Value>(run.stderr())
            .map_or(Value::Null, |error| error["error"]["code"].clone());
        if run.code() == Some(2) && code == "confirmation_required" {
            swept.insert(path);
        } else if STOP_AT_A_READ.contains(&path) && run.code() == Some(1) {
            stopped.insert(path);
        } else {
            report.push(format!("{path}: `{example}` exit {:?} {code}", run.code()));
        }
    }
    assert!(report.is_empty(), "{}", report.join("\n"));
    // The table holds no command that now reaches the question.
    let listed: BTreeSet<&str> = STOP_AT_A_READ.iter().copied().collect();
    assert_eq!(stopped, listed, "the commands that stop at a read");
    // Every always-confirmed command has an example with `--yes`, so none is left out.
    let missed: Vec<&&str> = always
        .iter()
        .filter(|path| !swept.contains(*path) && !stopped.contains(*path))
        .collect();
    assert!(missed.is_empty(), "no example runs {missed:?}");
}

/// The commands whose real run always sends more than one request, which a dry run lists
/// (`TENETS.md` tenet 5). A list replace is told by its `reads_first`, and Hub
/// `--replace` by its `may_cancel_first`.
const MORE_THAN_ONE_REQUEST: [&str; 25] = [
    // With `--access-key-file`; with `--token-file` it is a local `action`.
    "auth login",
    "auth status",
    "billing subscriptions",
    "billing auto-reload update",
    // The challenge, then the link with its signature.
    "billing gamma hotkeys create",
    "s3 tokens list",
    "compute vms delete",
    "compute backups restore",
    "compute runners limit update",
    "compute runners settings update",
    "cdn cap update",
    "compute kubernetes clusters delete",
    "compute kubernetes clusters kubeconfig",
    "compute kubernetes node-pools scale",
    "compute kubernetes nodes replace",
    "compute kubernetes operations cancel",
    "compute kubernetes operations resume",
    "account invites accept",
    "account projects transfer",
    "account project-transfers accept",
    "account project-transfers decline",
    "account project-transfers cancel",
    "hub subscription subscribe",
    "support tickets update",
    "compute app-engine apps create",
];

/// A dry run that would send requests lists every one in `requests`, the request the
/// document shows at the top among them, and writes a value only a read gives as a `<...>`
/// placeholder. A local change (`action`) sends nothing and lists none.
fn assert_one_dry_run_shape(doc: &Value, example: &str, path: &str, confirmations: &Confirmations) {
    assert_eq!(doc["dry_run"], true, "`{example}`: {doc}");
    if doc.get("action").is_some() {
        assert!(doc.get("requests").is_none(), "`{example}`: {doc}");
        assert!(doc.get("confirmation").is_none(), "`{example}`: {doc}");
        return;
    }
    // Whether the real run asks for `--yes`, and when, as the schema says it.
    let (confirmation, when) = &confirmations[path];
    assert_eq!(&doc["confirmation"], confirmation, "`{example}`: {doc}");
    assert_eq!(&doc["confirmation_when"], when, "`{example}`: {doc}");
    let requests = doc["requests"]
        .as_array()
        .unwrap_or_else(|| panic!("`{example}` lists no requests: {doc}"));
    // A command whose real run reads first, or fans out, lists more than its own request,
    // so a list cut down to that request alone fails here.
    // A billing email change reads the profile first, to ask only when it changes.
    let reads_the_profile = path == "billing profile update" && example.contains("--billing-email");
    // A launch that opens ports sets the firewall after it.
    let opens_ports = path == "compute vms create" && example.contains("--open-");
    let several = MORE_THAN_ONE_REQUEST.contains(&path)
        || reads_the_profile
        || opens_ports
        || doc.get("reads_first").is_some()
        || doc.get("may_cancel_first").is_some();
    if several {
        assert!(
            requests.len() > 1,
            "`{example}` lists too few requests: {doc}"
        );
    } else {
        assert_eq!(requests.len(), 1, "`{example}`: {doc}");
    }
    for request in requests {
        let url = request["url"].as_str().expect("a request has a URL");
        // The Hub cancel keeps the `{...}` it shipped with (tenet 7).
        let url = url.replace("/registry/subscriptions/{subscription_id}/", "");
        assert!(
            !url.contains(['{', '}']),
            "`{example}` writes a placeholder another way: {url}"
        );
    }
    if doc.get("method").is_some() {
        let top = |request: &Value| {
            (
                request["method"].clone(),
                request["url"].clone(),
                request["body"].clone(),
            )
        };
        assert!(
            requests.iter().any(|request| top(request) == top(doc)),
            "`{example}` leaves its own request out of `requests`: {doc}"
        );
    }
}

/// The scan reads the indented lines after `Examples:` up to the first line that is not
/// indented, and nothing from a help without examples; the path skips a leading global
/// flag and takes the longest match.
#[test]
fn the_example_scan_reads_the_section_and_the_longest_path() {
    let help = "About\n\nExamples:\n  hipc one\n  hipc two --yes\n\nExit codes:\n  0  ok\n";
    assert_eq!(examples_in(help), ["hipc one", "hipc two --yes"]);
    assert_eq!(
        examples_in("About\n  hipc not-an-example\n"),
        Vec::<String>::new()
    );

    let commands: Commands = [("a".to_owned(), None), ("a b".to_owned(), None)].into();
    assert_eq!(path_of(&["--dry-run", "a", "b", "c"], &commands), "a b");
    assert_eq!(path_of(&["a", "c"], &commands), "a");
}
