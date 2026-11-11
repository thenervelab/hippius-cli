//! The launch of a VM or an App Engine app: the clauses its question shares, whether it
//! waits, and the wait for `running`.

use std::time::{Duration, Instant};

use hippius::routes::compute::{self, Vm, VmStatus};
use hippius::{Client, Code, Error, Request, Result, phrase, view_of};
use serde_json::Value;

use crate::cli::{BackupInterval, FailoverMode};
use crate::commands::Printed;
use crate::commands::compute::VMS;
use crate::commands::poll::{Next, Schedule, Wait, poll_until};
use crate::context::Context;

/// How often a launch is polled, 2 seconds: a launch takes on the order of a
/// minute, so a shorter interval only adds requests.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// The launch clause of `--on-guest-poweroff stop`, for the VM and the app questions: the VM
/// is then billed while it sits stopped.
pub(crate) const STAYS_STOPPED: &str = "a power-off from inside the VM leaving it stopped, and \
                                        billed as a stopped VM until it is started or deleted";

/// The launch clause of `--backup-interval`, for the VM and the app questions: how often, the
/// failover, and that stored backups are billed, so neither question can leave the bill out.
pub(crate) fn backups_clause(interval: BackupInterval, failover: Option<FailoverMode>) -> String {
    let failover = failover.map_or(String::new(), |mode| {
        format!(" and {}", failover_clause(mode))
    });
    format!(
        "backups every {}{failover}, stored backups billed per GB-hour at the price that `hipc \
         compute backups options` shows",
        phrase::count(u64::from(interval.seconds()), "second")
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
pub(crate) fn launch_wait(
    ctx: &Context<'_>,
    wait: bool,
    no_wait: bool,
    wait_timeout: Option<u64>,
) -> Option<Duration> {
    let waits = wait || wait_timeout.is_some() || !(no_wait || ctx.reader_is_program());
    waits.then(|| wait_timeout.map_or(WAIT_TIMEOUT, Duration::from_secs))
}

/// What ends a launch wait.
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

/// Where a launch stands, from the VM's `status`, `boot_stalled`, and `network_ready`.
#[derive(Debug)]
enum Launch {
    /// The goal is met.
    Done,
    /// A terminal status: `failed` without `boot_stalled`, `decommissioning`, or
    /// `decommissioned`.
    Ended(VmStatus),
    /// Not there yet, so the wait goes on.
    Waiting(Pending),
}

/// Why a launch wait goes on, which a timeout names.
#[derive(Debug)]
enum Pending {
    /// `pending`, `provisioning`, a missing status, or a status this version does not
    /// know, which may be new and transient.
    Booting,
    /// `failed` with `boot_stalled` true. Swagger: such a VM "returns to `running` by itself
    /// once the guest signals", so it is not terminal.
    Stalled,
    /// `running`, but `network_ready` is not true yet.
    Unreachable,
}

fn launch_state(vm: &Value, goal: Goal) -> Launch {
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
        Some(
            status @ (VmStatus::Failed | VmStatus::Decommissioning | VmStatus::Decommissioned),
        ) => Launch::Ended(status),
        Some(VmStatus::Other(_)) | None => Launch::Waiting(Pending::Booting),
    }
}

/// Polls the launched VM until `goal` is met, its status is terminal, or `timeout` has
/// passed. Every failure prints the last VM document seen, so `vm_id` is not lost, and is
/// never retryable: a retry of the create would launch, and bill, a second VM
/// (`TENETS.md` tenet 4).
///
/// A poll can end up to `--request-timeout` past the deadline, because the last request
/// is not cut short. A run killed during the wait prints nothing on stdout; text mode
/// names the VM on stderr before the wait, unless `--quiet`, and JSON mode stays silent
/// (tenet 3).
pub(super) fn wait(
    ctx: &Context<'_>,
    client: &Client,
    launched: Value,
    goal: Goal,
    timeout: Duration,
) -> Result<Printed> {
    let Some(vm_id) = view_of::<Vm>(&launched).vm_id else {
        return Err(
            Error::internal("the launch answer has no `vm_id`, so it cannot be waited on")
                .with_hint(format!("run `{VMS}` to find the new VM"))
                .after_write(launched),
        );
    };
    // The id comes from the server, so every message quotes it through `phrase::quote`.
    let vm_quoted = phrase::quote(&vm_id);
    let hint = match crate::cli::as_argument(&vm_id, crate::cli::vm_id) {
        Some(id) => format!(
            "run {} to see where the launch stands",
            phrase::quote(format!("hipc compute vms get {id}"))
        ),
        None => format!("run `{VMS}` to find the new VM"),
    };
    if let Launch::Waiting(_) = launch_state(&launched, goal) {
        // The id goes into the poll URL, so it is held to the rule of the `vm_id` argument
        // before the first poll: `a?b=1` or `a/b` would poll another route. A launch answer
        // that already meets the goal or is terminal needs no poll, so it is not refused.
        if let Err(rule) = crate::cli::vm_id(&vm_id) {
            let message = format!(
                "the launch answer names the VM {vm_quoted}, which `hipc` cannot poll: {rule}"
            );
            return Err(Error::internal(message)
                .with_hint(hint)
                .after_write(launched));
        }
        let until = match goal {
            Goal::Running => "it is running",
            Goal::Reachable => "it is running and its public IPv4 is reachable",
        };
        ctx.note(&format!(
            "launched the VM {vm_quoted}; waiting until {until}"
        ));
    }
    let request = Request::to(&compute::VM, [&vm_id]).about(format!("poll the VM {vm_quoted}"));
    let schedule = Schedule {
        first: POLL_INTERVAL,
        interval: POLL_INTERVAL,
        deadline: Instant::now() + timeout,
    };
    let launch = VmLaunch {
        vm: launched,
        goal,
        pending: Pending::Booting,
        vm_id: &vm_id,
        vm_quoted: &vm_quoted,
        hint: &hint,
        timeout,
    };
    poll_until(ctx, client, &request, schedule, launch)
}

/// The wait of a VM launch: the last VM document seen, and what the wait says of it.
struct VmLaunch<'a> {
    vm: Value,
    goal: Goal,
    /// What the VM was waiting for when it was last looked at.
    pending: Pending,
    vm_id: &'a str,
    vm_quoted: &'a str,
    hint: &'a str,
    timeout: Duration,
}

impl Wait for VmLaunch<'_> {
    type Output = Printed;

    fn next(&mut self) -> Next<Printed> {
        match launch_state(&self.vm, self.goal) {
            Launch::Done => Next::Stop(Ok(Printed::Document(std::mem::take(&mut self.vm)))),
            Launch::Ended(status) => {
                let message = format!(
                    "the VM {} reached {} instead of `running`",
                    self.vm_quoted,
                    phrase::quote(status.as_str())
                );
                Next::Stop(Err(Error::new(Code::PartialFailure, message)
                    .with_hint(self.hint)
                    .after_write(std::mem::take(&mut self.vm))))
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
            Some(id) if id == self.vm_id => {
                self.vm = answer;
                Ok(())
            }
            other => {
                let named = match other {
                    Some(other) => format!("the VM {}", phrase::quote(other)),
                    None => "no VM".to_owned(),
                };
                let message = format!(
                    "the poll of the VM {} answered with the document of {named}",
                    self.vm_quoted
                );
                Err(Error::internal(message)
                    .with_hint(self.hint)
                    .after_write(std::mem::take(&mut self.vm)))
            }
        }
    }

    fn timed_out(self) -> Result<Printed> {
        let (vm, after) = (
            self.vm_quoted,
            phrase::count(self.timeout.as_secs(), "second"),
        );
        let message = match self.pending {
            Pending::Booting => format!("the VM {vm} was not running after {after}"),
            Pending::Stalled => format!(
                "the VM {vm} was not running after {after}: its boot stalled (`boot_stalled`) \
                 and has not recovered"
            ),
            Pending::Unreachable => format!(
                "the VM {vm} was running, but its public IPv4 was not reachable \
                 (`network_ready`) after {after}"
            ),
        };
        Err(Error::new(Code::Timeout, message)
            .with_hint(self.hint)
            .after_write(self.vm))
    }

    // A poll is a read, so polling on after a passing outage cannot launch twice: the last
    // good document is kept, and only a run of failures ends the wait.
    fn failed(self, error: Error) -> Error {
        error.with_hint(self.hint).after_write(self.vm)
    }
}
