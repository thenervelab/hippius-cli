//! The grammar of `hipc billing`.

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

use crate::cli::account::email;
use crate::cli::values::{Spaces, is_uuid, label};
use crate::cli::{INVOICE_ID, ListArgs, examples, new_file, number};
use crate::units::{Decimal, ONE, Planck, Usd, usd};

#[derive(Debug, Subcommand)]
pub(crate) enum Billing {
    /// The account's balance (GET /api/billing/credits/balance/)
    #[command(after_help = examples(&[
        "hipc billing balance",
    ]))]
    Balance,
    /// Every plan in one list: Drive, S3, Hub, and a legacy Stripe top-up plan
    ///
    /// The token's own account only.
    #[command(after_help = examples(&[
        "hipc billing subscriptions",
    ]))]
    Subscriptions,
    /// The transaction history, one page at a time
    #[command(after_help = examples(&[
        "hipc billing transactions",
        "hipc billing transactions --limit 20",
        "hipc billing transactions --all",
    ]))]
    Transactions(ListArgs),
    /// The legacy Stripe top-up plan, if there is one
    #[command(after_help = examples(&[
        "hipc billing stripe",
    ]))]
    Stripe,
    /// What compute burns per hour, and how long the balance lasts at that rate
    ///
    /// burn_rate_per_hour, arrears, and each item's per_hour are base units (10^18 = $1);
    /// balance is dollars; runway_hours is null when nothing burns. Text mode shows those
    /// three in credits (1 credit = $1), as the balance is, and says so in a note; the JSON
    /// output keeps the base units. charge_cap is printed as sent. Drive and S3 plans are not
    /// in it.
    #[command(after_help = examples(&[
        "hipc billing summary",
    ]))]
    Summary,
    /// Every closed UTC hour of compute usage in one month, with its priced lines
    ///
    /// Amounts are base units (10^18 = $1). Text mode shows each total, unit_price, and amount
    /// in credits (1 credit = $1) and says so in a note; the JSON output keeps the base units.
    /// Each hour's status says whether it was charged.
    #[command(after_help = examples(&[
        "hipc billing usage",
        "hipc billing usage --month 2026-09",
    ]))]
    Usage {
        /// The month, as YYYY-MM in UTC
        ///
        /// Without it the server uses the current month.
        #[arg(long, value_name = "YYYY-MM", value_parser = month)]
        month: Option<String>,
    },
    /// List the invoices and credit notes, and download one as a PDF
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc billing invoices list",
        "hipc billing invoices get 42 --output-file invoice-42.pdf",
    ]))]
    Invoices(Invoices),
    /// The balance auto-reload setting, the card it charges, and the attempts it made
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc billing auto-reload get",
        "hipc billing auto-reload history",
        "hipc billing auto-reload update --enabled false",
        "hipc billing auto-reload card-setup",
    ]))]
    AutoReload(AutoReload),
    /// The billing profile: the company name, address, tax id, and email invoices carry
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc billing profile get",
        "hipc billing profile update --company-name Acme --country GB",
    ]))]
    Profile(BillingProfile),
    /// The hAlpha price and the account's hAlpha: what a hAlpha top-up would buy
    ///
    /// GET /api/billing/native/quote/. enabled and stale say whether a buy can go through now;
    /// alpha_price is credits per hAlpha, and min_alpha and native_balance are hAlpha, all
    /// 18-decimal integers (10^18 per hAlpha or credit), kept exact in JSON and shown as
    /// decimals in text mode. custodial says whether the API signs the account's buys, and
    /// address is its Hippius address. The route reads the signed-in user's own hAlpha, not an
    /// account's or a billing account's, so a membership (from a flag, a variable, or the
    /// profile) is refused, and so is a project or a billing account from a flag or a variable;
    /// one stored in the profile is skipped.
    #[command(after_help = examples(&[
        "hipc billing native-quote",
    ]))]
    NativeQuote,
    /// Pay with the Bittensor gamma of subnet 75: link a hotkey, read its deposit, and burn its
    /// gamma for credits
    ///
    /// Gamma is subnet 75's prepaid credit on Bittensor, minted by burning its alpha; one gamma
    /// is worth one US dollar. A conversion burns gamma from a deposit you hold and adds the
    /// same amount in USD to the balance. The deposit must name the Hippius spender, which
    /// hipc billing gamma get prints, and its hotkey must be linked to the account first. The
    /// routes act on the signed-in user's own account: a membership (from a flag, a variable,
    /// or the profile) is refused, and so is a project or a billing account from a flag or a
    /// variable. Every route answers 404 while the server has gamma payments off.
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc billing gamma get",
        "hipc billing gamma hotkeys create --hotkey-file hotkey.txt",
        "hipc billing gamma conversions create --hotkey 5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty --amount 10 --yes",
        "hipc billing gamma conversions get 6f0b2c1e-7b4d-4e8a-9c01-5d6e7f8a5f60",
    ]))]
    Gamma(Gamma),
    /// Top up the balance by card, Bitcoin, or hAlpha; needs --yes
    ///
    /// A card or Bitcoin top-up takes --amount in US dollars and prints its checkout_url.
    /// Nothing is paid until a person pays there; then the balance grows by the amount. hipc
    /// does not wait for the payment: hipc billing balance shows it. A member tops up only a
    /// billing account, from --billing, HIPPIUS_BILLING, or hipc account billing-accounts use;
    /// without one a membership is refused with account_scope_unsupported.
    ///
    /// A hAlpha top-up takes --alpha, the hAlpha to spend, and converts it into credits on
    /// chain (1 credit = 1 USD). It reads GET /api/billing/native/quote/ and the purchases
    /// first, and refuses when the quote is stale or not enabled, when --alpha is below its
    /// min_alpha, when --alpha and the 0.01 hAlpha kept for the fee are more than its
    /// native_balance, when a purchase from the last 15 minutes is still in flight, or when the
    /// account signs its own buys (custodial false), which are made in the console.
    /// The question names the address whose hAlpha is spent, the credits the quote gives,
    /// rounded down, and the minimum it accepts, 1 percent less; a price that falls further
    /// makes the chain refuse the buy. A person who was asked has the purchases read again
    /// before the buy. Then it sends POST /api/billing/native/buy/ with an Idempotency-Key, and
    /// follows GET /api/billing/native/purchases/ every 3 seconds until the purchase is
    /// completed or failed, for at most --wait-timeout. A run that stops first prints the
    /// purchase as it stands and exits 0; a purchase the chain refused is partial_failure, with
    /// the purchase on stdout. The key is printed as idempotency_key. A buy that may have
    /// reached the server is retried with the command its error names, which adds
    /// --min-credits and --idempotency-key: the retry sends the amounts unchanged, so the API
    /// answers it as the first buy, and reads only the quote first, refusing a --min-credits
    /// below 90 percent of what --alpha buys now, since under a key the API has not seen it is
    /// a new buy. The routes act on the signed-in user's own hAlpha, and the credits go to the
    /// user's own balance: a membership (from a flag, a variable, or the profile) is refused,
    /// and so is a project from anywhere or a billing account from a flag or a variable; a
    /// billing account stored in the profile is skipped. USDC and TAO top-ups stay in the
    /// console.
    #[command(after_help = examples(&[
        "hipc billing top-up --amount 50 --method card --yes",
        "hipc billing top-up --method halpha --alpha 1.5 --yes",
    ]))]
    TopUp(TopUp),
}

#[derive(Debug, Subcommand)]
pub(crate) enum AutoReload {
    /// The balance auto-reload setting, and the card it charges
    ///
    /// On or off, its threshold, target, and monthly cap in USD (1 credit = $1), and its state.
    /// available is false, and nothing else is sent, while the feature is off on the server.
    #[command(after_help = examples(&[
        "hipc billing auto-reload get",
    ]))]
    Get,
    /// The auto-reload attempts, newest first, with their status, credits, and invoice
    ///
    /// The route has no pages: it sends the newest --limit attempts and nothing says whether
    /// older ones exist. Amounts are credits (1 credit = $1).
    #[command(after_help = examples(&[
        "hipc billing auto-reload history",
        "hipc billing auto-reload history --limit 10",
    ]))]
    History {
        /// How many attempts to read, 1 to 100, the most the route sends
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=100))]
        limit: u32,
    },
    /// Change the auto-reload setting; spends money when it leaves auto-reload on, and then
    /// needs --yes when it turns it on, finds it in a state other than armed, raises the
    /// threshold, the target, or the monthly cap, or cannot read the setting in force
    ///
    /// Auto-reload charges the saved card whenever the balance falls below the threshold, back
    /// up to the target. The route replaces the whole setting, so hipc reads it first and sends
    /// it back with only the flags given changed. The read and the write are separate requests,
    /// since the route takes no version, so a change saved by someone else in between is
    /// written over. Turning auto-reload off, or lowering an amount while it is armed, needs no
    /// --yes. A save while it is on checks the balance at once, which may charge the card now,
    /// so an update that leaves it on is confirmed in every state but armed. A field the
    /// setting does not hold, such as the threshold of a setting never saved, must be given.
    /// A member acts only in a billing account, from --billing, HIPPIUS_BILLING, or hipc account
    /// billing-accounts use.
    #[command(after_help = examples(&[
        "hipc billing auto-reload update --threshold 20 --target 100",
        "hipc billing auto-reload update --enabled true --threshold 20 --target 100 --yes",
        "hipc billing auto-reload update --enabled false",
    ]))]
    Update(AutoReloadUpdate),
    /// Start saving the card auto-reload charges, and print its checkout URL
    ///
    /// A Stripe checkout in setup mode: nothing is charged, and the card is saved once a person
    /// completes it at checkout_url. hipc does not wait for it; next_step names hipc billing
    /// auto-reload get, which shows the card.
    #[command(after_help = examples(&[
        "hipc billing auto-reload card-setup",
    ]))]
    CardSetup,
}

/// `hipc billing auto-reload update`: the fields of the setting to change; the others are kept.
#[derive(Debug, Args)]
#[command(group(
    clap::ArgGroup::new("change")
        .required(true)
        .multiple(true)
        .args(["enabled", "threshold", "target", "monthly_cap"])
))]
pub(crate) struct AutoReloadUpdate {
    /// Turn auto-reload on or off: true or false
    #[arg(long, value_name = "BOOL")]
    pub(crate) enabled: Option<bool>,
    /// The balance in US dollars, to the cent, below which the card is charged
    #[arg(long, value_name = "USD", value_parser = usd)]
    pub(crate) threshold: Option<Usd>,
    /// The balance in US dollars, to the cent, that a charge brings the balance back up to
    ///
    /// Each charge is the target less the balance, not a fixed amount.
    #[arg(long, value_name = "USD", value_parser = usd)]
    pub(crate) target: Option<Usd>,
    /// The most charged to the card in a calendar month (UTC), in US dollars to the cent, or
    /// none for no limit
    #[arg(long, value_name = "USD", value_parser = reload_cap)]
    pub(crate) monthly_cap: Option<ReloadCap>,
}

/// `--monthly-cap` of auto-reload: an amount, or no limit, which is sent as `null`.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ReloadCap {
    Usd(Usd),
    None,
}

fn reload_cap(value: &str) -> Result<ReloadCap, String> {
    if value == "none" {
        return Ok(ReloadCap::None);
    }
    usd(value).map(ReloadCap::Usd).map_err(|_| {
        "a cap is US dollars to the cent, such as `500` or `12.50`, or `none`".to_owned()
    })
}

#[derive(Debug, Subcommand)]
pub(crate) enum BillingProfile {
    /// The billing profile; every field is empty when none was saved
    #[command(after_help = examples(&[
        "hipc billing profile get",
    ]))]
    Get,
    /// Change the fields given of the billing profile; the others are kept; needs --yes when
    /// it changes the billing email
    ///
    /// Only the flags given are sent, and the server keeps every field left out. An empty
    /// value, such as --address-line2 "", clears a field. Invoices issued after the update
    /// carry it. A member acts only in a billing account, from --billing, HIPPIUS_BILLING, or
    /// hipc account billing-accounts use.
    ///
    /// With --billing-email, the profile is read first: invoices go to that address, so the
    /// update needs --yes unless the profile already holds that exact email. A profile that is
    /// not found, or holds no email, counts as a change.
    #[command(after_help = examples(&[
        "hipc billing profile update --company-name Acme --country GB",
        "hipc billing profile update --billing-email invoices@example.com --yes",
    ]))]
    Update(Box<ProfileUpdate>),
}

/// `hipc billing profile update`: the fields to change, named as the route names them.
#[derive(Debug, Args)]
#[command(group(
    clap::ArgGroup::new("change")
        .required(true)
        .multiple(true)
        .args([
            "company_name",
            "address_line1",
            "address_line2",
            "city",
            "postal_code",
            "region",
            "country",
            "tax_id",
            "billing_email",
        ])
))]
pub(crate) struct ProfileUpdate {
    /// The company name invoices carry
    #[arg(long, value_name = "NAME", value_parser = profile_text)]
    pub(crate) company_name: Option<String>,
    /// The first line of the address
    #[arg(long, value_name = "LINE", value_parser = profile_text)]
    pub(crate) address_line1: Option<String>,
    /// The second line of the address
    #[arg(long, value_name = "LINE", value_parser = profile_text)]
    pub(crate) address_line2: Option<String>,
    /// The city
    #[arg(long, value_name = "CITY", value_parser = profile_text)]
    pub(crate) city: Option<String>,
    /// The postal code
    #[arg(long, value_name = "CODE", value_parser = profile_text)]
    pub(crate) postal_code: Option<String>,
    /// The region, state, or province
    #[arg(long, value_name = "REGION", value_parser = profile_text)]
    pub(crate) region: Option<String>,
    /// The country, as a two-letter ISO 3166-1 code such as GB, sent in upper case
    #[arg(long, value_name = "CC", value_parser = country)]
    pub(crate) country: Option<String>,
    /// The tax id, such as a VAT number
    #[arg(long, value_name = "ID", value_parser = profile_text)]
    pub(crate) tax_id: Option<String>,
    /// Where invoices are mailed; empty means the account's email
    #[arg(long, value_name = "EMAIL", value_parser = billing_email)]
    pub(crate) billing_email: Option<String>,
}

/// `--country`: two ASCII letters, sent in upper case as the console sends them, or empty to
/// clear the field.
fn country(value: &str) -> Result<String, String> {
    if value.is_empty() || (value.len() == 2 && value.bytes().all(|b| b.is_ascii_alphabetic())) {
        Ok(value.to_ascii_uppercase())
    } else {
        Err("a country is a two-letter ISO 3166-1 code, such as `GB`, or empty".to_owned())
    }
}

#[derive(Debug, Args)]
pub(crate) struct TopUp {
    /// The amount of a card or Bitcoin top-up in US dollars, to the cent, such as 50 or 12.50
    ///
    /// Required with --method card or bitcoin, and refused with halpha. A card top-up takes 10
    /// to 10000, and a Bitcoin top-up 20 to 100000.
    #[arg(long, value_name = "USD", value_parser = usd)]
    pub(crate) amount: Option<Usd>,
    /// The hAlpha a hAlpha top-up spends, a decimal such as 1.5 or 0.01
    ///
    /// Required with --method halpha, and refused with card or bitcoin. At least 0.01, the
    /// chain's minimum, with at most 18 digits after the point.
    #[arg(long, value_name = "HALPHA", value_parser = alpha)]
    pub(crate) alpha: Option<Planck>,
    /// How the top-up is paid: a Stripe card checkout, a BTCPay Bitcoin invoice (on-chain or
    /// Lightning), or the account's hAlpha
    #[arg(long, value_enum)]
    pub(crate) method: TopUpMethod,
    /// The Idempotency-Key of a hAlpha buy to retry, a UUID: the one a run that failed
    /// printed, so the retry is not a second buy
    ///
    /// Only with --method halpha. Without it each run sends a new one. With it, the run resends
    /// that buy as it was: it needs --min-credits from the same hint and the same --alpha, reads
    /// the quote but not the purchases first, and refuses a --min-credits below 90 percent of what
    /// --alpha buys now.
    #[arg(long, value_name = "UUID", value_parser = idempotency_key)]
    pub(crate) idempotency_key: Option<String>,
    /// The min_credits of the hAlpha buy to retry, in credits, as the failed run printed it
    ///
    /// Only with --method halpha and --idempotency-key: a retry sends the first run's amounts
    /// unchanged.
    #[arg(long, value_name = "CREDITS", value_parser = min_credits)]
    pub(crate) min_credits: Option<Planck>,
    /// How long a hAlpha top-up follows the purchase, in seconds, 1 to 900, default 900
    ///
    /// Only with --method halpha. A poll still in flight at the deadline can add up to
    /// --request-timeout.
    #[arg(
        long,
        value_name = "SECONDS",
        value_parser = clap::value_parser!(u64).range(1..=900)
    )]
    pub(crate) wait_timeout: Option<u64>,
}

/// The least `Marketplace.buy_credits` takes, 0.01 hAlpha: less fails on chain with
/// `BuyAmountTooLow`.
pub(crate) const CHAIN_MINIMUM: Planck = Planck(ONE / 100);

/// `--alpha`: hAlpha in the amount grammar, with at most 18 decimals, to an exact planck
/// count ([`Planck::parse`]).
pub(crate) fn alpha(text: &str) -> Result<Planck, String> {
    let planck = Planck::parse(text, "hAlpha", "`1.5` or `0.01`")?;
    if planck < CHAIN_MINIMUM {
        return Err(format!(
            "the chain takes at least `{CHAIN_MINIMUM}` hAlpha, and a smaller buy fails with \
             `BuyAmountTooLow`"
        ));
    }
    Ok(planck)
}

/// `--min-credits`: credits as [`alpha`] reads hAlpha, above zero.
fn min_credits(text: &str) -> Result<Planck, String> {
    let planck = Planck::parse(text, "credits", "`6.12` or `990`")?;
    if planck.0 == 0 {
        return Err("a buy accepts at least some credits, so the minimum is above `0`".to_owned());
    }
    Ok(planck)
}

/// A text field of the billing profile, which invoices print: a [`label`], empty to clear it.
/// The API documents no length.
fn profile_text(value: &str) -> Result<String, String> {
    label(
        value,
        0..=usize::MAX,
        Spaces::Kept,
        "a billing profile field",
    )
}

/// `--billing-email`: an email address as an invite takes it, or empty for the account's own.
fn billing_email(value: &str) -> Result<String, String> {
    if value.is_empty() {
        Ok(String::new())
    } else {
        email(value)
    }
}

/// `--idempotency-key`: a UUID, as each run makes one: hex in groups of 8, 4, 4, 4, and 12.
fn idempotency_key(text: &str) -> Result<String, String> {
    if is_uuid(text) {
        Ok(text.to_owned())
    } else {
        Err(
            "a key is a UUID such as `3f2c9a1e-7b4d-4e8a-9c01-5d6e7f8a9b0c`, as a run prints it"
                .to_owned(),
        )
    }
}

/// How a top-up is paid. USDC and TAO are left to the console.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum TopUpMethod {
    /// A Stripe card checkout
    Card,
    /// A BTCPay invoice, paid on-chain or over Lightning
    Bitcoin,
    /// The account's hAlpha, converted into credits on chain
    Halpha,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Invoices {
    /// The invoices and credit notes, newest first, one page at a time
    #[command(after_help = examples(&[
        "hipc billing invoices list",
        "hipc billing invoices list --all",
    ]))]
    List(ListArgs),
    /// Download one invoice as a PDF, to a new file
    ///
    /// The PDF goes only to the file --output-file names, created with mode 0600; an existing
    /// file is never overwritten. stdout is a JSON document that names the file.
    #[command(after_help = examples(&[
        "hipc billing invoices get 42 --output-file invoice-42.pdf",
    ]))]
    Get {
        /// The invoice id, from hipc billing invoices list
        #[arg(value_parser = number(INVOICE_ID))]
        invoice_id: u64,
        /// The file to create; it must not exist yet
        #[arg(long, value_name = "PATH", value_parser = new_pdf_file)]
        output_file: PathBuf,
    },
}

/// A month as the compute usage route takes it: `YYYY-MM`, months 01 to 12. Checked at
/// parse time, so a bad month is a usage error before anything is sent; the server answers
/// one with 400 (swagger `compute_usage_list`).
fn month(text: &str) -> Result<String, String> {
    let valid = match text.as_bytes() {
        [y1, y2, y3, y4, b'-', m1, m2] => {
            [y1, y2, y3, y4, m1, m2].iter().all(|b| b.is_ascii_digit())
                && matches!((m1, m2), (b'0', b'1'..=b'9') | (b'1', b'0'..=b'2'))
        }
        _ => false,
    };
    if valid {
        Ok(text.to_owned())
    } else {
        Err("a month is YYYY-MM, such as `2026-09`".to_owned())
    }
}

/// The file an invoice PDF goes to ([`new_file`]).
fn new_pdf_file(text: &str) -> Result<PathBuf, String> {
    new_file(text, "the PDF")
}

#[derive(Debug, Subcommand)]
pub(crate) enum Gamma {
    /// The subnet, the Hippius spender a deposit must name, its service hotkey, the minimum,
    /// and each linked hotkey with its deposit read from the chain
    ///
    /// spender is the account a deposit must name, and service_hotkey the hotkey that btcli's
    /// gamma mint takes to name it; service_hotkey is null until Hippius
    /// publishes its service. A hotkey's deposit is null when it holds none on the subnet, or
    /// when the chain could not be read, which chain unavailable says. names_hippius false means
    /// the deposit names another spender, so Hippius cannot spend it. spendable is the balance
    /// as the deposit's daily spend cap limits it now. Amounts are gamma with 9 decimals, as
    /// strings.
    #[command(after_help = examples(&[
        "hipc billing gamma get",
    ]))]
    Get,
    /// Link a hotkey to the account, or unlink one
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc billing gamma hotkeys create --hotkey-file hotkey.txt",
        "hipc billing gamma hotkeys delete 5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty --yes",
    ]))]
    Hotkeys(GammaHotkeys),
    /// Burn gamma for credits, and follow the conversions
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc billing gamma conversions create --hotkey 5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty --amount 10 --yes",
        "hipc billing gamma conversions list",
        "hipc billing gamma conversions get 6f0b2c1e-7b4d-4e8a-9c01-5d6e7f8a5f60",
    ]))]
    Conversions(GammaConversions),
}

#[derive(Debug, Subcommand)]
pub(crate) enum GammaHotkeys {
    /// Link the hotkey whose words --hotkey-file holds: hipc signs the server's challenge with
    /// it, and the words go nowhere else
    ///
    /// The hotkey is the SS58 address the words make. hipc asks POST
    /// /api/billing/gamma/hotkeys/challenge/ for a challenge, checks that its message is a
    /// link of that hotkey, signs it with the hotkey's sr25519 key, and sends the signature to
    /// POST /api/billing/gamma/hotkeys/. A challenge is spent by its first answer, even a
    /// refusal; a hotkey linked to another account is refused with 409 hotkey_linked_elsewhere.
    /// The file is held to the config file's checks: a regular file of the current user, with
    /// no bits for the group or others. A file of - reads the words from stdin. A dry run reads the
    /// words too, to name the hotkey.
    #[command(after_help = examples(&[
        "hipc billing gamma hotkeys create --hotkey-file hotkey.txt",
    ]))]
    Create {
        /// The file that holds the hotkey's BIP-39 words, or - for stdin
        #[arg(long, value_name = "PATH")]
        hotkey_file: PathBuf,
    },
    /// Unlink a hotkey from the account; needs --yes
    ///
    /// A conversion from it then needs it linked again, which takes its words. Its gamma stays
    /// in its deposit on the chain.
    #[command(after_help = examples(&[
        "hipc billing gamma hotkeys delete 5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty --yes",
    ]))]
    Delete {
        /// The hotkey's SS58 address, as hipc billing gamma get lists it
        #[arg(value_parser = hotkey)]
        hotkey: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum GammaConversions {
    /// Burn gamma from a linked hotkey's deposit for the same amount in USD; spends money, and
    /// needs --yes
    ///
    /// POST /api/billing/gamma/conversions/ answers 202 with the conversion pending: the API
    /// then signs the spend on chain and credits the balance once a finalized block holds it.
    /// hipc does not wait: hipc billing gamma conversions get <id> shows it, spent once it is
    /// credited, skipped or failed when nothing was spent or credited. Burned gamma cannot be
    /// refunded. The API refuses a hotkey not linked to the account, a deposit that does not
    /// name its spender or cannot spend the amount now, a chain price too far from its own,
    /// a daily limit reached, and a second conversion while one is in flight.
    #[command(after_help = examples(&[
        "hipc billing gamma conversions create --hotkey 5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty --amount 10 --yes",
    ]))]
    Create {
        /// The linked hotkey whose deposit pays, as hipc billing gamma get lists it
        #[arg(long, value_name = "SS58", value_parser = hotkey)]
        hotkey: String,
        /// The gamma to burn, at least 0.1 and at most 9 decimals; credited 1:1 in USD
        #[arg(long, value_name = "GAMMA", value_parser = gamma_amount)]
        amount: String,
    },
    /// The account's conversions, newest first, one page at a time
    #[command(after_help = examples(&[
        "hipc billing gamma conversions list",
        "hipc billing gamma conversions list --limit 20",
        "hipc billing gamma conversions list --all",
    ]))]
    List(ListArgs),
    /// One conversion: its status, its on-chain reference, and the transaction that credited it
    #[command(after_help = examples(&[
        "hipc billing gamma conversions get 6f0b2c1e-7b4d-4e8a-9c01-5d6e7f8a5f60",
    ]))]
    Get {
        /// The conversion id, from the answer of hipc billing gamma conversions create or list
        #[arg(value_parser = conversion_id)]
        conversion_id: String,
    },
}

/// A hotkey's SS58 address: it goes into the URL path, so only the Base58 alphabet is
/// accepted, 46 to 48 characters as a 32-byte key's address has. The server checks the
/// address itself.
fn hotkey(text: &str) -> Result<String, String> {
    let base58 = |b: u8| b.is_ascii_alphanumeric() && !matches!(b, b'0' | b'O' | b'I' | b'l');
    if (46..=48).contains(&text.len()) && text.bytes().all(base58) {
        Ok(text.to_owned())
    } else {
        Err("a hotkey is an SS58 address, such as \
             `5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty`"
            .to_owned())
    }
}

/// `--amount` of gamma: an amount with at most 9 decimals, at least `0.1`, sent as written.
fn gamma_amount(text: &str) -> Result<String, String> {
    let Some(amount) = Decimal::parse(text) else {
        return Err("an amount is gamma in digits, such as `10` or `0.5`".to_owned());
    };
    if amount.fraction().len() > 9 {
        return Err(
            "gamma has 9 decimals, so an amount has at most 9 digits after the point".to_owned(),
        );
    }
    let whole_zero = amount.whole().bytes().all(|b| b == b'0');
    let tenths = amount.fraction().bytes().next().unwrap_or(b'0');
    if whole_zero && tenths == b'0' {
        return Err("a conversion takes at least `0.1` gamma".to_owned());
    }
    Ok(text.to_owned())
}

/// A conversion id: a UUID, as the API answers it, which goes into the URL path.
pub(crate) fn conversion_id(text: &str) -> Result<String, String> {
    idempotency_key(text).map_err(|_| {
        "a conversion id is a UUID such as `6f0b2c1e-7b4d-4e8a-9c01-5d6e7f8a5f60`, as hipc billing \
         gamma conversions create prints it"
            .to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each top-up flag that only one kind of top-up takes says which, in its help, since the
    /// run refuses it with the other kinds and `hipc schema` lists no rule for it.
    #[test]
    fn each_top_up_flag_names_the_methods_that_take_it() {
        let root = <crate::cli::Cli as clap::CommandFactory>::command();
        let top_up = root
            .find_subcommand("billing")
            .and_then(|billing| billing.find_subcommand("top-up"))
            .expect("hipc billing top-up is a command");
        let help = |id: &str| {
            top_up
                .get_arguments()
                .find(|arg| arg.get_id() == id)
                .and_then(clap::Arg::get_long_help)
                .map(ToString::to_string)
                .unwrap_or_default()
        };
        for (id, says) in [
            (
                "amount",
                "Required with --method card or bitcoin, and refused with halpha",
            ),
            (
                "alpha",
                "Required with --method halpha, and refused with card or bitcoin",
            ),
            ("idempotency_key", "Only with --method halpha"),
            (
                "min_credits",
                "Only with --method halpha and --idempotency-key",
            ),
            ("wait_timeout", "Only with --method halpha"),
        ] {
            assert!(help(id).contains(says), "{id}: {}", help(id));
        }
    }

    /// Every amount up to the largest top-up, and a stride past it, reads back as written in
    /// each spelling `--amount` takes: `d.cc`, `d.c` for whole tenths, and `d` for whole
    /// dollars, with leading zeros too.
    #[test]
    fn alpha_reads_a_decimal_to_exact_planck() {
        for (text, planck) in [
            ("0.01", 10_000_000_000_000_000),
            ("1", ONE),
            ("1.5", 1_500_000_000_000_000_000),
            ("01.50", 1_500_000_000_000_000_000),
            ("0.010000000000000001", 10_000_000_000_000_001),
            // Trailing zeros hold no value, as in a credit amount.
            ("1.0000000000000000000", ONE),
            (
                "123456789.123456789123456789",
                123_456_789_123_456_789_123_456_789,
            ),
            ("340282366920938463463", 340_282_366_920_938_463_463 * ONE),
        ] {
            assert_eq!(super::alpha(text), Ok(Planck(planck)), "{text}");
        }
        let shape = "an amount is hAlpha in digits, such as `1.5` or `0.01`";
        let minimum = "the chain takes at least `0.01` hAlpha, and a smaller buy fails with \
                       `BuyAmountTooLow`";
        let decimals = "hAlpha has 18 decimals, so an amount has at most 18 digits after the point";
        let large = "the amount is larger than any hAlpha balance";
        for (text, error) in [
            ("", shape),
            (".5", shape),
            ("1.", shape),
            ("-1", shape),
            ("+1", shape),
            ("1e3", shape),
            (" 1", shape),
            ("1.2.3", shape),
            ("1,5", shape),
            ("0", minimum),
            ("0.0", minimum),
            ("0.009999999999999999", minimum),
            ("0.0000000000000000001", decimals),
            ("1.0000000000000000001", decimals),
            ("340282366920938463464", large),
            ("9999999999999999999999999999999999999999", large),
        ] {
            assert_eq!(super::alpha(text), Err(error.to_owned()), "{text:?}");
        }
        assert_eq!(CHAIN_MINIMUM.to_string(), "0.01");
    }

    #[test]
    fn idempotency_keys_are_uuids() {
        for key in [
            "3f2c9a1e-7b4d-4e8a-9c01-5d6e7f8a9b0c",
            "3F2C9A1E-7B4D-4E8A-9C01-5D6E7F8A9B0C",
        ] {
            assert_eq!(super::idempotency_key(key).as_deref(), Ok(key));
        }
        for bad in [
            "",
            "3f2c9a1e7b4d4e8a9c015d6e7f8a9b0c",
            "3f2c9a1e-7b4d-4e8a-9c01-5d6e7f8a9b0",
            "3f2c9a1e-7b4d-4e8a-9c01-5d6e7f8a9b0cc",
            "3f2c9a1g-7b4d-4e8a-9c01-5d6e7f8a9b0c",
            "3f2c9a1e-7b4d-4e8a-9c01-5d6e-7f8a9b0c",
            " 3f2c9a1e-7b4d-4e8a-9c01-5d6e7f8a9b0c",
        ] {
            assert!(super::idempotency_key(bad).is_err(), "{bad:?}");
        }
    }
}
