//! `hipc compute backups`: a VM's backup policy, its backups, and a restore.

use hippius::routes::Route;
use hippius::routes::compute::{self, Vm};
use hippius::{Client, Error, Request, Response, Result, phrase, view_of};
use serde_json::{Value, json};

use crate::cli::{
    BackupPolicy, BackupPolicyUpdate, BackupRestore, Backups, FailoverMode, VmId, VmTarget,
};
use crate::commands::compute::launch::failover_clause;
use crate::commands::compute::{VMS_LIST_COMMAND, refused_as};
use crate::commands::{
    Catalog, Decided, Printed, keyed_list, print_body, print_body_or, print_list_with,
    read_then_write, send_from_catalog,
};
use crate::context::{ConfirmedStep, Context, Gate, WritePlan};

pub(super) fn backups(ctx: &Context<'_>, client: &Client, command: &Backups) -> Result<Printed> {
    let get = |request: Request| print_body(ctx, client, &request, Gate::Open);
    let of_vm = |route: &Route<1>, what: &str, vm_id: &VmId| {
        Request::to(route, [vm_id])
            .about(format!("get the {what} of the VM `{vm_id}`"))
            .listed_by(VMS_LIST_COMMAND)
    };
    match command {
        Backups::Options => get(options()),
        Backups::Policy(BackupPolicy::Get(VmTarget { vm_id })) => {
            get(of_vm(&compute::BACKUP_POLICY, "backup policy", vm_id))
        }
        Backups::Policy(BackupPolicy::Update(args)) => policy_update(ctx, client, args),
        // Swagger: "Idempotent". The 204 has no body, so the document says what is now true.
        Backups::Policy(BackupPolicy::Delete(VmTarget { vm_id })) => {
            let request = Request::to(&compute::BACKUP_POLICY_DELETE, [vm_id])
                .about(format!("turn off the backups of the VM `{vm_id}`"))
                .listed_by(VMS_LIST_COMMAND);
            let gate = Gate::Confirm {
                consequence: Some(BACKUPS_KEPT),
            };
            let done = json!({ "vm_id": vm_id, "enabled": false });
            print_body_or(ctx, client, &request, gate, done)
        }
        // The answer is an object (`state`, `stored_bytes`, `restore_points`, ...): the list
        // envelope holds `restore_points` as `items`, with the other fields beside it.
        // Swagger requires no field of the answer, so one without `restore_points`, or with
        // `null`, such as a VM whose backups are off, is an empty list.
        Backups::List(VmTarget { vm_id }) => {
            let request = of_vm(&compute::BACKUPS, "backups", vm_id);
            print_list_with(ctx, client, &request, |response| {
                let mut answer = response.json()?;
                if let Value::Object(fields) = &mut answer
                    && fields.get("restore_points").is_none_or(Value::is_null)
                {
                    fields.insert("restore_points".to_owned(), Value::Array(Vec::new()));
                }
                keyed_list(answer, &request, "restore_points")
            })
        }
        Backups::Restore(args) => restore(ctx, client, args),
        // A restore runs for minutes. Operations that finish later get a status command,
        // not a wait, so this reads once.
        Backups::RestoreStatus(VmTarget { vm_id }) => get(of_vm(
            &compute::BACKUP_RESTORE_STATUS,
            "restore status",
            vm_id,
        )),
    }
}

/// `PUT .../backup/` with `interval_s`, and `failover_mode` only when passed: swagger says
/// an omitted mode keeps the current one. The interval is checked against the backup options
/// after the question ([`send_from_catalog`]).
fn policy_update(ctx: &Context<'_>, client: &Client, args: &BackupPolicyUpdate) -> Result<Printed> {
    let vm_id = &args.vm.vm_id;
    let seconds = args.interval;
    let failover = args.failover.map_or_else(String::new, |mode| {
        format!(", with {}", failover_clause(mode))
    });
    let request = Request::to(&compute::BACKUP_POLICY_UPDATE, [vm_id])
        .json(&compute::BackupPolicyBody {
            interval_s: seconds,
            failover_mode: args.failover.map(FailoverMode::code),
        })
        .listed_by(VMS_LIST_COMMAND)
        .about(format!(
            "back up the VM `{vm_id}` every {}{failover}",
            phrase::count(u64::from(seconds), "second")
        ));
    let consequence = "stored backups are billed per GB-hour from the balance, at the price \
                       `hipc compute backups options` shows";
    let wanted = seconds.to_string();
    let catalog = intervals(&wanted);
    match send_from_catalog(ctx, client, Some(&catalog), &request, Some(consequence))? {
        ConfirmedStep::Sent(response, _covers_nothing_more) => {
            response.document().map(Printed::Document)
        }
        ConfirmedStep::DryRun(doc) => Ok(doc.into()),
    }
}

/// The backup intervals the server offers, which `wanted`, a whole number of seconds, is
/// checked against before a policy update or a launch with `--backup-interval` sends it
/// (owner decisions D5 and D19): `intervals[].seconds` of the backup options.
pub(super) fn intervals(wanted: &str) -> Catalog<'_> {
    Catalog {
        read: options(),
        codes: |answer| {
            let options: compute::BackupOptions = view_of(answer);
            options
                .intervals
                .into_iter()
                .flatten()
                .filter_map(|interval| interval.seconds.map(|seconds| seconds.to_string()))
                .collect()
        },
        what: "a backup interval",
        wanted,
        listed_by: "hipc compute backups options",
    }
}

/// The read of the backup options: the intervals, the retention, and the prices.
fn options() -> Request {
    Request::to(&compute::BACKUP_OPTIONS, []).about("get the backup options")
}

/// What a dry run shows for the server's `confirm` field. A real run fills it with the
/// VM's `name`, which only a read can give, and a dry run sends nothing.
const NAME_PLACEHOLDER: &str = "<the VM's name>";

/// `POST .../backups/restore/`. Swagger: "`confirm` must be the VM's `name`, exactly". `hipc` fills
/// it from `GET /api/compute/vms/<vm_id>/` rather than taking a `--confirm-name` flag, which would
/// be a second confirmation beside `--yes`. The read comes before the question, so the question
/// names the VM whose disk is overwritten and the name the server checks is one the person saw (a
/// read after the question would let a wrong id be confirmed under a name nobody read). With
/// `--yes` nobody sees the question, and the server's check is the only guard. The local refusals
/// (an account the route cannot use, the token) run before that read. A failed read is the error,
/// and nothing is written. A dry run sends neither: its document is the POST with the name as a
/// placeholder, and its `requests` lists the read, then the POST.
fn restore(ctx: &Context<'_>, client: &Client, args: &BackupRestore) -> Result<Printed> {
    let vm_id = &args.vm.vm_id;
    let point = &args.run_id;
    let post = |name: &str| {
        Request::to(&compute::BACKUP_RESTORE, [vm_id]).json(&compute::BackupRestoreBody {
            point_id: point,
            mode: compute::RestoreMode::InPlace,
            confirm: name,
            accept_rollback: args.accept_rollback,
        })
    };
    let read = Request::to(&compute::VM, [vm_id])
        .about(format!("get the VM `{vm_id}`"))
        .listed_by(VMS_LIST_COMMAND);
    let draft = post(NAME_PLACEHOLDER);
    let decide = || {
        let vm: Vm = view_of(&ctx.send(client, &read)?.document()?);
        let Some(name) = vm.name.as_deref() else {
            return Err(Error::internal(format!(
                "the VM `{vm_id}` has no `name`, so the restore's `confirm` cannot be filled"
            )));
        };
        let rollback = if args.accept_rollback {
            ", which may be a point from before its last reboot"
        } else {
            ""
        };
        let request = post(name).listed_by(VMS_LIST_COMMAND).about(format!(
            "overwrite the disk of the VM {} (`{vm_id}`) with the backup `{point}`{rollback}",
            phrase::quote(name)
        ));
        let consequence = "everything written after that backup is lost, and the VM stops for \
                           a few minutes";
        Ok(Decided::asked(request, Some(consequence)))
    };
    let render = |sent: Result<Response>| {
        sent.and_then(|response| response.document())
            .map_err(|error| {
                // A 400 for a point from before the VM's last reboot sent without
                // `accept_rollback`. Any other 400 gets no hint: a wrong hint is worse than none.
                if refused_as(&error, 400, "rollback-not-accepted") {
                    error.with_hint(
                        "pass `--accept-rollback` to restore a point from before the VM's last \
                         reboot",
                    )
                } else {
                    error
                }
            })
    };
    read_then_write(
        ctx,
        client,
        &WritePlan::new(&[&read], &draft),
        decide,
        render,
    )
}

/// What a backup policy delete leaves, for its question and `hipc api`'s.
pub(crate) const BACKUPS_KEPT: &str =
    "the backups already taken are kept 7 days, billed while kept, then deleted";
