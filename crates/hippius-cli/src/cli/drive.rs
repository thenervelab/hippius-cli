//! The grammar of `hipc drive`.

use clap::{Args, Subcommand, ValueEnum};

use crate::cli::{ListArgs, PlanSetting, examples, history_limit, intent_id};

#[derive(Debug, Subcommand)]
pub(crate) enum Drive {
    /// The Drive plan catalog; needs no token
    #[command(after_help = examples(&[
        "hipc drive plans",
    ]))]
    Plans,
    /// Read, buy, change, or cancel the Drive plan, and read its history
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc drive subscription get",
        "hipc drive subscription subscribe --plan solo --yes",
    ]))]
    Subscription(DriveSubscription),
}

#[derive(Debug, Subcommand)]
pub(crate) enum DriveSubscription {
    /// The Drive plan, or {"active": false} on Free
    ///
    /// next_charge_unix_day counts days since 1970-01-01 UTC, as the server sends it; hipc adds
    /// next_charge_date beside it, the same day as YYYY-MM-DD.
    #[command(after_help = examples(&[
        "hipc drive subscription get",
    ]))]
    Get,
    /// Buy a Drive plan; spends money: needs --yes
    ///
    /// Paid from the balance by default, which buys it now. --pay-from card starts a Stripe
    /// checkout and prints its URL: nothing is bought until it is paid. A wallet-only (12-word key)
    /// account has no Free plan. An account on a paid plan switches to another with hipc drive
    /// subscription change --plan.
    #[command(after_help = examples(&[
        "hipc drive subscription subscribe --plan solo --yes",
        "hipc drive subscription subscribe --plan duo --period annual --yes",
        "hipc drive subscription subscribe --plan solo --pay-from card --yes",
    ]))]
    Subscribe(DriveSubscribe),
    /// Switch the Drive plan, how its renewals are paid, or its overage cap; spends money:
    /// needs --yes
    ///
    /// Takes one of --plan, --pay-from, and --storage-cap. A plan switch is paid from the
    /// balance.
    #[command(after_help = examples(&[
        "hipc drive subscription change --plan duo --yes",
        "hipc drive subscription change --pay-from card --yes",
        "hipc drive subscription change --storage-cap default --yes",
    ]))]
    Change(DriveChange),
    /// Cancel the Drive plan; needs --yes
    ///
    /// The plan stays active until the end of the period paid for, and nothing is refunded.
    /// Then a Google, GitHub, or Apple account goes back to Free Drive. A wallet-only (12-word
    /// key) account has no Free plan, so it is left with no Drive plan.
    #[command(after_help = examples(&[
        "hipc drive subscription cancel --yes",
    ]))]
    Cancel,
    /// The progress of a card checkout: pending, paid, then fulfilled, or failed
    ///
    /// The plan is active only at fulfilled. Owner only.
    #[command(after_help = examples(&[
        "hipc drive subscription checkout pi_3Nabc",
    ]))]
    Checkout {
        /// The intent_id the card subscribe printed
        #[arg(value_parser = intent_id)]
        intent_id: String,
    },
    /// Every event of the Drive plan, newest first: purchases, renewals, changes, cancels,
    /// and refunds, one page at a time
    ///
    /// The token's own account only.
    #[command(mut_arg("limit", history_limit))]
    #[command(after_help = examples(&[
        "hipc drive subscription history",
        "hipc drive subscription history --limit 20",
    ]))]
    History(ListArgs),
}

#[derive(Debug, Args)]
pub(crate) struct DriveSubscribe {
    /// The plan code: solo (Starter), duo (Plus), max, or scale
    #[arg(long, value_enum)]
    pub(crate) plan: DrivePlan,
    /// The billing period
    ///
    /// Without it the server uses monthly.
    #[arg(long, value_enum)]
    pub(crate) period: Option<Period>,
    /// Pay from the balance (credits), or start a card checkout (card)
    ///
    /// Drive offers the card only to an account with no Drive plan at all, such as a wallet-only
    /// (12-word key) account. A Google, GitHub, or Apple account starts on Free Drive, so its
    /// move to a paid plan, like every later change, is paid from the balance, and a card
    /// subscribe is refused.
    #[arg(long, value_enum, default_value_t = PayFrom::Credits)]
    pub(crate) pay_from: PayFrom,
}

/// Swagger `drive_subscription_partial_update` separates a plan change from a funding switch
/// ("Either a plan change (`plan`, optional `period`) or a funding switch (`funding`)"), and
/// `storage_cap_bytes` goes "without `plan`"; whether `funding` and `storage_cap_bytes` may
/// share one PATCH is not documented, so one kind of change is taken at a time.
#[derive(Debug, Args)]
#[command(group(
    clap::ArgGroup::new("change").required(true).args(["plan", "pay_from", "storage_cap"])
))]
pub(crate) struct DriveChange {
    /// The plan code to switch to, paid from the balance
    #[arg(long, value_enum)]
    pub(crate) plan: Option<DrivePlan>,
    /// The billing period, with --plan
    ///
    /// Without it the server keeps its default.
    // `requires` alone let `--period annual --pay-from card` through (clap 4.6.7, checked by
    // the e2e test), so the conflicts name the other two kinds of change.
    #[arg(
        long,
        value_enum,
        requires = "plan",
        conflicts_with_all = ["pay_from", "storage_cap"]
    )]
    pub(crate) period: Option<Period>,
    #[command(flatten)]
    pub(crate) setting: PlanSetting,
}

/// The Drive plan codes the API sells. Free is the absence of a plan.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum DrivePlan {
    Solo,
    Duo,
    Max,
    Scale,
}

impl DrivePlan {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Solo => "solo",
            Self::Duo => "duo",
            Self::Max => "max",
            Self::Scale => "scale",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum Period {
    Monthly,
    Annual,
}

impl Period {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Monthly => "monthly",
            Self::Annual => "annual",
        }
    }
}

/// How a plan is paid.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum PayFrom {
    /// The account balance: bought now
    Credits,
    /// A Stripe checkout: bought once the checkout is paid
    Card,
}

impl PayFrom {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Credits => "credits",
            Self::Card => "card",
        }
    }

    /// How a purchase is paid, as the confirmation prompt says it.
    pub(crate) fn how(self) -> &'static str {
        match self {
            Self::Credits => "it is paid from the balance now",
            Self::Card => "it is paid through a card checkout, and nothing is bought until then",
        }
    }
}
