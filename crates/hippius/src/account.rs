//! Which account a request acts in, and whether `X-Hippius-Account` goes on it
//! (`TENETS.md` tenet 12).
//!
//! The server sorts routes into three classes: routes that act in the selected account,
//! personal routes that ignore the header, and routes that refuse it. A route a command
//! sends has a row in `routes`, which says its class; the prefixes below decide only a path
//! no row matches. They are copied from the console web client, which follows the backend's
//! route classification (the console's header rules).
//!
//! A route neither lists is treated as refusing the header. With a membership
//! selected, the CLI refuses such a route locally instead of sending a request that would
//! either be refused or, without the header, act on the caller's own account.

use std::borrow::Cow;
use std::cmp::Reverse;

use crate::error::Result;
use crate::method::Method;
use crate::phrase::quote;
use crate::routes::{self, Endpoint, Guard, RouteClass, RouteFlag};
use crate::selector::Selection;
use crate::url::{with_trailing_slash, without_query, without_query_values};

/// Prefixes of the routes that act in the selected account.
///
/// The hAlpha routes (`/api/billing/native/quote/`, `buy/`, `purchases/`) are left out on
/// purpose, here, among the personal routes, and among the billing-account routes: they act
/// on the signed-in user's own Hippius address, which the quote names, and the console sends
/// them with no account, project, or billing header. Unlisted, they refuse a membership and a
/// project or billing account from a flag or a variable, rather than spend the caller's own
/// hAlpha while another account looks selected.
const SCOPED_PREFIXES: &[&str] = &[
    "/api/accounts/",
    "/api/compute/",
    "/api/databases/",
    "/api/kubernetes/",
    "/api/runners/",
    "/api/cdn/",
    "/api/network/",
    "/api/objectstore/",
    "/api/s3/",
    "/api/billing/credits/balance/",
    "/api/billing/invoices/",
    "/api/billing/profile/",
    "/api/billing/stripe/create-checkout-session/",
    "/api/billing/btcpay/create-invoice/",
    "/api/billing/auto-reload/",
];

/// Prefixes of personal routes, checked before the scoped list because several sit inside
/// a scoped prefix.
const PERSONAL_PREFIXES: &[&str] = &[
    "/api/accounts/project-transfers/",
    "/api/compute/edge/",
    "/api/kubernetes/agent/",
    "/api/runners/agent/",
    "/api/cdn/node/",
    "/api/network/infra/",
    "/api/objectstore/tokens/",
    "/api/ssh-keys/",
    // The console never sends the header to support routes, and names support among the
    // personal routes that "keep acting as the signed-in user" in its header rules. Only the
    // customer ticket routes are listed: that is what `hipc support tickets` sends to, and the chat
    // routes and the other routes under `/api/support/`, which the customer console does not
    // call, stay refused.
    "/api/support/tickets/",
    // Swagger: "List my referral codes", "the acting user's single on-chain code", and the
    // analytics of "the current user's referral code", cached per calling user. The console
    // sends them without the header. A referral code is the user's, never an account's, so
    // a membership must not reach another user's code: the header never goes on them.
    "/api/referral-codes/",
];

/// Exact personal paths. The Drive and Hub catalogs are not in the console's lists, but
/// they are public and the same for every account, so with a membership selected they are
/// sent without the header instead of refused. The signed-in user, their notification
/// settings, and their marketing consent are not in those lists either: the console sends the
/// header only under its scoped prefixes, and its docs name "settings, the session" among the
/// personal routes that act as the signed-in user. Swagger calls the consent "the
/// authenticated customer's".
pub(crate) const PERSONAL_PATHS: &[&str] = &[
    "/api/accounts/",
    "/api/drive/plans/",
    "/api/registry/plans/",
    "/api/accounts/invites/accept/",
    "/api/accounts/invites/preview/",
    "/api/databases/metrics/",
    "/api/s3/plans/accounts/",
    "/api/users/me/",
    "/api/notifications/settings/",
    "/api/referral/analytics/",
    "/api/notifications/marketing-consent/",
];

/// Whether `method` to `path` (no query), answered 503 with `Retry-After`, did nothing: its
/// row says so ([`Endpoint::undone_on_503`]). Swagger documents the Drive and S3 subscribe
/// 503 as "Chain unreachable; retry after Retry-After seconds": the purchase never reached
/// the chain. Only those rows, only a 503, and only with the header; every other 5xx to a
/// write stays `partial` (TENETS 4), a path no row matches included. A path without its
/// trailing `/` is the same route. A format suffix or another letter case is not read here
/// ([`classify`]): keeping such a write `partial` is the stricter answer.
pub(crate) fn not_done_on_503(method: Method, path: &str) -> bool {
    routes::row(method, without_query(path)).is_some_and(Endpoint::undone_on_503)
}

/// The requests with no row whose answer holds a secret, each a method and a path pattern in
/// which `{n}` is one segment of ASCII digits and any other `{name}` one non-empty segment. A
/// row says it of its own route ([`Endpoint::reveals`]); this table holds the routes `hipc`
/// reaches only through `hipc api`, and a test refuses an entry a row covers.
///
/// Most answer with a one-time secret: a credential or a setup key the server shows once.
/// Others answer with a secret that lasts: the account's master token, a short-lived shell or
/// sign-in ticket, a payment `client_secret`, or a signed URL that reads data with no other
/// credential until it expires. A route whose answer is unknown but may carry a session or an
/// invitation is listed too, which fails closed: the cost is one `--reveal`. A `HEAD` answer
/// has no body to print, so no `HEAD` needs `--reveal`.
const SECRET_ROUTES: &[(Method, &str)] = &[
    (Method::Get, "/api/databases/{n}/backup-token/"),
    (Method::Post, "/api/databases/{n}/backup-token/"),
    (Method::Post, "/api/network/peers/enroll/"),
    (Method::Post, "/api/compute/vms/{vm_id}/terminal-ticket/"),
    (Method::Post, "/api/infrastructure/compute-miners/prepare/"),
    (Method::Post, "/api/chat/visitors/"),
    (Method::Post, "/api/chat/transfer/"),
    (Method::Post, "/api/chat/transfer/redeem/"),
    (Method::Post, "/api/billing/transactions/"),
    (Method::Post, "/api/objectstore/tokens/auth/"),
    (Method::Get, "/api/auth/token/"),
    (Method::Post, "/api/auth/exchange/"),
    (Method::Post, "/api/auth/verify/"),
    (
        Method::Get,
        "/api/chat/conversations/{conversation_id}/attachments/{attachment_id}/",
    ),
    (
        Method::Get,
        "/api/support/chat/tickets/{ticket_id}/attachments/{attachment_id}/",
    ),
    (Method::Post, "/api/auth/sso/ticket/"),
    (Method::Post, "/api/auth/session/"),
    (Method::Post, "/api/auth/sign-in-methods/step-up/verify/"),
    (Method::Post, "/api/network/infra/enroll/"),
    (Method::Post, "/api/chat/workspaces/{workspace_id}/invites/"),
];

/// Every request whose answer holds a secret, as a method and a path template: each row that
/// reveals one, then the routes with no row that do. `hipc api --reveal`'s help lists them.
pub fn secret_routes() -> impl Iterator<Item = (Method, &'static str)> {
    routes::all()
        .filter(|row| row.reveals())
        .map(|row| (row.method(), row.template()))
        .chain(SECRET_ROUTES.iter().copied())
}

/// Whether `method` to `path` (under `/api/`, no query) answers with a secret. `hipc api`
/// needs `--reveal` for these, as the curated commands that reach some of them do. A `HEAD`
/// never does: its answer has no body to print.
///
/// A path without its trailing `/` matches as if it had one: a server or a proxy that adds
/// the slash delivers `/api/objectstore/master-tokens` to the same route, and the secret
/// must not print because the caller left the slash off. The header tables match the same
/// way, so `/api/ssh-keys` stays personal and `/api/billing/credits/balance` scoped, as
/// with the slash, and a `--all` page reached by a link that adds the slash
/// ([`same_route`]) gets the headers the first page got.
///
/// A path with a format suffix issues a secret when either of its forms does
/// ([`classify`]), and a path in another letter case when its lowercase form does
/// ([`folded`]).
pub(crate) fn returns_secret(method: Method, path: &str) -> bool {
    method != Method::Head && decided(method, path, Endpoint::reveals, SECRET_ROUTES)
}

/// Whether `method` to `path` (under `/api/`, no query) may create something on the
/// server although its method reads, as its row says ([`Endpoint::provisions`]), matched as
/// [`returns_secret`] matches, slashless and suffixed forms included. A path no row matches
/// does not.
///
/// Swagger: the first `GET /api/network/me/` for an account creates its network, and a
/// `HEAD` reaches the `GET` row ([`routes::row`]), so a `HEAD` counts too.
pub(crate) fn provisions(method: Method, path: &str) -> bool {
    decided(method, path, Endpoint::provisions, &[])
}

/// Whether `method` to `path` (under `/api/`, no query) cannot be undone, as its row says
/// ([`Endpoint::permanent`]), matched as [`returns_secret`] matches. `--yes` with the warning
/// is the whole confirmation (tenet 5): no second flag stands beside it.
pub(crate) fn permanent(method: Method, path: &str) -> bool {
    decided(method, path, Endpoint::permanent, &[])
}

/// What the row of each form of `path` says by `from_row`, or for a form no row matches,
/// whether `table` lists it; true when any form's answer is (for the forms, see
/// [`returns_secret`]).
fn decided(
    method: Method,
    path: &str,
    from_row: fn(&Endpoint) -> bool,
    table: &[(Method, &str)],
) -> bool {
    let listed = |form: &str| {
        let form = with_trailing_slash(form);
        table
            .iter()
            .any(|&(route, pattern)| route == method && matches_pattern(pattern, &form))
    };
    folded(path).iter().any(|path| {
        classify(path, false, |form| {
            routes::row(method, form)
                .map(from_row)
                .or_else(|| listed(form).then_some(true))
        })
    })
}

/// The row `method` to `path` reaches in any of its forms ([`returns_secret`]), or `None`
/// when no form has one: a route `hipc` does not know. The path as written wins, then its
/// plain form, then the lowercase ones.
pub(crate) fn row_of(method: Method, path: &str) -> Option<&'static Endpoint> {
    folded(without_query(path)).iter().find_map(|path| {
        let plain = without_format_suffix(path);
        [Some(&**path), plain.as_deref()]
            .into_iter()
            .flatten()
            .find_map(|form| routes::row(method, form))
    })
}

/// The value of the first row of `table` that lists `method` to `path`, matched as
/// [`returns_secret`] describes: slashless and suffixed forms included, and in any letter
/// case, for the reason [`route_rules`] gives. A caller's table, such as the support writes
/// `hipc api` checks a body on, is matched by the same rules as this module's own.
pub(crate) fn first_listed<'t, T>(
    table: &'t [(Method, &str, T)],
    method: Method,
    path: &str,
) -> Option<&'t T> {
    let row = |form: &str| {
        let form = with_trailing_slash(form);
        table
            .iter()
            .position(|(route, pattern, _)| *route == method && matches_pattern(pattern, &form))
    };
    // The lower row wins, so `Reverse` makes it the larger outcome `classify` keeps.
    let at = |path: &str| classify(path, None, |form| row(form).map(|at| Some(Reverse(at))));
    folded(path)
        .iter()
        .map(|path| at(path))
        .max()
        .flatten()
        .map(|Reverse(at)| &table[at].2)
}

/// A guard `hipc api` applies to a request, and the route it comes from, which a refusal
/// names.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Rule {
    pub(crate) guard: Guard,
    /// The template of the row, or the pattern of the table entry, the guard comes from.
    pub(crate) route: &'static str,
}

/// The hint of a `hipc api` compose update that carries `secrets`. `apps create` launches a
/// second, billed app, so it is not named here.
const COMPOSE_SECRETS_HINT: &str = "a compose update with secrets is not in this version of \
                                    `hipc`: send the body without `secrets`, or change the \
                                    app's secrets in the console";

/// The hint of a `hipc api` request to the account token route, on any method.
const AUTH_TOKEN_HINT: &str = "create a token in the console settings at \
                               `https://console.hippius.com/dashboard/settings`";

/// Why `hipc api` refuses a read of the account token route.
const AUTH_TOKEN_READ: &str = "its answer is the account's API token";

/// Why `hipc api` refuses a write of the account token route.
const AUTH_TOKEN_WRITE: &str = "the route hands out the account's API token";

/// The refusal of a compose update whose body sets `secrets`, on every method the API may
/// take for it.
const COMPOSE_SECRETS: Guard = Guard::RefusedWhenSet {
    key: "secrets",
    why: crate::routes::compute::APP_SECRETS,
    hint: COMPOSE_SECRETS_HINT,
};

/// The requests with no row that `hipc api` holds back beyond `--yes`, in the pattern
/// language of [`SECRET_ROUTES`]. A row says it of its own route ([`Endpoint::guard`]),
/// and a test refuses an entry whose path a row matches, so a request meets the rows or this
/// table, never both.
///
/// A route is refused outright when a curated command, or the console, is the one safe path
/// to its effect: the console for an account deletion, which locks the account and revokes
/// every token at once, and for the account token route, on every method, whose answer would
/// put the account's own API token in the output, where a log or an agent reads it; and the
/// App Engine commands, which read secret values from a checked private file, while `hipc
/// api` reads its body from any file and prints it in a dry run and in a 400. A `HEAD` of a
/// refused read is refused too: Django REST framework runs a `HEAD` through the `GET`
/// handler.
///
/// A network isolation switch needs `--change-isolation` whatever the body: either mode
/// changes which peers of the network reach each other. A write that replaces a whole list
/// needs `--remove-others` outright: `hipc api` does not read the list to compare, since that
/// would be a second copy of each route's list parser, and a body that copy misread would
/// turn the guard off.
const ROUTE_RULES: &[(Method, &str, Guard)] = &[
    (
        Method::Get,
        "/api/auth/token/",
        Guard::Refused {
            why: AUTH_TOKEN_READ,
            hint: AUTH_TOKEN_HINT,
        },
    ),
    (
        Method::Head,
        "/api/auth/token/",
        Guard::Refused {
            why: AUTH_TOKEN_READ,
            hint: AUTH_TOKEN_HINT,
        },
    ),
    (
        Method::Post,
        "/api/auth/token/",
        Guard::Refused {
            why: AUTH_TOKEN_WRITE,
            hint: AUTH_TOKEN_HINT,
        },
    ),
    // Swagger: a `DELETE` withdraws a pending deletion request, which locks and revokes
    // nothing, so it is sent as any write is; the other write methods are refused as the
    // `POST` is.
    (Method::Delete, "/api/account/deletion/", Guard::Needs(&[])),
    (
        Method::Post,
        "/api/account/deletion/",
        Guard::Refused {
            why: "an account deletion locks the account and revokes every token at once",
            hint: "request the deletion in the console, under Settings, Delete Account",
        },
    ),
    (
        Method::Put,
        "/api/compute/app-engine/apps/{vm_id}/compose/",
        COMPOSE_SECRETS,
    ),
    (
        Method::Post,
        "/api/network/isolation/",
        Guard::Needs(&[RouteFlag::ChangeIsolation]),
    ),
    (
        Method::Put,
        "/api/v1/operator/nodes/{node_id}/storage/",
        Guard::Needs(&[RouteFlag::RemoveOthers]),
    ),
];

/// The rules on `method` to `path` (under `/api/`, no query), all of which apply: none or
/// one, or for a path with a format suffix, the rule of each of its two forms that has one
/// ([`classify`]), and for a path in another letter case, the rules of its lowercase form
/// too ([`folded`]). A path without its trailing `/` matches as if it had one, for the reason
/// [`returns_secret`] gives.
///
/// A form's row decides ([`rule_of`]). A write method with no row of its own on a route
/// takes the guard of another write method's row there: swagger names one method, but a view
/// may take another (Django REST framework's update views take `PUT` beside `PATCH`), and a
/// guard that a second method skipped would guard nothing. A refusal borrowed this way names
/// the request's own method ([`Client::route_flags`](crate::Client::route_flags)). A guard
/// with no flag (`Needs(&[])`) lists a method the API documents as doing something else, so
/// nothing is borrowed from it.
pub(crate) fn route_rules(method: Method, path: &str) -> Vec<Rule> {
    let mut rules = Vec::new();
    for form in folded(path) {
        let plain = without_format_suffix(&form);
        rules.extend(
            [Some(&*form), plain.as_deref()]
                .into_iter()
                .flatten()
                .filter_map(|form| rule_of(method, &with_trailing_slash(form))),
        );
    }
    rules
}

/// `path`, then its lowercase form when that differs: every route table matches both. The
/// server matches letter case, but a front end that folds it would deliver
/// `/api/Network/isolation/` to the isolation route, so a guard holds in any case.
fn folded(path: &str) -> Vec<Cow<'_, str>> {
    let lower = path.to_ascii_lowercase();
    if lower == path {
        vec![Cow::Borrowed(path)]
    } else {
        vec![Cow::Borrowed(path), Cow::Owned(lower)]
    }
}

/// The rule on `method` to one `form` (with its trailing `/`): the guard of its row, or for a
/// method with none, of [`ROUTE_RULES`]' entry; else one a write borrows from another write
/// method of the path. `None` when nothing beyond `--yes` is needed.
fn rule_of(method: Method, form: &str) -> Option<Rule> {
    let own = routes::row(method, form)
        .map(|row| (row.template(), row.guard()))
        .or_else(|| {
            ROUTE_RULES
                .iter()
                .find(|&&(route, pattern, _)| route == method && matches_pattern(pattern, form))
                .map(|&(_, pattern, guard)| (pattern, guard))
        });
    let lent = || {
        let rows = routes::matching(form).map(|row| (row.method(), row.template(), row.guard()));
        let table = ROUTE_RULES
            .iter()
            .filter(|(_, pattern, _)| matches_pattern(pattern, form))
            .copied();
        rows.chain(table)
            .find(|&(route, _, guard)| guard.lends() && !route.is_read() && !method.is_read())
            .map(|(_, route, guard)| (route, borrowed(guard, method)))
    };
    let (route, guard) = own.or_else(lent)?;
    (guard != Guard::None).then_some(Rule { guard, route })
}

/// `guard`, borrowed from another write method's row for `method`. A `PUT` replaces the whole
/// resource, so a field its body leaves out may be reset rather than kept: a guard that asks
/// for a flag when the body sets a key asks for it whatever a `PUT` body sets.
fn borrowed(guard: Guard, method: Method) -> Guard {
    match (guard, method) {
        (Guard::NeedsWhenSet(flags, _), Method::Put) => Guard::Needs(flags),
        _ => guard,
    }
}

/// The outcome of a route table for `path` (its query ignored): `table` gives the outcome
/// for one form of the path, or `None` when no entry matches it, and `fallback` is the
/// outcome when none does. Outcomes are ordered from the least to the most strict.
///
/// A path whose last segment ends in a format suffix, a `.` and one or more ASCII letters
/// or digits (`/api/items.json`), has two forms: as written, and without the suffix
/// (`/api/items`). Its outcome is the stricter of the plain form's, fallback included,
/// and the written form's when an entry matches it. Django REST framework can serve a
/// route under a format suffix as well as without one (the pattern
/// `\.(?P<format>[a-z0-9]+)/?$`,
/// <https://github.com/encode/django-rest-framework/blob/4acc64013b3ca101631efe5ebcacbb18960bbe37/rest_framework/urlpatterns.py#L109>),
/// so a table that matched only the path as written could miss the route a suffixed path
/// reaches. Both forms count, which fails closed whether or not a suffix is served.
///
/// The written form's fallback does not count, so a suffixed path is not refused where its
/// plain form is allowed only because no table lists a dotted path. The plain form's
/// fallback does, so a suffixed path is never allowed where its plain form is refused. The
/// cost accepted: a real last segment that holds a dot, such as a bucket or a host name, is
/// also read without its last `.` part, and may need a flag its own route would not. There
/// is deliberately no way to turn this off.
pub(crate) fn classify<T: Ord>(path: &str, fallback: T, table: impl Fn(&str) -> Option<T>) -> T {
    let path = without_query(path);
    let Some(plain) = without_format_suffix(path) else {
        return table(path).unwrap_or(fallback);
    };
    let plain = table(&plain).unwrap_or(fallback);
    match table(path) {
        Some(written) => written.max(plain),
        None => plain,
    }
}

/// `path` (no query) without the format suffix of its last segment, its trailing `/` kept,
/// or `None` when it has none: `/api/cdn/zones/` for `/api/cdn/zones.json/`. See
/// [`classify`].
pub(crate) fn without_format_suffix(path: &str) -> Option<String> {
    let (body, slash) = path
        .strip_suffix('/')
        .map_or((path, ""), |body| (body, "/"));
    let last = body.rsplit_once('/').map_or(body, |(_, last)| last);
    let (_, suffix) = last.rsplit_once('.')?;
    let is_suffix = !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_alphanumeric());
    is_suffix.then(|| format!("{}{slash}", &body[..body.len() - suffix.len() - 1]))
}

/// Whether two API paths, without their queries, name one route: a path without its
/// trailing `/` is the same route as with it, by the rule `returns_secret` and
/// `route_rules` match by. A format suffix makes another route here, so `--all` refuses
/// a `next` link that adds one.
pub fn same_route(a: &str, b: &str) -> bool {
    with_trailing_slash(a) == with_trailing_slash(b)
}

/// Whether `path` matches `pattern` (see [`SECRET_ROUTES`] and [`ROUTE_RULES`])
/// segment by segment: `{n}` is one segment of ASCII digits, `{..}` as the last segment one
/// or more non-empty segments, and any other `{name}` one non-empty segment.
pub(crate) fn matches_pattern(pattern: &str, path: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix("{..}/") {
        return path
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix('/'))
            .is_some_and(|rest| rest.split('/').all(|segment| !segment.is_empty()));
    }
    let mut segments = path.split('/');
    let each_matches = pattern.split('/').all(|expected| {
        segments.next().is_some_and(|segment| match expected {
            "{n}" => !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit()),
            placeholder if placeholder.starts_with('{') => !segment.is_empty(),
            literal => segment == literal,
        })
    });
    each_matches && segments.next().is_none()
}

/// A PUT of one ticket: swagger's `PUT /api/support/tickets/{id}/`
/// (`support_tickets_update`) replaces the status, priority, and assignee, which the support
/// team sets, and the console never sends it, so it stays refused with a membership like every
/// route the console does not list. The `PATCH` of the same path is the customer's close
/// (the console sends `{"status": "closed"}` to it as the signed-in user, with no account
/// header), so it is personal, as the rest of the ticket routes are. Swagger's
/// `support_tickets_partial_update` takes those fields too, and `hipc api` sends whatever
/// body the caller wrote: the server is the guard on which fields a customer may change.
const TICKET_PUT: &str = "/api/support/tickets/{id}/";

/// The class of `method` to a request path such as `/api/compute/vms/`: its row's, else the
/// tables'. A query string is ignored. In the tables, only a PUT of one ticket depends on the
/// method. A path with a format suffix takes
/// the stricter class of its two forms ([`classify`]).
pub(crate) fn route_class(method: Method, path: &str) -> RouteClass {
    classify(path, RouteClass::Unsupported, |form| {
        routes::row(method, form)
            .map(Endpoint::account)
            .or_else(|| listed_class(method, form))
    })
}

/// The class the tables give `method` to `path` (one form, no query), or `None` when they
/// do not list it. Only a path no row matches reaches the tables ([`route_class`]).
pub(crate) fn listed_class(method: Method, path: &str) -> Option<RouteClass> {
    let path = &*with_trailing_slash(path);
    if method == Method::Put && matches_pattern(TICKET_PUT, path) {
        return Some(RouteClass::Unsupported);
    }
    let personal = PERSONAL_PATHS.contains(&path)
        || PERSONAL_PREFIXES.iter().any(|p| path.starts_with(p))
        || is_database_personal(path);
    if personal {
        Some(RouteClass::Personal)
    } else if SCOPED_PREFIXES.iter().any(|p| path.starts_with(p)) {
        Some(RouteClass::Scoped)
    } else {
        None
    }
}

/// `/api/databases/<n>/backup-token/` and `/api/databases/<n>/members/`.
fn is_database_personal(path: &str) -> bool {
    matches!(database_action(path), Some("backup-token/" | "members/"))
}

/// What follows `/api/databases/<n>/` in `path`, for a database id `<n>` of digits only:
/// `restore/` for `/api/databases/7/restore/`.
fn database_action(path: &str) -> Option<&str> {
    let (id, tail) = path.strip_prefix("/api/databases/")?.split_once('/')?;
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then_some(tail)
}

/// The recurring card routes. They act in the selected account only beside a billing
/// account; elsewhere they stay the user's own, so without `X-Hippius-Billing` they keep the
/// account header's class.
pub(crate) const BILLING_NAMED_ONLY_PATHS: &[&str] = &[
    "/api/billing/stripe/active-subscription/",
    "/api/billing/stripe/create-subscription/",
    "/api/billing/stripe/customer-portal/",
];

/// Whether `method` to `path` (a request path, its query ignored) is a recurring card
/// route, which acts in the selected account only beside `X-Hippius-Billing`: its row says
/// so, or for a path no row matches, the table lists it. A path with a format suffix is one
/// only when its plain form is one: no entry holds a dot, so the written form can never name
/// one, and the plain form decides ([`classify`]).
pub(crate) fn names_billing_only(method: Method, path: &str) -> bool {
    let path = without_query(path);
    let plain = without_format_suffix(path);
    let form = plain.as_deref().unwrap_or(path);
    routes::row(method, form).map_or_else(
        || BILLING_NAMED_ONLY_PATHS.contains(&&*with_trailing_slash(form)),
        Endpoint::account_beside_billing,
    )
}

/// The `X-Hippius-Account` value for a `method` request to `path`, or `None` when no
/// header goes on it. `billing_header` is the `X-Hippius-Billing` value the request carries:
/// the recurring card routes act in the selected account only beside one, as the console
/// sends them from a billing account's page.
///
/// # Errors
///
/// `account_scope_unsupported` when a membership is selected and the route cannot act in
/// another account. No request may be sent then.
pub(crate) fn header<'a>(
    selection: &'a Selection,
    method: Method,
    path: &str,
    billing_header: Option<&str>,
) -> Result<Option<&'a str>> {
    let Selection::Member(id) = selection else {
        return Ok(None);
    };
    match route_class(method, path) {
        RouteClass::Scoped => Ok(Some(id.as_str())),
        RouteClass::Unsupported if billing_header.is_some() && names_billing_only(method, path) => {
            Ok(Some(id.as_str()))
        }
        RouteClass::Personal => Ok(None),
        // A `hipc api` path can carry a secret in its query (tenet 9).
        RouteClass::Unsupported => Err(id.0.refusal(&format!(
            "{} cannot act in another account",
            quote(without_query_values(path))
        ))),
    }
}

#[cfg(test)]
pub(crate) mod tests;
