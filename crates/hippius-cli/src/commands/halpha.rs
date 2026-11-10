//! `hipc billing native-quote` and `hipc billing top-up --method halpha`: the hAlpha price,
//! and a buy of credits with the account's hAlpha.
//!
//! Every amount is an 18-decimal integer (10^18 planck is one hAlpha, and one credit), sent
//! and printed as a string of digits and computed in `u128`, never in a float: an amount one
//! planck off is a different buy, and a `min_credits` one planck high is a refused one.
//!
//! The custodial buy is `POST /api/billing/native/buy/`: the API signs
//! `Marketplace.buy_credits` from the account's own Hippius address. An account whose key the
//! API does not hold signs its own buys (`custodial` false in the quote, or a 409
//! `self_sign`), which this version refuses.

use std::time::{Duration, Instant, SystemTime};

use hippius::{Client, Code, Error, Method, Request, Result, SideEffects, phrase};
use serde_json::{Value, json};

use crate::cli::{CHAIN_MINIMUM, ONE, Planck};
use crate::commands::Printed;
use crate::commands::credits::to_credits;
use crate::commands::poll::{Next, Schedule, Wait, poll_until};
use crate::commands::print_with;
use crate::context::{Context, Gate, Write};
use crate::output::Mode;

const QUOTE: &str = "/billing/native/quote/";
const BUY: &str = "/billing/native/buy/";
const PURCHASES: &str = "/billing/native/purchases/";

/// How often a purchase is read again: every 3 seconds, as the console reads it.
const POLL_INTERVAL: Duration = Duration::from_secs(3);

/// How long a purchase is followed without `--wait-timeout`: 15 minutes, the time the console
/// treats a purchase as in flight.
const WAIT_TIMEOUT: Duration = Duration::from_mins(15);

/// The share of the quoted credits a buy accepts at least, in basis points: 1 percent
/// slippage, the console's default. A price that falls further between the quote and the
/// block refuses the buy on chain (`SlippageExceeded`) rather than mint less.
const MIN_CREDITS_BPS: u128 = 9_900;

/// How long a purchase in flight blocks a new buy: the 15 minutes the console treats one as in
/// flight, in seconds.
const IN_FLIGHT_SECONDS: u64 = 15 * 60;

/// The share of the credits `--alpha` buys at the quote now that a retry's `--min-credits`
/// must reach, in basis points: a retry under a key the API has not seen is a new buy, and
/// without this floor it would accept any price its `--min-credits` names.
const RETRY_FLOOR_BPS: u128 = 9_000;

/// The statuses of a purchase still in flight. `completed` and `failed` are final.
const IN_FLIGHT: [&str; 3] = ["queued", "submitted", "included"];

/// The read of the quote, which a top-up sends first.
fn quote_request() -> Request {
    Request::get(QUOTE).about("get the hAlpha quote")
}

/// The fields of a quote that are 18-decimal amounts.
const QUOTE_AMOUNTS: &[&str] = &["alpha_price", "min_alpha", "native_balance"];

/// The fields of a purchase that are 18-decimal amounts.
const PURCHASE_AMOUNTS: &[&str] = &["alpha_amount", "credit_amount", "alpha_price"];

/// `hipc billing native-quote`: the quote as the server sent it, with its amounts as decimals
/// in text mode.
pub(crate) fn native_quote(ctx: &Context<'_>) -> Result<Printed> {
    let client = ctx.load_authed_client()?;
    print_with(ctx, &client, &quote_request(), Gate::Open, |response| {
        Ok(shown_as_decimals(ctx, response.document()?, QUOTE_AMOUNTS))
    })
}

/// `document` as text mode shows it: each of `fields` that holds an integer string written as
/// a decimal of 18 places, and a note naming them. In JSON mode, `document` unchanged.
fn shown_as_decimals(ctx: &Context<'_>, mut document: Value, fields: &[&str]) -> Value {
    if matches!(ctx.mode(), Mode::Json) {
        return document;
    }
    let mut changed = Vec::new();
    for field in fields {
        if let Some(Value::String(text)) = document.get_mut(*field)
            && let Some(decimal) = to_credits(text)
        {
            *text = decimal;
            changed.push(phrase::quote(field));
        }
    }
    if !changed.is_empty() {
        let verb = if changed.len() == 1 { "is" } else { "are" };
        ctx.note(&format!(
            "{} {verb} decimals here, in hAlpha or credits (1 credit = 1 USD); the JSON output \
             keeps 18-decimal integers",
            phrase::join(&changed, "and")
        ));
    }
    document
}

/// The fields of a quote a buy depends on.
#[derive(Debug)]
struct Quote {
    enabled: bool,
    stale: bool,
    custodial: bool,
    alpha_price: u128,
    min_alpha: u128,
    native_balance: u128,
    reason: Option<String>,
    /// The Hippius address whose hAlpha a buy spends, as the quote names it.
    address: Option<String>,
}

impl Quote {
    /// The quote in `answer`, or `internal` when a field a buy depends on is missing or not
    /// its type.
    fn read(answer: &Value) -> Result<Self> {
        let unreadable = || crate::commands::unreadable("get the hAlpha quote");
        let flag = |name: &str| {
            answer
                .get(name)
                .and_then(Value::as_bool)
                .ok_or_else(unreadable)
        };
        let amount = |name: &str| answer.get(name).and_then(integer).ok_or_else(unreadable);
        Ok(Self {
            enabled: flag("enabled")?,
            stale: flag("stale")?,
            custodial: flag("custodial")?,
            alpha_price: amount("alpha_price")?,
            min_alpha: amount("min_alpha")?,
            native_balance: amount("native_balance")?,
            reason: answer
                .get("reason")
                .and_then(Value::as_str)
                .map(str::to_owned),
            address: answer
                .get("address")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    }

    /// The refusal of a retry this quote rules out: an account that signs its own buys, or a
    /// `min_credits` below [`RETRY_FLOOR_BPS`] of what `alpha` buys now. The other refusals of
    /// [`Quote::order`] do not apply: the first buy may have spent the balance, and the API
    /// answers a key it has seen as it answered the first, whatever the quote says now.
    fn check_retry(&self, alpha: Planck, min_credits: Planck) -> Result<()> {
        if !self.custodial {
            return Err(self_sign(Error::new(Code::Conflict, String::new())));
        }
        let credits = credits_for(alpha.0, self.alpha_price).unwrap_or(u128::MAX);
        let floor =
            credits / 10_000 * RETRY_FLOOR_BPS + credits % 10_000 * RETRY_FLOOR_BPS / 10_000;
        if min_credits.0 < floor {
            return Err(Error::new(
                Code::Conflict,
                format!(
                    "`--min-credits {min_credits}` is below 90 percent of the `{}` credits \
                     `--alpha {alpha}` buys at the quoted price now, and under a key the API has \
                     not seen the retry would be a new buy at that price",
                    Planck(credits)
                ),
            )
            .with_hint(format!(
                "run `hipc api GET {PURCHASES}` to find the first buy; if it is not there, top \
                 up again without `--idempotency-key`"
            )));
        }
        Ok(())
    }

    /// The credits `alpha` buys at this quote, and the fewest the buy accepts, or the refusal
    /// of a quote that rules the buy out. Nothing has been sent but the read, so each refusal
    /// is `conflict` with no HTTP status (tenet 4).
    fn order(&self, alpha: Planck) -> Result<(Planck, Planck)> {
        let again = "run `hipc billing native-quote` until it says";
        if !self.enabled {
            let why = self
                .reason
                .as_deref()
                .map(|reason| format!(": the API says {}", phrase::quote(reason)))
                .unwrap_or_default();
            return Err(Error::new(
                Code::Conflict,
                format!("hAlpha top-ups are off (`enabled` is `false` in the quote){why}"),
            )
            .with_hint(format!("{again} `enabled` is `true`, then top up again")));
        }
        if self.stale {
            return Err(Error::new(
                Code::Conflict,
                "the hAlpha price on chain is stale (`stale` is `true` in the quote), and the \
                 chain refuses a buy at a stale price",
            )
            .with_hint(format!("{again} `stale` is `false`, then top up again")));
        }
        if !self.custodial {
            return Err(self_sign(Error::new(Code::Conflict, String::new())));
        }
        if alpha.0 < self.min_alpha {
            let least = Planck(self.min_alpha);
            return Err(Error::new(
                Code::Conflict,
                format!("`--alpha {alpha}` is below the quote's `min_alpha` of `{least}` hAlpha"),
            )
            .with_hint(format!("pass `--alpha` of at least `{least}`")));
        }
        // The fee comes out of what the buy leaves, and the pallet keeps the account alive, so
        // the console keeps the chain minimum back for it.
        if alpha.0.saturating_add(CHAIN_MINIMUM.0) > self.native_balance {
            let held = Planck(self.native_balance);
            return Err(Error::new(
                Code::Conflict,
                format!(
                    "`--alpha {alpha}` and the `{CHAIN_MINIMUM}` hAlpha kept for the fee are more \
                     than the account's `native_balance` of `{held}` hAlpha"
                ),
            )
            .with_hint(format!(
                "pass `--alpha` of at most `{}`",
                Planck(self.native_balance.saturating_sub(CHAIN_MINIMUM.0))
            )));
        }
        let credits = credits_for(alpha.0, self.alpha_price).ok_or_else(|| {
            Error::internal(format!(
                "the quote's `alpha_price` of {} makes a buy of `{alpha}` hAlpha larger than \
                 `hipc` counts",
                phrase::quote(self.alpha_price)
            ))
        })?;
        let least = min_credits(credits);
        if least == 0 {
            return Err(Error::new(
                Code::Conflict,
                format!("`--alpha {alpha}` buys no credits at the quoted `alpha_price`"),
            )
            .with_hint("pass a larger `--alpha`"));
        }
        Ok((Planck(credits), Planck(least)))
    }
}

/// An 18-decimal amount as the API sends it: a string of digits, or a JSON integer.
fn integer(value: &Value) -> Option<u128> {
    let text = match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// The credits `alpha` buys at `price` (credits per hAlpha, both 18-decimal):
/// `floor(alpha * price / 10^18)`, as the pallet computes it. The product can pass `u128`
/// (a million hAlpha at a price of a few credits), so it is split on 10^18: with
/// `alpha = a1 * 10^18 + a0` and `price = p1 * 10^18 + p0`, the quotient is
/// `a1 * price + a0 * p1 + floor(a0 * p0 / 10^18)`, exactly, and `a0 * p0` is below 10^36.
/// `None` when the credits themselves pass `u128`.
pub(crate) fn credits_for(alpha: u128, price: u128) -> Option<u128> {
    let (a1, a0) = (alpha / ONE, alpha % ONE);
    let (p1, p0) = (price / ONE, price % ONE);
    a1.checked_mul(price)?
        .checked_add(a0.checked_mul(p1)?)?
        .checked_add(a0 * p0 / ONE)
}

/// The fewest credits a buy of `credits` accepts: `floor(credits * 9900 / 10000)`, split on
/// 10000 so the product cannot pass `u128`.
pub(crate) fn min_credits(credits: u128) -> u128 {
    credits / 10_000 * MIN_CREDITS_BPS + credits % 10_000 * MIN_CREDITS_BPS / 10_000
}

/// The refusal of a buy the account must sign itself: from a quote that says `custodial` is
/// `false`, or the API's 409 `self_sign`. `error` keeps its HTTP status and details.
fn self_sign(error: Error) -> Error {
    error
        .with_message(
            "the account signs its own hAlpha buys (`self_sign`), which `hipc` cannot sign yet; a \
             card or Bitcoin top-up, or one in the console, works",
        )
        .with_hint(SELF_SIGN_HINT)
}

/// [`self_sign`]'s hint: a top-up that works for such an account today, one action (tenet 15);
/// the message names the others. No `hipc` release signs `Marketplace.buy_credits`, so
/// neither names one.
const SELF_SIGN_HINT: &str = "run `hipc billing top-up --amount <USD> --method card --yes` \
     and pay at its `checkout_url`";

/// The buy, with the `min_credits` it sends and its key.
fn buy_request(alpha: Planck, min_credits: &str, key: &str) -> Request {
    Request::new(Method::Post, BUY)
        .json(&json!({
            "alpha_amount": alpha.0.to_string(),
            "min_credits": min_credits,
        }))
        .idempotency_key(key)
}

/// The purchases list, newest first: swagger's `ordering` on `created_at`, so the purchase a
/// run follows, and any other in flight, is on the first page.
fn purchases_request() -> Request {
    Request::get(PURCHASES)
        .query("ordering", "-created_at")
        .about("read the hAlpha purchases")
}

/// A retry of a buy that may have reached the server: the key it sent and its exact
/// `min_credits`, so the repeat is the same request and the API answers it as the first.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Retry<'a> {
    pub(crate) key: &'a str,
    pub(crate) min_credits: Planck,
}

/// Refuses a new buy while `page`, the purchases list, has a purchase in flight created in
/// the last [`IN_FLIGHT_SECONDS`] before `now` (seconds since 1970). A purchase whose
/// `created_at` cannot be read counts as recent: a second buy is the costlier mistake.
fn refuse_in_flight(page: &Value, now: u64) -> Result<()> {
    let rows = page
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| crate::commands::unreadable("read the hAlpha purchases"))?;
    let recent = |row: &Value| {
        row.get("created_at")
            .and_then(Value::as_str)
            .and_then(hippius::rfc3339_seconds)
            .is_none_or(|created| now.saturating_sub(created) < IN_FLIGHT_SECONDS)
    };
    if let Some(row) = rows.iter().find(|row| in_flight(row) && recent(row)) {
        let id = purchase_id(row).map_or_else(|| "without an id".to_owned(), phrase::quote);
        return Err(Error::new(
            Code::Conflict,
            format!(
                "the purchase {id} is still {}, and a second buy now could spend twice",
                status_of(row)
            ),
        )
        .with_hint(format!(
            "run `hipc api GET {PURCHASES}` until it is `completed` or `failed`, then top up again"
        )));
    }
    Ok(())
}

/// Reads the purchases list with `recent` and refuses a new buy while one is in flight
/// ([`refuse_in_flight`]), measured by the earlier of the answer's `Date` and the local clock
/// (`hippius::Now`): the API dated the purchases by its own clock, so a local clock that runs
/// fast cannot age a purchase in flight out of the window, and a `Date` ahead of time cannot
/// either.
fn check_in_flight(ctx: &Context<'_>, client: &Client, recent: &Request) -> Result<()> {
    let answer = ctx.send(client, recent)?;
    let action = "check the hAlpha purchases in flight";
    let now = hippius::Now::of(answer.date(), SystemTime::now(), action)?;
    refuse_in_flight(&answer.document()?, now.earliest)
}

/// `hipc billing top-up --method halpha --alpha <HALPHA>`: reads the quote and the purchases,
/// asks, reads the purchases again when a person was asked, buys, and follows the purchase. A
/// `retry` resends a buy that may have reached the server, with its key and amounts: it reads
/// only the quote, for [`Quote::check_retry`], since a new `min_credits` would make the
/// repeat a different request.
pub(crate) fn top_up(
    ctx: &Context<'_>,
    alpha: Planck,
    retry: Option<Retry<'_>>,
    wait_timeout: Option<u64>,
) -> Result<Printed> {
    let client = ctx.load_authed_client()?;
    let read = quote_request();
    let recent = purchases_request();
    let draft = match retry {
        Some(retry) => buy_request(alpha, &retry.min_credits.0.to_string(), retry.key),
        None => buy_request(
            alpha,
            "<the credits the quote gives, less 1 percent>",
            "<a new key for this run>",
        ),
    }
    .about(format!("buy credits with `{alpha}` hAlpha"));
    let checked = match ctx.check_write(&client, &draft)? {
        Write::DryRun(mut doc) => {
            // The quote is read before the question, as a list replace reads its list.
            doc["reads_first"] = ctx.describe(&client, &read)?;
            // The polls of the follow are left out, as a VM launch's are (tenet 5).
            let requests = if retry.is_some() {
                vec![&read, &draft]
            } else {
                vec![&read, &recent, &draft]
            };
            doc["requests"] = ctx.describe_each(&client, &requests)?;
            return Ok(Printed::Document(doc));
        }
        Write::Checked(checked) => checked,
    };
    let quote = Quote::read(&ctx.send(&client, &read)?.document()?)?;
    let from = quote
        .address
        .as_deref()
        .map(|address| format!(" of {}", phrase::quote(address)))
        .unwrap_or_default();
    let (key, least, action) = if let Some(retry) = retry {
        quote.check_retry(alpha, retry.min_credits)?;
        (
            retry.key.to_owned(),
            retry.min_credits,
            format!(
                "buy credits with `{alpha}` hAlpha{from} again under the key `{}`, at least `{}` \
                 credits; a key the API has not seen is a new buy",
                retry.key, retry.min_credits
            ),
        )
    } else {
        let (credits, least) = quote.order(alpha)?;
        check_in_flight(ctx, &client, &recent)?;
        let action = format!(
            "buy credits with `{alpha}` hAlpha{from}: about `{credits}` credits at the quoted \
             price, and at least `{least}`, for the signed-in user's own balance"
        );
        (hippius::new_idempotency_key()?, least, action)
    };
    let buy = buy_request(alpha, &least.0.to_string(), &key).about(action);
    let asked = ctx.will_ask();
    ctx.confirm_write(
        checked,
        &buy.action(),
        Some("the hAlpha is spent on chain, and a buy cannot be undone"),
    )?;
    // A buy another `hipc` or the console started while the question waited was not seen
    // when it was asked, so the list is read again.
    if asked && retry.is_none() {
        check_in_flight(ctx, &client, &recent)?;
    }
    let started = ctx
        .send(&client, &buy)
        .and_then(|response| response.document())
        .map_err(|error| buy_failed(ctx, error, retry.is_some(), least, &key))?;
    let wait = wait_timeout.map_or(WAIT_TIMEOUT, Duration::from_secs);
    follow(ctx, &client, started, &key, wait)
}

/// The error of a buy the API refused or may have done. A 409 `self_sign` is the refusal of
/// an account that signs its own buys. A failure after the buy may have reached the server
/// names the key, whose repeat the API answers as the first rather than buying again: the
/// run's own command line, with its profile, origin, and account, and the key and the
/// minimum added unless it was already a retry.
fn buy_failed(ctx: &Context<'_>, error: Error, retried: bool, least: Planck, key: &str) -> Error {
    let code = error
        .details()
        .and_then(|details| details.get("code"))
        .and_then(Value::as_str);
    if error.http_status() == Some(409) && code == Some("self_sign") {
        return self_sign(error);
    }
    if matches!(error.side_effects(), SideEffects::Partial) {
        let mut args = ctx.args().to_vec();
        if !retried {
            let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
            let added = [
                "--min-credits".to_owned(),
                least.to_string(),
                "--idempotency-key".to_owned(),
                key.to_owned(),
            ];
            args.splice(end..end, added);
        }
        return error.with_hint(format!(
            "run `{}` to retry this exact buy, which the API answers as the first rather than \
             buying again",
            phrase::command(&args)
        ));
    }
    error
}

/// The purchase's id as text, which the purchases list is searched for: the console matches
/// `String(id)`, whether the API sends a number or a string.
fn purchase_id(row: &Value) -> Option<String> {
    match row.get("id")? {
        Value::String(id) => Some(id.clone()),
        Value::Number(id) => Some(id.to_string()),
        _ => None,
    }
}

/// Whether `row` is still in flight: a status of [`IN_FLIGHT`], or none yet.
fn in_flight(row: &Value) -> bool {
    match row.get("status") {
        None | Some(Value::Null) => true,
        Some(status) => status.as_str().is_some_and(|s| IN_FLIGHT.contains(&s)),
    }
}

/// How a follow ended: with the purchase out of flight, or at the deadline with it printed.
enum Followed {
    Final(Value),
    Stopped(Printed),
}

/// The follow of a purchase: the last row seen, and whether the list has shown it yet.
struct Purchase<'a> {
    ctx: &'a Context<'a>,
    key: &'a str,
    id: &'a str,
    quoted: &'a str,
    follow_hint: &'a str,
    wait: Duration,
    row: Value,
    found: bool,
}

impl Purchase<'_> {
    fn print(&self) -> Value {
        print_purchase(self.ctx, &self.row, self.key)
    }
}

impl Wait for Purchase<'_> {
    type Output = Followed;

    fn next(&mut self) -> Next<Followed> {
        if in_flight(&self.row) {
            Next::Poll
        } else {
            Next::Stop(Ok(Followed::Final(std::mem::take(&mut self.row))))
        }
    }

    fn take(&mut self, answer: Value) -> Result<()> {
        if let Some(listed) = find(&answer, self.id) {
            self.row = listed;
            self.found = true;
        }
        Ok(())
    }

    fn timed_out(self) -> Result<Followed> {
        let unseen = if self.found {
            ""
        } else {
            ", and the purchases list never showed it"
        };
        self.ctx.warning(&format!(
            "the purchase {} was still {} after {}{unseen}; `hipc billing balance` shows the \
             credits once it completes",
            self.quoted,
            status_of(&self.row),
            phrase::count(self.wait.as_secs(), "second")
        ));
        Ok(Followed::Stopped(Printed::Document(self.print())))
    }

    fn failed(self, error: Error) -> Error {
        error.with_hint(self.follow_hint).after_write(self.print())
    }
}

/// `row`, a purchase, as a follow prints it: with the run's idempotency key, and its amounts
/// as decimals in text mode.
fn print_purchase(ctx: &Context<'_>, row: &Value, key: &str) -> Value {
    let mut doc = row.clone();
    doc["idempotency_key"] = key.into();
    shown_as_decimals(ctx, doc, PURCHASE_AMOUNTS)
}

/// Follows the purchase `started` names until it leaves [`IN_FLIGHT`] or `wait` has passed,
/// and prints the last row with the key. A stop at the deadline is not an error: the buy was
/// accepted, and the row says where it stands.
fn follow(
    ctx: &Context<'_>,
    client: &Client,
    started: Value,
    key: &str,
    wait: Duration,
) -> Result<Printed> {
    let print = |row: &Value| print_purchase(ctx, row, key);
    if !started.is_object() {
        return Err(
            Error::internal("the buy answer is not a purchase, so it cannot be followed")
                .with_hint("run `hipc billing balance` to see whether the credits arrived")
                .after_write(json!({ "idempotency_key": key })),
        );
    }
    let Some(id) = purchase_id(&started) else {
        ctx.warning("the buy answer names no purchase, so it was not followed");
        return Ok(Printed::Document(print(&started)));
    };
    let quoted = phrase::quote(&id);
    // Said at once: a run stopped during the follow has printed nothing else.
    ctx.note(&format!(
        "the buy was accepted as the purchase {quoted} under the key `{key}`; following it"
    ));
    let follow_hint =
        format!("run `hipc api GET {PURCHASES}` to see where the purchase {quoted} stands");
    let schedule = Schedule {
        // The first read goes out at once, whatever the deadline.
        first: Duration::ZERO,
        interval: POLL_INTERVAL,
        deadline: Instant::now() + wait,
    };
    let purchase = Purchase {
        ctx,
        key,
        id: &id,
        quoted: &quoted,
        follow_hint: &follow_hint,
        wait,
        row: started,
        found: false,
    };
    let row = match poll_until(ctx, client, &purchases_request(), schedule, purchase)? {
        Followed::Final(row) => row,
        Followed::Stopped(printed) => return Ok(printed),
    };
    match row.get("status").and_then(Value::as_str) {
        Some("completed") => {
            ctx.note(&format!(
                "the purchase {quoted} completed; `hipc billing balance` shows the credits"
            ));
            Ok(Printed::Document(print(&row)))
        }
        // The buy was accepted and the chain refused it: part of the command went through,
        // so stdout holds the purchase and `details` the chain's error.
        Some("failed") => {
            let error = row.get("error").and_then(Value::as_str).unwrap_or_default();
            let why = if error.is_empty() {
                String::new()
            } else {
                format!(": the API says {}", phrase::quote(error))
            };
            Err(Error::partial_failure(
                format!("the purchase {quoted} failed on chain{why}"),
                print(&row),
                json!({ "purchase": id, "status": "failed", "error": error }),
                SideEffects::Partial,
            )
            .with_hint("run `hipc billing native-quote` to check the price and the hAlpha"))
        }
        // A status `hipc` does not know may be a refusal: not reported as a success.
        _ => {
            let status = status_of(&row);
            Err(Error::partial_failure(
                format!("the purchase {quoted} is {status}, a status `hipc` does not know"),
                print(&row),
                json!({ "purchase": id, "status": row.get("status").cloned().unwrap_or(Value::Null) }),
                SideEffects::Partial,
            )
            .with_hint("run `hipc billing balance` to see whether the credits arrived"))
        }
    }
}

/// The row's status as a message names it.
fn status_of(row: &Value) -> String {
    row.get("status")
        .and_then(Value::as_str)
        .map_or_else(|| "without a status".to_owned(), phrase::quote)
}

/// The row of the purchase `id` on the first page of `page`, the purchases list, newest first.
fn find(page: &Value, id: &str) -> Option<Value> {
    page.get("results")?
        .as_array()?
        .iter()
        .find(|row| purchase_id(row).as_deref() == Some(id))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credits_are_the_exact_floor_of_the_product() {
        let cases = [
            (ONE, ONE, ONE),
            (ONE * 3 / 2, 5 * ONE, 7_500_000_000_000_000_000),
            (1, 1, 0),
            (ONE - 1, ONE - 1, ONE - 2),
            (ONE / 100, 4_123_456_789_012_345_678, 41_234_567_890_123_456),
            // A million hAlpha at 340 credits: the product is 3.4e44, past u128.
            (1_000_000 * ONE, 340 * ONE, 340_000_000 * ONE),
            (0, ONE, 0),
        ];
        for (alpha, price, credits) in cases {
            assert_eq!(
                credits_for(alpha, price),
                Some(credits),
                "{alpha} * {price}"
            );
        }
        assert_eq!(credits_for(u128::MAX, 2 * ONE), None);
        assert_eq!(credits_for(ONE, u128::MAX), Some(u128::MAX));
        assert_eq!(credits_for(2 * ONE, u128::MAX), None);
        assert_eq!(credits_for(ONE + 1, u128::MAX), None);
    }

    /// Against 256-bit long multiplication done by hand on values that fit the check.
    #[test]
    fn credits_agree_with_plain_math_where_it_fits() {
        let values = [
            0,
            1,
            7,
            999,
            ONE - 1,
            ONE,
            ONE + 1,
            12_345 * ONE + 678,
            10_u128.pow(19),
        ];
        for alpha in values {
            for price in values {
                if let Some(product) = alpha.checked_mul(price) {
                    assert_eq!(
                        credits_for(alpha, price),
                        Some(product / ONE),
                        "{alpha} {price}"
                    );
                }
            }
        }
    }

    #[test]
    fn min_credits_is_one_percent_less_rounded_down() {
        for (credits, least) in [
            (0, 0),
            (1, 0),
            (100, 99),
            (10_000, 9_900),
            (10_001, 9_900),
            (ONE, 990_000_000_000_000_000),
            (7_500_000_000_000_000_000, 7_425_000_000_000_000_000),
            (
                u128::MAX,
                u128::MAX / 10_000 * 9_900 + u128::MAX % 10_000 * 9_900 / 10_000,
            ),
        ] {
            assert_eq!(min_credits(credits), least, "{credits}");
        }
        for credits in [3_u128, 12_345, 99_999, 10_u128.pow(30) + 7] {
            assert_eq!(min_credits(credits), credits * 9_900 / 10_000, "{credits}");
        }
    }

    #[test]
    fn ids_and_statuses_of_another_type_are_not_read() {
        let row = json!({"id": true, "status": 3});
        assert_eq!(purchase_id(&row), None);
        assert!(
            !in_flight(&row),
            "a status that is not text has left the known ones"
        );
        assert!(in_flight(&json!({"status": null})));
        assert!(in_flight(&json!({})));
        assert_eq!(purchase_id(&json!({"id": 7})).as_deref(), Some("7"));
    }

    #[test]
    fn rfc3339_times_read_as_seconds_since_1970() {
        for (text, seconds) in [
            ("1970-01-01T00:00:00Z", 0),
            ("2026-11-03T10:00:00Z", 1_793_700_000),
            ("2026-11-03T10:00:00.123456Z", 1_793_700_000),
            ("2026-11-03T12:00:00+02:00", 1_793_700_000),
            ("2026-11-03T05:30:00-04:30", 1_793_700_000),
            ("2024-02-29T23:59:60Z", 1_709_251_200),
        ] {
            assert_eq!(hippius::rfc3339_seconds(text), Some(seconds), "{text}");
        }
        for bad in [
            "",
            "2026-11-03",
            "2026-11-03T10:00:00",
            "1969-12-31T23:59:59Z",
            "1970-01-01T00:00:00+00:01",
            "2026-13-03T10:00:00Z",
            "2026-00-03T10:00:00Z",
            "2026-11-32T10:00:00Z",
            "2026-11-00T10:00:00Z",
            "2026-11-03T24:00:00Z",
            "2026-11-03T10:60:00Z",
            "2026-11-03T10:00:61Z",
            "2026-11-03T10:00:00.Z",
            "2026-11-03T10:00:00.1aZ",
            "2026-11-03T10:00Z",
            "2026-11-03T10:00:00+0200",
            "2026-11-03T10:00:00+xx:00",
            "2026-11-03T10:00:00+02:xx",
            "2026-x1-03T10:00:00Z",
            "2026-11-03T1x:00:00Z",
            "2026-11-03T10:00:00Q",
            "2026-11-03T10:00:00Z02:00",
        ] {
            assert_eq!(hippius::rfc3339_seconds(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_recent_purchase_in_flight_blocks_a_new_buy() {
        let clock = 1_793_700_000;
        let page = |rows: Value| json!({ "results": rows });
        let at = |status: &str, created_at: Value| json!({"id": "p_9", "status": status, "created_at": created_at});
        for blocking in [
            at("queued", Value::from("2026-11-03T09:55:00Z")),
            at("included", Value::from("not a time")),
            at("submitted", Value::Null),
        ] {
            let error = refuse_in_flight(&page(json!([blocking])), clock).unwrap_err();
            assert_eq!(error.code(), Code::Conflict);
        }
        for passing in [
            json!([]),
            json!([at("completed", Value::from("2026-11-03T09:59:00Z"))]),
            json!([at("queued", Value::from("2026-11-03T09:44:59Z"))]),
            json!([{"status": "failed"}]),
        ] {
            assert!(
                refuse_in_flight(&page(passing.clone()), clock).is_ok(),
                "{passing}"
            );
        }
        let error = refuse_in_flight(&json!({}), clock).unwrap_err();
        assert_eq!(error.code(), Code::Internal);
        let unnamed = json!([{"status": "queued"}]);
        let error = refuse_in_flight(&page(unnamed), clock).unwrap_err();
        assert!(error.message().contains("without an id"));
    }

    #[test]
    fn integers_are_digits_only() {
        assert_eq!(integer(&Value::from("10000000000000000")), Some(ONE / 100));
        assert_eq!(integer(&json!(12)), Some(12));
        for bad in [
            Value::from(""),
            Value::from("-1"),
            Value::from("1.5"),
            Value::from(" 1"),
            json!(1.5),
            json!(-3),
            Value::Null,
            Value::from("1".repeat(40)),
        ] {
            assert_eq!(integer(&bad), None, "{bad}");
        }
    }
}
