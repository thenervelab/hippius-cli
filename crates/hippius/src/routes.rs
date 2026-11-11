//! Every endpoint of the console API that `hipc` calls, one constant per method and path,
//! with what the route does with the selector headers and what guards it needs.
//!
//! A route is spelled here once, with its API version in the path
//! (`/api/accounts/members/{member_id}/`), so a move to another version is a new constant
//! beside the old one, and the commands that move change the constant they name. The
//! version is never a client setting: a release sends the version of each route it was
//! tested against.
//!
//! The row decides. The client, for a curated command and for `hipc api` alike, asks
//! `row` for the row of a request's method and path first, and takes its policy: its
//! account class, how it takes a project and a billing account, whether its answer reveals a
//! secret, whether a read provisions, whether it cannot be undone, whether its 503 means
//! nothing was done, and what `hipc api` holds it back for. Only a path no row matches falls
//! back to the prefix tables in `account.rs` and `project.rs`, which fail closed for a route
//! they do not list either (tenet 12), and `hipc api` treats such a route as one `hipc` does
//! not know.
//!
//! The type carries the number of placeholders, so a request built from a route fills each
//! one, checked when it compiles: `Route<1>` takes one value.

/// Declares a service's routes as public constants and lists their endpoints in `ALL`, so
/// a route cannot be added without the agreement test seeing it.
macro_rules! routes {
    ($($(#[$doc:meta])* $name:ident: $n:literal = $route:expr;)*) => {
        $($(#[$doc])* pub const $name: crate::routes::Route<$n> = $route.declared();)*

        /// Every route of this service.
        pub(crate) const ALL: &[crate::routes::Endpoint] = &[$(*$name.endpoint()),*];
    };
}

/// Declares a server vocabulary, the values a field of an answer takes, as an enum with one
/// variant per value `hipc` acts on and `Other` for any other text, so a value the
/// server adds is not an error. `as_str` gives the server's spelling back, for a message.
macro_rules! vocabulary {
    (
        $(#[$doc:meta])*
        $name:ident { $($(#[$variant_doc:meta])* $variant:ident = $text:literal,)+ }
    ) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
        #[serde(from = "String")]
        pub enum $name {
            $($(#[$variant_doc])* $variant,)+
            /// A value `hipc` does not act on, as the API spells it.
            Other(String),
        }

        impl $name {
            /// The value as the API spells it.
            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $text,)+
                    Self::Other(text) => text,
                }
            }
        }

        impl From<String> for $name {
            fn from(text: String) -> Self {
                match text.as_str() {
                    $($text => Self::$variant,)+
                    _ => Self::Other(text),
                }
            }
        }
    };
}

pub mod account;
pub mod billing;
pub mod cdn;
pub mod compute;
pub mod databases;
pub mod drive;
pub mod hub;
pub mod kubernetes;
pub mod network;
pub mod operator;
pub mod runners;
pub mod s3;
pub mod support;

use std::fmt;

use serde::Serialize;

use crate::encode::percent_encode_keeping;
use crate::error::Code;
use crate::method::Method;
use crate::selector::{Selector, names_one_of};
use crate::url::with_trailing_slash;

/// The path every route of the console API sits under, before its version or service. A
/// path `hipc api` is given without it is sent under it.
pub const API_BASE: &str = "/api";

/// How a route treats `X-Hippius-Account`.
///
/// The variants are declared from the least to the most strict, and
/// [`classify`](crate::account::classify) keeps the greater of two. Sending the header is
/// stricter than leaving it off: a personal route ignores it, while a scoped route without it
/// acts on the caller's own account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RouteClass {
    /// Acts as the caller whatever the header says. The header is never sent.
    Personal,
    /// Acts in the account the header names.
    Scoped,
    /// Acts only in the caller's own account ([`Route::own_account`]): it refuses the
    /// header with `account_scope_unsupported`.
    OwnAccount,
}

/// What a route does with a selector header: `X-Hippius-Project` or `X-Hippius-Billing`.
///
/// The variants are declared from the least to the most strict, and
/// [`classify`](crate::account::classify) keeps the greater of two, for the reason
/// [`RouteClass`] gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Take {
    /// A personal route: it acts as the signed-in user, and the header is not sent.
    Skip,
    /// The route acts in the selected project or billing account.
    Send,
    /// The route refuses the header but names a project or a billing account in its path, so
    /// a stored selection is left off it, writes included; one from a flag or a variable is
    /// refused. Either selector: a route that names one of them takes the other neither.
    Named,
    /// The route refuses the header.
    Refuse,
}

/// A flag `hipc api` needs beyond `--yes` to send a route whose effect reaches the whole
/// account, or is one a curated command guards with its own flag, because an agent passes
/// `--yes` by habit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteFlag {
    /// Changes which peers of the account's network can reach each other.
    ChangeIsolation,
    /// Lets a CDN zone serve a bucket that may be private, as `hipc cdn zones create` does.
    Public,
    /// Lets a write that replaces a whole list remove the entries it leaves out, as `hipc
    /// compute firewall update` does.
    RemoveOthers,
}

impl RouteFlag {
    /// Every flag, in the order `hipc schema` lists `hipc api`'s guards. A test matches each
    /// variant to its place here with no catch-all arm, so a new variant does not compile
    /// until it is listed.
    pub const ALL: [Self; 3] = [Self::ChangeIsolation, Self::Public, Self::RemoveOthers];

    /// The flag as the caller passes it, such as `--change-isolation`: the one place each is
    /// spelled.
    pub const fn flag(self) -> &'static str {
        match self {
            Self::ChangeIsolation => "--change-isolation",
            Self::Public => "--public",
            Self::RemoveOthers => "--remove-others",
        }
    }

    /// The code `hipc api` refuses with when the flag is missing, the one whose flag this is
    /// (tenet 4): the curated commands' code for a flag they share.
    pub const fn code(self) -> Code {
        match self {
            Self::ChangeIsolation => Code::ChangeIsolationRequired,
            Self::Public => Code::PublicRequired,
            Self::RemoveOthers => Code::RemoveOthersRequired,
        }
    }
}

/// What `hipc api` needs, beyond `--yes` on a write, to send a route. A refusal carries why
/// and the one path to the route's effect; the message that names the request's method and
/// route is built when it refuses, so a write that borrows the refusal of another write
/// method of its route names its own method. `hipc api`'s help and `hipc schema` say each
/// guard from [`crate::api_routes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guard {
    /// Nothing beyond the usual confirmation of a write.
    None,
    /// Never sent, whatever the flags, a dry run included: refused as `usage` because `why`,
    /// with `hint`, which names the one path to the route's effect.
    Refused {
        /// Why, as the end of a sentence: "its answer holds the cluster's sealed kubeconfig".
        why: &'static str,
        /// The one path to the route's effect, as an imperative.
        hint: &'static str,
    },
    /// Refused as [`Guard::Refused`] is when the body sets `key`, a dry run included, and
    /// otherwise sent as usual. A body that is not a JSON object counts as setting it, so a
    /// shape `hipc` does not read fails closed.
    RefusedWhenSet {
        /// The top-level key of the body that the refusal is about.
        key: &'static str,
        /// Why, as [`Guard::Refused`] says it.
        why: &'static str,
        /// The one path to the route's effect, as [`Guard::Refused`] says it.
        hint: &'static str,
    },
    /// A write method the API documents as doing something other than the other write
    /// methods of its route: it needs nothing beyond `--yes`, and lends no guard to a method
    /// that has no row of its own, as [`Guard::None`] lends none.
    Distinct,
    /// Sent only with each of these flags, at least one.
    Needs(&'static [RouteFlag]),
    /// Sent only with each of these flags when the body sets this top-level key. A body
    /// that is not a JSON object counts as setting it, so a shape `hipc` does not read fails
    /// closed.
    NeedsWhenSet(&'static [RouteFlag], &'static str),
    /// Sent without these flags only when the body is a JSON object whose top-level key is
    /// exactly `false`. Anything else needs them, no body and a body without the key
    /// included, since the server may read those as a toggle; this fails closed.
    NeedsUnlessFalse(&'static [RouteFlag], &'static str),
}

impl Guard {
    /// Whether a write method of the same route that has no row of its own takes this guard:
    /// every guard but [`Guard::None`] and [`Guard::Distinct`].
    pub fn lends(self) -> bool {
        !matches!(self, Self::None | Self::Distinct)
    }

    /// The guard a write method `to` with no row of its own takes from this guard on the
    /// write row of method `from` of the same route, or `None` when it takes none: a read
    /// neither lends nor borrows, and a guard that does not [`lend`](Guard::lends) lends
    /// nothing. A `PUT` replaces the whole resource, so a field its body leaves out may be
    /// reset rather than kept: a flag a guard needs when the body sets a key, for what that
    /// change does, a borrowing `PUT` needs whatever its body sets. A refusal on a key stays on
    /// the key: it is about what the body carries, such as an app's secret values, which a body
    /// without the key does not.
    pub fn lent_to(self, from: Method, to: Method) -> Option<Self> {
        if from.is_read() || to.is_read() || !self.lends() {
            return None;
        }
        Some(match (self, to) {
            (Self::NeedsWhenSet(flags, _), Method::Put) => Self::Needs(flags),
            (guard, _) => guard,
        })
    }
}

/// One endpoint: a method, a path template under [`API_BASE`], and its policy. A
/// [`Route`] holds one, with the number of its placeholders in its type.
///
/// A placeholder is `{name}`, one path segment, or `{name..}`, one or more segments, as a
/// Hub repository name (`namespace/app`) is. The name says what fills it, as the command's
/// argument names it where it has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is a separate fact about the route, kept apart as the policy tables keep \
              them, and set by a named builder method"
)]
pub struct Endpoint {
    method: Method,
    template: &'static str,

    // The selector headers (tenet 12). `None` only while a constant is being built: the
    // `routes!` table refuses a row that never says its class.
    account: Option<RouteClass>,
    /// Acts in the selected account beside `X-Hippius-Billing`, though it refuses the account
    /// header alone: the recurring card routes.
    account_beside_billing: bool,
    project: Take,
    billing: Take,

    // What the answer or the effect is.
    reveals: bool,
    provisions: bool,
    permanent: bool,
    /// A 503 with `Retry-After` means nothing was done.
    undone_on_503: bool,
    /// How the server pages the list it answers; a command takes the page flags on a paged
    /// route.
    paging: Option<Paging>,
    guard: Guard,
}

/// How the server pages the list a route answers (tenet 6): the query that asks for a page,
/// and the shape of the answer, which `hipc`'s page loop reads by it. A route that changes
/// how it pages changes its row, and a list read the old way is never taken for complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paging {
    /// Django REST framework's page numbers: `page` and `page_size`, answered `{count, next,
    /// previous, results}`; the next page is the number in the `next` link.
    Numbered,
    /// Page numbers without a `next` link: `page` and `page_size`, answered `{page,
    /// page_size, total, <items>}` with the items under `items`, as the Hub model index
    /// answers; the next page follows while `total` says items remain.
    Totalled {
        /// The key that holds the items, such as `results` or `artifacts`.
        items: &'static str,
    },
    /// Page numbers with neither a `next` link nor a total: `page` and `page_size`, answered
    /// `{page, page_size, <items>}`, as the Hub usage by repository answers; the next page
    /// follows while a page is full, so the list ends at the first page shorter than
    /// `page_size`, an empty one included.
    UntilShort {
        /// The key that holds the items, such as `repositories`.
        items: &'static str,
    },
    /// A cursor: `before`, answered `{results, next_before}`; the server fixes the page size.
    Cursor,
    /// S3's `ListObjectsV2`: `max_keys` and `continuation_token`, answered `{Contents,
    /// NextContinuationToken, IsTruncated}`.
    Continuation,
}

impl Paging {
    /// Whether the pages are numbered, so the first is page `1` and an answer may name the
    /// page it is; the others go on from a token the previous page gave.
    pub const fn numbered(self) -> bool {
        matches!(
            self,
            Self::Numbered | Self::Totalled { .. } | Self::UntilShort { .. }
        )
    }

    /// The query parameter that names the page to read: its number or its token.
    pub const fn token_query(self) -> &'static str {
        match self {
            Self::Numbered | Self::Totalled { .. } | Self::UntilShort { .. } => "page",
            Self::Cursor => "before",
            Self::Continuation => "continuation_token",
        }
    }

    /// The query parameter of the page size, or `None` where the server fixes it.
    pub const fn size_query(self) -> Option<&'static str> {
        match self {
            Self::Numbered | Self::Totalled { .. } | Self::UntilShort { .. } => Some("page_size"),
            Self::Cursor => None,
            Self::Continuation => Some("max_keys"),
        }
    }
}

impl Endpoint {
    /// The method.
    pub fn method(&self) -> Method {
        self.method
    }

    /// The path with its placeholders, such as `/api/accounts/members/{member_id}/`.
    pub const fn template(&self) -> &'static str {
        self.template
    }

    /// Whether the answer holds a secret, so `hipc api` needs `--reveal` to print it
    /// (tenet 9).
    pub fn reveals(&self) -> bool {
        self.reveals
    }

    /// Whether a read may create something, so `hipc api` confirms it as a write.
    pub fn provisions(&self) -> bool {
        self.provisions
    }

    /// How the route treats `X-Hippius-Account`.
    pub(crate) fn account(&self) -> RouteClass {
        self.account
            .expect("`routes!` refuses a row that does not say its class")
    }

    pub(crate) fn account_beside_billing(&self) -> bool {
        self.account_beside_billing
    }

    pub(crate) fn project(&self) -> Take {
        self.project
    }

    pub(crate) fn billing(&self) -> Take {
        self.billing
    }

    pub(crate) fn permanent(&self) -> bool {
        self.permanent
    }

    pub(crate) fn undone_on_503(&self) -> bool {
        self.undone_on_503
    }

    /// What `hipc api` needs, beyond `--yes` on a write, to send the route.
    pub fn guard(&self) -> Guard {
        self.guard
    }

    /// The segments of the template that are no placeholder, which rank two rows that match
    /// one path: `/api/databases/refusals/` before `/api/databases/{database_id}/`.
    fn literal_segments(&self) -> usize {
        self.template
            .split('/')
            .filter(|segment| !segment.is_empty() && Placeholder::in_segment(segment).is_none())
            .count()
    }

    /// The template with every placeholder filled by `fill`, which gets the placeholder's
    /// name, without its braces, and its kind.
    fn filled(&self, mut fill: impl FnMut(&str, Placeholder) -> String) -> String {
        let mut path = String::with_capacity(self.template.len());
        let mut rest = self.template;
        while let Some((before, after)) = rest.split_once('{') {
            let (name, after) = after
                .split_once('}')
                .expect("a template closes each placeholder; `Route::new` checks it");
            path.push_str(before);
            let (name, kind) = Placeholder::parse(name);
            path.push_str(&fill(name, kind));
            rest = after;
        }
        path.push_str(rest);
        path
    }

    /// How the server pages the list this route answers, or `None` for a route it does not
    /// page.
    pub fn paging(&self) -> Option<Paging> {
        self.paging
    }

    /// Whether `path` (no query) is this template with a non-empty value in each
    /// placeholder: the match the route lookup makes for every request the client
    /// classifies, and the end-to-end tests' check of the route a recorded request reached.
    /// It splits both into segments on each call, a few small allocations, which is nothing
    /// beside the request it classifies.
    pub fn matches(&self, path: &str) -> bool {
        let segments: Vec<&str> = path.split('/').collect();
        let parts: Vec<&str> = self.template.split('/').collect();
        let mut at = 0;
        for (i, part) in parts.iter().enumerate() {
            match Placeholder::in_segment(part) {
                Some(Placeholder::Segments) => {
                    // One or more segments: as many as the parts after this one leave.
                    let after = parts.len() - i - 1;
                    let Some(end) = segments.len().checked_sub(after).filter(|&end| end > at)
                    else {
                        return false;
                    };
                    if !fits(Placeholder::name(part), &segments[at..end]) {
                        return false;
                    }
                    at = end;
                }
                Some(Placeholder::Segment) => {
                    let Some(value) = segments.get(at) else {
                        return false;
                    };
                    if !fits(Placeholder::name(part), &[value]) {
                        return false;
                    }
                    at += 1;
                }
                None => {
                    if segments.get(at) != Some(part) {
                        return false;
                    }
                    at += 1;
                }
            }
        }
        at == segments.len()
    }

    /// The template with each placeholder filled by `sample`, for a test that asks the
    /// policy tables about the route.
    #[cfg(test)]
    pub(crate) fn sample(&self, sample: impl Fn(&str) -> String) -> String {
        self.filled(|name, _kind| sample(name))
    }
}

/// What one placeholder of a template stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placeholder {
    /// `{name}`: one segment.
    Segment,
    /// `{name..}`: one or more segments.
    Segments,
}

impl Placeholder {
    /// The name and the kind of the placeholder whose text between the braces is `inner`.
    fn parse(inner: &str) -> (&str, Self) {
        inner
            .strip_suffix("..")
            .map_or((inner, Self::Segment), |name| (name, Self::Segments))
    }

    /// The kind of placeholder a whole template segment is, or `None` for a literal one.
    fn in_segment(segment: &str) -> Option<Self> {
        Self::inner(segment).map(|inner| Self::parse(inner).1)
    }

    /// The name of the placeholder a whole template segment is, empty for a literal one.
    fn name(segment: &str) -> &str {
        Self::inner(segment).map_or("", |inner| Self::parse(inner).0)
    }

    fn inner(segment: &str) -> Option<&str> {
        segment
            .strip_prefix('{')
            .and_then(|inner| inner.strip_suffix('}'))
    }
}

/// The body of a POST that carries nothing, `{}`, as the console sends it: a Kubernetes node
/// removal, an operation's cancel and resume, a WAF version's rollback, and an auto-reload
/// card setup outside a billing account's page.
#[derive(Debug, Serialize)]
#[expect(
    clippy::empty_structs_with_brackets,
    reason = "a struct with braces is written as `{}`, the empty body the routes take; a unit \
              struct would be `null`"
)]
pub struct EmptyBody {}

/// A field of a write body that is left out, cleared, or set. A field left out keeps the
/// server's value, so it is not sent (`#[serde(skip_serializing_if = "Change::is_keep")]`), and
/// a cleared one is sent as `null`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Change<T> {
    /// Not sent: the server keeps what it holds.
    #[default]
    Keep,
    /// Sent as `null`, which the route reads as none, or as its default.
    Clear,
    /// Sent as the value.
    Set(T),
}

impl<T> Change<T> {
    /// Whether the field is left out of the body.
    pub fn is_keep(&self) -> bool {
        matches!(self, Self::Keep)
    }
}

impl<T: Serialize> Serialize for Change<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            // A body skips a field it keeps, so `Keep` reaches here only in a body that sends
            // every field, where `null` is what keeping nothing means.
            Self::Keep | Self::Clear => serializer.serialize_none(),
            Self::Set(value) => value.serialize(serializer),
        }
    }
}

/// The body of the Drive and the S3 subscription `PATCH` that keeps the plan and changes one
/// setting: how renewals are paid, or the cap on storage billed past the quota.
#[derive(Debug, Default, Serialize)]
pub struct SettingChangeBody<'a> {
    /// How each later renewal is paid: `credits` or `card`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub funding: Option<&'a str>,
    /// The cap on bytes stored past the quota; cleared, it is the default cap.
    #[serde(skip_serializing_if = "Change::is_keep")]
    pub storage_cap_bytes: Change<u64>,
}

/// Whether `value`, the path segments a placeholder named `name` takes (one for `{name}`),
/// fits it: each segment non-empty, and the shape the name holds the value to, where the
/// server's router or the API's ids hold it to one. This is the one place a name decides a
/// shape, so a template that names its placeholder anything else takes any value.
///
/// - `database_id` and `invoice_id`: digits, as the server's router takes them, so
///   `/api/databases/metrics/` is not one database's route but a personal one the tables
///   decide.
/// - `project_id` and `billing_account_id`: a project or billing account id, or `default`,
///   so a row that names one in its path (which takes no selector header) is not reached by
///   another word, such as `/api/accounts/projects/bulk/`, which the tables then decide and
///   refuse with a selection, as they decide every path no row matches.
/// - `model`: two segments, `namespace/repository`, as `hipc hub models list` prints a model
///   and `hipc hub models get` takes it, so `/api/models/acme/llama/v1/` is a version of the
///   model `acme/llama`, not a model of three segments.
fn fits(name: &str, value: &[&str]) -> bool {
    let one = |shape: fn(&str) -> bool| matches!(value, [segment] if shape(segment));
    let digits = |text: &str| text.bytes().all(|b| b.is_ascii_digit());
    !value.iter().any(|segment| segment.is_empty())
        && match name {
            "database_id" | "invoice_id" => one(digits),
            "project_id" => one(|text| names_one_of(Selector::Project, text)),
            "billing_account_id" => one(|text| names_one_of(Selector::Billing, text)),
            "model" => value.len() == 2,
            _ => true,
        }
}

/// An endpoint whose template has `N` placeholders, so a request built from it fills each.
#[derive(Debug, Clone, Copy)]
pub struct Route<const N: usize> {
    endpoint: Endpoint,
}

impl<const N: usize> Route<N> {
    /// A route that refuses every selector header and needs nothing beyond `--yes`, which
    /// the methods below widen.
    ///
    /// # Panics
    ///
    /// When `template` does not start with [`API_BASE`] and `/`, does not end with `/`, or
    /// does not hold `N` placeholders, each a whole segment. A route is a constant, so the
    /// panic stops the build.
    const fn new(method: Method, template: &'static str) -> Self {
        assert!(
            placeholders(template) == N,
            "a route's type counts the placeholders of its template"
        );
        assert!(starts_with_base(template), "a route starts with `/api/`");
        Self {
            endpoint: Endpoint {
                method,
                template,
                account: None,
                account_beside_billing: false,
                project: Take::Refuse,
                billing: Take::Refuse,
                reveals: false,
                provisions: false,
                permanent: false,
                undone_on_503: false,
                paging: None,
                guard: Guard::None,
            },
        }
    }

    pub(crate) const fn get(template: &'static str) -> Self {
        Self::new(Method::Get, template)
    }

    pub(crate) const fn post(template: &'static str) -> Self {
        Self::new(Method::Post, template)
    }

    pub(crate) const fn put(template: &'static str) -> Self {
        Self::new(Method::Put, template)
    }

    pub(crate) const fn patch(template: &'static str) -> Self {
        Self::new(Method::Patch, template)
    }

    pub(crate) const fn delete(template: &'static str) -> Self {
        Self::new(Method::Delete, template)
    }

    /// Acts in the selected account.
    pub(crate) const fn scoped(mut self) -> Self {
        self.endpoint.account = Some(RouteClass::Scoped);
        self
    }

    /// Refuses a membership: it acts only in the caller's own account.
    pub(crate) const fn own_account(mut self) -> Self {
        self.endpoint.account = Some(RouteClass::OwnAccount);
        self
    }

    /// Acts as the signed-in user in every account, project, and billing account, so no
    /// selector header goes on it.
    pub(crate) const fn personal(mut self) -> Self {
        self.endpoint.account = Some(RouteClass::Personal);
        self.endpoint.project = Take::Skip;
        self.endpoint.billing = Take::Skip;
        self
    }

    /// Acts in the selected account beside `X-Hippius-Billing` only.
    pub(crate) const fn beside_billing(mut self) -> Self {
        self.endpoint.account_beside_billing = true;
        self
    }

    /// Acts in the selected project.
    pub(crate) const fn project(mut self) -> Self {
        self.endpoint.project = Take::Send;
        self
    }

    /// Acts in the selected billing account.
    pub(crate) const fn billing(mut self) -> Self {
        self.endpoint.billing = Take::Send;
        self
    }

    /// Names the project or billing account it acts on in its path.
    pub(crate) const fn names_one(mut self) -> Self {
        self.endpoint.project = Take::Named;
        self.endpoint.billing = Take::Named;
        self
    }

    /// Its answer holds a secret (tenet 9).
    pub(crate) const fn reveals(mut self) -> Self {
        self.endpoint.reveals = true;
        self
    }

    /// A read that may create something.
    pub(crate) const fn provisions(mut self) -> Self {
        self.endpoint.provisions = true;
        self
    }

    /// Its effect cannot be undone.
    pub(crate) const fn permanent(mut self) -> Self {
        self.endpoint.permanent = true;
        self
    }

    /// A 503 with `Retry-After` means nothing was done.
    pub(crate) const fn undone_on_503(mut self) -> Self {
        self.endpoint.undone_on_503 = true;
        self
    }

    /// The server pages its list as `paging` says.
    pub(crate) const fn paged(mut self, paging: Paging) -> Self {
        self.endpoint.paging = Some(paging);
        self
    }

    /// What `hipc api` holds it back for.
    ///
    /// # Panics
    ///
    /// When a [`Guard::Needs`] names no flag: a write that needs nothing is
    /// [`Guard::Distinct`] or [`Guard::None`]. A route is a constant, so the panic stops the
    /// build.
    pub(crate) const fn guard(mut self, guard: Guard) -> Self {
        if let Guard::Needs(flags) = guard {
            assert!(!flags.is_empty(), "a `Guard::Needs` names a flag");
        }
        self.endpoint.guard = guard;
        self
    }

    /// This route, checked to say how it treats the account header: a row that leaves it out
    /// stops the build, rather than fall back to a class nobody chose.
    const fn declared(self) -> Self {
        assert!(
            self.endpoint.account.is_some(),
            "a route says whether it is scoped, personal, or for the own account only"
        );
        self
    }

    /// The endpoint, without the count of its placeholders.
    pub const fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// The method.
    pub fn method(&self) -> Method {
        self.endpoint.method
    }

    /// The path with `values` in its placeholders, in order, each percent-encoded as
    /// RFC 3986 (section 3.3) encodes a path segment: the characters a segment may hold as
    /// they are (`SEGMENT_KEEPS`) stay, a `{name..}` placeholder keeps the `/` between its
    /// segments, and a `{name}` one encodes it, so no value reaches another route. A Hub
    /// digest such as `sha256:0a1b` goes as it is.
    pub fn path(&self, values: [&dyn fmt::Display; N]) -> String {
        let mut values = values.into_iter();
        self.endpoint.filled(|_name, kind| {
            let value = values
                .next()
                .expect("the type holds one value per placeholder")
                .to_string();
            match kind {
                Placeholder::Segment => percent_encode_keeping(&value, SEGMENT_KEEPS),
                Placeholder::Segments => percent_encode_keeping(&value, SEGMENTS_KEEP),
            }
        })
    }
}

/// The characters beside the unreserved ones that a path segment holds as they are: RFC 3986
/// `pchar`'s sub-delims, `:` and `@`, without `;`, which the client refuses in a path because a
/// front end may read it as the end of the segment.
const SEGMENT_KEEPS: &[u8] = b"!$&'()*+,=:@";

/// [`SEGMENT_KEEPS`], and the `/` between the segments of a `{name..}` value.
const SEGMENTS_KEEP: &[u8] = b"!$&'()*+,=:@/";

/// The number of placeholders in `template`, or `usize::MAX` when one is not a whole
/// segment, `{`, a name, and `}` between two `/`.
const fn placeholders(template: &str) -> usize {
    let bytes = template.as_bytes();
    let mut count = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if i == 0 || bytes[i - 1] != b'/' {
                return usize::MAX;
            }
            while i < bytes.len() && bytes[i] != b'}' {
                i += 1;
            }
            if i + 1 >= bytes.len() || bytes[i + 1] != b'/' {
                return usize::MAX;
            }
            count += 1;
        }
        i += 1;
    }
    count
}

/// Whether `template` is under [`API_BASE`] and ends with `/`.
const fn starts_with_base(template: &str) -> bool {
    let (base, path) = (API_BASE.as_bytes(), template.as_bytes());
    if path.len() <= base.len() + 1 || path[base.len()] != b'/' || path[path.len() - 1] != b'/' {
        return false;
    }
    let mut i = 0;
    while i < base.len() {
        if path[i] != base[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// The row `method` to `form` reaches: one form of a request path (its query removed, a
/// format suffix kept or removed, in one letter case), matched with its trailing `/` added,
/// since a server or a proxy that adds the slash delivers the slashless path to the same
/// route. The path picks the route ([`route_at`]), and then the method picks its row there; a
/// method the route has no row for reaches none, not a row of another template. A `HEAD`
/// reaches the `GET` row: Django REST framework runs a `HEAD` through the `GET` handler.
pub(crate) fn row(method: Method, form: &str) -> Option<&'static Endpoint> {
    let method = if method == Method::Head {
        Method::Get
    } else {
        method
    };
    route_at(&with_trailing_slash(form)).find(|endpoint| endpoint.method == method)
}

/// The rows of the route `form`, a path with its trailing `/`, reaches, whatever their
/// method. As the server's router does, the path alone picks the route: the template with
/// the most literal segments among those that match (`/api/databases/refusals/` before
/// `/api/databases/{database_id}/`), the first in table order where two tie. No two
/// templates of the table tie on a path today: `routes::tests` checks that every row is the
/// row its own path reaches. A write method with no row there borrows a guard only from these
/// rows ([`Guard::lent_to`]).
pub(crate) fn route_at(form: &str) -> impl Iterator<Item = &'static Endpoint> {
    let template = matching(form)
        .min_by_key(|endpoint| std::cmp::Reverse(endpoint.literal_segments()))
        .map(Endpoint::template);
    all().filter(move |endpoint| Some(endpoint.template) == template)
}

/// Every row whose template matches `form`, a path with its trailing `/`, whatever its
/// method, in table order.
pub(crate) fn matching(form: &str) -> impl Iterator<Item = &'static Endpoint> {
    all().filter(move |endpoint| endpoint.matches(form))
}

/// Every endpoint of every service: what the client consults first for a request
/// (`row`), what `hipc api`'s help lists of the routes that reveal a secret or provision,
/// and what the tests in both crates check `hipc schema` and the end-to-end requests against.
pub fn all() -> impl Iterator<Item = &'static Endpoint> {
    [
        account::ALL,
        billing::ALL,
        cdn::ALL,
        compute::ALL,
        databases::ALL,
        drive::ALL,
        hub::ALL,
        kubernetes::ALL,
        network::ALL,
        operator::ALL,
        runners::ALL,
        s3::ALL,
        support::ALL,
    ]
    .into_iter()
    .flatten()
}

#[cfg(test)]
mod tests;
