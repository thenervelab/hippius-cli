//! `hipc billing`: the balance, the compute burn rate and usage, every plan in one view, the
//! transaction history, the invoices, the billing profile, the auto-reload setting, its card,
//! and its attempts, and a card, Bitcoin, or hAlpha top-up (the last in `halpha`).
//!
//! USDC and TAO top-ups are not wrapped: they go through `hipc api` or the console.
//!
//! Swagger types no body of `/api/billing/profile/`: its fields are the console's billing
//! form, and swagger says a field its `PUT` leaves out keeps its value, so the update sends
//! only the fields given. The auto-reload `PUT` requires the whole setting, so its update reads
//! the setting first and sends every field back, with only the flags given changed. A field
//! that read does not hold in the shape the route takes is refused rather than guessed, so the
//! update never resets a field the caller did not name.

use std::path::Path;

use hippius::{Code, Error, Method, Request, Result, Selection, SideEffects, phrase};
use serde_json::{Map, Value, json};

use crate::cli::{
    AutoReload, AutoReloadUpdate, Billing, BillingProfile, Invoices, ProfileUpdate, ReloadCap,
    TopUp, TopUpMethod, Usd, usd,
};
use crate::commands::credits::{AmountPath, shown_in_credits};
use crate::commands::files::write_output_file;
use crate::commands::{
    Printed, dry_run_requests, failed_part, gamma, halpha, list, print_body, print_with,
};
use crate::context::{Context, Gate, Step, Write};

const PROFILE: &str = "/billing/profile/";
const AUTO_RELOAD: &str = "/billing/auto-reload/";

pub(crate) fn run(ctx: &Context<'_>, command: &Billing) -> Result<Printed> {
    // The config file is read only once a command needs a client, after its argv checks, such
    // as the top-up amount (tenet 5).
    let client = || ctx.load_authed_client();
    match command {
        Billing::Balance => print_body(
            ctx,
            &client()?,
            &Request::get("/billing/credits/balance/").about("get the balance"),
            Gate::Open,
        ),
        Billing::Stripe => print_body(
            ctx,
            &client()?,
            &Request::get("/billing/stripe/active-subscription/")
                .about("get the legacy Stripe top-up plan"),
            Gate::Open,
        ),
        Billing::Transactions(args) => list::run(
            ctx,
            &client()?,
            &Request::get("/billing/transactions/").about("list the transactions"),
            args,
        ),
        Billing::Subscriptions => subscriptions(ctx, &client()?),
        Billing::Summary => print_in_credits(
            ctx,
            &client()?,
            &Request::get("/compute/billing/summary/").about("get the compute burn rate"),
            SUMMARY_AMOUNTS,
        ),
        Billing::Usage { month } => {
            let request = Request::get("/compute/usage/");
            let request = match month {
                Some(month) => request
                    .query("month", month)
                    .about(format!("get the compute usage of `{month}`")),
                None => request.about("get the compute usage of this month"),
            };
            print_in_credits(ctx, &client()?, &request, USAGE_AMOUNTS)
        }
        Billing::Invoices(Invoices::List(args)) => list::run(
            ctx,
            &client()?,
            &Request::get("/billing/invoices/").about("list the invoices"),
            args,
        ),
        Billing::Invoices(Invoices::Get {
            invoice_id,
            output_file,
        }) => download(ctx, &client()?, *invoice_id, output_file),
        Billing::AutoReload(AutoReload::Get) => {
            billing_read(ctx, &read_auto_reload(), "an auto-reload read")
        }
        Billing::AutoReload(AutoReload::Update(args)) => update_auto_reload(ctx, args),
        Billing::AutoReload(AutoReload::CardSetup) => card_setup(ctx, &client()?),
        Billing::Profile(BillingProfile::Get) => {
            billing_read(ctx, &read_profile(), "a billing profile read")
        }
        Billing::Profile(BillingProfile::Update(args)) => update_profile(ctx, args),
        // `{results}` with no `next`: printed as the server's object rather than as a list
        // envelope, whose null `next_page_token` would say no older attempt exists, which
        // the route does not say (tenet 6).
        Billing::AutoReload(AutoReload::History { limit }) => billing_read(
            ctx,
            &Request::get("/billing/auto-reload/history/")
                .query("limit", &limit.to_string())
                .about("list the auto-reload attempts"),
            "an auto-reload history read",
        ),
        Billing::NativeQuote => halpha::native_quote(ctx),
        Billing::Gamma(command) => gamma::run(ctx, command),
        Billing::TopUp(top_up) => start_top_up(ctx, top_up),
    }
}

/// The base-unit amounts of the compute summary, as the console reads them. `balance` is in
/// credits already.
const SUMMARY_AMOUNTS: &[AmountPath] = &[
    &["burn_rate_per_hour"],
    &["arrears"],
    &["items", "*", "per_hour"],
];

/// The base-unit amounts of a usage month, as the console reads them. A line's `quantity`
/// is seconds or bytes, not money, so it is not named.
const USAGE_AMOUNTS: &[AmountPath] = &[
    &["total"],
    &["hours", "*", "total"],
    &["hours", "*", "lines", "*", "unit_price"],
    &["hours", "*", "lines", "*", "amount"],
];

/// Prints the server's body, with `amounts` in credits in text mode ([`shown_in_credits`]).
fn print_in_credits(
    ctx: &Context<'_>,
    client: &hippius::Client,
    request: &Request,
    amounts: &[AmountPath],
) -> Result<Printed> {
    print_with(ctx, client, request, Gate::Open, |response| {
        Ok(shown_in_credits(ctx, response.document()?, amounts))
    })
}

/// What a top-up `method` is called in a message, where it is sent (swagger
/// `create_checkout_session` and `billing_btcpay_create-invoice_create`), and the amounts it
/// takes. The card range is swagger's `minimum` and `maximum` on `amount`. The Bitcoin range
/// is the default that swagger's description gives; a server setting may set a lower ceiling,
/// which the server then enforces itself. Both match the ranges the console offers.
///
/// A hAlpha top-up has no rail: [`start_top_up`] sends it to [`halpha::top_up`] first.
const fn rail(method: TopUpMethod) -> (&'static str, &'static str, Usd, Usd) {
    match method {
        TopUpMethod::Card => (
            "card",
            "/billing/stripe/create-checkout-session/",
            Usd::from_cents(10 * 100),
            Usd::from_cents(10_000 * 100),
        ),
        TopUpMethod::Bitcoin | TopUpMethod::Halpha => (
            "bitcoin",
            "/billing/btcpay/create-invoice/",
            Usd::from_cents(20 * 100),
            Usd::from_cents(100_000 * 100),
        ),
    }
}

/// Starts a top-up and prints the server's answer, which holds `checkout_url`. Nothing is
/// paid here: the balance grows only once a person pays at that URL, and `hipc` does not
/// wait for it.
///
/// The amount goes as a decimal string to the cent, as the console sends it, so it never
/// passes through a float. No return URL is sent: the server falls back to its own, and
/// ignores one off the console origin.
fn start_top_up(ctx: &Context<'_>, top_up: &TopUp) -> Result<Printed> {
    let amount = match (top_up.method, top_up.amount, top_up.alpha) {
        (TopUpMethod::Halpha, None, Some(alpha)) => {
            let retry = match (top_up.idempotency_key.as_deref(), top_up.min_credits) {
                (Some(key), Some(min_credits)) => Some(halpha::Retry { key, min_credits }),
                (None, None) => None,
                (Some(_), None) => {
                    return Err(Error::usage(
                        "a retry with `--idempotency-key` resends the first buy's exact amounts, \
                         and needs its `--min-credits`",
                        "pass `--min-credits <CREDITS>` as the failed run printed it",
                    ));
                }
                (None, Some(_)) => {
                    return Err(Error::usage(
                        "`--min-credits` is the minimum of a buy to retry, and needs that buy's \
                         `--idempotency-key`",
                        "pass `--idempotency-key <UUID>` as the failed run printed it",
                    ));
                }
            };
            return halpha::top_up(ctx, alpha, retry, top_up.wait_timeout);
        }
        (TopUpMethod::Halpha, Some(_), _) => {
            return Err(Error::usage(
                "`--amount` is US dollars, for a `card` or `bitcoin` top-up; a `halpha` top-up \
                 takes `--alpha`",
                "pass the hAlpha to spend as `--alpha <HALPHA>` in place of `--amount`",
            ));
        }
        (TopUpMethod::Halpha, None, None) => {
            return Err(Error::usage(
                "a `halpha` top-up needs `--alpha`, the hAlpha to spend",
                "pass `--alpha <HALPHA>`, such as `--alpha 1.5`",
            ));
        }
        (method, _, Some(_)) => {
            return Err(Error::usage(
                format!(
                    "`--alpha` is hAlpha, for a `halpha` top-up; a `{}` top-up takes `--amount` \
                     in US dollars",
                    rail(method).0
                ),
                "pass the US dollars as `--amount <USD>` in place of `--alpha`",
            ));
        }
        (method, None, None) => {
            return Err(Error::usage(
                format!(
                    "a `{}` top-up needs `--amount`, in US dollars",
                    rail(method).0
                ),
                "pass `--amount <USD>`, such as `--amount 50`",
            ));
        }
        (_, Some(amount), None) => amount,
    };
    let (name, path, least, most) = rail(top_up.method);
    for (given, flag) in [
        (top_up.idempotency_key.is_some(), "--idempotency-key"),
        (top_up.min_credits.is_some(), "--min-credits"),
        (top_up.wait_timeout.is_some(), "--wait-timeout"),
    ] {
        if given {
            return Err(Error::usage(
                format!("`{flag}` is for a `halpha` top-up, and a `{name}` top-up sends none"),
                format!("leave out `{flag}`"),
            ));
        }
    }
    if !(least..=most).contains(&amount) {
        return Err(Error::usage(
            format!(
                "`--amount {amount}` is outside what a {name} top-up takes: `{least}` to \
                 `{most}` USD"
            ),
            format!("pass `--amount` from `{least}` to `{most}`"),
        ));
    }
    let client = &ctx.load_authed_client()?;
    let request = Request::new(Method::Post, path)
        .json(&json!({ "amount": amount.to_string() }))
        .about(format!("start a {name} top-up of `{amount}` USD"));
    require_billing_account(client, &request, "a top-up")?;
    let gate = Gate::Confirm {
        consequence: Some("nothing is paid until the checkout is paid"),
    };
    print_with(ctx, client, &request, gate, |response| {
        let body = response.document()?;
        // A person may paste the note into a log, so the URL loses its query values there
        // (tenet 9); the whole URL is on stdout.
        let at = body
            .get("checkout_url")
            .and_then(Value::as_str)
            .map(|url| format!(" at {}", phrase::quote(hippius::without_query_values(url))))
            .unwrap_or_default();
        ctx.note(&format!(
            "nothing has been paid yet: the balance grows by `{amount}` USD once the \
             checkout{at} is paid"
        ));
        Ok(body)
    })
}

/// Refuses `request`, `what` in a message, in a membership without a billing account.
///
/// The console sends `X-Hippius-Account` on the top-up, billing profile, and auto-reload
/// routes only from a billing account's page, beside `X-Hippius-Billing`, and swagger never
/// says these routes honour the account header alone: sent so, a write could change the
/// caller's own wallet or invoice profile. So a membership without a selected billing account,
/// from a flag, the variable, or the profile, is refused here, before the dry run.
fn require_billing_account(client: &hippius::Client, request: &Request, what: &str) -> Result<()> {
    if let Selection::Member(account) = client.account()
        && client.account_header(request)?.is_some()
        && client.billing_header(request)?.is_none()
    {
        // The code's fixed hint, `--account self`, stays the one fix it names (tenet 4); the
        // message says the other, a billing account, and where the account came from.
        return Err(account.refusal(&format!(
            "{what} in another account needs a billing account from `--billing`, \
             `HIPPIUS_BILLING`, or `hipc account billing-accounts use`"
        )));
    }
    Ok(())
}

/// Prints the answer to `request`, a read of a billing route that needs a billing account in
/// a membership ([`require_billing_account`]).
fn billing_read(ctx: &Context<'_>, request: &Request, what: &str) -> Result<Printed> {
    let client = ctx.load_authed_client()?;
    require_billing_account(&client, request, what)?;
    print_body(ctx, &client, request, Gate::Open)
}

/// The hint of a write that failed after it may have reached the server (a timeout, a 5xx, a
/// redirect): it replaces the library's "check whether the change was made" with `read`, the
/// command that shows whether it took, before a retry sends it again.
fn check_first(error: Error, read: &str) -> Error {
    if matches!(error.side_effects(), SideEffects::Partial) {
        error.with_hint(format!(
            "run `{read}` to check whether it was saved before running the command again"
        ))
    } else {
        error
    }
}

fn read_profile() -> Request {
    Request::get(PROFILE).about("get the billing profile")
}

fn read_auto_reload() -> Request {
    Request::get(AUTO_RELOAD).about("get the auto-reload setting")
}

/// Starts a Stripe checkout in setup mode that saves the card auto-reload charges, and prints
/// the server's answer, which holds `checkout_url`. Nothing is charged (swagger), so nothing
/// is asked; the card is saved once a person completes the checkout, and `hipc` does not wait
/// for it.
///
/// The body is empty, as the console sends it outside a billing account's page: the return
/// URLs are the console's own pages, and the server falls back to its own without them. The
/// document gains `next_step`, the command that shows the saved card: tenet 3 allows it on a
/// document that waits for a person, since JSON mode prints no note. It repeats the run's
/// selectors, or is left out when one is not a plain word ([`Context::next_command`]).
fn card_setup(ctx: &Context<'_>, client: &hippius::Client) -> Result<Printed> {
    let request = Request::new(Method::Post, format!("{AUTO_RELOAD}card-setup/"))
        .json(&json!({}))
        .about("start saving a card for auto-reload");
    require_billing_account(client, &request, "a card setup")?;
    print_with(ctx, client, &request, Gate::Open, |response| {
        let mut body = response.document()?;
        crate::commands::drop_next_step(&mut body);
        let start = "nothing is charged: the card is saved once the checkout at `checkout_url` \
                     in the output is completed";
        match (ctx.next_command("billing auto-reload get"), &mut body) {
            (Some(command), Value::Object(fields)) => {
                ctx.note(&format!("{start}; `{command}` then shows the card"));
                fields.insert("next_step".to_owned(), Value::from(command));
            }
            _ => ctx.note(start),
        }
        Ok(body)
    })
}

/// The `PUT` of `path` with `body`.
fn put(path: &str, body: Map<String, Value>) -> Request {
    Request::new(Method::Put, path).json(&Value::Object(body))
}

/// `PUT /api/billing/profile/` with only the fields given. Swagger says the `PUT` creates or
/// replaces the profile and that a field left out keeps its value, so nothing is read first:
/// no field the caller did not name is sent, and no other client's change in between is
/// written over. An empty value is sent as `""`, which clears the field. Nothing here spends.
///
/// A new `--billing-email` sends the invoices and billing emails to another address, so with
/// it the profile is read first (after the write's refusals and the token check), and the
/// update is confirmed unless the profile already holds that exact email. A profile that is
/// not found (404) or holds no email (missing, `null`, or empty) counts as a change; any other
/// failed read stops the command before the `PUT`. The dry run lists that read, then the
/// `PUT`. The route takes no version or `If-Match`, so a change another client saves between
/// the read and the `PUT` is not seen; the question names the email the read found.
fn update_profile(ctx: &Context<'_>, args: &ProfileUpdate) -> Result<Printed> {
    let client = ctx.load_authed_client()?;
    let given: Map<String, Value> = args
        .fields()
        .into_iter()
        .filter_map(|(key, given)| given.map(|value| (key.to_owned(), Value::from(value))))
        .collect();
    let named: Vec<String> = given.keys().map(|key| format!("`{key}`")).collect();
    let request = put(PROFILE, given).about(format!(
        "update the billing profile's {}",
        phrase::join(&named, "and")
    ));
    require_billing_account(&client, &request, "a billing profile update")?;
    let check = |error| check_first(error, "hipc billing profile get");
    let Some(email) = args.billing_email.as_deref() else {
        return print_body(ctx, &client, &request, Gate::Open).map_err(check);
    };
    let checked = match ctx.check_write(&client, &request)? {
        Write::DryRun(mut doc) => {
            doc["requests"] = ctx.describe_each(&client, &[&read_profile(), &request])?;
            return Ok(Printed::Document(doc));
        }
        Write::Checked(checked) => checked,
    };
    let current = match ctx.send(&client, &read_profile()) {
        Ok(response) => response
            .document()?
            .get("billing_email")
            .and_then(Value::as_str)
            .filter(|email| !email.is_empty())
            .map(str::to_owned),
        Err(error) if error.code() == Code::NotFound => None,
        Err(error) => return Err(error),
    };
    // Clearing an email the profile does not hold sends invoices nowhere new.
    let action = match (current.as_deref(), email) {
        (Some(old), _) if old == email => None,
        (None, "") => None,
        (Some(old), "") => Some(format!("clear the billing email {}", phrase::quote(old))),
        (Some(old), _) => Some(format!(
            "change the billing email from {} to {}",
            phrase::quote(old),
            phrase::quote(email)
        )),
        (None, _) => Some(format!("set the billing email to {}", phrase::quote(email))),
    };
    if let Some(action) = action {
        let consequence = if email.is_empty() {
            "the invoices and billing emails no longer go to that address"
        } else {
            "the invoices and billing emails go to the new address"
        };
        ctx.confirm_write(checked, &action, Some(consequence))?;
    }
    ctx.send(&client, &request)
        .and_then(|response| response.document())
        .map(Printed::Document)
        .map_err(check)
}

/// `PUT /api/billing/auto-reload/` with the whole setting: the flags given, and the rest as
/// the read just before says. Swagger requires `enabled`, `threshold`, and `target` in every
/// `PUT`, so the setting is read and sent whole. Each amount is US dollars to the cent, a
/// string (swagger: "Credits (1 credit = 1 USD), 2 decimals"); a `monthly_cap` of `null` is
/// no limit (the console's form).
///
/// The setting arms a recurring charge to the card, so an update is confirmed when it may
/// charge more ([`charges_more`]), and never when it leaves auto-reload off. Which one it is
/// needs the setting in force, so the read comes after the write's own refusals and the token
/// check, and before the question; the dry run lists that read, then the write.
///
/// The route takes no version or `If-Match`, so a change another client saves between the read
/// and the `PUT` is written over by the fields this one sends back; the question, when there
/// is one, names the whole setting sent.
fn update_auto_reload(ctx: &Context<'_>, args: &AutoReloadUpdate) -> Result<Printed> {
    let client = ctx.load_authed_client()?;
    let given = [
        ("enabled", args.enabled.map(Value::from)),
        ("threshold", args.threshold.map(dollars)),
        ("target", args.target.map(dollars)),
        (
            "monthly_cap",
            args.monthly_cap.map(|cap| match cap {
                ReloadCap::Usd(amount) => dollars(amount),
                ReloadCap::None => Value::Null,
            }),
        ),
    ];
    let draft = given
        .iter()
        .map(|(key, value)| {
            let shown = || Value::from(format!("<the setting's {key}>"));
            ((*key).to_owned(), value.clone().unwrap_or_else(shown))
        })
        .collect();
    let save = "save the auto-reload setting";
    let draft = put(AUTO_RELOAD, draft).about(save);
    require_billing_account(&client, &draft, "an auto-reload update")?;
    let checked = match ctx.check_write(&client, &draft)? {
        Write::DryRun(mut doc) => {
            doc["requests"] = ctx.describe_each(&client, &[&read_auto_reload(), &draft])?;
            return Ok(Printed::Document(doc));
        }
        Write::Checked(checked) => checked,
    };
    let current = ctx.send(&client, &read_auto_reload())?.document()?;
    // Swagger: `available: false`, and nothing else, while the feature is off on the server.
    // No flag can fix that, so it is said before any field is checked.
    if current.get("available") == Some(&Value::Bool(false)) {
        return Err(Error::new(
            Code::Conflict,
            "auto-reload is not available on this server: the setting reads `available` \
             `false`",
        ));
    }
    let missing: Vec<&str> = given
        .iter()
        .filter(|(key, value)| value.is_none() && kept(&current, key).is_none())
        .map(|(key, _)| *key)
        .collect();
    if !missing.is_empty() {
        let quoted: Vec<String> = missing.iter().map(|key| format!("`{key}`")).collect();
        let flags: Vec<String> = missing
            .iter()
            .map(|key| format!("`--{}`", key.replace('_', "-")))
            .collect();
        return Err(Error::usage(
            format!(
                "the auto-reload setting holds no {} hipc can send back unchanged",
                phrase::join(&quoted, "or")
            ),
            format!(
                "pass {} to set {}",
                phrase::join(&flags, "and"),
                if flags.len() == 1 { "it" } else { "them" }
            ),
        ));
    }
    let body: Map<String, Value> = given
        .into_iter()
        .map(|(key, value)| {
            let value = value.or_else(|| kept(&current, key)).unwrap_or_default();
            (key.to_owned(), value)
        })
        .collect();
    let request = match charges_more(&current, &body) {
        Some((action, consequence)) => {
            let request = put(AUTO_RELOAD, body).about(action);
            ctx.confirm_write(checked, &request.action(), Some(consequence))?;
            request
        }
        None => put(AUTO_RELOAD, body).about(save),
    };
    ctx.send(&client, &request)
        .and_then(|response| response.document())
        .map(Printed::Document)
        .map_err(|error| check_first(error, "hipc billing auto-reload get"))
}

/// An amount as the auto-reload route takes it: a string of dollars to the cent.
fn dollars(amount: Usd) -> Value {
    Value::from(amount.to_string())
}

/// The value of `key` in the setting read, to send back unchanged, or `None` when it is not
/// in the shape the `PUT` takes: `enabled` a boolean, an amount a string, and `monthly_cap` a
/// string or `null`. A setting never saved has `null` amounts.
fn kept(current: &Value, key: &str) -> Option<Value> {
    current
        .get(key)
        .filter(|value| match key {
            "enabled" => value.is_boolean(),
            "monthly_cap" => value.is_string() || value.is_null(),
            _ => value.is_string(),
        })
        .cloned()
}

/// What the auto-reload setting does once `body` is saved, as a question names it.
fn reload_setting(body: &Map<String, Value>) -> String {
    let text = |key: &str| phrase::quote(body[key].as_str().unwrap_or_default());
    let cap = match body["monthly_cap"].as_str() {
        Some(cap) => format!("a monthly cap of {} USD", phrase::quote(cap)),
        None => "no monthly cap".to_owned(),
    };
    format!(
        "charge the card when the balance falls below {} USD, back up to {} USD, with {cap}",
        text("threshold"),
        text("target")
    )
}

/// The question an auto-reload update asks, as its action and consequence, or `None` when it
/// cannot charge more than the setting in force does.
///
/// Nothing is asked when `body` leaves auto-reload off. Leaving it on is asked about when the
/// setting in force is off; when its state is anything but `armed`, since a save closes a
/// failure episode and the server checks the balance at once (swagger), which may charge the
/// card now, during a `cooldown` or an `in_flight` reload too; when it raises the threshold,
/// the target, or the monthly cap, `null` being no cap and so the highest; and when any of
/// those cannot be read as the fields compared, so a shape hipc does not know never lets a
/// raise through unasked.
fn charges_more(current: &Value, body: &Map<String, Value>) -> Option<(String, &'static str)> {
    if body["enabled"] != Value::Bool(true) {
        return None;
    }
    let setting = reload_setting(body);
    let on = "the card is then charged whenever the balance falls below the threshold, and the \
              balance is checked at once";
    let unknown = |key: &str| {
        let shown = current
            .get(key)
            .map_or_else(|| "missing".to_owned(), |value| phrase::quote(shown(value)));
        Some((
            format!(
                "set auto-reload to {setting}, where the setting in force is unknown: its \
                 `{key}` is {shown}"
            ),
            "if the setting in force is lower, the card may then be charged more",
        ))
    };
    match current.get("enabled") {
        Some(Value::Bool(true)) => {}
        Some(Value::Bool(false)) => {
            return Some((format!("turn on auto-reload, to {setting}"), on));
        }
        _ => return unknown("enabled"),
    }
    match current.get("state").and_then(Value::as_str) {
        Some("armed") => {}
        Some(
            state @ ("off"
            | "disabled_failures"
            | "retry_wait"
            | "in_flight"
            | "cooldown"
            | "monthly_cap_reached"),
        ) => {
            return Some((
                format!("save auto-reload on while its state is `{state}`, to {setting}"),
                on,
            ));
        }
        _ => return unknown("state"),
    }
    let mut raised = Vec::new();
    for (key, name) in [
        ("threshold", "threshold"),
        ("target", "target"),
        ("monthly_cap", "monthly cap"),
    ] {
        let old = current.get(key).and_then(|value| limit(key, value));
        match (limit(key, &body[key]), old) {
            (Some(new), Some(old)) if new > old => raised.push(name),
            (Some(_), Some(_)) => {}
            _ => return unknown(key),
        }
    }
    (!raised.is_empty()).then(|| {
        (
            format!(
                "raise the auto-reload {}, to {setting}",
                phrase::join(&raised, "and")
            ),
            "the card may then be charged more",
        )
    })
}

/// A JSON value as a question shows it: a string as its text, anything else as JSON.
fn shown(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

/// An amount of the auto-reload setting, compared as cents, so no float decides a raise.
/// No cap ranks above every amount, by the order of the variants.
#[derive(PartialEq, PartialOrd)]
enum Limit {
    Usd(Usd),
    NoCap,
}

/// The amount `key` holds, or `None` for anything that is not an amount to the cent. Only
/// `monthly_cap` may be `null`: a `null` threshold or target in force is a setting never
/// saved, which a question must not read as the highest.
fn limit(key: &str, value: &Value) -> Option<Limit> {
    match value {
        Value::Null if key == "monthly_cap" => Some(Limit::NoCap),
        Value::String(text) => usd(text).ok().map(Limit::Usd),
        _ => None,
    }
}

/// Downloads the invoice `id` as a PDF into `path`, a new file of mode 0600, and prints
/// `{id, output_file, bytes}`. The PDF never reaches stdout, so it cannot be printed to a
/// terminal. `path` was checked at parse time to hold nothing; the file is created only
/// after the PDF arrived, so a failed request leaves nothing behind.
fn download(ctx: &Context<'_>, client: &hippius::Client, id: u64, path: &Path) -> Result<Printed> {
    let request = Request::get(format!("/billing/invoices/{id}/pdf/"))
        .accept("application/pdf")
        .about(format!("download the invoice `{id}`"))
        .listed_by("hipc billing invoices list");
    print_with(ctx, client, &request, Gate::Open, |response| {
        let pdf = response.pdf()?;
        write_output_file(path, pdf)?;
        Ok(json!({"id": id, "output_file": path.to_string_lossy(), "bytes": pdf.len()}))
    })
}

/// The four plan lists `hipc billing subscriptions` reads, one per service, in order.
const LEGS: [(&str, &str); 4] = [
    ("drive", "/drive/subscription/"),
    ("s3", "/s3/subscription/"),
    ("hub", "/registry/subscriptions/"),
    ("stripe", "/billing/stripe/active-subscription/"),
];

/// Reads every plan source and lists them as rows `{service, subscription}`, each with the
/// server's body unchanged.
///
/// Three of the four routes cannot act in another account, so with a membership selected
/// their route class refuses the command before anything is sent. When some legs fail, the
/// rows that succeeded are still printed and the command exits 1 with `partial_failure`.
/// When every leg fails, the first failure is the error, because then nothing is partial.
fn subscriptions(ctx: &Context<'_>, client: &hippius::Client) -> Result<Printed> {
    for (_, path) in LEGS {
        client.url(&Request::get(path))?;
    }
    let mut rows = Vec::new();
    let mut dry_runs = Vec::new();
    let mut failures = Vec::new();
    let mut failed = Vec::new();
    let mut first_error = None;
    for (service, path) in LEGS {
        let error = match ctx.call(
            client,
            &Request::get(path).about(format!("get the `{service}` plans")),
            Gate::Open,
        ) {
            Ok(Step::Sent(response)) => match response.document() {
                Ok(body) => {
                    rows.push(json!({ "service": service, "subscription": body }));
                    continue;
                }
                Err(error) => error,
            },
            Ok(Step::DryRun(doc)) => {
                dry_runs.push(doc);
                continue;
            }
            Err(error) => error,
        };
        failures.push(failed_part("service", service, &error));
        failed.push(phrase::quote(service));
        first_error.get_or_insert(error);
    }
    if !dry_runs.is_empty() {
        return Ok(Printed::Document(dry_run_requests(dry_runs)));
    }
    match first_error {
        None => Ok(Printed::Document(hippius::envelope(rows))),
        Some(error) if rows.is_empty() => Err(error),
        Some(_) => {
            let mut done = hippius::envelope(rows);
            if let Value::Object(map) = &mut done {
                map.insert("partial_failure".to_owned(), Value::Bool(true));
            }
            Err(Error::partial_failure(
                format!(
                    "cannot read the plans of {}: the list holds the other {}",
                    phrase::join(&failed, "and"),
                    phrase::count((LEGS.len() - failed.len()) as u64, "service")
                ),
                done,
                Value::Array(failures),
                SideEffects::None,
            ))
        }
    }
}
