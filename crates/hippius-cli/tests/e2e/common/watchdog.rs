//! A deadline on every `hipc` process a test starts: a run that waits for input nobody types,
//! or for a server that never answers, is killed and fails its test by name, instead of
//! holding the whole test binary until the CI job's time limit.

use std::io::{ErrorKind, Read, Write as _};
use std::process::{Child, Command, Output};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long one `hipc` process may run, in seconds. Arbitrary, chosen far beyond the slowest
/// run a test makes: 30 s, the default `--request-timeout` a [`super::Reply::never`] waits
/// out, plus a VM wait's 2 s polls, on a loaded host running a debug build.
pub(crate) const RUN_DEADLINE: Duration = Duration::from_secs(120);

/// Waits for `child`, which runs `hipc <args>`, writing `stdin` to its piped stdin and
/// reading its piped stdout and stderr meanwhile, so no pipe fills while another is
/// waited on. A pipe the child does not have reads as empty.
///
/// Past [`RUN_DEADLINE`] the child is killed and the test fails, naming itself, the
/// arguments, and the stderr read so far.
pub(crate) fn finish(child: Child, stdin: Option<Vec<u8>>, args: &[&str]) -> Output {
    finish_within(RUN_DEADLINE, child, stdin, args)
}

/// [`finish`] with the deadline `limit`.
fn finish_within(
    limit: Duration,
    mut child: Child,
    stdin: Option<Vec<u8>>,
    args: &[&str],
) -> Output {
    let deadline = Instant::now() + limit;
    let writer = stdin.map(|bytes| {
        let mut pipe = child.stdin.take().expect("a piped stdin");
        std::thread::spawn(move || {
            // `hipc` may refuse, and exit, before it reads its stdin, closing the pipe while
            // this write is under way; what it did is in its output, which is what a test
            // checks.
            if let Err(error) = pipe.write_all(&bytes) {
                assert_eq!(
                    error.kind(),
                    ErrorKind::BrokenPipe,
                    "write hipc's stdin: {error}"
                );
            }
        })
    });
    let (closed, eof) = mpsc::channel();
    let stdout = child.stdout.take().map(|pipe| drain(pipe, closed.clone()));
    let stderr = child.stderr.take().map(|pipe| drain(pipe, closed.clone()));
    drop(closed);
    // The pipes close when `hipc` exits, so this waits without polling, and the exit
    // itself follows at once.
    let pipes = usize::from(stdout.is_some()) + usize::from(stderr.is_some());
    let closed_in_time = (0..pipes).all(|_| eof.recv_timeout(left(deadline)).is_ok());
    let exited = loop {
        if let Some(status) = child.try_wait().expect("poll hipc") {
            break Some(status);
        }
        if !closed_in_time || Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let status = exited.unwrap_or_else(|| {
        child.kill().expect("kill a hipc past its deadline");
        child.wait().expect("reap the killed hipc")
    });
    let joined = |reader: Option<JoinHandle<Vec<u8>>>| {
        reader.map_or_else(Vec::new, |r| {
            r.join().expect("a pipe reader does not panic")
        })
    };
    let output = Output {
        status,
        stdout: joined(stdout),
        stderr: joined(stderr),
    };
    if let Some(Err(panic)) = writer.map(JoinHandle::join) {
        std::panic::resume_unwind(panic);
    }
    assert!(
        exited.is_some(),
        "{}: `hipc {}` still ran after {limit:?} and was killed; its stderr: {}",
        std::thread::current().name().unwrap_or("a test"),
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// Runs `command`, a `hipc` run built some other way than [`super::Hipc`], to its end under
/// the deadline of every run, and checks its output as [`super::Hipc::run`] checks every
/// run's. The caller pipes stdout and stderr, or sends them elsewhere.
pub(crate) fn finished(command: &mut Command) -> Output {
    let args: Vec<String> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let child = command
        .spawn()
        .expect("the hipc binary built by cargo runs");
    super::check_output(finish(child, None, &args))
}

/// The time left until `deadline`, zero once it has passed.
fn left(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// Reads `pipe` to its end on a thread of its own, then says so on `closed`.
fn drain(mut pipe: impl Read + Send + 'static, closed: mpsc::Sender<()>) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        drop(pipe.read_to_end(&mut bytes));
        closed.send(()).ok();
        bytes
    })
}

/// A run past its deadline is killed, and the test fails naming the run; a run that ends
/// in time keeps its output.
#[test]
fn a_run_past_its_deadline_is_killed_and_named() {
    use std::process::{Command, Stdio};

    // `sh` stands in for a `hipc` that waits for ever, and a short limit for the real one.
    let spawn = |script: &str| {
        Command::new("sh")
            .args(["-c", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn sh")
    };
    let limit = Duration::from_millis(300);
    let waiting = spawn("echo started >&2; exec sleep 600");
    let failed = std::panic::catch_unwind(|| finish_within(limit, waiting, None, &["wait"]))
        .expect_err("a run past its deadline fails");
    let message = failed
        .downcast_ref::<String>()
        .expect("a formatted message");
    assert!(
        message.contains("`hipc wait` still ran after 300ms and was killed; its stderr: started"),
        "{message}"
    );
    let echo = spawn("cat; echo done >&2");
    let output = finish_within(RUN_DEADLINE, echo, Some(b"typed".to_vec()), &["echo"]);
    assert_eq!(output.stdout, b"typed");
    assert_eq!(output.stderr, b"done\n");
    assert!(output.status.success());
}
