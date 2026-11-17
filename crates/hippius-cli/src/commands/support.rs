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
use hippius::{Error, Failure, Request, Result, SideEffects, phrase, view_of};
use zeroize::Zeroizing;

use crate::cli::{
    Support, TicketBody, TicketCreate, TicketList, TicketMessages, TicketUpdateStatus, Tickets,
};
use crate::commands::{Decided, Input, Printed, list, print_body, read_then_write};
use crate::context::{self, Context, Gate, WritePlan};

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
            let fields = [&args.category, &args.resource_type, &args.resource_id];
            let texts: Vec<&str> = [&args.subject]
                .into_iter()
                .chain(fields.into_iter().flatten())
                .map(String::as_str)
                .chain([description.as_str()])
                .collect();
            let client = client_without_token_in(ctx, &texts)?;
            let request = create_request(args, &description);
            let gate = Gate::Confirm {
                consequence: Some("a person on the Hippius support team reads it"),
            };
            print_body(ctx, &client, &request, gate)
        }
        Tickets::Reply {
            ticket_id,
            body,
            client_msg_id,
        } => {
            let text = message(body)?;
            let client = client_without_token_in(ctx, &[&text])?;
            let id = match client_msg_id {
                Some(id) => MessageId::Given(id.clone()),
                None => MessageId::New(hippius::new_idempotency_key()?),
            };
            let request = Request::to(&support::MESSAGE_CREATE, [ticket_id])
                .json(&support::MessageCreateBody {
                    body: &text,
                    client_msg_id: id.as_str(),
                })
                .listed_by("hipc support tickets list")
                .about(format!("reply to the ticket `{ticket_id}`"));
            print_body(ctx, &client, &request, Gate::Open)
                .map_err(|error| reply_failed(ctx, error, *ticket_id, &id, body))
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
        .json(&support::TicketUpdateBody {
            status: status.code(),
        })
        .listed_by("hipc support tickets list")
        .about(format!("close the ticket `{ticket_id}`"));
    let decide = || {
        let ticket: support::Ticket = view_of(&ctx.send(&client, &read)?.document()?);
        if ticket.status.as_deref() == Some(status.code()) {
            return Err(Error::new(
                Failure::Conflict,
                format!("the ticket `{ticket_id}` is already closed"),
            ));
        }
        // Swagger requires a subject; a ticket without one is still closed, named by its id.
        let action = match ticket.subject {
            Some(subject) => format!("close the ticket `{ticket_id}`, {}", phrase::quote(subject)),
            None => format!("close the ticket `{ticket_id}`"),
        };
        let consequence = "the support team stops working on it";
        Ok(Decided::asked_as(draft.clone(), action, Some(consequence)))
    };
    // Swagger answers the whole ticket, printed as sent.
    let plan = WritePlan::new(&[&read], &draft);
    read_then_write(ctx, &client, &plan, decide, |sent| sent?.document())
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

/// The `client_msg_id` a reply sends.
#[derive(Debug)]
enum MessageId {
    /// The one `--client-msg-id` gave, already on the command line.
    Given(String),
    /// A new one this run made.
    New(String),
}

impl MessageId {
    fn as_str(&self) -> &str {
        match self {
            Self::Given(id) | Self::New(id) => id,
        }
    }
}

/// The error of a reply that may have been posted, with a hint that names the messages list,
/// to see whether it was. After a 5xx or no answer at all, the reply may not have been posted,
/// so the hint also gives the run's own command line with `--client-msg-id` added, unless the
/// run already gave one, so a second run sends the same `client_msg_id`; it says when that
/// command reads the message from stdin again. A 2xx whose body could not be read posted the
/// reply, and a redirect is not followed, so neither offers to send it again. Swagger documents
/// no deduplication by the id, so the hint promises none. Any other error is returned as is.
fn reply_failed(
    ctx: &Context<'_>,
    error: Error,
    ticket_id: u64,
    id: &MessageId,
    body: &TicketBody,
) -> Error {
    if !matches!(error.side_effects(), SideEffects::Partial) {
        return error;
    }
    let list = format!(
        "run `hipc support tickets messages list {ticket_id}` to see whether the reply was posted"
    );
    if error.http_status().is_some_and(|status| status < 500) {
        return error.with_hint(list);
    }
    let again = match id {
        MessageId::Given(_) => phrase::command(ctx.args()),
        MessageId::New(id) => phrase::command_line(ctx.args(), &["--client-msg-id", id]),
    };
    let stdin = if body.body_file.as_os_str() == "-" {
        ", with the message on stdin again"
    } else {
        ""
    };
    error.with_hint(format!(
        "{list}; to send it again with the same client_msg_id, run `{again}`{stdin}"
    ))
}

/// `POST /api/support/tickets/ {"subject", "description", "priority"?, "category"?,
/// "resource_type"?, "resource_id"?}`.
fn create_request(args: &TicketCreate, description: &str) -> Request {
    let body = support::TicketCreateBody {
        subject: &args.subject,
        description,
        priority: args.priority.map(crate::cli::TicketPriority::code),
        category: args.category.as_deref(),
        resource_type: args.resource_type.as_deref(),
        resource_id: args.resource_id.as_deref(),
    };
    Request::to(&support::TICKET_CREATE, [])
        .json(&body)
        .about(format!(
            "open the support ticket {}",
            phrase::quote(&args.subject)
        ))
}

/// The client for a ticket or a reply whose text is `texts`, after a refusal of text that
/// holds a secret ([`secret_in_any_order`]). A person reads what is sent, so a message an injected
/// instruction filled with the user's config would hand that person the account. The refusal
/// comes before the dry run, which would print the body, and never quotes the text.
fn client_without_token_in(ctx: &Context<'_>, texts: &[&str]) -> Result<hippius::Client> {
    let config = context::config()?;
    if let Some(secret) = secret_in_any_order(ctx, &config, texts)? {
        return Err(Error::usage(
            format!(
                "the ticket text holds {}, which a person on the Hippius support team would read",
                secret.what()
            ),
            format!(
                "remove {} from the ticket text; the support team never needs it",
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
/// Each text is scanned, then all of them joined in the order given, so a secret split from
/// one text into the next is found. It guards against an accident, such as a pasted config
/// file or a log, not against text written to hide a secret: an encoded or reordered one is
/// not found. `hipc api` applies this check to the support routes ([`crate::commands::api`]);
/// a ticket or a reply of `hipc support` is scanned in every order ([`secret_in_any_order`]).
pub(crate) fn secret_in(
    ctx: &Context<'_>,
    config: &hippius::ConfigFile,
    texts: &[&str],
) -> Result<Option<Secret>> {
    let found = secret_finder(ctx, config)?;
    let mut all = Zeroizing::new(String::with_capacity(
        texts.iter().map(|text| text.len() + 1).sum(),
    ));
    for text in texts {
        all.push_str(text);
        all.push('\n');
    }
    Ok(texts.iter().copied().chain([all.as_str()]).find_map(found))
}

/// The secret `texts`, the separate texts of one ticket or reply, hold, as [`secret_in`]
/// finds one, but with the texts joined in every order rather than the one given: nothing
/// says which the user wrote first, so a key or a token that starts in the message and
/// continues in a flag is found as well as the reverse, and so is one split across three. A
/// long text is joined by its edges alone ([`edges`]), so the scans stay small: a create has
/// at most five texts, 120 orders.
fn secret_in_any_order(
    ctx: &Context<'_>,
    config: &hippius::ConfigFile,
    texts: &[&str],
) -> Result<Option<Secret>> {
    let found = secret_finder(ctx, config)?;
    if let Some(secret) = texts.iter().copied().find_map(&found) {
        return Ok(Some(secret));
    }
    let edges: Vec<Zeroizing<String>> = texts.iter().map(|text| edges(text)).collect();
    let size = edges.iter().map(|edge| edge.len() + 1).sum();
    for order in orders(texts.len()) {
        // Sized up front so it never grows and leaves a copy behind; cleared when dropped.
        let mut joined = Zeroizing::new(String::with_capacity(size));
        for at in order {
            joined.push_str(&edges[at]);
            joined.push('\n');
        }
        if let Some(secret) = found(&joined) {
            return Ok(Some(secret));
        }
    }
    Ok(None)
}

/// What finds a secret in one text, as [`secret_in`] describes it.
fn secret_finder<'a>(
    ctx: &Context<'_>,
    config: &'a hippius::ConfigFile,
) -> Result<impl Fn(&str) -> Option<Secret> + 'a> {
    let variable = ctx.overrides()?.token;
    let holds_token = move |text: &str| {
        variable.as_ref().is_some_and(|token| token.is_in(text))
            || config.holds_a_stored_token(text)
    };
    Ok(move |text: &str| {
        // Cleared when dropped, and sized up front so it never grows and leaves a copy
        // behind. Whitespace and the invisible characters that join a word are dropped, so a
        // wrapped token is whole again.
        let mut joined = Zeroizing::new(String::with_capacity(text.len()));
        joined.extend(
            text.chars()
                .filter(|c| !c.is_whitespace() && !hippius::INVISIBLE_JOINS.contains(c)),
        );
        if holds_token(text) || holds_token(&joined) {
            Some(Secret::Token)
        } else if hippius::holds_checked_phrase(text) {
            Some(Secret::AccessKey)
        } else {
            None
        }
    })
}

/// The characters kept at each end of a long text joined with others, 4096. Arbitrary: far
/// more than the part of a secret split into the next text can take, a console token (40
/// characters, 120 percent-encoded) or the 23 words of a 24-word key with what lies between
/// them, and small enough that 120 joins of five texts stay a few megabytes of scanning.
const EDGE_CHARS: usize = 4096;

/// What stands for the middle of a long text cut to its edges: a NUL, which no token
/// spelling holds, around `x`, which is not on the word list, so neither a token nor a run of
/// listed words is read across the cut.
const EDGE_GAP: &str = "\0x\0";

/// `text` as it is when at most twice [`EDGE_CHARS`] characters, else its first and last
/// [`EDGE_CHARS`] characters around [`EDGE_GAP`]. A secret split between this text and
/// another lies at one of its edges, since the whole text was scanned alone first.
fn edges(text: &str) -> Zeroizing<String> {
    let count = text.chars().count();
    if count <= 2 * EDGE_CHARS {
        return Zeroizing::new(text.to_owned());
    }
    let at = |nth: usize| {
        text.char_indices()
            .nth(nth)
            .map_or(text.len(), |(at, _)| at)
    };
    let (head, tail) = (&text[..at(EDGE_CHARS)], &text[at(count - EDGE_CHARS)..]);
    let mut edges = Zeroizing::new(String::with_capacity(
        head.len() + EDGE_GAP.len() + tail.len(),
    ));
    edges.push_str(head);
    edges.push_str(EDGE_GAP);
    edges.push_str(tail);
    edges
}

/// Every order of the indices `0..n`: n! lists.
fn orders(n: usize) -> Vec<Vec<usize>> {
    let Some(last) = n.checked_sub(1) else {
        return vec![Vec::new()];
    };
    let mut all = Vec::new();
    for shorter in orders(last) {
        for at in 0..=shorter.len() {
            let mut order = shorter.clone();
            order.insert(at, last);
            all.push(order);
        }
    }
    all
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_lists_every_order_once() {
        assert_eq!(orders(0), [Vec::<usize>::new()]);
        assert_eq!(orders(1), [[0]]);
        let mut three = orders(3);
        three.sort();
        assert_eq!(
            three,
            [
                [0, 1, 2],
                [0, 2, 1],
                [1, 0, 2],
                [1, 2, 0],
                [2, 0, 1],
                [2, 1, 0]
            ]
        );
        assert_eq!(orders(5).len(), 120);
    }

    /// A text up to twice the edge is kept whole; a longer one keeps its first and last
    /// `EDGE_CHARS` characters, counted as characters, around the gap.
    #[test]
    fn a_long_text_is_cut_to_its_edges() {
        let whole = "é".repeat(2 * EDGE_CHARS);
        assert_eq!(*edges(&whole), whole);
        let long = format!(
            "{}{}{}",
            "a".repeat(EDGE_CHARS),
            "é",
            "b".repeat(EDGE_CHARS)
        );
        let cut = format!(
            "{}{EDGE_GAP}{}",
            "a".repeat(EDGE_CHARS),
            "b".repeat(EDGE_CHARS)
        );
        assert_eq!(*edges(&long), cut);
        let long = format!("é{}", "b".repeat(2 * EDGE_CHARS));
        let cut = format!(
            "é{}{EDGE_GAP}{}",
            "b".repeat(EDGE_CHARS - 1),
            "b".repeat(EDGE_CHARS)
        );
        assert_eq!(*edges(&long), cut);
    }
}
