//! The one page loop (`TENETS.md` tenet 6): one page of a paginated list, or every page with
//! `--all`, read the way the route's row says the server pages it
//! ([`hippius::routes::Paging`]), and the reads of `hipc s3 tokens list` and `hipc api --all`,
//! which follow pages through it too.

use std::collections::HashSet;

use hippius::routes::Paging;
use hippius::{Client, Error, Next, Page, Request, Result, phrase};
use serde_json::Value;

use crate::cli::{
    CursorArgs, ListArgs, MAX_CONTINUATION_TOKEN_BYTES, ObjectsList, continuation_token,
};
use crate::commands::Printed;
use crate::context::Context;

/// The page flags of one list command: `--limit`, which a cursor list does not take,
/// `--page-token`, and `--all`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PageFlags<'a> {
    limit: Option<u32>,
    page_token: Option<&'a str>,
    all: bool,
}

impl<'a> From<&'a ListArgs> for PageFlags<'a> {
    fn from(args: &'a ListArgs) -> Self {
        Self {
            limit: Some(args.limit),
            page_token: args.page_token.as_deref(),
            all: args.all,
        }
    }
}

impl<'a> From<&'a CursorArgs> for PageFlags<'a> {
    fn from(args: &'a CursorArgs) -> Self {
        Self {
            limit: None,
            page_token: args.page_token.as_deref(),
            all: args.all,
        }
    }
}

impl<'a> From<&'a ObjectsList> for PageFlags<'a> {
    fn from(args: &'a ObjectsList) -> Self {
        Self {
            limit: Some(args.limit),
            page_token: args.page_token.as_deref(),
            all: args.all,
        }
    }
}

/// Fetches `request`, a read of a route its row says is paged, with `--limit` as the page
/// size. One page prints the envelope with `next_page_token`. `--all` follows each next page
/// until the server stops, and prints one JSON array.
///
/// # Panics
///
/// For a request whose route is not paged: only a paged route's command takes the page flags
/// (`spec::tests`).
pub(crate) fn run<'f>(
    ctx: &Context<'_>,
    client: &Client,
    request: &Request,
    flags: impl Into<PageFlags<'f>>,
) -> Result<Printed> {
    let flags = flags.into();
    let paging = request
        .paging()
        .expect("a command takes the page flags only on a route its row says is paged");
    let limit = flags.limit.map(|limit| limit.to_string());
    run_with(ctx, client, paging, flags, |token| {
        at_page(
            sized(request.clone(), paging, limit.as_deref()),
            paging,
            token,
        )
    })
}

/// [`run`], with `page_request` building the request for each page from its token, `None`
/// for the first page without `--page-token`: for a route whose filters go between the size
/// and the token in its query, as a bucket's objects do.
pub(crate) fn run_with(
    ctx: &Context<'_>,
    client: &Client,
    paging: Paging,
    flags: PageFlags<'_>,
    page_request: impl Fn(Option<&str>) -> Request,
) -> Result<Printed> {
    let first = page_request(flags.page_token);
    if ctx.dry_run() {
        return ctx.dry_run_of(client, &[&first]).map(Printed::from);
    }
    // A numbered list starts at page 1; a token list's first page has no token, and `""`
    // stands for it, since no token is empty.
    let first_page = if paging.numbered() { "1" } else { "" };
    let start = flags.page_token.unwrap_or(first_page);
    let mut asked = start.to_owned();
    let mut items = Vec::new();
    let mut after_first_page = false;
    walk(ctx, client, first, start, |answer| {
        let page = read_page(paging, answer)?;
        if paging.numbered() {
            // A first page asked by `--page-token` may be gone because the list shrank since
            // the token was printed; a later page under `--all`, because it shrank while it
            // was read. A first page asked for without a token has no such excuse: a bug, so
            // the default hint.
            let start_over = if after_first_page {
                Some(CHANGED_WHILE_READ)
            } else {
                flags.page_token.map(|_| CHANGED_SINCE_TOKEN)
            };
            answered_as_asked(&page, &asked, start_over)?;
        }
        if !flags.all {
            return Ok(Turn::Done(Printed::List(page.envelope()?)));
        }
        let next = page.next_page_token()?;
        items.extend(page.items);
        let Some(token) = next else {
            let items = std::mem::take(&mut items);
            return Ok(Turn::Done(Printed::List(Value::Array(items))));
        };
        after_first_page = true;
        asked.clone_from(&token);
        let request = page_request(Some(&token));
        Ok(Turn::Next { token, request })
    })
}

/// `request` with `limit` as its page size, in the query `paging` names; as it is for a route
/// whose server fixes the size, or with no limit.
pub(crate) fn sized(request: Request, paging: Paging, limit: Option<&str>) -> Request {
    match (paging.size_query(), limit) {
        (Some(name), Some(limit)) => request.query(name, limit),
        _ => request,
    }
}

/// `request` asking for the page `token` names, in the query `paging` names; the first page
/// for `None`.
pub(crate) fn at_page(request: Request, paging: Paging, token: Option<&str>) -> Request {
    match token {
        Some(token) => request.query(paging.token_query(), token),
        None => request,
    }
}

/// An answer read as a page of `paging`.
pub(crate) fn read_page(paging: Paging, answer: Value) -> Result<Page> {
    match paging {
        // The next page comes from the `next` link alone, never from `page_size`, so a size the
        // server names is not checked: an empty list may name 0, and `Pages` stops a repeat.
        Paging::Numbered => hippius::page(answer),
        Paging::Totalled { items } => {
            let mut page = hippius::page(with_items_as_results(answer, items))?;
            next_from_total(&mut page)?;
            Ok(page)
        }
        Paging::UntilShort { items } => {
            let mut page = hippius::page(with_items_as_results(answer, items))?;
            next_while_full(&mut page)?;
            Ok(page)
        }
        Paging::Cursor => hippius::cursor_page(answer),
        Paging::Continuation => objects_page(answer),
    }
}

/// What the page loop does after a page ([`walk`]).
#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "one is returned per page read and moved into the loop at once; a box would only add an allocation"
)]
pub(crate) enum Turn<T> {
    /// The list ends here with `T`.
    Done(T),
    /// The list goes on with `request`, the page `token` names.
    Next { token: String, request: Request },
}

/// Reads a list page by page: sends `first`, hands each answer to `turn`, and sends the next
/// page it names, until it is done. `start` names the first page as [`Pages`] counts it.
/// Every page is counted, and every next page admitted, through one [`Pages`], so a loop, a
/// list past [`MAX_PAGES`], or one past [`MAX_LIST_BYTES`] is refused the same way for every
/// list.
///
/// # Panics
///
/// Under `--dry-run`, which the caller answers first, since nothing is sent.
pub(crate) fn walk<T>(
    ctx: &Context<'_>,
    client: &Client,
    first: Request,
    start: &str,
    mut turn: impl FnMut(Value) -> Result<Turn<T>>,
) -> Result<T> {
    let mut pages = Pages::starting_at(start);
    let mut request = first;
    loop {
        let response = ctx.send(client, &request)?;
        pages.read(response.bytes().len())?;
        match turn(response.json()?)? {
            Turn::Done(done) => return Ok(done),
            Turn::Next {
                token,
                request: next,
            } => {
                pages.admit(&token)?;
                request = next;
            }
        }
    }
}

/// The key a page's items sit under, as `hippius::page` reads them.
const RESULTS: &str = "results";

/// Sets `page.next` from its `page`, `page_size`, and `total` when it has no `next` link, for
/// a route that pages by number but answers `{page, page_size, total, <items>}` with no `next`
/// link, as the Hub model index does: the next page is the one after `page` while `page`
/// times `page_size` is below `total` and the page held items. Whether `page` is the page
/// asked for is [`answered_as_asked`]'s check.
///
/// # Errors
///
/// `internal` when any of the three is missing or not a whole number, when a page of items
/// has a `page_size` of 0, or when a page holds fewer items than `total` puts on it (all of
/// `page_size`, or what remains of `total` after the pages before it on the last page),
/// an empty one included: reading the page as complete would print a short list as if it
/// were, and working out the next page from a size of 0 would hand out a next page
/// forever.
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
    // The items `total` puts on this page: a whole page, or on the last one what the pages
    // before it leave. An empty page past the end ends the list.
    let placed = total
        .saturating_sub(at.saturating_sub(1).saturating_mul(size))
        .min(size);
    let held = page.items.len() as u64;
    if held < placed {
        let answered = if held == 0 {
            format!("empty though its `total` of `{total}` puts {placed} items on it")
        } else {
            format!("with {held} of the {placed} items its `total` of `{total}` puts on it")
        };
        return Err(Error::internal(format!(
            "the API answered page `{at}` {answered}, so the rest of the list is unknown"
        )));
    }
    if page.items.is_empty() {
        return Ok(());
    }
    if at.saturating_mul(size) < total {
        page.next = Some(Next::Token(at.saturating_add(1).to_string()));
    }
    Ok(())
}

/// Sets `page.next` from its `page` and `page_size` when it has no `next` link, for a route
/// that pages by number but answers `{page, page_size, <items>}` with neither a `next` link
/// nor a total, as the Hub usage by repository does: the next page is the one after `page`
/// while the page is full, so the list ends at the first page shorter than `page_size`.
/// Whether `page` is the page asked for is [`answered_as_asked`]'s check.
///
/// # Errors
///
/// `internal` when `page` or `page_size` is missing or not a whole number, or when a page of
/// items has a `page_size` of 0: either way where the list ends is unknown.
fn next_while_full(page: &mut Page) -> Result<()> {
    if page.next.is_some() {
        return Ok(());
    }
    // As leniently as `answered_as_asked` reads `page`: a decimal string or `2.0` too.
    let number = |key: &str| page.fields.get(key).and_then(whole_number);
    let (Some(at), Some(size)) = (number("page"), number("page_size")) else {
        return Err(Error::internal(
            "the API answered a page without a whole-number `page` and `page_size`, so whether \
             more pages exist is unknown",
        ));
    };
    if size == 0 && !page.items.is_empty() {
        return Err(Error::internal(
            "the API answered a page of items with a `page_size` of 0, so where the next page \
             starts is unknown",
        ));
    }
    if !page.items.is_empty() && page.items.len() as u64 >= size {
        page.next = Some(Next::Token(at.saturating_add(1).to_string()));
    }
    Ok(())
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
/// `internal` when `page` is not a whole number or is another number than `asked`, with
/// `start_over` as its hint when given.
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

/// Reads an S3 `ListObjectsV2` answer, a page of a bucket's objects: `Contents`, with
/// `NextContinuationToken` as the token and the answer's other fields, such as `KeyCount` and
/// `IsTruncated`, beside them. `Contents` may be missing, as boto3 leaves it out of
/// an empty page. An empty `NextContinuationToken` is no token, as the AWS SDK paginators
/// read it, so a loop never sends it and fetches the first page again.
///
/// # Errors
///
/// `internal` when the answer is not an object, its `Contents` is not an array, its token is
/// not text or is one `--page-token` would refuse, or it is an `IsTruncated: true` page with
/// no token or has an `IsTruncated` that is neither a boolean nor `null`: reading that
/// as the last page would print a short list as if it were complete. A page with
/// `IsTruncated: false` is the last, whatever token it carries.
fn objects_page(answer: Value) -> Result<Page> {
    let not_a_list =
        || Error::internal("the API answered a list request with something other than a list");
    let Value::Object(mut fields) = answer else {
        return Err(not_a_list());
    };
    let items = match fields.shift_remove("Contents") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items,
        Some(_) => return Err(not_a_list()),
    };
    let next = match fields.shift_remove("NextContinuationToken") {
        None | Some(Value::Null) => None,
        Some(Value::String(token)) => Some(token).filter(|token| !token.is_empty()),
        Some(_) => {
            return Err(Error::internal(
                "the API's `NextContinuationToken` is not text, so the next page cannot be read",
            ));
        }
    };
    let next = match (fields.get("IsTruncated"), next) {
        // S3 sends a token only on a truncated page; one beside `IsTruncated: false` leads
        // nowhere, and following it would fetch a page the server says does not exist.
        (Some(Value::Bool(false)), _) => None,
        (Some(Value::Bool(true)), None) => {
            return Err(Error::internal(
                "the API says the list goes on (`IsTruncated`) but gives no \
                 `NextContinuationToken`, so the next page cannot be read",
            ));
        }
        // A string `"true"` or a number `1` is no boolean to trust either way: read as absent
        // it would make a page without a token the last, a short list printed as complete.
        (Some(truncated), _) if !truncated.is_null() && !truncated.is_boolean() => {
            return Err(Error::internal(
                "the API's `IsTruncated` is not `true` or `false`, so whether the list goes on \
                 cannot be read",
            ));
        }
        (_, next) => next,
    };
    // Held to the rule `--page-token` checks, so a printed `next_page_token` is always one
    // the next call accepts. The token is not quoted: it is the server's, and may be long.
    if let Some(token) = &next
        && continuation_token(token).is_err()
    {
        return Err(Error::internal(format!(
            "the API's `NextContinuationToken` is not one `--page-token` takes: it is empty, \
             holds a space or a control character, or is longer than \
             {MAX_CONTINUATION_TOKEN_BYTES} bytes"
        )));
    }
    Ok(Page {
        items,
        next: next.map(Next::Token),
        fields,
    })
}

/// The most pages one command follows. Arbitrary: far more than any account's list holds
/// at the API's page sizes, and it stops a server that keeps answering a new `next` from
/// turning one command into a request storm and an unbounded buffer.
const MAX_PAGES: usize = 1000;

/// The most bytes of answers one list keeps across its pages (`s3 tokens list` reads two
/// lists, each held to this). Arbitrary: every page is
/// held until the list ends, and each may be up to the client's 32 MiB body cap, so the
/// page cap alone would allow gigabytes. No account's list comes near this.
const MAX_LIST_BYTES: usize = 64 * 1024 * 1024;

/// The pages one list has followed. Every page loop reports each page it read and admits
/// each `next` through it, so a link back to a page already read, a page past
/// [`MAX_PAGES`], or a list past [`MAX_LIST_BYTES`] is refused the same way everywhere.
#[derive(Debug)]
struct Pages {
    seen: HashSet<String>,
    bytes: usize,
}

impl Pages {
    /// A list whose first page is `first`: a page number, a cursor, or a URL for `hipc api`.
    fn starting_at(first: &str) -> Self {
        Self {
            seen: HashSet::from([first.to_owned()]),
            bytes: 0,
        }
    }

    /// Counts a page of `bytes` that was read.
    ///
    /// # Errors
    ///
    /// `internal` once the list holds more than [`MAX_LIST_BYTES`].
    fn read(&mut self, bytes: usize) -> Result<()> {
        self.bytes = self.bytes.saturating_add(bytes);
        if self.bytes > MAX_LIST_BYTES {
            return Err(Error::internal(format!(
                "the list is larger than {} MiB across its pages, so reading stopped",
                MAX_LIST_BYTES / (1024 * 1024)
            )));
        }
        Ok(())
    }

    /// Admits the next page.
    ///
    /// # Errors
    ///
    /// `internal` when `next` was already read (a loop) or is past [`MAX_PAGES`].
    fn admit(&mut self, next: &str) -> Result<()> {
        if self.seen.len() >= MAX_PAGES {
            return Err(Error::internal(format!(
                "the list goes on past {MAX_PAGES} pages, so reading stopped"
            )));
        }
        if !self.seen.insert(next.to_owned()) {
            // A URL (`hipc api`'s next link) shows no query values (tenet 9); a page number or
            // an opaque cursor is data, quoted whole, since cutting it at a `?` or `#` would
            // name another token.
            let shown = if next.contains("://") {
                hippius::without_query_values(next)
            } else {
                next.to_owned()
            };
            return Err(Error::internal(format!(
                "the API's next page leads back to {}, which was already read",
                phrase::quote(shown)
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use hippius::Code;

    use super::*;

    /// A totalled page that holds fewer items than its `total` puts on it is `internal`,
    /// whether it is empty, short of a whole page, or short of the last page's remainder; a
    /// whole page goes on, and a last page or one past the end ends the list.
    #[test]
    fn a_totalled_page_short_of_what_its_total_places_on_it_is_internal() {
        let read = |at: u64, size: u64, total: u64, held: usize| {
            let answer = serde_json::json!({
                "page": at,
                "page_size": size,
                "total": total,
                "artifacts": vec![Value::Null; held],
            });
            read_page(Paging::Totalled { items: "artifacts" }, answer)
        };
        for (at, size, total, held) in [(1, 100, 80, 50), (1, 2, 5, 1), (3, 2, 5, 0), (2, 2, 5, 0)]
        {
            let error = read(at, size, total, held).unwrap_err();
            assert_eq!(error.code(), Code::Internal, "{at} {size} {total} {held}");
            let answered = if held == 0 {
                format!("page `{at}` empty")
            } else {
                format!("page `{at}` with {held} of the")
            };
            assert!(error.message().contains(&answered), "{}", error.message());
        }
        let next = |at, size, total, held| read(at, size, total, held).unwrap().next;
        assert_eq!(next(1, 2, 5, 2), Some(Next::Token("2".to_owned())));
        assert_eq!(next(3, 2, 5, 1), None);
        assert_eq!(next(1, 100, 80, 80), None);
        assert_eq!(next(4, 2, 5, 0), None);
    }

    /// A page of a list that names no total goes on while it is full, and ends at the first
    /// short page, an empty one included; with no whole-number `page` and `page_size`, or a
    /// `page_size` of 0 beside items, where the list ends is unknown.
    #[test]
    fn a_page_without_a_total_goes_on_while_it_is_full() {
        let read = |answer: Value| {
            read_page(
                Paging::UntilShort {
                    items: "repositories",
                },
                answer,
            )
        };
        let page = |at: u64, size: u64, held: usize| serde_json::json!({"page": at, "page_size": size, "repositories": vec![Value::Null; held]});
        let next = |at, size, held| read(page(at, size, held)).unwrap().next;
        assert_eq!(next(1, 2, 2), Some(Next::Token("2".to_owned())));
        assert_eq!(next(2, 2, 1), None);
        assert_eq!(next(3, 2, 0), None);
        assert_eq!(next(1, 0, 0), None);
        // A `next` link, if the server ever sends one, is the next page as it names it.
        let linked = serde_json::json!({
            "page": 1,
            "page_size": 1,
            "repositories": [null],
            "next": "https://api.hippius.com/api/registry/usage/repositories/?page=2",
        });
        assert!(matches!(read(linked).unwrap().next, Some(Next::Link(_))));
        for answer in [
            page(1, 0, 1),
            serde_json::json!({"page": 1, "repositories": [null]}),
            serde_json::json!({"page_size": "x", "page": 1, "repositories": []}),
        ] {
            let error = read(answer.clone()).unwrap_err();
            assert_eq!(error.code(), Code::Internal, "{answer}");
        }
    }

    #[test]
    fn pages_refuse_a_loop_a_page_past_the_cap_and_a_list_past_its_bytes() {
        let mut pages = Pages::starting_at("1");
        assert_eq!(pages.admit("1").unwrap_err().code(), Code::Internal);
        pages.admit("2").unwrap();
        pages.read(MAX_LIST_BYTES).unwrap();
        assert_eq!(pages.read(1).unwrap_err().code(), Code::Internal);
        let mut pages = Pages::starting_at("1");
        for n in 2..=MAX_PAGES {
            pages.admit(&n.to_string()).unwrap();
        }
        assert_eq!(pages.admit("x").unwrap_err().code(), Code::Internal);
    }

    #[test]
    fn a_loop_quotes_a_cursor_whole_and_a_url_without_its_query_values() {
        let looped = |next: &str| {
            let mut pages = Pages::starting_at(next);
            pages.admit(next).unwrap_err().message().to_owned()
        };
        assert_eq!(
            looped("a?b=c#d`e\n"),
            "the API's next page leads back to `a?b=c#d\\u{60}e\\n`, which was already read"
        );
        assert_eq!(
            looped("https://api.example/x/?page=2&key=s3cr3t"),
            "the API's next page leads back to `https://api.example/x/?page=<redacted>&key=<redacted>`, which was already read"
        );
    }
}
