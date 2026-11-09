//! `hipc hub`: Hub plans, the Hub subscription, the registry namespace and its robot login,
//! the registry usage, its repositories and artifacts, its CI keys (in `hub/keys.rs`), and its
//! audit events and the model index (in `hub/models.rs`) (`TENETS.md` tenet 11).
//!
//! Hub is hidden in another account, so `/api/registry/` refuses a membership before
//! anything is sent; only the public catalog goes out, without the header. Blob uploads
//! stay in `hippius-hub`, and `hipc` never shells out to it.

use std::path::{Path, PathBuf};

use hippius::{
    CHECK_FIRST_HINT, Client, Code, ConfigFile, Error, HubMarker, Method, Request, Result,
    SideEffects, Source, phrase, without_field,
};
use serde_json::{Value, json};

use crate::cli::{
    Hub, HubArtifacts, HubNamespace, HubRepositories, HubRobot, HubSubscribe, HubSubscription,
    PayFrom, Visibility,
};
use crate::commands::files::{save_secret, shown_once};
use crate::commands::{
    Printed, failed_part, print_body, print_body_or, print_envelope, print_with, require_public,
    require_reveal_or_file, run_hint, set_up_first,
};
use crate::context::{self, Context, Gate, Write};

pub(crate) mod keys;
mod models;

/// How long the double-subscribe marker holds, in seconds. A chain sync is on the order
/// of a few minutes; ten covers it with room to spare.
pub(crate) const MARKER_SECONDS: u64 = 10 * 60;

/// The Hub registry's host, which `docker login` takes (docs.hippius.com). The API does not name
/// it.
const REGISTRY_HOST: &str = "registry.hippius.com";

pub(crate) fn run(ctx: &Context<'_>, command: &Hub) -> Result<Printed> {
    match command {
        // The catalog needs no token, and sends one only when it is configured.
        Hub::Plans => print_envelope(
            ctx,
            &ctx.client(&context::config()?)?,
            &Request::get(PLANS).about("list the Hub plans"),
        ),
        Hub::Subscription(HubSubscription::Get) => print_envelope(
            ctx,
            &ctx.load_authed_client()?,
            &Request::get(SUBSCRIPTIONS).about("list the Hub subscriptions"),
        ),
        Hub::Subscription(HubSubscription::Subscribe(args)) => subscribe(ctx, args),
        Hub::Subscription(HubSubscription::Cancel { subscription_id }) => {
            cancel(ctx, &ctx.load_authed_client()?, *subscription_id)
        }
        Hub::Namespace(command) => namespace(ctx, command),
        Hub::Usage { repositories } => usage(ctx, *repositories),
        Hub::Keys(command) => keys::run(ctx, command),
        Hub::Repositories(command) => {
            repositories(ctx, command).map_err(|error| with_provision_hint(ctx, error))
        }
        Hub::Artifacts(command) => {
            artifacts(ctx, command).map_err(|error| with_provision_hint(ctx, error))
        }
        Hub::Events(command) => {
            models::events(ctx, command).map_err(|error| with_provision_hint(ctx, error))
        }
        Hub::Models(command) => models::run(ctx, command),
    }
}

/// The namespace's repositories (`registry_repositories_list`).
const REPOSITORIES: &str = "/registry/repositories/";

/// `hipc hub repositories`. Swagger lists no page parameter for the list, so it is read
/// whole: a `next` link is `internal`. It has no GET of one repository either, so `get`
/// reads the list and picks the row by `name`.
fn repositories(ctx: &Context<'_>, command: &HubRepositories) -> Result<Printed> {
    let list = Request::get(REPOSITORIES).about("list the Hub repositories");
    match command {
        HubRepositories::List => print_envelope(ctx, &ctx.load_authed_client()?, &list),
        HubRepositories::Get { repository } => {
            let client = ctx.load_authed_client()?;
            print_with(ctx, &client, &list, Gate::Open, |response| {
                let rows = hippius::page(response.json()?)?.into_complete(REPOSITORIES)?;
                repository_named(rows, repository)
            })
        }
        HubRepositories::Delete { repository } => {
            let client = ctx.load_authed_client()?;
            let request = Request::new(Method::Delete, format!("{REPOSITORIES}{repository}/"))
                .listed_by("hipc hub repositories list")
                .about(format!(
                    "delete the Hub repository `{repository}` with every artifact and tag in it"
                ));
            let done = json!({ "name": repository, "deleted": true });
            print_body_or(ctx, &client, &request, PERMANENT, done)
        }
    }
}

/// The confirmation of a Hub delete: the prompt adds that it cannot be undone. What is lost
/// is in each request's action, so the refusal without `--yes` names it too. That warning
/// and `--yes` are the whole confirmation: a second flag beside `--yes` would be one more
/// an agent learns to pass by habit (tenet 5).
pub(crate) const PERMANENT: Gate<'static> = Gate::Confirm {
    consequence: Some("this cannot be undone"),
};

/// The row of `rows` whose `name` is `wanted`.
///
/// # Errors
///
/// `not_found` when no row has that name.
fn repository_named(rows: Vec<Value>, wanted: &str) -> Result<Value> {
    rows.into_iter()
        .find(|row| row.get("name").and_then(Value::as_str) == Some(wanted))
        .ok_or_else(|| {
            Error::new(
                Code::NotFound,
                format!("the Hub registry has no repository `{wanted}`"),
            )
            .with_hint("run `hipc hub repositories list` to see each repository's name")
        })
}

/// `hipc hub artifacts`, under `/registry/repositories/<repository>/artifacts/`. Swagger
/// lists no page parameter for the list, so it is read whole, as the repositories are.
fn artifacts(ctx: &Context<'_>, command: &HubArtifacts) -> Result<Printed> {
    match command {
        HubArtifacts::List { repository } => {
            let client = ctx.load_authed_client()?;
            let action = format!("list the artifacts of the Hub repository `{repository}`");
            let request = Request::get(format!("{REPOSITORIES}{repository}/artifacts/"))
                .listed_by("hipc hub repositories list")
                .about(action);
            print_envelope(ctx, &client, &request)
        }
        HubArtifacts::Get {
            repository,
            reference,
        } => {
            let client = ctx.load_authed_client()?;
            let request = artifact(repository, reference, Method::Get).about(format!(
                "get the artifact `{reference}` of the Hub repository `{repository}`"
            ));
            print_body(ctx, &client, &request, Gate::Open)
        }
        HubArtifacts::Delete {
            repository,
            reference,
        } => {
            let client = ctx.load_authed_client()?;
            // Deleting by a tag deletes the artifact it points at, so its other tags go too.
            let request = artifact(repository, reference, Method::Delete).about(format!(
                "delete the artifact `{reference}` from the Hub repository `{repository}`, \
                 with every tag on it"
            ));
            let done = json!({ "repository": repository, "reference": reference, "deleted": true });
            print_body_or(ctx, &client, &request, PERMANENT, done)
        }
    }
}

/// A request for one artifact. Both names were checked when argv was parsed, so neither can
/// change the route, and the hint of a 404 can repeat them as words.
fn artifact(repository: &str, reference: &str, method: Method) -> Request {
    Request::new(
        method,
        format!("{REPOSITORIES}{repository}/artifacts/{reference}/"),
    )
    .listed_by(format!("hipc hub artifacts list {repository}"))
}

/// `GET /api/registry/usage/`, an object printed as sent, or with `--repositories` the
/// per-repository list `GET /api/registry/usage/repositories/` as an envelope. Swagger lists
/// no page parameter for the list, so it is read whole: a `next` link is `internal`.
fn usage(ctx: &Context<'_>, repositories: bool) -> Result<Printed> {
    let client = ctx.load_authed_client()?;
    let printed = if repositories {
        let request =
            Request::get("/registry/usage/repositories/").about("list the Hub usage by repository");
        print_envelope(ctx, &client, &request)
    } else {
        let request = Request::get("/registry/usage/").about("get the Hub usage");
        print_body(ctx, &client, &request, Gate::Open)
    };
    printed.map_err(|error| with_provision_hint(ctx, error))
}

/// Subscribes to a Hub plan without charging twice (tenet 11).
///
/// Order: argv checks, then `--dry-run` and the POST's refusals (a membership sends
/// nothing: the catalog read alone is personal and would go out), then the token, then the
/// confirmation, then the reads (the catalog to resolve the plan and to price the active
/// subscriptions, then the subscriptions), then the guard, then the writes. Under
/// `--dry-run` nothing is read either: a plan given by name shows as a placeholder
/// `plan_id`, and `requests` lists the reads, the cancel `--replace` may send, and the POST.
fn subscribe(ctx: &Context<'_>, args: &HubSubscribe) -> Result<Printed> {
    // The marker is kept in the profile this client was built from.
    let (settings, client) = ctx.authed(&context::config()?)?;
    // A real run resolves a name after the question, from the catalog it reads then.
    let plan_id = match args.plan.parse::<u64>() {
        Ok(id) => Value::from(id),
        Err(_) => Value::from(format!("<looked up from {}>", args.plan)),
    };
    let draft =
        Request::new(Method::Post, "/registry/subscribe/").json(&json!({ "plan_id": plan_id }));
    // The reads a real run sends after the question, which a dry run lists.
    let plans = Request::get(PLANS).about("list the Hub plans");
    let subscriptions = Request::get(SUBSCRIPTIONS).about("list the Hub subscriptions");
    let checked = match ctx.check_write(&client, &draft)? {
        Write::DryRun(mut doc) => {
            // Which subscriptions a real run cancels is known only after it reads them, which
            // a dry run does not do: the document adds the DELETE it may send first, once per
            // active paid subscription, with the id left as a placeholder, and lists it in
            // `requests` between the reads and the POST.
            // `{subscription_id}` is the one placeholder not written `<...>`: it shipped
            // before the style was set, and changing a field's value is a break (tenet 7).
            let cancel = Request::new(Method::Delete, "/registry/subscriptions/{subscription_id}/");
            let mut requests = vec![&plans, &subscriptions];
            if args.replace {
                doc["may_cancel_first"] = ctx.describe(&client, &cancel)?;
                requests.push(&cancel);
            }
            requests.push(&draft);
            doc["requests"] = ctx.describe_each(&client, &requests)?;
            return Ok(Printed::Document(doc));
        }
        Write::Checked(checked) => checked,
    };
    let action = if args.replace {
        format!(
            "cancel every active paid Hub plan, then subscribe to the Hub plan `{}`",
            args.plan
        )
    } else {
        format!("subscribe to the Hub plan `{}`", args.plan)
    };
    ctx.confirm_write(checked, &action, Some(PayFrom::Credits.how()))?;
    let plans = read_list(ctx, &client, &plans)?;
    let plan_id = resolve_plan(&plans, &args.plan)?;
    let subscriptions = read_list(ctx, &client, &subscriptions)?;
    let paid = active_paid(&plans, &subscriptions)?;
    if !paid.is_empty() && !args.replace {
        return Err(Error::replace_required(
            format!(
                "a paid Hub plan is active ({}), and subscribing again charges twice",
                subscriptions_named(&paid)
            ),
            ctx.args(),
        ));
    }
    let mark = Mark::set(ctx, &client, &settings.profile, &paid)?;
    let mut cancelled = Vec::new();
    for id in &paid {
        let request = Request::new(Method::Delete, format!("/registry/subscriptions/{id}/"))
            .about(format!("cancel the Hub subscription `{id}`"));
        if let Err(error) = ctx.send(&client, &request) {
            return Err(mark.fail(&cancelled, error, "cancel"));
        }
        cancelled.push(*id);
    }
    let request = Request::new(Method::Post, "/registry/subscribe/")
        .json(&json!({ "plan_id": plan_id }))
        .about(format!("subscribe to the Hub plan `{}`", args.plan));
    // A body that is not JSON, or none, goes through `after_cancels` too, so the cancels
    // are named.
    match ctx
        .send(&client, &request)
        .and_then(|response| response.document())
    {
        Ok(body) => {
            ctx.note(
                "the subscribe was submitted: the plan shows as active in `hipc hub subscription \
                 get` after a few minutes",
            );
            Ok(Printed::Document(body))
        }
        Err(error) => Err(mark.fail(&cancelled, error, "subscribe")),
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The double-subscribe marker one run set. A new subscription shows in the list
/// only after its block is confirmed, so a subscribe sent in the last minutes is invisible
/// to the paid-plan guard. The marker covers that window: it is checked and set in one
/// locked edit, so two subscribes at once cannot both pass, and it is keyed by API origin
/// and token, so another origin or token on the same profile name is not blocked.
struct Mark {
    path: PathBuf,
    profile: String,
    marker: HubMarker,
    /// The marker this one replaced, put back by [`Mark::fail`]: a `--replace` that
    /// passed inside another run's window and then changed nothing on the server must not
    /// lift the protection that run's purchase still needs. Of three overlapping runs, the
    /// last to fail can put back the middle one's marker rather than the first's; for the
    /// same key that one is newer and, because a marker set inside a window carries that
    /// window's cancels too, blocks at least as much, the safe side.
    previous: Option<HubMarker>,
}

impl Mark {
    /// Refuses with `replace_required` when a subscribe was sent as this origin and token,
    /// from any profile of the config file, in the last [`MARKER_SECONDS`], and otherwise
    /// records this one in `profile`. A run with another `HIPPIUS_CONFIG` reads another
    /// file, and does not see those markers. `paid` is
    /// the active paid subscriptions the run cancels first; it is always empty without
    /// `--replace`, because the paid-plan guard refuses a listed paid plan without the flag.
    ///
    /// Inside the window only a listed paid plan that the marker's run did not cancel lets
    /// the run through, which `--replace` then cancels. Such a plan is the one
    /// bought in the window, now synced into the list, or one older than it. Otherwise
    /// `--replace` has nothing new to cancel: the plan bought in the window is not in the
    /// list yet, and a row that run cancelled can still show as active (the list is as
    /// last synced), so a POST would charge twice.
    ///
    /// A marker set inside another one's window keeps that window's cancels, those of every
    /// profile's marker in the window, as well as its own, so a later run that reads an older
    /// copy of the list, one that still shows a row the earlier run cancelled, is refused too.
    /// What this cannot catch is a list that goes back in time and shows a row cancelled
    /// before the window as active: an id outside `cancels` is then taken for the plan bought
    /// in the window. The POST answer names no subscription id, so hipc cannot tell the two
    /// apart; it takes the list to move forward.
    fn set(ctx: &Context<'_>, client: &Client, profile: &str, paid: &[u64]) -> Result<Self> {
        let path = context::config_path()?;
        let at = unix_now();
        let key = client.identity();
        let (previous, marker) = ConfigFile::edit(path.clone(), |config| {
            let previous = config.hub_marker(profile).cloned();
            // Every profile's marker for this origin and token counts, not only this one's:
            // two profiles can hold the same token, and a second subscribe from the other
            // would charge twice. A marker from the future (the clock stepped back) still
            // blocks, however far: a refused repeat is the safe side.
            let in_window: Vec<(String, HubMarker)> = config
                .hub_markers()
                .filter(|(_, old)| {
                    old.key == key
                        && at
                            .checked_sub(old.at)
                            .is_none_or(|age| age < MARKER_SECONDS)
                })
                .map(|(name, old)| (name.to_owned(), old.clone()))
                .collect();
            let mut cancels: Vec<u64> = Vec::new();
            for id in in_window.iter().flat_map(|(_, old)| &old.cancels) {
                if !cancels.contains(id) {
                    cancels.push(*id);
                }
            }
            let known = cancels.len();
            for id in paid {
                if !cancels.contains(id) {
                    cancels.push(*id);
                }
            }
            // Inside the window, a run with nothing new to cancel would buy on top of the
            // plan the list does not show yet.
            if !in_window.is_empty() && cancels.len() == known {
                let sent_from = |name: &str| {
                    if name == profile {
                        "this profile".to_owned()
                    } else {
                        format!(
                            "the profile {}, which holds the same token,",
                            phrase::quote(name)
                        )
                    }
                };
                // A marker further ahead of the clock than the window is from a clock that
                // stepped back by more than a few minutes; waiting it out could take hours.
                // The marker still blocks, the safe side, and the hint names the field to
                // remove once the list shows the last subscribe.
                if let Some((owner, _)) = in_window
                    .iter()
                    .find(|(_, old)| old.at.saturating_sub(at) > MARKER_SECONDS)
                {
                    let marker = Source::Profile {
                        field: "hub_subscribe",
                        profile: owner.clone(),
                        path: path.clone(),
                    };
                    return Err(Error::replace_required(
                        format!(
                            "the Hub subscribe marker of {} is dated after the clock, so it \
                             blocks a subscribe until {} after that date; subscribing again \
                             could charge twice",
                            sent_from(owner),
                            phrase::count(MARKER_SECONDS / 60, "minute")
                        ),
                        ctx.args(),
                    )
                    .with_hint(format!(
                        "remove {marker}, once `hipc hub subscription get` lists the last \
                         subscribe"
                    )));
                }
                // This profile is named when it sent one, else the first that did.
                let owner = in_window
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .find(|name| *name == profile)
                    .or_else(|| in_window.first().map(|(name, _)| name.as_str()))
                    .unwrap_or(profile);
                // No flag gets past this: `--replace` cannot cancel what the list does not
                // show yet, so the hint waits instead of naming the flag (tenet 11).
                return Err(Error::replace_required(
                    format!(
                        "a Hub subscribe was sent from {} in the last {}, and it may not show in \
                         the list yet, so `--replace` cannot cancel it; subscribing again could \
                         charge twice",
                        sent_from(owner),
                        phrase::count(MARKER_SECONDS / 60, "minute")
                    ),
                    ctx.args(),
                )
                .with_hint("wait a few minutes, then run `hipc hub subscription get`"));
            }
            let marker = HubMarker { at, key, cancels };
            config.set_hub_marker(profile, Some(marker.clone()));
            Ok((previous, marker))
        })?;
        Ok(Self {
            path,
            profile: profile.to_owned(),
            marker,
            previous,
        })
    }

    /// The error to report once `step` fails, after the cancels in `cancelled` went
    /// through. Puts back the marker this one replaced only when nothing changed on the
    /// server: no cancel went through and the failed request had no side effects.
    /// Otherwise this run's marker stays, because it lists every row the run set out to
    /// cancel: a row that was or may have been cancelled can still show as active until
    /// the list syncs, and a marker that does not list it would let a later `--replace`
    /// pass on it and charge twice.
    fn fail(&self, cancelled: &[u64], error: Error, step: &str) -> Error {
        if cancelled.is_empty() && matches!(error.side_effects(), SideEffects::None) {
            self.put_back();
        }
        after_cancels(cancelled, error, step)
    }

    /// Puts back the marker this one replaced, if the file still holds this run's; does
    /// nothing when another run has replaced it since. Best effort: a marker left behind
    /// only refuses a repeat for a few minutes, the safe side, so a failure to write it
    /// does not hide the error being reported.
    fn put_back(&self) {
        drop(ConfigFile::edit(self.path.clone(), |config| {
            if config.hub_marker(&self.profile) == Some(&self.marker) {
                config.set_hub_marker(&self.profile, self.previous.clone());
            }
            Ok(())
        }));
    }
}

/// The error once a step fails. Before any cancel went through it is that step's own
/// error. After one did, it is a partial failure whose stdout names the cancelled
/// subscriptions. It does not say whether the subscribe went through: after a timeout or a
/// 5xx it may have, and only `hipc hub subscription get` can tell, once the chain syncs.
fn after_cancels(cancelled: &[u64], error: Error, step: &str) -> Error {
    if cancelled.is_empty() {
        return error;
    }
    let partial = Error::partial_failure(
        format!(
            "cancelled {}, then the {step} failed",
            subscriptions_named(cancelled)
        ),
        json!({ "cancelled": cancelled }),
        Value::Array(vec![failed_part("step", step, &error)]),
        SideEffects::Partial,
    );
    // A cancel went through, so this run's marker stays (`Mark::fail`), and a rerun inside
    // its window is refused even though the failed step changed nothing: the hint waits the
    // window out. A step that may have been done keeps its own hint, which says to check.
    let window = phrase::count(MARKER_SECONDS / 60, "minute");
    let hint = match (error.side_effects(), error.code()) {
        (SideEffects::Partial, _) => error.hint().unwrap_or(CHECK_FIRST_HINT).to_owned(),
        (SideEffects::None, Code::PaymentRequired) => {
            format!("top up the balance in the console, wait {window}, then run the command again")
        }
        (SideEffects::None, _) => format!("wait {window}, then run the command again"),
    };
    partial.with_hint(hint)
}

/// `the subscription `7``, or `the subscriptions `7` and `8``.
fn subscriptions_named(ids: &[u64]) -> String {
    let ids: Vec<String> = ids.iter().map(phrase::quote).collect();
    format!(
        "the {} {}",
        phrase::plural(ids.len() as u64, "subscription"),
        phrase::join(&ids, "and")
    )
}

/// The Hub plan catalog, which a subscribe reads to resolve the plan and to price the active
/// subscriptions.
const PLANS: &str = "/registry/plans/";

/// The caller's Hub subscriptions, which a subscribe reads for the paid-plan guard.
const SUBSCRIPTIONS: &str = "/registry/subscriptions/";

/// Sends `request`, a GET whose answer is a list, after the command's dry-run check and
/// confirmation.
fn read_list(ctx: &Context<'_>, client: &Client, request: &Request) -> Result<Vec<Value>> {
    hippius::page(ctx.send(client, request)?.json()?)?.into_complete(request.target())
}

/// The plan id for a numeric id or a name in any case, from the catalog.
fn resolve_plan(plans: &[Value], wanted: &str) -> Result<u64> {
    let id_of = |plan: &Value| plan.get("id").and_then(Value::as_u64);
    let found = match wanted.parse::<u64>() {
        Ok(id) => plans.iter().find(|p| id_of(p) == Some(id)),
        Err(_) => plans.iter().find(|p| {
            p.get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.eq_ignore_ascii_case(wanted))
        }),
    };
    found.and_then(id_of).ok_or_else(|| {
        let names: Vec<String> = plans
            .iter()
            .filter_map(|p| p.get("name").and_then(Value::as_str))
            .map(phrase::quote)
            .collect();
        Error::usage(
            format!(
                "no Hub plan has the name or id `{wanted}`: the plans are {}",
                phrase::join(&names, "and")
            ),
            "run `hipc hub plans` to see each plan's name and id",
        )
    })
}

/// The ids of active subscriptions to a paid plan. A plan the catalog does not price counts
/// as paid, the safe side: refusing a subscribe costs a retry, charging twice costs money.
///
/// # Errors
///
/// `internal` for a row whose `active` is not a boolean, or an active paid row whose
/// `subscription_id` is not a number. Skipping such a row would let the subscribe charge
/// twice; it cannot be cancelled by id either.
fn active_paid(plans: &[Value], subscriptions: &[Value]) -> Result<Vec<u64>> {
    let free = |plan_id: u64| {
        plans.iter().any(|p| {
            p.get("id").and_then(Value::as_u64) == Some(plan_id)
                && p.get("price_credits").and_then(Value::as_f64) == Some(0.0)
        })
    };
    let unreadable = || {
        Error::internal(
            "a Hub subscription has no readable `active` or `subscription_id`, so the check \
             for an active paid plan cannot run",
        )
        .with_hint("run `hipc hub subscription get` to see the subscriptions as the API sent them")
    };
    let mut paid = Vec::new();
    for row in subscriptions {
        let active = row
            .get("active")
            .and_then(Value::as_bool)
            .ok_or_else(unreadable)?;
        if !active || row.get("plan_id").and_then(Value::as_u64).is_some_and(free) {
            continue;
        }
        let id = row
            .get("subscription_id")
            .and_then(Value::as_u64)
            .ok_or_else(unreadable)?;
        paid.push(id);
    }
    Ok(paid)
}

fn cancel(ctx: &Context<'_>, client: &Client, subscription_id: u64) -> Result<Printed> {
    let request = Request::new(
        Method::Delete,
        format!("/registry/subscriptions/{subscription_id}/"),
    )
    .listed_by("hipc hub subscription get")
    .about(format!("cancel the Hub subscription `{subscription_id}`"));
    let done = json!({ "subscription_id": subscription_id, "status": "cancel_submitted" });
    let gate = Gate::Confirm {
        consequence: Some(CANCELLED),
    };
    print_body_or(ctx, client, &request, gate, done)
}

fn namespace(ctx: &Context<'_>, command: &HubNamespace) -> Result<Printed> {
    match command {
        HubNamespace::Availability { namespace } => {
            check_namespace(namespace)?;
            let request = Request::get("/registry/namespaces/check/")
                .query("name", namespace)
                .about(format!("check the Hub namespace `{namespace}`"));
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        // Neither prints the docker robot secret: only the call that issues it does.
        HubNamespace::Status => print_with(
            ctx,
            &ctx.load_authed_client()?,
            &Request::get("/registry/provision/status/").about("get the Hub namespace status"),
            Gate::Open,
            |r| Ok(without_field(r.document()?, "robot_secret")),
        ),
        HubNamespace::Get => print_with(
            ctx,
            &ctx.load_authed_client()?,
            &Request::get("/registry/me/").about("get the Hub namespace"),
            Gate::Open,
            |r| Ok(without_field(r.document()?, "robot_secret")),
        )
        .map_err(|error| with_provision_hint(ctx, error)),
        // `hipc` prints a `docker login` command it builds and never runs it: logging in
        // to the registry is the person's or the agent's step, with docker's own store.
        HubNamespace::Robot(HubRobot::Get) => print_with(
            ctx,
            &ctx.load_authed_client()?,
            &Request::get("/registry/me/robot/").about("get the Hub robot login"),
            Gate::Open,
            |r| robot_login(ctx, r.document()?),
        )
        .map_err(|error| with_provision_hint(ctx, error)),
        HubNamespace::Robot(HubRobot::Rotate {
            reveal,
            secret_file,
        }) => rotate_robot(ctx, *reveal, secret_file.path.as_deref())
            .map_err(|error| with_provision_hint(ctx, error)),
        HubNamespace::Create {
            namespace,
            reveal,
            secret_file,
        } => provision(ctx, namespace, *reveal, secret_file.path.as_deref()),
        HubNamespace::Update { visibility, public } => update_namespace(ctx, *visibility, *public)
            .map_err(|error| with_provision_hint(ctx, error)),
    }
}

/// `PATCH /api/registry/me/publicity/ {"public": <bool>}` (swagger
/// `registry_me_publicity_partial_update`), the answer printed as sent. Making the namespace
/// public lets anyone pull from it, so it needs `--public` beside `--yes` (tenet 5), checked
/// before the config file is read; making it private needs `--yes` alone. Either way the
/// storage allowance changes on the server (the `hippius-hub` client: "quota changes
/// server-side"), which the question says.
fn update_namespace(ctx: &Context<'_>, visibility: Visibility, public: bool) -> Result<Printed> {
    let opens = visibility == Visibility::Public;
    if opens {
        require_public(
            ctx,
            public,
            "making the Hub namespace public lets anyone pull its images and models without \
             logging in",
        )?;
    }
    let (word, who) = if opens {
        (
            "public",
            "anyone can pull its images and models without logging in",
        )
    } else {
        ("private", "only logins with access can pull from it")
    };
    let request = Request::new(Method::Patch, "/registry/me/publicity/")
        .json(&json!({ "public": opens }))
        .about(format!("make the Hub namespace {word}"));
    let consequence = format!("{who}, and its storage allowance changes");
    let gate = Gate::Confirm {
        consequence: Some(&consequence),
    };
    print_body(ctx, &ctx.load_authed_client()?, &request, gate)
}

/// `POST /api/registry/robot/rotate/` (swagger `registry_robot_rotate_create`), no body. The
/// answer holds the new robot secret once (the `hippius-hub` client reads `robot_login` and
/// `robot_secret`), so `--reveal` is required before anything is sent (tenet 9), and the
/// secret goes on stdout only.
///
/// The `docker login` handling is the robot read's: the server's command fields are dropped,
/// whatever they hold, `docker_login_command` is built by `hipc` and reads the secret from
/// stdin, and an answer with any other `docker login` command is refused whole, because a
/// command can pass the secret in a way no check can list and could reach a shell history.
/// The secret keys stay, since printing the new secret is what the command is for, unless
/// `--secret-file` names the file it goes to instead.
fn rotate_robot(ctx: &Context<'_>, reveal: bool, secret_file: Option<&Path>) -> Result<Printed> {
    require_reveal_or_file(ctx, reveal, secret_file.is_some())?;
    let request = Request::new(Method::Post, "/registry/robot/rotate/")
        .listed_by("hipc hub namespace robot get")
        .about("rotate the Hub robot secret");
    let consequence = format!(
        "every docker login that uses the old secret stops working, and {}",
        shown_once("the new secret", secret_file)
    );
    let gate = Gate::Confirm {
        consequence: Some(&consequence),
    };
    let client = ctx.load_authed_client()?;
    print_with(ctx, &client, &request, gate, |response| {
        issued_robot(
            ctx,
            response.document()?,
            Issued {
                what: "the Hub robot rotate answer",
                done: "the old secret no longer works",
                again: Some(ROTATE_ROBOT),
                secret_file,
            },
        )
    })
}

/// What [`issued_robot`] needs to know about the answer it prints.
#[derive(Clone, Copy)]
pub(crate) struct Issued<'a> {
    /// The answer, named in a refusal.
    pub(crate) what: &'a str,
    /// What has happened on the server already.
    pub(crate) done: &'a str,
    /// The words after `hipc` of the command that issues a new secret, when there is one.
    pub(crate) again: Option<&'a str>,
    /// The file `--secret-file` names, which the secret goes to instead of stdout.
    pub(crate) secret_file: Option<&'a Path>,
}

/// An answer that issues a robot or key secret as `hipc` prints it: the namespace create,
/// the robot rotate ([`rotate_robot`]), and the key create and rotate. The secret keys
/// stay, since printing the new secret is what the command is for, but the server's
/// `docker login` command fields are dropped, whatever they hold, and `docker_login_command`
/// is built by `hipc` and reads the secret from stdin. The robot's name is `robot_login`
/// (the `hippius-hub` client), `robot_name` (the provision answer), or `username` or
/// `login`, as the robot read and the key list name it; with none, `docker_login_command`
/// is null.
///
/// With `--secret-file`, the secret goes to that file and not to stdout ([`save_secret`]
/// with [`is_robot_secret`]), before either refusal below, so a refused answer loses no
/// secret: the refusal names the file. A text-mode note names it on success.
///
/// # Errors
///
/// `internal`, with nothing printed and `side_effects` `partial`, when the answer holds a
/// `docker login` command outside the keys that are dropped, since a command can pass the
/// secret in a way no check can list and could reach a shell history; or a login
/// [`docker_login_command`] refuses. The write is done already, so the hint says to issue a
/// new secret rather than to retry blindly.
fn issued_robot(ctx: &Context<'_>, body: Value, issued: Issued<'_>) -> Result<Value> {
    let Issued {
        what,
        done,
        again,
        secret_file,
    } = issued;
    let again = again
        .and_then(|words| ctx.next_command(words))
        .map(|command| format!("run `{command}` to issue a new secret, and if it is refused too, "))
        .unwrap_or_default();
    let refused = |message: String| {
        Error::internal(message)
            .with_side_effects(SideEffects::Partial)
            .with_hint(format!(
                "{again}report the refused answer at `{}`",
                hippius::ISSUES_URL
            ))
    };
    let body = without_keys(body, &|key| {
        LOGIN_COMMAND_KEYS.contains(&key.to_ascii_lowercase().as_str())
    });
    // The secret goes to its file before any refusal below, which prints nothing: with
    // `--secret-file` the refused answer then loses no secret.
    let (mut body, saved) = match secret_file {
        Some(path) => {
            let (body, written) = save_secret(path, body, &is_robot_secret)?;
            (body, written.then_some(path))
        }
        None => (body, None),
    };
    let kept = saved
        .map(|path| format!("; the secret is in {}", phrase::quote(path.display())))
        .unwrap_or_default();
    if holds_a_login_command(&body) {
        return Err(refused(format!(
            "{what} holds a `docker login` command, which may pass the secret where it could be \
             copied into a shell history, so `hipc` printed none of it; {done}{kept}"
        )));
    }
    let login = ["robot_login", "robot_name", "username", "login"]
        .iter()
        .find_map(|key| body.get(key)?.as_str())
        .map(docker_login_command)
        .transpose()
        .map_err(|error| refused(format!("{}; {done}{kept}", error.message())))?;
    if let Value::Object(fields) = &mut body {
        fields.insert("docker_login_command".to_owned(), login.into());
    }
    if let Some(path) = saved {
        ctx.note(&format!(
            "the secret is in {}, and not printed: give that file on stdin to the \
             document's `docker_login_command`, and delete it once the secret is stored where it \
             is used",
            phrase::quote(path.display())
        ));
    }
    Ok(body)
}

/// The keys the console reads a robot secret from, because Harbor and the Hippius backend
/// have named it differently over time.
///
/// `namespace get` and `status` remove only `robot_secret`, the one name the provision
/// answer uses. Removing the other three there would drop fields those documents print
/// today, which tenet 7 allows only in a new major version.
const ROBOT_SECRET_KEYS: [&str; 4] = ["secret", "password", "robot_secret", "token"];

/// The keys a server `docker login` command may sit under: the robot read's `docker_login`,
/// the `hippius-hub` client's `docker_login_cmd`, and two generic names. Each is dropped whatever
/// its value holds, even an encoding no check can read, such as base64.
const LOGIN_COMMAND_KEYS: [&str; 4] = ["docker_login", "docker_login_cmd", "login_cmd", "command"];

/// `value` as the robot read and the key list and delete of `hub keys` print it, since a key
/// is a robot too, or `None` when it must not be printed at all.
///
/// Dropped at any depth: a key that is one of the [`ROBOT_SECRET_KEYS`] in any case, or
/// holds `password` in any case (`DOCKER_PASSWORD`), and a key in [`LOGIN_COMMAND_KEYS`].
/// What is left must not hold a `docker login` command anywhere ([`holds_a_login_command`]):
/// a command can pass the secret in more ways than a check can list (`-p`, `--password=`,
/// `echo ... |`, a `DOCKER_PASSWORD=` prefix, an argv array, or a form not yet seen), and a
/// secret cannot be cut out of one reliably, so such an answer is refused whole and nothing
/// is printed: this fails closed.
fn without_robot_secrets(value: Value) -> Option<Value> {
    let value = without_secret_keys(value);
    (!holds_a_login_command(&value)).then_some(value)
}

fn without_secret_keys(value: Value) -> Value {
    without_keys(value, &is_robot_secret)
}

/// Whether `key` may hold a robot or key secret, or a `docker login` command: one of the
/// [`ROBOT_SECRET_KEYS`] or [`LOGIN_COMMAND_KEYS`], or a key with `password` in it, in any case.
fn is_robot_secret(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    ROBOT_SECRET_KEYS.contains(&key.as_str())
        || LOGIN_COMMAND_KEYS.contains(&key.as_str())
        || key.contains("password")
}

/// `value` without every key, at any depth, that `withheld` holds.
fn without_keys(value: Value, withheld: &dyn Fn(&str) -> bool) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .filter(|(key, _)| !withheld(key))
                .map(|(key, value)| (key, without_keys(value, withheld)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| without_keys(item, withheld))
                .collect(),
        ),
        other => other,
    }
}

/// Whether any string or object key in `value`, or the strings of any array joined by
/// spaces (an argv array), holds `docker login` ([`holds_docker_login`]), at any depth.
fn holds_a_login_command(value: &Value) -> bool {
    match value {
        Value::String(text) => holds_docker_login(text),
        Value::Array(items) => {
            let words: Vec<&str> = items.iter().filter_map(Value::as_str).collect();
            holds_docker_login(&words.join(" ")) || items.iter().any(holds_a_login_command)
        }
        Value::Object(fields) => fields
            .iter()
            .any(|(key, value)| holds_docker_login(key) || holds_a_login_command(value)),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

/// The robot read's answer as `hipc` prints it ([`without_robot_secrets`]), with the
/// `docker_login_command` `hipc` builds. Swagger (`registry_me_robot_list`) says the answer
/// has no secret; this keeps one out of stdout if it ever carries one, because only the
/// call that issues the secret may print it (tenet 9).
///
/// The robot's name is `username`, as the console reads it, or `login`, as the
/// `hippius-hub` client's tests of this route answer it; with neither,
/// `docker_login_command` is null.
///
/// # Errors
///
/// `internal`, with nothing printed, when the answer holds a `docker login` command outside
/// the keys that are dropped.
fn robot_login(ctx: &Context<'_>, body: Value) -> Result<Value> {
    let Some(mut body) = without_robot_secrets(body) else {
        let rotate = ctx
            .next_command(ROTATE_ROBOT)
            .map(|rotate| format!("; `{rotate}` issues a new one"))
            .unwrap_or_default();
        return Err(Error::internal(
            "the Hub robot login answer holds a `docker login` command, which may pass the \
             secret, and `hipc` does not print a robot secret",
        )
        .with_hint(format!(
            "log in with `docker login --password-stdin`, which reads the secret from \
             stdin{rotate}"
        )));
    };
    let login = ["username", "login"]
        .iter()
        .find_map(|key| body.get(key)?.as_str())
        .map(docker_login_command)
        .transpose()?;
    if let Value::Object(fields) = &mut body {
        fields.insert("docker_login_command".to_owned(), login.into());
    }
    Ok(body)
}

/// Whether `text` holds the words `docker login`, in any case and with any whitespace
/// between them, as a shell would run them. The zero-width spaces and joiners (U+200B,
/// U+200C, U+200D) and the byte order mark (U+FEFF) count as spaces: Rust does not call them
/// whitespace, and a terminal shows nothing for them, so `docker\u{200B}login` reads as the
/// command. Quotes, backquotes, and backslashes are dropped first, as a shell drops them
/// from a word (`"docker" "login"`, `docke\r login`, a line ended by a backslash), and a word
/// that ends in `docker` followed later by the word `login` counts too, which a flag between
/// them (`docker -D login`) or a substitution (`$(printf docker) login`) would not hide.
/// This fails closed: a sentence that names both words is refused as well.
fn holds_docker_login(text: &str) -> bool {
    let spaced: String = text
        .chars()
        .filter(|c| !matches!(c, '"' | '\'' | '`' | '\\'))
        .map(|c| match c {
            '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' => ' ',
            c => c,
        })
        .collect::<String>()
        .to_ascii_lowercase();
    let words: Vec<&str> = spaced.split_whitespace().collect();
    let bare = |word: &str| {
        word.trim_matches(|c: char| !c.is_ascii_alphanumeric())
            .to_owned()
    };
    words.join(" ").contains("docker login")
        || words
            .iter()
            .position(|word| bare(word).ends_with("docker"))
            .is_some_and(|at| words.iter().skip(at + 1).any(|word| bare(word) == "login"))
}

/// The `docker login` command for the robot or key `login`, built by `hipc` from the registry
/// host it knows: it reads the secret from stdin, so it never holds one. The login is
/// single-quoted when a shell would change it, as Harbor's `robot$...` would be.
///
/// # Errors
///
/// `internal` for a login that starts with `-`: quoting does not stop `docker` reading it
/// as a flag, so the command would not log in as the robot it names.
fn docker_login_command(login: &str) -> Result<String> {
    if login.starts_with('-') {
        // The login is not quoted: a value that reads as a flag may be one, such as
        // `--password=...`.
        return Err(Error::internal(
            "the API answered a login that starts with `-`, which `docker login -u` would read \
             as a flag, so `hipc` printed none of the answer",
        ));
    }
    Ok(format!(
        "docker login {REGISTRY_HOST} -u {} --password-stdin",
        phrase::shell_word(login)
    ))
}
/// `error` with a hint when it is the API's answer to an account that has no Hub namespace yet
/// ([`set_up_first`]: `You must call /api/registry/provision/ first.`). The hint carries this run's
/// profile and origin, so the namespace it creates is on the server this run read.
fn with_provision_hint(ctx: &Context<'_>, error: Error) -> Error {
    if set_up_first(&error, "/api/registry/provision/") {
        let hint = run_hint(
            ctx,
            "hub namespace create <namespace> --reveal --yes",
            "to create the account's Hub namespace",
        );
        error.with_hint(hint)
    } else {
        error
    }
}

/// The words after `hipc` of the command that issues a new docker robot secret.
const ROTATE_ROBOT: &str = "hub namespace robot rotate --reveal --yes";

/// `POST /api/registry/provision/ {"namespace"}`. A fresh namespace returns the docker robot
/// secret once, so `--reveal` is required before anything is sent (tenet 9).
fn provision(
    ctx: &Context<'_>,
    name: &str,
    reveal: bool,
    secret_file: Option<&Path>,
) -> Result<Printed> {
    check_namespace(name)?;
    require_reveal_or_file(ctx, reveal, secret_file.is_some())?;
    let request = Request::new(Method::Post, "/registry/provision/")
        .json(&json!({ "namespace": name }))
        .about(format!("create the Hub namespace `{name}`"));
    // Without a file the question says nothing of the secret, as it always has.
    let consequence = secret_file.map(|_| shown_once("its robot secret", secret_file));
    let gate = Gate::Confirm {
        consequence: consequence.as_deref(),
    };
    let client = ctx.load_authed_client()?;
    print_with(ctx, &client, &request, gate, |response| {
        let body = issued_robot(
            ctx,
            response.document()?,
            Issued {
                what: "the Hub namespace create answer",
                done: "the create was sent",
                again: Some(ROTATE_ROBOT),
                secret_file,
            },
        )?;
        if body
            .get("robot_secret_file")
            .and_then(Value::as_str)
            .is_some()
        {
            // `issued_robot` named the file in its note.
        } else if body.get("robot_secret").and_then(Value::as_str).is_none() {
            let why = "no `robot_secret` was issued, because the namespace already \
                       exists or is pending";
            match ctx.next_command(ROTATE_ROBOT) {
                Some(rotate) => ctx.note(&format!("{why}; `{rotate}` issues one")),
                None => ctx.note(why),
            }
        } else if body.get("robot_name").and_then(Value::as_str).is_some() {
            // The robot name is not repeated: Harbor names a robot `robot$...`, and a shell
            // expands the `$`, so the person copies it from the output. `hipc` never runs the
            // login itself.
            ctx.note(&format!(
                "log in to the registry with `docker login {REGISTRY_HOST} --username \
                 <robot_name> --password-stdin`, giving it `robot_secret` on stdin"
            ));
        }
        Ok(body)
    })
    .map_err(|error| match error.code() {
        Code::PaymentRequired => error.with_hint(
            "top up the balance to at least $10 in the console, which a namespace needs even \
             on the Free plan, then run the command again",
        ),
        _ => error,
    })
}

/// Namespace names are 1 to 63 lowercase letters, digits, and dashes, and do not start with
/// a dash, which a command line would read as a flag.
fn check_namespace(name: &str) -> Result<()> {
    let valid = (1..=63).contains(&name.len())
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if valid {
        Ok(())
    } else {
        Err(Error::usage(
            format!(
                "`{name}` is not a namespace name: a name is 1 to 63 lowercase letters, \
                 digits, and `-`, and does not start with `-`"
            ),
            "pass a namespace name such as `my-team`",
        ))
    }
}

/// What a Hub subscription cancel leaves, for its question and `hipc api`'s. The grace period
/// and the deletion after it are the ones the Hub billing docs state.
pub(crate) const CANCELLED: &str = "the namespace stays available for a 30-day grace period, \
                                    and subscribing again within it keeps everything; when it \
                                    ends, the namespace and everything in it are deleted for good";
