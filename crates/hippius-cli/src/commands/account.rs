//! `hipc account`: the accounts one token can act in, and the team of the selected one.
//!
//! `GET /api/accounts/` is personal and never takes `X-Hippius-Account`. The team routes,
//! `/api/accounts/members/` and `/api/accounts/invites/`, are account-scoped: with a
//! membership selected, the header names the account whose team is managed. The invite
//! preview and accept are personal: they act as the token's own user.
//!
//! An invite token, from the invite email's link, is a bearer secret (tenet 9). It is read
//! from `--token-file`, sent only in a request body, and never printed: not in a message, a
//! hint, a `--debug` line, a dry-run document, or an answer that repeats it. It follows the
//! API token's origin rule (tenet 8): it goes to an origin other than `https://api.hippius.com`
//! only when the caller confirms that origin.

use std::fmt;
use std::path::Path;

use hippius::{
    BillingId, Client, Code, ConfigFile, Error, Given, Method, Overrides, ProjectId, Request,
    Result, Selection, Settings, Source, phrase, redact_value,
};
use serde_json::{Map, Value, json};
use zeroize::Zeroizing;

use crate::cli::{
    Access, Grant, GrantArg, InviteCreate, Invites, MarketingConsent, MemberUpdate, Members,
    Notifications,
};
use crate::commands::replace::{Entry, Found, Replace, replace_list};
use crate::commands::{
    Input, Printed, print_body, print_body_or, print_keyed_list, store_selection,
};
use crate::context::{self, Context, Gate, Step, Write};

/// The answer to `GET /api/accounts/`, or the `--dry-run` document in its place.
#[derive(Debug)]
enum Fetched {
    /// The `accounts` array (the caller's own account, then memberships; see
    /// [`own_account`]), and the answer's other fields, such as `sharing_enabled`.
    Accounts(hippius::Page),
    DryRun(Value),
}

/// The accounts list, `GET /api/accounts/`.
const ACCOUNTS: &str = "/accounts/";

fn accounts(ctx: &Context<'_>, client: &Client) -> Result<Fetched> {
    let request = Request::get(ACCOUNTS).about("list the accounts the token can act in");
    let response = match ctx.call(client, &request, Gate::Open)? {
        Step::Sent(response) => response,
        Step::DryRun(doc) => return Ok(Fetched::DryRun(doc)),
    };
    match response.json()? {
        Value::Object(mut body) => match body.shift_remove("accounts") {
            Some(Value::Array(accounts)) => {
                Ok(Fetched::Accounts(hippius::page_beside(accounts, body)?))
            }
            _ => Err(unexpected()),
        },
        _ => Err(unexpected()),
    }
}

/// Where the token's own account is in the `accounts` of `GET /api/accounts/`: the one entry
/// with `is_owner: true`, the field the console reads (a membership is `is_owner: false` and
/// has a `membership_id`). The console also lists that account first, but the position is not
/// trusted: a membership listed first would be shown as the token's own, and a project
/// transfer would name it as the source. `None` when no entry, or more than one, is marked.
pub(crate) fn own_account(accounts: &[Value]) -> Option<usize> {
    let mut owned = accounts
        .iter()
        .enumerate()
        .filter(|(_, account)| account.get("is_owner") == Some(&Value::Bool(true)));
    match (owned.next(), owned.next()) {
        (Some((at, _)), None) => Some(at),
        _ => None,
    }
}

/// The error of an accounts answer in which [`own_account`] finds no account.
pub(crate) fn no_own_account() -> Error {
    Error::internal(
        "the API's accounts list does not mark exactly one account as the token's own with \
         `is_owner`",
    )
}

fn unexpected() -> Error {
    Error::internal("the API answered `GET /api/accounts/` without an `accounts` list")
}

/// `hipc account list`.
pub(crate) fn list(ctx: &Context<'_>) -> Result<Printed> {
    Ok(
        match accounts(ctx, &ctx.authed_client(&context::config()?)?)? {
            Fetched::Accounts(page) => Printed::List(page.complete_envelope(ACCOUNTS)?),
            Fetched::DryRun(doc) => Printed::Document(doc),
        },
    )
}

/// `hipc account get`: the selected account, or the caller's own for `self`.
pub(crate) fn get(ctx: &Context<'_>) -> Result<Printed> {
    let (settings, client) = ctx.authed(&context::config()?)?;
    let selected = selected_scope(&ctx.overrides()?, &settings);
    let selection = settings.account;
    let mut page = match accounts(ctx, &client)? {
        Fetched::Accounts(page) => page,
        Fetched::DryRun(doc) => return Ok(Printed::Document(doc)),
    };
    let found = match &selection {
        Selection::Own => own_account(&page.items),
        Selection::Member(id) => page
            .items
            .iter()
            .position(|a| a.get("id").and_then(Value::as_str) == Some(id.as_str())),
    };
    if let Some(at) = found {
        // The account is on the page read, so a later page cannot change the answer.
        let mut account = page.items.swap_remove(at);
        if let Value::Object(fields) = &mut account {
            fields.extend(selected);
        }
        return Ok(Printed::Document(account));
    }
    // Missing from a page that is not the last is no proof the token cannot act in it: a
    // membership on a later page would otherwise be `not_a_member`.
    page.into_complete(ACCOUNTS)?;
    if matches!(selection, Selection::Own) {
        return Err(no_own_account());
    }
    Err(Error::new(
        Code::NotAMember,
        format!(
            "the token cannot act in the account `{}`",
            selection.as_str()
        ),
    )
    .with_hint("run `hipc account list` to see the accounts the token can act in"))
}

/// The project and the billing account this run acts in, and where each was selected, as
/// `hipc auth status` and `hipc account get` add them: `selected_project` and
/// `selected_billing`, an id or null for the Default one, and `selected_project_source` and
/// `selected_billing_source`, `flag`, `environment`, or `profile`, or null when nothing selected
/// one. A `--project default` is null from `flag`: the flag chose the Default project over
/// whatever the profile stores.
pub(crate) fn selected_scope(overrides: &Overrides, settings: &Settings) -> Map<String, Value> {
    let mut fields = Map::new();
    let scopes = [
        (
            "project",
            overrides.project.as_ref(),
            settings.project.as_ref().map(ProjectId::as_str),
        ),
        (
            "billing",
            overrides.billing.as_ref(),
            settings.billing.as_ref().map(BillingId::as_str),
        ),
    ];
    for (name, given, id) in scopes {
        // An override comes from a flag or a variable ([`Context::overrides`]).
        let source = given.map_or(id.map(|_| "profile"), |given| {
            Some(if matches!(given.source, Source::Variable(_)) {
                "environment"
            } else {
                "flag"
            })
        });
        fields.insert(format!("selected_{name}"), json!(id));
        fields.insert(format!("selected_{name}_source"), json!(source));
    }
    fields
}

const NOTIFICATIONS: &str = "/notifications/settings/";
const MARKETING_CONSENT: &str = "/notifications/marketing-consent/";
/// The console page where the consent is given, beside the wording it is given to.
const CONSENT_SETTINGS: &str = "https://console.hippius.com/dashboard/settings";

/// `hipc account notifications`: the signed-in user's `NotificationPreferences`, a personal
/// route, so a membership sends no header.
///
/// `get` prints them as sent. `update` sends a `PATCH` of only the settings given: swagger
/// documents a `PATCH` beside the `PUT` that replaces the whole set, so a setting left out
/// keeps its value and nothing is read first. It spends nothing; a repeat sets the same values.
/// Turning off an email that warns of a problem ([`ALERTS`]) hides that problem from the
/// user, so that update is confirmed; any other is not.
pub(crate) fn notifications(ctx: &Context<'_>, command: &Notifications) -> Result<Printed> {
    let client = ctx.load_authed_client()?;
    let mut off = Vec::new();
    let request = match command {
        Notifications::Get => Request::get(NOTIFICATIONS).about("read the notification settings"),
        Notifications::Update(args) => {
            off = args
                .fields()
                .into_iter()
                .filter(|(key, given)| *given == Some(false) && ALERTS.contains(key))
                .map(|(key, _)| key)
                .collect();
            let given: Map<String, Value> = args
                .fields()
                .into_iter()
                .filter_map(|(key, given)| given.map(|on| (key.to_owned(), Value::from(on))))
                .collect();
            let named = phrase::ticked(given.keys(), "and");
            Request::new(Method::Patch, NOTIFICATIONS)
                .json(&Value::Object(given))
                .about(format!("update the notification settings {named}"))
        }
    };
    let flags: Vec<String> = off
        .iter()
        .map(|key| format!("`--{}`", key.replace('_', "-")))
        .collect();
    let consequence = if off.contains(&"email_enabled") {
        "no notification email is sent at all, whatever the other settings say".to_owned()
    } else {
        format!(
            "the user is no longer emailed what {} covers",
            phrase::join(&flags, "or")
        )
    };
    let gate = if off.is_empty() {
        Gate::Open
    } else {
        Gate::Confirm {
            consequence: Some(&consequence),
        }
    };
    print_body(ctx, &client, &request, gate)
}

/// The notification settings whose email warns of a problem: every email, the low and the
/// empty balance, and a file that fails to pin. Turning one off is confirmed.
const ALERTS: [&str; 4] = [
    "email_enabled",
    "low_credit_alerts",
    "zero_balance_alerts",
    "file_status_updates",
];

/// `hipc account marketing-consent`: `GET` and `POST /api/notifications/marketing-consent/`, the
/// signed-in user's own consent (swagger: "The authenticated customer's marketing consent"), so
/// a personal route, as the notification settings are.
///
/// `update` only withdraws. Consent is to a wording (swagger: `wording_version`, "the version
/// of the text the person agreed to"), and `hipc` never shows that text, so a consent given from
/// here would be to words the person never read; `--granted true` is refused, before anything
/// is sent, with a hint to the console, which shows them. `hipc api` can still send the `POST`.
///
/// The `POST` names `source` `settings`, the value swagger gives a change outside the sign-up
/// form (its default), so a later default cannot file it as something else. A withdrawal
/// spends nothing, so nothing is asked; the server appends each `POST` to the consent history,
/// so a repeat is not a no-op.
pub(crate) fn marketing_consent(ctx: &Context<'_>, command: &MarketingConsent) -> Result<Printed> {
    if let MarketingConsent::Update { granted: true } = command {
        return Err(Error::usage(
            "`--granted true` cannot give the marketing consent from `hipc`: it never shows the \
             wording the consent is to",
            format!("give the consent in the console at `{CONSENT_SETTINGS}`"),
        ));
    }
    let client = ctx.load_authed_client()?;
    let request = match command {
        MarketingConsent::Get => {
            Request::get(MARKETING_CONSENT).about("read the marketing consent")
        }
        MarketingConsent::Update { .. } => Request::new(Method::Post, MARKETING_CONSENT)
            .json(&json!({ "granted": false, "source": "settings" }))
            .about("withdraw the marketing consent"),
    };
    print_body(ctx, &client, &request, Gate::Open)
}

/// `hipc account use <account_id>`: stores the selection in the profile. No request is sent.
pub(crate) fn select(ctx: &Context<'_>, id: &str) -> Result<Printed> {
    let selection = Selection::parse(id, &Source::Argument)?;
    store_selection(
        ctx,
        "select_account",
        ("account", selection.as_str()),
        ConfigFile::profile_name,
        |config, profile| config.set_account(profile, &selection),
    )
}

/// The command that lists the member ids, for the hint of a 404 on a member route.
const MEMBERS: &str = "hipc account members list";

/// `GET /api/accounts/members/`: every member, with their grants.
fn members_list() -> Request {
    Request::get(MEMBERS_PATH).about("list the team members")
}

/// The members list route.
const MEMBERS_PATH: &str = "/accounts/members/";

/// `hipc account members`.
pub(crate) fn members(ctx: &Context<'_>, command: &Members) -> Result<Printed> {
    match command {
        // The API answers `{members, owner}`; `owner` is kept.
        Members::List => {
            print_keyed_list(ctx, &ctx.load_authed_client()?, &members_list(), "members")
        }
        Members::Update(args) => update_member(ctx, args),
        Members::Delete { member_id } => {
            let request = Request::new(Method::Delete, format!("/accounts/members/{member_id}/"))
                .listed_by(MEMBERS)
                .about(format!("remove the member `{member_id}` from the team"));
            let done = json!({ "id": member_id, "deleted": true });
            let client = ctx.load_authed_client()?;
            print_body_or(
                ctx,
                &client,
                &request,
                Gate::Confirm { consequence: None },
                done,
            )
        }
    }
}

/// `hipc account invites`.
pub(crate) fn invites(ctx: &Context<'_>, command: &Invites) -> Result<Printed> {
    match command {
        // The API answers `{invites}`.
        Invites::List => print_keyed_list(
            ctx,
            &ctx.load_authed_client()?,
            &Request::get("/accounts/invites/").about("list the pending invites"),
            "invites",
        ),
        Invites::Create(args) => create_invite(ctx, args),
        // The console calls the preview signed out, so it needs no stored token, and sends one
        // only when there is one.
        Invites::Preview { token_file } => {
            let invite = InviteToken::read(token_file)?;
            let (settings, client) = ctx.settings_and_client(&context::config()?)?;
            preview(ctx, &client, &settings, &invite)
        }
        Invites::Accept { token_file } => {
            let invite = InviteToken::read(token_file)?;
            let (settings, client) = ctx.authed(&context::config()?)?;
            accept(ctx, &client, &settings, &invite)
        }
        Invites::Revoke { invite_id } => {
            let request = Request::new(Method::Delete, format!("/accounts/invites/{invite_id}/"))
                .listed_by("hipc account invites list")
                .about(format!("revoke the invite `{invite_id}`"));
            let done = json!({ "id": invite_id, "revoked": true });
            let client = ctx.load_authed_client()?;
            print_body_or(
                ctx,
                &client,
                &request,
                Gate::Confirm { consequence: None },
                done,
            )
        }
    }
}

const PREVIEW: &str = "/accounts/invites/preview/";

const ACCEPT: &str = "/accounts/invites/accept/";

/// What a dry run's body shows in place of the invite token.
const TOKEN_PLACEHOLDER: &str = "<the invite token from --token-file>";

/// An invite token, read from `--token-file`, cleared when dropped. `Debug` says one is
/// present, never what it is.
struct InviteToken(Zeroizing<String>);

impl InviteToken {
    /// Reads the token: surrounding whitespace is trimmed, and what is left must be one word
    /// of visible ASCII, as the last part of a link is.
    ///
    /// # Errors
    ///
    /// `usage` when the file cannot be read, or holds nothing but whitespace, or more than
    /// one word. The message names the file and never quotes what is in it.
    fn read(path: &Path) -> Result<Self> {
        let input = Input {
            path,
            flag: "--token-file",
            what: "invite token",
        };
        let text = input.read_secret()?;
        let token = text.trim();
        let problem = if token.is_empty() && !text.is_empty() {
            "holds only whitespace"
        } else if token.is_empty() {
            "is empty"
        } else if !token.bytes().all(|b| b.is_ascii_graphic()) {
            "holds a space, a line break, or a character outside visible ASCII"
        } else {
            return Ok(Self(Zeroizing::new(token.to_owned())));
        };
        let hint = if path == Path::new("-") {
            "pipe only the invite token, the last part of the invite link, to `--token-file -`"
        } else {
            "leave only the invite token, the last part of the invite link, in the file \
             `--token-file` names"
        };
        Err(Error::usage(
            format!("the invite token from {} {problem}", input.name()),
            hint,
        ))
    }
}

impl fmt::Debug for InviteToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("InviteToken(redacted)")
    }
}

/// `POST /api/accounts/invites/preview/ {"token"}`, the console's shape. The token is in the
/// body, so it stays out of access logs.
fn preview_request(token: &str) -> Request {
    Request::new(Method::Post, PREVIEW)
        .json(&json!({ "token": token }))
        .about("preview the invite")
        .changes_nothing()
}

/// `POST /api/accounts/invites/accept/`, with `confirm_without_email` always false: joining
/// as a user whose address the invite was not sent to is the console's question to ask.
fn accept_request(token: &str) -> Request {
    Request::new(Method::Post, ACCEPT).json(&json!({
        "token": token,
        "confirm_without_email": false,
    }))
}

/// `hipc account invites preview`: what the invite offers, as the server sent it. A dry run
/// sends nothing, so it asks nothing.
fn preview(
    ctx: &Context<'_>,
    client: &Client,
    settings: &Settings,
    invite: &InviteToken,
) -> Result<Printed> {
    if ctx.dry_run() {
        let doc = ctx.dry_run_document(client, &preview_request(TOKEN_PLACEHOLDER))?;
        return Ok(Printed::Document(doc));
    }
    read_preview(ctx, client, settings, invite).map(Printed::Document)
}

/// Sends the preview and returns its answer, with the token redacted from the answer or the
/// error wherever the server repeated it. The invite token goes to an origin other than the
/// default only once the caller confirms it, as `hipc auth login` asks before it binds the API
/// token to one (tenet 8). `settings` are the ones `client` was built from, so the origin
/// asked about is the one the request goes to.
fn read_preview(
    ctx: &Context<'_>,
    client: &Client,
    settings: &Settings,
    invite: &InviteToken,
) -> Result<Value> {
    let target = Given {
        value: settings.api_url.clone(),
        source: settings.api_url_source.clone(),
    };
    ctx.confirm_origin(
        &target,
        "send the invite token to",
        "whoever answers there holds the invite token",
    )?;
    ctx.send(client, &preview_request(&invite.0))
        .and_then(|response| response.document())
        .map(|offer| redact_value(offer, &invite.0))
        .map_err(|error| error.redacted(&invite.0))
}

/// `hipc account invites accept`: previews the invite, so the confirmation names the account
/// to join (the tenet 5 exception for a read that names the target), then accepts it. The
/// preview goes out after the accept's refusals and the token check, so a stored token
/// withheld from this origin stops it, and after the preview's own origin question. A dry
/// run sends neither: its document is the accept, and its `requests` lists the preview, then
/// the accept.
fn accept(
    ctx: &Context<'_>,
    client: &Client,
    settings: &Settings,
    invite: &InviteToken,
) -> Result<Printed> {
    let draft = accept_request(TOKEN_PLACEHOLDER);
    let checked = match ctx.check_write(client, &draft)? {
        Write::DryRun(mut doc) => {
            let preview = preview_request(TOKEN_PLACEHOLDER);
            doc["requests"] = ctx.describe_each(client, &[&preview, &draft])?;
            return Ok(Printed::Document(doc));
        }
        Write::Checked(checked) => checked,
    };
    let offer = read_preview(ctx, client, settings, invite)?;
    // The preview says when an invite can no longer be used (`valid`, with `reason`
    // `accepted`, `revoked`, or `expired`), so the person is not asked to join an account
    // the accept would refuse.
    if offer.get("valid") == Some(&Value::Bool(false)) {
        let why = match offer.get("reason").and_then(Value::as_str) {
            Some(reason) => format!("the API says it is {}", phrase::quote(reason)),
            None => "the API says it is not valid".to_owned(),
        };
        return Err(Error::new(
            Code::Conflict,
            format!("cannot accept the invite: {why}"),
        ));
    }
    let Some(account) = offer.get("account_name").and_then(Value::as_str) else {
        return Err(Error::internal(
            "the invite preview has no `account_name`, so the confirmation cannot name the \
             account to join",
        ));
    };
    let action = match offer.get("invited_by").and_then(Value::as_str) {
        Some(by) => format!(
            "join the account {}, invited by {}",
            phrase::quote(account),
            phrase::quote(by)
        ),
        None => format!("join the account {}", phrase::quote(account)),
    };
    let request = accept_request(&invite.0).about(action);
    let consequence = "the token then acts in that account with the access the invite grants";
    ctx.confirm_write(checked, &request.action(), Some(consequence))?;
    let joined = ctx
        .send(client, &request)
        .and_then(|response| response.document())
        .map(|joined| redact_value(joined, &invite.0))
        .map_err(|error| with_accept_hint(error.redacted(&invite.0)))?;
    use_note(ctx, &joined);
    Ok(Printed::Document(joined))
}

/// The note after an invite is accepted: the `hipc account use` that makes later commands act
/// in the joined account. The answer is `{"account": <entry>}`, and its id
/// is named only when `hipc account use` takes it as an account id; otherwise there is no
/// note. The command does not repeat the account this run acts in, which is the one being
/// left. `HIPPIUS_ACCOUNT` wins over the profile's pick, so while it is set the note says it
/// still selects that account; a `--account` flag lasts one run and needs no such word.
///
/// It runs after the accept, so it never fails: an error here would report an accepted
/// invite as a failure. Reading the flags and the environment can only fail as it already
/// did before the accept, so a failure skips the note instead.
fn use_note(ctx: &Context<'_>, joined: &Value) {
    let Some(id) = joined.pointer("/account/id").and_then(Value::as_str) else {
        return;
    };
    let Ok(Selection::Member(id)) = Selection::parse(id, &Source::Argument) else {
        return;
    };
    let words = format!("account use {}", id.as_str());
    let (Ok(Some(command)), Ok(overrides)) =
        (ctx.next_command_without_account(&words), ctx.overrides())
    else {
        return;
    };
    let note = format!("run `{command}` to act in the joined account");
    match overrides.account {
        Some(Given {
            value,
            source: Source::Variable(name),
        }) => ctx.note(&format!(
            "{note}; `{name}` selects `{value}` until it is unset"
        )),
        _ => ctx.note(&note),
    }
}

/// `error` with the hint for the two refusals the console explains: a 403 `email_mismatch`, an
/// invite sent to an address the token's user has not verified, and a 409
/// `email_confirmation_required`, which the console answers by asking the person and sending
/// `confirm_without_email: true`.
fn with_accept_hint(error: Error) -> Error {
    let code = error
        .details()
        .and_then(|details| details.get("code"))
        .and_then(Value::as_str);
    let hint = match (error.http_status(), code) {
        (Some(403), Some("email_mismatch")) => {
            "run `hipc auth login --token-file <path>` with a token of the user the invite was \
             emailed to"
        }
        (Some(409), Some("email_confirmation_required")) => {
            "accept the invite in the console, which asks before it joins a user whose email \
             is not verified"
        }
        _ => return error,
    };
    error.with_hint(hint)
}

/// `PATCH /api/accounts/members/<id>/` with only the keys given. `grants` replaces the
/// whole list on the server, so it is sent only when a `--grant` is given, and then it
/// holds every grant given: the current grants are never merged. They are read first,
/// unless `--remove-others` is passed, so a grant the member has now that the `--grant`s
/// leave out is refused ([`replace_list`]).
fn update_member(ctx: &Context<'_>, args: &MemberUpdate) -> Result<Printed> {
    let mut body = Map::new();
    let mut changes = Vec::new();
    let grants = grant_list(&args.access.grants)?;
    if let Some(grants) = &grants {
        changes.push(if grants.is_empty() {
            "remove every grant".to_owned()
        } else {
            format!(
                "replace every grant with {}",
                phrase::join(&labels(grants), "and")
            )
        });
        body.insert("grants".to_owned(), grants_json(grants));
    }
    for (key, value) in permissions(&args.access) {
        if let Some(value) = value {
            changes.push(format!("set `{key}` to `{value}`"));
            body.insert(key.to_owned(), value.into());
        }
    }
    if body.is_empty() {
        // An empty PATCH changes nothing and only returns the member.
        return Err(Error::usage(
            "`hipc account members update` was given nothing to change",
            "pass `--grant`, `--manage-team`, or `--billing-permission`",
        ));
    }
    let id = args.member_id;
    let request = Request::new(Method::Patch, format!("/accounts/members/{id}/"))
        .listed_by(MEMBERS)
        .json(&Value::Object(body))
        .about(format!(
            "update the member `{id}`: {}",
            phrase::join(&changes, "and")
        ));
    let client = ctx.load_authed_client()?;
    let gate = Gate::Confirm { consequence: None };
    let Some(grants) = grants else {
        return print_body(ctx, &client, &request, gate);
    };
    let replace = Replace {
        write: request,
        consequence: None,
        entries: grants
            .iter()
            .map(|g| Entry::new(grant_text(g.scope, g.level, g.target.key_and_id()).into()))
            .collect(),
        remove_others: args.remove_others,
        // There is no read of one member: the list holds every member's grants.
        read: members_list(),
        draft_body: None,
        // A member has no version and the PATCH takes no `If-Match`, so a change another
        // admin makes between the read and the PATCH is written over unseen: a grant revoked
        // in that window comes back when `--grant` names it. The help says so; send
        // `If-Match` here once the route takes one.
        draft_if_match: None,
        noun: "grant",
        owner: format!("the member `{id}`"),
    };
    replace_list(ctx, &client, &replace, |answer| {
        member_grants(&answer, id)
            .ok_or_else(|| crate::commands::unreadable(&members_list().action()))?
            .map(Found::new)
    })
}

/// A grant as one text, `scope=level` or `scope=level@id` for one on the project or billing
/// account `target` names: the entry the guard compares, and the label a confirmation shows,
/// so the two cannot drift. A new level for a scope removes the grant at the old level, so a
/// lower level is refused as a removal too. The id is part of the entry: without it, an
/// account-wide grant would read as the same entry as one limited to a project, and
/// replacing the one with the other would widen the member's access with the guard silent.
fn grant_text(scope: &str, level: &str, target: Option<(&str, &str)>) -> String {
    match target {
        None => format!("{scope}={level}"),
        Some((_, id)) => format!("{scope}={level}@{id}"),
    }
}

/// The grants the member `id` has now, from the answer of `GET /api/accounts/members/`
/// (`{members, owner}`); `None` for an answer of another shape.
///
/// A member the list does not hold is `not_found`, with no PATCH; one missing from a list
/// with a `next` link is `internal`, as the member may be on a later page. Sending the PATCH
/// unguarded would let the guard turn off without a sound if the list keyed its members
/// another way than the route does (the item shape comes from the console's types, not
/// a live list).
fn member_grants(answer: &Value, id: u64) -> Option<Result<Vec<Entry>>> {
    let members = answer.get("members")?.as_array()?;
    let Some(member) = members
        .iter()
        .find(|member| member.get("id").and_then(Value::as_u64) == Some(id))
    else {
        // Missing from a page that is not the last says nothing about a later page.
        let links = hippius::page_beside(Vec::new(), answer.as_object()?.clone());
        if let Err(error) = links.and_then(|links| links.into_complete(MEMBERS_PATH)) {
            return Some(Err(error));
        }
        let missing = Error::new(Code::NotFound, format!("no member with id `{id}`"));
        return Some(Err(
            missing.with_hint(format!("run `{MEMBERS}` to see what exists"))
        ));
    };
    let grants = member
        .get("grants")?
        .as_array()?
        .iter()
        .map(|grant| {
            // A grant read wrongly as account-wide would let a new list widen it with the
            // guard silent, so one is read only in the shape the guard knows: `scope` and
            // `level` text, and `project` or `billing` absent, null, or text (not both),
            // with no other key, such as a `project_id` the guard would not see.
            let fields = grant.as_object()?;
            if fields
                .keys()
                .any(|key| !matches!(key.as_str(), "scope" | "level" | "project" | "billing"))
            {
                return None;
            }
            let text = |key| fields.get(key).and_then(Value::as_str);
            let mut target = None;
            for key in ["project", "billing"] {
                match fields.get(key) {
                    None | Some(Value::Null) => {}
                    // A grant on both a project and a billing account is not one the
                    // guard can name.
                    Some(Value::String(_)) if target.is_some() => return None,
                    Some(Value::String(id)) => target = Some((key, id.as_str())),
                    Some(_) => return None,
                }
            }
            Some(Entry::new(
                grant_text(text("scope")?, text("level")?, target).into(),
            ))
        })
        .collect::<Option<Vec<Entry>>>()?;
    Some(Ok(grants))
}

/// `POST /api/accounts/invites/` with all four keys, as the console sends them: no
/// `--grant` is an empty list, and a permission not given is `false`.
fn create_invite(ctx: &Context<'_>, args: &InviteCreate) -> Result<Printed> {
    let grants = grant_list(&args.access.grants)?.unwrap_or_default();
    let mut body = Map::new();
    body.insert("email".to_owned(), args.email.as_str().into());
    body.insert("grants".to_owned(), grants_json(&grants));
    let mut given = labels(&grants);
    for (key, value) in permissions(&args.access) {
        let value = value.unwrap_or(false);
        if value {
            given.push(format!("`{key}`"));
        }
        body.insert(key.to_owned(), value.into());
    }
    let granted = if given.is_empty() {
        "nothing".to_owned()
    } else {
        phrase::join(&given, "and")
    };
    let request = Request::new(Method::Post, "/accounts/invites/")
        .json(&Value::Object(body))
        .about(format!(
            "email an invite to `{}` that grants {granted}",
            args.email
        ));
    let client = ctx.load_authed_client()?;
    print_body(ctx, &client, &request, Gate::Confirm { consequence: None })
}

/// The grant list the `--grant`s give: `None` when there is none, so the key is not sent,
/// and empty for `--grant none`.
///
/// # Errors
///
/// `usage` for `none` beside another `--grant`, or a scope given twice for the same
/// account, project, or billing account: either would leave the caller guessing which list
/// the server keeps.
fn grant_list(args: &[GrantArg]) -> Result<Option<Vec<Grant>>> {
    let mut grants: Vec<Grant> = Vec::new();
    for arg in args {
        match arg {
            GrantArg::NoGrants if args.len() == 1 => {}
            GrantArg::NoGrants => {
                return Err(Error::usage(
                    "`--grant none` cannot be combined with another `--grant`",
                    "pass `--grant none` alone to remove every grant",
                ));
            }
            GrantArg::Grant(grant) => {
                if grants
                    .iter()
                    .any(|g| g.scope == grant.scope && g.target == grant.target)
                {
                    let on = match grant.target.key_and_id() {
                        None => String::new(),
                        Some((_, id)) => format!(" on `{id}`"),
                    };
                    return Err(Error::usage(
                        format!(
                            "`--grant` names the scope `{}`{on} more than once",
                            grant.scope
                        ),
                        "pass each scope once",
                    ));
                }
                grants.push(grant.clone());
            }
        }
    }
    Ok((!args.is_empty()).then_some(grants))
}

/// The two permissions, by their body keys.
fn permissions(access: &Access) -> [(&'static str, Option<bool>); 2] {
    [
        ("can_manage_team", access.manage_team),
        ("can_billing", access.billing_permission),
    ]
}

/// Each grant as the console sends it: `scope` and `level`, and `project` or `billing`
/// for a grant on one of them.
fn grants_json(grants: &[Grant]) -> Value {
    grants
        .iter()
        .map(|g| {
            let mut grant = json!({ "scope": g.scope, "level": g.level });
            if let Some((key, id)) = g.target.key_and_id() {
                grant[key] = id.into();
            }
            grant
        })
        .collect()
}

/// Each grant as `` `scope=level` `` or `` `scope=level@id` ``, for a confirmation.
fn labels(grants: &[Grant]) -> Vec<String> {
    grants
        .iter()
        .map(|g| phrase::quote(grant_text(g.scope, g.level, g.target.key_and_id())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_invite_token_debugs_as_present_without_its_text() {
        let token = InviteToken(Zeroizing::new("inv_secret".to_owned()));
        assert_eq!(format!("{token:?}"), "InviteToken(redacted)");
    }
}
