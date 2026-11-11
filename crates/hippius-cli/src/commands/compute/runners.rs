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
use hippius::{
    Client, Error, ISSUES_URL, Request, Response, Result, Slot, TextOrNumber, phrase, view_of,
};
use serde_json::{Value, json};

use crate::cli::{
    MonthlyCap, RunnerInstallations, RunnerLimit, RunnerSettings, RunnerSettingsUpdate, Runners,
};
use crate::commands::spend_cap::{self, InForce, NewCap};
use crate::commands::{
    Decided, Printed, list, print_body, print_body_or, print_envelope, print_with, read_then_write,
};
use crate::context::{Context, Gate, WritePlan};
use crate::units::compare;

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
            list::run(ctx, &ctx.load_authed_client()?, &request, args)
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
        MonthlyCap::Credits(credits) => (Some(credits.as_str()), format!("`{credits}` credits")),
        MonthlyCap::None => (None, "none".to_owned()),
    };
    let put = || {
        Request::to(&runners::LIMIT_UPDATE, [])
            .json(&runners::LimitUpdateBody { monthly_cap: sent })
    };
    let draft = put().about(format!("set the monthly runner spend cap to {shown}"));
    let read = read_limit();
    let decide = || {
        let limit = ctx
            .send(&client, &read)
            .and_then(|response| response.document())
            .map_err(not_enabled)?;
        Ok(match limit_question(cap, &limit) {
            Some((action, consequence)) => Decided::asked(put().about(action), Some(&consequence)),
            None => Decided::unasked(draft.clone()),
        })
    };
    // Only the read's 404 is the switch being off: a 404 after a good read is plain.
    let render = |sent: Result<Response>| applied(&sent?, cap);
    read_then_write(
        ctx,
        &client,
        &WritePlan::new(&[&read], &draft),
        decide,
        render,
    )
}

/// The action and the consequence of the question an update to `cap` asks over the limit
/// read as `limit`, or `None` when it is sent unasked ([`spend_cap::question`]).
fn limit_question(cap: &MonthlyCap, limit: &Value) -> Option<(String, String)> {
    let new = match cap {
        MonthlyCap::Credits(credits) => NewCap::Credits(credits),
        MonthlyCap::None => NewCap::None,
    };
    let in_force = current_cap(limit);
    let order = match (new, &in_force) {
        (NewCap::Credits(new), InForce::Credits(old)) => compare(new, old),
        _ => None,
    };
    let runners_cap = spend_cap::Cap {
        named: "the monthly runner spend cap",
        read: "the limit",
        spender: "runners",
    };
    spend_cap::question(runners_cap, new, &in_force, order)
}

/// The answer to the `PUT`, checked. `monthly_cap` is an unverified name for this route, and a
/// server that ignores an unknown field answers 200 with the cap unchanged: a cut that did not
/// happen would then exit 0 while runners keep spending. Only an answer whose
/// `monthly_cap_credits` is the amount sent, compared as [`compare`] does, or that reads
/// [`InForce::Null`] after a removal, is the change made. Any other 2xx is `internal` with
/// `partial` side effects, and nothing goes to stdout, as for a peer rename: the `PUT` was
/// sent, and may or may not have changed the cap. The hint reads the limit, not the default
/// `internal` hint to run the command again, which would send a `PUT` whose effect is unknown.
fn applied(response: &Response, cap: &MonthlyCap) -> Result<Value> {
    let answer = response.document()?;
    let current = current_cap(&answer);
    let done = match (cap, &current) {
        (MonthlyCap::None, InForce::Null) => true,
        (MonthlyCap::Credits(new), InForce::Credits(now)) => {
            compare(new, now) == Some(Ordering::Equal)
        }
        _ => false,
    };
    if done {
        return Ok(answer);
    }
    let why = match current {
        InForce::Null => "the API answered a cap of `null`".to_owned(),
        InForce::Unknown => {
            "the API answered without an amount in `monthly_cap_credits`".to_owned()
        }
        InForce::Credits(now) => format!(
            "the API answered `monthly_cap_credits` {}",
            phrase::quote(&now)
        ),
    };
    Err(response.refused(&why).with_hint(format!(
        "run `hipc compute runners limit get` to see the cap in force, and report what it \
         prints at `{ISSUES_URL}`"
    )))
}

/// The cap in force, from `monthly_cap_credits` only: `null` only when `monthly_cap` is `null` too,
/// and unknown when a field is missing, since a server that leaves the field out says nothing of
/// the cap. If this route follows the CDN cap, `monthly_cap` holds base units
/// (`20000000000000000000` for 20 credits), and comparing credits against that would make every
/// raise look like a cut. A JSON number is read as serde writes it, so one in exponent form, such
/// as `1e+300`, is not a decimal amount and the cap counts as unknown on purpose: a cap hipc cannot
/// compare exactly is never judged a lower one.
fn current_cap(limit: &Value) -> InForce {
    let limit: runners::Limit = view_of(limit);
    match limit.monthly_cap_credits {
        Slot::Value(TextOrNumber(text)) => InForce::Credits(text),
        Slot::Null if limit.monthly_cap == Slot::Null => InForce::Null,
        Slot::Missing | Slot::Null | Slot::Other => InForce::Unknown,
    }
}

/// `PUT /api/runners/settings/` with only the keys given: the server keeps the keys not sent.
/// The settings in force are read first, after the write's own refusals and the token check,
/// and the question is asked only when [`may_spend_more`]; the dry run lists the read, then
/// the write.
fn update(ctx: &Context<'_>, args: &RunnerSettingsUpdate) -> Result<Printed> {
    let sizes = if args.allowed_sizes.is_empty() {
        None
    } else {
        Some(allowed_sizes(&args.allowed_sizes)?)
    };
    let body = runners::SettingsUpdateBody {
        enabled: args.enabled,
        max_concurrent: args.max_concurrent,
        allowed_sizes: sizes.as_deref(),
        default_region: args.default_region.as_deref(),
        allow_public_repos: args.allow_public_repos,
    };
    let mut changes = Vec::new();
    if let Some(enabled) = body.enabled {
        changes.push(format!("`enabled` to `{enabled}`"));
    }
    if let Some(max) = body.max_concurrent {
        changes.push(format!("`max_concurrent` to `{max}`"));
    }
    if let Some(sizes) = body.allowed_sizes {
        let shown: Vec<String> = sizes.iter().map(|size| format!("`{size}`")).collect();
        changes.push(format!(
            "`allowed_sizes` to {}",
            phrase::join(&shown, "and")
        ));
    }
    if let Some(region) = body.default_region {
        let shown = if region.is_empty() {
            "anywhere (`\"\"`)".to_owned()
        } else {
            format!("`{region}`")
        };
        changes.push(format!("`default_region` to {shown}"));
    }
    if let Some(public) = body.allow_public_repos {
        changes.push(format!("`allow_public_repos` to `{public}`"));
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
        .json(&body)
        .about(action);
    let read = Request::to(&runners::SETTINGS, []).about("get the runner settings");
    let client = ctx.load_authed_client()?;
    let decide = || {
        let current = view_of(&ctx.send(&client, &read)?.document()?);
        Ok(if may_spend_more(args, &current) {
            Decided::asked(request.clone(), Some(consequence))
        } else {
            Decided::unasked(request.clone())
        })
    };
    let plan = WritePlan::new(&[&read], &request);
    read_then_write(ctx, &client, &plan, decide, |sent| sent?.document())
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
fn may_spend_more(args: &RunnerSettingsUpdate, current: &runners::Settings) -> bool {
    let turns_on = |new: Option<bool>, old: Option<bool>| new == Some(true) && old != Some(true);
    let more_at_once = args.max_concurrent.is_some_and(|new| {
        let old = current.max_concurrent;
        match new {
            0 => old != Some(0),
            new => old.is_none_or(|old| u64::from(new) > old),
        }
    });
    let new_size = args.allowed_sizes.iter().any(|size| {
        current
            .allowed_sizes
            .as_ref()
            .is_none_or(|old| !old.iter().any(|old| old == size))
    });
    turns_on(args.enabled, current.enabled)
        || turns_on(args.allow_public_repos, current.allow_public_repos)
        || more_at_once
        || new_size
}

/// The `--allowed-size` codes, in the order given.
///
/// # Errors
///
/// `usage` for a size given twice, which would send a list with a repeat for the server to
/// judge.
fn allowed_sizes(given: &[String]) -> Result<Vec<&str>> {
    let mut sizes = Vec::new();
    for code in given.iter().map(String::as_str) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A limit answer shaped as the CDN cap's: base units in `monthly_cap`, and `credits`, a
    /// JSON value, in `monthly_cap_credits`; `null` makes both `null`.
    fn limit(credits: &str) -> Value {
        let base = if credits == "null" {
            "null"
        } else {
            r#""20000000000000000000""#
        };
        let text = format!(
            r#"{{"monthly_cap":{base},"monthly_cap_credits":{credits},"spent_this_month":"1.50"}}"#
        );
        serde_json::from_str(&text).expect("each limit is JSON")
    }

    /// The action `--monthly-cap <cap>` asks about over `limit`, or `None` when it is sent
    /// unasked.
    fn asked(limit: &Value, cap: &str) -> Option<String> {
        let cap = if cap == "none" {
            MonthlyCap::None
        } else {
            let credits = crate::units::credits(cap, "a cap");
            assert!(credits.is_ok(), "{cap}: {credits:?}");
            MonthlyCap::Credits(credits.expect("asserted above"))
        };
        limit_question(&cap, limit).map(|(action, _)| action)
    }

    /// A raise, or a removal of a cap in force, asks, naming the cap read from
    /// `monthly_cap_credits` and the new one, compared as exact decimals: the base units in
    /// `monthly_cap` are a far larger number, so a comparison with them would call each raise
    /// a cut.
    #[test]
    fn a_raise_or_a_removal_asks() {
        let raise = |from: &str, to: &str| {
            Some(format!(
                "raise the monthly runner spend cap from `{from}` to `{to}` credits"
            ))
        };
        for (credits, cap, action) in [
            (r#""20.000000""#, "50", raise("20.000000", "50")),
            (r#""0.45""#, "0.5", raise("0.45", "0.5")),
            ("9", "10", raise("9", "10")),
            // A leading zero does not make a cap longer.
            (r#""050""#, "60", raise("050", "60")),
            (r#""0""#, "0.01", raise("0", "0.01")),
            // The amount shown is the normal form of the one given.
            (r#""20""#, "050.500", raise("20", "50.5")),
            (
                r#""20""#,
                "none",
                Some("remove the monthly runner spend cap of `20` credits".to_owned()),
            ),
            // A cap that is not an amount still asks before it is removed, quoted as the
            // server's words.
            (
                r#""lots`""#,
                "none",
                Some(
                    "remove the monthly runner spend cap, where `monthly_cap_credits` is \
                     `lots\\u{60}`, which is not an amount of credits"
                        .to_owned(),
                ),
            ),
        ] {
            assert_eq!(asked(&limit(credits), cap), action, "{credits} {cap}");
        }
    }

    /// `null` is unconfirmed to mean no cap, and may be a server default, so a cap set over it
    /// asks. Without an amount in `monthly_cap_credits` the cap in force is unknown, whatever
    /// `monthly_cap` holds, so every change asks, `none` included.
    #[test]
    fn a_change_over_a_null_or_unknown_cap_asks() {
        assert_eq!(
            asked(&limit("null"), "50").as_deref(),
            Some(
                "set the monthly runner spend cap to `50` credits, where the limit shows \
                 `null`, unset or a server default"
            )
        );
        let set_unknown = "set the monthly runner spend cap to `50` credits, where the cap in \
                           force is unknown: the limit has no amount in `monthly_cap_credits`";
        let remove_unknown = "remove the monthly runner spend cap, whose current value is \
                              unknown: the limit has no amount in `monthly_cap_credits`";
        for answer in [
            r#"{"monthly_cap":"20000000000000000000"}"#,
            r#"{"monthly_cap":"20000000000000000000","monthly_cap_credits":null}"#,
            r#"{"spent_this_month":"1.50"}"#,
            r#"{"monthly_cap_credits":null}"#,
            // A missing `monthly_cap_credits` is not `null`: the cap in force is unknown.
            r#"{"monthly_cap":null}"#,
            // A `null` beside credits hipc cannot read is no proof that no cap is set.
            r#"{"monthly_cap":null,"monthly_cap_credits":true}"#,
            "[]",
        ] {
            let answer: Value = serde_json::from_str(answer).expect("each answer is JSON");
            assert_eq!(
                asked(&answer, "50").as_deref(),
                Some(set_unknown),
                "{answer}"
            );
            assert_eq!(
                asked(&answer, "none").as_deref(),
                Some(remove_unknown),
                "{answer}"
            );
        }
        // A `monthly_cap_credits` that is not a decimal amount is no cap to compare with, and
        // the question names it.
        for (credits, shown) in [
            (r#""-5""#, "`-5`"),
            ("-5", "`-5`"),
            (r#""5.""#, "`5.`"),
            (r#"".5""#, "`.5`"),
            (r#""5.x""#, "`5.x`"),
            (r#""5e1""#, "`5e1`"),
            ("1e300", "`1e+300`"),
            (r#""""#, "``"),
        ] {
            let bad = format!(
                "where `monthly_cap_credits` is {shown}, which is not an amount of credits"
            );
            assert_eq!(
                asked(&limit(credits), "50"),
                Some(format!(
                    "set the monthly runner spend cap to `50` credits, {bad}"
                )),
                "{credits}"
            );
            assert_eq!(
                asked(&limit(credits), "none"),
                Some(format!("remove the monthly runner spend cap, {bad}")),
                "{credits}"
            );
        }
        // One that is not a string or a number is no amount at all.
        for credits in ["true", r#"{"credits":"5"}"#, "[5]"] {
            assert_eq!(
                asked(&limit(credits), "50").as_deref(),
                Some(set_unknown),
                "{credits}"
            );
        }
    }

    /// A lower cap, the same cap, or `none` over a `null` cap allows no more spend, so it is
    /// sent unasked.
    #[test]
    fn a_cut_or_the_same_cap_is_sent_unasked() {
        for (credits, cap) in [
            (r#""20.000000""#, "5"),
            (r#""10""#, "9.99"),
            ("20.5", "20.50"),
            (r#""050""#, "50.0"),
            (r#""50""#, "050.00"),
            (r#""50""#, "50"),
            (r#""0.5""#, "0.45"),
            (r#""1""#, "000.000"),
            (r#""1""#, "0.10"),
            ("null", "none"),
            // 18 decimals is a credit's smallest part; trailing zeros past it are dropped.
            (r#""20""#, "0.000000000000000001"),
            (r#""20""#, "5.000000000000000000000"),
        ] {
            assert_eq!(asked(&limit(credits), cap), None, "{credits} {cap}");
        }
    }
}
