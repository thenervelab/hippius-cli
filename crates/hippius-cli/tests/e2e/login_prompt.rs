//! `hipc auth login` without `--token-file` at a real terminal: it asks for the token, reads it
//! with echo off, and puts the terminal back as it was (tenets 5 and 8).

use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::fd::OwnedFd;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
use rustix::termios::{LocalModes, tcgetattr};

use serde_json::json;

use crate::common::{self, ACCOUNTS, Home, PHRASES, Reply, Server, TOKEN, route};

/// The extra flags and the variables of one run.
type Case<'a> = (&'a [&'a str], &'a [(&'a str, &'a str)]);

const PROMPT: &str =
    "paste the token from `https://console.hippius.com/dashboard/settings` (it is not shown): ";

/// The flags of a run in text mode, where a person can be asked.
const TEXT: &[&str] = &["--output", "text"];

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

/// What one login at a terminal did: its exit code, what the terminal showed (line breaks
/// as `\n`), and whether the terminal echoes again after it.
struct Login {
    code: Option<i32>,
    shown: String,
    echo_after: bool,
}

/// One step at the keyboard: once the screen shows the text, or at once for `None`, type the
/// bytes.
type Step<'a> = (Option<&'a str>, &'a [u8]);

/// Runs `hipc <flags> auth login` with `HIPPIUS_API_URL` unset, so the default origin asks no
/// question about it, and stdin, stdout, and stderr on a terminal, and takes `steps` in order.
fn login_at_a_terminal(
    home: &Home,
    flags: &[&str],
    env: &[(&str, &str)],
    steps: &[Step<'_>],
) -> Login {
    run_at_a_terminal(home, flags, &[], env, steps)
}

/// [`login_at_a_terminal`] with `login_flags` after `auth login`.
fn run_at_a_terminal(
    home: &Home,
    flags: &[&str],
    login_flags: &[&str],
    env: &[(&str, &str)],
    steps: &[Step<'_>],
) -> Login {
    let (master, terminal) = pty();
    let side = || Stdio::from(terminal.try_clone().expect("clone the terminal side"));
    let mut command = common::command(home);
    command
        .env_remove("HIPPIUS_API_URL")
        .envs(env.iter().copied())
        .args(flags)
        .args(["auth", "login"])
        .args(login_flags)
        .stdin(side())
        .stdout(side())
        .stderr(side());
    let mut child = command.spawn().expect("spawn hipc");
    drop(command);
    let mut keyboard = File::from(master);
    let screen = keyboard.try_clone().expect("clone the keyboard side");
    let shown = Arc::new(Mutex::new(Vec::new()));
    let exited = Arc::new(AtomicBool::new(false));
    // As in `prompt::at_a_terminal`: the reader polls, and stops once `hipc` has exited and
    // nothing more arrives.
    let reader = {
        let (shown, exited) = (Arc::clone(&shown), Arc::clone(&exited));
        std::thread::spawn(move || {
            let mut screen = screen;
            let mut buffer = [0_u8; 4096];
            let tick = Timespec {
                tv_sec: 0,
                tv_nsec: 50_000_000,
            };
            loop {
                let done = exited.load(Ordering::Acquire);
                let mut fds = [PollFd::new(&screen, PollFlags::IN)];
                if poll(&mut fds, Some(&tick)).expect("poll the terminal") == 0 {
                    if done {
                        break;
                    }
                    continue;
                }
                match screen.read(&mut buffer) {
                    Ok(0) => break,
                    Err(error) => panic!("read the terminal: {error}"),
                    Ok(n) => shown
                        .lock()
                        .expect("the screen lock")
                        .extend_from_slice(&buffer[..n]),
                }
            }
        })
    };
    for (wait_for, typed) in steps {
        if let Some(text) = wait_for {
            // 30 s: arbitrary, far beyond a debug build's start on a loaded host.
            let deadline = Instant::now() + Duration::from_secs(30);
            while !String::from_utf8_lossy(&shown.lock().expect("the screen lock")).contains(text) {
                assert!(Instant::now() < deadline, "{text:?} never appeared");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        keyboard.write_all(typed).expect("type into the terminal");
    }
    // A login that waits for input nobody types is killed, so the test fails instead of
    // hanging: 30 s, as above.
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll hipc") {
            break status;
        }
        if Instant::now() > deadline {
            child.kill().expect("kill a waiting hipc");
            break child.wait().expect("wait for the killed hipc");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    exited.store(true, Ordering::Release);
    reader.join().expect("the screen reader does not panic");
    let echo_after = tcgetattr(&terminal)
        .expect("read the terminal's mode")
        .local_modes
        .contains(LocalModes::ECHO);
    let shown = String::from_utf8_lossy(&shown.lock().expect("the screen lock")).replace('\r', "");
    Login {
        code: status.code(),
        shown,
        echo_after,
    }
}

/// A token pasted at the prompt is stored, and never shown: echo is off while it is read, and
/// back on after.
#[test]
fn a_token_pasted_at_the_prompt_is_stored_and_not_shown() {
    let home = Home::new();
    let login = login_at_a_terminal(
        &home,
        TEXT,
        &[],
        &[(Some(PROMPT), format!("{TOKEN}\r").as_bytes())],
    );
    assert_eq!(login.code, Some(0), "{:?}", login.shown);
    assert!(login.shown.starts_with(PROMPT), "{:?}", login.shown);
    assert!(!login.shown.contains(TOKEN), "{:?}", login.shown);
    assert!(login.shown.contains("token_stored"), "{:?}", login.shown);
    assert!(login.echo_after);
    assert!(home.config_text().contains(&format!("token = \"{TOKEN}\"")));
}

/// Backspace edits the hidden line, as it would a shown one.
#[test]
fn backspace_edits_the_hidden_token() {
    let home = Home::new();
    let typed = format!("{TOKEN}xy\x7f\x7f\n");
    let login = login_at_a_terminal(&home, TEXT, &[], &[(Some(PROMPT), typed.as_bytes())]);
    assert_eq!(login.code, Some(0), "{:?}", login.shown);
    assert!(home.config_text().contains(&format!("token = \"{TOKEN}\"")));
}

/// Ctrl-C at the prompt ends the login with nothing stored, and the terminal echoes again:
/// the signal key is read as a byte while echo is off, so the process does not die with the
/// terminal still silent.
#[test]
fn ctrl_c_at_the_prompt_stores_nothing_and_restores_the_terminal() {
    let home = Home::new();
    let login = login_at_a_terminal(&home, TEXT, &[], &[(Some(PROMPT), b"half-a-tok\x03")]);
    assert_eq!(login.code, Some(2), "{:?}", login.shown);
    assert!(
        login.shown.contains(&format!(
            "{PROMPT}\nerror: the token prompt was interrupted\nhint: run `hipc --output text \
             auth login` and paste the token at the prompt, or pass `--token-file <path>`\n"
        )),
        "{:?}",
        login.shown
    );
    assert!(!login.shown.contains("half-a-tok"), "{:?}", login.shown);
    assert!(login.echo_after);
    assert!(!home.config().exists());
}

/// An empty line, or Ctrl-D, is no token: nothing is stored.
#[test]
fn an_empty_answer_stores_nothing() {
    for typed in [&b"\n"[..], b"   \n", b"\x04"] {
        let home = Home::new();
        let login = login_at_a_terminal(&home, TEXT, &[], &[(Some(PROMPT), typed)]);
        assert_eq!(login.code, Some(2), "{typed:?}: {:?}", login.shown);
        assert!(
            login
                .shown
                .contains("error: no token was typed at the prompt\n"),
            "{typed:?}: {:?}",
            login.shown
        );
        assert!(!home.config().exists());
    }
}

/// A pasted value that is not one word of visible ASCII is refused as a token from a file is,
/// with the hint to paste it again.
#[test]
fn a_token_with_a_space_is_refused() {
    let home = Home::new();
    let login = login_at_a_terminal(&home, TEXT, &[], &[(Some(PROMPT), b"two words\n")]);
    assert_eq!(login.code, Some(2), "{:?}", login.shown);
    assert!(
        login.shown.contains(
            "error: the token holds a space, a line break, or a character outside visible \
             ASCII\nhint: run `hipc --output text auth login` and paste the token at the prompt"
        ),
        "{:?}",
        login.shown
    );
    assert!(!home.config().exists());
}

/// `CI`, `AI_AGENT`, and `--no-input` each mean nobody may be asked, even at a terminal: the
/// login is refused at once and names `--token-file`, and nothing typed is read.
#[test]
fn ci_ai_agent_and_no_input_refuse_without_asking() {
    let refusal = "error: `--token-file` is required when nobody at a terminal can be asked for \
                   the token\nhint: pass `--token-file <path>`, or `--token-file -` to read the \
                   token from stdin\n";
    let cases: [Case<'_>; 3] = [
        (&[], &[("CI", "1")]),
        (&[], &[("AI_AGENT", "an-agent")]),
        (&["--no-input"], &[]),
    ];
    for (flags, env) in cases {
        let home = Home::new();
        // Ctrl-D, typed at once: a login that asked by mistake ends instead of waiting.
        let flags = [TEXT, flags].concat();
        let login = login_at_a_terminal(&home, &flags, env, &[(None, b"\x04")]);
        assert_eq!(login.code, Some(2), "{env:?}: {:?}", login.shown);
        assert!(!login.shown.contains(PROMPT), "{env:?}: {:?}", login.shown);
        assert!(login.shown.contains(refusal), "{env:?}: {:?}", login.shown);
        assert!(!home.config().exists());
    }
}

/// A line pasted after the token is discarded, not read as the answer to the question that
/// follows: replacing a stored token is confirmed, and a `y` that came in with the paste must
/// not confirm it unseen. The question waits for a typed answer.
#[test]
fn a_line_pasted_after_the_token_does_not_answer_the_next_question() {
    let home = Home::new();
    let first = home.file("first-token", b"token-stored-first");
    let run = common::Hipc::without_api_url(&home).run(&[
        "auth",
        "login",
        "--token-file",
        first.to_str().expect("UTF-8"),
    ]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    let question = "continue? [y/N] ";
    let login = login_at_a_terminal(
        &home,
        TEXT,
        &[],
        &[
            (Some(PROMPT), format!("{TOKEN}\ny\n").as_bytes()),
            (Some(question), b"n\n"),
        ],
    );
    assert_eq!(login.code, Some(2), "{:?}", login.shown);
    assert!(
        login.shown.contains(
            "this will replace the token stored in the profile `default` with a different one"
        ),
        "{:?}",
        login.shown
    );
    assert!(
        login.shown.contains("was not answered yes"),
        "{:?}",
        login.shown
    );
    assert!(home.config_text().contains("token-stored-first"));
}

/// JSON output at a terminal asks nothing, as a confirmation does not: an agent in a pty gets
/// the refusal, not a wait.
#[test]
fn json_output_at_a_terminal_refuses_without_asking() {
    let home = Home::new();
    let login = login_at_a_terminal(&home, &["--output", "json"], &[], &[(None, b"\x04")]);
    assert_eq!(login.code, Some(2), "{:?}", login.shown);
    assert!(!login.shown.contains(PROMPT), "{:?}", login.shown);
    assert!(
        login
            .shown
            .contains(r#""hint":"pass `--token-file <path>`"#),
        "{:?}",
        login.shown
    );
    assert!(!home.config().exists());
}

/// Ctrl-Z and Ctrl-\ interrupt the prompt as Ctrl-C does, rather than become part of the
/// token, and a line too long to be a token is refused.
#[test]
fn ctrl_z_and_a_line_too_long_store_nothing() {
    for (typed, error) in [
        (&b"tok\x1a"[..], "error: the token prompt was interrupted\n"),
        (&b"tok\x1c"[..], "error: the token prompt was interrupted\n"),
    ] {
        let home = Home::new();
        let login = login_at_a_terminal(&home, TEXT, &[], &[(Some(PROMPT), typed)]);
        assert_eq!(login.code, Some(2), "{typed:?}: {:?}", login.shown);
        assert!(login.shown.contains(error), "{typed:?}: {:?}", login.shown);
        assert!(!home.config().exists());
    }
    // Ctrl-S is read as a byte, not taken by the terminal to stop its output, which putting
    // the mode back would not restart: the token holding it is refused, and the error shows.
    let home = Home::new();
    let typed = format!("{TOKEN}\x13\n");
    let login = login_at_a_terminal(&home, TEXT, &[], &[(Some(PROMPT), typed.as_bytes())]);
    assert_eq!(login.code, Some(2), "{:?}", login.shown);
    assert!(
        login.shown.contains("error: the token holds a space"),
        "{:?}",
        login.shown
    );
    assert!(!home.config().exists());
    let home = Home::new();
    let long = vec![b'a'; 5000];
    let login = login_at_a_terminal(&home, TEXT, &[], &[(Some(PROMPT), &long)]);
    assert_eq!(login.code, Some(2), "{:?}", login.shown);
    assert!(
        login
            .shown
            .contains("error: the line typed at the prompt is too long to be a token\n"),
        "{:?}",
        login.shown
    );
    assert!(!home.config().exists());
}

const ACCESS_KEY_PROMPT: &str =
    "type or paste the 12 words of the access key (they are not shown): ";

/// A mock login for `PHRASES[0]`: the challenge, the verify, and the token check.
fn access_key_server() -> Server {
    let challenge = json!({
        "challenge": "8f2c",
        "message": "Sign this message to log in to Hippius: nonce 8f2c",
    });
    let verified = json!({
        "token": "tok_issued_by_the_login_0123456789abcdef",
        "substrate_address": "5DfhGyQdFobKM8NsWvEeAKk5EQQgYe9AydgJ7rMB6E1EqRzV",
        "is_new": false,
    });
    Server::start(vec![
        route(
            "POST",
            "/api/auth/mnemonic/",
            Reply::json(200, &challenge.to_string()),
        ),
        route(
            "POST",
            "/api/auth/verify/",
            Reply::json(200, &verified.to_string()),
        ),
        route("GET", "/api/accounts/", Reply::json(200, ACCOUNTS)),
    ])
}

/// `--access-key` asks for the 12 words with echo off, logs in with them, and stores only the
/// token: neither the screen nor the config file holds a word of the phrase.
#[test]
fn an_access_key_typed_at_the_prompt_logs_in_and_is_not_shown() {
    let home = Home::new();
    let server = access_key_server();
    let typed = format!("{}\r", PHRASES[0]);
    let login = run_at_a_terminal(
        &home,
        &["--output", "text", "--yes"],
        &["--access-key"],
        &[("HIPPIUS_API_URL", server.url())],
        &[(Some(ACCESS_KEY_PROMPT), typed.as_bytes())],
    );
    assert_eq!(login.code, Some(0), "{:?}", login.shown);
    assert!(
        login.shown.starts_with(ACCESS_KEY_PROMPT),
        "{:?}",
        login.shown
    );
    for word in PHRASES[0].split_whitespace() {
        assert!(!login.shown.contains(word), "{word}: {:?}", login.shown);
    }
    assert!(login.echo_after);
    let config = home.config_text();
    assert!(
        config.contains("token = \"tok_issued_by_the_login_0123456789abcdef\""),
        "{config}"
    );
    assert!(!config.contains("bottom"), "{config}");
    assert_eq!(server.requests().len(), 3);
}

/// Ctrl-C or an empty line at the access key prompt sends and stores nothing, and the
/// terminal echoes again.
#[test]
fn an_interrupted_or_empty_access_key_prompt_sends_nothing() {
    for (typed, error) in [
        (
            &b"bottom dri\x03"[..],
            "error: the access key prompt was interrupted\n",
        ),
        (b"\n", "error: no access key was typed at the prompt\n"),
    ] {
        let home = Home::new();
        let server = access_key_server();
        let login = run_at_a_terminal(
            &home,
            &["--output", "text", "--yes"],
            &["--access-key"],
            &[("HIPPIUS_API_URL", server.url())],
            &[(Some(ACCESS_KEY_PROMPT), typed)],
        );
        assert_eq!(login.code, Some(2), "{:?}", login.shown);
        assert!(login.shown.contains(error), "{:?}", login.shown);
        assert!(
            login.shown.contains(
                "auth login --access-key` and type the 12 words at the prompt, or pass \
                 `--access-key-file <path>`"
            ),
            "{:?}",
            login.shown
        );
        assert!(!login.shown.contains("bottom dri"), "{:?}", login.shown);
        assert!(login.echo_after);
        assert!(server.requests().is_empty());
        assert!(!home.config().exists());
    }
}

/// A word the list does not hold is refused by its position, never quoted, at the prompt as
/// from a file.
#[test]
fn a_mistyped_access_key_is_refused_by_position() {
    let home = Home::new();
    let server = access_key_server();
    let typed = PHRASES[0].replacen("drive", "drivx", 1) + "\n";
    let login = run_at_a_terminal(
        &home,
        &["--output", "text", "--yes"],
        &["--access-key"],
        &[("HIPPIUS_API_URL", server.url())],
        &[(Some(ACCESS_KEY_PROMPT), typed.as_bytes())],
    );
    assert_eq!(login.code, Some(2), "{:?}", login.shown);
    assert!(
        login.shown.contains(
            "error: word 2 of the access key phrase is not on the English BIP-39 word list\n\
             hint: type only the access key's words at the prompt\n"
        ),
        "{:?}",
        login.shown
    );
    assert!(!login.shown.contains("drivx"), "{:?}", login.shown);
    assert!(server.requests().is_empty());
}

/// Under `CI` nobody is asked for the access key, even at a terminal, and the hint names
/// `--access-key-file`.
#[test]
fn ci_refuses_the_access_key_prompt_without_asking() {
    let home = Home::new();
    let server = access_key_server();
    let login = run_at_a_terminal(
        &home,
        &["--output", "text", "--yes"],
        &["--access-key"],
        &[("HIPPIUS_API_URL", server.url()), ("CI", "1")],
        &[],
    );
    assert_eq!(login.code, Some(2), "{:?}", login.shown);
    assert!(
        !login.shown.contains(ACCESS_KEY_PROMPT),
        "{:?}",
        login.shown
    );
    assert!(
        login.shown.contains(
            "hint: pass `--access-key-file <path>`, or `--access-key-file -` to read the 12 \
             words from stdin\n"
        ),
        "{:?}",
        login.shown
    );
    assert!(server.requests().is_empty());
}

/// A stored token is replaced only after its own question: a person who confirms the origin
/// and declines the replacement keeps the stored token, nothing is sent, and the 12 words were
/// never asked for: both questions come before the prompt for them.
#[test]
fn declining_the_replacement_keeps_the_stored_token_and_sends_nothing() {
    let home = Home::new();
    let server = access_key_server();
    home.write_config(&format!(
        "[profiles.default]\ntoken = \"{TOKEN}\"\ntoken_origin = \"{}\"\n",
        server.url()
    ));
    let login = run_at_a_terminal(
        &home,
        &["--output", "text"],
        &["--access-key"],
        &[("HIPPIUS_API_URL", server.url())],
        &[
            (Some("continue? [y/N] "), b"y\r"),
            (
                Some(
                    "replace the token stored in the profile `default` with the token the \
                     access key's login answers; every later command with this profile acts as \
                     the new token's owner; continue? [y/N] ",
                ),
                b"n\r",
            ),
        ],
    );
    assert_ne!(login.code, Some(0), "{:?}", login.shown);
    assert!(login.shown.contains("error:"), "{:?}", login.shown);
    assert!(
        !login.shown.contains(ACCESS_KEY_PROMPT),
        "{:?}",
        login.shown
    );
    assert!(server.requests().is_empty());
    assert!(home.config_text().contains(TOKEN));
}
