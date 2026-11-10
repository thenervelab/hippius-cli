//! `hipc account referrals`: the signed-in user's referral links, and what they brought.
//!
//! `/api/referral-codes/` and `/api/referral/analytics/` act as the signed-in user: swagger
//! speaks of "my referral codes" and "the current user's referral code", and the console
//! sends no account header to them. So they are personal routes, and a membership, project,
//! or billing account selected is never sent there: a member cannot reach the owner's code
//! through them, and neither header could make the server answer with another user's.
//!
//! A referral code and its links are made to be shared, so they are not secrets (tenet 9)
//! and need no `--reveal`. Nothing here spends, redeems, or pays out: a create mints the
//! user's code on chain on first use, paid by the server's signer, and a rename changes a
//! label stored on Hippius only. So no command here is confirmed (tenet 5).

use hippius::{Method, Request, Result, phrase};
use serde_json::{Value, json};

use crate::cli::Referrals;
use crate::commands::{Printed, print_body, print_list_with};
use crate::context::{Context, Gate};

/// The command that lists the referral link ids, for the hint of a 404.
const LIST: &str = "hipc account referrals list";

pub(crate) fn run(ctx: &Context<'_>, command: &Referrals) -> Result<Printed> {
    let client = ctx.load_authed_client()?;
    match command {
        Referrals::List => {
            let request = Request::get("/referral-codes/").about("list the referral links");
            print_list_with(ctx, &client, &request, |response| {
                let envelope =
                    hippius::page(response.json()?)?.complete_envelope(request.target())?;
                warn_on_codes(ctx, &envelope);
                Ok(envelope)
            })
        }
        Referrals::Create { name } => {
            let request = Request::new(Method::Post, "/referral-codes/")
                .json(&json!({ "name": name }))
                .about(format!("create the referral link {}", phrase::quote(name)));
            print_body(ctx, &client, &request, Gate::Open)
        }
        Referrals::Update { referral_id, name } => {
            let request = Request::new(Method::Patch, format!("/referral-codes/{referral_id}/"))
                .json(&json!({ "name": name }))
                .listed_by(LIST)
                .about(format!(
                    "rename the referral link `{referral_id}` to {}",
                    phrase::quote(name)
                ));
            print_body(ctx, &client, &request, Gate::Open)
        }
        Referrals::Analytics { refresh } => {
            let request = Request::get("/referral/analytics/");
            let request = if *refresh {
                request.query("refresh", "true")
            } else {
                request
            };
            print_body(
                ctx,
                &client,
                &request.about("read the referral analytics"),
                Gate::Open,
            )
        }
    }
}

/// Warns, in text mode, when the links carry more than one referral `code`. Swagger says every
/// entry shares "the acting user's single on-chain code", and the route acts as the token's
/// user, so a second code is the server's inconsistency, not another user's link reaching
/// the caller: refusing the list would lock the user out of their own links for it, so they
/// are printed with a warning. An empty or missing `code` (a link made before the code was
/// minted) is not a second code.
fn warn_on_codes(ctx: &Context<'_>, envelope: &Value) {
    let mut codes = envelope["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("code").and_then(Value::as_str))
        .filter(|code| !code.is_empty());
    let first = codes.next();
    if codes.any(|code| Some(code) != first) {
        ctx.warning(
            "these links carry more than one referral `code`, though a user has only one: \
             check each link's `code` before sharing it",
        );
    }
}
