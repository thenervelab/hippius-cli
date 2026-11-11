//! The HTTP client for the console API.
//!
//! Every request carries `User-Agent: hipc/<version>`, because the API answers a default
//! client agent with 403 and error 1010. A token goes out as `Authorization: Token <token>`,
//! and only to the configured API origin. The account header follows [`crate::account`].
//! A non-2xx answer becomes an [`Error`] whose code comes from one status map.
//!
//! The client never retries and never follows a redirect: a retry could charge twice, and a
//! redirect could carry the token to another host.

use std::cell::RefCell;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use zeroize::Zeroizing;

use ureq::unversioned::resolver::DefaultResolver;
use ureq::unversioned::transport::{
    Buffers, ConnectionDetails, Connector, DefaultConnector, NextTimeout,
};

use crate::account::{self, Rule};
use crate::config::{Credential, Settings};
use crate::encode::percent_encode;
use crate::error::{Code, Error, Failure, Result, SideEffects};
use crate::method::{Access, Method};
use crate::phrase::quote;
use crate::project;
use crate::redact::{is_secret_field, token_spellings};
use crate::routes::{API_BASE, Endpoint, Guard, Paging, Route, RouteFlag};
use crate::selector::{BillingId, ProjectId, Selection, Selector};
use crate::source::{Given, Source};
use crate::time::date_of_unix_day;
use crate::url::{Origin, without_query_values};

/// The largest response body `hipc` reads, in bytes. Console answers are small JSON
/// documents; 32 MiB is an arbitrary ceiling far above any of them that still bounds
/// memory if a server misbehaves.
const MAX_BODY_BYTES: u64 = 32 * 1024 * 1024;

/// The longest URL the `http` crate builds a request for, in bytes: its `MAX_LEN`,
/// `u16::MAX - 1` (http 1.5.0, `src/uri/mod.rs`, which refuses a longer one as `uri too
/// long`).
const MAX_URL_BYTES: usize = (u16::MAX - 1) as usize;

/// The longest `X-Request-Id` kept. Arbitrary: far above a UUID or a trace id, and short
/// enough that a server cannot fill an error with one.
const MAX_REQUEST_ID_BYTES: usize = 200;

/// The most characters of one server string an error message quotes, such as a `detail`.
/// Arbitrary: a whole sentence or a few field errors, short enough that a server cannot fill
/// a log with one message; `details` keeps the whole body.
const MAX_QUOTED_CHARS: usize = 512;

/// The hint when no token is configured for `profile`: one imperative (tenet 15). An agent
/// cannot sign in, so it names who can and how: the user, at their own terminal, where
/// `hipc auth login` asks without echo, for the same profile, with a token created in the
/// console's settings.
fn no_token_hint(profile: &str) -> String {
    let login = if profile == crate::config::DEFAULT_PROFILE {
        "hipc auth login".to_owned()
    } else {
        format!("hipc auth login --profile {profile}")
    };
    format!(
        "ask the user to run {} in their own terminal, with a token created at \
         `https://console.hippius.com/dashboard/settings`",
        quote(&login)
    )
}

/// One request: a method, a path under `/api` or a full URL, a query, and a body. `Debug`
/// shows the path and counts the rest: a query value or a body can be a secret.
#[derive(Clone)]
pub struct Request {
    method: Method,
    target: String,
    query: Vec<(String, String)>,
    /// Cleared when dropped: a body can hold a secret sent once, such as an App Engine
    /// launch's secrets or a DNS provider credential.
    body: Option<Zeroizing<Vec<u8>>>,
    /// What the request does, as [`Request::action`] says it.
    about: Option<String>,
    /// [`Request::listed_by`].
    listed_by: Option<String>,
    /// [`Request::accept`].
    accept: Option<&'static str>,
    /// [`Request::if_match`].
    if_match: Option<String>,
    /// [`Request::idempotency_key`].
    idempotency_key: Option<String>,
    /// Whether the request may change state: from the method, unless
    /// [`Request::changes_nothing`] or [`Request::provisions`] says otherwise.
    access: Access,
    /// How the route pages its list, from its row ([`Request::paging`]).
    paging: Option<Paging>,
}

impl Request {
    /// A request without a body. `target` is a swagger path such as `/accounts/` (sent under
    /// [`API_BASE`], to `/api/accounts/`), a path that already starts with `/api/`, or a full
    /// URL. A command builds its request from a route instead ([`Request::to`]).
    pub fn new(method: Method, target: impl Into<String>) -> Self {
        Self {
            method,
            target: target.into(),
            query: Vec::new(),
            body: None,
            about: None,
            listed_by: None,
            accept: None,
            if_match: None,
            idempotency_key: None,
            access: if method.is_read() {
                Access::Read
            } else {
                Access::Write
            },
            paging: None,
        }
    }

    /// A GET.
    pub fn get(target: impl Into<String>) -> Self {
        Self::new(Method::Get, target)
    }

    /// A request without a body to `route`, with `values` in its placeholders
    /// ([`Route::path`]). The target is kept as `hipc api` takes it, without [`API_BASE`], so
    /// a hint that names the route reads as a caller would type it; the client adds the base
    /// back when it sends. The row decides how the request pages and whether a read of it
    /// provisions ([`Request::provisions`]), so no caller marks either by hand.
    pub fn to<const N: usize>(route: &Route<N>, values: [&dyn fmt::Display; N]) -> Self {
        let path = route.path(values);
        let target = path
            .strip_prefix(API_BASE)
            .expect("a route's template starts with the API base, which `Route::new` checks");
        let endpoint = route.endpoint();
        let request = Self {
            paging: endpoint.paging(),
            ..Self::new(route.method(), target)
        };
        if endpoint.provisions() {
            request.provisions()
        } else {
            request
        }
    }

    /// How the route the request was built for ([`Request::to`]) pages its list, as its row
    /// says; `None` for a route the server does not page and for a request built from a
    /// written target.
    pub fn paging(&self) -> Option<Paging> {
        self.paging
    }

    /// Names what the request does: a verb phrase with its literals in backticks, such as
    /// "delete the VM `vm_1`". The confirmation asks about it, and an error from the answer
    /// says it could not be done (tenet 15). It holds no secret: ids and names only.
    #[must_use]
    pub fn about(mut self, action: impl Into<String>) -> Self {
        self.about = Some(action.into());
        self
    }

    /// Names the command that lists what the request addresses by id, such as `hipc compute
    /// vms list`: a 404 then hints to run it.
    #[must_use]
    pub fn listed_by(mut self, command: impl Into<String>) -> Self {
        self.listed_by = Some(command.into());
        self
    }

    /// Sends `Accept: <media_type>` in place of the `Accept: application/json` every other
    /// request carries, for a route that answers another format when asked for it, such as
    /// an invoice PDF.
    #[must_use]
    pub fn accept(mut self, media_type: &'static str) -> Self {
        self.accept = Some(media_type);
        self
    }

    /// Sends `If-Match: <version>` as given, unquoted, as the Hippius WAF takes it
    /// (<https://docs.hippius.com/use/cdn/firewall>); the server refuses the write with 412
    /// when the resource changed since `version` was read. Only curated commands set it, from
    /// a version they read: `hipc api` takes no header.
    #[must_use]
    pub fn if_match(mut self, version: impl Into<String>) -> Self {
        self.if_match = Some(version.into());
        self
    }

    /// Sends `Idempotency-Key: <key>`: the server answers a repeat with the same key as it
    /// answered the first, rather than doing the work again. Only curated commands set it, on
    /// a route that honours it: `hipc api` takes no header.
    #[must_use]
    pub fn idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(key.into());
        self
    }

    /// What the request does: what [`Request::about`] named, else `call` and the method and
    /// target, with every query value redacted, as `hipc api` gives them.
    pub fn action(&self) -> String {
        self.about.clone().unwrap_or_else(|| {
            format!(
                "call `{} {}`",
                self.method,
                without_query_values(&self.target)
            )
        })
    }

    /// The target as given to [`Request::new`]: a swagger path such as `/accounts/`, as
    /// `hipc api` takes it, a path under `/api/`, or a full URL. Without the query.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Adds a query parameter. The value is percent-encoded when the URL is built.
    #[must_use]
    pub fn query(mut self, name: &str, value: &str) -> Self {
        self.query.push((name.to_owned(), value.to_owned()));
        self
    }

    /// Sets a JSON body, sent with `Content-Type: application/json`: a route's body type
    /// (beside its route in [`crate::routes`]), or a `Value` `hipc api` was given.
    ///
    /// # Panics
    ///
    /// When `body` cannot be written as JSON: a map whose keys are not strings, or a
    /// `Serialize` impl that fails. A route's body is strings, numbers, booleans, lists and
    /// structs of them, so it always can.
    #[must_use]
    pub fn json(mut self, body: &impl Serialize) -> Self {
        // Measured first and written into a buffer of that size, so the buffer never grows:
        // a growing `Vec` frees each smaller copy uncleared, and the body may hold a secret.
        let mut size = ByteCount(0);
        serde_json::to_writer(&mut size, body).expect("a request body is plain JSON data");
        let mut bytes = Zeroizing::new(Vec::with_capacity(size.0));
        serde_json::to_writer(&mut *bytes, body).expect("a request body is plain JSON data");
        self.body = Some(bytes);
        self
    }

    /// Sets a body the caller already holds as JSON bytes.
    #[must_use]
    pub fn raw_json(mut self, body: Vec<u8>) -> Self {
        self.body = Some(Zeroizing::new(body));
        self
    }

    /// Marks a POST that changes nothing on the server, such as an estimate: a failure is
    /// then a read's, with no side effects and safe to retry, as `hipc schema` calls the
    /// command `read_only`.
    #[must_use]
    pub fn changes_nothing(mut self) -> Self {
        self.access = Access::Read;
        self
    }

    /// Marks a GET that can create something on the server, such as the private network a
    /// first `GET /api/network/me/` provisions, for a request built from a written target
    /// ([`Request::to`] takes it from the route's row): it is then a write for the selector rules,
    /// so a stored project is refused rather than skipped (a stored billing account is
    /// skipped, as on every route that takes none), and a failure after it was sent has
    /// `partial` side effects.
    #[must_use]
    pub fn provisions(mut self) -> Self {
        self.access = Access::Write;
        self
    }

    /// The method.
    pub fn method(&self) -> Method {
        self.method
    }

    /// Whether the request only reads: a GET or a HEAD not marked with
    /// [`Request::provisions`], or one marked with [`Request::changes_nothing`]. Every other
    /// request may change state.
    pub fn is_read(&self) -> bool {
        self.access == Access::Read
    }

    pub(crate) fn access(&self) -> Access {
        self.access
    }
}

/// A writer that keeps nothing and counts the bytes written to it.
#[derive(Debug)]
struct ByteCount(usize);

impl std::io::Write for ByteCount {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("target", &without_query_values(&self.target))
            .field("query_params", &self.query.len())
            .field("body_len", &self.body.as_deref().map(Vec::len))
            .field("about", &self.about)
            .field("listed_by", &self.listed_by)
            .field("accept", &self.accept)
            .field("if_match", &self.if_match.is_some())
            .field("idempotency_key", &self.idempotency_key.is_some())
            .field("access", &self.access)
            .field("paging", &self.paging)
            .finish()
    }
}

/// A 2xx answer.
pub struct Response {
    status: u16,
    body: Vec<u8>,
    /// The request was not a read, so the server may have changed state.
    write: bool,
    /// [`Request::action`], for an error about the body.
    action: String,
    /// The server's `X-Request-Id`, so a report of an unreadable body can be traced.
    request_id: Option<String>,
    /// The server's `Date` header, as sent.
    date: Option<String>,
    /// The spellings of the token the request carried ([`token_spellings`]), redacted from
    /// an error about the body as [`Client::send`] redacts them from an error answer, and
    /// cleared on drop. Empty when no token went out.
    token_spellings: Vec<String>,
}

impl Drop for Response {
    fn drop(&mut self) {
        self.token_spellings
            .iter_mut()
            .for_each(zeroize::Zeroize::zeroize);
    }
}

impl Response {
    /// The status code.
    pub fn status(&self) -> u16 {
        self.status
    }

    /// The body bytes, exactly as received.
    pub fn bytes(&self) -> &[u8] {
        &self.body
    }

    /// The body bytes, exactly as received, taken without a copy.
    pub fn into_bytes(mut self) -> Vec<u8> {
        std::mem::take(&mut self.body)
    }

    /// The server's `Date` header, as sent, when it sent one that is visible ASCII.
    pub fn date(&self) -> Option<&str> {
        self.date.as_deref()
    }

    /// A 200 answer to a read with an empty body and this `Date` header, for a test of code
    /// that reads the date.
    #[cfg(test)]
    pub(crate) fn dated(date: Option<&str>) -> Self {
        Self {
            status: 200,
            body: Vec::new(),
            write: false,
            action: "read the thing".to_owned(),
            request_id: None,
            date: date.map(str::to_owned),
            token_spellings: Vec::new(),
        }
    }

    /// The body as JSON. An empty body is `null`. Numbers keep every digit the server
    /// sent.
    ///
    /// # Errors
    ///
    /// `internal` when the body is not JSON.
    pub fn json(&self) -> Result<Value> {
        parse_json(&self.body).ok_or_else(|| self.unusable("a body that is not JSON"))
    }

    /// The body as a JSON document, for a command that prints it: [`Response::json`], but
    /// an empty body is refused too, so stdout is never `null` on success
    /// (tenet 3). A command whose route may answer 204 prints its own document
    /// for that instead.
    ///
    /// # Errors
    ///
    /// `internal` when the body is empty or not JSON.
    pub fn document(&self) -> Result<Value> {
        match self.json()? {
            Value::Null => Err(self.unusable("an empty body")),
            document => Ok(document),
        }
    }

    /// The body as a PDF document, for a route asked for one with [`Request::accept`].
    ///
    /// # Errors
    ///
    /// `internal` when the body does not start with the `%PDF-` header every PDF file
    /// starts with (ISO 32000-2, section 7.5.2), such as a JSON body.
    pub fn pdf(&self) -> Result<&[u8]> {
        if self.body.starts_with(b"%PDF-") {
            Ok(&self.body)
        } else {
            Err(self.unusable("a body that is not a PDF"))
        }
    }

    /// A 2xx answer the command cannot take as its result, for a reason the caller found in
    /// the body: `internal`, as ``cannot <action>: <why>``, with the status, the request id,
    /// and the body as `details` when it is JSON, so the refusal can be traced like a
    /// server error. `partial` after a write, as [`Response::document`] is.
    ///
    /// The body is cleaned as an error answer's is: the secret fields are dropped and the
    /// token is redacted by value (tenet 9), since the body reaches stderr.
    pub fn refused(&self, why: &str) -> Error {
        let error = match parse_json(&self.body) {
            Some(details) if !details.is_null() => self
                .failed(why)
                .with_details(without_secret_fields(details, &[])),
            _ => self.failed(why),
        };
        self.without_token(error)
    }

    /// A 2xx answer that says the input is not valid, such as a server-side check that
    /// answers `valid: false`: `validation`, as ``cannot <action>: <why>``, with the status,
    /// the request id, and the body as `details`, cleaned as [`Response::refused`] cleans it.
    pub fn invalid(&self, why: &str) -> Error {
        let details =
            parse_json(&self.body).map_or(Value::Null, |body| without_secret_fields(body, &[]));
        let error = Error::new(
            Failure::Validation,
            format!("cannot {}: {why}", self.action),
        )
        .with_http(self.status, self.request_id.clone())
        .with_details(details);
        self.without_token(error)
    }

    /// `error` with each spelling of the token the request carried redacted.
    fn without_token(&self, error: Error) -> Error {
        error.redacted_all(&self.token_spellings)
    }

    fn unusable(&self, what: &str) -> Error {
        // The request id is the server's, and may repeat the token.
        self.without_token(self.failed(&format!("the API answered {} with {what}", self.status)))
    }

    fn failed(&self, why: &str) -> Error {
        let error = Error::internal(format!("cannot {}: {why}", self.action))
            .with_http(self.status, self.request_id.clone());
        // A 2xx to a write means the write was done, whatever the body says.
        if self.write {
            error.with_side_effects(SideEffects::Partial)
        } else {
            error
        }
    }
}

/// The body is left out: it can hold a one-time secret (tenet 9).
impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .field("body_len", &self.body.len())
            .field("write", &self.write)
            .finish_non_exhaustive()
    }
}

/// An empty body is `null`; anything else must parse as JSON.
fn parse_json(body: &[u8]) -> Option<Value> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Some(Value::Null);
    }
    serde_json::from_slice(body).ok()
}

/// Whether a request body is a JSON object whose top-level `key` is exactly `false`, for a
/// route rule that needs its flag otherwise. No body, a body without the key, and a body that
/// is not a JSON object all answer no, so a shape `hipc` does not read fails closed. A key
/// given twice counts as its last copy, the one `serde_json` and Python's `json` both keep.
fn sets_key_to_false(body: Option<&[u8]>, key: &str) -> bool {
    body.is_some_and(|body| match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(fields)) => fields.get(key) == Some(&Value::Bool(false)),
        _ => false,
    })
}

/// Whether a request body sets the top-level `key`, for a route rule that depends on it.
/// No body sets nothing. A body that is not a JSON object counts as setting it, so a shape
/// `hipc` does not read cannot turn the rule off. A key given twice is set whichever copy a
/// parser keeps.
fn sets_key(body: Option<&[u8]>, key: &str) -> bool {
    body.is_some_and(|body| match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(fields)) => fields.contains_key(key),
        _ => true,
    })
}

/// Where a request goes, before the account rules apply: always the configured API origin,
/// since a full URL on any other is refused ([`Client::target`]).
#[derive(Debug)]
struct Target {
    url: String,
    /// The path after the origin, with the target's own query, before `request.query` is
    /// appended to `url`.
    path_and_query: String,
}

impl Target {
    /// The path on the API origin, without the query.
    fn api_path(&self) -> &str {
        crate::url::without_query(&self.path_and_query)
    }
}

/// The warning for `method` to `path`, whose answer carried `deprecation` or `sunset`, or
/// `None` when it carried neither. Only a date the header holds in its standard form is
/// repeated (`@` and Unix seconds for `Deprecation`, an HTTP date for `Sunset`), as a day:
/// any other value is the server's text, which a warning does not quote.
fn deprecation_warning(
    method: Method,
    path: &str,
    deprecation: Option<&str>,
    sunset: Option<&str>,
) -> Option<String> {
    if deprecation.is_none() && sunset.is_none() {
        return None;
    }
    let day = |seconds: u64| i64::try_from(seconds / 86_400).ok().map(date_of_unix_day);
    let since = deprecation
        .and_then(|value| value.trim().strip_prefix('@'))
        .and_then(|seconds| seconds.parse::<u64>().ok())
        .and_then(day);
    let until = sunset
        .and_then(|value| crate::time::http_date(value.trim()))
        .and_then(day);
    let mut warning = format!("the API marks `{method} {path}` as deprecated");
    if let Some(since) = since {
        warning.push_str(&format!(" as of {since}"));
    }
    if let Some(until) = until {
        warning.push_str(&format!(", and it may stop answering on {until}"));
    }
    warning.push_str(": a newer `hipc` may send another route");
    Some(warning)
}

/// Where a request goes, and what it carries besides the body.
#[derive(Debug)]
struct Resolved {
    target: Target,
    account_header: Option<String>,
    project_header: Option<String>,
    billing_header: Option<String>,
    /// [`Client::skipped_billing`].
    skipped_billing: Option<String>,
}

/// The console API client for one invocation.
///
/// One request at a time: [`Client::send`] resets a flag the connections share, so the
/// client is not `Sync`.
pub struct Client {
    agent: ureq::Agent,
    /// Set by [`MarkWrites`] once a request byte is handed to a connection, for the request
    /// running now.
    written: Arc<AtomicBool>,
    /// Two threads sending at once would clear each other's `written`.
    _one_request_at_a_time: std::marker::PhantomData<std::cell::Cell<()>>,
    origin: Origin,
    credential: Credential,
    /// Set by [`Client::requiring_token`]: sending without a token is refused locally.
    needs_token: bool,
    account: Selection,
    /// The selected project, `None` for the Default one.
    project: Option<ProjectId>,
    /// The selected billing account, `None` for the Default one.
    billing: Option<BillingId>,
    /// The warnings of the routes the API marked deprecated in this client's answers, each
    /// once, in the order they came, until [`Client::take_deprecation_warnings`] takes them.
    deprecations: RefCell<Vec<String>>,
}

impl Client {
    /// A client for `settings`, with a timeout for each whole request.
    ///
    /// # Errors
    ///
    /// `usage` when the configured API URL is not a valid origin.
    pub fn new(settings: &Settings, timeout: Duration) -> Result<Self> {
        let origin = Origin::parse_from(&settings.api_url, &settings.api_url_source)?;
        // A proxy from `HTTP_PROXY` or `ALL_PROXY` would carry a plain `http` request, token
        // and all, in the clear to another host; the loopback origin is reached directly.
        // An https origin keeps the environment's proxy: the token stays inside TLS.
        let proxy = if origin.is_plain_http() {
            None
        } else {
            proxy_from_env()?
        };
        let config = ureq::Agent::config_builder()
            .proxy(proxy.clone())
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .max_redirects(0)
            .user_agent(concat!("hipc/", env!("CARGO_PKG_VERSION")))
            .accept("application/json")
            .build();
        let written = Arc::new(AtomicBool::new(false));
        let connector = DefaultConnector::new().chain(MarkWrites {
            written: Arc::clone(&written),
            proxied: proxy.is_some(),
        });
        let agent = ureq::Agent::with_parts(config, connector, DefaultResolver::default());
        Ok(Self {
            agent,
            written,
            _one_request_at_a_time: std::marker::PhantomData,
            origin,
            credential: settings.credential.clone(),
            needs_token: false,
            account: settings.account.clone(),
            project: settings.project.clone(),
            billing: settings.billing.clone(),
            deprecations: RefCell::new(Vec::new()),
        })
    }

    /// This client for routes that need a token: [`Client::send`] refuses without one, so
    /// a command can check its own flags and run `--dry-run` first, then fail on the token
    /// only when it would send (tenet 5).
    #[must_use = "the token requirement is on the returned client"]
    pub fn requiring_token(mut self) -> Self {
        self.needs_token = true;
        self
    }

    /// `Ok` for a client that does not require a token. Otherwise refuses, before anything
    /// is sent, a client from [`Client::requiring_token`] that has no token to send. A
    /// `HIPPIUS_TOKEN` bound to another origin is refused by every client: the caller asked
    /// for a token on this run, and a request sent without it would answer for no one.
    ///
    /// # Errors
    ///
    /// `auth` when no token is configured, or the stored one or `HIPPIUS_TOKEN` is for
    /// another origin.
    pub fn check_token(&self) -> Result<()> {
        if !self.needs_token && !matches!(self.credential, Credential::VariableWithheld { .. }) {
            return Ok(());
        }
        match &self.credential {
            Credential::Variable(_) | Credential::Stored(_) => Ok(()),
            // The message names where the token was looked for, so a profile or a config
            // file other than the one the caller meant shows at once (`no_token_hint` says
            // who can sign in).
            Credential::None { profile, path } => {
                // Only on this refusal, so a run with a token never looks at the path again.
                let found = if path.symlink_metadata().is_ok() {
                    format!(
                        "the profile {} in {} holds no token",
                        quote(profile),
                        crate::phrase::quote_path(path)
                    )
                } else {
                    format!(
                        "the config file {} does not exist",
                        crate::phrase::quote_path(path)
                    )
                };
                Err(Error::new(
                    Failure::Auth,
                    format!(
                        "no API token is configured for `{}`: {found}",
                        self.origin.as_str()
                    ),
                )
                .with_hint(no_token_hint(profile)))
            }
            Credential::UnknownProfile { profile, path } => {
                let profiles = format!("a profile in {}", crate::phrase::quote_path(path));
                let hint = match &profile.source {
                    Source::Variable(variable) => {
                        format!("set `{variable}` to the name of {profiles}")
                    }
                    // `--profile`, the one other source of a profile name.
                    _ => format!("pass `--profile` the name of {profiles}"),
                };
                Err(Error::new(
                    Failure::Auth,
                    format!(
                        "no API token is configured for `{}`: the config file {} has no \
                         profile {}",
                        self.origin.as_str(),
                        crate::phrase::quote_path(path),
                        quote(&profile.value)
                    ),
                )
                .with_hint(hint))
            }
            // The hint names the variable that binds the token, never this origin as a value
            // to copy: sending a CI secret to a new host is a choice the caller makes knowingly.
            Credential::VariableWithheld { bound_to } => Err(Error::new(
                Failure::Auth,
                format!(
                    "`HIPPIUS_TOKEN` is for `{bound_to}`, and this command targets `{}`, so \
                     the token was not sent",
                    self.origin.as_str()
                ),
            )
            .with_hint(
                "set `HIPPIUS_TOKEN_ORIGIN` to the origin `HIPPIUS_TOKEN` is meant for; it is \
                 sent to no other",
            )),
            // The hint points back at the stored origin, never at logging in for the new
            // one: a variable from a repository's `.envrc` must not talk the caller into
            // storing a token for a host they did not choose. It names the flag, which wins
            // over the variable and the profile wherever the target came from; removing the
            // flag or the variable would not help when the target is the profile's `api_url`
            // or the default.
            Credential::Withheld { stored_for } => Err(Error::new(
                Failure::Auth,
                format!(
                    "the stored token is for `{stored_for}`, and this command targets `{}`, so \
                     the token was not sent",
                    self.origin.as_str()
                ),
            )
            .with_hint(format!(
                "pass `--api-url {stored_for}` to send the stored token to the origin it was \
                 stored for"
            ))),
        }
    }

    /// Who requests go out as: the canonical origin and a fingerprint of the token, so
    /// `HTTPS://API.hippius.com:443` and `https://api.hippius.com` are one identity and two
    /// tokens are two.
    ///
    /// The fingerprint is a 64-bit `SipHash` with std's fixed keys. It is not a secret and
    /// not stable across Rust releases, which is enough for a local marker that lasts
    /// minutes: a mismatch after an upgrade only lets one repeat through the marker. A
    /// token is long and random, so the hash does not help anyone guess it.
    pub fn identity(&self) -> String {
        let fingerprint = self.credential.token().map_or(0, |token| {
            let mut hasher = std::hash::DefaultHasher::new();
            std::hash::Hash::hash(token.expose(), &mut hasher);
            std::hash::Hasher::finish(&hasher)
        });
        format!("{} {fingerprint:016x}", self.origin.as_str())
    }

    /// Whether the answer to `request` holds a secret, which hipc prints only with
    /// `--reveal` (tenet 9): its route's row says so ([`crate::routes`]), and for a route with
    /// no row, the secret table in `account`.
    ///
    /// The answer depends on the method and the target alone, not on the selected account,
    /// so a caller can check `--reveal` before the account refusals (tenet 5).
    ///
    /// # Errors
    ///
    /// The local refusals of the target that [`Client::send`] makes: a target that is
    /// neither a path nor an http(s) URL, a full URL off the API origin, and an ambiguous
    /// API path; and the `usage` of a route never sent ([`Client::route_flags`]). Never the
    /// account refusal, `account_scope_unsupported`.
    pub fn returns_secret(&self, request: &Request) -> Result<bool> {
        // A route never sent is refused first, so its refusal is not hidden behind a
        // `--reveal` that would only lead to it.
        self.route_flags(request)?;
        let target = self.target(request)?;
        Ok(crate::account::returns_secret(
            request.method,
            target.api_path(),
        ))
    }

    /// Whether `request` reads by its method yet may create something on the server, such
    /// as the first `GET /api/network/me/` for an account: `hipc api` then marks it with
    /// [`Request::provisions`] and confirms it as the curated command does (tenets 5 and
    /// 7). Its route's row says so ([`crate::routes`]); a route with no row does not.
    ///
    /// Like [`Client::returns_secret`], it depends on the method and the target alone, so
    /// a caller can mark the request before the account refusals read its access.
    ///
    /// # Errors
    ///
    /// The same local refusals of the target as [`Client::returns_secret`].
    pub fn provisions(&self, request: &Request) -> Result<bool> {
        let target = self.target(request)?;
        Ok(account::provisions(request.method, target.api_path()))
    }

    /// Whether `request` cannot be undone, such as a Hub repository delete: `hipc api` then
    /// asks with the warning the curated command gives. The routes are the permanent table
    /// in `account`. Like [`Client::provisions`], it depends on the method and the target
    /// alone.
    ///
    /// # Errors
    ///
    /// The same local refusals of the target as [`Client::returns_secret`].
    pub fn is_permanent(&self, request: &Request) -> Result<bool> {
        let target = self.target(request)?;
        Ok(account::permanent(request.method, target.api_path()))
    }

    /// The value of the first row of `table`, `(method, route pattern, value)`, that names
    /// `request`, matched by the rules of the route tables here: a path without its trailing
    /// `/`, with a format suffix, or in another letter case, matches its route, and the first
    /// row that matches wins.
    /// `hipc api` keeps the consequences its questions give in such a table.
    ///
    /// # Errors
    ///
    /// The same local refusals of the target as [`Client::returns_secret`].
    pub fn route_value<'t, T>(
        &self,
        request: &Request,
        table: &'t [(Method, &str, T)],
    ) -> Result<Option<&'t T>> {
        let target = self.target(request)?;
        Ok(account::first_listed(
            table,
            request.method,
            target.api_path(),
        ))
    }

    /// The flags `hipc api` needs beyond `--yes` to send `request`, in the order it checks
    /// them, each with the template of the route whose guard needs it, and empty when it needs
    /// none (tenet 5). The template is the guard's own route's, also when a write method
    /// borrows it, so a refusal can name what that route changes. Its route's row gives the guard
    /// ([`crate::routes`]), and the route-rule table in `account` the routes with no row; a
    /// bucket ACL set needs two, and a path with a format suffix the flags of both its forms.
    ///
    /// Like [`Client::returns_secret`], it depends on the request alone, not the account
    /// or the token, so a caller can check it before the account refusals and the token.
    /// A rule that depends on the body, such as a zone PATCH that moves the origin, reads
    /// the body the request holds; a request without one sets nothing.
    ///
    /// # Errors
    ///
    /// The same local refusals of the target as [`Client::returns_secret`], and `usage`
    /// for a route never sent whatever the flags, a dry run included: an account deletion
    /// request, any request to the account token route, and the read of a Kubernetes
    /// cluster's sealed kubeconfig.
    pub fn route_flags(&self, request: &Request) -> Result<Vec<(RouteFlag, &'static str)>> {
        let target = self.target(request)?;
        let body = request.body.as_deref().map(Vec::as_slice);
        let mut needed = Vec::new();
        for Rule { guard, route } in account::route_rules(request.method, target.api_path()) {
            let flags = match guard {
                // `route_rules` gives neither.
                Guard::None | Guard::Distinct => &[],
                Guard::Needs(flags) => flags,
                Guard::NeedsWhenSet(flags, key) => {
                    if sets_key(body, key) {
                        flags
                    } else {
                        &[]
                    }
                }
                Guard::NeedsUnlessFalse(flags, key) => {
                    if sets_key_to_false(body, key) {
                        &[]
                    } else {
                        flags
                    }
                }
                Guard::Refused { why, hint } => {
                    return Err(refusal(request.method, route, None, why, hint));
                }
                Guard::RefusedWhenSet { key, why, hint } => {
                    if sets_key(body, key) {
                        return Err(refusal(request.method, route, Some(key), why, hint));
                    }
                    &[]
                }
            };
            for &flag in flags {
                if !needed.iter().any(|&(other, _)| other == flag) {
                    needed.push((flag, route));
                }
            }
        }
        Ok(needed)
    }

    /// The row of `hippius::routes` that `request` reaches, in any of the forms the route
    /// tables match (a slashless path, a format suffix, another letter case), or `None` for
    /// a route `hipc` does not know: no command sends it, and only `hipc api` can. Like
    /// [`Client::returns_secret`], it depends on the method and the target alone.
    ///
    /// # Errors
    ///
    /// The same local refusals of the target as [`Client::returns_secret`].
    pub fn route_of(&self, request: &Request) -> Result<Option<&'static Endpoint>> {
        let target = self.target(request)?;
        Ok(account::row_of(request.method, target.api_path()))
    }

    /// The account this client acts in.
    pub fn account(&self) -> &Selection {
        &self.account
    }

    /// The `X-Hippius-Account` value `request` would carry, or `None` when it carries none.
    ///
    /// # Errors
    ///
    /// The same local refusals [`Client::send`] makes, such as `account_scope_unsupported`.
    pub fn account_header(&self, request: &Request) -> Result<Option<String>> {
        self.resolve(request).map(|r| r.account_header)
    }

    /// The `X-Hippius-Project` value `request` would carry, or `None` when it carries none.
    ///
    /// # Errors
    ///
    /// The same local refusals [`Client::send`] makes, such as `project_scope_unsupported`.
    pub fn project_header(&self, request: &Request) -> Result<Option<String>> {
        self.resolve(request).map(|r| r.project_header)
    }

    /// The `X-Hippius-Billing` value `request` would carry, or `None` when it carries none.
    ///
    /// # Errors
    ///
    /// The same local refusals [`Client::send`] makes, such as `billing_scope_unsupported`.
    pub fn billing_header(&self, request: &Request) -> Result<Option<String>> {
        self.resolve(request).map(|r| r.billing_header)
    }

    /// The selected billing account `request` goes out without: a stored one on a route that
    /// takes none, whose charge then goes to the Default billing account (tenet 12). `None`
    /// when none is selected, when the request carries it, or on a personal route.
    ///
    /// # Errors
    ///
    /// The same local refusals [`Client::send`] makes, such as `billing_scope_unsupported`.
    pub fn skipped_billing(&self, request: &Request) -> Result<Option<String>> {
        self.resolve(request).map(|r| r.skipped_billing)
    }

    /// The URL `request` goes to.
    ///
    /// # Errors
    ///
    /// `usage` for a target that is neither a path nor an http(s) URL, and the same
    /// local refusals [`Client::send`] makes, such as `account_scope_unsupported`.
    pub fn url(&self, request: &Request) -> Result<String> {
        self.resolve(request).map(|r| r.target.url)
    }

    /// Where `request` goes, with the account, project, and billing rules applied: a route
    /// that cannot act in the selected account, or does not take the selected project or
    /// billing account, is refused here. The billing account is decided first, because the
    /// account header of the recurring card routes depends on it.
    fn resolve(&self, request: &Request) -> Result<Resolved> {
        let target = self.target(request)?;
        let (method, path) = (request.method(), target.path_and_query.as_str());
        let billing =
            project::billing_header(self.billing.as_ref(), method, request.access(), path)?;
        let account = account::header(&self.account, method, path, billing)?;
        let project =
            project::project_header(self.project.as_ref(), method, request.access(), path)?;
        Ok(Resolved {
            account_header: account.map(str::to_owned),
            project_header: project.map(str::to_owned),
            billing_header: billing.map(str::to_owned),
            skipped_billing: project::skipped_billing(self.billing.as_ref(), method, path)
                .map(str::to_owned),
            target,
        })
    }

    /// Where `request` goes, before the account rules: the checks that depend on the target
    /// alone.
    fn target(&self, request: &Request) -> Result<Target> {
        // A path is checked first, so a `://` inside its query is not taken for a URL.
        let (base, path_and_query) = if request.target.starts_with('/') {
            let path = if under_api_base(&request.target) {
                request.target.clone()
            } else {
                format!("{API_BASE}{}", request.target)
            };
            (self.origin.as_str().to_owned(), path)
        } else if request.target.contains("://") {
            let (base, rest) = split_url(&request.target)?;
            // Only the configured origin: `hipc` is no general HTTP client, so a full URL
            // cannot carry a body, a query, or a probe of a loopback port anywhere else, and
            // the token and the selector headers need no rule for another origin.
            match Origin::parse(&base) {
                Ok(origin) if origin == self.origin => (origin.into_string(), rest),
                _ => {
                    return Err(Error::usage(
                        format!(
                            "{} is not on the API origin `{}`: `hipc` sends only there",
                            quote(without_query_values(&request.target)),
                            self.origin.as_str()
                        ),
                        "pass a path such as `/compute/vms/`, or use curl for another host",
                    ));
                }
            }
        } else {
            return Err(Error::usage(
                format!(
                    "{} is not an API path: a path starts with `/`, or is a full URL",
                    quote(without_query_values(&request.target))
                ),
                "pass a path such as `/compute/vms/`, or a full URL on the API origin",
            ));
        };
        check_path(&path_and_query, &request.target)?;
        let mut url = format!("{base}{path_and_query}");
        for (i, (name, value)) in request.query.iter().enumerate() {
            let separator = if i == 0 && !path_and_query.contains('?') {
                '?'
            } else {
                '&'
            };
            url.push(separator);
            url.push_str(&percent_encode(name));
            url.push('=');
            url.push_str(&percent_encode(value));
        }
        // Checked here, not left to the HTTP library, so a dry run refuses what the real run
        // would, and the hint names the fix: the library's own refusal reads like a bad
        // character.
        if url.len() > MAX_URL_BYTES {
            return Err(Error::usage(
                format!(
                    "the URL is {} bytes long, and the HTTP library sends at most \
                     {MAX_URL_BYTES}",
                    url.len()
                ),
                "shorten the path or its query",
            ));
        }
        // The same check the HTTP library makes when it builds the request, made here so a
        // dry run refuses a space or another character a URL does not allow, as the real run
        // does.
        if let Err(error) = ureq::http::Uri::try_from(url.as_str()) {
            return Err(unbuildable(error.into()));
        }
        Ok(Target {
            url,
            path_and_query,
        })
    }

    /// The `--dry-run` document for `request`: what would be sent, with no header values
    /// but the account, project, and billing account ids. Sends nothing.
    ///
    /// # Errors
    ///
    /// The same local refusals [`Client::send`] makes, such as `account_scope_unsupported`.
    pub fn describe(&self, request: &Request) -> Result<Value> {
        let resolved = self.resolve(request)?;
        let body = request
            .body
            .as_deref()
            .map_or(Value::Null, |b| parse_json(b).unwrap_or(Value::Null));
        let mut doc = serde_json::json!({
            "dry_run": true,
            "method": request.method.as_str(),
            "url": resolved.target.url,
            // Every scope header, `null` when the request goes out without it, so an agent
            // reads one shape whatever is selected.
            "account_header": resolved.account_header,
            "project_header": resolved.project_header,
            "billing_header": resolved.billing_header,
            "body": body,
        });
        // `if_match` belongs to the few writes that send a precondition, not to a selection
        // every request may carry, so it is present only on those (tenet 5).
        if let Some(version) = &request.if_match {
            doc["if_match"] = version.as_str().into();
        }
        // Likewise present only on the writes that send one.
        if let Some(key) = &request.idempotency_key {
            doc["idempotency_key"] = key.as_str().into();
        }
        Ok(doc)
    }

    /// The request line and the headers of `request`, which goes to the API origin.
    fn builder(&self, request: &Request, resolved: &Resolved) -> ureq::http::request::Builder {
        let mut builder = ureq::http::Request::builder()
            .method(request.method.as_str())
            .uri(&resolved.target.url);
        if let Some(token) = self.credential.token() {
            // Sized up front, so the string never grows and frees a copy of part of the token.
            let mut value =
                zeroize::Zeroizing::new(String::with_capacity(6 + token.expose().len()));
            value.push_str("Token ");
            value.push_str(token.expose());
            builder = builder.header("Authorization", value.as_str());
        }
        if let Some(account) = &resolved.account_header {
            builder = builder.header("X-Hippius-Account", account);
        }
        if let Some(project) = &resolved.project_header {
            builder = builder.header("X-Hippius-Project", project);
        }
        if let Some(billing) = &resolved.billing_header {
            builder = builder.header("X-Hippius-Billing", billing);
        }
        if let Some(version) = &request.if_match {
            builder = builder.header("If-Match", version);
        }
        if let Some(key) = &request.idempotency_key {
            builder = builder.header("Idempotency-Key", key);
        }
        if let Some(media_type) = request.accept {
            builder = builder.header("Accept", media_type);
        }
        builder
    }

    /// Sends `request` and returns a 2xx answer.
    ///
    /// # Errors
    ///
    /// A local refusal before anything is sent (`auth` from [`Client::check_token`] for a
    /// client that requires a token and has none), a transport failure, or the error the
    /// status map gives a non-2xx answer.
    pub fn send(&self, request: &Request) -> Result<Response> {
        // The request's own refusals first, so a command fails the same way with a token
        // or without one, and the same way under `--dry-run`.
        let resolved = self.resolve(request)?;
        self.check_token()?;
        check_if_match(request)?;
        let builder = self.builder(request, &resolved);
        let write = !request.is_read();
        // A POST, PUT, or PATCH without a body still goes out with `Content-Length: 0`, as
        // curl sends it. Without a body ureq frames those methods as chunked with one empty
        // chunk, which servers and proxies handle inconsistently.
        let empty = Vec::new();
        let body = match request.body.as_deref() {
            Some(body) => Some(body),
            None if request.method.always_has_body() => Some(&empty),
            None => None,
        };
        let host = split_url(&resolved.target.url)
            .map(|(origin, _)| origin)
            .expect("a resolved URL is an origin and a path");
        let attempt = Attempt {
            action: request.action(),
            host: host.split_once("://").map_or(host.as_str(), |(_, h)| h),
            member: resolved.account_header.is_some(),
            project: resolved.project_header.is_some(),
            billing: resolved.billing_header.is_some(),
            listed_by: request.listed_by.as_deref(),
            token: self.token_from(),
        };
        self.written.store(false, Ordering::Relaxed);
        let failed = |error: ureq::Error| {
            let progress = if self.written.load(Ordering::Relaxed) {
                Progress::Written
            } else {
                Progress::Unwritten
            };
            transport_error(&error, progress, write, &attempt)
        };
        let sent = match body {
            Some(body) => builder
                .header("Content-Type", "application/json")
                .body(body.as_slice())
                .map_err(unbuildable)
                .and_then(|r| self.agent.run(r).map_err(failed)),
            None => builder
                .body(())
                .map_err(unbuildable)
                .and_then(|r| self.agent.run(r).map_err(failed)),
        };
        let mut response = sent?;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let retry_after = header("retry-after");
        // An id longer than any request id is not one, and would fill the error.
        let request_id = header("x-request-id").filter(|id| id.len() <= MAX_REQUEST_ID_BYTES);
        let location = header("location");
        let date = header("date");
        self.note_deprecation(request.method, resolved.target.api_path(), &header);
        // The status has arrived, so it decides even when the body cannot be read: a 2xx
        // write was done (`partial`, not retryable), and a 4xx is that status's error.
        // The limit is one byte past the cap: ureq 3.5.0 refuses a read once its limit is used
        // up, before it sees the end of the body (`LimitReader::read` in `src/body/limit.rs`),
        // so a limit of the cap itself would refuse a body of exactly the cap.
        let (body, body_unread) = match response
            .body_mut()
            .with_config()
            .limit(MAX_BODY_BYTES + 1)
            .read_to_vec()
        {
            Ok(body) => (body, None),
            // The status arrived, so the request reached the server whatever broke the
            // body: a TLS alert or an unreachable network here must not read as "not sent".
            Err(error) if (200..300).contains(&status) => {
                let error = transport_error_as(&error, Transport::MaybeSent, write, &attempt);
                return Err(self.without_token(error.with_http(status, request_id)));
            }
            Err(error) => (Vec::new(), Some(unread_body(&error))),
        };
        let mut token_spellings = self.sent_token_spellings();
        if (200..300).contains(&status) {
            return Ok(Response {
                status,
                body,
                write,
                action: attempt.action,
                request_id,
                date,
                token_spellings: std::mem::take(&mut *token_spellings),
            });
        }
        let sent_secrets = sent_secret_spellings(request.body.as_deref().map(Vec::as_slice));
        let error = status_error(&StatusAnswer {
            attempt: &attempt,
            write,
            not_done_on_503: account::not_done_on_503(request.method, resolved.target.api_path()),
            status,
            body: &body,
            body_unread,
            retry_after: retry_after.as_deref(),
            request_id,
            location: location.as_deref(),
            token_spellings: &token_spellings,
            sent_secrets: &sent_secrets,
        });
        Err(self.without_token(error))
    }

    /// Takes the warnings of the routes the API marked deprecated in this client's answers
    /// since the last take, with a `Deprecation` (RFC 9745) or a `Sunset` (RFC 8594) header,
    /// each route once, in the order they came, so the caller prints them as `warning:` lines
    /// or a `warnings` array (tenet 15). `hipc` surfaces them and changes nothing about the
    /// request.
    pub fn take_deprecation_warnings(&self) -> Vec<String> {
        self.deprecations.take()
    }

    /// Keeps the warning of an answer to `method` to `path` whose `header`s say the route is
    /// deprecated ([`deprecation_warning`]) for [`Client::take_deprecation_warnings`], once
    /// per route: a path is named by the template of its row, so `/api/compute/vms/a/` and
    /// `/b/` are one warning, and a path with no row by itself.
    fn note_deprecation(
        &self,
        method: Method,
        path: &str,
        header: &dyn Fn(&str) -> Option<String>,
    ) {
        let deprecation = header("deprecation");
        let sunset = header("sunset");
        let route = account::row_of(method, path).map_or(path, |row| row.template());
        let Some(warning) =
            deprecation_warning(method, route, deprecation.as_deref(), sunset.as_deref())
        else {
            return;
        };
        let mut warnings = self.deprecations.borrow_mut();
        if !warnings.contains(&warning) {
            warnings.push(warning);
        }
    }

    /// The spellings of the token a request carries, empty when it carries none, cleared on
    /// drop on every path, as the copy a `Response` keeps is.
    fn sent_token_spellings(&self) -> zeroize::Zeroizing<Vec<String>> {
        zeroize::Zeroizing::new(
            self.credential
                .token()
                .map(|token| token_spellings(token.expose()))
                .unwrap_or_default(),
        )
    }

    /// Where the token a request carries came from, for the hint of a 401.
    fn token_from(&self) -> TokenFrom {
        match self.credential {
            Credential::Variable(_) => TokenFrom::Variable,
            Credential::Stored(_)
            | Credential::VariableWithheld { .. }
            | Credential::None { .. }
            | Credential::UnknownProfile { .. }
            | Credential::Withheld { .. } => TokenFrom::Stored,
        }
    }

    /// `error`, from an answer, with the token this client holds redacted by value. An
    /// answer can repeat the token it was sent (a proxy's "invalid token X", an echo of the
    /// request in `X-Request-Id`): it is the caller's credential, so it is redacted wherever
    /// the error quotes the answer, as the invite token is, in each spelling
    /// [`token_spellings`] lists.
    fn without_token(&self, error: Error) -> Error {
        let Some(token) = self.credential.token() else {
            return error;
        };
        // Cleared on drop, as [`Client::sent_token_spellings`] is.
        let spellings = zeroize::Zeroizing::new(token_spellings(token.expose()));
        error.redacted_all(&spellings)
    }
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("origin", &self.origin)
            .field("has_token", &self.credential.token().is_some())
            .field("account", &self.account)
            .field("project", &self.project)
            .field("billing", &self.billing)
            .finish_non_exhaustive()
    }
}

/// Refuses an `If-Match` value no header can carry. The version comes from an earlier
/// answer, so such a value is the server's, not a path the caller wrote: `internal`, not
/// the `usage` the HTTP library's own refusal would give.
fn check_if_match(request: &Request) -> Result<()> {
    match &request.if_match {
        Some(version) if ureq::http::HeaderValue::from_str(version).is_err() => {
            Err(Error::internal(format!(
                "cannot {}: the version the API gave holds a character an `If-Match` header \
                 cannot carry",
                request.action()
            )))
        }
        _ => Ok(()),
    }
}

/// The proxy variables, in the order the HTTP library reads them (ureq 3.5.0,
/// `Proxy::try_from_env`).
const PROXY_VARIABLES: [&str; 6] = [
    "ALL_PROXY",
    "all_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
];

/// The proxy the environment names, or `None` when no proxy variable is set (an empty one
/// counts as unset, as it does to curl).
///
/// The HTTP library skips a variable it cannot use and goes on to the next, or connects
/// directly: a SOCKS proxy, which this build does not speak, or a URL it cannot parse. Both
/// would send the request around a proxy the caller set to carry all traffic, so the first
/// set variable must be an `http` or `https` proxy, or nothing is sent.
///
/// # Errors
///
/// `usage` when the first set variable is not an `http` or `https` proxy URL.
fn proxy_from_env() -> Result<Option<ureq::Proxy>> {
    let Some((name, value)) = PROXY_VARIABLES.iter().find_map(|name| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(|value| (*name, value))
    }) else {
        return Ok(None);
    };
    let usable = value
        .to_str()
        .and_then(|value| ureq::Proxy::new(value).ok())
        .filter(|proxy| {
            matches!(
                proxy.protocol(),
                ureq::ProxyProtocol::Http | ureq::ProxyProtocol::Https
            )
        });
    if usable.is_none() {
        return Err(Error::usage(
            format!("`{name}` is not an `http` or `https` proxy URL, the only proxies `hipc` uses"),
            format!("unset `{name}`, or set it to an `http://` or `https://` proxy"),
        ));
    }
    // The library's own read, so the proxy carries `NO_PROXY` as it always has.
    Ok(ureq::Proxy::try_from_env())
}

/// The error for a request the HTTP library cannot build, such as a path with a character a
/// URL does not allow.
fn unbuildable(error: ureq::http::Error) -> Error {
    Error::usage(
        format!(
            "the HTTP library refuses to build the request: {}",
            quote(error)
        ),
        "write the path with only the characters a URL allows",
    )
}

/// Whether `path` is already under [`API_BASE`], so the client sends it as it is.
fn under_api_base(path: &str) -> bool {
    path.strip_prefix(API_BASE)
        .is_some_and(|under| under.starts_with('/'))
}

/// Refuses an API path the server could read as a different route than `hipc` classified.
///
/// The account header is decided from the path as written (`crate::account`). A `.` or
/// `..` segment, an encoded `.` or `/`, or a backslash can be normalized by a proxy or the
/// server into another route, for example a personal path that `..` turns into a scoped
/// one, which would then act on the caller's own account without the header.
///
/// The message quotes `written`, the target as the caller gave it, not the path with the
/// `/api` prefix hipc adds.
fn check_path(path_and_query: &str, written: &str) -> Result<()> {
    let written = written
        .split_once(['?', '#'])
        .map_or(written, |(path, _)| path);
    // A fragment is never sent, so the server sees a different path than the one the
    // account-header rules classified: `/accounts/#/../drive/` is sent as `/accounts/`.
    if path_and_query.contains('#') {
        return Err(Error::usage(
            "the API path has a `#` fragment, which is never sent",
            "remove the `#` and what follows it",
        ));
    }
    let path = crate::url::without_query(path_and_query);
    // Any percent-encoding in the path is refused, not only `%2e`, `%2f`, and `%5c`: the
    // server decodes it, so `/registry/robot/rot%61te/` would reach a route the route
    // tables (account header, secret-issuing) did not recognize. No API path needs it.
    // An empty segment is refused for the same reason: a front end that merges slashes
    // delivers `/registry//keys/` as `/registry/keys/`, which the tables would not match. So is
    // a `;`, which a front end that drops path parameters reads as the end of the segment
    // (`/auth/token/;` as `/auth/token/`), and any byte outside ASCII, which a front end may
    // fold into another character. No API path needs either.
    let ambiguous = path
        .split('/')
        .any(|segment| segment == "." || segment == "..")
        || path.contains("//")
        || path.contains(['%', '\\', ';'])
        || !path.is_ascii();
    if ambiguous {
        return Err(Error::usage(
            format!(
                "{} has an empty, `.`, or `..` segment, a percent-encoding, a backslash, a \
                 `;`, or a character outside ASCII",
                quote(written)
            ),
            "write the path as the API names it, such as `/compute/vms/`",
        ));
    }
    Ok(())
}

/// The request for a `next` link found on the list page at `page_url`, which is a URL
/// from [`Client::url`].
///
/// The request always goes to the page's own origin, and a path is resolved against it:
/// a link to another origin is refused, except a plain `http` link to the host of an
/// `https` page, which the API writes. So a list on the API cannot send the next request
/// elsewhere, and a page URL off the API origin is refused by [`Client::url`] before this.
///
/// # Errors
///
/// `internal` when the link is not a URL or a path, or points at another origin.
pub fn follow(page_url: &str, next: &str) -> Result<Request> {
    // A link with whitespace or a control character cannot become a request; it is the
    // server's error, not the caller's, so it is refused here as `internal`.
    if next.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(Error::internal(format!(
            "the API's `next` link {} holds whitespace or a control character",
            quote(without_query_values(next))
        )));
    }
    let (page_origin, _) = split_url(page_url)?;
    // A relative link such as `?page=2` is refused, not resolved against the page, and the
    // message says what it is instead of that it leaves the origin, which it does not.
    let target = if next.starts_with('/') {
        format!("{page_origin}{next}")
    } else if next
        .find("://")
        .is_some_and(|scheme_end| !next[..scheme_end].contains(['/', '?', '#']))
    {
        next.to_owned()
    } else {
        return Err(Error::internal(format!(
            "the API's `next` link {} is neither a URL nor a path",
            quote(without_query_values(next))
        )));
    };
    let away = || {
        Error::internal(format!(
            "the API's `next` link {} leaves `{page_origin}`, the origin of the list",
            quote(without_query_values(next))
        ))
    };
    let (next_origin, rest) = split_url(&target).map_err(|_| away())?;
    // Compared as origins: case and a default port (`:443`, `:80`) do not make another one.
    let next_origin = without_default_port(&next_origin);
    let page_origin = without_default_port(&page_origin);
    // The API behind its TLS proxy can write `next` with an `http` scheme. The same host on plain
    // http is the same list, so the link is followed on the page's own https origin: the path is
    // the server's, the scheme stays the one the caller chose, and the token never goes out in the
    // clear.
    let downgraded = page_origin
        .strip_prefix("https://")
        .is_some_and(|host| next_origin.strip_prefix("http://") == Some(host));
    // Either way the request is built on the page's origin, never the link's authority.
    if next_origin == page_origin || downgraded {
        Ok(Request::get(format!("{page_origin}{rest}")))
    } else {
        Err(away())
    }
}

/// `scheme://authority` in lower case, without the scheme's default port (`:443` for
/// https, `:80` for http): the one rule that makes two spellings one origin.
fn without_default_port(origin: &str) -> String {
    let origin = origin.to_ascii_lowercase();
    let default = if origin.starts_with("https://") {
        ":443"
    } else {
        ":80"
    };
    match origin.strip_suffix(default) {
        Some(stripped) => stripped.to_owned(),
        None => origin,
    }
}

/// Whether `url` names the default API origin, `https://api.hippius.com`, in any spelling.
///
/// # Errors
///
/// `usage` when `url` is not a valid API origin; the message names where it came from.
pub fn is_default_origin(url: &Given) -> Result<bool> {
    Ok(Origin::parse_from(&url.value, &url.source)?
        == Origin::parse(crate::config::DEFAULT_API_URL)?)
}

/// The hint for a target that is neither a path nor a full URL [`split_url`] can read.
const FULL_URL_HINT: &str = "pass a full URL such as `https://api.hippius.com/api/accounts/`";

/// Splits `scheme://authority/path?query` into the origin and the rest (at least `/`).
fn split_url(url: &str) -> Result<(String, String)> {
    // The URL may come from the server (a `next` link), so its query values stay out of the
    // message.
    let quoted = quote(without_query_values(url));
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| Error::usage(format!("{quoted} is not a URL"), FULL_URL_HINT))?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" && scheme != "http" {
        return Err(Error::usage(
            format!("{quoted} is not an `http` or `https` URL"),
            FULL_URL_HINT,
        ));
    }
    let split = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(split);
    if authority.is_empty() {
        return Err(Error::usage(format!("{quoted} has no host"), FULL_URL_HINT));
    }
    let tail = if tail.starts_with('/') {
        tail.to_owned()
    } else {
        format!("/{tail}")
    };
    Ok((format!("{scheme}://{authority}"), tail))
}

/// What the status map needs from a non-2xx answer.
#[derive(Debug)]
struct StatusAnswer<'a> {
    attempt: &'a Attempt<'a>,
    write: bool,
    /// The route's 503 means the write was not done (see `account::not_done_on_503`).
    not_done_on_503: bool,
    status: u16,
    body: &'a [u8],
    /// Why the body could not be read, as the end of a sentence about it; `body` is then
    /// empty, and the status still decides the code.
    body_unread: Option<String>,
    retry_after: Option<&'a str>,
    request_id: Option<String>,
    location: Option<&'a str>,
    /// The spellings of the token the request carried, redacted from the body before any of
    /// it is quoted, so a cut cannot split one and leave half of it in the message.
    token_spellings: &'a [String],
    /// The spellings of each value the request sent in a secret field, redacted from the
    /// field errors of a secret field ([`without_secret_fields`]), which may quote it.
    sent_secrets: &'a [String],
}

/// The error for a non-2xx answer (tenet 4).
fn status_error(answer: &StatusAnswer<'_>) -> Error {
    // An error body has no reason to carry a secret, and it is printed whole as
    // `error.details` in JSON mode, so the secret fields are removed from it.
    let details = parse_json(answer.body)
        .filter(|v| !v.is_null())
        .map(|body| without_secret_fields(body, answer.sent_secrets))
        .map(|body| crate::redact::redact_value_all(body, answer.token_spellings));
    let server_code = details
        .as_ref()
        .and_then(|d| d.get("code"))
        .and_then(Value::as_str);
    let refused = match answer.status {
        401 => Refused::Failed(Failure::Auth),
        402 => Refused::Failed(Failure::PaymentRequired),
        403 => server_code
            .and_then(account_refusal)
            .unwrap_or(Refused::Failed(Failure::Permission)),
        404 => Refused::Failed(Failure::NotFound),
        // A 412 answers an `If-Match` that no longer holds: another client changed the
        // resource first, which is what a 409 says too.
        409 | 412 => Refused::Failed(Failure::Conflict),
        429 => Refused::Failed(Failure::RateLimited),
        // A redirect is not followed, so it is a failure of the request as built.
        // 501 and the 5xx the contract does not name are not a passing outage.
        300..=399 | 500 | 501 | 505..=599 => Refused::Failed(Failure::Internal),
        425 | 502..=504 => Refused::Failed(Failure::Unavailable),
        // 400, 422, and every other status the contract does not name.
        _ => Refused::Failed(Failure::Validation),
    };
    // The message is hipc's own sentence about what it was doing; the server's words follow,
    // quoted, so a person and an agent can tell the two apart (tenet 15). The server's body
    // stays whole in `details`.
    let server_says = details.as_ref().and_then(server_says);
    let status = answer.status;
    let why = match (status, answer.location, &server_says) {
        (300..=399, Some(location), _) => format!(
            "the API answered {status} with a redirect to {}, and a redirect is never \
             followed",
            capped(
                &without_token_in(without_query_values(location), answer.token_spellings),
                ""
            )
        ),
        (300..=399, None, _) => {
            format!("the API answered {status} with a redirect and no `Location`")
        }
        (_, _, Some(says)) => says.clone(),
        (_, _, None) => match &answer.body_unread {
            Some(reason) => format!("the API answered {status}, and its body {reason}"),
            None => format!("the API answered {status}"),
        },
    };
    // An empty object or list explains nothing either, and a hint that points at it would
    // send the caller to nothing.
    let unexplained = server_says.is_none()
        && details.as_ref().is_some_and(|details| match details {
            Value::Object(map) => !map.is_empty(),
            Value::Array(items) => !items.is_empty(),
            _ => true,
        });
    let message = format!("cannot {}: {why}", answer.attempt.action);
    // A scope refusal is the exit-2 code a server answer can carry, which takes its hint at
    // construction. A refusal of a header the request did not carry is about the route
    // itself, and the flag value the hint names would change nothing.
    let attempt = answer.attempt;
    let error = match refused {
        Refused::Failed(failure) => Error::new(failure, message),
        Refused::Scope(selector) => {
            let sent = match selector {
                Selector::Account => attempt.member,
                Selector::Project => attempt.project,
                Selector::Billing => attempt.billing,
            };
            let error = selector.refusal(message);
            if sent {
                error
            } else {
                error.with_hint(unscoped_refusal_hint(selector))
            }
        }
    };
    let code = error.code();
    let mut error = error.with_http(status, answer.request_id.clone());
    if let Some(details) = details {
        error = error.with_details(details);
    }
    // A write that got a 5xx may have been done, so it is `partial` and not retryable: a
    // retry could do it twice (tenet 4). So may a write that got a redirect: the server
    // received it and answered, and `303 See Other` is the usual answer to a POST that was
    // done. A 429 was refused before any work, so it is retryable. The one exception is a
    // 503 with a `Retry-After` delay, of any length, from a route whose 503 means "chain
    // unreachable, nothing done".
    let not_done = answer.not_done_on_503
        && answer.status == 503
        && answer
            .retry_after
            .is_some_and(|value| value.trim().parse::<u64>().is_ok());
    let maybe_done =
        answer.write && (answer.status >= 500 || (300..400).contains(&answer.status)) && !not_done;
    if matches!(code, Code::Unavailable | Code::RateLimited) && !maybe_done {
        error = error.retryable(answer.retry_after.and_then(retry_after_ms));
    }
    if maybe_done {
        error = error.with_side_effects(SideEffects::Partial);
    }
    // The route this `hipc` sends is gone from the API (tenet 15); a newer release sends the
    // one it serves now.
    if status == 410 && !maybe_done {
        return error.with_hint(RETIRED_HINT);
    }
    // An answer whose body gave no text has only its `details` to explain it.
    if code == Code::Validation && unexplained {
        // Right in both modes: JSON mode prints `details` already, and text mode does not.
        return error.with_hint("check the API's answer in `details`, which `--json` prints");
    }
    answer.attempt.with_hint(error)
}

/// The hint of a `410 Gone`: the API retired the route.
const RETIRED_HINT: &str = "the API has retired this route: upgrade `hipc` to its latest release, \
                            which sends the routes the API serves now";

/// Why the body of a non-2xx answer could not be read, as the end of a sentence about it.
fn unread_body(error: &ureq::Error) -> String {
    match error {
        ureq::Error::BodyExceedsLimit(_) => format!(
            "is larger than {} MiB, so it was not read",
            MAX_BODY_BYTES / (1024 * 1024)
        ),
        ureq::Error::Timeout(_) => "did not arrive within the request timeout".to_owned(),
        _ => "could not be read".to_owned(),
    }
}

/// The spellings of each value `body`, a request's JSON body, sends in a secret field
/// ([`sent_secrets`]), for [`without_secret_fields`]; the parsed copy and the values are
/// cleared.
fn sent_secret_spellings(body: Option<&[u8]>) -> zeroize::Zeroizing<Vec<String>> {
    let Some(sent) = body
        .and_then(parse_json)
        .map(crate::secret::SecretValue::new)
    else {
        return zeroize::Zeroizing::new(Vec::new());
    };
    let mut values = zeroize::Zeroizing::new(Vec::new());
    sent_secrets(sent.as_value(), false, &mut values);
    crate::redact::redactions(values.iter().map(String::as_str))
}

/// Pushes onto `values` the text of each value `body`, a request's JSON, sends under a field
/// named as a secret ([`is_secret_field`]), at any depth (`under_secret` says `body` is
/// under one already): a string as it is, a number as its JSON text, and each string and
/// number inside an object or a list there, so a field error that repeats `123456` of
/// `{"password": 123456}` loses it too. A boolean or a null is no secret value, and its text
/// would redact every `true` of the answer.
fn sent_secrets(body: &Value, under_secret: bool, values: &mut Vec<String>) {
    match body {
        Value::String(text) if under_secret => values.push(text.clone()),
        Value::Number(number) if under_secret => values.push(number.to_string()),
        Value::Object(fields) => {
            for (key, value) in fields {
                sent_secrets(value, under_secret || is_secret_field(key), values);
            }
        }
        Value::Array(items) => {
            for item in items {
                sent_secrets(item, under_secret, values);
            }
        }
        _ => {}
    }
}

/// `text` with each of `spellings` redacted, before a cut could split one.
fn without_token_in(text: String, spellings: &[String]) -> String {
    crate::redact::redact_all(text, spellings)
}

/// What a non-2xx answer refuses: a failure, or a selector the route cannot use.
#[derive(Debug, Clone, Copy)]
enum Refused {
    /// A failure that exits 1.
    Failed(Failure),
    /// `account_scope_unsupported`, `project_scope_unsupported`, or
    /// `billing_scope_unsupported`, which exit 2 with the flag value that fixes them.
    Scope(Selector),
}

/// The account and grant codes a 403 body may carry, which pass through unchanged; `None`
/// for any other.
fn account_refusal(code: &str) -> Option<Refused> {
    Some(match code {
        "account_scope_unsupported" => Refused::Scope(Selector::Account),
        "project_scope_unsupported" => Refused::Scope(Selector::Project),
        "billing_scope_unsupported" => Refused::Scope(Selector::Billing),
        "not_a_member" => Refused::Failed(Failure::NotAMember),
        "owner_only" => Refused::Failed(Failure::OwnerOnly),
        "insufficient_grant" => Refused::Failed(Failure::InsufficientGrant),
        "team_management_required" => Refused::Failed(Failure::TeamManagementRequired),
        "billing_required" => Refused::Failed(Failure::BillingRequired),
        _ => return None,
    })
}

/// The hint of a scope refusal the server sent for a request that carried no such header:
/// no flag value helps, so the route itself refuses the selector and the fix is a report.
fn unscoped_refusal_hint(selector: Selector) -> String {
    let header = match selector {
        Selector::Account => "X-Hippius-Account",
        Selector::Project => "X-Hippius-Project",
        Selector::Billing => "X-Hippius-Billing",
    };
    format!(
        "the request carried no `{header}`, so no selector flag changes this answer: report \
         what `--debug` prints at `{}`",
        crate::error::ISSUES_URL
    )
}

/// What a request was doing, and where, for the error it ends in.
#[derive(Debug)]
struct Attempt<'a> {
    /// [`Request::action`].
    action: String,
    /// The host the request went to, such as `api.hippius.com`.
    host: &'a str,
    /// The request carries `X-Hippius-Account`, so a refusal may be about that account's
    /// grants. A personal route acts as the caller even with a membership selected.
    member: bool,
    /// The request carries `X-Hippius-Project`.
    project: bool,
    /// The request carries `X-Hippius-Billing`.
    billing: bool,
    /// [`Request::listed_by`].
    listed_by: Option<&'a str>,
    /// Where the token came from, so a 401 names the fix that applies.
    token: TokenFrom,
}

/// Where the token a request carried came from: `HIPPIUS_TOKEN`, which wins over a stored
/// one, or the profile. A request without a token counts as the profile: storing one is
/// the fix.
#[derive(Debug)]
enum TokenFrom {
    Variable,
    Stored,
}

impl Attempt<'_> {
    /// `error` with the one change likely to fix it, when one is known for its code
    /// (tenet 15): a wrong hint is worse than none.
    fn with_hint(&self, error: Error) -> Error {
        let hint = match error.code() {
            // A write that may have been done must not be repeated blindly.
            _ if matches!(error.side_effects(), SideEffects::Partial) => {
                Some(crate::error::CHECK_FIRST_HINT.to_owned())
            }
            // `HIPPIUS_TOKEN` wins over a stored token, so a login cannot fix its 401.
            Code::Auth => Some(match self.token {
                TokenFrom::Variable => {
                    "unset `HIPPIUS_TOKEN`, or set it to a valid console token".to_owned()
                }
                TokenFrom::Stored => {
                    "run `hipc auth login --token-file <path>` to store a valid token".to_owned()
                }
            }),
            Code::Permission
            | Code::OwnerOnly
            | Code::InsufficientGrant
            | Code::TeamManagementRequired
            | Code::BillingRequired
                if self.member =>
            {
                Some(crate::error::ACCOUNT_SELF_HINT.to_owned())
            }
            Code::NotAMember => {
                Some("run `hipc account list` to see the accounts the token can act in".to_owned())
            }
            Code::NotFound => self
                .listed_by
                .map(|command| format!("run `{command}` to see what exists")),
            Code::PaymentRequired => {
                Some("top up the balance in the console, then run the command again".to_owned())
            }
            Code::RateLimited => Some(
                wait_for(error.retry_after_ms())
                    .unwrap_or_else(|| "wait a minute, then run the command again".to_owned()),
            ),
            Code::Timeout if error.is_retryable() => {
                Some("run the command again with a larger `--request-timeout`".to_owned())
            }
            // No status: the connection itself failed.
            Code::Unavailable if error.is_retryable() && error.http_status().is_none() => {
                Some(format!(
                    "check the network, and that `{}` is the API host to use",
                    self.host
                ))
            }
            // The server's `Retry-After`, when it sent one, says how long.
            Code::Unavailable if error.is_retryable() => Some(
                wait_for(error.retry_after_ms())
                    .unwrap_or_else(|| "wait a few minutes, then run the command again".to_owned()),
            ),
            _ => None,
        };
        match hint {
            Some(hint) => error.with_hint(hint),
            None => error,
        }
    }
}

/// The hint to wait out the server's `Retry-After`, in whole seconds rounded up, or `None`
/// when it sent none.
fn wait_for(retry_after_ms: Option<u64>) -> Option<String> {
    retry_after_ms.map(|ms| {
        format!(
            "wait {}, then run the command again",
            crate::phrase::count(ms.div_ceil(1000), "second")
        )
    })
}

/// The `usage` refusal of `hipc api` sending `method` to `route`, the template or pattern of
/// the row that refuses it, written as a person types it (`<cluster_id>` for
/// `{cluster_id}`), because of `why`; `key` names the body key the refusal depends on. The
/// method is the request's, so a write that borrows the refusal of another write method of
/// its route (`account::route_rules`) names what was asked.
fn refusal(method: Method, route: &str, key: Option<&str>, why: &str, hint: &'static str) -> Error {
    let route = route.replace('{', "<").replace('}', ">");
    let with = key.map(|key| format!(" with `{key}`")).unwrap_or_default();
    Error::usage(
        format!("`hipc api` does not send `{method} {route}`{with}: {why}"),
        hint,
    )
}

/// `body` without any field named as a secret ([`is_secret_field`]), at any depth, but for
/// Django REST framework's field errors: a secret field whose value is a list of strings is a
/// validation error about the field, which says why the request was refused, so it stays:
/// `{"secret_access_key": ["This field may not be blank."]}`. Such a message may quote the value
/// sent in the field (`'hunter2' is too common`), so each of `sent_secrets` is redacted from
/// it; only from it, since a short value would cut every other text of the body.
fn without_secret_fields(body: Value, sent_secrets: &[String]) -> Value {
    match body {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .filter(|(key, value)| !is_secret_field(key) || is_field_error(value))
                .map(|(key, value)| {
                    let value = if is_secret_field(&key) {
                        crate::redact::redact_value_all(value, sent_secrets)
                    } else {
                        without_secret_fields(value, sent_secrets)
                    };
                    (key, value)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| without_secret_fields(item, sent_secrets))
                .collect(),
        ),
        other => other,
    }
}

/// Whether `value` is a field's validation errors: a non-empty list of short strings.
fn is_field_error(value: &Value) -> bool {
    value.as_array().is_some_and(|items| {
        !items.is_empty()
            && items
                .iter()
                .all(|item| item.as_str().is_some_and(|text| text.len() <= 200))
    })
}

/// The most field errors an error message quotes. Arbitrary: enough for the usual one or
/// two missing fields, short enough for one line; `details` holds them all.
const QUOTED_FIELDS: usize = 3;

/// What the server said in an error body, as the end of a message: ``the API says `...` ``,
/// or `None` when the body holds no text.
///
/// The text is `error`, `detail`, or `message`, if it is a string. Otherwise it is Django
/// REST framework's field errors, the commonest 400 shape: `{"name": ["This field is
/// required."]}` reads `name: This field is required.`, a field per `; `, at most
/// [`QUOTED_FIELDS`] of them followed by how many more there are, in the server's order. A
/// field whose value is not a list of strings (a nested object, a list of objects) is left to
/// `details`. A bare list of strings, DRF's non-field form, is the strings.
fn server_says(details: &Value) -> Option<String> {
    let says = |text: &str| {
        format!(
            "the API says {}",
            capped(text, " (all in `details`, which `--json` prints)")
        )
    };
    if let Some(text) = ["error", "detail", "message"]
        .iter()
        .find_map(|key| details.get(*key).and_then(Value::as_str))
    {
        return Some(says(text));
    }
    // A body that is one JSON string, such as `"Rate limit hit"`, is the text itself.
    if let Some(text) = details.as_str().filter(|text| !text.trim().is_empty()) {
        return Some(says(text));
    }
    let fields: Vec<String> = match details {
        Value::Object(map) => map
            .iter()
            .filter_map(|(field, value)| Some(format!("{field}: {}", strings(value)?)))
            .collect(),
        Value::Array(_) => strings(details).into_iter().collect(),
        _ => Vec::new(),
    };
    if fields.is_empty() {
        return None;
    }
    let quoted = says(&fields[..fields.len().min(QUOTED_FIELDS)].join("; "));
    let more = fields.len().saturating_sub(QUOTED_FIELDS) as u64;
    Some(if more == 0 {
        quoted
    } else {
        format!(
            "{quoted} and {more} more {}",
            crate::phrase::plural(more, "field")
        )
    })
}

/// `text` quoted, cut after [`MAX_QUOTED_CHARS`] characters and followed by how many more
/// there are and `rest_in`, where the rest can be read: never cut short without saying so
/// (tenet 3).
fn capped(text: &str, rest_in: &str) -> String {
    match text.char_indices().nth(MAX_QUOTED_CHARS) {
        None => quote(text),
        Some((end, _)) => {
            let more = text[end..].chars().count() as u64;
            format!(
                "{} and {more} more {}{rest_in}",
                quote(&text[..end]),
                crate::phrase::plural(more, "character")
            )
        }
    }
}

/// A non-empty list of strings, joined by spaces, or `None` for anything else.
fn strings(value: &Value) -> Option<String> {
    let items = value.as_array().filter(|items| !items.is_empty())?;
    let texts: Option<Vec<&str>> = items.iter().map(Value::as_str).collect();
    texts.map(|texts| texts.join(" "))
}

/// `Retry-After` in delay-seconds form. The HTTP-date form is ignored: no Hippius route
/// sends it, and its delay would depend on the local clock. A delay over [`MAX_RETRY_AFTER_SECS`]
/// is ignored too: no outage is planned that far ahead, and an agent that waits as told would
/// wait for ever, or read a `retry_after_ms` past what a JSON number holds exactly.
fn retry_after_ms(value: &str) -> Option<u64> {
    value
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|&secs| secs <= MAX_RETRY_AFTER_SECS)
        .map(|secs| secs * 1000)
}

/// The longest `Retry-After` taken, in seconds: a day.
const MAX_RETRY_AFTER_SECS: u64 = 24 * 60 * 60;

/// The error for a request that got no complete answer.
///
/// Three kinds: the request never reached the server and a retry may help (a refused
/// connection, a DNS failure, any failure before a request byte was written); it never
/// reached the server and a retry cannot help (a TLS error such as an untrusted
/// certificate, a proxy, a malformed URL); or it may have reached the server, and a write
/// may have been applied though the answer was lost.
///
/// Whether a request byte was written is known exactly ([`MarkWrites`]), not guessed from
/// the error: ureq reports a stalled DNS lookup, connect, or TLS handshake as the same
/// global timeout as a stalled answer, and a connection closed during the handshake as the
/// same end of file as one closed while the answer is read.
fn transport_error(
    error: &ureq::Error,
    progress: Progress,
    write: bool,
    attempt: &Attempt<'_>,
) -> Error {
    transport_error_as(error, how_far(error, progress), write, attempt)
}

/// How far a request got before `error`.
fn how_far(error: &ureq::Error, progress: Progress) -> Transport {
    // Once a request byte went out, nothing the error says can prove the server did not
    // act on it: Linux can report `NetworkUnreachable` on an established socket whose
    // retransmits failed, and a TLS error can arrive while the answer is read.
    if let Progress::Written = progress {
        return Transport::MaybeSent;
    }
    match error {
        // ureq reports a failed handshake (an untrusted certificate, a server that does not
        // speak TLS) as an io::Error that wraps the rustls error.
        ureq::Error::Io(io)
            if io
                .get_ref()
                .is_some_and(<dyn std::error::Error + Send + Sync>::is::<rustls::Error>) =>
        {
            Transport::Refused
        }
        // An answer that is not valid HTTP comes after the request; before it, the request
        // could not be framed, and a retry would frame it the same way.
        ureq::Error::Rustls(_)
        | ureq::Error::Protocol(_)
        | ureq::Error::LargeResponseHeader(..)
        | ureq::Error::Tls(_)
        | ureq::Error::Pem(_)
        | ureq::Error::TlsRequired
        | ureq::Error::ConnectProxyFailed(_)
        | ureq::Error::InvalidProxyUrl
        | ureq::Error::RequireHttpsOnly(_)
        | ureq::Error::BadUri(_)
        | ureq::Error::Http(_) => Transport::Refused,
        // A DNS failure, a refused or unreachable connect, a stalled handshake: nothing
        // reached the server, and trying again may work.
        _ => Transport::NotSent,
    }
}

/// Whether any byte of the request reached a connection before it failed.
#[derive(Debug, Clone, Copy)]
enum Progress {
    Unwritten,
    Written,
}

/// The last link of the connector chain, after TLS: it wraps each connection in
/// [`MarkedTransport`], which sets the flag the first time request bytes are handed to it.
/// The TLS handshake runs inside the rustls connector, before this wrapper exists, so
/// handshake bytes do not count.
///
/// A proxy `CONNECT` does not count either. ureq opens the proxy leg through this whole
/// chain, this link included, with a copy of the config that has no proxy
/// (`ConnectProxyConnector`, ureq 3.5.0 `src/unversioned/transport/connect.rs`). So on a
/// client that uses a proxy, a connection whose config has none is that leg: it gets a flag
/// of its own that nothing reads. The connection to the API is wrapped again around it and
/// is marked.
///
/// This uses `ureq::unversioned`, which ureq keeps out of its semver promise: a ureq
/// upgrade may need it updated, and the build says so.
#[derive(Debug)]
struct MarkWrites {
    written: Arc<AtomicBool>,
    /// Whether the client goes through a proxy, so its `CONNECT` leg can be told apart.
    proxied: bool,
}

impl Connector<Box<dyn ureq::unversioned::transport::Transport>> for MarkWrites {
    type Out = MarkedTransport;

    fn connect(
        &self,
        details: &ConnectionDetails<'_>,
        chained: Option<Box<dyn ureq::unversioned::transport::Transport>>,
    ) -> std::result::Result<Option<Self::Out>, ureq::Error> {
        let proxy_leg = self.proxied && details.config.proxy().is_none();
        Ok(chained.map(|inner| MarkedTransport {
            inner,
            written: if proxy_leg {
                Arc::new(AtomicBool::new(false))
            } else {
                Arc::clone(&self.written)
            },
        }))
    }
}

/// A connection that records in `written` that request bytes were handed to it.
#[derive(Debug)]
struct MarkedTransport {
    inner: Box<dyn ureq::unversioned::transport::Transport>,
    written: Arc<AtomicBool>,
}

impl ureq::unversioned::transport::Transport for MarkedTransport {
    fn buffers(&mut self) -> &mut dyn Buffers {
        self.inner.buffers()
    }

    fn transmit_output(
        &mut self,
        amount: usize,
        timeout: NextTimeout,
    ) -> std::result::Result<(), ureq::Error> {
        // Set before the bytes go out, so a failure halfway through counts as written.
        // `Relaxed` is enough: ureq runs the request on the calling thread, which stores
        // `false` before the request and loads the flag after it (`Client::send`).
        self.written.store(true, Ordering::Relaxed);
        self.inner.transmit_output(amount, timeout)
    }

    fn await_input(&mut self, timeout: NextTimeout) -> std::result::Result<bool, ureq::Error> {
        self.inner.await_input(timeout)
    }

    fn is_open(&mut self) -> bool {
        self.inner.is_open()
    }

    fn is_tls(&self) -> bool {
        self.inner.is_tls()
    }
}

/// The error for a transport failure, once `kind` says how far the request got.
fn transport_error_as(
    error: &ureq::Error,
    kind: Transport,
    write: bool,
    attempt: &Attempt<'_>,
) -> Error {
    let failure = match error {
        ureq::Error::Timeout(_) => Failure::Timeout,
        // An answer that is not valid HTTP gets the same answer on a retry.
        ureq::Error::BodyExceedsLimit(_)
        | ureq::Error::BadUri(_)
        | ureq::Error::Http(_)
        | ureq::Error::Protocol(_)
        | ureq::Error::LargeResponseHeader(..) => Failure::Internal,
        _ => Failure::Unavailable,
    };
    // hipc's sentence names the host and what went wrong; ureq's own text, which names
    // neither the operation nor the fix, goes to `details`.
    let message = format!(
        "cannot {}: {}",
        attempt.action,
        transport_kind(error, kind, attempt.host)
    );
    let error = Error::new(failure, message)
        .with_details(serde_json::json!({ "transport_error": error.to_string() }));
    let error = match kind {
        Transport::Refused => error,
        // A write that may have been sent is `partial`, and not retryable: a retry could
        // do it twice. A read may be retried.
        Transport::MaybeSent if write => error.with_side_effects(SideEffects::Partial),
        Transport::MaybeSent if failure == Failure::Internal => error,
        Transport::NotSent | Transport::MaybeSent => error.retryable(None),
    };
    attempt.with_hint(error)
}

/// What went wrong on the way to `host`, as the end of a sentence.
fn transport_kind(error: &ureq::Error, kind: Transport, host: &str) -> String {
    let refused =
        matches!(error, ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused);
    match (error, kind) {
        (ureq::Error::Timeout(_), _) => {
            format!("`{host}` did not answer within the request timeout")
        }
        (ureq::Error::BodyExceedsLimit(_), _) => format!(
            "the answer from `{host}` is larger than {} MiB",
            MAX_BODY_BYTES / (1024 * 1024)
        ),
        (ureq::Error::HostNotFound, _) => format!("the host `{host}` was not found"),
        (ureq::Error::Protocol(_) | ureq::Error::LargeResponseHeader(..), Transport::MaybeSent) => {
            format!("the answer from `{host}` is not valid HTTP")
        }
        (_, Transport::MaybeSent) => {
            format!("the connection to `{host}` broke before the answer arrived")
        }
        _ if refused => format!("the connection to `{host}` was refused"),
        (_, Transport::Refused) => format!("the connection to `{host}` could not be set up"),
        (_, Transport::NotSent) => format!("the connection to `{host}` failed"),
    }
}

/// How far a request got before the transport failed.
#[derive(Debug, Clone, Copy)]
enum Transport {
    /// Nothing reached the server, and a retry may succeed.
    NotSent,
    /// Nothing reached the server, and a retry cannot help until something is fixed.
    Refused,
    /// The request may have reached the server.
    MaybeSent,
}

#[cfg(test)]
mod tests;
