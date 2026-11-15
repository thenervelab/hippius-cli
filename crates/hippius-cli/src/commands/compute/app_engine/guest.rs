//! `hipc compute app-engine guest-components update`: read the guest components, and when a
//! newer release is offered, `POST .../guest-upgrade/` onto it.
//!
//! The order (tenet 5): argv; the dry run, which sends nothing and shows a placeholder for
//! the release numbers only a read can give; the token; `GET guest-components/`; then either
//! nothing, when the app is already on the newest release, or the question and the post. No
//! `--yes` is required when nothing is posted. A 409 `guest-already-current` is that same
//! success, after the post is read again: the post did not change the app.

use hippius::routes::compute::{self, GuestComponents};
use hippius::{Client, Error, Request, Response, Result, Slot, view_of};
use serde_json::{Value, json};

use super::super::refused_as;
use super::{APPS_LIST_COMMAND, unavailable_hint, warn_needs_operator};
use crate::cli::AppTarget;
use crate::commands::Printed;
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
        Write::DryRun(doc) => return Ok(doc.into()),
        Write::Checked(checked) => checked,
    };
    let components = read_components(ctx, &client, &read)?;
    warn_needs_operator(ctx, &vm_id, &components);
    match offer(&view_of(&components)) {
        Ok(Offer::Current) => {
            // Nothing is posted, so this check is not a confirmation that was skipped.
            drop(checked);
            ctx.note(&already(vm_id));
            Ok(Printed::Document(components))
        }
        Ok(Offer::Upgrade { current, newest }) => schedule(
            ctx,
            &client,
            Upgrade {
                vm_id,
                current,
                newest,
            },
            checked,
        ),
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
#[derive(Clone, Copy)]
struct Upgrade<'a> {
    vm_id: &'a str,
    current: Option<u64>,
    newest: u64,
}

/// Asks, then posts the upgrade. `guest-already-current` exits 0 with the read taken again.
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
            current_again(ctx, client, vm_id)
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
        .map_err(|error| error.with_side_effects(hippius::SideEffects::Partial))?;
    if let Some(command) = ctx.next_command(&format!(
        "compute app-engine guest-components update-status {vm_id}"
    )) {
        ctx.note(&format!("poll the upgrade with `{command}`"));
    }
    Ok(Printed::Document(scheduled))
}

/// The post lost a race and changed nothing. The read taken again is what is printed; its
/// failure is returned as it is, since the post did not change the app.
fn current_again(ctx: &Context<'_>, client: &Client, vm_id: &str) -> Result<Printed> {
    let components = read_components(ctx, client, &guest_read(vm_id))?;
    ctx.note(&already(vm_id));
    Ok(Printed::Document(components))
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
