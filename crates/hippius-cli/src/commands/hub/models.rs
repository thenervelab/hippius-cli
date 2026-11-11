//! `hipc hub events` and `hipc hub models`: the namespace's audit events, and the model
//! index, both read-only.
//!
//! The model index is the catalog the Hub's public model pages read. Swagger lists it beside
//! the customer routes, and its own words say an anonymous caller sees the public namespaces
//! and a signed-in one its own private namespaces too, so it is a customer read, not a staff
//! view: it is sent with the token when one is configured and without one otherwise, as the
//! Hub plan catalog is. It is not in the console's lists of personal or account routes, so a
//! membership is refused before anything is sent (tenet 12) rather than answered with the
//! caller's own private models.

use hippius::routes::hub;
use hippius::{Request, Result};

use crate::cli::{HubEvents, HubModels};
use crate::commands::{Printed, list, print_body, print_envelope};
use crate::context::{self, Context, Gate};

/// `GET /api/registry/events/` (swagger `registry_events_list`). Swagger lists no page
/// parameter, and the `hippius-hub` client sends none, so it is read whole: a `next` link is
/// `internal` rather than a list cut short.
pub(super) fn events(ctx: &Context<'_>, command: &HubEvents) -> Result<Printed> {
    match command {
        HubEvents::List => print_envelope(
            ctx,
            &ctx.load_authed_client()?,
            &Request::to(&hub::EVENTS, []).about("list the Hub audit events"),
        ),
    }
}

pub(super) fn run(ctx: &Context<'_>, command: &HubModels) -> Result<Printed> {
    let client = ctx.client(&context::config()?)?;
    match command {
        // Swagger lists no parameter, but the index pages by `page` and `page_size` and
        // answers `{page, page_size, total, results}` with no `next` link (the `hippius-hub`
        // client and the Hub's model pages), so the next page comes from `total`.
        HubModels::List(args) => list::run_by_total(
            ctx,
            &client,
            &Request::to(&hub::MODELS, []).about("list the Hub models"),
            args,
            list::RESULTS,
        ),
        // An object of three lists (`formats`, `architectures`, `quantizations`), the
        // filters of the index, printed as sent.
        HubModels::Formats => print_body(
            ctx,
            &client,
            &Request::to(&hub::MODEL_FORMATS, []).about("get the Hub model formats"),
            Gate::Open,
        ),
        // Both names were checked when argv was parsed, so neither can change the route.
        HubModels::Get {
            model,
            reference,
            page,
        } => match reference {
            // The versions page as the index does, `{page, page_size, total, artifacts}`
            // with no `next` link, so they are listed as the index is.
            None => list::run_by_total(
                ctx,
                &client,
                &Request::to(&hub::MODEL, [model])
                    .listed_by("hipc hub models list")
                    .about(format!("get the Hub model `{model}`")),
                page,
                "artifacts",
            ),
            Some(reference) => print_body(
                ctx,
                &client,
                &Request::to(&hub::MODEL_VERSION, [model, reference])
                    .listed_by(format!("hipc hub models get {model}"))
                    .about(format!(
                        "get the version `{reference}` of the Hub model `{model}`"
                    )),
                Gate::Open,
            ),
        },
    }
}
