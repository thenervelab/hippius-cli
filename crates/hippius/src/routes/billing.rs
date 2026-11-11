//! Credits, top-ups, invoices, the billing profile, auto-reload, the hAlpha purchase, and
//! the gamma conversion.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::routes::{Paging, Route};
use crate::view::{field, text_or_number};

routes! {
    /// The credit balance.
    BALANCE: 0 = Route::get("/api/billing/credits/balance/").scoped().billing();

    /// The credit transactions.
    TRANSACTIONS: 0 = Route::get("/api/billing/transactions/").own_account().paged(Paging::Numbered);

    /// The invoices.
    INVOICES: 0 = Route::get("/api/billing/invoices/").scoped().billing().paged(Paging::Numbered);

    /// One invoice as a PDF.
    INVOICE_PDF: 1 = Route::get("/api/billing/invoices/{invoice_id}/pdf/").scoped().billing();

    /// The billing profile.
    PROFILE: 0 = Route::get("/api/billing/profile/").scoped().billing();

    /// Replaces the billing profile.
    PROFILE_UPDATE: 0 = Route::put("/api/billing/profile/").scoped().billing();

    /// The auto-reload setting.
    AUTO_RELOAD: 0 = Route::get("/api/billing/auto-reload/").scoped().billing();

    /// Replaces the auto-reload setting.
    AUTO_RELOAD_UPDATE: 0 = Route::put("/api/billing/auto-reload/").scoped().billing();

    /// The auto-reload charges.
    AUTO_RELOAD_HISTORY: 0 = Route::get("/api/billing/auto-reload/history/").scoped().billing();

    /// Starts saving a card for auto-reload.
    AUTO_RELOAD_CARD_SETUP: 0 = Route::post("/api/billing/auto-reload/card-setup/")
        .scoped()
        .billing();

    /// The recurring card subscription.
    STRIPE_SUBSCRIPTION: 0 = Route::get("/api/billing/stripe/active-subscription/")
        .own_account()
        .beside_billing()
        .billing();

    /// Starts a card top-up.
    CARD_CHECKOUT: 0 = Route::post("/api/billing/stripe/create-checkout-session/")
        .scoped()
        .billing();

    /// Starts a bitcoin top-up.
    BITCOIN_INVOICE: 0 = Route::post("/api/billing/btcpay/create-invoice/").scoped().billing();

    /// The hAlpha price quote.
    NATIVE_QUOTE: 0 = Route::get("/api/billing/native/quote/").own_account();

    /// Buys credits with hAlpha.
    NATIVE_BUY: 0 = Route::post("/api/billing/native/buy/").own_account();

    /// The hAlpha purchases.
    NATIVE_PURCHASES: 0 = Route::get("/api/billing/native/purchases/").own_account();

    /// The gamma conversion setup.
    GAMMA: 0 = Route::get("/api/billing/gamma/").own_account();

    /// The gamma conversions.
    GAMMA_CONVERSIONS: 0 = Route::get("/api/billing/gamma/conversions/").own_account().paged(Paging::Numbered);

    /// One gamma conversion.
    GAMMA_CONVERSION: 1 = Route::get("/api/billing/gamma/conversions/{conversion_id}/")
        .own_account();

    /// Converts gamma to credits.
    GAMMA_CONVERSION_CREATE: 0 = Route::post("/api/billing/gamma/conversions/").own_account();

    /// The challenge a hotkey signs to be linked.
    GAMMA_HOTKEY_CHALLENGE: 0 = Route::post("/api/billing/gamma/hotkeys/challenge/").own_account();

    /// Links a hotkey.
    GAMMA_HOTKEY_CREATE: 0 = Route::post("/api/billing/gamma/hotkeys/").own_account();

    /// Unlinks a hotkey.
    GAMMA_HOTKEY_DELETE: 1 = Route::delete("/api/billing/gamma/hotkeys/{hotkey}/").own_account();
}

/// The body of [`CARD_CHECKOUT`] and [`BITCOIN_INVOICE`].
#[derive(Debug, Serialize)]
pub struct TopUpBody<'a> {
    /// US dollars to the cent, as a decimal string.
    pub amount: &'a str,
}

/// The body of [`PROFILE_UPDATE`]: only the fields given, so one left out keeps its value. An
/// empty string clears a field.
#[derive(Debug, Default, Serialize)]
pub struct ProfileUpdateBody<'a> {
    /// The company name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub company_name: Option<&'a str>,
    /// The first address line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address_line1: Option<&'a str>,
    /// The second address line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address_line2: Option<&'a str>,
    /// The city.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<&'a str>,
    /// The postal code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub postal_code: Option<&'a str>,
    /// The region or state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<&'a str>,
    /// The two-letter country code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<&'a str>,
    /// The tax id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tax_id: Option<&'a str>,
    /// Where the invoices and billing emails go.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub billing_email: Option<&'a str>,
}

/// The body of [`AUTO_RELOAD_UPDATE`], whole: swagger requires `enabled`, `threshold`, and
/// `target` in every `PUT`. Each amount is US dollars to the cent, a string.
#[derive(Debug, Serialize)]
pub struct AutoReloadUpdateBody {
    /// Whether the card is charged when the balance falls below `threshold`.
    pub enabled: bool,
    /// The balance below which the card is charged.
    pub threshold: String,
    /// The balance a charge brings the account back up to.
    pub target: String,
    /// The most charged in a month; `null` is no limit.
    pub monthly_cap: Option<String>,
}

/// The body of [`NATIVE_BUY`].
#[derive(Debug, Serialize)]
pub struct NativeBuyBody<'a> {
    /// The hAlpha spent, in planck (18 decimals), as a decimal string.
    pub alpha_amount: &'a str,
    /// The fewest credits the buy accepts, 18 decimals, as a decimal string.
    pub min_credits: &'a str,
}

/// The body of [`GAMMA_HOTKEY_CHALLENGE`].
#[derive(Debug, Serialize)]
pub struct HotkeyChallengeBody<'a> {
    /// The hotkey's SS58 address.
    pub hotkey: &'a str,
}

/// The body of [`GAMMA_HOTKEY_CREATE`].
#[derive(Debug, Serialize)]
pub struct HotkeyCreateBody<'a> {
    /// The challenge's nonce.
    pub nonce: &'a str,
    /// The hotkey's sr25519 signature of the challenge's message.
    pub signature: &'a str,
}

/// The body of [`GAMMA_CONVERSION_CREATE`].
#[derive(Debug, Serialize)]
pub struct GammaConversionBody<'a> {
    /// The hotkey whose deposit is burned.
    pub hotkey: &'a str,
    /// The gamma burned, as a decimal string.
    pub amount: &'a str,
}

/// What a route that starts a card checkout answers, as far as `hipc` decides on it: the
/// Drive and S3 subscribe, a top-up, and the auto-reload card setup.
#[derive(Debug, Default, Deserialize)]
pub struct Checkout {
    /// The checkout's id, which the subscription checkout route takes.
    #[serde(default, deserialize_with = "field")]
    pub intent_id: Option<String>,
    /// Where a person pays.
    #[serde(default, deserialize_with = "field")]
    pub checkout_url: Option<String>,
}

/// The billing profile of [`PROFILE`], as far as `hipc` decides on it.
#[derive(Debug, Default, Deserialize)]
pub struct Profile {
    /// Where the invoices and billing emails go.
    #[serde(default, deserialize_with = "field")]
    pub billing_email: Option<String>,
}

/// The auto-reload setting of [`AUTO_RELOAD`], as far as `hipc` decides on it. A field not
/// in the shape the `PUT` takes reads as `None`.
#[derive(Debug, Default, Deserialize)]
#[expect(
    clippy::option_option,
    reason = "a cap that is not text or `null` is not read, and `null` is no cap"
)]
pub struct AutoReload {
    /// `false`, and nothing else, while the feature is off on the server.
    #[serde(default, deserialize_with = "field")]
    pub available: Option<bool>,
    /// Whether the card is charged when the balance falls below `threshold`.
    #[serde(default, deserialize_with = "field")]
    pub enabled: Option<bool>,
    /// Where the reload stands.
    #[serde(default, deserialize_with = "field")]
    pub state: Option<AutoReloadState>,
    /// The balance below which the card is charged; `null` until a setting is saved.
    #[serde(default, deserialize_with = "field")]
    pub threshold: Option<String>,
    /// The balance a charge brings the account back up to; `null` until a setting is saved.
    #[serde(default, deserialize_with = "field")]
    pub target: Option<String>,
    /// The most charged in a month: `Some(None)` is `null`, no limit.
    #[serde(default, deserialize_with = "field")]
    pub monthly_cap: Option<Option<String>>,
}

/// Where an auto-reload stands, as swagger lists the states. A state the server adds is
/// [`AutoReloadState::Other`], not an error.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "String")]
pub enum AutoReloadState {
    /// On, and waiting for the balance to fall below the threshold.
    Armed,
    /// Off.
    Off,
    /// Turned off after charges failed.
    DisabledFailures,
    /// Waiting to retry a failed charge.
    RetryWait,
    /// A charge is being made.
    InFlight,
    /// Waiting after a charge before the next.
    Cooldown,
    /// The month's cap is spent.
    MonthlyCapReached,
    /// A state `hipc` does not know.
    Other(String),
}

impl AutoReloadState {
    /// The state as the API spells it.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Armed => "armed",
            Self::Off => "off",
            Self::DisabledFailures => "disabled_failures",
            Self::RetryWait => "retry_wait",
            Self::InFlight => "in_flight",
            Self::Cooldown => "cooldown",
            Self::MonthlyCapReached => "monthly_cap_reached",
            Self::Other(text) => text,
        }
    }
}

impl From<String> for AutoReloadState {
    fn from(text: String) -> Self {
        match text.as_str() {
            "armed" => Self::Armed,
            "off" => Self::Off,
            "disabled_failures" => Self::DisabledFailures,
            "retry_wait" => Self::RetryWait,
            "in_flight" => Self::InFlight,
            "cooldown" => Self::Cooldown,
            "monthly_cap_reached" => Self::MonthlyCapReached,
            _ => Self::Other(text),
        }
    }
}

/// The quote of [`NATIVE_QUOTE`], as far as a buy depends on it. It is read whole: a field a
/// buy needs that is missing or not its type fails the read.
#[derive(Debug, Deserialize)]
pub struct NativeQuote {
    /// Whether hAlpha top-ups are on.
    pub enabled: bool,
    /// Whether the price on chain is stale, which the chain refuses a buy at.
    pub stale: bool,
    /// Whether Hippius signs the account's buys, rather than the account itself.
    pub custodial: bool,
    /// Credits per hAlpha, 18 decimals.
    #[serde(deserialize_with = "planck")]
    pub alpha_price: u128,
    /// The least hAlpha a buy takes, 18 decimals.
    #[serde(deserialize_with = "planck")]
    pub min_alpha: u128,
    /// The account's hAlpha, 18 decimals.
    #[serde(deserialize_with = "planck")]
    pub native_balance: u128,
    /// Why top-ups are off, when they are.
    #[serde(default, deserialize_with = "field")]
    pub reason: Option<String>,
    /// The Hippius address whose hAlpha a buy spends.
    #[serde(default, deserialize_with = "field")]
    pub address: Option<String>,
}

/// An 18-decimal amount as the API sends it: a string of digits, or a JSON integer, read
/// exactly. Anything else, a sign or a fraction included, fails the read.
fn planck<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u128, D::Error> {
    let text = match Value::deserialize(deserializer)? {
        Value::String(text) => text,
        Value::Number(number) => number.to_string(),
        _ => String::new(),
    };
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(serde::de::Error::custom("not an 18-decimal amount"));
    }
    text.parse()
        .map_err(|_| serde::de::Error::custom("an amount past u128"))
}

/// One hAlpha purchase, from [`NATIVE_BUY`] or a row of [`NATIVE_PURCHASES`], as far as
/// `hipc` decides on it.
#[derive(Debug, Default, Deserialize)]
pub struct Purchase {
    /// Its id as text, whether the API sends a number or a string, as the console matches
    /// `String(id)`.
    #[serde(default, deserialize_with = "text_or_number")]
    pub id: Option<String>,
    /// Where it stands.
    #[serde(default)]
    pub status: PurchaseStatus,
    /// When it was made, RFC 3339.
    #[serde(default, deserialize_with = "field")]
    pub created_at: Option<String>,
    /// The chain's error, for a failed purchase.
    #[serde(default, deserialize_with = "field")]
    pub error: Option<String>,
}

/// Where an hAlpha purchase stands. A status the server adds is
/// [`PurchaseStatus::Other`], not an error.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PurchaseStatus {
    /// No status yet: missing or `null`.
    #[default]
    Unset,
    /// Waiting to be sent to the chain.
    Queued,
    /// Sent to the chain.
    Submitted,
    /// In a block, not yet final.
    Included,
    /// Final: the credits are on the balance.
    Completed,
    /// Refused by the chain.
    Failed,
    /// A status `hipc` does not know.
    Other(String),
    /// A status that is not text.
    NotText,
}

impl PurchaseStatus {
    /// Whether the purchase may still complete: no status yet, `queued`, `submitted`, or
    /// `included`.
    pub fn in_flight(&self) -> bool {
        match self {
            Self::Unset | Self::Queued | Self::Submitted | Self::Included => true,
            Self::Completed | Self::Failed | Self::Other(_) | Self::NotText => false,
        }
    }

    /// The status as the API spells it, or `None` without one in text.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Unset | Self::NotText => None,
            Self::Queued => Some("queued"),
            Self::Submitted => Some("submitted"),
            Self::Included => Some("included"),
            Self::Completed => Some("completed"),
            Self::Failed => Some("failed"),
            Self::Other(text) => Some(text),
        }
    }
}

impl<'de> Deserialize<'de> for PurchaseStatus {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Value::deserialize(deserializer)? {
            Value::Null => Self::Unset,
            Value::String(text) => match text.as_str() {
                "queued" => Self::Queued,
                "submitted" => Self::Submitted,
                "included" => Self::Included,
                "completed" => Self::Completed,
                "failed" => Self::Failed,
                _ => Self::Other(text),
            },
            _ => Self::NotText,
        })
    }
}

/// The answer of [`GAMMA_HOTKEY_CHALLENGE`], as far as `hipc` decides on it.
#[derive(Debug, Default, Deserialize)]
pub struct HotkeyChallenge {
    /// The text the hotkey signs.
    #[serde(default, deserialize_with = "field")]
    pub message: Option<String>,
    /// The nonce the link sends back.
    #[serde(default, deserialize_with = "field")]
    pub nonce: Option<String>,
}

/// A gamma conversion of [`GAMMA_CONVERSION_CREATE`], as far as `hipc` decides on it.
#[derive(Debug, Default, Deserialize)]
pub struct GammaConversion {
    /// The conversion's id.
    #[serde(default, deserialize_with = "field")]
    pub id: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::view::view_of;

    #[test]
    fn an_auto_reload_body_sends_no_cap_as_null_and_card_setup_is_empty() {
        let body = AutoReloadUpdateBody {
            enabled: true,
            threshold: "5.00".to_owned(),
            target: "20.00".to_owned(),
            monthly_cap: None,
        };
        assert_eq!(
            serde_json::to_string(&body).unwrap(),
            r#"{"enabled":true,"threshold":"5.00","target":"20.00","monthly_cap":null}"#
        );
    }

    /// Each state swagger lists reads as its variant and back; another is `Other`, not an
    /// error, and a state that is not text is not read.
    #[test]
    fn an_auto_reload_state_the_server_adds_is_other() {
        for state in [
            "armed",
            "off",
            "disabled_failures",
            "retry_wait",
            "in_flight",
            "cooldown",
            "monthly_cap_reached",
            "paused",
        ] {
            let read: AutoReload = view_of(&json!({ "state": state }));
            let read = read.state.unwrap();
            assert_eq!(read.as_str(), state);
            assert_eq!(matches!(read, AutoReloadState::Other(_)), state == "paused");
        }
        let read: AutoReload = view_of(&json!({"state": 1, "monthly_cap": null}));
        assert_eq!(read.state, None);
        assert_eq!(read.monthly_cap, Some(None));
    }
}
