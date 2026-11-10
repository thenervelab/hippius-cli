//! One page of a paginated list, or every page with `--all` (`TENETS.md` tenet 6): by page
//! number, or by cursor.

use hippius::{Client, Error, Page, Request, Result, phrase};
use serde_json::Value;

use crate::cli::{CursorArgs, ListArgs};
use crate::commands::{Pages, Printed};
use crate::context::{Context, Gate, Step};

/// Fetches `request` with `page_size` set from `--limit`. One page prints the envelope
/// with `next_page_token`. `--all` follows the `page` numbers of the `next` links until the
/// server stops, and prints one JSON array.
pub(crate) fn run(
    ctx: &Context<'_>,
    client: &Client,
    request: &Request,
    args: &ListArgs,
) -> Result<Printed> {
    // The next page comes from the `next` link alone, never from `page_size`, so a size the
    // server names is not checked: an empty list may name 0, and `Pages` stops a repeat.
    run_paged(ctx, client, request, args, &hippius::page)
}

/// The key a page's items sit under, as `hippius::page` reads them.
pub(crate) const RESULTS: &str = "results";

/// [`run`], for a route that pages by number but answers `{page, page_size, total, <items>}`
/// with no `next` link, as the Hub model index does: the next page is the one after `page`
/// while `page` times `page_size` is below `total` and the page held items. `items` is the
/// key the answer holds its items under, such as `results` or a model's `artifacts`; the
/// envelope prints them as `items`.
pub(crate) fn run_by_total(
    ctx: &Context<'_>,
    client: &Client,
    request: &Request,
    args: &ListArgs,
    items: &str,
) -> Result<Printed> {
    run_paged(ctx, client, request, args, &|answer| {
        let mut page = hippius::page(with_items_as_results(answer, items))?;
        next_from_total(&mut page)?;
        Ok(page)
    })
}

/// Sets `page.next` from its `page`, `page_size`, and `total` when it has no `next` link.
/// Whether `page` is the page asked for is [`answered_as_asked`]'s check.
///
/// # Errors
///
/// `internal` when any of the three is missing or not a whole number, when a page of items
/// has a `page_size` of 0, or when a page is empty though `total` says items remain past
/// the pages before it: reading the page as the last would print a short list as if it
/// were complete, and working out the next page from a size of 0 would hand out a next
/// page forever.
fn next_from_total(page: &mut Page) -> Result<()> {
    if page.next.is_some() {
        return Ok(());
    }
    // As leniently as `answered_as_asked` reads `page`: a decimal string or `2.0` too.
    let number = |key: &str| page.fields.get(key).and_then(whole_number);
    let (Some(at), Some(size), Some(total)) =
        (number("page"), number("page_size"), number("total"))
    else {
        return Err(Error::internal(
            "the API answered a page without a whole-number `page`, `page_size`, and `total`, \
             so whether more pages exist is unknown",
        ));
    };
    // An empty page is the last one whatever its size says.
    if size == 0 && !page.items.is_empty() {
        return Err(Error::internal(
            "the API answered a page of items with a `page_size` of 0, so where the next page \
             starts is unknown",
        ));
    }
    if page.items.is_empty() {
        // An empty page past the end ends the list. One that `total` puts items on, as the
        // pages before it hold fewer than `total`, is a hole.
        if at.saturating_sub(1).saturating_mul(size) < total {
            return Err(Error::internal(format!(
                "the API answered page `{at}` empty though its `total` of `{total}` puts \
                 items on it, so the rest of the list is unknown"
            )));
        }
        return Ok(());
    }
    if at.saturating_mul(size) < total {
        // The form `hippius::page` reads a `next` link's page number from.
        page.next = Some(format!("?page={}", at.saturating_add(1)));
    }
    Ok(())
}

fn run_paged(
    ctx: &Context<'_>,
    client: &Client,
    request: &Request,
    args: &ListArgs,
    // Reads an answer as a page.
    read: &dyn Fn(Value) -> Result<Page>,
) -> Result<Printed> {
    let limit = args.limit.to_string();
    let page_request = |page: Option<&str>| {
        let request = request.clone().query("page_size", &limit);
        match page {
            Some(page) => request.query("page", page),
            None => request,
        }
    };
    let mut items = Vec::new();
    // The start page counts as seen, so a `next` link back to it is caught before it is
    // fetched again. Without `--page-token` the start is page 1.
    let mut pages = Pages::starting_at(args.page_token.as_deref().unwrap_or("1"));
    let mut page_number = args.page_token.clone();
    // Whether a page came before this one, under `--all`.
    let mut read_one = false;
    loop {
        let request = page_request(page_number.as_deref());
        let answer = match ctx.call(client, &request, Gate::Open)? {
            Step::Sent(response) => {
                pages.read(response.bytes().len())?;
                response.json()?
            }
            Step::DryRun(doc) => return Ok(Printed::Document(doc)),
        };
        let page = read(answer)?;
        // A first page asked by `--page-token` may be gone because the list shrank since the
        // token was printed; a later page under `--all`, because it shrank while it was
        // read. A first page asked for without a token has no such excuse: a bug, so the
        // default hint.
        let start_over = if read_one {
            Some(CHANGED_WHILE_READ)
        } else {
            args.page_token.as_ref().map(|_| CHANGED_SINCE_TOKEN)
        };
        // Without `--page-token` the first page is page 1.
        answered_as_asked(&page, page_number.as_deref().unwrap_or("1"), start_over)?;
        if !args.all {
            return Ok(Printed::Document(page.envelope()?));
        }
        let next = page.next_page_token()?;
        items.extend(page.items);
        let Some(number) = next else {
            return Ok(Printed::Document(Value::Array(items)));
        };
        pages.admit(&number)?;
        page_number = Some(number);
        read_one = true;
    }
}

/// The hint of a first page asked by `--page-token` that is not the page asked for.
const CHANGED_SINCE_TOKEN: &str =
    "run again without `--page-token`, as the list may have changed while it was read";

/// The hint of a later page under `--all` that is not the page asked for.
pub(crate) const CHANGED_WHILE_READ: &str =
    "run the command again, as the list may have changed while it was read";

/// Refuses a page that names another `page` than `asked`, the page token sent: a server
/// that answers page 1 to `page=2` would hand out `2` as the next token forever. A page
/// that names no `page`, or a null one, as a DRF page does not, is read as the page asked
/// for, and so is one that names it as a number, a decimal string, or a float such as `2.0`.
///
/// # Errors
///
/// `internal` for a `page` that is not a whole number, and for one that is another number
/// than `asked`, with `start_over` as its hint when given.
pub(crate) fn answered_as_asked(page: &Page, asked: &str, start_over: Option<&str>) -> Result<()> {
    let Some(at) = page.fields.get("page").filter(|at| !at.is_null()) else {
        return Ok(());
    };
    let Some(at) = whole_number(at) else {
        let shown = match at {
            Value::String(text) => phrase::quote(text),
            other => phrase::quote(other),
        };
        let what = if digits(at) {
            "too large to be a page number"
        } else {
            "not a whole number"
        };
        return Err(Error::internal(format!(
            "the API answered a `page` that is {what}, {shown}, so the list cannot be read in \
             order"
        )));
    };
    if asked.parse() == Ok(at) {
        return Ok(());
    }
    // Most likely the list shrank and the server clamped the page number; the code stays
    // `internal`, as the answer cannot be read as the page asked for.
    let error = Error::internal(format!(
        "the API answered page `{at}` to a request for page `{asked}`, so the list cannot be \
         read in order"
    ));
    Err(match start_over {
        Some(hint) => error.with_hint(hint),
        None => error,
    })
}

/// `value` as a whole number: a JSON number without a fraction, `2.0` included, or a
/// string of decimal digits, at most `u64::MAX`.
fn whole_number(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64().or_else(|| digits_of(number)?.parse().ok()),
        Value::String(text) if digits(value) => text.parse().ok(),
        _ => None,
    }
}

/// Whether `value` is a whole number in decimal digits, of any size: a string of digits, or
/// a JSON number whose float has no fraction and no sign.
fn digits(value: &Value) -> bool {
    let all_digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    match value {
        Value::Number(number) => number.as_u64().is_some() || digits_of(number).is_some(),
        Value::String(text) => all_digits(text),
        _ => false,
    }
}

/// A JSON number's float as its shortest text, when that is all digits: `2.0` is `2`, and
/// `1e20` is `100000000000000000000`, without a lossy cast; a fraction or a sign is `None`.
fn digits_of(number: &serde_json::Number) -> Option<String> {
    let text = number.as_f64()?.to_string();
    text.bytes().all(|b| b.is_ascii_digit()).then_some(text)
}

/// `answer` with its items under `results`, where `hippius::page` reads them, when the route
/// holds them under `key`. An answer without `key`, or with a `results` of its own, is left
/// as it is, so it reads as the route sent it.
fn with_items_as_results(answer: Value, key: &str) -> Value {
    match answer {
        Value::Object(mut fields) if key != RESULTS && !fields.contains_key(RESULTS) => {
            if let Some(items) = fields.shift_remove(key) {
                fields.insert(RESULTS.to_owned(), items);
            }
            Value::Object(fields)
        }
        other => other,
    }
}

/// Fetches `request`, with `before` set from `--page-token`, from a route that pages by
/// cursor (`{results, next_before}`). One page prints the envelope, whose
/// `next_page_token` is the cursor. `--all` sends each `next_before` back as `before`
/// until it is null, and prints one JSON array.
pub(crate) fn run_cursor(
    ctx: &Context<'_>,
    client: &Client,
    request: &Request,
    args: &CursorArgs,
) -> Result<Printed> {
    let page_request = |cursor: Option<&str>| match cursor {
        Some(cursor) => request.clone().query("before", cursor),
        None => request.clone(),
    };
    let mut items = Vec::new();
    // The start counts as seen, so a cursor back to it is caught before it is fetched
    // again. The first page has no cursor; `""` stands for it, and no cursor is empty.
    let mut pages = Pages::starting_at(args.page_token.as_deref().unwrap_or(""));
    let mut cursor = args.page_token.clone();
    loop {
        let request = page_request(cursor.as_deref());
        let answer = match ctx.call(client, &request, Gate::Open)? {
            Step::Sent(response) => {
                pages.read(response.bytes().len())?;
                response.json()?
            }
            Step::DryRun(doc) => return Ok(Printed::Document(doc)),
        };
        let page = hippius::cursor_page(answer)?;
        if !args.all {
            return Ok(Printed::Document(page.envelope()));
        }
        items.extend(page.items);
        let Some(next) = page.next_page_token else {
            return Ok(Printed::Document(Value::Array(items)));
        };
        pages.admit(&next)?;
        cursor = Some(next);
    }
}
