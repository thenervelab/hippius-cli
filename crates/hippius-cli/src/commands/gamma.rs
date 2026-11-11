//! `hipc billing gamma`: the Bittensor gamma of subnet 75 as a payment, through
//! `/api/billing/gamma/`.
//!
//! Gamma sits in a deposit on Bittensor keyed by a hotkey. A hotkey is linked to the account
//! once, by signing the server's challenge with it; a conversion then has the API spend
//! (burn) gamma from that hotkey's deposit and credit the same amount in USD. The routes act
//! on the signed-in user's own account, as the hAlpha routes do, so they are on none of the
//! account header lists: a membership is refused before anything is sent.
//!
//! The hotkey's words are read from a file or stdin, never from argv, and only its sr25519
//! key signs, and only a message that is a link challenge of that hotkey
//! ([`hippius::Hotkey::sign_gamma_link`]).

use std::path::Path;

use hippius::routes::billing;
use hippius::{Error, Hotkey, Request, Result, SideEffects, Source, phrase, view_of};
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::cli::{Gamma, GammaConversions, GammaHotkeys};
use crate::commands::{Input, Printed, list, print_body, print_body_or, print_with};
use crate::context::{Context, Gate};

const FLAG: &str = "--hotkey-file";

pub(crate) fn run(ctx: &Context<'_>, command: &Gamma) -> Result<Printed> {
    dispatch(ctx, command).map_err(with_gamma_hint)
}

/// The hint of an API refusal whose `code` is one of the gamma routes' own: what the code
/// means for the caller, and the command that helps. Any other error is passed on unchanged.
fn with_gamma_hint(error: Error) -> Error {
    let code = error
        .details()
        .and_then(|details| details.get("code"))
        .and_then(Value::as_str);
    let hint = match code {
        Some("gamma_unavailable") => {
            "gamma payments are not enabled on this server yet: they open once gamma is enabled \
             on subnet 75 and Hippius announces it"
        }
        Some("gamma_personal_only") => {
            "gamma pays for the signed-in user's own account only: run it without `--account`, \
             `--project`, or `--billing`, and without a stored selection, or with `--account self`"
        }
        Some("gamma_hotkey_not_found") => {
            "the hotkey is not linked to this account: `hipc billing gamma get` lists the linked \
             hotkeys"
        }
        Some("gamma_page_not_found") => {
            "the page is past the end of the list: run `hipc billing gamma conversions list` \
             again without `--page-token`"
        }
        Some("gamma_conversion_not_found") => {
            "this account has no such conversion: `hipc billing gamma conversions list` lists \
             them"
        }
        _ => return error,
    };
    error.with_hint(hint)
}

fn dispatch(ctx: &Context<'_>, command: &Gamma) -> Result<Printed> {
    match command {
        Gamma::Get => print_body(
            ctx,
            &ctx.load_authed_client()?,
            &Request::to(&billing::GAMMA, [])
                .about("get the gamma spender and the deposits of the linked hotkeys"),
            Gate::Open,
        ),
        Gamma::Hotkeys(GammaHotkeys::Create { hotkey_file }) => link(ctx, hotkey_file),
        Gamma::Hotkeys(GammaHotkeys::Delete { hotkey }) => {
            let request = Request::to(&billing::GAMMA_HOTKEY_DELETE, [hotkey])
                .about(format!("unlink the hotkey {}", phrase::quote(hotkey)));
            print_body_or(
                ctx,
                &ctx.load_authed_client()?,
                &request,
                Gate::Confirm {
                    consequence: Some(
                        "a conversion from it then needs it linked again, which takes its words",
                    ),
                },
                json!({ "hotkey": hotkey, "deleted": true }),
            )
        }
        Gamma::Conversions(GammaConversions::Create { hotkey, amount }) => {
            convert(ctx, hotkey, amount)
        }
        Gamma::Conversions(GammaConversions::List(args)) => list::run(
            ctx,
            &ctx.load_authed_client()?,
            &Request::to(&billing::GAMMA_CONVERSIONS, []).about("list the gamma conversions"),
            args,
        ),
        Gamma::Conversions(GammaConversions::Get { conversion_id }) => print_body(
            ctx,
            &ctx.load_authed_client()?,
            &Request::to(&billing::GAMMA_CONVERSION, [conversion_id])
                .about(format!("get the gamma conversion `{conversion_id}`")),
            Gate::Open,
        ),
    }
}

/// The hotkey's words from `path` (`-` for stdin), cleared when dropped.
fn hotkey_words(path: &Path) -> Result<(Zeroizing<String>, Source)> {
    let source = Source::File {
        flag: FLAG,
        path: path.to_path_buf(),
    };
    let words = if path == Path::new("-") {
        Input {
            path,
            flag: FLAG,
            what: "hotkey phrase",
        }
        .read_secret()?
    } else {
        hippius::read_hotkey_file(path, FLAG)?
    };
    Ok((words, source))
}

/// Links the hotkey whose words `path` holds: the challenge for its address, the signature
/// of the challenge's message, and the link. No error quotes the words.
fn link(ctx: &Context<'_>, path: &Path) -> Result<Printed> {
    let (words, source) = hotkey_words(path)?;
    let key = Hotkey::from_phrase(&words, &source)?;
    drop(words);
    let hotkey = key.ss58_address().to_owned();
    let hotkey = hotkey.as_str();
    let client = ctx.load_authed_client()?;
    let challenge = Request::to(&billing::GAMMA_HOTKEY_CHALLENGE, [])
        .json(&billing::HotkeyChallengeBody { hotkey })
        .about(format!(
            "ask for a link challenge for the hotkey {}",
            phrase::quote(hotkey)
        ));
    let submit = |nonce: &str, signature: &str| {
        Request::to(&billing::GAMMA_HOTKEY_CREATE, [])
            .json(&billing::HotkeyCreateBody { nonce, signature })
            .about(format!("link the hotkey {}", phrase::quote(hotkey)))
    };
    if ctx.dry_run() {
        let unsigned = submit(
            "<the challenge's nonce>",
            "<the hotkey's sr25519 signature of the challenge message>",
        );
        return ctx
            .dry_run_of(&client, &[&challenge, &unsigned])
            .map(Printed::from);
    }
    let answer = view_of(&ctx.send(&client, &challenge)?.document()?);
    let billing::HotkeyChallenge {
        message: Some(message),
        nonce: Some(nonce),
    } = answer
    else {
        return Err(crate::commands::unreadable(
            "ask for a gamma link challenge",
        ));
    };
    let signature = key.sign_gamma_link(&message, &nonce)?;
    // The keys are not needed for the link itself, so they are cleared before it is sent.
    drop(key);
    print_body(ctx, &client, &submit(&nonce, &signature), Gate::Open)
}

/// Burns `amount` gamma of `hotkey`'s deposit for the same amount in USD, once confirmed,
/// and names the read that follows the conversion.
fn convert(ctx: &Context<'_>, hotkey: &str, amount: &str) -> Result<Printed> {
    let client = ctx.load_authed_client()?;
    let request = Request::to(&billing::GAMMA_CONVERSION_CREATE, [])
        .json(&billing::GammaConversionBody { hotkey, amount })
        .about(format!(
            "burn `{amount}` gamma of the hotkey {} for `{amount}` USD of credits",
            phrase::quote(hotkey)
        ));
    let gate = Gate::Confirm {
        consequence: Some(
            "the gamma is burned on chain and cannot be refunded; the balance grows by the same \
             amount in USD once the spend is final",
        ),
    };
    print_with(ctx, &client, &request, gate, |response| {
        let body = response.document()?;
        let conversion: billing::GammaConversion = view_of(&body);
        let Some(id) = conversion
            .id
            .as_deref()
            .and_then(|id| crate::cli::as_argument(id, crate::cli::conversion_id))
        else {
            // The API took the conversion, and may burn the gamma, but its answer names no
            // conversion to follow: not a success to build on.
            return Err(Error::partial_failure(
                "the API accepted the conversion, but its answer names no conversion id to \
                 follow it by",
                body,
                Value::Null,
                SideEffects::Partial,
            )
            .with_hint(
                "run `hipc billing gamma conversions list` to find it before converting again",
            ));
        };
        ctx.note(&format!(
            "the conversion is pending: the balance grows once the spend is in a finalized \
             block; `hipc billing gamma conversions get {id}` follows it"
        ));
        Ok(body)
    })
}
