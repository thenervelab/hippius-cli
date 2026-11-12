//! The one wait loop of the commands that wait for what the server does after a write: the
//! VM launch wait, the App Engine launch wait, and the hAlpha purchase follow. Each reads one
//! route again and again until what it reads says the wait is over, its deadline passes, or
//! a read fails for good.
//!
//! Every pause of the loop's own goes through [`paced`], so a debug build run by the test
//! suite can shorten them all with [`TEST_POLL_INTERVAL_MS`]. A release build has no such
//! variable: the code that reads it is not compiled in.

#[cfg(debug_assertions)]
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use hippius::{Client, Error, Request, Result};
use serde_json::Value;

use crate::context::Context;

/// How many reads in a row may fail with a retryable error (a 502, 503, 504, 425 or 429, a
/// timeout, or a failed connection; a 500 ends the wait at once) before the wait gives up: a
/// read cannot launch or buy twice, so a passing outage, such as a restart behind the API, is
/// waited out. The wait's own deadline bounds reads that time out.
pub(crate) const MAX_POLL_FAILURES: u32 = 3;

/// What a wait does next, from what it holds.
#[derive(Debug)]
pub(crate) enum Turn<T> {
    /// The wait is over, with this result.
    Stop(Result<T>),
    /// The wait goes on: read again.
    Poll,
}

/// A wait: what it holds, what one answer changes, and how it ends.
pub(crate) trait Wait {
    /// What the wait gives once it is over.
    type Output;

    /// Whether the wait is over, from what it holds: first what it starts from, then after
    /// each read.
    fn next(&mut self) -> Turn<Self::Output>;

    /// Takes the document one read answered. An error ends the wait with it.
    ///
    /// # Errors
    ///
    /// What the wait refuses in the answer, such as the document of another VM.
    fn take(&mut self, answer: Value) -> Result<()>;

    /// The result once the deadline has passed and the wait is not over.
    ///
    /// # Errors
    ///
    /// The wait's timeout, when running out of time is a failure.
    fn timed_out(self) -> Result<Self::Output>;

    /// The error once a read fails for good: a failure that is not retryable, or the one past
    /// [`MAX_POLL_FAILURES`] in a row.
    fn failed(self, error: Error) -> Error;
}

/// When a wait reads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Schedule {
    /// The pause before the first read. A read with no pause before it is sent whatever the
    /// deadline: the hAlpha follow reads the purchase once even with no time left.
    pub(crate) first: Duration,
    /// The pause between two reads.
    pub(crate) interval: Duration,
    /// When the wait gives up.
    pub(crate) deadline: Instant,
}

/// Reads `request` until `wait` says it is over ([`Wait::next`]), the deadline passes
/// ([`Wait::timed_out`]), or a read fails for good ([`Wait::failed`]).
///
/// A pause never runs past the deadline. A read that fails with a retryable error is tried
/// again, up to [`MAX_POLL_FAILURES`] in a row, after the interval, or after the server's
/// `Retry-After` when it asks for longer; when what it asks for runs past the deadline, the
/// wait sleeps until the deadline and ends there, without the read the server asked it not to
/// send. A read can end up to `--request-timeout` past the deadline, because a request is not
/// cut short.
pub(crate) fn poll_until<W: Wait>(
    ctx: &Context<'_>,
    client: &Client,
    request: &Request,
    schedule: Schedule,
    mut wait: W,
) -> Result<W::Output> {
    let interval = paced(schedule.interval);
    let mut failures = 0;
    let mut pause = paced(schedule.first);
    let mut pause_from_retry_after = false;
    loop {
        if let Turn::Stop(done) = wait.next() {
            return done;
        }
        if !pause.is_zero() {
            let left = schedule.deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return wait.timed_out();
            }
            std::thread::sleep(pause.min(left));
            if pause_from_retry_after && pause > left {
                return wait.timed_out();
            }
        }
        match ctx
            .send(client, request)
            .and_then(|response| response.document())
        {
            Ok(answer) => {
                failures = 0;
                pause = interval;
                pause_from_retry_after = false;
                wait.take(answer)?;
            }
            Err(error) if error.is_retryable() && failures < MAX_POLL_FAILURES => {
                failures += 1;
                (pause, pause_from_retry_after) = after(error.retry_after_ms(), interval);
            }
            Err(error) => return Err(wait.failed(error)),
        }
    }
}

/// The pause `interval` stands for in this build: `interval` itself, except that a debug
/// build run with [`TEST_POLL_INTERVAL_MS`] set waits that long instead. A zero pause stays
/// zero, so a read sent at once still is.
fn paced(interval: Duration) -> Duration {
    if interval.is_zero() {
        return interval;
    }
    test_poll_interval().unwrap_or(interval)
}

/// The variable a debug build reads for the pause of every wait, in milliseconds, so the test
/// suite does not sleep through the real 2 or 3 seconds between two reads. Only debug builds
/// have it: the release build (`.github/actions/release-build`) fails when its binary holds
/// the name.
#[cfg(debug_assertions)]
const TEST_POLL_INTERVAL_MS: &str = "HIPC_TEST_POLL_INTERVAL_MS";

/// The pause [`TEST_POLL_INTERVAL_MS`] asks for: none when it is unset or not a whole number
/// of milliseconds above zero. Zero is refused because a zero pause skips the deadline check
/// in [`poll_until`], so a wait that never ends would read for ever.
#[cfg(debug_assertions)]
fn test_poll_interval() -> Option<Duration> {
    let ms = crate::context::env_os(TEST_POLL_INTERVAL_MS)?;
    let ms: NonZeroU64 = ms.to_str()?.parse().ok()?;
    Some(Duration::from_millis(ms.get()))
}

/// A release build has no test override: every wait keeps its real interval.
#[cfg(not(debug_assertions))]
const fn test_poll_interval() -> Option<Duration> {
    None
}

/// The pause before the read after a retryable failure whose `Retry-After` was
/// `retry_after_ms`, and whether the server asked for it: its `Retry-After` when that is at
/// least `interval`, else `interval`, the loop's own pause, which a deadline cuts short
/// without skipping the last read (tenet 4).
fn after(retry_after_ms: Option<u64>, interval: Duration) -> (Duration, bool) {
    match retry_after_ms.map(Duration::from_millis) {
        Some(asked) if asked >= interval => (asked, true),
        _ => (interval, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only a `Retry-After` at least the interval is the server's pause; a shorter one leaves
    /// the loop's own, so a deadline inside it still gets the last read.
    #[test]
    fn a_short_retry_after_is_the_loops_own_pause() {
        let interval = Duration::from_secs(5);
        assert_eq!(after(None, interval), (interval, false));
        assert_eq!(after(Some(1_000), interval), (interval, false));
        assert_eq!(after(Some(5_000), interval), (interval, true));
        assert_eq!(after(Some(9_000), interval), (Duration::from_secs(9), true));
    }
}
