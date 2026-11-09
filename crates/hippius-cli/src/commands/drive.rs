//! `hipc drive`: the Drive catalog and the Drive plan.
//!
//! Drive is not part of a shared account, so every subscription route refuses a
//! membership before anything is sent. The catalog is public: with a membership it is sent
//! without the account header. Drive file bytes stay in the Drive apps and SDK.

use hippius::{Method, Request, Result};
use serde_json::{Map, Value};

use crate::cli::{Drive, DriveChange, DriveSubscribe, DriveSubscription};
use crate::commands::{Printed, list, plan_setting, print_envelope, print_with, subscribed};
use crate::context::{self, Context, Gate};

pub(crate) fn run(ctx: &Context<'_>, command: &Drive) -> Result<Printed> {
    let config = context::config()?;
    match command {
        Drive::Plans => {
            // The catalog needs no token, and sends one only when it is configured.
            print_envelope(
                ctx,
                &ctx.client(&config)?,
                &Request::get("/drive/plans/").about("list the Drive plans"),
            )
        }
        Drive::Subscription(command) => {
            let client = ctx.authed_client(&config)?;
            subscription(ctx, &client, command)
        }
    }
}

fn subscription(
    ctx: &Context<'_>,
    client: &hippius::Client,
    command: &DriveSubscription,
) -> Result<Printed> {
    let (request, gate) = match command {
        DriveSubscription::Get => (
            Request::get("/drive/subscription/").about("get the Drive plan"),
            Gate::Open,
        ),
        DriveSubscription::Subscribe(args) => (
            subscribe_request(args),
            Gate::Confirm {
                consequence: Some(args.pay_from.how()),
            },
        ),
        DriveSubscription::Change(args) => change_request(args)?,
        DriveSubscription::Cancel => (
            Request::new(Method::Delete, "/drive/subscription/").about("cancel the Drive plan"),
            Gate::Confirm {
                // A wallet-only account has no Free plan to go back to.
                consequence: Some(CANCELLED),
            },
        ),
        DriveSubscription::Checkout { intent_id } => (
            Request::get(format!("/drive/subscription/checkout/{intent_id}/"))
                .about(format!("get the card checkout `{intent_id}`")),
            Gate::Open,
        ),
        DriveSubscription::History(args) => {
            let request =
                Request::get("/drive/subscription/history/").about("list the Drive plan history");
            return list::run(ctx, client, &request, args);
        }
    };
    print_with(ctx, client, &request, gate, |response| match command {
        DriveSubscription::Subscribe(_) => subscribed(ctx, &response, "drive"),
        // History never gets here: it returned from the match above, through `list::run`.
        DriveSubscription::Get => response.document().map(|mut plan| {
            add_next_charge_date(&mut plan);
            plan
        }),
        DriveSubscription::History(_)
        | DriveSubscription::Change(_)
        | DriveSubscription::Cancel
        | DriveSubscription::Checkout { .. } => response.document(),
    })
}

/// `POST /api/drive/subscription/ {"plan", "payment", "period"?}`.
fn subscribe_request(args: &DriveSubscribe) -> Request {
    let mut body = Map::new();
    body.insert("plan".to_owned(), args.plan.code().into());
    body.insert("payment".to_owned(), args.pay_from.code().into());
    if let Some(period) = args.period {
        body.insert("period".to_owned(), period.code().into());
    }
    Request::new(Method::Post, "/drive/subscription/")
        .json(&Value::Object(body))
        .about(format!("buy the Drive plan `{}`", args.plan.code()))
}

/// `PATCH /api/drive/subscription/` with `{"plan", "period"?}`, or with the one setting
/// [`plan_setting`] sends. The PATCH has no `payment` field: plan changes are paid from the
/// balance.
fn change_request(args: &DriveChange) -> Result<(Request, Gate<'static>)> {
    let Some(plan) = args.plan else {
        return plan_setting("/drive/subscription/", "Drive", &args.setting);
    };
    let mut body = Map::new();
    body.insert("plan".to_owned(), plan.code().into());
    if let Some(period) = args.period {
        body.insert("period".to_owned(), period.code().into());
    }
    let request = Request::new(Method::Patch, "/drive/subscription/")
        .json(&Value::Object(body))
        .about(format!("switch the Drive plan to `{}`", plan.code()));
    let gate = Gate::Confirm {
        consequence: Some("it is paid from the balance"),
    };
    Ok((request, gate))
}

/// What a Drive plan cancel leaves, for its question and `hipc api`'s.
pub(crate) const CANCELLED: &str = "the plan stays until the end of the period paid for, with \
                                    no refund, and then a Google, GitHub, or Apple account goes \
                                    back to Free Drive, and a wallet-only account is left with \
                                    no Drive plan";

/// Adds `next_charge_date`, the UTC date as `YYYY-MM-DD`, beside every integer
/// `next_charge_unix_day` in `value`, at any depth: the server counts days since 1970-01-01,
/// which a reader cannot read at a glance. The raw field stays as sent, and a
/// `next_charge_date` the server sends is kept. A day outside 1970-01-01 to 9999-12-31 gets no
/// date: it is no charge day, and the arithmetic stays far from overflow.
fn add_next_charge_date(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            let day = fields
                .get("next_charge_unix_day")
                .and_then(Value::as_i64)
                .filter(|day| (0..=LAST_DAY).contains(day));
            if let Some(day) = day
                && !fields.contains_key("next_charge_date")
            {
                fields.insert("next_charge_date".to_owned(), date_of_unix_day(day).into());
            }
            fields.values_mut().for_each(add_next_charge_date);
        }
        Value::Array(items) => items.iter_mut().for_each(add_next_charge_date),
        _ => {}
    }
}

/// 9999-12-31, the last day [`add_next_charge_date`] dates.
const LAST_DAY: i64 = 2_932_896;

/// The proleptic Gregorian date `day` days after 1970-01-01, as `YYYY-MM-DD` (Howard
/// Hinnant's `civil_from_days`).
fn date_of_unix_day(day: i64) -> String {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    /// The date of a day count, checked against dates worked out by hand, a leap day and
    /// the turn of a century included.
    #[test]
    fn a_unix_day_is_its_utc_date() {
        for (day, date) in [
            (0, "1970-01-01"),
            (-1, "1969-12-31"),
            (super::LAST_DAY, "9999-12-31"),
            (59, "1970-03-01"),
            (11_016, "2000-02-29"),
            (20_784, "2026-11-27"),
            (47_541, "2100-03-01"),
        ] {
            assert_eq!(super::date_of_unix_day(day), date, "{day}");
        }
    }

    /// The date goes beside every integer day count, at any depth, and nothing else changes.
    #[test]
    fn the_date_goes_beside_each_day_count() {
        let mut plan = json!({
            "active": true,
            "next_charge_unix_day": 20_784,
            "items": [
                {"next_charge_unix_day": 0},
                {"next_charge_unix_day": "x"},
                {"next_charge_unix_day": -1},
                {"next_charge_unix_day": i64::MAX},
            ],
            "kept": {"next_charge_unix_day": 1, "next_charge_date": "theirs"},
        });
        super::add_next_charge_date(&mut plan);
        assert_eq!(
            plan,
            json!({
                "active": true,
                "next_charge_unix_day": 20_784,
                "next_charge_date": "2026-11-27",
                "items": [
                    {"next_charge_unix_day": 0, "next_charge_date": "1970-01-01"},
                    {"next_charge_unix_day": "x"},
                    {"next_charge_unix_day": -1},
                    {"next_charge_unix_day": i64::MAX},
                ],
                "kept": {"next_charge_unix_day": 1, "next_charge_date": "theirs"},
            })
        );
    }
}
