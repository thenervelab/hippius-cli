//! Fixtures and small helpers that many suites share: one spelling of each id, one way to
//! list what a server received, and one guard for the tests that rely on permission bits.

use serde_json::Value;

use super::{Run, Server};

/// The project the tests select or name.
pub(crate) const PROJECT: &str = "proj_0a1b2c3d4e5f60718293";

/// The billing account the tests select or name.
pub(crate) const BILLING: &str = "bill_9f8e7d6c5b4a39281706";

/// The VM the compute tests act on.
pub(crate) const VM: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

/// The token's own account, first in [`super::ACCOUNTS`].
pub(crate) const OWN_ACCOUNT: &str = "acct_ffffffffffffffffffff";

/// A JSON fixture, parsed.
pub(crate) fn parsed(text: &str) -> Value {
    serde_json::from_str(text).expect("the fixture is JSON")
}

impl Server {
    /// Each request received, as `METHOD path?query`, in order.
    pub(crate) fn targets(&self) -> Vec<String> {
        self.requests()
            .iter()
            .map(|r| format!("{} {}", r.method, r.target))
            .collect()
    }

    /// Each request received, as `METHOD path` without the query, in order.
    pub(crate) fn calls(&self) -> Vec<String> {
        self.requests()
            .iter()
            .map(|r| format!("{} {}", r.method, r.path()))
            .collect()
    }
}

impl Run {
    /// The JSON error object on stderr, its `error` field: one object on one line.
    pub(crate) fn error(&self) -> Value {
        let stderr = self.stderr();
        assert_eq!(stderr.lines().count(), 1, "{stderr}");
        let error: Value = serde_json::from_str(stderr).expect("stderr is one JSON object");
        error["error"].clone()
    }

    /// The tenet 4 failure with its one exception: a failure after a write was done keeps
    /// the document of what was done on stdout. This exit code, one JSON object on stderr
    /// with this `code`; stdout is the caller's to check. Returns the object's `error`.
    pub(crate) fn fails_keeping_stdout(&self, exit: i32, code: &str) -> Value {
        assert_eq!(self.code(), Some(exit), "stderr: {}", self.stderr());
        let error = self.error();
        assert_eq!(error["code"], code, "{}", self.stderr());
        error
    }
}

/// Whether this test can rely on permission bits: false when the suite runs as root, whom
/// a mode such as 0500 does not stop. A test that makes a file or directory unreadable or
/// unwritable on purpose starts with `if !permission_bits_apply() { return; }`, and so is
/// skipped, with a note, in a root container instead of failing there.
#[expect(
    clippy::print_stderr,
    reason = "a skipped test says so where the test runner shows a test's output"
)]
pub(crate) fn permission_bits_apply() -> bool {
    let normal_user = !rustix::process::geteuid().is_root();
    if !normal_user {
        eprintln!(
            "note: {} skipped: root is not stopped by permission bits",
            std::thread::current().name().unwrap_or("a test")
        );
    }
    normal_user
}
