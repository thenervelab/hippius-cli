//! `hipc compute app-engine guest-components update`: read the guest components, and when a
//! newer release is offered, `POST .../guest-upgrade/` onto it.
//!
//! The order (tenet 5): argv; the dry run, which sends nothing and shows a placeholder for
//! the release numbers only a read can give; the token; `GET guest-components/`; then either
//! nothing, when no newer release is offered, or the question and the post. No `--yes` is
//! required when nothing is posted. The note says the app is on the newest release only when
//! `release` and `newest_release` are the same number. A 409 `guest-already-current` reads
//! the guest components again: an offer that is still an update is `conflict`, and that read
//! is printed; otherwise the command exits 0 with the note the re-read supports. An upgrade
//! already in flight is `conflict` before the question, and nothing is posted.

use hippius::routes::compute::{self, GuestComponents};
use hippius::{Client, Error, Failure, Request, Response, Result, SideEffects, Slot, view_of};
use serde_json::{Value, json};

use super::{APPS_LIST_COMMAND, unavailable_hint, warn_needs_operator};
use crate::cli::AppTarget;
use crate::commands::compute::refused_as;
use crate::commands::{Printed, run_hint};
use crate::context::{Checked, Context, Write, WritePlan};

/// The release number a dry run cannot know until the read.
const NEWEST: &str = "<the newest guest components release>";

/// The release the app runs, which a dry run cannot know until the read. The bare base is
/// JSON null in a real post, and this placeholder in a dry run.
const CURRENT: &str = "<the release the app runs>";

/// What the question says the upgrade does to the running app.
const RESTART: &str = "the VM restarts and its attestation measurement changes; volumes and \
     its network identity are kept";

/// What the guest-components read says to do.
#[derive(Debug, PartialEq, Eq)]
enum Offer {
    /// Already on the newest release, or none is offered. Print the read and exit 0.
    Current,
    /// Install `newest`. `current` is `None` on the bare base.
    Upgrade { current: Option<u64>, newest: u64 },
}

pub(super) fn update(ctx: &Context<'_>, target: &AppTarget) -> Result<Printed> {
    let vm_id = target.vm_id.as_str();
    let client = ctx.load_authed_client()?;
    let read = guest_read(vm_id);
    let draft = guest_draft(vm_id);
    let checked = match ctx.check_write(&client, &WritePlan::new(&[&read], &draft))? {
        // The guest components are read before the question, which names the releases.
        Write::DryRun(doc) => {
            return Ok(doc.reads_first(ctx.describe(&client, &read)?).into());
        }
        Write::Checked(checked) => checked,
    };
    let components = read_components(ctx, &client, &read)?;
    warn_needs_operator(ctx, &vm_id, &components);
    let components_view = view_of(&components);
    match offer(&components_view) {
        Ok(Offer::Current) => {
            // Nothing is posted, so this check is not a confirmation that was skipped.
            drop(checked);
            ctx.note(&current_note(vm_id, &components_view));
            Ok(Printed::Document(components))
        }
        Ok(Offer::Upgrade { current, newest }) => {
            if upgrade_in_flight(&components_view) {
                // The post would be refused, so the question is not asked.
                drop(checked);
                return Err(already_in_flight(ctx, vm_id));
            }
            schedule(
                ctx,
                &client,
                Upgrade {
                    vm_id,
                    current,
                    newest,
                },
                checked,
            )
        }
        Err(error) => Err(error.with_hint(reread(vm_id))),
    }
}

/// `GET` the guest components. A missing app is `not_found` with the list hint.
fn guest_read(vm_id: &str) -> Request {
    Request::to(&compute::APP_ENGINE_GUEST_COMPONENTS, [&vm_id])
        .about(format!("get the guest components of the app `{vm_id}`"))
        .listed_by(APPS_LIST_COMMAND)
}

/// The post a dry run describes. The numbers are placeholders: a dry run does not read them.
fn guest_draft(vm_id: &str) -> Request {
    let body = json!({
        "confirm": true,
        "release": NEWEST,
        "expected_current_release": CURRENT,
    });
    Request::to(&compute::APP_ENGINE_GUEST_UPGRADE_CREATE, [&vm_id])
        .json(&body)
        .about(format!("upgrade the guest components of the app `{vm_id}`"))
        .listed_by(APPS_LIST_COMMAND)
}

/// Sends `read`. A 403 outside the feature names `app-engine get`.
fn read_components(ctx: &Context<'_>, client: &Client, read: &Request) -> Result<Value> {
    ctx.send(client, read)
        .and_then(|response| response.document())
        .map_err(unavailable_hint)
}

/// The upgrade the read offered: which app, from which release, onto which.
#[derive(Debug, Clone, Copy)]
struct Upgrade<'a> {
    vm_id: &'a str,
    current: Option<u64>,
    newest: u64,
}

/// Asks, then posts the upgrade. `guest-already-current` is read again.
fn schedule(
    ctx: &Context<'_>,
    client: &Client,
    upgrade: Upgrade<'_>,
    checked: Checked,
) -> Result<Printed> {
    let Upgrade {
        vm_id,
        current,
        newest,
    } = upgrade;
    let action = upgrade_action(vm_id, current, newest);
    let confirmed = ctx.confirm_write(checked, &action, Some(RESTART))?;
    let request = upgrade_post(vm_id, current, newest, &action);
    match ctx.send_confirmed(client, &request, &confirmed) {
        Ok(response) => accepted(ctx, vm_id, &response),
        Err(error) if refused_as(&error, 409, "guest-already-current") => {
            current_again(ctx, client, vm_id, error)
        }
        Err(error) => Err(unavailable_hint(error)),
    }
}

/// The post, with `expected_current_release` null on the bare base so the check is not skipped.
fn upgrade_post(vm_id: &str, current: Option<u64>, newest: u64, action: &str) -> Request {
    let body = compute::GuestUpgradeBody {
        confirm: true,
        release: newest,
        expected_current_release: current,
    };
    Request::to(&compute::APP_ENGINE_GUEST_UPGRADE_CREATE, [&vm_id])
        .json(&body)
        .about(action)
        .listed_by(APPS_LIST_COMMAND)
}

/// The 202. A body hipc cannot read is `partial`: the upgrade was accepted.
fn accepted(ctx: &Context<'_>, vm_id: &str, response: &Response) -> Result<Printed> {
    let scheduled = response
        .document()
        .map_err(|error| error.with_side_effects(SideEffects::Partial))?;
    if let Some(command) = ctx.next_command(&format!(
        "compute app-engine guest-components update-status {vm_id}"
    )) {
        ctx.note(&format!("poll the upgrade with `{command}`"));
    }
    Ok(Printed::Document(scheduled))
}

/// The post lost a race. The read taken again decides: still an update is `conflict`, with
/// that read printed; otherwise the command exits 0. A failure of the re-read is returned
/// as it is, since the post did not change the app.
fn current_again(
    ctx: &Context<'_>,
    client: &Client,
    vm_id: &str,
    refused: Error,
) -> Result<Printed> {
    let components = read_components(ctx, client, &guest_read(vm_id))?;
    let read = view_of(&components);
    match offer(&read) {
        Ok(Offer::Current) => {
            ctx.note(&current_note(vm_id, &read));
            Ok(Printed::Document(components))
        }
        Ok(Offer::Upgrade { .. }) => Err(still_offered(vm_id, refused, components)),
        Err(error) => Err(error.with_hint(reread(vm_id))),
    }
}

/// The 409 rejected the post. The re-read is printed because it still offers an update, and
/// side effects stay none: the write did not go through.
fn still_offered(vm_id: &str, refused: Error, components: Value) -> Error {
    // `after_write` is what puts the re-read on stdout. Side effects go back to none: the
    // 409 rejected the post, so the app is unchanged.
    refused
        .with_message(format!(
            "the API answered 409 `guest-already-current` for the app `{vm_id}`, and the \
             guest components it then returned still offer an update"
        ))
        .with_hint(reread(vm_id))
        .after_write(components)
        .with_side_effects(SideEffects::None)
}

/// What the upgrade does, for the question: the two release numbers the read showed.
fn upgrade_action(vm_id: &str, current: Option<u64>, newest: u64) -> String {
    let from = match current {
        Some(release) => format!("release `{release}`"),
        None => "the bare base".to_owned(),
    };
    format!("upgrade the guest components of the app `{vm_id}` from {from} to release `{newest}`")
}

fn already(vm_id: &str) -> String {
    format!("`{vm_id}` is already on the newest guest components release")
}

/// The note for a read that offers no upgrade. Both numbers equal is the newest release.
/// A null newest, or `update_available: false` beside two different numbers, is not.
fn current_note(vm_id: &str, read: &GuestComponents) -> String {
    if on_newest(read) {
        already(vm_id)
    } else {
        format!("no newer guest components release is offered for `{vm_id}`")
    }
}

fn on_newest(read: &GuestComponents) -> bool {
    matches!(
        (&read.release, &read.newest_release),
        (Slot::Value(current), Slot::Value(newest)) if current == newest
    )
}

fn upgrade_in_flight(read: &GuestComponents) -> bool {
    read.upgrade.as_ref().and_then(|job| job.in_flight) == Some(true)
}

fn already_in_flight(ctx: &Context<'_>, vm_id: &str) -> Error {
    let words = format!("compute app-engine guest-components update-status {vm_id}");
    Error::new(
        Failure::Conflict,
        format!("an upgrade of the guest components of the app `{vm_id}` is already in flight"),
    )
    .with_hint(run_hint(ctx, &words, "to poll the upgrade"))
}

fn reread(vm_id: &str) -> String {
    format!("run `hipc compute app-engine guest-components get {vm_id}` and read it again")
}

/// What `read` offers. `update_available: false` is current even when the numbers differ.
/// A newest release with no `release` hipc can name is internal: the post would skip the
/// check that catches a race. Nothing here is posted.
fn offer(read: &GuestComponents) -> Result<Offer> {
    if read.update_available == Some(false) {
        return Ok(Offer::Current);
    }
    let newest = match &read.newest_release {
        Slot::Null if read.update_available == Some(true) => {
            return Err(cannot_tell(
                "`newest_release` is null while `update_available` is true",
            ));
        }
        Slot::Null => return Ok(Offer::Current),
        Slot::Missing => return Err(cannot_tell("`newest_release` is missing")),
        Slot::Other => return Err(cannot_tell("`newest_release` is not a release number")),
        Slot::Value(newest) => *newest,
    };
    match &read.release {
        Slot::Value(current) if *current == newest => Ok(Offer::Current),
        Slot::Value(current) => Ok(Offer::Upgrade {
            current: Some(*current),
            newest,
        }),
        Slot::Null => Ok(Offer::Upgrade {
            current: None,
            newest,
        }),
        Slot::Missing => Err(cannot_tell(
            "`release` is missing, so `expected_current_release` cannot be named",
        )),
        Slot::Other => Err(cannot_tell(
            "`release` is not a release number or null, so `expected_current_release` cannot \
             be named",
        )),
    }
}

fn cannot_tell(why: &str) -> Error {
    Error::internal(format!(
        "cannot tell which guest components release to install: {why}"
    ))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn read(value: &Value) -> GuestComponents {
        view_of(value)
    }

    fn told(value: &Value) -> Result<Offer> {
        offer(&read(value))
    }

    #[test]
    fn the_question_names_the_two_releases_and_the_restart() {
        assert_eq!(
            upgrade_action("vm-1a2b", Some(3), 7),
            "upgrade the guest components of the app `vm-1a2b` from release `3` to release `7`"
        );
        assert_eq!(
            upgrade_action("vm-1a2b", None, 7),
            "upgrade the guest components of the app `vm-1a2b` from the bare base to release `7`"
        );
        assert!(RESTART.contains("attestation measurement"));
        assert!(RESTART.contains("network identity"));
        assert_eq!(
            already("vm-1a2b"),
            "`vm-1a2b` is already on the newest guest components release"
        );
    }

    /// Equal numbers are the newest release. A false flag, or a null newest, is not, even
    /// though [`offer`] calls both current.
    #[test]
    fn the_note_names_the_newest_release_only_when_the_numbers_are_equal() {
        let equal = json!({"release": 7, "newest_release": 7, "update_available": true});
        assert_eq!(
            current_note("vm-1a2b", &read(&equal)),
            "`vm-1a2b` is already on the newest guest components release"
        );
        let stale = json!({"release": 7, "newest_release": 9, "update_available": false});
        assert_eq!(
            current_note("vm-1a2b", &read(&stale)),
            "no newer guest components release is offered for `vm-1a2b`"
        );
        let none = json!({"release": 7, "newest_release": null});
        assert_eq!(
            current_note("vm-1a2b", &read(&none)),
            "no newer guest components release is offered for `vm-1a2b`"
        );
        let both_null = json!({"release": null, "newest_release": null});
        assert_eq!(
            current_note("vm-1a2b", &read(&both_null)),
            "no newer guest components release is offered for `vm-1a2b`"
        );
    }

    /// Only a boolean `true` is in flight. A missing job, null, false, or another type is not.
    #[test]
    fn an_upgrade_in_flight_is_only_a_true_flag() {
        assert!(upgrade_in_flight(&read(
            &json!({"upgrade": {"in_flight": true}})
        )));
        assert!(!upgrade_in_flight(&read(
            &json!({"upgrade": {"in_flight": false}})
        )));
        assert!(!upgrade_in_flight(&read(&json!({}))));
        assert!(!upgrade_in_flight(&read(&json!({"upgrade": null}))));
        assert!(!upgrade_in_flight(&read(
            &json!({"upgrade": {"in_flight": "true"}})
        )));
    }

    /// `false` is current even when a newer number is shown, and so is a newest of null
    /// unless the read also says an update is available.
    #[test]
    fn false_or_no_newest_release_is_current() {
        let current = Offer::Current;
        assert_eq!(
            told(&json!({"update_available": false, "release": 3, "newest_release": 9})).unwrap(),
            current
        );
        assert_eq!(
            told(&json!({"update_available": false, "newest_release": null})).unwrap(),
            current
        );
        assert_eq!(
            told(&json!({"newest_release": null, "release": 3})).unwrap(),
            current
        );
        assert_eq!(
            told(&json!({"update_available": true, "release": 7, "newest_release": 7})).unwrap(),
            current
        );
    }

    /// A different number, or the bare base, is an upgrade, including when the flag is absent.
    #[test]
    fn a_newer_release_or_the_bare_base_is_an_upgrade() {
        assert_eq!(
            told(&json!({"update_available": true, "release": 3, "newest_release": 7})).unwrap(),
            Offer::Upgrade {
                current: Some(3),
                newest: 7,
            }
        );
        assert_eq!(
            told(&json!({"release": 3, "newest_release": 7})).unwrap(),
            Offer::Upgrade {
                current: Some(3),
                newest: 7,
            }
        );
        assert_eq!(
            told(&json!({"update_available": true, "release": null, "newest_release": 7})).unwrap(),
            Offer::Upgrade {
                current: None,
                newest: 7,
            }
        );
    }

    /// A read that contradicts itself, or that cannot name the release to expect, is internal
    /// and posts nothing.
    #[test]
    fn a_read_that_cannot_name_the_release_is_internal() {
        let message = |value: &Value| told(value).unwrap_err().message().to_owned();
        assert!(
            message(&json!({"update_available": true, "newest_release": null, "release": 3}))
                .contains("null while `update_available` is true"),
        );
        assert!(message(&json!({"release": 3})).contains("`newest_release` is missing"));
        assert!(
            message(&json!({"newest_release": "7", "release": 3})).contains("not a release number"),
        );
        assert!(message(&json!({"newest_release": 7})).contains("`release` is missing"),);
        assert!(
            message(&json!({"newest_release": 7, "release": "3"}))
                .contains("not a release number or null"),
        );
    }
}
