//! `hipc compute runners`: GitHub Actions runners.
//!
//! Every route here is under `/api/runners/`, which is account-scoped, so a membership
//! sends `X-Hippius-Account`; `/api/runners/agent/` is personal and not wrapped. Swagger
//! types none of the bodies: the shapes are the console's. No answer holds a secret, so nothing
//! needs `--reveal`: `connect`'s `state` only proves that this account started the flow, and the
//! link call that redeems it also needs the caller's token and the GitHub OAuth `code` (swagger,
//! `runners_github_link_create`).
//!
//! `limit` is the monthly spend cap on runners. Swagger says the route answers 404 while the
//! server-wide switch for runner usage limits is off, so a 404 there says the limit is not
//! enabled on the server rather than that something is missing. Swagger types neither its body
//! nor `usage`'s. The shape assumed is the CDN zone cap's: credits sent in `monthly_cap`, and
//! read back from `monthly_cap_credits`, `monthly_cap` holding base units. Both names are
//! unverified against the live API. So `limit update` fails closed: whatever it cannot tell is
//! not a raise (a `null` cap, or no amount in `monthly_cap_credits`) is asked about, and an
//! answer to the `PUT` that does not show the cap sent is an error, not a success. That answer
//! is compared exactly, as decimal digits, with the amount sent, which is why `--monthly-cap`
//! takes at most 18 digits after the point, a credit's smallest part: a finer amount could be
//! rounded by the server and never compare equal.
//!
//! Nothing here waits. `connect` returns the install URL and does not wait for the link,
//! and `jobs` reads once: operations that finish later are read again by the caller, not
//! polled.

use std::cmp::Ordering;

use hippius::routes::runners;
use hippius::{Client, Error, ISSUES_URL, Request, Response, Result, phrase};
use serde_json::{Map, Value, json};

use crate::cli::{
    MonthlyCap, RunnerInstallations, RunnerLimit, RunnerSettings, RunnerSettingsUpdate, RunnerSize,
    Runners,
};
use crate::commands::credits::{amount, compare};
use crate::commands::{Printed, list, print_body, print_body_or, print_envelope, print_with};
use crate::context::{Context, Gate, Write};

pub(super) fn run(ctx: &Context<'_>, command: &Runners) -> Result<Printed> {
    match command {
        Runners::Settings(RunnerSettings::Get) => {
            let request = Request::to(&runners::SETTINGS, []).about("get the runner settings");
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        Runners::Settings(RunnerSettings::Update(args)) => update(ctx, args),
        // The API answers a bare array.
        Runners::Installations(RunnerInstallations::List) => {
            let request =
                Request::to(&runners::INSTALLATIONS, []).about("list the GitHub App installations");
            print_envelope(ctx, &ctx.load_authed_client()?, &request)
        }
        Runners::Installations(RunnerInstallations::Delete { installation_id }) => {
            let request = Request::to(&runners::INSTALLATION_DELETE, [installation_id])
                .listed_by("hipc compute runners installations list")
                .about(format!(
                    "unlink the GitHub App installation `{installation_id}` from the account"
                ));
            let gate = Gate::Confirm {
                consequence: Some(UNLINKED),
            };
            let done = json!({ "id": installation_id, "deleted": true });
            print_body_or(ctx, &ctx.load_authed_client()?, &request, gate, done)
        }
        Runners::Jobs(args) => {
            let request = Request::to(&runners::JOBS, []).about("list the runner jobs");
            list::run_cursor(ctx, &ctx.load_authed_client()?, &request, args)
        }
        Runners::Connect => connect(ctx, &ctx.load_authed_client()?),
        Runners::Usage => {
            let request = Request::to(&runners::USAGE, []).about("get the runner spend");
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        Runners::Limit(RunnerLimit::Get) => {
            let client = ctx.load_authed_client()?;
            print_body(ctx, &client, &read_limit(), Gate::Open).map_err(not_enabled)
        }
        Runners::Limit(RunnerLimit::Update { monthly_cap }) => update_limit(ctx, monthly_cap),
    }
}

fn read_limit() -> Request {
    Request::to(&runners::LIMIT, []).about("get the runner spend limit")
}

/// A 404 on the limit is the server-wide switch being off, not a missing resource the
/// caller named.
fn not_enabled(error: Error) -> Error {
    if error.http_status() == Some(404) {
        error.with_hint(
            "the runner spend limit is not enabled on this server; run `hipc compute runners \
             settings get`, then lower `--max-concurrent` or `--allowed-size` with `hipc compute \
             runners settings update`",
        )
    } else {
        error
    }
}

/// `PUT /api/runners/limit/` with `monthly_cap` in credits. A change that may allow more spend
/// is confirmed (tenet 5): a higher cap, none in place of a cap, or any change from a cap hipc
/// cannot read as an amount of credits. Which one it is needs the cap in force, so the limit is
/// read after the write's own refusals and the token check, and before the question; the dry
/// run lists that read, then the write. The amount sent is the parser's normal form.
fn update_limit(ctx: &Context<'_>, cap: &MonthlyCap) -> Result<Printed> {
    let client = ctx.load_authed_client()?;
    let (sent, shown) = match cap {
        MonthlyCap::Credits(credits) => (
            Value::from(credits.as_str()),
            format!("`{credits}` credits"),
        ),
        MonthlyCap::None => (Value::Null, "none".to_owned()),
    };
    // Credits go in `monthly_cap`, as `cdn cap update` sends them, although the CDN cap reads
    // back base units there and credits in `monthly_cap_credits`. Both names are assumptions
    // for this route until a live run checks them (swagger types no body).
    let put = || Request::to(&runners::LIMIT_UPDATE, []).json(&json!({ "monthly_cap": sent }));
    let draft = put().about(format!("set the monthly runner spend cap to {shown}"));
    let checked = match ctx.check_write(&client, &draft)? {
        Write::DryRun(mut doc) => {
            doc["requests"] = ctx.describe_each(&client, &[&read_limit(), &draft])?;
            return Ok(Printed::Document(doc));
        }
        Write::Checked(checked) => checked,
    };
    let limit = ctx
        .send(&client, &read_limit())
        .and_then(|response| response.document())
        .map_err(not_enabled)?;
    let more = "runners may then spend more of the balance each month";
    let unknown = "if the cap in force is higher than it shows, runners may then spend more of \
                   the balance each month";
    let set_unknown = |new: &str| {
        format!(
            "set the monthly runner spend cap to `{new}` credits, where the current cap is \
             unknown: the limit has no amount in `monthly_cap_credits`"
        )
    };
    let not_an_amount = |old: &str| {
        format!(
            "where `monthly_cap_credits` is {}, which is not an amount of credits",
            phrase::quote(old)
        )
    };
    // Whether `null` means no cap, rather than a server default, is not confirmed against the
    // live API, so a cap set over `null` is asked about. Removing a cap that already reads
    // `null` sends the same `null` back, so it asks nothing.
    let question = match (cap, current_cap(&limit)) {
        (MonthlyCap::None, Current::Null) => None,
        (MonthlyCap::None, Current::Unknown) => Some((
            "remove the monthly runner spend cap, whose current value is unknown: the limit \
             has no amount in `monthly_cap_credits`"
                .to_owned(),
            unknown,
        )),
        (MonthlyCap::None, Current::Credits(old)) => Some(if amount(&old).is_some() {
            (
                format!(
                    "remove the monthly runner spend cap of {} credits",
                    phrase::quote(&old)
                ),
                more,
            )
        } else {
            (
                format!(
                    "remove the monthly runner spend cap, {}",
                    not_an_amount(&old)
                ),
                unknown,
            )
        }),
        (MonthlyCap::Credits(new), Current::Null) => Some((
            format!(
                "set the monthly runner spend cap to `{new}` credits, where the limit shows \
                 `null`, unset or a server default"
            ),
            unknown,
        )),
        (MonthlyCap::Credits(new), Current::Unknown) => Some((set_unknown(new), unknown)),
        (MonthlyCap::Credits(new), Current::Credits(old)) => match compare(new, &old) {
            Some(Ordering::Greater) => Some((
                format!(
                    "raise the monthly runner spend cap from {} to `{new}` credits",
                    phrase::quote(&old)
                ),
                more,
            )),
            Some(Ordering::Less | Ordering::Equal) => None,
            None => Some((
                format!(
                    "set the monthly runner spend cap to `{new}` credits, {}",
                    not_an_amount(&old)
                ),
                unknown,
            )),
        },
    };
    let request = match question {
        Some((action, consequence)) => {
            let request = put().about(action);
            ctx.confirm_write(checked, &request.action(), Some(consequence))?;
            request
        }
        None => draft,
    };
    // Only the read's 404 is the switch being off: a 404 after a good read is plain.
    let response = ctx.send(&client, &request)?;
    applied(&response, cap).map(Printed::Document)
}

/// The answer to the `PUT`, checked. `monthly_cap` is an unverified name for this route, and a
/// server that ignores an unknown field answers 200 with the cap unchanged: a cut that did not
/// happen would then exit 0 while runners keep spending. Only an answer whose
/// `monthly_cap_credits` is the amount sent, compared as [`compare`] does, or that reads
/// [`Current::Null`] after a removal, is the change made. Any other 2xx is `internal` with
/// `partial` side effects, and nothing goes to stdout, as for a peer rename: the `PUT` was
/// sent, and may or may not have changed the cap. The hint reads the limit rather than
/// sending the `PUT` again, whose effect is unknown.
fn applied(response: &Response, cap: &MonthlyCap) -> Result<Value> {
    let answer = response.document()?;
    let current = current_cap(&answer);
    let done = match (cap, &current) {
        (MonthlyCap::None, Current::Null) => true,
        (MonthlyCap::Credits(new), Current::Credits(now)) => {
            compare(new, now) == Some(Ordering::Equal)
        }
        _ => false,
    };
    if done {
        return Ok(answer);
    }
    let why = match current {
        Current::Null => "the API answered a cap of `null`".to_owned(),
        Current::Unknown => {
            "the API answered without an amount in `monthly_cap_credits`".to_owned()
        }
        Current::Credits(now) => format!(
            "the API answered `monthly_cap_credits` {}",
            phrase::quote(&now)
        ),
    };
    // Not the default `internal` hint, which says to run the command again: that would send
    // a `PUT` whose effect is unknown, so the hint reads the limit instead (tenet 15).
    Err(response.refused(&why).with_hint(format!(
        "run `hipc compute runners limit get` to see the cap in force, and report what it \
         prints at `{ISSUES_URL}`"
    )))
}

/// What the limit says of the cap in force.
enum Current {
    /// `monthly_cap` and `monthly_cap_credits` are both present and `null`: no cap, or a
    /// server default; which is unconfirmed.
    Null,
    /// Neither a cap in credits nor `null`: no `monthly_cap_credits`, or no `monthly_cap`, or
    /// an answer that is not an object. A missing field is not read as `null`: a server that
    /// leaves the field out says nothing of the cap.
    Unknown,
    /// `monthly_cap_credits`, a string or a number, which [`compare`]
    /// reads as an amount or not.
    Credits(String),
}

/// The cap in force, from `monthly_cap_credits` only. If this route follows the CDN cap,
/// `monthly_cap` holds base units (`20000000000000000000` for 20 credits), and comparing
/// credits against that would make every raise look like a cut. A JSON number is read as
/// serde writes it, so one in exponent form, such as `1e+300`, is not a decimal amount and the
/// cap counts as unknown on purpose: a cap hipc cannot compare exactly is never judged a lower
/// one.
fn current_cap(limit: &Value) -> Current {
    let credits = limit.get("monthly_cap_credits");
    match credits {
        Some(Value::String(text)) => return Current::Credits(text.clone()),
        Some(Value::Number(number)) => return Current::Credits(number.to_string()),
        _ => {}
    }
    let null = |field: &str| limit.get(field).is_some_and(Value::is_null);
    if null("monthly_cap_credits") && null("monthly_cap") {
        Current::Null
    } else {
        Current::Unknown
    }
}

/// `PUT /api/runners/settings/` with only the keys given: the server keeps the keys not sent.
/// The settings in force are read first, after the write's own refusals and the token check,
/// and the question is asked only when [`may_spend_more`]; the dry run lists the read, then
/// the write.
fn update(ctx: &Context<'_>, args: &RunnerSettingsUpdate) -> Result<Printed> {
    let mut body = Map::new();
    let mut changes = Vec::new();
    let mut set = |key: &str, value: Value, shown: String| {
        changes.push(format!("`{key}` to {shown}"));
        body.insert(key.to_owned(), value);
    };
    if let Some(enabled) = args.enabled {
        set("enabled", enabled.into(), format!("`{enabled}`"));
    }
    if let Some(max) = args.max_concurrent {
        set("max_concurrent", max.into(), format!("`{max}`"));
    }
    if !args.allowed_sizes.is_empty() {
        let sizes = allowed_sizes(&args.allowed_sizes)?;
        let shown: Vec<String> = sizes.iter().map(|size| format!("`{size}`")).collect();
        set("allowed_sizes", sizes.into(), phrase::join(&shown, "and"));
    }
    if let Some(region) = &args.default_region {
        let shown = if region.is_empty() {
            "anywhere (`\"\"`)".to_owned()
        } else {
            format!("`{region}`")
        };
        set("default_region", region.as_str().into(), shown);
    }
    if let Some(public) = args.allow_public_repos {
        set("allow_public_repos", public.into(), format!("`{public}`"));
    }
    let mut action = format!("set the runner settings {}", phrase::join(&changes, "and"));
    // Who can spend the balance is part of the action, so an agent's refusal names it too.
    if args.allow_public_repos == Some(true) {
        action.push_str(
            ", which lets a pull request from anyone start a job on a public repository, \
             billed to the balance",
        );
    }
    let consequence = if args.enabled == Some(true) {
        "while runners are enabled, every job runs on a VM billed to the balance"
    } else {
        "runners may then spend more of the balance"
    };
    let request = Request::to(&runners::SETTINGS_UPDATE, [])
        .json(&Value::Object(body))
        .about(action);
    let read = Request::to(&runners::SETTINGS, []).about("get the runner settings");
    let client = ctx.load_authed_client()?;
    let checked = match ctx.check_write(&client, &request)? {
        Write::DryRun(mut doc) => {
            doc["requests"] = ctx.describe_each(&client, &[&read, &request])?;
            return Ok(Printed::Document(doc));
        }
        Write::Checked(checked) => checked,
    };
    let current = ctx.send(&client, &read)?.document()?;
    if may_spend_more(args, &current) {
        ctx.confirm_write(checked, &request.action(), Some(consequence))?;
    }
    ctx.send(&client, &request)?
        .document()
        .map(Printed::Document)
}

/// Whether the settings `args` sets may let runners spend more than the settings in force,
/// `current`, do, so the update is confirmed (tenet 5), as every spend cap is. Turning runners
/// on, a higher `max_concurrent`, a `max_concurrent` of `0` where the setting is not `0`, a
/// size the allowed list lacks, and opening public repositories may spend more; each value it
/// cannot compare counts as one. The server does not document what `0` means, and it may mean
/// no cap, so it is confirmed as a raise is, by the owner's call. A pure cut (turning runners
/// off, a lower or equal non-zero `max_concurrent`, fewer sizes, closing public repositories),
/// a repeat of the settings in force, and a new default region are sent unasked: the region
/// moves where a job runs, not how many run or on what size.
fn may_spend_more(args: &RunnerSettingsUpdate, current: &Value) -> bool {
    let turns_on = |new: Option<bool>, key: &str| {
        new == Some(true) && current.get(key) != Some(&Value::Bool(true))
    };
    let more_at_once = args.max_concurrent.is_some_and(|new| {
        let old = current.get("max_concurrent").and_then(Value::as_u64);
        match new {
            0 => old != Some(0),
            new => old.is_none_or(|old| u64::from(new) > old),
        }
    });
    let allowed: Option<Vec<&str>> = current
        .get("allowed_sizes")
        .and_then(Value::as_array)
        .and_then(|sizes| sizes.iter().map(Value::as_str).collect());
    let new_size = args.allowed_sizes.iter().any(|size| {
        allowed
            .as_ref()
            .is_none_or(|old| !old.contains(&size.code()))
    });
    turns_on(args.enabled, "enabled")
        || turns_on(args.allow_public_repos, "allow_public_repos")
        || more_at_once
        || new_size
}

/// The `--allowed-size` codes, in the order given.
///
/// # Errors
///
/// `usage` for a size given twice, which would send a list with a repeat for the server to
/// judge.
fn allowed_sizes(given: &[RunnerSize]) -> Result<Vec<&'static str>> {
    let mut sizes = Vec::new();
    for size in given {
        let code = size.code();
        if sizes.contains(&code) {
            return Err(Error::usage(
                format!("`--allowed-size` names `{code}` more than once"),
                "pass each size once",
            ));
        }
        sizes.push(code);
    }
    Ok(sizes)
}

/// `POST /api/runners/github/connect/` with no body. The answer is printed as sent, and
/// text mode says what is left to do: the link needs the browser and the console.
fn connect(ctx: &Context<'_>, client: &Client) -> Result<Printed> {
    let request = Request::to(&runners::CONNECT, []).about("start the GitHub App installation");
    print_with(ctx, client, &request, Gate::Open, |response| {
        let started = response.document()?;
        // The URL is not repeated here: it is on stdout, and a quoted URL would have its
        // query values, the signed state among them, redacted (tenet 15).
        ctx.note(
            "open the `url` in the output to install the Hippius Runners GitHub App; nothing \
             is linked until the App is installed and the console finishes the link",
        );
        Ok(started)
    })
}

/// What a GitHub App installation unlink ends, for its question and `hipc api`'s.
pub(crate) const UNLINKED: &str = "its repositories stop getting Hippius runners";
