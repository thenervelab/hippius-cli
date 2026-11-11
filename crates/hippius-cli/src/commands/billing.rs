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

use hippius::routes::billing::{self, AutoReloadState, AutoReloadUpdateBody};
use hippius::routes::{self, Route};
use hippius::{Code, Error, Failure, Request, Result, Selection, SideEffects, phrase, view_of};
use serde_json::{Map, Value, json};

use crate::cli::{
    AutoReload, AutoReloadUpdate, Billing, BillingProfile, Invoices, ProfileUpdate, ReloadCap,
    TopUp, TopUpMethod, Usd, usd,
};
use crate::commands::credits::{AmountPath, shown_in_credits};
use crate::commands::files::write_output_file;
use crate::commands::{Printed, failed_part, gamma, halpha, list, print_body, print_with};
use crate::context::{Context, Gate, Write};

pub(crate) fn run(ctx: &Context<'_>, command: &Billing) -> Result<Printed> {
    // The config file is read only once a command needs a client, after its argv checks, such
    // as the top-up amount (tenet 5).
    let client = || ctx.load_authed_client();
    match command {
        Billing::Balance => print_body(
            ctx,
            &client()?,
            &Request::to(&billing::BALANCE, []).about("get the balance"),
            Gate::Open,
        ),
        Billing::Stripe => print_body(
            ctx,
            &client()?,
            &Request::to(&billing::STRIPE_SUBSCRIPTION, [])
                .about("get the legacy Stripe top-up plan"),
            Gate::Open,
        ),
        Billing::Transactions(args) => list::run(
            ctx,
            &client()?,
            &Request::to(&billing::TRANSACTIONS, []).about("list the transactions"),
            args,
        ),
        Billing::Subscriptions => subscriptions(ctx, &client()?),
        Billing::Summary => print_in_credits(
            ctx,
            &client()?,
            &Request::to(&routes::compute::BILLING_SUMMARY, []).about("get the compute burn rate"),
            SUMMARY_AMOUNTS,
        ),
        Billing::Usage { month } => {
            let request = Request::to(&routes::compute::USAGE, []);
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
            &Request::to(&billing::INVOICES, []).about("list the invoices"),
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
            &Request::to(&billing::AUTO_RELOAD_HISTORY, [])
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
/// A hAlpha top-up has no rail, so it is `None`: no caller can send hAlpha to a checkout
/// route, and [`start_top_up`] sends it to [`halpha_top_up`] instead.
const fn rail(method: TopUpMethod) -> Option<(&'static str, &'static Route<0>, Usd, Usd)> {
    match method {
        TopUpMethod::Card => Some((
            "card",
            &billing::CARD_CHECKOUT,
            Usd::from_cents(10 * 100),
            Usd::from_cents(10_000 * 100),
        )),
        TopUpMethod::Bitcoin => Some((
            "bitcoin",
            &billing::BITCOIN_INVOICE,
            Usd::from_cents(20 * 100),
            Usd::from_cents(100_000 * 100),
        )),
        TopUpMethod::Halpha => None,
    }
}

/// A hAlpha top-up: `--alpha`, never `--amount`, and a retry's key and minimum together.
fn halpha_top_up(ctx: &Context<'_>, top_up: &TopUp) -> Result<Printed> {
    match (top_up.amount, top_up.alpha) {
        (Some(_), _) => Err(Error::usage(
            "`--amount` is US dollars, for a `card` or `bitcoin` top-up; a `halpha` top-up \
             takes `--alpha`",
            "pass the hAlpha to spend as `--alpha <HALPHA>` in place of `--amount`",
        )),
        (None, None) => Err(Error::usage(
            "a `halpha` top-up needs `--alpha`, the hAlpha to spend",
            "pass `--alpha <HALPHA>`, such as `--alpha 1.5`",
        )),
        (None, Some(alpha)) => {
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
            halpha::top_up(ctx, alpha, retry, top_up.wait_timeout)
        }
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
    let Some((name, route, least, most)) = rail(top_up.method) else {
        return halpha_top_up(ctx, top_up);
    };
    let amount = match (top_up.amount, top_up.alpha) {
        (_, Some(_)) => {
            return Err(Error::usage(
                format!(
                    "`--alpha` is hAlpha, for a `halpha` top-up; a `{name}` top-up takes \
                     `--amount` in US dollars"
                ),
                "pass the US dollars as `--amount <USD>` in place of `--alpha`",
            ));
        }
        (None, None) => {
            return Err(Error::usage(
                format!("a `{name}` top-up needs `--amount`, in US dollars"),
                "pass `--amount <USD>`, such as `--amount 50`",
            ));
        }
        (Some(amount), None) => amount,
    };
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
    let request = Request::to(route, [])
        .json(&billing::TopUpBody {
            amount: &amount.to_string(),
        })
        .about(format!("start a {name} top-up of `{amount}` USD"));
    require_billing_account(client, &request, "a top-up")?;
    let gate = Gate::Confirm {
        consequence: Some("nothing is paid until the checkout is paid"),
    };
    print_with(ctx, client, &request, gate, |response| {
        let body = response.document()?;
        // A person may paste the note into a log, so the URL loses its query values there
        // (tenet 9); the whole URL is on stdout.
        let at = view_of::<billing::Checkout>(&body)
            .checkout_url
            .map(|url| format!(" at {}", phrase::quote(hippius::without_query_values(&url))))
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
    Request::to(&billing::PROFILE, []).about("get the billing profile")
}

fn read_auto_reload() -> Request {
    Request::to(&billing::AUTO_RELOAD, []).about("get the auto-reload setting")
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
/// selectors, or is left out when one is not printable ASCII ([`Context::next_command`]).
fn card_setup(ctx: &Context<'_>, client: &hippius::Client) -> Result<Printed> {
    let request = Request::to(&billing::AUTO_RELOAD_CARD_SETUP, [])
        .json(&billing::CardSetupBody {})
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
    let given = args
        .fields()
        .into_iter()
        .filter_map(|(key, given)| given.map(|_| key));
    let named = phrase::ticked(given, "and");
    let body = billing::ProfileUpdateBody {
        company_name: args.company_name.as_deref(),
        address_line1: args.address_line1.as_deref(),
        address_line2: args.address_line2.as_deref(),
        city: args.city.as_deref(),
        postal_code: args.postal_code.as_deref(),
        region: args.region.as_deref(),
        country: args.country.as_deref(),
        tax_id: args.tax_id.as_deref(),
        billing_email: args.billing_email.as_deref(),
    };
    let request = Request::to(&billing::PROFILE_UPDATE, [])
        .json(&body)
        .about(format!("update the billing profile's {named}"));
    require_billing_account(&client, &request, "a billing profile update")?;
    let check = |error| check_first(error, "hipc billing profile get");
    let Some(email) = args.billing_email.as_deref() else {
        return print_body(ctx, &client, &request, Gate::Open).map_err(check);
    };
    let checked = match ctx.check_write(&client, &request)? {
        Write::DryRun(_) => {
            return ctx
                .dry_run_of(&client, &[&read_profile(), &request])
                .map(Printed::Document);
        }
        Write::Checked(checked) => checked,
    };
    let current = match ctx.send(&client, &read_profile()) {
        Ok(response) => view_of::<billing::Profile>(&response.document()?)
            .billing_email
            .filter(|email| !email.is_empty()),
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
    let given = ReloadGiven::of(args);
    let draft = ReloadField::ALL
        .iter()
        .map(|&field| {
            let shown = || Value::from(format!("<the setting's {}>", field.key()));
            (
                field.key().to_owned(),
                given.value(field).unwrap_or_else(shown),
            )
        })
        .collect::<Map<String, Value>>();
    let save = "save the auto-reload setting";
    let draft = Request::to(&billing::AUTO_RELOAD_UPDATE, [])
        .json(&draft)
        .about(save);
    require_billing_account(&client, &draft, "an auto-reload update")?;
    let checked = match ctx.check_write(&client, &draft)? {
        Write::DryRun(_) => {
            return ctx
                .dry_run_of(&client, &[&read_auto_reload(), &draft])
                .map(Printed::Document);
        }
        Write::Checked(checked) => checked,
    };
    let read = ctx.send(&client, &read_auto_reload())?.document()?;
    let current: billing::AutoReload = view_of(&read);
    // Swagger: `available: false`, and nothing else, while the feature is off on the server.
    // No flag can fix that, so it is said before any field is checked.
    if current.available == Some(false) {
        return Err(Error::new(
            Failure::Conflict,
            "auto-reload is not available on this server: the setting reads `available` \
             `false`",
        ));
    }
    let body = given.kept_from(&current)?;
    let request = Request::to(&billing::AUTO_RELOAD_UPDATE, []).json(&body);
    let request = match charges_more(&read, &current, &body) {
        Some((action, consequence)) => {
            let request = request.about(action);
            ctx.confirm_write(checked, &request.action(), Some(consequence))?;
            request
        }
        None => request.about(save),
    };
    ctx.send(&client, &request)
        .and_then(|response| response.document())
        .map(Printed::Document)
        .map_err(|error| check_first(error, "hipc billing auto-reload get"))
}

/// A field of the auto-reload setting: its key in the route's body, and the flag that sets
/// it, which a hint names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReloadField {
    Enabled,
    Threshold,
    Target,
    MonthlyCap,
}

impl ReloadField {
    /// Every field, in the order the body sends them.
    const ALL: [Self; 4] = [
        Self::Enabled,
        Self::Threshold,
        Self::Target,
        Self::MonthlyCap,
    ];

    const fn key(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Threshold => "threshold",
            Self::Target => "target",
            Self::MonthlyCap => "monthly_cap",
        }
    }

    const fn flag(self) -> &'static str {
        match self {
            Self::Enabled => "--enabled",
            Self::Threshold => "--threshold",
            Self::Target => "--target",
            Self::MonthlyCap => "--monthly-cap",
        }
    }
}

/// The auto-reload fields the flags give, each `None` when its flag is not given. An amount
/// is a string of dollars to the cent, as the route takes it, and a `monthly_cap` of `None`
/// is no limit, sent as `null`.
#[derive(Debug)]
#[expect(
    clippy::option_option,
    reason = "a cap not given is read from the setting, and one given as none is sent as `null`"
)]
struct ReloadGiven {
    enabled: Option<bool>,
    threshold: Option<String>,
    target: Option<String>,
    monthly_cap: Option<Option<String>>,
}

impl ReloadGiven {
    fn of(args: &AutoReloadUpdate) -> Self {
        Self {
            enabled: args.enabled,
            threshold: args.threshold.map(|amount| amount.to_string()),
            target: args.target.map(|amount| amount.to_string()),
            monthly_cap: args.monthly_cap.map(|cap| match cap {
                ReloadCap::Usd(amount) => Some(amount.to_string()),
                ReloadCap::None => None,
            }),
        }
    }

    /// The value `field` is given, as the body sends it.
    fn value(&self, field: ReloadField) -> Option<Value> {
        match field {
            ReloadField::Enabled => self.enabled.map(Value::from),
            ReloadField::Threshold => self.threshold.clone().map(Value::from),
            ReloadField::Target => self.target.clone().map(Value::from),
            ReloadField::MonthlyCap => self
                .monthly_cap
                .clone()
                .map(|cap| cap.map_or(Value::Null, Value::from)),
        }
    }

    /// The whole setting to send: each field given, and each other one as `current`, the
    /// setting read, holds it in the shape the `PUT` takes (`enabled` a boolean, an amount a
    /// string, `monthly_cap` a string or `null`; a setting never saved has `null` amounts).
    ///
    /// # Errors
    ///
    /// `usage` naming every field that is neither given nor held so, and its flag.
    fn kept_from(self, current: &billing::AutoReload) -> Result<AutoReloadUpdateBody> {
        let enabled = self.enabled.or(current.enabled);
        let threshold = self.threshold.or_else(|| current.threshold.clone());
        let target = self.target.or_else(|| current.target.clone());
        let monthly_cap = self.monthly_cap.or_else(|| current.monthly_cap.clone());
        let (Some(enabled), Some(threshold), Some(target), Some(monthly_cap)) =
            (enabled, &threshold, &target, &monthly_cap)
        else {
            let missing: Vec<ReloadField> = [
                (ReloadField::Enabled, enabled.is_none()),
                (ReloadField::Threshold, threshold.is_none()),
                (ReloadField::Target, target.is_none()),
                (ReloadField::MonthlyCap, monthly_cap.is_none()),
            ]
            .into_iter()
            .filter_map(|(field, missing)| missing.then_some(field))
            .collect();
            return Err(not_kept(&missing));
        };
        Ok(AutoReloadUpdateBody {
            enabled,
            threshold: threshold.clone(),
            target: target.clone(),
            monthly_cap: monthly_cap.clone(),
        })
    }
}

/// The refusal of an update that would have to guess the `missing` fields.
fn not_kept(missing: &[ReloadField]) -> Error {
    let quoted: Vec<String> = missing
        .iter()
        .map(|field| format!("`{}`", field.key()))
        .collect();
    let flags: Vec<String> = missing
        .iter()
        .map(|field| format!("`{}`", field.flag()))
        .collect();
    Error::usage(
        format!(
            "the auto-reload setting holds no {} hipc can send back unchanged",
            phrase::join(&quoted, "or")
        ),
        format!(
            "pass {} to set {}",
            phrase::join(&flags, "and"),
            if flags.len() == 1 { "it" } else { "them" }
        ),
    )
}

/// What the auto-reload setting does once `body` is saved, as a question names it.
fn reload_setting(body: &AutoReloadUpdateBody) -> String {
    let cap = match &body.monthly_cap {
        Some(cap) => format!("a monthly cap of {} USD", phrase::quote(cap)),
        None => "no monthly cap".to_owned(),
    };
    format!(
        "charge the card when the balance falls below {} USD, back up to {} USD, with {cap}",
        phrase::quote(&body.threshold),
        phrase::quote(&body.target)
    )
}

/// The question an auto-reload update asks, as its action and consequence, or `None` when it
/// cannot charge more than the setting in force, `current` (read as `read`), does.
///
/// Nothing is asked when `body` leaves auto-reload off. Leaving it on is asked about when the
/// setting in force is off; when its state is anything but `armed`, since a save closes a
/// failure episode and the server checks the balance at once (swagger), which may charge the
/// card now, during a `cooldown` or an `in_flight` reload too; when it raises the threshold,
/// the target, or the monthly cap, `null` being no cap and so the highest; and when any of
/// those cannot be read as the fields compared, so a shape hipc does not know never lets a
/// raise through unasked.
fn charges_more(
    read: &Value,
    current: &billing::AutoReload,
    body: &AutoReloadUpdateBody,
) -> Option<(String, &'static str)> {
    if !body.enabled {
        return None;
    }
    let setting = reload_setting(body);
    let on = "the card is then charged whenever the balance falls below the threshold, and the \
              balance is checked at once";
    // The question quotes what the server sent, whatever its type.
    let unknown = |key: &str| {
        let shown = read
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
    match current.enabled {
        Some(true) => {}
        Some(false) => {
            return Some((format!("turn on auto-reload, to {setting}"), on));
        }
        None => return unknown(ReloadField::Enabled.key()),
    }
    match &current.state {
        Some(AutoReloadState::Armed) => {}
        Some(
            state @ (AutoReloadState::Off
            | AutoReloadState::DisabledFailures
            | AutoReloadState::RetryWait
            | AutoReloadState::InFlight
            | AutoReloadState::Cooldown
            | AutoReloadState::MonthlyCapReached),
        ) => {
            return Some((
                format!(
                    "save auto-reload on while its state is `{}`, to {setting}",
                    state.as_str()
                ),
                on,
            ));
        }
        Some(AutoReloadState::Other(_)) | None => return unknown("state"),
    }
    let mut raised = Vec::new();
    let amounts = [
        (
            ReloadField::Threshold,
            "threshold",
            amount(Some(&body.threshold)),
            current
                .threshold
                .as_deref()
                .and_then(|old| amount(Some(old))),
        ),
        (
            ReloadField::Target,
            "target",
            amount(Some(&body.target)),
            current.target.as_deref().and_then(|old| amount(Some(old))),
        ),
        (
            ReloadField::MonthlyCap,
            "monthly cap",
            amount(body.monthly_cap.as_deref()),
            current
                .monthly_cap
                .as_ref()
                .and_then(|old| amount(old.as_deref())),
        ),
    ];
    for (field, name, new, old) in amounts {
        match (new, old) {
            (Some(new), Some(old)) if new > old => raised.push(name),
            (Some(_), Some(_)) => {}
            _ => return unknown(field.key()),
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

/// An amount the setting holds: `None` there is no cap, and a text that is not an amount to
/// the cent is no amount at all. Only `monthly_cap` passes `None`: a `null` threshold or
/// target in force is a setting never saved, which a question must not read as the highest,
/// so its caller never asks for it.
fn amount(text: Option<&str>) -> Option<Limit> {
    match text {
        None => Some(Limit::NoCap),
        Some(text) => usd(text).ok().map(Limit::Usd),
    }
}

/// Downloads the invoice `id` as a PDF into `path`, a new file of mode 0600, and prints
/// `{id, output_file, bytes}`. The PDF never reaches stdout, so it cannot be printed to a
/// terminal. `path` was checked at parse time to hold nothing; the file is created only
/// after the PDF arrived, so a failed request leaves nothing behind.
fn download(ctx: &Context<'_>, client: &hippius::Client, id: u64, path: &Path) -> Result<Printed> {
    let request = Request::to(&billing::INVOICE_PDF, [&id])
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
const LEGS: [(&str, &Route<0>); 4] = [
    ("drive", &routes::drive::SUBSCRIPTION),
    ("s3", &routes::s3::SUBSCRIPTION),
    ("hub", &routes::hub::SUBSCRIPTIONS),
    ("stripe", &billing::STRIPE_SUBSCRIPTION),
];

/// Reads every plan source and lists them as rows `{service, subscription}`, each with the
/// server's body unchanged.
///
/// Three of the four routes cannot act in another account, so with a membership selected
/// their route class refuses the command before anything is sent. When some legs fail, the
/// rows that succeeded are still printed and the command exits 1 with `partial_failure`.
/// When every leg fails, the first failure is the error, because then nothing is partial.
fn subscriptions(ctx: &Context<'_>, client: &hippius::Client) -> Result<Printed> {
    for (_, route) in LEGS {
        client.url(&Request::to(route, []))?;
    }
    let reads = LEGS.map(|(service, route)| {
        (
            service,
            Request::to(route, []).about(format!("get the `{service}` plans")),
        )
    });
    if ctx.dry_run() {
        let requests: Vec<&Request> = reads.iter().map(|(_, request)| request).collect();
        return ctx.dry_run_of(client, &requests).map(Printed::Document);
    }
    let mut rows = Vec::new();
    let mut failures = Vec::new();
    let mut failed = Vec::new();
    let mut first_error = None;
    for (service, request) in &reads {
        let error = match ctx
            .send(client, request)
            .and_then(|response| response.document())
        {
            Ok(body) => {
                rows.push(json!({ "service": service, "subscription": body }));
                continue;
            }
            Err(error) => error,
        };
        failures.push(failed_part("service", service, &error));
        failed.push(phrase::quote(service));
        first_error.get_or_insert(error);
    }
    match first_error {
        None => Ok(Printed::List(hippius::envelope(rows))),
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
            )
            .with_stdout_as_list())
        }
    }
}
