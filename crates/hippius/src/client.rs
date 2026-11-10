//! The HTTP client for the console API.
//!
//! Every request carries `User-Agent: hipc/<version>`, because the API answers a default
//! client agent with 403 and error 1010. A token goes out as `Authorization: Token <token>`,
//! and only to the configured API origin. The account header follows [`crate::account`].
//! A non-2xx answer becomes an [`Error`] whose code comes from one status map.
//!
//! The client never retries and never follows a redirect: a retry could charge twice, and a
//! redirect could carry the token to another host.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;

use ureq::unversioned::resolver::DefaultResolver;
use ureq::unversioned::transport::{
    Buffers, ConnectionDetails, Connector, DefaultConnector, NextTimeout,
};

use crate::account::{self, RouteFlag, RouteRule, Selection};
use crate::config::{Credential, Settings, Token};
use crate::error::{Code, Error, Result, SideEffects};
use crate::phrase::quote;
use crate::project::{self, BillingId, ProjectId};
use crate::source::{Given, Source};

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
/// `hipc auth login` asks without echo, for the same profile, with a token for a Google,
/// GitHub, or Apple account, since the console's settings page shows one only after such a
/// sign-in, or with `--access-key` for an access-key account.
fn no_token_hint(profile: &str) -> String {
    let login = if profile == crate::config::DEFAULT_PROFILE {
        "hipc auth login".to_owned()
    } else {
        format!("hipc auth login --profile {profile}")
    };
    format!(
        "ask the user to run {} in their own terminal, with a token from \
         `https://console.hippius.com/dashboard/settings`, or {} for an access-key account",
        quote(&login),
        quote(format!("{login} --access-key"))
    )
}

/// The HTTP methods `hipc` sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// GET.
    Get,
    /// HEAD.
    Head,
    /// POST.
    Post,
    /// PUT.
    Put,
    /// PATCH.
    Patch,
    /// DELETE.
    Delete,
}

impl Method {
    /// The names [`Method::parse`] accepts, in upper case.
    const NAMES: [&'static str; 6] = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"];

    /// Parses a method name, in any case.
    ///
    /// # Errors
    ///
    /// `usage` for any other name.
    pub fn parse(text: &str) -> Result<Self> {
        match text.to_ascii_uppercase().as_str() {
            "GET" => Ok(Self::Get),
            "HEAD" => Ok(Self::Head),
            "POST" => Ok(Self::Post),
            "PUT" => Ok(Self::Put),
            "PATCH" => Ok(Self::Patch),
            "DELETE" => Ok(Self::Delete),
            _ => {
                let names = crate::phrase::join(&Self::NAMES.map(crate::phrase::quote), "or");
                Err(Error::usage(
                    format!("{} is not an HTTP method: a method is {names}", quote(text)),
                    format!("pass the method as {names}"),
                ))
            }
        }
    }

    /// The upper-case name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }

    /// GET and HEAD read. Every other method may change state.
    pub fn is_read(self) -> bool {
        matches!(self, Self::Get | Self::Head)
    }

    /// Whether a request with this method always carries a body, an empty one when none is
    /// given. A DELETE carries one only when it is given.
    pub(crate) fn always_has_body(self) -> bool {
        matches!(self, Self::Post | Self::Put | Self::Patch)
    }
}

/// Whether a request may change state on the server, which decides the stored-selection
/// rule (`crate::project`) and the side effects of a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    /// It changes nothing.
    Read,
    /// It may change state.
    Write,
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An API origin: `https://host[:port]`, or `http://` to a loopback host.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Origin(String);

impl Origin {
    /// Parses an origin such as `https://api.hippius.com`. A trailing `/` is allowed.
    ///
    /// Plain `http` is refused unless the host is `127.0.0.1` or `[::1]`: the
    /// token travels in a header, and `HIPPIUS_API_URL=http://...` must not be a way to
    /// send it in the clear.
    ///
    /// # Errors
    ///
    /// `usage` for anything else: another scheme, a path, a query, credentials, no host.
    pub(crate) fn parse(text: &str) -> Result<Self> {
        Self::parse_from(text, &Source::Argument)
    }

    /// [`Origin::parse`] for a value from `source`, which the error names, with the fix
    /// that belongs to it.
    pub(crate) fn parse_from(text: &str, source: &Source) -> Result<Self> {
        let example = "`https://api.hippius.com`";
        let hint = match source {
            Source::Flag(flag) => format!("pass an `{flag}` such as {example}"),
            Source::Variable(name) => {
                format!("unset `{name}`, or set it to an origin such as {example}")
            }
            Source::Profile {
                field: "token_origin",
                ..
            } => "run `hipc auth login --token-file <path>` to store the token again".to_owned(),
            Source::Profile { .. } => format!("edit {source}"),
            Source::Argument | Source::File { .. } | Source::Default => {
                format!("use an origin such as {example}")
            }
        };
        let invalid = |why: &str| {
            Error::usage(
                format!(
                    "{}{} is not an API origin: {why}",
                    quote(without_query_values(text)),
                    source.suffix()
                ),
                hint.clone(),
            )
        };
        let (scheme, rest) = text
            .split_once("://")
            .ok_or_else(|| invalid("it has no scheme"))?;
        let scheme = scheme.to_ascii_lowercase();
        let authority = rest.strip_suffix('/').unwrap_or(rest);
        if authority.is_empty() {
            return Err(invalid("it has no host"));
        }
        if authority.contains(['/', '?', '#']) {
            return Err(invalid("it has a path, a query, or a fragment"));
        }
        if authority.contains('@') {
            return Err(invalid("it holds a user name or a password"));
        }
        if authority
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(invalid("it holds whitespace"));
        }
        // A host outside ASCII can spell a look-alike of the real one (`аpi` with a
        // Cyrillic `а`) in a confirmation question; IDN hosts are given in punycode.
        if !authority.is_ascii() {
            return Err(invalid(
                "its host is not ASCII, and an IDN host is written in punycode",
            ));
        }
        let host = host_of(authority).to_ascii_lowercase();
        // `https://:443` has a port and no host, and a bracket that does not hold an IPv6
        // address (`[`, `[]`, `[x]`) holds none; either would reach `send` as a URL with no
        // host to connect to.
        let not_ipv6 = host.strip_prefix('[').is_some_and(|inner| {
            inner
                .strip_suffix(']')
                .and_then(|address| address.parse::<std::net::Ipv6Addr>().ok())
                .is_none()
        });
        if host.is_empty() || not_ipv6 {
            return Err(invalid("it has no host"));
        }
        // After the host only `:port` may follow, so `[::1]evil.example` is not taken for
        // the loopback host and `host:abc` is not a port. A colon outside brackets belongs
        // to the port, so `host:abc:443` is refused too.
        if !host.starts_with('[') && host.contains(':') {
            return Err(invalid("it has more than one `:` outside an IPv6 address"));
        }
        // A host name is letters, digits, `-`, and `.` (RFC 1123, section 2.1). Anything else
        // (a backtick, a quote, `<`, `{`) names no host, would reach a confirmation question
        // as a broken literal, and is refused by the HTTP library only when a request is
        // built, after a dry run accepted it.
        let host_name = host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
        if !host.starts_with('[') && !host_name {
            return Err(invalid(
                "its host holds a character other than a letter, a digit, `-`, or `.`",
            ));
        }
        let after_host = &authority[host.len()..];
        let port = match after_host.strip_prefix(':') {
            None if after_host.is_empty() => None,
            Some(port) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => Some(
                port.parse::<u16>()
                    .map_err(|_| invalid("its port is above 65535"))?,
            ),
            _ => {
                return Err(invalid(
                    "it has something after the host that is not a port",
                ));
            }
        };
        match scheme.as_str() {
            "https" => {}
            // Literal loopback addresses only: `localhost` goes through the resolver, and a
            // container without it in its hosts file could resolve it to a remote host.
            "http" if matches!(host.as_str(), "127.0.0.1" | "[::1]") => {}
            "http" => {
                return Err(invalid(
                    "plain `http` is allowed only to `127.0.0.1` and `[::1]`",
                ));
            }
            _ => return Err(invalid("its scheme is not `https`")),
        }
        // The port is written as the number it is, and the default one is dropped, so
        // `https://api.hippius.com:0443` and `:443` are the same origin as
        // `https://api.hippius.com`, which ureq connects to, and a full URL to it still gets
        // the token.
        let default = if scheme == "https" { 443 } else { 80 };
        Ok(match port {
            Some(port) if port != default => Self(format!("{scheme}://{host}:{port}")),
            _ => Self(format!("{scheme}://{host}")),
        })
    }

    /// The origin as text, without a trailing slash.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// Plain `http`, which `parse` allows only to a loopback host.
    fn is_plain_http(&self) -> bool {
        self.0.starts_with("http://")
    }
}

/// The host of `host[:port]` or `[v6]:port`.
fn host_of(authority: &str) -> &str {
    if authority.starts_with('[') {
        authority
            .find(']')
            .map_or(authority, |end| &authority[..=end])
    } else {
        authority
            .rsplit_once(':')
            .map_or(authority, |(host, _)| host)
    }
}

/// One request: a method, a path under `/api` or a full URL, a query, and a body. `Debug`
/// shows the path and counts the rest: a query value or a body can be a secret.
#[derive(Clone)]
pub struct Request {
    method: Method,
    target: String,
    query: Vec<(String, String)>,
    body: Option<Vec<u8>>,
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
}

impl Request {
    /// A request without a body. `target` is a swagger path such as `/accounts/` (sent to
    /// `/api/accounts/`), a path that already starts with `/api/`, or a full URL.
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
        }
    }

    /// A GET.
    pub fn get(target: impl Into<String>) -> Self {
        Self::new(Method::Get, target)
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

    /// Sets a JSON body, sent with `Content-Type: application/json`.
    #[must_use]
    pub fn json(mut self, body: &Value) -> Self {
        self.body = Some(body.to_string().into_bytes());
        self
    }

    /// Sets a body the caller already holds as JSON bytes.
    #[must_use]
    pub fn raw_json(mut self, body: Vec<u8>) -> Self {
        self.body = Some(body);
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
    /// first `GET /api/network/me/` provisions: it is then a write for the selector rules,
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

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("target", &without_query_values(&self.target))
            .field("query_params", &self.query.len())
            .field("body_len", &self.body.as_ref().map(Vec::len))
            .field("about", &self.about)
            .field("listed_by", &self.listed_by)
            .field("accept", &self.accept)
            .field("if_match", &self.if_match)
            .field("idempotency_key", &self.idempotency_key)
            .field("access", &self.access)
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
    /// (`TENETS.md` tenet 3). A command whose route may answer 204 prints its own document
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
                .with_details(without_secret_fields(details)),
            _ => self.failed(why),
        };
        self.without_token(error)
    }

    /// A 2xx answer that says the input is not valid, such as a server-side check that
    /// answers `valid: false`: `validation`, as ``cannot <action>: <why>``, with the status,
    /// the request id, and the body as `details`, cleaned as [`Response::refused`] cleans it.
    pub fn invalid(&self, why: &str) -> Error {
        let details = parse_json(&self.body).map_or(Value::Null, without_secret_fields);
        let error = Error::new(Code::Validation, format!("cannot {}: {why}", self.action))
            .with_http(self.status, self.request_id.clone())
            .with_details(details);
        self.without_token(error)
    }

    /// `error` with each spelling of the token the request carried redacted.
    fn without_token(&self, error: Error) -> Error {
        self.token_spellings
            .iter()
            .fold(error, |error, spelling| error.redacted(spelling))
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
        account::without_query(&self.path_and_query)
    }
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
        })
    }

    /// A client for `target` that sends no token and no account, project, or billing header:
    /// for the login routes, which start a session rather than act in one. A stored token or
    /// selection is not read, so a broken one cannot stop the login that replaces it.
    ///
    /// # Errors
    ///
    /// `usage` when `target` is not a valid origin.
    pub fn signed_out(target: &Given, timeout: Duration) -> Result<Self> {
        let settings = Settings {
            profile: String::new(),
            api_url: target.value.clone(),
            api_url_source: target.source.clone(),
            credential: Credential::None {
                profile: String::new(),
                path: std::path::PathBuf::new(),
            },
            account: Selection::Own,
            project: None,
            billing: None,
        };
        Self::new(&settings, timeout)
    }

    /// [`Client::signed_out`] with `token`, for the read that checks a token a login was
    /// answered with before it is stored. It sends no selection header either.
    ///
    /// # Errors
    ///
    /// `usage` when `target` is not a valid origin.
    pub fn signed_in(target: &Given, token: &Token, timeout: Duration) -> Result<Self> {
        let mut client = Self::signed_out(target, timeout)?;
        client.credential = Credential::Stored(token.clone());
        Ok(client.requiring_token())
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
            // file other than the one the caller meant shows at once. The hint is for an
            // agent too: it cannot sign in, but the person it works for can, at a prompt
            // that does not echo, with a token from either kind of console sign-in.
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
                    Code::Auth,
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
                    Code::Auth,
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
            // The hint points back at the stored origin, never at logging in for the new
            // one: a variable from a repository's `.envrc` must not talk the caller into
            // storing a token for a host they did not choose. It names the flag, which wins
            // over the variable and the profile wherever the target came from; removing the
            // flag or the variable would not help when the target is the profile's `api_url`
            // or the default.
            // The hint names the variable that binds the token, never this origin as a value
            // to copy: sending a CI secret to a new host is a choice the caller makes knowingly.
            Credential::VariableWithheld { bound_to } => Err(Error::new(
                Code::Auth,
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
            Credential::Withheld { stored_for } => Err(Error::new(
                Code::Auth,
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
    /// `--reveal` (TENETS 9). The routes are the secret table in `account`.
    ///
    /// The answer depends on the method and the target alone, not on the selected account,
    /// so a caller can check `--reveal` before the account refusals (TENETS 5).
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
    /// [`Request::provisions`] and confirms it as the curated command does (TENETS 5 and
    /// 7). The routes are the provisioning table in `account`.
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
    /// them, and empty when it needs none (TENETS 5). The routes are the route-rule table in
    /// `account`; a bucket ACL set needs two, and a path with a format suffix the flags of
    /// both its forms.
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
    pub fn route_flags(&self, request: &Request) -> Result<Vec<RouteFlag>> {
        let target = self.target(request)?;
        let rules = account::route_rules(request.method, target.api_path());
        let mut needed = Vec::new();
        for rule in rules {
            let flags = match rule {
                RouteRule::Needs(flags) => flags,
                RouteRule::NeedsWhenSet(flags, key) => {
                    if sets_key(request.body.as_deref(), key) {
                        flags
                    } else {
                        &[]
                    }
                }
                RouteRule::NeedsUnlessFalse(flags, key) => {
                    if sets_key_to_false(request.body.as_deref(), key) {
                        &[]
                    } else {
                        flags
                    }
                }
                RouteRule::Refused { message, hint } => {
                    return Err(Error::usage(for_method(message, request.method), hint));
                }
                RouteRule::RefusedWhenSet { key, message, hint } => {
                    if sets_key(request.body.as_deref(), key) {
                        return Err(Error::usage(for_method(message, request.method), hint));
                    }
                    &[]
                }
            };
            for &flag in flags {
                if !needed.contains(&flag) {
                    needed.push(flag);
                }
            }
        }
        Ok(needed)
    }

    /// Whether `request` is the Hub namespace publicity `PATCH`, whose `--public` refusal
    /// names the namespace rather than a bucket. It is the one route with a
    /// `RouteRule::NeedsUnlessFalse` rule.
    ///
    /// # Errors
    ///
    /// The same local refusals of the target as [`Client::route_flags`].
    pub fn opens_a_namespace(&self, request: &Request) -> Result<bool> {
        let target = self.target(request)?;
        Ok(account::route_rules(request.method, target.api_path())
            .iter()
            .any(|rule| matches!(rule, RouteRule::NeedsUnlessFalse(..))))
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
            let path = if request.target.starts_with("/api/") {
                request.target.clone()
            } else {
                format!("/api{}", request.target)
            };
            (self.origin.as_str().to_owned(), path)
        } else if request.target.contains("://") {
            let (base, rest) = split_url(&request.target)?;
            // Only the configured origin: `hipc` is no general HTTP client, so a full URL
            // cannot carry a body, a query, or a probe of a loopback port anywhere else, and
            // the token and the selector headers need no rule for another origin.
            match Origin::parse(&base) {
                Ok(origin) if origin == self.origin => (origin.0, rest),
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
        let body = match &request.body {
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
        // The status has arrived, so it decides even when the body cannot be read: a 2xx
        // write was done (`partial`, not retryable), and a 4xx is that status's error.
        // The limit is one byte past the cap: ureq 3.5.0 refuses a read once its limit is used
        // up, before it sees the end of the body (`LimitReader::read` in `src/body/limit.rs`),
        // so a limit of the cap itself would refuse a body of exactly the cap.
        let body = match response
            .body_mut()
            .with_config()
            .limit(MAX_BODY_BYTES + 1)
            .read_to_vec()
        {
            Ok(body) => body,
            // The status arrived, so the request reached the server whatever broke the
            // body: a TLS alert or an unreachable network here must not read as "not sent".
            Err(error) if (200..300).contains(&status) => {
                let error = transport_error_as(&error, Transport::MaybeSent, write, &attempt);
                return Err(self.without_token(error.with_http(status, request_id)));
            }
            Err(_) => Vec::new(),
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
        let error = status_error(&StatusAnswer {
            attempt: &attempt,
            write,
            not_done_on_503: request.method == Method::Post
                && crate::account::not_done_on_503(resolved.target.api_path()),
            status,
            body: &body,
            retry_after: retry_after.as_deref(),
            request_id,
            location: location.as_deref(),
            token_spellings: &token_spellings,
        });
        Err(self.without_token(error))
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
        zeroize::Zeroizing::new(token_spellings(token.expose()))
            .iter()
            .fold(error, |error, spelling| error.redacted(spelling))
    }
}

/// The ways an error can spell `token`: as it is, as [`quote`] writes
/// it in a message (a backtick as `\u{60}`), and percent-encoded with uppercase or
/// lowercase hex, as a URL in a redirect or a body may carry it. The longest come first, so
/// no shorter spelling is replaced inside a longer one before the longer one is.
pub(crate) fn token_spellings(token: &str) -> Vec<String> {
    let encoded = percent_encode(token);
    let mut spellings = vec![
        token.to_owned(),
        token.replace('`', r"\u{60}"),
        lowercase_escapes(&encoded),
        encoded,
    ];
    spellings.sort_by_key(|spelling| std::cmp::Reverse(spelling.len()));
    // A copy dropped as a duplicate is cleared first.
    spellings.dedup_by(|later, earlier| {
        let same = later == earlier;
        if same {
            zeroize::Zeroize::zeroize(later);
        }
        same
    });
    spellings
}

/// `encoded`, a [`percent_encode`] result, with the two hex digits after each `%` in
/// lowercase. The unreserved characters between them keep their case.
pub(crate) fn lowercase_escapes(encoded: &str) -> String {
    let mut out = String::with_capacity(encoded.len());
    let mut hex_left = 0;
    for c in encoded.chars() {
        if hex_left > 0 {
            out.push(c.to_ascii_lowercase());
            hex_left -= 1;
        } else {
            out.push(c);
            if c == '%' {
                hex_left = 2;
            }
        }
    }
    out
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
    let path = account::without_query(path_and_query);
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

/// `url` with every query value replaced by `<redacted>`, for the `--debug` line
/// and for any message that quotes a URL. hipc
/// cannot know which parameter of a `hipc api` path is a secret, so it shows none of them
/// (tenet 9: `--debug` redacts tokens and secrets). The names stay, which is what a person
/// debugging a route needs.
pub fn without_query_values(url: &str) -> String {
    // A fragment is never sent and can carry anything; userinfo is a password in a URL.
    let url = url.split_once('#').map_or(url, |(before, _)| before);
    let url = match url.split_once("://") {
        Some((scheme, rest)) => {
            let end = rest.find(['/', '?']).unwrap_or(rest.len());
            match rest[..end].rsplit_once('@') {
                Some((_, host)) => format!("{scheme}://<redacted>@{host}{}", &rest[end..]),
                None => url.to_owned(),
            }
        }
        None => url.to_owned(),
    };
    let Some((base, query)) = url.split_once('?') else {
        return url;
    };
    // A query part without `=` can be a bare secret (`?s3cr3t`), so it is redacted too.
    let names: Vec<String> = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((name, _)) => format!("{name}=<redacted>"),
            None if pair.is_empty() => String::new(),
            None => "<redacted>".to_owned(),
        })
        .collect();
    format!("{base}?{}", names.join("&"))
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

/// Percent-encodes everything outside the unreserved set of RFC 3986, section 2.3.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
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
    retry_after: Option<&'a str>,
    request_id: Option<String>,
    location: Option<&'a str>,
    /// The spellings of the token the request carried, redacted from the body before any of
    /// it is quoted, so a cut cannot split one and leave half of it in the message.
    token_spellings: &'a [String],
}

/// The error for a non-2xx answer (`TENETS.md` tenet 4).
fn status_error(answer: &StatusAnswer<'_>) -> Error {
    // An error body has no reason to carry a secret, and it is printed whole as
    // `error.details` in JSON mode, so the secret fields are removed from it.
    let details = parse_json(answer.body)
        .filter(|v| !v.is_null())
        .map(without_secret_fields)
        .map(|body| {
            answer.token_spellings.iter().fold(body, |body, spelling| {
                crate::json::redact_value(body, spelling)
            })
        });
    let server_code = details
        .as_ref()
        .and_then(|d| d.get("code"))
        .and_then(Value::as_str);
    let code = match answer.status {
        401 => Code::Auth,
        402 => Code::PaymentRequired,
        403 => server_code
            .and_then(Code::from_account_refusal)
            .unwrap_or(Code::Permission),
        404 => Code::NotFound,
        // A 412 answers an `If-Match` that no longer holds: another client changed the
        // resource first, which is what a 409 says too.
        409 | 412 => Code::Conflict,
        429 => Code::RateLimited,
        // A redirect is not followed, so it is a failure of the request as built.
        // 501 and the 5xx the contract does not name are not a passing outage.
        300..=399 | 500 | 501 | 505..=599 => Code::Internal,
        425 | 502..=504 => Code::Unavailable,
        // 400, 422, and every other status the contract does not name.
        _ => Code::Validation,
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
        (_, _, None) => format!("the API answered {status}"),
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
    // The exit-2 codes a server answer can carry, which take their hints at construction.
    // A refusal of a header the request did not carry is about the route itself, and the
    // flag value the hint names would change nothing.
    let attempt = answer.attempt;
    let error = match code {
        Code::AccountScopeUnsupported => Error::account_scope_unsupported(message),
        Code::ProjectScopeUnsupported => Error::project_scope_unsupported(message),
        Code::BillingScopeUnsupported => Error::billing_scope_unsupported(message),
        _ => Error::new(code, message),
    };
    let unsent = match code {
        Code::AccountScopeUnsupported => !attempt.member,
        Code::ProjectScopeUnsupported => !attempt.project,
        Code::BillingScopeUnsupported => !attempt.billing,
        _ => false,
    };
    let error = if unsent {
        error.with_hint(unscoped_refusal_hint(code))
    } else {
        error
    };
    let mut error = error.with_http(status, answer.request_id.clone());
    if let Some(details) = details {
        error = error.with_details(details);
    }
    // A write that got a 5xx may have been done, so it is `partial` and not retryable: a
    // retry could do it twice (tenet 4). So may a write that got a redirect: the server
    // received it and answered, and `303 See Other` is the usual answer to a POST that was
    // done. A 429 was refused before any work, so it is retryable. The
    // one exception is a 503 with `Retry-After` from a route whose 503 means "chain
    // unreachable, nothing done" (TENETS 4).
    // Any delay counts here, a day or more included: the route says nothing was done.
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
    // An answer whose body gave no text has only its `details` to explain it.
    if code == Code::Validation && unexplained {
        // Right in both modes: JSON mode prints `details` already, and text mode does not.
        return error.with_hint("check the API's answer in `details`, which `--json` prints");
    }
    answer.attempt.with_hint(error)
}

/// `text` with each of `spellings` redacted, before a cut could split one.
fn without_token_in(text: String, spellings: &[String]) -> String {
    spellings
        .iter()
        .fold(text, |text, spelling| crate::json::redact(text, spelling))
}

/// The hint of a scope refusal the server sent for a request that carried no such header:
/// no flag value helps, so the route itself refuses the selector and the fix is a report.
fn unscoped_refusal_hint(code: Code) -> String {
    let header = match code {
        Code::ProjectScopeUnsupported => "X-Hippius-Project",
        Code::BillingScopeUnsupported => "X-Hippius-Billing",
        _ => "X-Hippius-Account",
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

/// `message`, a refusal that names the method of its row, such as
/// ``... does not send `POST /api/...` ``, with that method replaced by `method`: a write
/// borrows the refusal of another write method on its route (`account::route_rules`), and
/// the message names what was asked.
fn for_method(message: &str, method: Method) -> String {
    ["POST", "PUT", "PATCH", "DELETE"]
        .iter()
        .find_map(|listed| {
            let named = format!("`{listed} /");
            message
                .contains(&named)
                .then(|| message.replacen(&named, &format!("`{method} /"), 1))
        })
        .unwrap_or_else(|| message.to_owned())
}

/// The fields that hold a secret in a Hippius answer, dropped from an error body (tenet 9).
/// A bare `token` is deliberately kept: the API keys its validation errors by field name
/// (`{"token": ["Invalid invite."]}`), and dropping it would hide why an invite was refused.
/// A route whose answer holds a real `token` is in the secret table, so its error body is
/// printed only to a caller who passed `--reveal`. The API token itself is redacted by value
/// from every answer the client quotes ([`Client::send`], [`Response::refused`]).
///
/// A field matches in any letter case and with or without `_` or `-` between its words
/// ([`is_error_body_secret`]), so `secretKey`, `Secret-Key`, and `secret_key` are one name.
const ERROR_BODY_SECRET_FIELDS: [&str; 21] = [
    "secret",
    "robot_secret",
    "credentials",
    "access_token",
    "refresh_token",
    "ticket",
    "setup_key",
    "join_command",
    "netbird_setup_key",
    "client_secret",
    "encrypted_secret",
    "password",
    "secret_key",
    "secret_access_key",
    "private_key",
    "access_key",
    "api_key",
    "api_token",
    "session_token",
    "passphrase",
    "aws_secret_access_key",
];

/// Whether `key` is one of [`ERROR_BODY_SECRET_FIELDS`], compared in lowercase without `_`
/// and `-`.
fn is_error_body_secret(key: &str) -> bool {
    let folded = |name: &str| -> String {
        name.chars()
            .filter(|c| !matches!(c, '_' | '-'))
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };
    let key = folded(key);
    ERROR_BODY_SECRET_FIELDS
        .iter()
        .any(|name| folded(name) == key)
}

/// `body` without any of [`ERROR_BODY_SECRET_FIELDS`], at any depth, but for Django REST
/// framework's field errors: a secret field whose value is a list of strings is a validation
/// error about the field, such as `{"secret_access_key": ["This field may not be blank."]}`,
/// which says why the request was refused and holds no secret, so it stays.
fn without_secret_fields(body: Value) -> Value {
    match body {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .filter(|(key, value)| !is_error_body_secret(key) || is_field_error(value))
                .map(|(key, value)| {
                    let value = if is_error_body_secret(&key) {
                        value
                    } else {
                        without_secret_fields(value)
                    };
                    (key, value)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(without_secret_fields).collect()),
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
/// sends it, and parsing dates would need a date crate. A delay over [`MAX_RETRY_AFTER_SECS`]
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
/// (`ConnectProxyConnector`, ureq 3.5.0 `src/unversioned/transport/connect.rs`). So on a client that uses
/// a proxy, a connection whose config has none is that leg: it gets a flag of its own that
/// nothing reads. The connection to the API is wrapped again around it and is marked.
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
    let code = match error {
        ureq::Error::Timeout(_) => Code::Timeout,
        // An answer that is not valid HTTP gets the same answer on a retry.
        ureq::Error::BodyExceedsLimit(_)
        | ureq::Error::BadUri(_)
        | ureq::Error::Http(_)
        | ureq::Error::Protocol(_)
        | ureq::Error::LargeResponseHeader(..) => Code::Internal,
        _ => Code::Unavailable,
    };
    // hipc's sentence names the host and what went wrong; ureq's own text, which names
    // neither the operation nor the fix, goes to `details`.
    let message = format!(
        "cannot {}: {}",
        attempt.action,
        transport_kind(error, kind, attempt.host)
    );
    let error = Error::new(code, message)
        .with_details(serde_json::json!({ "transport_error": error.to_string() }));
    let error = match kind {
        Transport::Refused => error,
        // A write that may have been sent is `partial`, and not retryable: a retry could
        // do it twice. A read may be retried.
        Transport::MaybeSent if write => error.with_side_effects(SideEffects::Partial),
        Transport::MaybeSent if code == Code::Internal => error,
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
mod tests {
    use serde_json::json;

    use std::sync::atomic::AtomicUsize;

    use super::*;

    fn settings(api_url: &str, account: Selection) -> Settings {
        Settings {
            profile: "default".into(),
            api_url: api_url.into(),
            api_url_source: Source::Flag("--api-url"),
            credential: Credential::Stored(
                crate::config::Token::new("t0k3n", &Source::Argument).unwrap(),
            ),
            account,
            project: None,
            billing: None,
        }
    }

    fn client(account: Selection) -> Client {
        Client::new(
            &settings("https://api.hippius.com", account),
            Duration::from_secs(5),
        )
        .unwrap()
    }

    fn no_token() -> Credential {
        Credential::None {
            profile: "default".into(),
            path: "/home/u/.hippius/config.toml".into(),
        }
    }

    fn member() -> Selection {
        Selection::parse("acct_0123456789abcdef0123", &Source::Argument).unwrap()
    }

    #[test]
    fn a_client_requiring_a_token_refuses_to_send_without_one() {
        let without = |credential: Credential| {
            let mut settings = settings("http://127.0.0.1:9", Selection::Own);
            settings.credential = credential;
            Client::new(&settings, Duration::from_secs(5)).unwrap()
        };
        // Not required: the check passes, and the request would go out without a token.
        assert!(without(no_token()).check_token().is_ok());
        for credential in [
            no_token(),
            Credential::Withheld {
                stored_for: "https://api.hippius.com".into(),
            },
        ] {
            let client = without(credential).requiring_token();
            let error = client.send(&Request::get("/accounts/")).unwrap_err();
            assert_eq!(error.code(), Code::Auth);
            assert!(error.http_status().is_none(), "nothing was sent");
        }
    }

    #[test]
    fn no_token_names_the_profile_and_file_and_who_signs_in() {
        let mut settings = settings("https://api.hippius.com", Selection::Own);
        settings.credential = no_token();
        let error = Client::new(&settings, Duration::from_secs(5))
            .unwrap()
            .requiring_token()
            .check_token()
            .unwrap_err();
        assert_eq!(error.code(), Code::Auth);
        // `no_token` names a file that does not exist.
        assert_eq!(
            error.message(),
            "no API token is configured for `https://api.hippius.com`: the config file \
             `/home/u/.hippius/config.toml` does not exist"
        );
        let hint = error.hint().unwrap();
        assert_eq!(hint, no_token_hint("default"));
        assert!(hint.starts_with("ask the user to run `hipc auth login` in their own terminal"));
        assert!(hint.ends_with("or `hipc auth login --access-key` for an access-key account"));
        // Another profile is named, so the login stores the token where the run looks.
        assert!(
            no_token_hint("work")
                .starts_with("ask the user to run `hipc auth login --profile work` in their own")
        );
    }

    #[test]
    fn identity_is_the_canonical_origin_and_a_token_fingerprint() {
        let identity = |url: &str, token: Option<&str>| {
            let mut settings = settings(url, Selection::Own);
            settings.credential = token.map_or_else(no_token, |t| {
                Credential::Stored(crate::config::Token::new(t, &Source::Argument).unwrap())
            });
            Client::new(&settings, Duration::from_secs(5))
                .unwrap()
                .identity()
        };
        let canonical = identity("https://api.hippius.com", Some("t0k3n"));
        assert_eq!(
            identity("HTTPS://API.hippius.com:443", Some("t0k3n")),
            canonical
        );
        assert!(
            canonical.starts_with("https://api.hippius.com "),
            "{canonical}"
        );
        assert!(!canonical.contains("t0k3n"), "{canonical}");
        assert_ne!(
            identity("https://api.hippius.com", Some("t0k3m")),
            canonical
        );
        assert_ne!(identity("https://api.hippius.com", None), canonical);
        assert_ne!(identity("https://other.example", Some("t0k3n")), canonical);
        assert_eq!(
            identity("https://api.hippius.com:0443", Some("t0k3n")),
            canonical
        );
        // The whole token, not its last four characters.
        assert_ne!(
            identity("https://api.hippius.com", Some("aaaaaaaaaaaa0123")),
            identity("https://api.hippius.com", Some("bbbbbbbbbbbb0123"))
        );
    }

    /// The longest URL the HTTP library builds goes through; one byte more is refused. The
    /// query a request adds counts, percent-encoded.
    #[test]
    fn a_url_is_refused_one_byte_past_what_the_http_library_builds() {
        let client = client(Selection::Own);
        let prefix = "https://api.hippius.com/api/x/?q=";
        let at_limit = Request::get(format!(
            "/x/?q={}",
            "a".repeat(MAX_URL_BYTES - prefix.len())
        ));
        assert_eq!(client.url(&at_limit).unwrap().len(), MAX_URL_BYTES);
        ureq::http::Uri::try_from(client.url(&at_limit).unwrap()).expect("the library builds it");
        let over = Request::get(format!(
            "/x/?q={}",
            "a".repeat(MAX_URL_BYTES - prefix.len() + 1)
        ));
        assert_eq!(client.url(&over).unwrap_err().code(), Code::Usage);
        let encoded = Request::get("/x/").query("q", &" ".repeat(MAX_URL_BYTES / 3));
        assert_eq!(client.url(&encoded).unwrap_err().code(), Code::Usage);
    }

    #[test]
    fn debug_of_a_request_hides_query_values_and_the_body() {
        let request = Request::new(Method::Post, "/x/?key=s3cr3t")
            .query("token", "s3cr3t")
            .json(&serde_json::json!({"secret": "s3cr3t"}));
        let debug = format!("{request:?}");
        assert!(!debug.contains("s3cr3t"), "{debug}");
        assert!(debug.contains("query_params: 1"), "{debug}");
    }

    #[test]
    fn urls_in_messages_show_query_names_but_no_values() {
        assert_eq!(without_query_values("https://h/api/x/"), "https://h/api/x/");
        assert_eq!(
            without_query_values("https://h/api/x/?token=s3cr3t&page=2&flag"),
            "https://h/api/x/?token=<redacted>&page=<redacted>&<redacted>"
        );
        assert_eq!(
            without_query_values("https://user:s3cr3t@h/x/?a=1#t=s3cr3t"),
            "https://<redacted>@h/x/?a=<redacted>"
        );
        assert_eq!(without_query_values("/x/?"), "/x/?");
    }

    #[test]
    fn methods_parse_in_any_case_and_know_their_bodies() {
        for (text, method, read, body) in [
            ("get", Method::Get, true, false),
            ("HEAD", Method::Head, true, false),
            ("Post", Method::Post, false, true),
            ("put", Method::Put, false, true),
            ("PATCH", Method::Patch, false, true),
            ("delete", Method::Delete, false, false),
        ] {
            let parsed = Method::parse(text).unwrap();
            assert_eq!(parsed, method);
            assert_eq!(parsed.is_read(), read, "{text}");
            assert_eq!(parsed.always_has_body(), body, "{text}");
            assert_eq!(parsed.to_string(), text.to_ascii_uppercase());
        }
        assert_eq!(Method::parse("TRACE").unwrap_err().code(), Code::Usage);
    }

    /// A connection that takes every byte, records how many it was asked to send, and
    /// answers nothing.
    #[derive(Debug)]
    struct Sink(ureq::unversioned::transport::LazyBuffers, Arc<AtomicUsize>);

    impl ureq::unversioned::transport::Transport for Sink {
        fn buffers(&mut self) -> &mut dyn Buffers {
            &mut self.0
        }

        fn transmit_output(
            &mut self,
            amount: usize,
            _: NextTimeout,
        ) -> std::result::Result<(), ureq::Error> {
            self.1.fetch_add(amount, Ordering::Relaxed);
            Ok(())
        }

        fn await_input(&mut self, _: NextTimeout) -> std::result::Result<bool, ureq::Error> {
            Ok(false)
        }

        fn is_open(&mut self) -> bool {
            true
        }
    }

    #[test]
    fn a_marked_connection_records_the_first_request_byte_and_forwards_the_rest() {
        use ureq::unversioned::transport::Transport as _;

        let written = Arc::new(AtomicBool::new(false));
        let sent = Arc::new(AtomicUsize::new(0));
        let sink: Box<dyn ureq::unversioned::transport::Transport> = Box::new(Sink(
            ureq::unversioned::transport::LazyBuffers::new(16, 16),
            Arc::clone(&sent),
        ));
        let mut marked = MarkedTransport {
            inner: sink,
            written: Arc::clone(&written),
        };
        let timeout = || NextTimeout {
            after: std::time::Duration::from_secs(1).into(),
            reason: ureq::Timeout::Global,
            ..NextTimeout::default()
        };
        // Reading and checking the connection is not writing a request.
        assert!(!marked.await_input(timeout()).unwrap());
        assert!(marked.is_open());
        assert!(!marked.is_tls());
        // The sink's own 16-byte output buffer, not a copy.
        assert_eq!(marked.buffers().output().len(), 16);
        assert!(!written.load(Ordering::Relaxed));
        marked.transmit_output(7, timeout()).unwrap();
        assert!(written.load(Ordering::Relaxed));
        assert_eq!(
            sent.load(Ordering::Relaxed),
            7,
            "the amount reaches the connection"
        );
    }

    /// A 2xx the command refuses on its content carries what a server error carries: the
    /// status, the request id, and the body as `details`. A write is `partial`, a read
    /// `none`; a body that is not JSON, or is empty, adds no `details`.
    #[test]
    fn a_refused_2xx_carries_its_status_request_id_and_body() {
        let answer = |write: bool, body: &[u8]| Response {
            status: 200,
            body: body.to_vec(),
            write,
            action: "rename the peer `12` to `build`".to_owned(),
            request_id: Some("rid".into()),
            date: None,
            token_spellings: Vec::new(),
        };
        let json = answer(true, br#"{"id":"12"}"#)
            .refused("the answer has no `name`")
            .to_json();
        assert_eq!(json["error"]["code"], "internal");
        assert_eq!(
            json["error"]["message"],
            "cannot rename the peer `12` to `build`: the answer has no `name`"
        );
        assert_eq!(json["error"]["http_status"], 200);
        assert_eq!(json["error"]["request_id"], "rid");
        assert_eq!(json["error"]["side_effects"], "partial");
        assert_eq!(json["error"]["retryable"], false);
        assert_eq!(json["error"]["details"], json!({"id": "12"}));
        for body in [b"nope".as_slice(), b""] {
            let json = answer(false, body).refused("why").to_json();
            assert_eq!(json["error"]["side_effects"], "none");
            assert_eq!(json["error"]["http_status"], 200);
            assert_eq!(json["error"]["details"], Value::Null, "{json}");
        }
    }

    #[test]
    fn a_2xx_body_failure_is_maybe_sent_whatever_the_error_says() {
        // On its own, a TLS error is "refused" and an unreachable network is "not sent".
        let tls = || ureq::Error::Io(std::io::Error::other(rustls::Error::DecryptError));
        let unreachable =
            || ureq::Error::Io(std::io::Error::from(std::io::ErrorKind::NetworkUnreachable));
        assert!(matches!(
            how_far(&tls(), Progress::Unwritten),
            Transport::Refused
        ));
        assert!(matches!(
            how_far(&unreachable(), Progress::Unwritten),
            Transport::NotSent
        ));
        // Once a byte went out, neither says the server did not act.
        for error in [tls(), unreachable()] {
            assert!(matches!(
                how_far(&error, Progress::Written),
                Transport::MaybeSent
            ));
        }
        for error in [tls(), unreachable()] {
            let json = transport_error_as(&error, Transport::MaybeSent, true, &attempt()).to_json();
            assert_eq!(json["error"]["side_effects"], "partial", "{error}");
            assert_eq!(json["error"]["retryable"], false, "{error}");
        }
    }

    #[test]
    fn follow_compares_origins_and_refuses_unusable_links() {
        let page = "https://api.hippius.com/api/k/";
        for next in [
            "https://api.hippius.com:443/api/k/?page=2",
            "HTTPS://API.hippius.com/api/k/?page=2",
            "/api/k/?page=2",
        ] {
            assert!(follow(page, next).is_ok(), "{next}");
        }
        // The API's own `http://` links are followed on the page's https origin.
        for next in [
            "http://api.hippius.com/api/k/?page=2",
            "http://API.hippius.com:80/api/k/?page=2",
        ] {
            assert_eq!(
                follow(page, next).unwrap().target,
                "https://api.hippius.com/api/k/?page=2",
                "{next}"
            );
        }
        for next in [
            "https://api.hippius.com/api/k/?page=2 ",
            "/api/k/?page=2\n",
            "https://evil.example/api/k/",
            "http://evil.example/api/k/",
            "http://api.hippius.com:8080/api/k/",
            "http://api.hippius.com:443/api/k/",
            "http://api.hippius.com.evil.example/api/k/",
            "http://api.hippius.com./api/k/",
            "http://api.hippius.com@evil.example/api/k/",
            "http://u@api.hippius.com/api/k/",
            "http://api.hippius.com\\@evil.example/",
        ] {
            assert_eq!(
                follow(page, next).unwrap_err().code(),
                Code::Internal,
                "{next:?}"
            );
        }
        for (next, quoted) in [
            ("?page=2", "`?page=<redacted>`"),
            ("api/k/?page=2", "`api/k/?page=<redacted>`"),
            ("", "``"),
            ("?r=https://x", "`?r=<redacted>`"),
        ] {
            let error = follow(page, next).unwrap_err();
            assert_eq!(error.code(), Code::Internal, "{next:?}");
            assert_eq!(
                error.to_string(),
                format!("the API's `next` link {quoted} is neither a URL nor a path"),
            );
        }
        // Never the other way: an http page does not move onto https, or anywhere else.
        assert_eq!(
            follow("http://127.0.0.1:9/api/k/", "https://127.0.0.1:9/api/k/")
                .unwrap_err()
                .code(),
            Code::Internal
        );
        // A page URL with the default port spelled out is the same origin.
        assert_eq!(
            follow(
                "https://api.hippius.com:443/api/k/",
                "http://api.hippius.com/api/k/?page=2"
            )
            .unwrap()
            .target,
            "https://api.hippius.com/api/k/?page=2"
        );
    }

    #[test]
    fn only_a_port_may_follow_the_host() {
        for bad in [
            "http://[::1]evil.example",
            "http://[::1].attacker.example:9",
            "https://api.hippius.com:abc",
            "https://api.hippius.com:",
            "http://localhost:80x",
            "https://api.hippius.com:abc:443",
            "https://\u{430}pi.hippius.com",
        ] {
            assert_eq!(Origin::parse(bad).unwrap_err().code(), Code::Usage, "{bad}");
        }
        assert!(Origin::parse("http://[::1]:9").is_ok());
        assert!(Origin::parse("https://api.hippius.com:8443").is_ok());
        assert!(Origin::parse("https://api.hippius.com:65536").is_err());
        assert_eq!(
            Origin::parse("https://api.hippius.com:0443")
                .unwrap()
                .as_str(),
            "https://api.hippius.com"
        );
        assert_eq!(
            Origin::parse("http://127.0.0.1:09").unwrap().as_str(),
            "http://127.0.0.1:9"
        );
    }

    #[test]
    fn an_origin_without_a_host_is_refused() {
        for bad in [
            "https://:443",
            "https://:8443",
            "https://[",
            "https://[evil",
            "https://[]",
            "https://[]:443",
            "https://[x]",
            "https://[:]",
        ] {
            let error = Origin::parse(bad).unwrap_err();
            assert_eq!(error.code(), Code::Usage, "{bad}");
            assert_eq!(
                error.to_string(),
                format!("`{bad}` is not an API origin: it has no host"),
            );
        }
    }

    /// A host name is letters, digits, `-`, and `.` (RFC 1123, section 2.1): anything else
    /// would reach a confirmation as a broken literal, and a dry run would accept an origin
    /// the HTTP library refuses to build a request for.
    #[test]
    fn a_host_holds_only_letters_digits_hyphens_and_dots() {
        for bad in [
            "https://a\"b<c>{d}",
            "https://evil.example`,_named_by_the_default_`x",
            "https://a_b.example",
            "https://a|b",
            "https://a\\b",
            "https://a^b",
            "https://a'b",
            "https://a(b)",
            "https://a;b",
            "https://a!b:443",
        ] {
            let error = Origin::parse(bad).unwrap_err();
            assert_eq!(error.code(), Code::Usage, "{bad}");
            assert!(
                error.to_string().ends_with(
                    "is not an API origin: its host holds a character other than a letter, a \
                     digit, `-`, or `.`"
                ),
                "{error}"
            );
            // The message quotes the value through `phrase::quote`, so a backtick in it
            // cannot end the literal.
            assert!(error.to_string().starts_with(&quote(bad)), "{error}");
        }
        for good in [
            "https://api-2.hippius.com",
            "https://127.0.0.1:8443",
            "https://API.example.",
        ] {
            Origin::parse(good).unwrap();
        }
    }

    #[test]
    fn a_default_port_is_the_same_origin() {
        let plain = Origin::parse("https://api.hippius.com").unwrap();
        assert_eq!(
            Origin::parse("https://API.hippius.com:443/").unwrap(),
            plain
        );
        assert_eq!(
            Origin::parse("http://127.0.0.1:80").unwrap().as_str(),
            "http://127.0.0.1"
        );
        assert_eq!(
            Origin::parse("https://api.hippius.com:8443")
                .unwrap()
                .as_str(),
            "https://api.hippius.com:8443"
        );
        assert_eq!(
            Origin::parse("http://[::1]:80").unwrap().as_str(),
            "http://[::1]"
        );
        assert!(!plain.is_plain_http());
        assert!(Origin::parse("http://127.0.0.1:9").unwrap().is_plain_http());
    }

    #[test]
    fn origins_are_https_or_loopback_http() {
        for (text, origin) in [
            ("https://api.hippius.com", "https://api.hippius.com"),
            ("https://API.Hippius.com/", "https://api.hippius.com"),
            (
                "HTTPS://api.hippius.com:8443",
                "https://api.hippius.com:8443",
            ),
            ("http://127.0.0.1:9", "http://127.0.0.1:9"),
            ("http://[::1]:7", "http://[::1]:7"),
        ] {
            assert_eq!(Origin::parse(text).unwrap().as_str(), origin, "{text}");
        }
        for bad in [
            "api.hippius.com",
            "http://api.hippius.com",
            "http://127.0.0.2",
            "http://localhost:8080",
            "ftp://api.hippius.com",
            "https://",
            "https:///",
            "https://api.hippius.com/api",
            "https://api.hippius.com?x=1",
            "https://user:pass@api.hippius.com",
            "https://api.hippius .com",
            "https://api.hippius.com\n",
        ] {
            assert_eq!(
                Origin::parse(bad).unwrap_err().code(),
                Code::Usage,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn paths_join_under_api_once_and_queries_are_encoded() {
        let c = client(Selection::Own);
        assert_eq!(
            c.url(&Request::get("/accounts/")).unwrap(),
            "https://api.hippius.com/api/accounts/"
        );
        assert_eq!(
            c.url(&Request::get("/api/accounts/")).unwrap(),
            "https://api.hippius.com/api/accounts/"
        );
        assert_eq!(
            c.url(&Request::get("/registry/namespaces/check/").query("name", "a b&c/é"))
                .unwrap(),
            "https://api.hippius.com/api/registry/namespaces/check/?name=a%20b%26c%2F%C3%A9"
        );
        assert_eq!(
            c.url(&Request::get("/x/?page=2").query("page_size", "5"))
                .unwrap(),
            "https://api.hippius.com/api/x/?page=2&page_size=5"
        );
        assert_eq!(
            c.url(&Request::get("https://API.hippius.com/api/accounts/"))
                .unwrap(),
            "https://api.hippius.com/api/accounts/"
        );
        assert_eq!(
            c.url(&Request::get("https://api.hippius.com?x")).unwrap(),
            "https://api.hippius.com/?x"
        );
        for bad in ["accounts/", "ftp://x/y", "https:///x", "nope://"] {
            assert_eq!(
                c.url(&Request::get(bad)).unwrap_err().code(),
                Code::Usage,
                "{bad}"
            );
        }
    }

    /// The account token route is never sent, on any method: the refusal comes before the
    /// secret check, so `hipc api` names it rather than ask for a `--reveal` that leads to it.
    #[test]
    fn the_account_token_route_is_refused_before_the_secret_check() {
        let c = client(Selection::Own);
        for method in [
            Method::Get,
            Method::Head,
            Method::Post,
            Method::Put,
            Method::Patch,
            Method::Delete,
        ] {
            for path in ["/auth/token/", "/api/auth/token", "/auth/token.json"] {
                let request = Request::new(method, path);
                let error = c.returns_secret(&request).unwrap_err();
                assert_eq!(error.code(), Code::Usage, "{method} {path}");
                let message = error.message();
                assert!(message.contains("/api/auth/token/`"), "{message}");
                assert_eq!(c.route_flags(&request).unwrap_err().code(), Code::Usage);
            }
        }
    }

    /// A full URL off the configured origin is refused before the account rules, a
    /// look-alike host and another port included, and every public check of the request
    /// refuses it the same way; the configured origin in another spelling is that origin.
    #[test]
    fn a_full_url_on_another_origin_is_refused() {
        let c = client(member());
        for url in [
            "https://evil.example/api/compute/vms/",
            "https://api.hippius.com.evil.example/api/compute/vms/",
            "https://api.hippius.com:8443/api/compute/vms/",
            "http://api.hippius.com/api/compute/vms/",
            "http://127.0.0.1:9/api/compute/vms/",
            "https://user@api.hippius.com/api/compute/vms/",
        ] {
            let request = Request::get(url);
            let error = c.resolve(&request).unwrap_err();
            assert_eq!(error.code(), Code::Usage, "{url}");
            assert_eq!(
                error.hint(),
                Some("pass a path such as `/compute/vms/`, or use curl for another host")
            );
            for refused in [
                c.returns_secret(&request).map(|_| ()),
                c.provisions(&request).map(|_| ()),
                c.is_permanent(&request).map(|_| ()),
                c.route_flags(&request).map(|_| ()),
                c.opens_a_namespace(&request).map(|_| ()),
                c.send(&request).map(|_| ()),
            ] {
                assert_eq!(refused.unwrap_err().code(), Code::Usage, "{url}");
            }
        }
        let error = c
            .resolve(&Request::get("https://evil.example/x/?key=s3cr3t"))
            .unwrap_err();
        assert_eq!(
            error.message(),
            "`https://evil.example/x/?key=<redacted>` is not on the API origin \
             `https://api.hippius.com`: `hipc` sends only there"
        );
        for url in [
            "https://api.hippius.com/api/compute/vms/",
            "HTTPS://API.hippius.com:443/api/compute/vms/",
        ] {
            let resolved = c.resolve(&Request::get(url)).unwrap();
            assert_eq!(
                resolved.account_header.as_deref(),
                Some("acct_0123456789abcdef0123"),
                "{url}"
            );
        }
    }

    /// The answer comes before the account rules, so `--reveal` can be checked first: a
    /// membership on a route that cannot act in another account still gets an answer.
    #[test]
    fn whether_a_route_issues_a_secret_ignores_the_selected_account() {
        let c = client(member());
        let post = |target: &str| Request::new(Method::Post, target);
        let provision = post("/registry/provision/");
        assert_eq!(
            c.account_header(&provision).unwrap_err().code(),
            Code::AccountScopeUnsupported
        );
        assert!(c.returns_secret(&provision).unwrap());
        assert!(
            !c.returns_secret(&Request::get("/registry/provision/"))
                .unwrap()
        );
        assert!(
            c.returns_secret(&post("https://api.hippius.com/api/registry/provision/?a=1"))
                .unwrap()
        );
        assert!(
            c.returns_secret(&post("https://evil.example/api/registry/provision/"))
                .is_err()
        );
        assert!(c.returns_secret(&post("registry/provision/")).is_err());
    }

    /// Whether a read provisions depends on the method and the target alone, as the secret
    /// answer does: a client in a project and a billing account, which the route refuses
    /// once the request is marked a write, still gets the answer; a full URL on the API
    /// origin counts, and a foreign origin or a bad target is the same refusal `send` makes.
    #[test]
    fn whether_a_read_provisions_ignores_the_selection() {
        let c = selecting(member());
        let summary = Request::get("/network/me/");
        assert!(c.provisions(&summary).unwrap());
        let marked = summary.clone().provisions();
        assert_eq!(
            c.url(&marked).unwrap_err().code(),
            Code::BillingScopeUnsupported
        );
        assert!(c.provisions(&marked).unwrap());
        assert!(
            c.provisions(&Request::new(Method::Head, "/network/me"))
                .unwrap()
        );
        assert!(
            !c.provisions(&Request::new(Method::Post, "/network/me/"))
                .unwrap()
        );
        assert!(!c.provisions(&Request::get("/network/peers/")).unwrap());
        assert!(
            c.provisions(&Request::get("https://api.hippius.com/api/network/me/?a=1"))
                .unwrap()
        );
        assert!(
            c.provisions(&Request::get("https://evil.example/api/network/me/"))
                .is_err()
        );
        assert!(c.provisions(&Request::get("network/me/")).is_err());
    }

    /// The route flag, like the secret check, comes before the account rules, ignores the
    /// query, and refuses another origin.
    #[test]
    fn the_route_flag_ignores_the_account_and_the_query() {
        let c = client(member());
        let put = |target: &str| Request::new(Method::Put, target);
        let storage = put("/v1/operator/nodes/n1/storage/");
        assert_eq!(
            c.account_header(&storage).unwrap_err().code(),
            Code::AccountScopeUnsupported
        );
        let names = |request: &Request| flag_names(&c, request);
        assert_eq!(names(&storage), ["--remove-others"]);
        assert_eq!(
            names(&put(
                "https://api.hippius.com/api/v1/operator/nodes/n1/storage/?force=1"
            )),
            ["--remove-others"]
        );
        assert_eq!(
            names(&Request::new(Method::Post, "/network/isolation/?a=1")),
            ["--change-isolation"]
        );
        let foreign = put("https://evil.example/api/v1/operator/nodes/n1/storage/");
        assert_eq!(c.route_flags(&foreign).unwrap_err().code(), Code::Usage);
        assert_eq!(names(&Request::get("/network/isolation/")), NONE);
        // A Hub delete needs `--yes` alone.
        let repository = Request::new(Method::Delete, "/registry/repositories/app/");
        assert_eq!(names(&repository), NONE);
        assert!(
            c.route_flags(&put("v1/operator/nodes/n1/storage/"))
                .is_err()
        );
    }

    /// A route table's value is its first row that names the request, slashless and suffixed
    /// forms included; a target the client refuses is an error.
    #[test]
    fn a_route_value_is_the_first_row_that_names_the_request() {
        let c = client(member());
        let table = [
            (Method::Delete, "/api/compute/vms/{id}/", "vm"),
            (Method::Delete, "/api/registry/subscriptions/{n}/", "plan"),
            (Method::Delete, "/api/registry/subscriptions/{id}/", "later"),
        ];
        let value = |method: Method, target: &str| {
            c.route_value(&Request::new(method, target), &table)
                .unwrap()
                .copied()
        };
        assert_eq!(value(Method::Delete, "/compute/vms/vm-1/"), Some("vm"));
        // In any letter case, as a front end that folds it would deliver it.
        assert_eq!(value(Method::Delete, "/Compute/VMS/vm-1/"), Some("vm"));
        assert_eq!(value(Method::Delete, "/compute/vms/vm-1"), Some("vm"));
        assert_eq!(
            value(Method::Delete, "/registry/subscriptions/7.json"),
            Some("plan")
        );
        assert_eq!(
            value(Method::Delete, "/registry/subscriptions/x/"),
            Some("later")
        );
        assert_eq!(value(Method::Get, "/compute/vms/vm-1/"), None);
        assert_eq!(value(Method::Delete, "/compute/vms/"), None);
        assert!(
            c.route_value(&Request::new(Method::Delete, "compute/vms/1/"), &table)
                .is_err()
        );
    }

    /// A Hub delete is permanent on the API origin, its query ignored; another origin, and
    /// any target the client refuses, is an error.
    #[test]
    fn a_hub_delete_is_permanent_on_the_api_origin_only() {
        let c = client(member());
        let delete = |target: &str| Request::new(Method::Delete, target);
        assert!(
            c.is_permanent(&delete("/registry/repositories/team/app/"))
                .unwrap()
        );
        assert!(
            c.is_permanent(&delete(
                "https://api.hippius.com/api/registry/repositories/app/artifacts/v1/?a=1"
            ))
            .unwrap()
        );
        assert!(
            c.is_permanent(&delete(
                "https://evil.example/api/registry/repositories/app/"
            ))
            .is_err()
        );
        assert!(
            !c.is_permanent(&Request::get("/registry/repositories/app/"))
                .unwrap()
        );
        assert!(
            c.is_permanent(&delete("registry/repositories/app/"))
                .is_err()
        );
    }

    /// No route flag.
    const NONE: [&str; 0] = [];

    /// The flags `request` needs, as the caller types them.
    fn flag_names(c: &Client, request: &Request) -> Vec<&'static str> {
        c.route_flags(request)
            .unwrap()
            .iter()
            .map(|flag| flag.flag())
            .collect()
    }

    /// A zone PATCH needs `--public` only when its body sets `origin`, and a member PATCH
    /// or a WAF list PATCH needs `--remove-others` only when it sets `grants` or `items`; a
    /// body that is not an object fails closed, and no body sets nothing.
    #[test]
    fn a_body_rule_reads_the_top_level_keys_of_the_body() {
        let c = client(member());
        let patch = |target: &str, body: Option<&str>| {
            let request = Request::new(Method::Patch, target);
            match body {
                Some(body) => request.raw_json(body.as_bytes().to_vec()),
                None => request,
            }
        };
        let names = |request: &Request| flag_names(&c, request);
        for (target, key, other, expected) in [
            ("/cdn/zones/z1/", "origin", "name", "--public"),
            (
                "/accounts/members/7/",
                "grants",
                "can_billing",
                "--remove-others",
            ),
            (
                "/cdn/lists/blocked/",
                "items",
                "description",
                "--remove-others",
            ),
        ] {
            for body in [
                format!(r#"{{"{key}":{{}}}}"#),
                format!(r#"{{"{key}":null}}"#),
                format!(r#"{{"{other}":1,"{key}":[]}}"#),
                "[]".to_owned(),
                "null".to_owned(),
                "\"x\"".to_owned(),
                "{".to_owned(),
            ] {
                assert_eq!(names(&patch(target, Some(&body))), [expected], "{body}");
            }
            for body in [format!(r#"{{"{other}":1}}"#), "{}".to_owned()] {
                assert_eq!(names(&patch(target, Some(&body))), NONE, "{body}");
            }
            // A key below the top level is not the key.
            let nested = format!(r#"{{"{other}":{{"{key}":1}}}}"#);
            assert_eq!(names(&patch(target, Some(&nested))), NONE);
            assert_eq!(names(&patch(target, None)), NONE);
        }
    }

    /// A Hub namespace publicity PATCH needs `--public` unless its body is an object that sets
    /// `public` to `false`: no body, `{}`, and a body that is not an object fail closed, since
    /// swagger calls the route a toggle; a `public` below the top level is not the key, and a
    /// key given twice counts as its last copy.
    #[test]
    fn a_publicity_patch_needs_public_unless_it_makes_the_namespace_private() {
        let c = client(member());
        let patch = |body: Option<&str>| {
            let request = Request::new(Method::Patch, "/registry/me/publicity/");
            match body {
                Some(body) => request.raw_json(body.as_bytes().to_vec()),
                None => request,
            }
        };
        for body in [
            r#"{"public":true}"#,
            r#"{"public":"false"}"#,
            r#"{"public":0}"#,
            r#"{"public":null}"#,
            r#"{"public":false,"public":true}"#,
            "[]",
            "null",
            "{",
            "{}",
            r#"{"name":{"public":false}}"#,
        ] {
            assert_eq!(flag_names(&c, &patch(Some(body))), ["--public"], "{body}");
        }
        assert_eq!(flag_names(&c, &patch(None)), ["--public"]);
        for body in [
            r#"{"public":false}"#,
            r#"{"public":true,"public":false}"#,
            r#"{"name":{"public":true},"public":false}"#,
        ] {
            assert_eq!(flag_names(&c, &patch(Some(body))), NONE, "{body}");
        }
        let get = Request::get("/registry/me/publicity/");
        assert_eq!(flag_names(&c, &get), NONE);
    }

    /// An App Engine launch, compose update, or compose check is refused when its body sets
    /// `secrets` at the top level, or is not a JSON object; otherwise it needs no flag.
    #[test]
    fn an_app_engine_body_with_secrets_is_refused() {
        let c = client(member());
        for (method, target, hint) in [
            (Method::Post, "/compute/app-engine/apps/", "apps create"),
            (
                Method::Put,
                "/compute/app-engine/apps/vm-1/compose/",
                "not in this version",
            ),
            (
                Method::Patch,
                "/compute/app-engine/apps/vm-1/compose",
                "not in this version",
            ),
            (Method::Post, "/compute/app-engine/validate", "validate`"),
        ] {
            let with = |body: &str| Request::new(method, target).raw_json(body.as_bytes().to_vec());
            for body in [
                r#"{"compose":"x","secrets":{}}"#,
                r#"{"secrets":null}"#,
                "[]",
                "{",
            ] {
                let error = c.route_flags(&with(body)).unwrap_err();
                assert_eq!(error.code(), Code::Usage, "{target} {body}");
                assert!(error.message().contains("with `secrets`"), "{target}");
                assert!(error.hint().unwrap().contains(hint), "{target}");
            }
            for body in [r#"{"compose":"x"}"#, r#"{"compose":{"secrets":{}}}"#] {
                assert_eq!(flag_names(&c, &with(body)), NONE, "{target} {body}");
            }
            assert_eq!(flag_names(&c, &Request::new(method, target)), NONE);
        }
    }

    /// A bucket ACL set needs `--public` and then `--remove-others`, whatever its body.
    #[test]
    fn a_bucket_acl_set_needs_both_flags_whatever_the_body() {
        let c = client(member());
        for body in [r#"{"policy":{}}"#, r#"{"grants":{}}"#, "{}", "[]"] {
            let request = Request::new(Method::Put, "/objectstore/buckets/b/acl/set/")
                .raw_json(body.as_bytes().to_vec());
            assert_eq!(
                flag_names(&c, &request),
                ["--public", "--remove-others"],
                "{body}"
            );
        }
        let bare = Request::new(Method::Put, "/objectstore/buckets/b/acl/set");
        assert_eq!(flag_names(&c, &bare), ["--public", "--remove-others"]);
    }

    /// An account deletion request is refused whatever the flags, with a hint that points
    /// at the console, and the query does not hide it.
    #[test]
    fn an_account_deletion_request_is_left_to_the_console() {
        let c = client(member());
        let error = c
            .route_flags(&Request::new(Method::Post, "/account/deletion/?a=1"))
            .unwrap_err();
        assert_eq!(error.code(), Code::Usage);
        assert_eq!(
            error.hint(),
            Some("request the deletion in the console, under Settings, Delete Account")
        );
    }

    /// A Kubernetes cluster's secrets read, GET or HEAD, is refused whatever the flags, with
    /// a hint that points at `clusters kubeconfig`.
    #[test]
    fn a_cluster_secrets_read_is_left_to_the_kubeconfig_command() {
        let c = client(member());
        for (method, target) in [
            (Method::Get, "/kubernetes/clusters/7/secrets/?a=1"),
            (Method::Get, "/kubernetes/clusters/7/secrets"),
            (Method::Head, "/kubernetes/clusters/7/secrets/"),
        ] {
            let error = c.route_flags(&Request::new(method, target)).unwrap_err();
            assert_eq!(error.code(), Code::Usage, "{target}");
            assert!(
                error
                    .hint()
                    .unwrap()
                    .starts_with("run `hipc compute kubernetes clusters kubeconfig`"),
                "{target}"
            );
        }
    }

    #[test]
    fn dry_run_describes_without_header_values() {
        let c = client(member());
        let doc = c
            .describe(
                &Request::new(Method::Post, "/s3/subscription/").json(&json!({"plan": "pro"})),
            )
            .unwrap();
        assert_eq!(
            doc,
            json!({
                "dry_run": true,
                "method": "POST",
                "url": "https://api.hippius.com/api/s3/subscription/",
                "account_header": "acct_0123456789abcdef0123",
                "project_header": null,
                "billing_header": null,
                "body": {"plan": "pro"}
            })
        );
        assert!(!doc.to_string().contains("t0k3n"));
        let refused = c
            .describe(&Request::get("/drive/subscription/"))
            .unwrap_err();
        assert_eq!(refused.code(), Code::AccountScopeUnsupported);
        let not_json = Request::new(Method::Post, "/x/").raw_json(b"{".to_vec());
        assert_eq!(
            client(Selection::Own).describe(&not_json).unwrap()["body"],
            Value::Null
        );
    }

    const PROJECT: &str = "proj_0a1b2c3d4e5f60718293";
    const BILLING: &str = "bill_9f8e7d6c5b4a39281706";
    const MEMBER: &str = "acct_0123456789abcdef0123";

    /// A client in the project and billing account of the docs' examples. No route takes
    /// both, so every route but the personal ones refuses one of them.
    fn selecting(account: Selection) -> Client {
        let mut settings = settings("https://api.hippius.com", account);
        settings.project = ProjectId::parse(PROJECT, &Source::Argument).unwrap();
        settings.billing = BillingId::parse(BILLING, &Source::Argument).unwrap();
        Client::new(&settings, Duration::from_secs(5)).unwrap()
    }

    /// A client in the project of the docs' example, or in its billing account.
    fn in_project(account: Selection) -> Client {
        let mut settings = settings("https://api.hippius.com", account);
        settings.project = ProjectId::parse(PROJECT, &Source::Argument).unwrap();
        Client::new(&settings, Duration::from_secs(5)).unwrap()
    }

    fn on_billing_account(account: Selection) -> Client {
        let mut settings = settings("https://api.hippius.com", account);
        settings.billing = BillingId::parse(BILLING, &Source::Argument).unwrap();
        Client::new(&settings, Duration::from_secs(5)).unwrap()
    }

    /// The dry run names each scope header it sends, and `null` for one the request goes
    /// out without, whether nothing is selected or the route takes no such header, so its
    /// shape never depends on the selection.
    #[test]
    fn a_dry_run_always_shows_the_three_scope_headers() {
        // Each header as `get` reads it, so a missing key (`None`) fails where `null` is
        // expected.
        fn headers(doc: &Value) -> [Option<&Value>; 3] {
            ["account_header", "project_header", "billing_header"].map(|field| doc.get(field))
        }
        let (null, project, billing) = (Value::Null, json!(PROJECT), json!(BILLING));
        let doc = in_project(Selection::Own)
            .describe(&Request::get("/compute/vms/"))
            .unwrap();
        assert_eq!(headers(&doc), [Some(&null), Some(&project), Some(&null)]);
        let doc = on_billing_account(Selection::Own)
            .describe(&Request::get("/billing/credits/balance/"))
            .unwrap();
        assert_eq!(headers(&doc), [Some(&null), Some(&null), Some(&billing)]);
        let doc = selecting(Selection::Own)
            .describe(&Request::get("/ssh-keys/"))
            .unwrap();
        assert_eq!(headers(&doc), [Some(&null); 3]);
        let doc = client(Selection::Own)
            .describe(&Request::get("/compute/vms/"))
            .unwrap();
        let keys: Vec<&str> = doc
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "dry_run",
                "method",
                "url",
                "account_header",
                "project_header",
                "billing_header",
                "body"
            ]
        );
        assert_eq!(headers(&doc), [Some(&null); 3]);
    }

    /// The billing account is checked first, so on a route that takes no selector a billing
    /// refusal comes before the project's and the account's.
    #[test]
    fn the_selector_refusals_come_billing_first() {
        let both = selecting(member());
        for path in ["/drive/subscription/", "/compute/vms/"] {
            let error = both.describe(&Request::get(path)).unwrap_err();
            assert_eq!(error.code(), Code::BillingScopeUnsupported, "{path}");
        }
        let error = both
            .describe(&Request::get("/billing/credits/balance/"))
            .unwrap_err();
        assert_eq!(error.code(), Code::ProjectScopeUnsupported);
        let project = in_project(member());
        let error = project
            .describe(&Request::get("/drive/subscription/"))
            .unwrap_err();
        assert_eq!(error.code(), Code::AccountScopeUnsupported);
        let vms = Request::get("/compute/vms/");
        assert_eq!(project.describe(&vms).unwrap()["project_header"], PROJECT);
        assert_eq!(project.billing_header(&vms).unwrap(), None);
        assert_eq!(
            project.account_header(&vms).unwrap().as_deref(),
            Some(MEMBER)
        );
    }

    /// A membership on a recurring card route is sent beside a billing account, with both
    /// headers, and refused without one.
    #[test]
    fn a_recurring_card_route_takes_the_account_beside_a_billing_account() {
        let stripe = Request::get("/billing/stripe/active-subscription/");
        let doc = on_billing_account(member()).describe(&stripe).unwrap();
        assert_eq!(doc["account_header"], MEMBER);
        assert_eq!(doc["billing_header"], BILLING);
        let error = client(member()).describe(&stripe).unwrap_err();
        assert_eq!(error.code(), Code::AccountScopeUnsupported);
    }

    /// A server 403 with a selector code passes through with the hint that fixes it.
    #[test]
    fn a_403_passes_the_selector_codes_through_with_their_hints() {
        for (code, hint) in [
            (
                "project_scope_unsupported",
                "pass `--project default` to act in the Default project",
            ),
            (
                "billing_scope_unsupported",
                "pass `--billing default` to use the Default billing account",
            ),
        ] {
            let body = format!(r#"{{"code":"{code}","detail":"no"}}"#);
            let sent = Attempt {
                project: true,
                billing: true,
                ..attempt()
            };
            let error = status_error(&StatusAnswer {
                attempt: &sent,
                write: false,
                not_done_on_503: false,
                status: 403,
                body: body.as_bytes(),
                retry_after: None,
                request_id: None,
                location: None,
                token_spellings: &[],
            });
            assert_eq!(error.code().as_str(), code);
            assert_eq!(error.code().exit_code(), 2);
            assert_eq!(error.hint(), Some(hint));
        }
    }

    /// A scope refusal of a header the request did not carry keeps its code (tenet 4), but
    /// its hint names no flag value: none would change the answer.
    #[test]
    fn a_scope_refusal_without_its_header_names_no_flag() {
        for (code, header) in [
            ("account_scope_unsupported", "X-Hippius-Account"),
            ("project_scope_unsupported", "X-Hippius-Project"),
            ("billing_scope_unsupported", "X-Hippius-Billing"),
        ] {
            let body = format!(r#"{{"code":"{code}"}}"#);
            let error = status_error(&StatusAnswer {
                attempt: &attempt(),
                write: false,
                not_done_on_503: false,
                status: 403,
                body: body.as_bytes(),
                retry_after: None,
                request_id: None,
                location: None,
                token_spellings: &[],
            });
            assert_eq!(error.code().as_str(), code);
            let hint = error.hint().unwrap();
            assert!(hint.contains(&format!("no `{header}`")), "{hint}");
            assert!(!hint.contains("pass `--"), "{hint}");
        }
    }

    /// A precondition shows in the dry run only on the request that carries it, and its
    /// `Debug` names it, as a version is no secret.
    #[test]
    fn dry_run_shows_an_if_match_only_when_set() {
        let c = client(Selection::Own);
        let put = Request::new(Method::Put, "/cdn/zones/z1/waf/rules/").json(&json!({}));
        assert!(c.describe(&put).unwrap().get("if_match").is_none());
        let put = put.if_match("7");
        assert_eq!(c.describe(&put).unwrap()["if_match"], "7");
        assert!(format!("{put:?}").contains(r#"if_match: Some("7")"#));
    }

    #[test]
    fn debug_of_a_client_hides_the_token() {
        let text = format!("{:?}", client(Selection::Own));
        assert!(!text.contains("t0k3n"), "{text}");
        assert!(text.contains("has_token: true"), "{text}");
    }

    /// A request to get the VM `vm_1` from `api.hippius.com`, in the token's own account.
    fn attempt() -> Attempt<'static> {
        Attempt {
            action: "get the VM `vm_1`".to_owned(),
            host: "api.hippius.com",
            member: false,
            project: false,
            billing: false,
            listed_by: Some("hipc compute vms list"),
            token: TokenFrom::Stored,
        }
    }

    fn answer(write: bool, status: u16, body: &str) -> Error {
        status_error(&StatusAnswer {
            attempt: &attempt(),
            write,
            not_done_on_503: false,
            status,
            body: body.as_bytes(),
            retry_after: Some("7"),
            request_id: Some("rid".into()),
            location: Some("https://elsewhere/"),
            token_spellings: &[],
        })
    }

    #[test]
    fn every_status_maps_to_its_contract_code() {
        for (status, code) in [
            (400, Code::Validation),
            (401, Code::Auth),
            (402, Code::PaymentRequired),
            (403, Code::Permission),
            (404, Code::NotFound),
            (405, Code::Validation),
            (409, Code::Conflict),
            (412, Code::Conflict),
            (418, Code::Validation),
            (422, Code::Validation),
            (425, Code::Unavailable),
            (429, Code::RateLimited),
            (500, Code::Internal),
            (502, Code::Unavailable),
            (503, Code::Unavailable),
            (504, Code::Unavailable),
            (501, Code::Internal),
            (505, Code::Internal),
            (599, Code::Internal),
            (301, Code::Internal),
            (302, Code::Internal),
            (100, Code::Validation),
        ] {
            assert_eq!(answer(false, status, "").code(), code, "{status}");
        }
    }

    #[test]
    fn a_403_passes_account_codes_through() {
        let body = r#"{"code":"owner_only","error":"Only the owner can do this."}"#;
        let error = answer(false, 403, body);
        assert_eq!(error.code(), Code::OwnerOnly);
        let json = error.to_json();
        // The server's text is quoted unchanged, its capital and its period included.
        assert_eq!(
            json["error"]["message"],
            "cannot get the VM `vm_1`: the API says `Only the owner can do this.`"
        );
        assert_eq!(json["error"]["details"]["code"], "owner_only");
        assert_eq!(
            answer(false, 403, r#"{"code":"project_not_found"}"#).code(),
            Code::Permission
        );
    }

    #[test]
    fn message_comes_from_error_detail_or_message_else_the_status() {
        let cases = [
            (r#"{"error":"e"}"#, "the API says `e`"),
            (r#"{"detail":"d"}"#, "the API says `d`"),
            (r#"{"message":"m"}"#, "the API says `m`"),
            (r#"{"error":5,"detail":"d"}"#, "the API says `d`"),
            (r#"{"other":"x"}"#, "the API answered 400"),
            ("not json", "the API answered 400"),
            ("", "the API answered 400"),
            // Django REST framework's field errors, the commonest 400: each
            // field with its messages, the first three in the server's order.
            (
                r#"{"name":["This field is required."]}"#,
                "the API says `name: This field is required.`",
            ),
            (
                r#"{"a":["x","y"],"b":["z"],"c":["w"]}"#,
                "the API says `a: x y; b: z; c: w`",
            ),
            (
                r#"{"a":["x"],"b":["z"],"c":["w"],"d":["v"],"e":["u"]}"#,
                "the API says `a: x; b: z; c: w` and 2 more fields",
            ),
            (
                r#"{"a":["x"],"b":["z"],"c":["w"],"d":["v"]}"#,
                "the API says `a: x; b: z; c: w` and 1 more field",
            ),
            (r#"["Invalid invite."]"#, "the API says `Invalid invite.`"),
            // A field whose errors are not plain text (a nested list item) is left to
            // `details`, and does not count among the others.
            (
                r#"{"rules":[{"port":["bad"]}],"name":["x"]}"#,
                "the API says `name: x`",
            ),
            (r#"{"rules":[{"port":["bad"]}]}"#, "the API answered 400"),
            (r#"{"name":[]}"#, "the API answered 400"),
            ("[1]", "the API answered 400"),
            // A body that is one JSON string is the text itself; a blank one says nothing.
            (r#""Rate limit hit""#, "the API says `Rate limit hit`"),
            (r#""  ""#, "the API answered 400"),
        ];
        for (body, why) in cases {
            assert_eq!(
                answer(false, 400, body).to_string(),
                format!("cannot get the VM `vm_1`: {why}"),
                "{body}"
            );
        }
        // A body that gives no text leaves `details` as the only explanation. The hint holds
        // in both modes: JSON mode already prints `details`, and text mode needs `--json`.
        assert_eq!(
            answer(false, 400, r#"{"other":"x"}"#).hint(),
            Some("check the API's answer in `details`, which `--json` prints")
        );
        assert_eq!(answer(false, 400, r#"{"name":["required"]}"#).hint(), None);
        assert_eq!(answer(false, 400, r#"{"error":"e"}"#).hint(), None);
        // An empty object or list explains nothing, so no hint points at it.
        for empty in ["{}", "[]"] {
            assert_eq!(answer(false, 400, empty).hint(), None, "{empty}");
        }
        assert_eq!(
            answer(false, 400, "not json").to_json()["error"]["details"],
            Value::Null
        );
    }

    #[test]
    fn an_error_body_never_carries_a_one_time_secret_into_details() {
        let error = answer(
            false,
            400,
            concat!(
                r#"{"detail":[{"secret":"s3cr3t","robot_secret":"s3cr3t"}],"secret":"s3cr3t","#,
                r#""credentials":{"password":"s3cr3t"},"access_token":"s3cr3t","#,
                r#""refresh_token":"s3cr3t","ticket":"s3cr3t","setup_key":"s3cr3t","#,
                r#""join_command":"s3cr3t","netbird_setup_key":"s3cr3t","#,
                r#""client_secret":"s3cr3t","encrypted_secret":"s3cr3t","#,
                r#""token":["Invalid invite."],"ok":1}"#
            ),
        );
        let json = error.to_json();
        assert!(!json.to_string().contains("s3cr3t"), "{json}");
        // A bare `token` holds a validation error here, not a secret, so it stays.
        assert_eq!(
            json["error"]["details"],
            serde_json::json!({"detail": [{}], "token": ["Invalid invite."], "ok": 1})
        );
        // Every other spelling of a secret field, in any case and with `_`, `-`, or nothing
        // between its words, is dropped too; a field that only holds such a word is kept, and
        // so is a secret field's list of validation errors, which says why and holds none.
        let error = answer(
            false,
            400,
            concat!(
                r#"{"Password":"s3cr3t","secretKey":"s3cr3t","SECRET-ACCESS-KEY":"s3cr3t","#,
                r#""private_key":"s3cr3t","accessKey":"s3cr3t","api_key":"s3cr3t","#,
                r#""nested":[{"Client-Secret":"s3cr3t"}],"access_key_id":"AKIA","#,
                r#""password_set":true,"secret_access_key":["This field may not be blank."]}"#
            ),
        );
        let json = error.to_json();
        assert!(!json.to_string().contains("s3cr3t"), "{json}");
        assert_eq!(
            json["error"]["details"],
            serde_json::json!({
                "nested": [{}],
                "access_key_id": "AKIA",
                "password_set": true,
                "secret_access_key": ["This field may not be blank."]
            })
        );
    }

    #[test]
    fn retry_and_side_effects_follow_the_method_and_status() {
        let read_503 = answer(false, 503, "").to_json();
        assert_eq!(read_503["error"]["retryable"], true);
        assert_eq!(read_503["error"]["retry_after_ms"], 7000);
        assert_eq!(read_503["error"]["side_effects"], "none");
        assert_eq!(read_503["error"]["request_id"], "rid");
        let write_503 = answer(true, 503, "").to_json();
        assert_eq!(write_503["error"]["side_effects"], "partial");
        let write_409 = answer(true, 409, "").to_json();
        assert_eq!(write_409["error"]["side_effects"], "none");
        assert_eq!(write_409["error"]["retryable"], false);
        // A redirect answers a write the server received, and `303 See Other` is the usual
        // answer to a POST that was done; only a rejecting 4xx is `none`.
        for status in [301, 302, 303, 307, 308] {
            let redirected = answer(true, status, "");
            assert!(
                matches!(redirected.side_effects(), SideEffects::Partial),
                "{status}"
            );
            assert_eq!(
                redirected.hint(),
                Some("check whether the change was made before running the command again")
            );
            assert!(matches!(
                answer(false, status, "").side_effects(),
                SideEffects::None
            ));
        }
        assert_eq!(answer(false, 429, "").to_json()["error"]["retryable"], true);
        let auth = answer(false, 401, "");
        assert!(auth.hint().unwrap().contains("hipc auth login"));
        let redirect = answer(false, 302, "").to_string();
        assert!(redirect.contains("https://elsewhere/"), "{redirect}");
        let nowhere = status_error(&StatusAnswer {
            attempt: &attempt(),
            write: false,
            not_done_on_503: false,
            status: 301,
            body: b"",
            retry_after: None,
            request_id: None,
            location: None,
            token_spellings: &[],
        });
        assert_eq!(
            nowhere.to_string(),
            "cannot get the VM `vm_1`: the API answered 301 with a redirect and no `Location`"
        );
    }

    #[test]
    fn retry_after_takes_seconds_only() {
        assert_eq!(retry_after_ms(" 3 "), Some(3000));
        assert_eq!(retry_after_ms("Wed, 21 Oct 2015 07:28:00 GMT"), None);
        assert_eq!(retry_after_ms("-1"), None);
        assert_eq!(retry_after_ms(&u64::MAX.to_string()), None);
        // A day is the most taken; past that the value is ignored, not clamped.
        assert_eq!(retry_after_ms("86400"), Some(86_400_000));
        assert_eq!(retry_after_ms("86401"), None);
        assert_eq!(retry_after_ms("18446744073709551"), None);
    }

    /// The cut falls on a character boundary, counts the characters left, and never cuts a
    /// string of the limit or less.
    #[test]
    fn server_text_is_cut_on_a_character_and_says_how_much_is_left() {
        let limit = "é".repeat(MAX_QUOTED_CHARS);
        assert_eq!(capped(&limit, "!"), format!("`{limit}`"));
        let over = format!("{limit}éa");
        assert_eq!(
            capped(&over, "!"),
            format!("`{limit}` and 2 more characters!")
        );
        assert_eq!(
            capped(&format!("{limit}a"), ""),
            format!("`{limit}` and 1 more character")
        );
    }

    /// A version no header can carry came from the server, so it is `internal`, refused
    /// before anything is sent.
    #[test]
    fn an_if_match_no_header_can_carry_is_internal() {
        let c = Client::new(
            &settings("http://127.0.0.1:1", Selection::Own),
            Duration::from_secs(1),
        )
        .unwrap();
        let put = Request::new(Method::Put, "/cdn/zones/z1/waf/rules/")
            .json(&json!({}))
            .if_match("v\u{1}7");
        let error = c.send(&put).unwrap_err();
        assert_eq!(error.code(), Code::Internal);
        let message = error.message();
        assert!(message.contains("`If-Match`"), "{message}");
        assert_eq!(error.http_status(), None);
    }

    /// A dry run refuses what the HTTP library would refuse to build, as the real run does,
    /// and a path or target is quoted, so a backtick in it cannot close the literal.
    #[test]
    fn a_dry_run_refuses_a_target_the_real_run_cannot_send() {
        let c = client(Selection::Own);
        for path in ["/x/?a=b c", "/account/deletion/ ", "/x/\u{7f}/"] {
            let error = c.describe(&Request::get(path)).unwrap_err();
            assert_eq!(error.code(), Code::Usage, "{path:?}");
            assert_eq!(
                error.hint(),
                Some("write the path with only the characters a URL allows"),
                "{path:?}"
            );
        }
        let error = c.describe(&Request::get("x` then run `y")).unwrap_err();
        let message = error.message();
        let quoted = r"`x\u{60} then run \u{60}y` is not an API path";
        assert!(message.starts_with(quoted), "{message}");
        let error = c.describe(&Request::get("/a/../b`x/")).unwrap_err();
        let message = error.message();
        assert!(
            message.starts_with(r"`/a/../b\u{60}x/` has an empty"),
            "{message}"
        );
    }

    /// Checks `transport_error` for each case: (error, is a write, code, side effects,
    /// retryable), with the request's bytes written or not as `progress` says.
    fn check_transport(progress: Progress, cases: Vec<(ureq::Error, bool, Code, &str, bool)>) {
        for (error, write, code, side_effects, retryable) in cases {
            let mapped = transport_error(&error, progress, write, &attempt());
            assert_eq!(mapped.code(), code, "{error}");
            let json = mapped.to_json();
            assert_eq!(json["error"]["side_effects"], side_effects, "{error}");
            assert_eq!(json["error"]["retryable"], retryable, "{error}");
        }
    }

    #[test]
    fn a_request_that_never_left_is_retryable_with_no_side_effects() {
        use std::io::ErrorKind;
        // Before any request byte: whatever the error, nothing reached the server. ureq
        // reports a stalled DNS lookup, connect, or handshake as the global timeout.
        check_transport(
            Progress::Unwritten,
            vec![
                (
                    ureq::Error::Timeout(ureq::Timeout::Global),
                    true,
                    Code::Timeout,
                    "none",
                    true,
                ),
                (
                    ureq::Error::Io(std::io::Error::from(ErrorKind::UnexpectedEof)),
                    true,
                    Code::Unavailable,
                    "none",
                    true,
                ),
            ],
        );
        // These say so themselves.
        check_transport(
            Progress::Unwritten,
            vec![
                (
                    ureq::Error::ConnectionFailed,
                    true,
                    Code::Unavailable,
                    "none",
                    true,
                ),
                (
                    ureq::Error::HostNotFound,
                    true,
                    Code::Unavailable,
                    "none",
                    true,
                ),
                (
                    ureq::Error::Io(ErrorKind::ConnectionRefused.into()),
                    true,
                    Code::Unavailable,
                    "none",
                    true,
                ),
                (
                    ureq::Error::Io(ErrorKind::NetworkUnreachable.into()),
                    true,
                    Code::Unavailable,
                    "none",
                    true,
                ),
            ],
        );
    }

    #[test]
    fn a_request_that_may_have_landed_is_partial_and_not_retryable_for_a_write() {
        check_transport(
            Progress::Written,
            vec![
                (
                    ureq::Error::Io(std::io::ErrorKind::NetworkUnreachable.into()),
                    true,
                    Code::Unavailable,
                    "partial",
                    false,
                ),
                (
                    ureq::Error::Tls("alert while reading"),
                    true,
                    Code::Unavailable,
                    "partial",
                    false,
                ),
                (
                    ureq::Error::Timeout(ureq::Timeout::Global),
                    true,
                    Code::Timeout,
                    "partial",
                    false,
                ),
                (
                    ureq::Error::Timeout(ureq::Timeout::Global),
                    false,
                    Code::Timeout,
                    "none",
                    true,
                ),
                (
                    ureq::Error::Io(std::io::Error::other("reset")),
                    true,
                    Code::Unavailable,
                    "partial",
                    false,
                ),
                (
                    ureq::Error::BodyStalled,
                    false,
                    Code::Unavailable,
                    "none",
                    true,
                ),
                (
                    ureq::Error::BodyExceedsLimit(1),
                    true,
                    Code::Internal,
                    "partial",
                    false,
                ),
            ],
        );
    }

    #[test]
    fn tls_proxy_and_url_failures_are_not_retryable_and_sent_nothing() {
        check_transport(
            Progress::Unwritten,
            vec![
                (
                    ureq::Error::Tls("bad certificate"),
                    true,
                    Code::Unavailable,
                    "none",
                    false,
                ),
                (
                    ureq::Error::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer),
                    )),
                    true,
                    Code::Unavailable,
                    "none",
                    false,
                ),
                (
                    ureq::Error::TlsRequired,
                    true,
                    Code::Unavailable,
                    "none",
                    false,
                ),
                (
                    ureq::Error::ConnectProxyFailed("refused".into()),
                    true,
                    Code::Unavailable,
                    "none",
                    false,
                ),
                (
                    ureq::Error::InvalidProxyUrl,
                    true,
                    Code::Unavailable,
                    "none",
                    false,
                ),
                (
                    ureq::Error::RequireHttpsOnly("x".into()),
                    true,
                    Code::Unavailable,
                    "none",
                    false,
                ),
                (
                    ureq::Error::BadUri("x".into()),
                    true,
                    Code::Internal,
                    "none",
                    false,
                ),
            ],
        );
    }

    #[test]
    fn ambiguous_api_paths_are_refused_before_anything_is_sent() {
        let c = client(member());
        for bad in [
            "/ssh-keys/../compute/vms/",
            "/ssh-keys/./x/",
            "/compute/%2e%2e/drive/",
            "/compute/%2E/x/",
            "/compute/vms%2F1/",
            "/compute/vms%5c1/",
            "/compute\\vms/",
            "/..",
            "//compute/vms/",
            "/registry//keys/",
            "https://api.hippius.com/api/registry//keys/",
            "https://api.hippius.com/api/ssh-keys/../drive/",
        ] {
            let error = c.url(&Request::get(bad)).unwrap_err();
            assert_eq!(error.code(), Code::Usage, "{bad}");
        }
        for fine in [
            "/compute/vms/a.b/",
            "/compute/vms/...x/",
            "/compute/x/?next=../y&u=https://z",
            "/compute/x/?q=%2e",
        ] {
            assert!(c.url(&Request::get(fine)).is_ok(), "{fine}");
        }
        // Another origin is refused whatever its path.
        assert_eq!(
            c.url(&Request::get("https://other.example/a/../b"))
                .unwrap_err()
                .code(),
            Code::Usage
        );
    }

    #[test]
    fn a_url_inside_a_query_does_not_make_a_path_a_url() {
        let c = client(Selection::Own);
        assert_eq!(
            c.url(&Request::get("/x/?next=https://y/z")).unwrap(),
            "https://api.hippius.com/api/x/?next=https://y/z"
        );
    }

    #[test]
    fn response_document_refuses_an_empty_or_null_body() {
        for body in [&b""[..], b"  \n", b"null"] {
            for (write, side_effects) in [(false, "none"), (true, "partial")] {
                let response = Response {
                    status: 200,
                    body: body.to_vec(),
                    write,
                    action: "read the thing".to_owned(),
                    request_id: None,
                    date: None,
                    token_spellings: Vec::new(),
                };
                let error = response.document().unwrap_err();
                assert_eq!(error.code(), Code::Internal);
                assert_eq!(error.to_json()["error"]["side_effects"], side_effects);
            }
        }
        let response = Response {
            status: 200,
            body: b"[]".to_vec(),
            write: false,
            action: "read the thing".to_owned(),
            request_id: None,
            date: None,
            token_spellings: Vec::new(),
        };
        assert_eq!(response.document().unwrap(), serde_json::json!([]));
    }

    #[test]
    fn response_json_keeps_large_numbers_and_rejects_non_json() {
        let ok = Response {
            status: 200,
            body: br#"{"paid_per_month":99000000000000000000}"#.to_vec(),
            write: false,
            action: "read the thing".to_owned(),
            request_id: None,
            date: None,
            token_spellings: Vec::new(),
        };
        assert_eq!(
            ok.json().unwrap().to_string(),
            r#"{"paid_per_month":99000000000000000000}"#
        );
        assert_eq!(ok.status(), 200);
        let empty = Response {
            status: 204,
            body: b" \n".to_vec(),
            write: false,
            action: "read the thing".to_owned(),
            request_id: None,
            date: None,
            token_spellings: Vec::new(),
        };
        assert_eq!(empty.json().unwrap(), Value::Null);
        let html = Response {
            status: 200,
            body: b"<html>".to_vec(),
            write: false,
            action: "read the thing".to_owned(),
            request_id: None,
            date: None,
            token_spellings: Vec::new(),
        };
        let error = html.json().unwrap_err();
        assert_eq!(error.code(), Code::Internal);
        assert_eq!(error.to_json()["error"]["side_effects"], "none");
        assert_eq!(error.http_status(), Some(200));
        assert_eq!(html.bytes(), b"<html>");
        let written = Response {
            status: 201,
            body: b"<html>".to_vec(),
            write: true,
            action: "read the thing".to_owned(),
            request_id: None,
            date: None,
            token_spellings: Vec::new(),
        };
        let error = written.json().unwrap_err();
        assert_eq!(error.to_json()["error"]["side_effects"], "partial");
        assert_eq!(error.to_json()["error"]["retryable"], false);
        let debug = format!("{written:?}");
        assert!(
            debug.contains("body_len: 6") && !debug.contains("html"),
            "{debug}"
        );
    }

    #[test]
    fn host_of_handles_ports_and_ipv6() {
        assert_eq!(host_of("api.hippius.com:443"), "api.hippius.com");
        assert_eq!(host_of("api.hippius.com"), "api.hippius.com");
        assert_eq!(host_of("[::1]:7"), "[::1]");
        assert_eq!(host_of("[::1"), "[::1");
    }
}
