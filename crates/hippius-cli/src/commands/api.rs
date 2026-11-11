//! `hipc api <METHOD> <PATH>`: any console endpoint, with the same auth, account-header
//! rules, and error object as the curated commands (`TENETS.md` tenet 7).
//!
//! A 2xx body is never re-serialized, so a number too large for a float keeps every digit.
//! On a pipe in JSON mode it is printed as the bytes received; whenever a person may read
//! stdout (text mode, or a terminal in any mode) its control and invisible characters are
//! escaped. `--all` is the exception: it collects the pages into one JSON array, as `--all`
//! does on every list command.

use hippius::routes::{self, Endpoint};
use hippius::{Client, Error, Method, Request, Result, RouteFlag};
use serde_json::Value;

use crate::cli::Api;
use crate::commands::{
    Input, Pages, Printed, cdn, compute, drive, hub, network, require_reveal, s3,
};
use crate::context::{self, Context, Gate, Step};

/// The largest `--body-file` body read, in bytes. Arbitrary: far above any console request
/// body, and small enough that a wrong path is refused rather than sent.
const MAX_INPUT_BYTES: u64 = 8 * 1024 * 1024;

/// `hipc api`.
pub(crate) fn run(ctx: &Context<'_>, args: &Api) -> Result<Printed> {
    let method = Method::parse(&args.method)?;
    if args.all && method != Method::Get {
        return Err(Error::usage(
            format!("`--all` works with `GET` only, and the method is `{method}`"),
            "remove `--all`, which follows the pages of a `GET` only",
        ));
    }
    let mut request = Request::new(method, args.path.clone());
    let mut body = None;
    // Every method that may change state takes a body: `DELETE
    // /api/cdn/lists/<name>/items/` needs one (swagger `cdn_lists_items_delete`).
    match (&args.body_file, method.is_read()) {
        (Some(path), false) => {
            let input = Input {
                path,
                flag: "--body-file",
                what: "request body",
            };
            let (bytes, value) = input.read_json(MAX_INPUT_BYTES)?;
            body = Some((String::from_utf8_lossy(&bytes).into_owned(), value));
            request = request.raw_json(bytes);
        }
        (Some(_), true) => {
            return Err(Error::usage(
                format!(
                    "`--body-file` sends a body, and `{method}` has none: a body goes with `POST`, \
                     `PUT`, `PATCH`, or `DELETE`"
                ),
                "remove `--body-file`",
            ));
        }
        (None, _) => {}
    }
    let config = context::config()?;
    let client = ctx.client(&config)?;
    refuse_token_to_support(ctx, &client, &config, &request, body.as_ref())?;
    // `None` for a route `hipc` does not know: no command sends it.
    let row = client.route_of(&request)?;
    // Before the selected account's refusals and the confirmation, as for every curated
    // command (TENETS 5 and 9): `returns_secret` applies no account rule. An answer `hipc`
    // knows nothing of may hold a secret too, so a `GET` of a route with no row needs
    // `--reveal` as well (tenet 9); a `HEAD` answer has no body to print.
    if client.returns_secret(&request)? {
        require_reveal(ctx, args.reveal)?;
    } else if row.is_none() && method == Method::Get {
        require_reveal(ctx, args.reveal).map_err(|refusal| {
            refusal.with_message(format!(
                "`hipc` does not know `{method} {}`, so its answer may hold a secret, and the \
                 command needs `--reveal` to print it",
                hippius::without_query_values(&args.path)
            ))
        })?;
    }
    // A write `hipc` does not know is sent as any write is, with `--yes`, but no curated
    // command says what it changes.
    if row.is_none() && !method.is_read() {
        ctx.run_warning(&format!(
            "`hipc` does not know `{method} {}`: no command sends it, so `hipc` cannot say \
             what it changes",
            hippius::without_query_values(&args.path)
        ));
    }
    // After `--reveal` and before the same refusals, for the same reason. A route never
    // sent, such as an account deletion request, was refused already, inside
    // `returns_secret`, whatever the flags.
    // A route that needs two flags refuses for the first one missing, so a rerun with its
    // hint meets the second.
    let namespace = row == Some(routes::hub::PUBLICITY_UPDATE.endpoint());
    for route_flag in client.route_flags(&request)? {
        require_route_flag(ctx, args, route_flag, namespace)?;
    }
    // A read that may create something, such as the first `GET /api/network/me/`, is a
    // write from here on: confirmed with the curated command's consequence, refused with a
    // stored project, and `partial` when it fails after it was sent. Marked
    // before `url`, which applies the selection rules by the request's access.
    let provisions = client.provisions(&request)?;
    if provisions {
        request = request.provisions();
    }
    if args.all {
        // `--all` sends the first page unconfirmed to follow its links; a route that creates
        // on a read must not be sent that way, and it is no list to page through.
        if provisions {
            return Err(Error::usage(
                format!(
                    "`--all` sends the first page without a confirmation, and `{method} {}` \
                     may create the account's network",
                    hippius::without_query_values(&args.path)
                ),
                "remove `--all`",
            ));
        }
        return every_page(ctx, &client, request);
    }
    // The question names the whole URL, so the origin it goes to is in it. Query values can
    // be secrets, so it shows the names only, as --debug does.
    let url = hippius::without_query_values(&client.url(&request)?);
    let request = request.about(format!("call `{method} {url}`"));
    let permanent = client.is_permanent(&request)?;
    let consequence = row.and_then(|row| {
        CONSEQUENCES
            .iter()
            .find(|(route, _)| *route == row)
            .map(|&(_, consequence)| consequence)
    });
    let gate = gate(&request, provisions, permanent, consequence);
    match ctx.call(&client, &request, gate)? {
        Step::Sent(response) => Ok(Printed::Body(response.into_bytes())),
        Step::DryRun(doc) => Ok(Printed::Document(doc)),
    }
}

/// The writes whose body a person on the support team may read: every `POST`, `PUT`, `PATCH`,
/// and `DELETE` under `/api/support/`, such as a new ticket, a reply, a ticket `PATCH`, or a
/// chat handoff. Matched by [`hippius::Client::route_value`].
const READ_BY_SUPPORT: &[(Method, &str, ())] = &[
    (Method::Post, "/api/support/{..}/", ()),
    (Method::Put, "/api/support/{..}/", ()),
    (Method::Patch, "/api/support/{..}/", ()),
    (Method::Delete, "/api/support/{..}/", ()),
];

/// Refuses a write to a route of [`READ_BY_SUPPORT`] whose `body` holds an API token as it
/// is, quoted, percent-encoded, or behind a JSON escape, or an access key
/// ([`crate::commands::support::secret_in`]), as
/// `hipc support tickets create` and `reply` refuse it: a person reads what is sent, so a body
/// an injected instruction filled with the user's config would hand that person the account.
/// The body is checked as written and as each string it decodes to, so a `\u` escape does not
/// hide a token. It runs before the dry run, which would print the body, and never quotes it;
/// a dry run included, whatever the flags.
fn refuse_token_to_support(
    ctx: &Context<'_>,
    client: &Client,
    config: &hippius::ConfigFile,
    request: &Request,
    body: Option<&(String, Value)>,
) -> Result<()> {
    let Some((raw, value)) = body else {
        return Ok(());
    };
    if client.route_value(request, READ_BY_SUPPORT)?.is_none() {
        return Ok(());
    }
    let mut texts = vec![raw.as_str()];
    strings(value, &mut texts);
    if let Some(secret) = crate::commands::support::secret_in(ctx, config, &texts)? {
        return Err(Error::usage(
            format!(
                "`hipc api` does not send a support ticket write whose body holds {}, which a \
                 person on the Hippius support team would read",
                secret.what()
            ),
            format!(
                "remove {} from the body; the support team never needs it",
                secret.it()
            ),
        ));
    }
    Ok(())
}

/// Every string of `value`, keys included, pushed onto `texts`.
fn strings<'v>(value: &'v Value, texts: &mut Vec<&'v str>) {
    match value {
        Value::String(text) => texts.push(text),
        Value::Array(items) => items.iter().for_each(|item| strings(item, texts)),
        Value::Object(fields) => {
            for (key, item) in fields {
                texts.push(key);
                strings(item, texts);
            }
        }
        _ => {}
    }
}

/// The gate of `request`: open for a read, for a read that `provisions` the gate `hipc
/// network get` takes, for a `permanent` write the gate of the curated Hub deletes, and for
/// any other write the question with the `consequence` the curated command gives on the same
/// route ([`CONSEQUENCES`]), or none, so each pair asks the same question. `--yes` with it is
/// the whole confirmation (tenet 5).
fn gate(
    request: &Request,
    provisions: bool,
    permanent: bool,
    consequence: Option<&'static str>,
) -> Gate<'static> {
    if provisions {
        crate::commands::network::creation_gate()
    } else if permanent {
        crate::commands::hub::PERMANENT
    } else if request.is_read() {
        Gate::Open
    } else {
        Gate::Confirm { consequence }
    }
}

/// The deletes and cancels whose curated command says what they end, by the route the
/// command sends, with that consequence, so `hipc api` asks the same question on the same
/// route. The route is the row a request reaches ([`Client::route_of`]), so a slashless or
/// suffixed path names its route too. A consequence a curated command builds from the read
/// or the id, such as a database delete's, is not here.
const CONSEQUENCES: [(&Endpoint, &str); 14] = [
    (
        routes::drive::SUBSCRIPTION_CANCEL.endpoint(),
        drive::CANCELLED,
    ),
    (routes::s3::SUBSCRIPTION_CANCEL.endpoint(), s3::CANCELLED),
    (routes::hub::SUBSCRIPTION_CANCEL.endpoint(), hub::CANCELLED),
    (routes::hub::KEY_DELETE.endpoint(), hub::keys::DELETED),
    (
        routes::compute::VM_DELETE.endpoint(),
        compute::VM_LEFT_BILLED,
    ),
    (
        routes::compute::PUBLIC_IP_DELETE.endpoint(),
        compute::IP_DETACHED,
    ),
    (
        routes::compute::EXPOSURE_DELETE.endpoint(),
        compute::EXPOSURE_DELETED,
    ),
    (routes::cdn::ZONE_DELETE.endpoint(), cdn::ZONE_DELETED),
    (
        routes::cdn::HOSTNAME_DELETE.endpoint(),
        cdn::HOSTNAME_DELETED,
    ),
    (
        routes::runners::INSTALLATION_DELETE.endpoint(),
        compute::runners::UNLINKED,
    ),
    (
        routes::network::PEER_DELETE.endpoint(),
        network::PEER_REMOVED,
    ),
    (
        routes::compute::BACKUP_POLICY_DELETE.endpoint(),
        compute::BACKUPS_KEPT,
    ),
    (
        routes::compute::SSH_KEY_DELETE.endpoint(),
        compute::SSH_KEY_KEPT,
    ),
    (
        routes::cdn::LIST_ITEMS_REMOVE.endpoint(),
        cdn::waf::LIST_REPUBLISHES,
    ),
];

/// Refuses a request that needs `route_flag` unless it was passed. A dry run sends nothing,
/// so it needs no flag, as it needs no `--reveal` (TENETS 5). Each flag refuses with its
/// own code, `--public` and `--remove-others` with the curated commands' codes.
/// `namespace` says the request is the Hub namespace publicity `PATCH`, whose `--public`
/// refusal names who can pull rather than a bucket.
fn require_route_flag(
    ctx: &Context<'_>,
    args: &Api,
    route_flag: RouteFlag,
    namespace: bool,
) -> Result<()> {
    let argv = ctx.args();
    let (passed, refusal) = match route_flag {
        RouteFlag::ChangeIsolation => (
            args.change_isolation,
            Error::change_isolation_required(argv),
        ),
        RouteFlag::Public => (
            args.public,
            Error::public_required(
                if namespace {
                    "this may make the Hub namespace public, so anyone can pull its images and \
                     models without logging in, and `--public` was not passed"
                } else {
                    "this may make a private bucket readable by anyone, and `--public` was not \
                     passed"
                },
                argv,
            ),
        ),
        // The curated command reads the list and names what a write removes; `hipc api`
        // reads nothing, for the reason the route-rule table in `hippius::account` gives.
        RouteFlag::RemoveOthers => (
            args.remove_others,
            Error::remove_others_unread(
                "this would replace a whole list or document, removing every entry it leaves \
                 out, and `--remove-others` was not passed",
                argv,
            ),
        ),
    };
    if passed || ctx.dry_run() {
        Ok(())
    } else {
        Err(refusal)
    }
}

/// Follows `next` links from a Django REST framework list and prints one JSON array, as
/// `--all` does on every list command. A bare array is one page.
///
/// A `next` link must stay on the origin of the first page ([`hippius::follow`]) and on its
/// path, and [`Pages`] refuses a loop and a list past its page cap.
fn every_page(ctx: &Context<'_>, client: &Client, first: Request) -> Result<Printed> {
    let mut items = Vec::new();
    // The first URL is the caller's: a refusal of it is their usage error.
    let mut url = client.url(&first)?;
    let list_path = path_of(&url).to_owned();
    let mut pages = Pages::starting_at(&url);
    let mut request = first;
    loop {
        let page = match ctx.call(client, &request, Gate::Open)? {
            Step::Sent(response) => {
                pages.read(response.bytes().len())?;
                response.json()?
            }
            Step::DryRun(doc) => return Ok(Printed::Document(doc)),
        };
        let page = hippius::page(page).map_err(|_| not_a_list(&url))?;
        items.extend(page.items);
        let Some(next) = page.next else {
            break;
        };
        request = hippius::follow(&url, &next)?;
        // Later URLs come from the server, so a refusal of one is not the caller's to fix
        // (tenet 4: exit 2 means the invocation must change): whatever `url` refuses it with
        // is `internal`. That holds for `account_scope_unsupported` too: `--account self`
        // would only meet the route check below.
        url = client.url(&request).map_err(|error| {
            Error::internal(format!(
                "the API's `next` link is refused: {}",
                error.message()
            ))
        })?;
        // The pages of one list share its route; only the query moves on. A link to another
        // route could reach one the first request was not checked for, such as one whose
        // answer holds a secret and needs `--reveal`, so it is refused before it is sent. A
        // trailing `/` alone does not make another route (`hippius::same_route`).
        if !hippius::same_route(path_of(&url), &list_path) {
            return Err(Error::internal(format!(
                "the API's `next` link {} leaves {}, the path of the list",
                hippius::phrase::quote(hippius::without_query_values(&url)),
                hippius::phrase::quote(list_path)
            )));
        }
        pages.admit(&url)?;
    }
    Ok(Printed::Document(Value::Array(items)))
}

/// The path of `url`, a URL from [`Client::url`], without its query.
fn path_of(url: &str) -> &str {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let path = after_scheme
        .find('/')
        .map_or("/", |start| &after_scheme[start..]);
    path.split_once('?').map_or(path, |(path, _)| path)
}

fn not_a_list(url: &str) -> Error {
    Error::internal(format!(
        "`--all` needs a list, and {} did not answer with one",
        hippius::phrase::quote(hippius::without_query_values(url))
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provisioning read takes the gate of `hipc network get`, consequence included, which
    /// the prompt shows (`context::tests`); a plain read is open, and a write asks alone.
    #[test]
    fn a_provisioning_read_takes_the_gate_of_the_curated_command() {
        let summary = Request::get("/network/me/").provisions();
        assert!(matches!(
            gate(&summary, true, false, None),
            Gate::Confirm { consequence: Some(text) }
                if text == crate::commands::network::NETWORK_CREATED
        ));
        assert!(matches!(
            gate(&Request::get("/accounts/"), false, false, None),
            Gate::Open
        ));
        assert!(matches!(
            gate(
                &Request::new(Method::Post, "/accounts/"),
                false,
                false,
                None
            ),
            Gate::Confirm { consequence: None }
        ));
        // A permanent write takes the curated Hub deletes' gate.
        let delete = Request::new(Method::Delete, "/registry/repositories/acme/app/");
        assert!(matches!(
            gate(&delete, false, true, None),
            Gate::Confirm {
                consequence: Some("this cannot be undone")
            }
        ));
        // Any other write asks with the consequence it is given, and a read never asks.
        let cancel = Request::new(Method::Delete, "/registry/subscriptions/7/");
        assert!(matches!(
            gate(&cancel, false, false, Some(hub::CANCELLED)),
            Gate::Confirm { consequence: Some(text) } if text == hub::CANCELLED
        ));
        assert!(matches!(
            gate(
                &Request::get("/accounts/"),
                false,
                false,
                Some(hub::CANCELLED)
            ),
            Gate::Open
        ));
    }
}
