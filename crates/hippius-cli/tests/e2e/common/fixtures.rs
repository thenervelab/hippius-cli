//! Fixtures and small helpers that many suites share: one spelling of each id, one way to
//! list what a server received, and one guard for the tests that rely on permission bits.

use serde_json::{Value, json};

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
///
/// # Panics
///
/// As root under `CI`: a CI job must run these tests, and a skip would pass green unseen.
#[expect(
    clippy::print_stderr,
    reason = "a skipped test says so where the test runner shows a test's output"
)]
pub(crate) fn permission_bits_apply() -> bool {
    let normal_user = !rustix::process::geteuid().is_root();
    assert!(
        normal_user || std::env::var_os("CI").is_none(),
        "CI runs the suite as root, where the permission tests would be skipped; run it as a \
         normal user"
    );
    if !normal_user {
        eprintln!(
            "note: {} skipped: root is not stopped by permission bits",
            std::thread::current().name().unwrap_or("a test")
        );
    }
    normal_user
}

/// The `docker login` commands a Hub answer may hold outside a command key, each passing
/// `secret` or able to, as a value to put under any key of the answer: one string, an argv
/// array, a nested list, invisible joiners, letter case, and the ways a shell reads one
/// word (quotes, backslashes, a continued line, a flag between the words, a substitution).
/// Every Hub read that prints a robot or key answer refuses each one whole, so the suites
/// of `hub keys list`, `hub namespace robot get`, and `robot rotate` share this list.
pub(crate) fn docker_logins(secret: &str) -> Vec<Value> {
    vec![
        json!(["docker", "login", "-u", "r", "-p", secret]),
        json!(["docker", "login", "-u", "r", format!("-p{secret}")]),
        json!(["docker", "login", "-u", "r", "--password", secret]),
        json!(["docker", "login", "-u", "r", format!("--password={secret}")]),
        json!([
            "echo",
            secret,
            "|",
            "docker",
            "login",
            "-u",
            "r",
            "--password-stdin"
        ]),
        json!([
            format!("DOCKER_PASSWORD={secret}"),
            "docker",
            "login",
            "-u",
            "r"
        ]),
        json!(["kept", format!("docker\u{200B}login -p {secret}")]),
        json!({"x": ["docker\u{FEFF}", "login", format!("-p{secret}")]}),
        json!(format!("docker\u{200D}login --password={secret}")),
        json!(format!("echo {secret} | docker login")),
        json!(format!(
            "run: echo {secret} | docker  LOGIN -u r --password-stdin"
        )),
        json!(format!("\"docker\" \"login\" -u u -p {secret}")),
        json!(format!("docker \\\nlogin -p {secret}")),
        json!(format!("docke\\r log\\in -p {secret}")),
        json!(format!("docker $'login' -p {secret}")),
        json!(format!("/usr/bin/docker -D login -p {secret}")),
        json!(format!("$(printf docker) login -p {secret}")),
        // A command that passes nothing yet, which a later field could complete.
        json!(["docker", "login", "-u", "r"]),
        json!("DOCKER LOGIN"),
    ]
}

/// The local clock, in seconds since 1970.
pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_secs()
}

/// `seconds` since the epoch as an IMF-fixdate, by Howard Hinnant's `civil_from_days`;
/// 1970-01-01 was a Thursday. The scripted server sends no `Date` of its own, and a fixed
/// one would be far from the clock of the machine that runs the test.
pub(crate) fn imf_fixdate(seconds: u64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = seconds / 86_400;
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    let clock = seconds % 86_400;
    let index = |n: u64| usize::try_from(n).expect("a weekday or a month fits");
    format!(
        "{}, {day:02} {} {year} {:02}:{:02}:{:02} GMT",
        DAYS[index(days % 7)],
        MONTHS[index(month - 1)],
        clock / 3_600,
        clock / 60 % 60,
        clock % 60
    )
}
