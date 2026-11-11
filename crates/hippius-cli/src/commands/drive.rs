//! `hipc drive`: the Drive catalog and the Drive plan.
//!
//! Drive is not part of a shared account, so every subscription route refuses a
//! membership before anything is sent. The catalog is public: with a membership it is sent
//! without the account header. Drive file bytes stay in the Drive apps and SDK.

use hippius::routes::drive;
use hippius::{Request, Result};
use serde_json::Value;

use crate::cli::{Drive, DriveChange, DriveSubscribe, DriveSubscription};
use crate::commands::{
    Printed, SettingChange, list, plan_setting, print_envelope, print_with, subscribed,
};
use crate::context::{self, Context, Gate};

pub(crate) fn run(ctx: &Context<'_>, command: &Drive) -> Result<Printed> {
    let config = context::config()?;
    match command {
        Drive::Plans => {
            // The catalog needs no token, and sends one only when it is configured.
            print_envelope(
                ctx,
                &ctx.client(&config)?,
                &Request::to(&drive::PLANS, []).about("list the Drive plans"),
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
            Request::to(&drive::SUBSCRIPTION, []).about("get the Drive plan"),
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
            Request::to(&drive::SUBSCRIPTION_CANCEL, []).about("cancel the Drive plan"),
            Gate::Confirm {
                // A wallet-only account has no Free plan to go back to.
                consequence: Some(CANCELLED),
            },
        ),
        DriveSubscription::Checkout { intent_id } => (
            Request::to(&drive::CHECKOUT, [intent_id])
                .about(format!("get the card checkout `{intent_id}`")),
            Gate::Open,
        ),
        DriveSubscription::History(args) => {
            let request = Request::to(&drive::HISTORY, []).about("list the Drive plan history");
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
    let body = drive::SubscribeBody {
        plan: args.plan.code(),
        payment: args.pay_from.code(),
        period: args.period.map(crate::cli::Period::code),
    };
    Request::to(&drive::SUBSCRIBE, [])
        .json(&body)
        .about(format!("buy the Drive plan `{}`", args.plan.code()))
}

/// `PATCH /api/drive/subscription/` with `{"plan", "period"?}`, or with the one setting
/// [`plan_setting`] sends. The PATCH has no `payment` field: plan changes are paid from the
/// balance.
fn change_request(args: &DriveChange) -> Result<(Request, Gate<'static>)> {
    let Some(plan) = args.plan else {
        return plan_setting(
            &drive::SUBSCRIPTION_CHANGE,
            "Drive",
            &args.setting,
            setting_body,
        );
    };
    let body = drive::PlanChangeBody {
        plan: plan.code(),
        period: args.period.map(crate::cli::Period::code),
    };
    let request = Request::to(&drive::SUBSCRIPTION_CHANGE, [])
        .json(&body)
        .about(format!("switch the Drive plan to `{}`", plan.code()));
    let gate = Gate::Confirm {
        consequence: Some("it is paid from the balance"),
    };
    Ok((request, gate))
}

/// The Drive plan PATCH that keeps the plan and changes `change`.
fn setting_body(change: SettingChange) -> drive::SettingChangeBody<'static> {
    match change {
        SettingChange::Funding(funding) => drive::SettingChangeBody {
            funding: Some(funding),
            ..drive::SettingChangeBody::default()
        },
        SettingChange::StorageCap(cap) => drive::SettingChangeBody {
            storage_cap_bytes: Some(cap),
            ..drive::SettingChangeBody::default()
        },
    }
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
                fields.insert(
                    "next_charge_date".to_owned(),
                    hippius::date_of_unix_day(day).into(),
                );
            }
            fields.values_mut().for_each(add_next_charge_date);
        }
        Value::Array(items) => items.iter_mut().for_each(add_next_charge_date),
        _ => {}
    }
}

/// 9999-12-31, the last day [`add_next_charge_date`] dates.
const LAST_DAY: i64 = 2_932_896;

#[cfg(test)]
mod tests {
    use serde_json::json;

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
