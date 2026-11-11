//! The confirmation prompt, the output mode, and the VM launch wait default at a real
//! terminal (tenets 3, 5, and 15): `hipc` runs with its stdin and stdout on
//! a pseudo-terminal, and stderr on it or on a pipe.

use std::fs::File;
use std::io::Write as _;
use std::process::{Output, Stdio};

use crate::common::terminal::{Screen, pty};
use crate::common::{self, Home, Reply, Server, TOKEN, no_server, route, route_each};

/// Runs `hipc --output text auth logout` at a terminal, as `at_a_terminal` does.
fn logout_at_a_terminal(typed: &[u8], stderr_on_terminal: bool) -> (Output, String) {
    let args = ["--output", "text", "auth", "logout"];
    at_a_terminal(&args, &[], typed, stderr_on_terminal)
}

/// Runs `hipc <args>` with `env` set, stdin and stdout on a terminal, and stderr on it
/// too, or on a pipe when `stderr_on_terminal` is false; types `typed`, and returns the
/// exit status, what the terminal showed (line breaks as `\n`), and the piped stderr.
///
/// `typed` is typed a line at a time, each line once one more question shows than lines were
/// typed, as a person answers: `hipc` discards what was typed before a question appeared.
/// Once `hipc` has exited, the lines left are not typed.
fn at_a_terminal(
    args: &[&str],
    env: &[(&str, &str)],
    typed: &[u8],
    stderr_on_terminal: bool,
) -> (Output, String) {
    let home = Home::new();
    let (master, terminal) = pty();
    let side = || Stdio::from(terminal.try_clone().expect("clone the terminal side"));
    let mut command = common::command(&home);
    command
        .envs(env.iter().copied())
        .args(args)
        .stdin(side())
        .stdout(side())
        .stderr(if stderr_on_terminal {
            side()
        } else {
            Stdio::piped()
        });
    let mut child = command.spawn().expect("spawn hipc");
    // `command` holds the copies of the terminal side it was given until dropped.
    drop(command);
    let mut keyboard = File::from(master);
    let screen = Screen::read(&keyboard);
    for (answered, line) in typed.split_inclusive(|&b| b == b'\n').enumerate() {
        let asked = screen.wait_until(
            &format!("question {answered}"),
            |shown| shown.matches("[y/N] ").count() > answered,
            || child.try_wait().expect("poll hipc").is_some(),
        );
        if !asked {
            break;
        }
        keyboard.write_all(line).expect("type into the terminal");
    }
    let output = common::finish(child, None, args);
    // On the terminal, stderr is part of the screen, which `close` checks.
    let output = if stderr_on_terminal {
        output
    } else {
        common::check_output(output)
    };
    let shown = screen.close();
    drop(terminal);
    (output, shown)
}

const PROMPT: &str =
    "this will remove the stored token from the profile `default`; continue? [y/N] ";

/// The refusal when nobody could be asked.
const REFUSAL: &str = "error: this would remove the stored token from the profile `default`, \
     and `--yes` was not passed\nhint: pass `--yes` to confirm\n";

#[test]
fn a_person_who_answers_no_is_told_the_prompt_was_declined() {
    let (output, shown) = logout_at_a_terminal(b"n\n", true);
    assert_eq!(output.status.code(), Some(2));
    assert!(shown.contains(PROMPT), "{shown:?}");
    assert!(
        shown.contains(concat!(
            "\nerror: the prompt to remove the stored token from the profile `default` was ",
            "not answered yes\nhint: pass `--yes` to confirm\n"
        )),
        "{shown:?}"
    );
}

#[test]
fn a_yes_at_the_prompt_runs_the_command() {
    let (output, shown) = logout_at_a_terminal(b"yes\n", true);
    assert_eq!(output.status.code(), Some(0), "{shown:?}");
    assert!(shown.contains(PROMPT), "{shown:?}");
    assert!(shown.contains("token_stored"), "{shown:?}");
}

/// End of input at the prompt is a no, and the `error:` line starts a line of its own.
#[test]
fn end_of_input_at_the_prompt_ends_its_line_before_the_error() {
    // Control-D, the terminal's end of file.
    let (output, shown) = logout_at_a_terminal(b"\x04", true);
    assert_eq!(output.status.code(), Some(2));
    assert!(shown.contains(PROMPT), "{shown:?}");
    assert!(
        shown
            .lines()
            .any(|line| line.starts_with("error: the prompt to remove")),
        "{shown:?}"
    );
}

/// A stderr that is not a terminal would hide the prompt while `hipc` waits, so there is no
/// prompt, and the refusal says nobody was asked.
#[test]
fn a_stderr_that_is_not_a_terminal_gets_the_refusal_and_no_prompt() {
    let (output, shown) = logout_at_a_terminal(b"yes\n", false);
    assert_eq!(output.status.code(), Some(2));
    assert!(!shown.contains("continue?"), "{shown:?}");
    assert_eq!(String::from_utf8_lossy(&output.stderr), REFUSAL);
}

/// The value observed in a Claude Code 2.1.310 Bash tool environment.
const AI_AGENT_SET: (&str, &str) = ("AI_AGENT", "claude-code_2-1-310_agent");

/// `auto` is JSON at a terminal when `AI_AGENT` counts, and text otherwise; `--output
/// text` wins over the variable.
#[test]
fn ai_agent_makes_auto_json_at_a_terminal() {
    let (output, shown) = at_a_terminal(&["version"], &[AI_AGENT_SET], b"", true);
    assert!(output.status.success(), "{shown:?}");
    assert!(shown.starts_with("{\"version\":"), "{shown:?}");
    for env in [vec![], vec![("AI_AGENT", " off ")], vec![("AI_AGENT", "")]] {
        let (output, shown) = at_a_terminal(&["version"], &env, b"", true);
        assert!(output.status.success(), "{env:?}: {shown:?}");
        assert!(shown.starts_with("version  "), "{env:?}: {shown:?}");
    }
    let args = ["--output", "text", "version"];
    let (output, shown) = at_a_terminal(&args, &[AI_AGENT_SET], b"", true);
    assert!(output.status.success(), "{shown:?}");
    assert!(shown.starts_with("version  "), "{shown:?}");
}

/// An agent at a terminal is refused, not asked, even in text mode: the typed yes is never
/// read. The terminal echoes it, before or after `hipc`'s lines, so the checks look for
/// those lines rather than at the start or the end of the screen.
#[test]
fn ai_agent_refuses_the_prompt_even_with_output_text() {
    let args = ["--output", "text", "auth", "logout"];
    let (output, shown) = at_a_terminal(&args, &[AI_AGENT_SET], b"yes\n", true);
    assert_eq!(output.status.code(), Some(2), "{shown:?}");
    assert!(!shown.contains("continue?"), "{shown:?}");
    assert!(shown.contains(REFUSAL), "{shown:?}");
}

#[test]
fn ai_agent_under_auto_refuses_with_the_json_error() {
    let (output, shown) = at_a_terminal(&["auth", "logout"], &[AI_AGENT_SET], b"yes\n", true);
    assert_eq!(output.status.code(), Some(2), "{shown:?}");
    assert!(!shown.contains("continue?"), "{shown:?}");
    let line = shown
        .lines()
        .find(|line| line.starts_with('{'))
        .unwrap_or_else(|| panic!("the terminal shows a JSON error object: {shown:?}"));
    // The terminal echoes the typed `yes` whenever it arrives, which can be on the same line
    // right after hipc's JSON; the error is the first JSON value on the line.
    let error: serde_json::Value = serde_json::Deserializer::from_str(line)
        .into_iter()
        .next()
        .expect("the line starts with a JSON value")
        .expect("the error object is one line of JSON");
    assert_eq!(error["error"]["code"], "confirmation_required", "{shown:?}");
    assert_eq!(
        error["error"]["hint"], "pass `--yes` to confirm",
        "{shown:?}"
    );
}

/// Detection never takes away a flag: `--yes` still confirms under `AI_AGENT`.
#[test]
fn yes_still_confirms_under_ai_agent() {
    let (output, shown) = at_a_terminal(&["--yes", "auth", "logout"], &[AI_AGENT_SET], b"", true);
    assert_eq!(output.status.code(), Some(0), "{shown:?}");
    assert!(shown.contains("\"token_stored\":"), "{shown:?}");
}

/// `AI_AGENT` set to an off word, and `CLAUDECODE` on its own, leave a person at a
/// terminal with the prompt and with text.
#[test]
fn an_off_ai_agent_or_claudecode_alone_still_prompts() {
    let args = ["--output", "text", "auth", "logout"];
    for env in [("AI_AGENT", "No"), ("CLAUDECODE", "1")] {
        let (output, shown) = at_a_terminal(&args, &[env], b"yes\n", true);
        assert_eq!(output.status.code(), Some(0), "{env:?}: {shown:?}");
        assert!(shown.contains(PROMPT), "{env:?}: {shown:?}");
    }
    let (_, shown) = at_a_terminal(&["version"], &[("CLAUDECODE", "1")], b"", true);
    assert!(shown.starts_with("version  "), "{shown:?}");
}

/// What reaches a terminal is redacted and escaped in every mode (tenet 15): `AI_AGENT`
/// puts a person's terminal in JSON mode, and the JSON path keeps query values and raw
/// bytes only for a pipe.
#[test]
fn a_terminal_in_json_mode_gets_redacted_urls_and_escaped_bodies() {
    let server = Server::start(vec![route(
        "GET",
        "/api/x/",
        Reply::json(200, "{\"a\":\"\u{1b}]0;owned\u{7}\"}"),
    )]);
    let env = [
        AI_AGENT_SET,
        ("HIPPIUS_API_URL", server.url()),
        ("HIPPIUS_TOKEN", TOKEN),
        ("HIPPIUS_TOKEN_ORIGIN", server.url()),
    ];
    let (_, shown) = at_a_terminal(
        &[
            "api",
            "--reveal",
            "GET",
            "/x/?token=s3cr3t&page=2",
            "--dry-run",
        ],
        &env,
        b"",
        true,
    );
    assert!(!shown.contains("s3cr3t"), "{shown}");
    assert!(shown.contains("token=<redacted>"), "{shown}");
    let (output, shown) = at_a_terminal(&["api", "--reveal", "GET", "/x/"], &env, b"", true);
    assert!(output.status.success(), "{shown}");
    assert!(!shown.contains('\u{1b}'), "{shown:?}");
    assert!(shown.contains("\\u001b]0;owned\\u0007"), "{shown:?}");
}

/// The Drive cancel prompt does not promise a Free plan: a wallet-only account has none.
/// The answer is no, so nothing is sent.
#[test]
fn the_drive_cancel_prompt_names_who_goes_back_to_free() {
    let args = ["--output", "text", "drive", "subscription", "cancel"];
    let env = [("HIPPIUS_API_URL", no_server()), ("HIPPIUS_TOKEN", TOKEN)];
    let (output, shown) = at_a_terminal(&args, &env, b"n\n", true);
    assert_eq!(output.status.code(), Some(2), "{shown:?}");
    assert!(
        shown.contains(
            "this will cancel the Drive plan; the plan stays until the end of the period paid \
             for, with no refund, and then a Google, GitHub, or Apple account goes back to Free \
             Drive, and a wallet-only account is left with no Drive plan; continue? [y/N] "
        ),
        "{shown:?}"
    );
}

/// The S3 cancel prompt says the plan runs to the end of the month paid for, with no
/// refund, and then the account is on pay as you go, as the billing docs say.
#[test]
fn the_s3_cancel_prompt_names_the_paid_month_and_pay_as_you_go() {
    let args = ["--output", "text", "s3", "subscription", "cancel"];
    let env = [("HIPPIUS_API_URL", no_server()), ("HIPPIUS_TOKEN", TOKEN)];
    let (output, shown) = at_a_terminal(&args, &env, b"n\n", true);
    assert_eq!(output.status.code(), Some(2), "{shown:?}");
    assert!(
        shown.contains(
            "this will cancel the S3 plan; the plan stays until the end of the month paid for, \
             with no refund, and then the account is on pay as you go; continue? [y/N] "
        ),
        "{shown:?}"
    );
}

/// The Hub cancel prompt names the 30-day grace period and what is deleted after it
/// (docs.hippius.com/use/console/hub).
#[test]
fn the_hub_cancel_prompt_names_the_grace_period_and_the_deletion() {
    let args = ["--output", "text", "hub", "subscription", "cancel", "7"];
    let env = [("HIPPIUS_API_URL", no_server()), ("HIPPIUS_TOKEN", TOKEN)];
    let (output, shown) = at_a_terminal(&args, &env, b"n\n", true);
    assert_eq!(output.status.code(), Some(2), "{shown:?}");
    assert!(
        shown.contains(
            "this will cancel the Hub subscription `7`; the namespace stays available for a \
             30-day grace period, and subscribing again within it keeps everything; when it \
             ends, the namespace and everything in it are deleted for good; continue? [y/N] "
        ),
        "{shown:?}"
    );
}

/// The Hub deletes name what is lost and that it cannot be undone. The answer is no,
/// so nothing is sent.
#[test]
fn the_hub_delete_prompts_name_what_is_lost() {
    let env = [("HIPPIUS_API_URL", no_server()), ("HIPPIUS_TOKEN", TOKEN)];
    for (args, question) in [
        (
            &["hub", "repositories", "delete", "acme/app"][..],
            "this will delete the Hub repository `acme/app` with every artifact and tag in it; \
             this cannot be undone; continue? [y/N] ",
        ),
        (
            &["hub", "artifacts", "delete", "acme/app", "v1"],
            "this will delete the artifact `v1` from the Hub repository `acme/app`, with every \
             tag on it; this cannot be undone; continue? [y/N] ",
        ),
    ] {
        let mut argv = vec!["--output", "text"];
        argv.extend(args);
        let (output, shown) = at_a_terminal(&argv, &env, b"n\n", true);
        assert_eq!(output.status.code(), Some(2), "{shown:?}");
        assert!(shown.contains(question), "{shown:?}");
    }
}

/// `hipc api` warns on a Hub repository or artifact delete as the curated commands do, in
/// the slashless and suffixed forms too; another DELETE gets the bare question. The answer
/// is no, so nothing is sent.
#[test]
fn the_api_hub_delete_prompt_says_it_cannot_be_undone() {
    let env = [("HIPPIUS_API_URL", no_server()), ("HIPPIUS_TOKEN", TOKEN)];
    for (path, warned) in [
        ("/registry/repositories/acme/app/", true),
        ("/registry/repositories/acme/app/artifacts/v1", true),
        ("/registry/repositories/acme/app.json/", true),
        ("/registry/namespaces/7/", false),
    ] {
        let (output, shown) = at_a_terminal(
            &["--output", "text", "api", "DELETE", path],
            &env,
            b"n\n",
            true,
        );
        assert_eq!(output.status.code(), Some(2), "{shown:?}");
        let call = format!("this will call `DELETE {}/api{path}`", no_server());
        let question = if warned {
            format!("{call}; this cannot be undone; continue? [y/N] ")
        } else {
            format!("{call}; continue? [y/N] ")
        };
        assert!(shown.contains(&question), "{path}: {shown:?}");
    }
}

/// Each `hipc api` request, and the consequence its question gives, or none.
const API_CONSEQUENCES: [(&str, &str, Option<&str>); 16] = [
    (
        "DELETE",
        "/registry/subscriptions/7/",
        Some(
            "the namespace stays available for a 30-day grace period, and subscribing again \
             within it keeps everything; when it ends, the namespace and everything in it \
             are deleted for good",
        ),
    ),
    (
        "DELETE",
        "/drive/subscription",
        Some(
            "the plan stays until the end of the period paid for, with no refund, and then \
             a Google, GitHub, or Apple account goes back to Free Drive, and a wallet-only \
             account is left with no Drive plan",
        ),
    ),
    (
        "DELETE",
        "/s3/subscription/",
        Some(
            "the plan stays until the end of the month paid for, with no refund, and then \
             the account is on pay as you go",
        ),
    ),
    (
        "DELETE",
        "/registry/keys/7/",
        Some("every CI job that uses its secret stops working"),
    ),
    (
        "DELETE",
        "/compute/vms/vm-1/",
        Some(
            "an attached public IPv4 is released with the VM and billed until then; stored \
             backups are billed until they are removed",
        ),
    ),
    (
        "DELETE",
        "/compute/vms/vm-1/public-ip/",
        Some("a later attach may get a different address"),
    ),
    (
        "DELETE",
        "/compute/vms/vm-1/expose/3/",
        Some("its hostname cannot be used again"),
    ),
    (
        "DELETE",
        "/cdn/zones/z1.json",
        Some("its hostnames stop serving, and its billing stops"),
    ),
    (
        "DELETE",
        "/cdn/zones/z1/hostnames/a.com/",
        Some("it stops serving, and its billing stops"),
    ),
    (
        "DELETE",
        "/runners/installations/7/",
        Some("its repositories stop getting Hippius runners"),
    ),
    (
        "DELETE",
        "/network/peers/7/",
        Some("it loses its access to the private network"),
    ),
    (
        "DELETE",
        "/compute/vms/vm-1/backup/",
        Some("the backups already taken are kept 7 days, billed while kept, then deleted"),
    ),
    ("DELETE", "/ssh-keys/3/", Some("a VM that has it keeps it")),
    (
        "DELETE",
        "/cdn/lists/blocked/items/",
        Some("every zone whose WAF uses the list publishes it again"),
    ),
    ("POST", "/compute/vms/vm-1/", None),
    ("DELETE", "/compute/vms/", None),
];

/// A delete or cancel through `hipc api` whose curated command says what it ends asks with
/// the same consequence, on its slashless and suffixed paths too; a `POST` to the same path
/// asks with none. The answer is no, so nothing is sent.
#[test]
fn the_api_prompt_gives_the_curated_consequence() {
    let env = [("HIPPIUS_API_URL", no_server()), ("HIPPIUS_TOKEN", TOKEN)];
    for (method, path, consequence) in API_CONSEQUENCES {
        let (output, shown) = at_a_terminal(
            &["--output", "text", "api", method, path],
            &env,
            b"n\n",
            true,
        );
        assert_eq!(output.status.code(), Some(2), "{shown:?}");
        let call = format!("this will call `{method} {}/api{path}`", no_server());
        let question = match consequence {
            Some(consequence) => format!("{call}; {consequence}; continue? [y/N] "),
            None => format!("{call}; continue? [y/N] "),
        };
        assert!(shown.contains(&question), "{path}: {shown:?}");
    }
}

/// The Hub key rotate and delete prompts name the key and say that every CI job using its
/// old secret stops working. The answer is no, so nothing is sent.
#[test]
fn the_hub_key_prompts_say_ci_jobs_stop_working() {
    let env = [("HIPPIUS_API_URL", no_server()), ("HIPPIUS_TOKEN", TOKEN)];
    for (args, question) in [
        (
            &["--output", "text", "hub", "keys", "rotate", "7", "--reveal"][..],
            "this will rotate the secret of the Hub key `7`; every CI job that uses its old \
             secret stops working, and the new secret is printed on stdout once; continue? \
             [y/N] ",
        ),
        (
            &["--output", "text", "hub", "keys", "delete", "7"],
            "this will delete the Hub key `7`; every CI job that uses its secret stops \
             working; continue? [y/N] ",
        ),
    ] {
        let (output, shown) = at_a_terminal(args, &env, b"n\n", true);
        assert_eq!(output.status.code(), Some(2), "{shown:?}");
        assert!(shown.contains(question), "{shown:?}");
    }
}

/// The launch prompt with `--public-ip` names the address, where its hourly price is
/// shown, and how long each is billed. The answer is no, so nothing is sent.
#[test]
fn the_launch_prompt_with_a_public_ip_names_its_price() {
    let args = [
        "--output",
        "text",
        "compute",
        "vms",
        "create",
        "--flavor",
        "small",
        "--ssh-key-id",
        "7",
        "--public-ip",
    ];
    let env = [("HIPPIUS_API_URL", no_server()), ("HIPPIUS_TOKEN", TOKEN)];
    let (output, shown) = at_a_terminal(&args, &env, b"n\n", true);
    assert_eq!(output.status.code(), Some(2), "{shown:?}");
    assert!(
        shown.contains(
            "this will launch a VM of flavor `small` with a public IPv4, billed per second at the \
             price that `hipc compute public-ip availability` shows (`price_hourly`, or free \
             until `free_until` while it is set, then `price_hourly_after_free`); the VM is \
             billed until it is deleted, and the address until it is detached or the VM is \
             deleted; continue? [y/N] "
        ),
        "{shown:?}"
    );
}

/// Turning the WAF off names what stops in the prompt: its rules and its billing.
#[test]
fn the_waf_off_prompt_names_what_stops() {
    let args = [
        "--output",
        "text",
        "cdn",
        "waf",
        "update",
        "zab12cd34ef56",
        "--mode",
        "off",
    ];
    let env = [("HIPPIUS_API_URL", no_server()), ("HIPPIUS_TOKEN", TOKEN)];
    let (output, shown) = at_a_terminal(&args, &env, b"n\n", true);
    assert_eq!(output.status.code(), Some(2), "{shown:?}");
    assert!(
        shown.contains(
            "this will update the WAF of the CDN zone `zab12cd34ef56`: set `mode` to `off`; the \
             zone's WAF rules stop applying, and its WAF billing stops; continue? [y/N] "
        ),
        "{shown:?}"
    );
}

/// A no to the origin question sends nothing: the invite token stays on the machine.
#[test]
fn a_no_to_the_invite_origin_question_sends_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let invite = home.file("invite.txt", b"inv_terminal_secret\n");
    let path = invite.to_str().expect("a temporary path is UTF-8");
    let args = [
        "--output",
        "text",
        "account",
        "invites",
        "preview",
        "--token-file",
        path,
    ];
    let env = [("HIPPIUS_API_URL", server.url())];
    let (output, shown) = at_a_terminal(&args, &env, b"n\n", true);
    assert_eq!(output.status.code(), Some(2), "{shown:?}");
    let declined = format!(
        "error: the prompt to send the invite token to `{}`, named by `HIPPIUS_API_URL` was \
         not answered yes",
        server.url()
    );
    assert!(shown.contains(&declined), "{shown:?}");
    assert!(!shown.contains("inv_terminal_secret"), "{shown:?}");
    assert!(server.requests().is_empty());
}

/// At an origin other than the default, `account invites accept` asks twice: first whether
/// the invite token may go to that origin, before the preview sends it (tenet 8), then
/// whether to join the account the preview names, and who sent the invite when the preview
/// says. A no to the second sends no accept.
#[test]
fn accept_at_another_origin_asks_for_the_origin_then_names_the_account() {
    let consequence = concat!(
        "; the token then acts in that account with the access the invite grants; ",
        "continue? [y/N] "
    );
    for (preview, joins) in [
        (
            r#"{"account_name":"Boss Co","invited_by":"boss@example.com","valid":true}"#,
            "join the account `Boss Co`, invited by `boss@example.com`",
        ),
        (
            r#"{"account_name":"Boss Co","valid":true}"#,
            "join the account `Boss Co`",
        ),
    ] {
        let server = Server::start(vec![route(
            "POST",
            "/api/accounts/invites/preview/",
            Reply::json(201, preview),
        )]);
        let home = Home::new();
        let invite = home.file("invite.txt", b"inv_terminal_secret\n");
        let args = [
            "--output",
            "text",
            "account",
            "invites",
            "accept",
            "--token-file",
            invite.to_str().expect("a temporary path is UTF-8"),
        ];
        let env = [
            ("HIPPIUS_API_URL", server.url()),
            ("HIPPIUS_TOKEN", TOKEN),
            ("HIPPIUS_TOKEN_ORIGIN", server.url()),
        ];
        let (output, shown) = at_a_terminal(&args, &env, b"y\nn\n", true);
        assert_eq!(output.status.code(), Some(2), "{shown:?}");
        let origin = format!(
            "this will send the invite token to `{}`, named by `HIPPIUS_API_URL`; whoever \
             answers there holds the invite token; continue? [y/N] ",
            server.url()
        );
        let join = format!("this will {joins}{consequence}");
        let asked_origin = shown.find(&origin).expect("the origin question is shown");
        let asked_join = shown.find(&join).expect("the join question is shown");
        assert!(asked_origin < asked_join, "{shown:?}");
        assert!(!shown.contains("inv_terminal_secret"), "{shown:?}");
        let requests = server.requests();
        assert_eq!(requests.len(), 1, "only the preview");
        assert_eq!(requests[0].path(), "/api/accounts/invites/preview/");
    }
}

/// `hipc compute vms create` at a terminal, with `env` and the extra `args`, against a
/// server whose VM is `pending` at the launch and `status` at every poll. Returns the
/// exit status, the screen, and the requests `hipc` sent, in order.
fn launch_at_a_terminal(
    args: &[&str],
    env: &[(&str, &str)],
    status: &str,
) -> (Output, String, Vec<String>) {
    let vm = |status: &str| format!(r#"{{"vm_id":"vm-1","status":"{status}"}}"#);
    let server = Server::start(vec![
        route(
            "POST",
            "/api/compute/vms/",
            Reply::json(202, &vm("pending")),
        ),
        route(
            "GET",
            "/api/compute/vms/vm-1/",
            Reply::json(200, &vm(status)),
        ),
    ]);
    let mut argv = vec![
        "compute",
        "vms",
        "create",
        "--flavor",
        "small",
        "--ssh-key-id",
        "7",
        "--yes",
    ];
    argv.extend(args);
    let mut env = env.to_vec();
    env.extend([
        ("HIPPIUS_API_URL", server.url()),
        ("HIPPIUS_TOKEN", TOKEN),
        ("HIPPIUS_TOKEN_ORIGIN", server.url()),
    ]);
    let (output, shown) = at_a_terminal(&argv, &env, b"", true);
    let sent = server
        .requests()
        .iter()
        .map(|request| format!("{} {}", request.method, request.path()))
        .collect();
    (output, shown, sent)
}

const LAUNCH: &str = "POST /api/compute/vms/";
const POLL: &str = "GET /api/compute/vms/vm-1/";

/// A person at a terminal waits for the launch, and `--wait-timeout` applies there without
/// `--wait`.
#[test]
fn a_vm_launch_waits_at_a_terminal() {
    let (output, shown, sent) = launch_at_a_terminal(&[], &[], "running");
    assert_eq!(output.status.code(), Some(0), "{shown:?}");
    assert!(
        shown.contains("note: launched the VM `vm-1`; waiting until it is running\n"),
        "{shown:?}"
    );
    assert_eq!(sent, [LAUNCH, POLL]);
    let args = ["--wait-timeout", "1"];
    let (output, shown, _) = launch_at_a_terminal(&args, &[], "provisioning");
    assert_eq!(output.status.code(), Some(1), "{shown:?}");
    assert!(shown.contains("after 1 second"), "{shown:?}");
    let (_, _, sent) = launch_at_a_terminal(&["--no-wait"], &[], "running");
    assert_eq!(sent, [LAUNCH], "--no-wait wins at a terminal");
    // The default follows where stdout goes, not the output mode `--json` picks.
    let (output, shown, sent) = launch_at_a_terminal(&["--json"], &[], "running");
    assert_eq!(output.status.code(), Some(0), "{shown:?}");
    assert_eq!(sent, [LAUNCH, POLL], "--json at a terminal still waits");
}

/// `CI` or `AI_AGENT` at a terminal picks JSON, and so the launch answer without a wait,
/// as on a pipe; `--wait` still waits.
#[test]
fn ci_or_ai_agent_at_a_terminal_launches_without_a_wait() {
    for env in [("CI", ""), AI_AGENT_SET] {
        let (output, shown, sent) = launch_at_a_terminal(&[], &[env], "running");
        assert_eq!(output.status.code(), Some(0), "{env:?}: {shown:?}");
        assert!(
            shown.contains(r#""status":"pending""#),
            "{env:?}: {shown:?}"
        );
        assert_eq!(sent, [LAUNCH], "{env:?}");
        let (output, shown, sent) = launch_at_a_terminal(&["--wait"], &[env], "running");
        assert_eq!(output.status.code(), Some(0), "{env:?}: {shown:?}");
        assert!(
            shown.contains(r#""status":"running""#),
            "{env:?}: {shown:?}"
        );
        assert_eq!(sent, [LAUNCH, POLL], "{env:?}");
    }
}

/// Runs `hipc --output text auth logout` at a terminal in `home`, runs `meanwhile` once the
/// prompt shows, as another `hipc` would while the person reads it, then answers yes. Returns
/// the exit code and what the terminal showed.
fn logout_while_another_run_changes_the_token(
    home: &Home,
    meanwhile: impl FnOnce(),
) -> (Option<i32>, String) {
    while_another_run_changes_the_token(home, &["auth", "logout"], meanwhile)
}

/// [`logout_while_another_run_changes_the_token`] for any `hipc --output text <args>`, with
/// `HIPPIUS_API_URL` unset, so a login asks only about the token it replaces.
fn while_another_run_changes_the_token(
    home: &Home,
    args: &[&str],
    meanwhile: impl FnOnce(),
) -> (Option<i32>, String) {
    let (master, terminal) = pty();
    let side = || Stdio::from(terminal.try_clone().expect("clone the terminal side"));
    let mut command = common::command(home);
    command
        .env_remove("HIPPIUS_API_URL")
        .args(["--output", "text"])
        .args(args)
        .stdin(side())
        .stdout(side())
        .stderr(side());
    let child = command.spawn().expect("spawn hipc");
    drop(command);
    let mut keyboard = File::from(master);
    let screen = Screen::read(&keyboard);
    screen.wait_until("the prompt", |shown| shown.contains("[y/N] "), || false);
    meanwhile();
    keyboard.write_all(b"y\n").expect("type into the terminal");
    let status = common::finish(child, None, args).status;
    let shown = screen.close();
    drop(terminal);
    (status.code(), shown)
}

/// Stores `token` in the default profile of `home`, as a second `hipc` run would.
fn login_with(home: &Home, token: &str) {
    let file = home.file("token", token.as_bytes());
    let run = common::Hipc::new(home).run(&[
        "--yes",
        "auth",
        "login",
        "--token-file",
        file.to_str().expect("a UTF-8 temp path"),
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
}

/// A logout removes the token its question was about. A token another run stores or
/// replaces while the prompt waits was not confirmed, so it stays, and the logout exits 2.
#[test]
fn logout_leaves_a_token_stored_while_its_prompt_waited() {
    let replacement = "token-replaced-during-prompt";
    // No token when asked, one stored meanwhile.
    let home = Home::new();
    let (code, shown) =
        logout_while_another_run_changes_the_token(&home, || login_with(&home, replacement));
    assert_eq!(code, Some(2), "{shown}");
    assert!(
        shown.contains(
            "error: the stored token of the profile `default` changed while the question \
             waited, so it was left in place\nhint: run `hipc --output text auth logout` to \
             remove the token stored since\n"
        ),
        "{shown:?}"
    );
    assert!(home.config_text().contains(replacement));
    // One token when asked, another stored meanwhile.
    let home = Home::new();
    login_with(&home, "token-confirmed-at-the-prompt");
    let (code, shown) =
        logout_while_another_run_changes_the_token(&home, || login_with(&home, replacement));
    assert_eq!(code, Some(2), "{shown}");
    assert!(home.config_text().contains(replacement), "{shown}");
    // Another logout removed it meanwhile: the goal is met, and nothing is left to keep.
    let home = Home::new();
    login_with(&home, "token-confirmed-at-the-prompt");
    let (code, shown) = logout_while_another_run_changes_the_token(&home, || {
        let run = common::Hipc::new(&home).run(&["--yes", "auth", "logout"]);
        assert_eq!(run.code(), Some(0), "{}", run.stderr());
    });
    assert_eq!(code, Some(0), "{shown}");
    assert!(!home.config_text().contains("token-confirmed"), "{shown}");
    // The token the question named is removed.
    let home = Home::new();
    login_with(&home, "token-confirmed-at-the-prompt");
    let (code, shown) = logout_while_another_run_changes_the_token(&home, || {});
    assert_eq!(code, Some(0), "{shown}");
    assert!(!home.config_text().contains("token-confirmed"), "{shown}");
}

/// A login that replaces a stored token asks first. A different token another run stores
/// while the prompt waits was not confirmed, so it stays, and the login exits 2; the same
/// token stored meanwhile, or none, lets the login go on.
#[test]
fn login_leaves_a_token_stored_while_its_prompt_waited() {
    let home = Home::new();
    login_with(&home, "token-confirmed-at-the-prompt");
    let file = home.file("token-new", b"token-the-login-stores");
    let args = [
        "auth",
        "login",
        "--token-file",
        file.to_str().expect("UTF-8"),
    ];
    let (code, shown) = while_another_run_changes_the_token(&home, &args, || {
        login_with(&home, "token-replaced-during-prompt");
    });
    assert_eq!(code, Some(2), "{shown}");
    assert!(
        shown.contains(
            "this will replace the token stored in the profile `default` with a different \
             one; every later command with this profile acts as the new token's owner; \
             continue? [y/N] "
        ),
        "{shown:?}"
    );
    assert!(
        shown.contains(
            "error: the stored token of the profile `default` changed while the question waited"
        ),
        "{shown:?}"
    );
    assert!(home.config_text().contains("token-replaced-during-prompt"));
    // The token the login stores, stored meanwhile by another run: nothing unconfirmed is lost.
    let (code, shown) = while_another_run_changes_the_token(&home, &args, || {
        login_with(&home, "token-the-login-stores");
    });
    assert_eq!(code, Some(0), "{shown}");
    // Nothing changed meanwhile: the confirmed replacement is made.
    login_with(&home, "token-confirmed-at-the-prompt");
    let (code, shown) = while_another_run_changes_the_token(&home, &args, || {});
    assert_eq!(code, Some(0), "{shown}");
    assert!(
        home.config_text().contains("token-the-login-stores"),
        "{shown}"
    );
}

/// The namespace update question says who can pull after it and that the storage allowance
/// changes; the robot rotate question says the old secret stops working. The answer is no,
/// so nothing is sent.
#[test]
fn the_hub_namespace_prompts_name_what_changes() {
    let env = [("HIPPIUS_API_URL", no_server()), ("HIPPIUS_TOKEN", TOKEN)];
    for (args, question) in [
        (
            &[
                "hub",
                "namespace",
                "update",
                "--visibility",
                "public",
                "--public",
            ][..],
            "this will make the Hub namespace public; anyone can pull its images and models \
             without logging in, and its storage allowance changes; continue? [y/N] ",
        ),
        (
            &["hub", "namespace", "update", "--visibility", "private"],
            "this will make the Hub namespace private; only logins with access can pull from \
             it, and its storage allowance changes; continue? [y/N] ",
        ),
        (
            &["hub", "namespace", "robot", "rotate", "--reveal"],
            "this will rotate the Hub robot secret; every docker login that uses the old secret \
             stops working, and the new secret is printed on stdout once; continue? [y/N] ",
        ),
    ] {
        let mut argv = vec!["--output", "text"];
        argv.extend(args);
        let (output, shown) = at_a_terminal(&argv, &env, b"n\n", true);
        assert_eq!(output.status.code(), Some(2), "{shown:?}");
        assert!(shown.contains(question), "{shown:?}");
    }
}

/// The App Engine launch asks after its compose check, in the words the owner chose, and a
/// no sends nothing more: the check went out, the launch did not, and no secret left.
#[test]
fn the_app_launch_question_names_the_flavor_the_address_and_the_spend() {
    let validated = concat!(
        r#"{"compose_sha256":"#,
        r#""5f6fad3dce5125e4e5595e34672d6d2bbe6149ab65095b42d9f77d66e3f8202c","#,
        r#""secret_names":["DB_PASSWORD"]}"#
    );
    let server = Server::start(vec![route(
        "POST",
        "/api/compute/app-engine/validate/",
        Reply::json(200, validated),
    )]);
    let files = Home::new();
    let compose = files.file("compose.yaml", b"services:\n  web:\n    image: nginx\n");
    let secrets = files.file("secrets.json", br#"{"DB_PASSWORD":"terminal-secret-Hk27"}"#);
    let args = [
        "--output",
        "text",
        "compute",
        "app-engine",
        "apps",
        "create",
        "--flavor",
        "small",
        "--region",
        "FR",
        "--compose-file",
        compose.to_str().expect("a temporary path is UTF-8"),
        "--secrets-file",
        secrets.to_str().expect("a temporary path is UTF-8"),
    ];
    let env = [
        ("HIPPIUS_API_URL", server.url()),
        ("HIPPIUS_TOKEN", TOKEN),
        ("HIPPIUS_TOKEN_ORIGIN", server.url()),
    ];
    let (output, shown) = at_a_terminal(&args, &env, b"n\n", true);
    assert_eq!(output.status.code(), Some(2), "{shown:?}");
    assert!(
        shown.contains(
            "this will launch an app VM of flavor `small` with a public IPv4 in `FR`, billed per \
             hour from the balance; continue? [y/N] "
        ),
        "{shown:?}"
    );
    assert!(!shown.contains("terminal-secret-Hk27"), "{shown:?}");
    assert_eq!(
        server.requests().len(),
        1,
        "only the compose check went out"
    );
}

/// The power policy update is not confirmed, as `hipc compute vms start` and `stop` are not:
/// at a terminal it asks nothing and sends the `PUT`, and with `stop` it notes the bill.
#[test]
fn the_power_policy_update_asks_nothing_at_a_terminal() {
    let vm = "0f8fad5b-d9cb-469f-a165-70867728950e";
    let path = format!("/api/compute/vms/{vm}/power-policy/");
    let server = Server::start(vec![route(
        "PUT",
        &path,
        Reply::json(
            200,
            r#"{"on_guest_poweroff":"stop","on_guest_poweroff_pending":false}"#,
        ),
    )]);
    let env = [
        ("HIPPIUS_API_URL", server.url()),
        ("HIPPIUS_TOKEN", TOKEN),
        ("HIPPIUS_TOKEN_ORIGIN", server.url()),
    ];
    let args = [
        "--output",
        "text",
        "compute",
        "vms",
        "power-policy",
        "update",
        vm,
        "--on-guest-poweroff",
        "stop",
    ];
    let (output, shown) = at_a_terminal(&args, &env, b"", true);
    assert_eq!(output.status.code(), Some(0), "{shown:?}");
    assert!(!shown.contains("continue?"), "{shown:?}");
    assert!(shown.contains("still billed as a stopped VM"), "{shown:?}");
    assert_eq!(server.only_request().path(), path);
}

/// A hAlpha buy a person was asked about reads the purchases again after the answer: one
/// another `hipc` or the console started while the question waited blocks the buy, and no
/// buy is sent.
#[test]
fn a_halpha_buy_started_while_the_question_waited_blocks_this_one() {
    let quote = serde_json::json!({
        "enabled": true,
        "stale": false,
        "custodial": true,
        "alpha_price": "4123456789012345678",
        "min_alpha": "10000000000000000",
        "native_balance": "20000000000000000000",
        "address": "5DfhGyQdFobKM8NsWvEeAKk5EQQgYe9AydgJ7rMB6E1EqRzV",
    });
    let empty = r#"{"count":0,"next":null,"previous":null,"results":[]}"#;
    // No `created_at`: a purchase whose time cannot be read counts as recent.
    let started =
        r#"{"count":1,"next":null,"previous":null,"results":[{"id":"p_9","status":"queued"}]}"#;
    let server = Server::start(vec![
        route(
            "GET",
            "/api/billing/native/quote/",
            Reply::json(200, &quote.to_string()),
        ),
        route_each(
            "GET",
            "/api/billing/native/purchases/",
            vec![Reply::json(200, empty), Reply::json(200, started)],
        ),
    ]);
    let args = [
        "--output", "text", "billing", "top-up", "--method", "halpha", "--alpha", "1.5",
    ];
    let env = [
        ("HIPPIUS_API_URL", server.url()),
        ("HIPPIUS_TOKEN", TOKEN),
        ("HIPPIUS_TOKEN_ORIGIN", server.url()),
    ];
    let (output, shown) = at_a_terminal(&args, &env, b"y\n", true);
    assert_eq!(output.status.code(), Some(1), "{shown:?}");
    assert!(
        shown.contains("this will buy credits with `1.5` hAlpha of `5DfhGyQdFobKM8NsWvEeAKk5EQQgYe9AydgJ7rMB6E1EqRzV`"),
        "{shown:?}"
    );
    assert!(
        shown.contains("error: the purchase `p_9` is still `queued`"),
        "{shown:?}"
    );
    let methods: Vec<String> = server
        .requests()
        .iter()
        .map(|r| format!("{} {}", r.method, r.path()))
        .collect();
    assert_eq!(
        methods,
        [
            "GET /api/billing/native/quote/",
            "GET /api/billing/native/purchases/",
            "GET /api/billing/native/purchases/",
        ]
    );
}
