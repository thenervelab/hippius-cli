//! A pseudo-terminal for the tests that run `hipc` where a person sits: the side `hipc` sees
//! as its terminal, and a screen the test reads while it types.

use std::fs::{File, OpenOptions};
use std::io::Read as _;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};

/// How long a test waits for text to show: arbitrary, far beyond a debug build's start on a
/// loaded host.
const SHOW_DEADLINE: Duration = Duration::from_secs(30);

/// A pseudo-terminal: the side the test types into and reads from, and the side `hipc` sees
/// as its terminal.
pub(crate) fn pty() -> (OwnedFd, File) {
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

/// What the terminal shows, read on a thread of its own.
///
/// The test keeps its own terminal-side handle open until the screen is read: macOS drops
/// what is buffered on the terminal once the last terminal-side handle closes, so a command
/// that exits at once (`version`) could leave an empty screen. With the handle open, a read
/// never ends on its own, so the reader polls and stops once [`Screen::close`] says `hipc`
/// has exited and nothing more arrives.
pub(crate) struct Screen {
    shown: Arc<Mutex<Vec<u8>>>,
    exited: Arc<AtomicBool>,
    reader: JoinHandle<()>,
}

impl Screen {
    /// Starts reading what `keyboard`, the test's side of the pty, receives.
    pub(crate) fn read(keyboard: &File) -> Self {
        let mut screen = keyboard.try_clone().expect("clone the keyboard side");
        let shown = Arc::new(Mutex::new(Vec::new()));
        let exited = Arc::new(AtomicBool::new(false));
        let reader = {
            let (shown, exited) = (Arc::clone(&shown), Arc::clone(&exited));
            std::thread::spawn(move || {
                let mut buffer = [0_u8; 4096];
                // 50 ms: arbitrary. It only adds latency after the exit; correctness does
                // not depend on it.
                let tick = Timespec {
                    tv_sec: 0,
                    tv_nsec: 50_000_000,
                };
                loop {
                    // Load the flag before polling: if it was already set, all of `hipc`'s
                    // output is buffered, so a poll that times out means the screen is
                    // drained.
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
                        Err(error) => {
                            panic!("read the terminal while its terminal side is open: {error}")
                        }
                        Ok(n) => shown
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .extend_from_slice(&buffer[..n]),
                    }
                }
            })
        };
        Self {
            shown,
            exited,
            reader,
        }
    }

    /// What the screen shows so far.
    pub(crate) fn text(&self) -> String {
        let shown = self.shown.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&shown).into_owned()
    }

    /// Waits until `shows` holds for the screen, failing the test after a deadline, or
    /// returns false at once when `exited` says `hipc` ended first.
    pub(crate) fn wait_until(
        &self,
        what: &str,
        shows: impl Fn(&str) -> bool,
        mut exited: impl FnMut() -> bool,
    ) -> bool {
        let deadline = Instant::now() + SHOW_DEADLINE;
        while !shows(&self.text()) {
            if exited() {
                return false;
            }
            assert!(
                Instant::now() < deadline,
                "{what} never appeared: {:?}",
                self.text()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }

    /// Everything the screen showed, line breaks as `\n`, once `hipc` has exited. It is
    /// the run's output, so it gets the secret check every run's output gets.
    pub(crate) fn close(self) -> String {
        // Release pairs with the reader's Acquire, so a reader that sees `true` polls after
        // `hipc`'s exit was reaped.
        self.exited.store(true, Ordering::Release);
        let Self { shown, reader, .. } = self;
        reader.join().expect("the screen reader does not panic");
        let shown = shown.lock().unwrap_or_else(PoisonError::into_inner);
        let shown = String::from_utf8_lossy(&shown).replace('\r', "");
        super::assert_no_secret_in(&shown);
        shown
    }
}
