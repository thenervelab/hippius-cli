//! The launch of a VM or an App Engine app: the clauses its question shares, whether it
//! waits, and the wait for `running`.

use std::time::{Duration, Instant};

use hippius::routes::Route;
use hippius::routes::compute::{self, BootPhase, Vm, VmStatus};
use hippius::{Client, Code, Error, Failure, Request, Result, phrase, view_of};
use serde_json::Value;

use crate::cli::{DiskTier, FailoverMode, LaunchFlags};
use crate::commands::Printed;
use crate::commands::compute::VMS;
use crate::commands::compute::app_engine::APPS;
use crate::commands::compute::app_engine::create::{printable, sanitized};
use crate::commands::poll::{Next, Schedule, Wait, poll_until};
use crate::context::Context;

/// How often a launch is polled, 2 seconds: a launch takes on the order of a
/// minute, so a shorter interval only adds requests.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// The launch clause of `--on-guest-poweroff stop`, for the VM and the app questions: the VM
/// is then billed while it sits stopped.
pub(crate) const STAYS_STOPPED: &str = "a power-off from inside the VM leaving it stopped, and \
                                        billed as a stopped VM until it is started or deleted";

/// The launch clause of `--os-disk-tier`, for the VM and the app questions: the tier is a
/// minimum, fixed for the VM's life, and where its price is shown.
pub(super) fn os_disk_clause(tier: DiskTier) -> String {
    let tier = tier.code();
    format!(
        "an OS disk on tier `{tier}` or faster, fixed for the VM's life and billed at the \
         `{tier}` price that `hipc compute prices` shows (`vm_by_os_disk_tier`)"
    )
}

/// The launch clause of `--backup-interval`, for the VM and the app questions: how often, the
/// failover, and that stored backups are billed, so neither question can leave the bill out.
pub(crate) fn backups_clause(seconds: u32, failover: Option<FailoverMode>) -> String {
    let failover = failover.map_or(String::new(), |mode| {
        format!(" and {}", failover_clause(mode))
    });
    format!(
        "backups every {}{failover}, stored backups billed per GB-hour at the price that `hipc \
         compute backups options` shows",
        phrase::count(u64::from(seconds), "second")
    )
}

/// The words that name a backup failover, for the launch questions and the backup policy
/// update: `auto` says what it loses.
pub(super) const fn failover_clause(mode: FailoverMode) -> &'static str {
    match mode {
        FailoverMode::Auto => {
            "failover `auto`, which restarts it on another server from its latest backup if its \
             server dies, losing what was written after that backup"
        }
        FailoverMode::Manual => "failover `manual`",
    }
}

/// The hint of a launch the API refused with 402, which it answers when the balance does not
/// cover the runway `hipc compute quota` shows: the two reads that tell how much is needed,
/// the hourly burn of what runs and the hours it must cover. Any other error is left as it is.
pub(crate) fn balance_hint(error: Error) -> Error {
    if error.code() == Code::PaymentRequired {
        error.with_hint(LAUNCH_BALANCE_HINT)
    } else {
        error
    }
}

/// [`balance_hint`]'s text.
const LAUNCH_BALANCE_HINT: &str = "run `hipc billing summary` and `hipc compute quota`, then \
     top up until the balance covers the runway hours at the hourly burn, this launch included";

/// How long the launch waits for `running` when `--wait-timeout` is not given: 300
/// seconds. Arbitrary. The `--wait-timeout` help in
/// `cli/compute.rs` and `cli/app_engine.rs` repeats this value.
const WAIT_TIMEOUT: Duration = Duration::from_secs(300);

/// How long the launch waits for `running`, or `None` when it does not wait. `--wait`,
/// `--wait-timeout`, and `--no-wait` decide; without any of them, a person at a terminal
/// waits and a program does not, by the rule that picks JSON for `--output auto`. A
/// program that waits and is killed first loses the launch answer, and an
/// agent's tool timeout is often shorter than the wait.
///
/// `--wait-timeout` asks for a wait, so it waits wherever it runs: a flag wins over
/// detection (tenet 15), and the same command line behaves the same at a terminal and on a
/// pipe. Beside `--no-wait` it means nothing, and clap refuses that pair.
///
/// `hipc compute app-engine apps create` takes the same three flags and waits by this rule.
pub(crate) fn launch_wait(ctx: &Context<'_>, launch: &LaunchFlags) -> Option<Duration> {
    let waits = launch.wait
        || launch.wait_timeout.is_some()
        || !(launch.no_wait || ctx.reader_is_program());
    waits.then(|| {
        launch
            .wait_timeout
            .map_or(WAIT_TIMEOUT, Duration::from_secs)
    })
}

/// What ends a VM launch wait.
#[derive(Debug, Clone, Copy)]
pub(super) enum Goal {
    /// `status` is `running`.
    Running,
    /// `status` is `running` and `network_ready` is true: a launch with `--public-ip`.
    /// Swagger: `network_ready` says the internet path to the address is open; it can lag the
    /// opened path by up to about 30 seconds, never lead it. A `public_ip_error` on the VM also
    /// ends it, since the attach failed and no address will turn ready.
    Reachable,
}

/// What a launch launched, which decides when its wait ends, what it prints, and how its
/// messages name it.
#[derive(Debug, Clone, Copy)]
pub(super) enum Kind<'a> {
    /// A VM, waited on until the [`Goal`].
    Vm(Goal),
    /// An App Engine app, waited on until its status and its boot phase are both `running`.
    /// Every decision reads the documents as they came; every document printed, and every
    /// error of a poll, is cleared of `redactions`, the spellings of the secret values the
    /// launch sent ([`printable`], [`sanitized`]).
    App { redactions: &'a [String] },
}

impl Kind<'_> {
    /// What it is, as a message names it.
    fn noun(self) -> &'static str {
        match self {
            Self::Vm(_) => "VM",
            Self::App { .. } => "app",
        }
    }

    /// The command that lists them, for a hint that cannot name the one launched.
    fn list(self) -> &'static str {
        match self {
            Self::Vm(_) => VMS,
            Self::App { .. } => APPS,
        }
    }

    /// The command that reads one, without its id.
    fn get(self) -> &'static str {
        match self {
            Self::Vm(_) => "hipc compute vms get",
            Self::App { .. } => "hipc compute app-engine apps get",
        }
    }

    /// The read the wait polls.
    fn poll(self) -> &'static Route<1> {
        match self {
            Self::Vm(_) => &compute::VM,
            Self::App { .. } => &compute::APP_ENGINE_APP,
        }
    }

    /// What the wait waits for, as the note names it.
    fn until(self) -> &'static str {
        match self {
            Self::Vm(Goal::Reachable) => "it is running and its public IPv4 is reachable",
            Self::Vm(Goal::Running) | Self::App { .. } => "it is running",
        }
    }

    /// Whether a message may quote `id`, the id an answer gives: a VM's always, through
    /// `phrase::quote`; an app's only when it can be polled, since the printed copy of an
    /// app redacts any id that may be a value sent, and a message must not print it raw.
    fn may_quote(self, id: &str) -> bool {
        match self {
            Self::Vm(_) => true,
            Self::App { .. } => self.pollable(id).is_ok(),
        }
    }

    /// Whether `id` can go into the poll URL, or why not: it must pass the rule of the `vm_id`
    /// argument, so `a?b=1` or `a/b` cannot poll another route, and an app's must hold no
    /// value sent.
    fn pollable(self, id: &str) -> std::result::Result<(), String> {
        match self {
            Self::App { redactions } if hippius::holds_secret(id, redactions) => {
                Err("it may be a secret value sent".to_owned())
            }
            Self::Vm(_) | Self::App { .. } => crate::cli::VmId::parse(id).map(drop),
        }
    }

    /// Where the launch stands by `doc`, the last document seen.
    fn state(self, doc: &Value) -> Launch {
        match self {
            Self::Vm(goal) => vm_state(doc, goal),
            Self::App { .. } => app_state(doc),
        }
    }

    /// `doc` as it is printed.
    fn printed(self, doc: Value) -> Value {
        match self {
            Self::Vm(_) => doc,
            Self::App { redactions } => printable(&doc, redactions),
        }
    }

    /// The error of a poll, `action`, that failed.
    fn failed(self, error: Error, action: &str) -> Error {
        match self {
            Self::Vm(_) => error,
            Self::App { redactions } => sanitized(error, action, redactions),
        }
    }
}

/// Where a launch stands.
#[derive(Debug, PartialEq, Eq)]
enum Launch {
    /// The wait's end is reached.
    Done,
    /// A status it does not come back from, as the message quotes it.
    Ended(&'static str),
    /// Not there yet, so the wait goes on.
    Waiting(Pending),
}

/// Why a launch wait goes on, which a timeout names.
#[derive(Debug, PartialEq, Eq)]
enum Pending {
    /// `pending`, `provisioning`, a missing status, or a status this version does not
    /// know, which may be new and transient.
    Booting,
    /// `failed` with `boot_stalled` true. Swagger: such a VM "returns to `running` by itself
    /// once the guest signals", so it is not terminal; an app is a VM, so it is waited on too.
    Stalled,
    /// `running`, but `network_ready` is not true yet.
    Unreachable,
}

/// Where a VM launch stands, from its `status`, `boot_stalled`, and `network_ready`: it ends
/// at `failed` without `boot_stalled`, `decommissioning`, or `decommissioned`.
fn vm_state(vm: &Value, goal: Goal) -> Launch {
    let vm: Vm = view_of(vm);
    match vm.status {
        Some(VmStatus::Running) => match goal {
            Goal::Running => Launch::Done,
            Goal::Reachable if vm.network_ready == Some(true) || vm.public_ip_error.is_some() => {
                Launch::Done
            }
            Goal::Reachable => Launch::Waiting(Pending::Unreachable),
        },
        Some(VmStatus::Failed) if vm.boot_stalled == Some(true) => {
            Launch::Waiting(Pending::Stalled)
        }
        Some(VmStatus::Failed) => Launch::Ended("failed"),
        Some(VmStatus::Decommissioning) => Launch::Ended("decommissioning"),
        Some(VmStatus::Decommissioned) => Launch::Ended("decommissioned"),
        Some(VmStatus::Other(_)) | None => Launch::Waiting(Pending::Booting),
    }
}

/// Where an App Engine launch stands, from the app's `status`, `boot_phase`, and
/// `boot_stalled`. Swagger documents neither field's values on an app, so these are the
/// VM's, unverified on an app: it ends at a terminal VM status, or a `boot_phase` of `failed`.
fn app_state(app: &Value) -> Launch {
    let app: Vm = view_of(app);
    let stalled = app.boot_stalled == Some(true);
    match (app.status, app.boot_phase) {
        (Some(VmStatus::Running), Some(BootPhase::Running)) => Launch::Done,
        (Some(VmStatus::Failed), _) if stalled => Launch::Waiting(Pending::Stalled),
        (Some(VmStatus::Decommissioning), _) => Launch::Ended("decommissioning"),
        (Some(VmStatus::Decommissioned), _) => Launch::Ended("decommissioned"),
        (Some(VmStatus::Failed), _) | (_, Some(BootPhase::Failed)) => Launch::Ended("failed"),
        _ => Launch::Waiting(Pending::Booting),
    }
}

/// Polls what a launch launched until its wait ends ([`Kind::state`]), it reaches a status it
/// does not come back from, or `timeout` has passed. Every failure prints the last document
/// seen, so `vm_id` is not lost, and is never retryable: a retry of the launch would launch,
/// and bill, a second VM (`TENETS.md` tenet 4).
///
/// A poll can end up to `--request-timeout` past the deadline, because the last request
/// is not cut short. A run killed during the wait prints nothing on stdout; text mode
/// names the VM on stderr before the wait, unless `--quiet`, and JSON mode stays silent
/// (tenet 3).
pub(super) fn wait(
    ctx: &Context<'_>,
    client: &Client,
    launched: Value,
    kind: Kind<'_>,
    timeout: Duration,
) -> Result<Printed> {
    let noun = kind.noun();
    let Some(id) = view_of::<Vm>(&launched).vm_id else {
        return Err(
            Error::internal("the launch answer has no `vm_id`, so it cannot be waited on")
                .with_hint(format!("run `{}` to find the new {noun}", kind.list()))
                .after_write(kind.printed(launched)),
        );
    };
    let quoted = kind.may_quote(&id);
    let named = if quoted {
        format!("the {noun} {}", phrase::quote(&id))
    } else {
        format!("the {noun}")
    };
    let argument = quoted
        .then(|| crate::cli::as_argument(&id, crate::cli::VmId::parse))
        .flatten();
    let hint = match argument {
        Some(id) => format!(
            "run {} to see where the launch stands",
            phrase::quote(format!("{} {id}", kind.get()))
        ),
        None => format!("run `{}` to find the new {noun}", kind.list()),
    };
    // A launch answer that already ends the wait needs no poll, so its id is not refused.
    if let Launch::Waiting(_) = kind.state(&launched) {
        if let Err(rule) = kind.pollable(&id) {
            let message = if quoted {
                format!("the launch answer names {named}, which `hipc` cannot poll: {rule}")
            } else {
                format!("the launch answer names the {noun} by an id `hipc` cannot poll: {rule}")
            };
            return Err(Error::internal(message)
                .with_hint(hint)
                .after_write(kind.printed(launched)));
        }
        ctx.note(&format!("launched {named}; waiting until {}", kind.until()));
    }
    let request = Request::to(kind.poll(), [&id]).about(format!("poll {named}"));
    let schedule = Schedule {
        first: POLL_INTERVAL,
        interval: POLL_INTERVAL,
        deadline: Instant::now() + timeout,
    };
    let launch = LaunchWait {
        kind,
        doc: launched,
        pending: Pending::Booting,
        id: &id,
        named: &named,
        hint: &hint,
        action: request.action(),
        timeout,
    };
    poll_until(ctx, client, &request, schedule, launch)
}

/// The wait of a launch: the last document seen, and what the wait says of it.
struct LaunchWait<'a> {
    kind: Kind<'a>,
    doc: Value,
    /// What the launch was waiting for when it was last looked at.
    pending: Pending,
    id: &'a str,
    /// The launched VM or app, as the messages name it.
    named: &'a str,
    hint: &'a str,
    /// The poll's action, for the error of a failed poll ([`Kind::failed`]).
    action: String,
    timeout: Duration,
}

impl LaunchWait<'_> {
    /// The last document seen, as it is printed.
    fn printed(&mut self) -> Value {
        self.kind.printed(std::mem::take(&mut self.doc))
    }
}

impl Wait for LaunchWait<'_> {
    type Output = Printed;

    fn next(&mut self) -> Next<Printed> {
        match self.kind.state(&self.doc) {
            Launch::Done => Next::Stop(Ok(Printed::Document(self.printed()))),
            Launch::Ended(status) => {
                let message = format!(
                    "{} reached {} instead of `running`",
                    self.named,
                    phrase::quote(status)
                );
                Next::Stop(Err(Error::new(Failure::PartialFailure, message)
                    .with_hint(self.hint)
                    .after_write(self.printed())))
            }
            Launch::Waiting(pending) => {
                self.pending = pending;
                Next::Poll
            }
        }
    }

    fn take(&mut self, answer: Value) -> Result<()> {
        // A document for another VM, or for none, is not this launch: printing it would
        // report another VM's status as this one's.
        match view_of::<Vm>(&answer).vm_id {
            Some(id) if id == self.id => {
                self.doc = answer;
                Ok(())
            }
            other => {
                let noun = self.kind.noun();
                let other = match other {
                    Some(other) if self.kind.may_quote(&other) => {
                        format!("the {noun} {}", phrase::quote(other))
                    }
                    Some(_) => format!("another {noun}"),
                    None => format!("no {noun}"),
                };
                let message = format!(
                    "the poll of {} answered with the document of {other}",
                    self.named
                );
                Err(Error::internal(message)
                    .with_hint(self.hint)
                    .after_write(self.printed()))
            }
        }
    }

    fn timed_out(mut self) -> Result<Printed> {
        let (named, after) = (self.named, phrase::count(self.timeout.as_secs(), "second"));
        let message = match self.pending {
            Pending::Booting => format!("{named} was not running after {after}"),
            Pending::Stalled => format!(
                "{named} was not running after {after}: its boot stalled (`boot_stalled`) and \
                 has not recovered"
            ),
            Pending::Unreachable => format!(
                "{named} was running, but its public IPv4 was not reachable (`network_ready`) \
                 after {after}"
            ),
        };
        Err(Error::new(Failure::Timeout, message)
            .with_hint(self.hint)
            .after_write(self.printed()))
    }

    // A poll is a read, so polling on after a passing outage cannot launch twice: the last
    // good document is kept, and only a run of failures ends the wait.
    fn failed(mut self, error: Error) -> Error {
        let error = self.kind.failed(error, &self.action);
        error.with_hint(self.hint).after_write(self.printed())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn an_app_launch_is_done_only_when_status_and_boot_phase_are_running() {
        let state = |app: Value| app_state(&app);
        assert_eq!(
            state(json!({"status": "running", "boot_phase": "running"})),
            Launch::Done
        );
        for app in [
            json!({"status": "running", "boot_phase": "pulling"}),
            json!({"status": "running"}),
            json!({"status": "provisioning", "boot_phase": "running"}),
            json!({}),
            json!({"status": 1, "boot_phase": null}),
        ] {
            assert_eq!(
                state(app.clone()),
                Launch::Waiting(Pending::Booting),
                "{app}"
            );
        }
        assert_eq!(
            state(json!({"status": "failed", "boot_stalled": true})),
            Launch::Waiting(Pending::Stalled)
        );
        for (app, ended) in [
            (
                json!({"status": "failed", "boot_phase": "running"}),
                "failed",
            ),
            (json!({"status": "decommissioning"}), "decommissioning"),
            (json!({"status": "decommissioned"}), "decommissioned"),
            (
                json!({"status": "running", "boot_phase": "failed"}),
                "failed",
            ),
        ] {
            assert_eq!(state(app.clone()), Launch::Ended(ended), "{app}");
        }
    }
}
