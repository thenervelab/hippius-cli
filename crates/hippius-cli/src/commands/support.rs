//! `hipc support tickets`: open a ticket, list the token's own tickets, read one, reply, list
//! its messages, and close it.
//!
//! Every route is under `/api/support/tickets/`, which acts as the signed-in user whatever
//! account is selected, so the account header is never sent there. A new ticket is read by
//! a person, so it needs `--yes`; a reply to an open ticket does not. A close ends the
//! support team's work on the ticket, as a cancel does, so it needs `--yes` too. The message
//! of a create or a reply is read from `--body-file`, never from argv. Attachments (a file
//! upload) and the chat handoff stay on `hipc api`.

use hippius::routes::support;
use hippius::{Code, Error, Request, Result, phrase};
use serde_json::{Map, Value, json};
use zeroize::Zeroizing;

use crate::cli::{
    Support, TicketBody, TicketCreate, TicketList, TicketMessages, TicketUpdateStatus, Tickets,
};
use crate::commands::{Input, Printed, list, print_body};
use crate::context::{self, Context, Gate, Write};

/// The largest message read, in bytes. Swagger sets no maximum on a ticket's `description`
/// or a message's `body`; the limit is arbitrary, far above a written message, and stops a
/// wrong path (a log, a disk image) from being read whole and sent.
const MAX_MESSAGE_BYTES: u64 = 1024 * 1024;

pub(crate) fn run(ctx: &Context<'_>, command: &Support) -> Result<Printed> {
    let Support::Tickets(command) = command;
    match command {
        Tickets::List(args) => list_tickets(ctx, args),
        Tickets::Get { ticket_id } => {
            let request = Request::to(&support::TICKET, [ticket_id])
                .listed_by("hipc support tickets list")
                .about(format!("get the ticket `{ticket_id}`"));
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        Tickets::Create(args) => {
            let description = message(&args.body)?;
            let client = client_without_token_in(ctx, &[&args.subject, &description])?;
            let request = create_request(args, &description);
            let gate = Gate::Confirm {
                consequence: Some("a person on the Hippius support team reads it"),
            };
            print_body(ctx, &client, &request, gate)
        }
        Tickets::Reply { ticket_id, body } => {
            let text = message(body)?;
            let client = client_without_token_in(ctx, &[&text])?;
            let request = Request::to(&support::MESSAGE_CREATE, [ticket_id])
                .json(&json!({ "body": text }))
                .listed_by("hipc support tickets list")
                .about(format!("reply to the ticket `{ticket_id}`"));
            print_body(ctx, &client, &request, Gate::Open)
        }
        Tickets::Update { ticket_id, status } => close(ctx, *ticket_id, *status),
        Tickets::Messages(TicketMessages::List { ticket_id, list }) => {
            // Swagger pages this route (`page`, `page_size`) and documents a bare array; the
            // list reads both. A message body is the user's or the support team's own text,
            // which text mode escapes like every string from the server.
            let request = Request::to(&support::MESSAGES, [ticket_id])
                .listed_by("hipc support tickets list")
                .about(format!("list the messages of the ticket `{ticket_id}`"));
            list::run(ctx, &ctx.load_authed_client()?, &request, list)
        }
    }
}

/// `hipc support tickets update --status closed`: reads the ticket, so the question names
/// its subject (the tenet 5 exception for a read that names the target), then sends a
/// `PATCH` with the one field given, as the console's close does. The read goes out after
/// the close's refusals and the token check, and a ticket it shows closed already is
/// `conflict`, with nothing else sent. A dry run sends neither: its document is the
/// `PATCH`, and its `requests` lists the read, then the `PATCH`.
fn close(ctx: &Context<'_>, ticket_id: u64, status: TicketUpdateStatus) -> Result<Printed> {
    let client = ctx.load_authed_client()?;
    let read = Request::to(&support::TICKET, [&ticket_id])
        .listed_by("hipc support tickets list")
        .about(format!("get the ticket `{ticket_id}`"));
    let draft = Request::to(&support::TICKET_UPDATE, [&ticket_id])
        .json(&json!({ "status": status.code() }))
        .listed_by("hipc support tickets list")
        .about(format!("close the ticket `{ticket_id}`"));
    let checked = match ctx.check_write(&client, &draft)? {
        Write::DryRun(mut doc) => {
            doc["requests"] = ctx.describe_each(&client, &[&read, &draft])?;
            return Ok(Printed::Document(doc));
        }
        Write::Checked(checked) => checked,
    };
    let ticket = ctx.send(&client, &read)?.document()?;
    if ticket.get("status").and_then(Value::as_str) == Some(status.code()) {
        return Err(Error::new(
            Code::Conflict,
            format!("the ticket `{ticket_id}` is already closed"),
        ));
    }
    // Swagger requires a subject; a ticket without one is still closed, named by its id.
    let action = match ticket.get("subject").and_then(Value::as_str) {
        Some(subject) => format!("close the ticket `{ticket_id}`, {}", phrase::quote(subject)),
        None => format!("close the ticket `{ticket_id}`"),
    };
    let consequence = "the support team stops working on it";
    ctx.confirm_write(checked, &action, Some(consequence))?;
    // Swagger answers the whole ticket, printed as sent.
    ctx.send(&client, &draft)?.document().map(Printed::Document)
}

/// `GET /api/support/tickets/`, with `status` only when `--status` is given.
fn list_tickets(ctx: &Context<'_>, args: &TicketList) -> Result<Printed> {
    let request = Request::to(&support::TICKETS, []);
    let request = match args.status {
        Some(status) => request
            .query("status", status.code())
            .about(format!("list the `{}` tickets", status.code())),
        None => request.about("list the tickets"),
    };
    list::run(ctx, &ctx.load_authed_client()?, &request, &args.list)
}

/// `POST /api/support/tickets/ {"subject", "description", "priority"?}`.
fn create_request(args: &TicketCreate, description: &str) -> Request {
    let mut body = Map::new();
    body.insert("subject".to_owned(), args.subject.as_str().into());
    body.insert("description".to_owned(), description.into());
    if let Some(priority) = args.priority {
        body.insert("priority".to_owned(), priority.code().into());
    }
    Request::to(&support::TICKET_CREATE, [])
        .json(&Value::Object(body))
        .about(format!(
            "open the support ticket {}",
            phrase::quote(&args.subject)
        ))
}

/// The client for a ticket or a reply whose text is `texts`, after a refusal of text that
/// holds a secret ([`secret_in`]). A person reads what is sent, so a message an injected
/// instruction filled with the user's config would hand that person the account. The refusal
/// comes before the dry run, which would print the body, and never quotes the text.
fn client_without_token_in(ctx: &Context<'_>, texts: &[&str]) -> Result<hippius::Client> {
    let config = context::config()?;
    if let Some(secret) = secret_in(ctx, &config, texts)? {
        return Err(Error::usage(
            format!(
                "the ticket text holds {}, which a person on the Hippius support team would read",
                secret.what()
            ),
            format!(
                "remove {} from the subject and the message; the support team never needs it",
                secret.it()
            ),
        ));
    }
    ctx.authed_client(&config)
}

/// A secret [`secret_in`] finds in a text a person on the support team reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Secret {
    /// `HIPPIUS_TOKEN`, or a token a profile of the config file stores.
    Token,
    /// What [`hippius::holds_checked_phrase`] takes for an access key.
    AccessKey,
}

impl Secret {
    /// The secret as a refusal names it, after "holds".
    pub(crate) fn what(self) -> &'static str {
        match self {
            Self::Token => "an API token",
            Self::AccessKey => "what looks like an access key",
        }
    }

    /// The secret as a hint names it, after "remove".
    pub(crate) fn it(self) -> &'static str {
        match self {
            Self::Token => "the token",
            Self::AccessKey => "the 12 words",
        }
    }
}

/// The secret any of `texts` holds that a person must not read, or `None`:
///
/// - `HIPPIUS_TOKEN`, or a token any profile of `config` stores, in any spelling an error
///   redacts it in (tenet 9), also with the text's whitespace removed, so a token wrapped
///   across lines or split by a space is found;
/// - an access key whose checksum holds ([`hippius::holds_checked_phrase`]), with anything
///   that is not a letter read as a space, so a numbered or comma-separated list of the words
///   is found, while everyday prose of listed words is not taken for one.
///
/// It guards against an accident, such as a pasted config file or a log, not against text
/// written to hide a secret: an encoded or reordered one is not found. `hipc api` applies
/// the same check to the support routes ([`crate::commands::api`]).
pub(crate) fn secret_in(
    ctx: &Context<'_>,
    config: &hippius::ConfigFile,
    texts: &[&str],
) -> Result<Option<Secret>> {
    let variable = ctx.overrides()?.token;
    let holds_token = |text: &str| {
        variable.as_ref().is_some_and(|token| token.is_in(text))
            || config.holds_a_stored_token(text)
    };
    // The texts joined too, so a token split between the subject and the message is found.
    let mut all = Zeroizing::new(String::with_capacity(
        texts.iter().map(|text| text.len() + 1).sum(),
    ));
    for text in texts {
        all.push_str(text);
        all.push('\n');
    }
    for text in texts.iter().copied().chain([all.as_str()]) {
        // Cleared when dropped, and sized up front so it never grows and leaves a copy
        // behind. Whitespace and the invisible characters that join a word are dropped, so a
        // wrapped token is whole again.
        let mut joined = Zeroizing::new(String::with_capacity(text.len()));
        joined.extend(
            text.chars()
                .filter(|c| !c.is_whitespace() && !hippius::INVISIBLE_JOINS.contains(c)),
        );
        if holds_token(text) || holds_token(&joined) {
            return Ok(Some(Secret::Token));
        }
        if hippius::holds_checked_phrase(text) {
            return Ok(Some(Secret::AccessKey));
        }
    }
    Ok(None)
}

/// The message `--body-file` names, as written. Swagger gives the ticket's `description`
/// and a message's `body` a minimum length of 1, and the server trims them, so a blank
/// message is refused here, before the question and before anything is sent.
fn message(body: &TicketBody) -> Result<String> {
    let input = Input {
        path: &body.body_file,
        flag: "--body-file",
        what: "message",
    };
    let text = input.read_text(MAX_MESSAGE_BYTES)?;
    if text.trim().is_empty() {
        return Err(Error::usage(
            format!("the message from {} is blank", input.name()),
            "write the message into the file `--body-file` names",
        ));
    }
    Ok(text)
}
