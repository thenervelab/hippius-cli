//! Every endpoint of the console API that `hipc` calls, one constant per method and path,
//! with what the route does with the selector headers and what guards it needs.
//!
//! A route is spelled here once, with its API version in the path
//! (`/api/accounts/members/{member_id}/`), so a move to another version is a new constant
//! beside the old one, and the commands that move change the constant they name. The
//! version is never a client setting: a release sends the version of each route it was
//! tested against.
//!
//! The policy of a row (its account class, how it takes a project and a billing account,
//! whether its answer reveals a secret, whether a read provisions, whether it cannot be
//! undone, what `hipc api` holds it back for) restates what the tables in `account.rs` and
//! `project.rs` decide for its path. Those tables still decide; a test holds every row
//! against them, so a row that disagrees, or a route that has no row, fails the build's
//! tests rather than change a header.
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

pub mod account;
pub mod billing;
pub mod cdn;
pub mod compute;
pub mod databases;
pub mod drive;
pub mod hub;
pub mod kubernetes;
pub mod network;
pub mod runners;
pub mod s3;
pub mod support;

use std::fmt;

use crate::encode::percent_encode_keeping;
use crate::error::Code;
use crate::method::Method;

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
    /// Refuses the header with `account_scope_unsupported`.
    Unsupported,
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

/// What `hipc api` needs, beyond `--yes` on a write, to send a route: the kind of its row in
/// `account.rs`'s route rules, without the message, which stays there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Guard {
    /// Nothing beyond the usual confirmation of a write.
    None,
    /// Never sent.
    Refused,
    /// Never sent with this top-level key in the body.
    RefusedWhenSet(&'static str),
    /// Sent only with each of these flags.
    Needs(&'static [RouteFlag]),
    /// Sent only with each of these flags when the body sets this top-level key.
    NeedsWhenSet(&'static [RouteFlag], &'static str),
    /// Sent without these flags only when this top-level key is `false`.
    NeedsUnlessFalse(&'static [RouteFlag], &'static str),
}

/// One endpoint: a method, a path template under [`API_BASE`], and its policy. A
/// [`Route`] holds one, with the number of its placeholders in its type.
///
/// A placeholder is `{name}`, one path segment, or `{name..}`, one or more segments, as a
/// Hub repository name (`namespace/app`) is. The name says what fills it, as the command's
/// argument names it where it has one.
#[derive(Debug, Clone, Copy)]
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
    /// The server pages the list it answers, so a command takes the page flags on it.
    paged: bool,
    guard: Guard,
}

impl Endpoint {
    /// The method.
    pub fn method(&self) -> Method {
        self.method
    }

    /// The path with its placeholders, such as `/api/accounts/members/{member_id}/`.
    pub fn template(&self) -> &'static str {
        self.template
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

    /// Whether the server pages the list this route answers, so a command that reads it
    /// takes the page flags.
    pub fn paged(&self) -> bool {
        self.paged
    }

    /// Whether `path` (no query) is this template with a non-empty value in each
    /// placeholder, for a test that finds the route a recorded request reached.
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
                    if segments[at..end].iter().any(|segment| segment.is_empty()) {
                        return false;
                    }
                    at = end;
                }
                Some(Placeholder::Segment) => {
                    if segments.get(at).is_none_or(|segment| segment.is_empty()) {
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
        segment
            .strip_prefix('{')
            .and_then(|inner| inner.strip_suffix('}'))
            .map(|inner| Self::parse(inner).1)
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
                paged: false,
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
        self.endpoint.account = Some(RouteClass::Unsupported);
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

    /// The server pages its list.
    pub(crate) const fn paged(mut self) -> Self {
        self.endpoint.paged = true;
        self
    }

    /// What `hipc api` holds it back for.
    pub(crate) const fn guard(mut self, guard: Guard) -> Self {
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
    /// they are ([`SEGMENT_KEEPS`]) stay, a `{name..}` placeholder keeps the `/` between its
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

/// Every endpoint of every service. Only tests read the whole table: the agreement tests in
/// both crates and the check that `hipc schema` names only routes listed here.
#[doc(hidden)]
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
        runners::ALL,
        s3::ALL,
        support::ALL,
    ]
    .into_iter()
    .flatten()
}

#[cfg(test)]
mod tests;
