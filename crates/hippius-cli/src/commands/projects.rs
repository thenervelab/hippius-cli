//! `hipc account projects` and `hipc account billing-accounts`: the ids `--project` and
//! `--billing` take, a billing account's costs, and the selection stored in the profile.
//!
//! The routes are under `/api/accounts/`, so a membership sends `X-Hippius-Account`, and
//! they take neither selector: the accounts API is the account's own. A selected project or
//! billing account refuses them, and `--project default` or `--billing default` reads them.
//! Swagger gives none of them an answer shape, and none of the writes a body; the shapes
//! `hipc` expects and sends are the ones the console reads and sends.
//!
//! The project transfers, `/api/accounts/project-transfers/`, are personal: they are the
//! token's own account's whichever account is selected, so they never take the header.

use hippius::{
    BillingId, Client, Code, ConfigFile, Error, Method, ProjectId, Request, Result, Selection,
    Source, phrase,
};
use serde_json::{Map, Value, json};

use crate::cli::{
    BillingAccounts, Budget, BudgetUpdate, Costs, ProjectTransfer, ProjectTransfers, Projects,
    Quota, QuotaLimit, QuotaUpdate,
};
use crate::commands::account;
use crate::commands::credits::{AmountPath, shown_in_credits};
use crate::commands::{
    Printed, keyed_list, print_body, print_body_or, print_keyed_list, print_with, store_selection,
};
use crate::context::{Context, Gate, Write};
use crate::output::Mode;

/// The command that lists the billing account ids, for the hint of a 404.
const BILLING_ACCOUNTS: &str = "hipc account billing-accounts list";

/// The command that lists the project ids, for the hint of a 404.
const PROJECTS: &str = "hipc account projects list";

/// The command that lists the transfer ids, for the hint of a 404.
const TRANSFERS: &str = "hipc account project-transfers list";

/// The most alert thresholds a budget takes: the console's limit.
const MAX_ALERTS: usize = 5;

/// `hipc account projects`.
pub(crate) fn projects(ctx: &Context<'_>, command: &Projects) -> Result<Printed> {
    match command {
        Projects::List => print_keyed_list(
            ctx,
            &ctx.load_authed_client()?,
            &Request::get("/accounts/projects/").about("list the projects"),
            "projects",
        ),
        Projects::Create(args) => {
            // The console sends `billing: null` for the Default billing account.
            let request = Request::new(Method::Post, "/accounts/projects/")
                .json(&json!({ "name": args.name, "billing": null }))
                .about(format!("create the project {}", phrase::quote(&args.name)));
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        Projects::Update(args) => {
            let request = Request::new(
                Method::Patch,
                format!("/accounts/projects/{}/", args.project_id),
            )
            .json(&json!({ "name": args.name }))
            .listed_by(PROJECTS)
            .about(format!(
                "rename the project `{}` to {}",
                args.project_id,
                phrase::quote(&args.name)
            ));
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        Projects::Delete { project_id } => {
            let request = Request::new(Method::Delete, format!("/accounts/projects/{project_id}/"))
                .listed_by(PROJECTS)
                .about(format!(
                    "archive the project `{project_id}`, which must own nothing"
                ));
            let done = json!({ "id": project_id, "archived": true });
            let config = crate::context::config()?;
            let (settings, client) = ctx.authed(&config)?;
            let printed = print_body_or(
                ctx,
                &client,
                &request,
                Gate::Confirm { consequence: None },
                done,
            )
            .map_err(|error| still_holds(error, project_id))?;
            let stored = settings
                .project
                .as_ref()
                .is_some_and(|p| p.is_stored() && p.as_str() == project_id.as_str());
            if stored && !ctx.dry_run() {
                forget_archived(ctx, &config, &settings.profile, project_id);
            }
            Ok(printed)
        }
        Projects::Transfer(args) => transfer(ctx, args),
        Projects::Budget(command) => budget(ctx, command),
        Projects::Quota(command) => quota(ctx, command),
        Projects::Use { project_id } => {
            let project = ProjectId::parse(project_id, &Source::Argument)?;
            let shown = project.as_ref().map_or(hippius::DEFAULT, ProjectId::as_str);
            store_selection(
                ctx,
                "select_project",
                ("project", shown),
                ConfigFile::selection_profile,
                |config, profile| {
                    config.set_project(profile, project.as_ref());
                },
            )
        }
    }
}

/// Clears the stored project `project_id` of `profile` once it is archived, as `hipc account
/// projects use default` would: every later command would otherwise act in a project that is
/// gone. The archive is done whatever happens here, so a failure is a warning, not an error.
/// Only `hipc account projects delete` clears it: a `hipc api DELETE` of the project does not.
///
/// `config` was read before the archive, so the stored project is compared again under the
/// file's lock: a `hipc account projects use` that another run stored meanwhile is kept.
fn forget_archived(ctx: &Context<'_>, config: &ConfigFile, profile: &str, project_id: &str) {
    let cleared = ConfigFile::edit(config.path().to_path_buf(), |config| {
        Ok(config.clear_project_if(profile, project_id))
    });
    match cleared {
        Ok(false) => {}
        Ok(true) => ctx.note(&format!(
            "the archived project `{project_id}` was the stored selection, so it is cleared: \
             commands now act in the Default project"
        )),
        Err(error) => ctx.warning(&format!(
            "the archived project `{project_id}` is still the stored selection, since {}: run \
             `hipc account projects use default`",
            error.message()
        )),
    }
}

/// `hipc account billing-accounts`.
pub(crate) fn billing_accounts(ctx: &Context<'_>, command: &BillingAccounts) -> Result<Printed> {
    match command {
        BillingAccounts::List => print_keyed_list(
            ctx,
            &ctx.load_authed_client()?,
            &Request::get("/accounts/billing-accounts/").about("list the billing accounts"),
            "billing_accounts",
        ),
        BillingAccounts::Create { name } => {
            let request = Request::new(Method::Post, "/accounts/billing-accounts/")
                .json(&json!({ "name": name }))
                .about(format!(
                    "create the billing account {}",
                    phrase::quote(name)
                ));
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        BillingAccounts::Update {
            billing_account_id,
            name,
        } => {
            let request = Request::new(
                Method::Patch,
                format!("/accounts/billing-accounts/{billing_account_id}/"),
            )
            .json(&json!({ "name": name }))
            .listed_by(BILLING_ACCOUNTS)
            .about(format!(
                "rename the billing account `{billing_account_id}` to {}",
                phrase::quote(name)
            ));
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        BillingAccounts::Costs(args) => costs(ctx, args),
        BillingAccounts::Use { billing_account_id } => {
            let billing = BillingId::parse(billing_account_id, &Source::Argument)?;
            let shown = billing.as_ref().map_or(hippius::DEFAULT, BillingId::as_str);
            store_selection(
                ctx,
                "select_billing",
                ("billing", shown),
                ConfigFile::selection_profile,
                |config, profile| {
                    config.set_billing(profile, billing.as_ref());
                },
            )
        }
    }
}

/// `GET /api/accounts/billing-accounts/<id>/costs/` with the console's query names: `from`
/// and `to`, and `group` as a comma list in the console's order. The query sends only
/// those three, each only when given, so the server's defaults apply.
fn costs(ctx: &Context<'_>, args: &Costs) -> Result<Printed> {
    let mut request = Request::get(format!(
        "/accounts/billing-accounts/{}/costs/",
        args.billing_account_id
    ))
    .about(format!(
        "get the costs of the billing account `{}`",
        args.billing_account_id
    ))
    .listed_by(BILLING_ACCOUNTS);
    if let Some(from) = &args.from {
        request = request.query("from", from);
    }
    if let Some(to) = &args.to {
        request = request.query("to", to);
    }
    if !args.group_by.is_empty() {
        let mut groups = args.group_by.clone();
        groups.sort();
        groups.dedup();
        let groups: Vec<&str> = groups.iter().map(|group| group.as_str()).collect();
        request = request.query("group", &groups.join(","));
    }
    print_with(
        ctx,
        &ctx.load_authed_client()?,
        &request,
        Gate::Open,
        |response| Ok(shown_in_credits(ctx, response.document()?, COST_AMOUNTS)),
    )
}

/// The base-unit amounts of a costs answer, which text mode shows in credits: credit base
/// units, 10^18 to 1 credit, as the console reads them.
const COST_AMOUNTS: &[AmountPath] = &[&["total"], &["rows", "*", "amount"]];

/// `POST /api/accounts/projects/<id>/transfer/` with the console's body, `{to_account}`.
/// The question names the project and both accounts, so the projects are read first (tenet
/// 5 allows a read that only names the target), and, when the token's own account is the
/// one the project leaves, the accounts too, for its id. A selected membership is named by
/// [`Context::confirm_write`]. A project that is not in the account, or an offer to the
/// account the project is already in, is refused there, with nothing else sent. A dry run
/// sends none of them and lists them all.
fn transfer(ctx: &Context<'_>, args: &ProjectTransfer) -> Result<Printed> {
    let client = ctx.load_authed_client()?;
    let member = match client.account() {
        Selection::Own => None,
        Selection::Member(id) => Some(id.as_str().to_owned()),
    };
    // Decided from the command line alone, so the invocation must change: `usage`.
    if member.as_deref() == Some(args.to_account.as_str()) {
        return Err(Error::usage(
            same_account(&args.to_account),
            SAME_ACCOUNT_HINT,
        ));
    }
    let write = |project: &str, from: &str| {
        Request::new(
            Method::Post,
            format!("/accounts/projects/{}/transfer/", args.project_id),
        )
        .json(&json!({ "to_account": args.to_account }))
        .listed_by(PROJECTS)
        .about(format!(
            "offer the project {project} to the account `{}`{from}",
            args.to_account
        ))
    };
    let own = " from the token's own account";
    let shown_id = format!("`{}`", args.project_id);
    let draft = write(&shown_id, if member.is_some() { "" } else { own });
    let projects = Request::get("/accounts/projects/")
        .about("list the projects")
        .listed_by(PROJECTS);
    let accounts = Request::get("/accounts/").about("list the accounts the token can act in");
    let checked = match ctx.check_write(&client, &draft)? {
        Write::DryRun(mut doc) => {
            let reads: Vec<&Request> = if member.is_some() {
                vec![&projects, &draft]
            } else {
                vec![&projects, &accounts, &draft]
            };
            doc["requests"] = ctx.describe_each(&client, &reads)?;
            return Ok(Printed::Document(doc));
        }
        Write::Checked(checked) => checked,
    };
    let list = keyed_list(ctx.send(&client, &projects)?.json()?, &projects, "projects")?;
    let Some(project) = list["items"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|project| project.get("id").and_then(Value::as_str) == Some(&args.project_id))
    else {
        let account = member.as_ref().map_or_else(
            || "the token's own account".to_owned(),
            |id| format!("the account `{id}`"),
        );
        // Decided by what the server listed, not by the command line (tenet 4).
        let missing = Error::new(
            Code::NotFound,
            format!(
                "the project `{}` is not one of the projects of {account}",
                args.project_id
            ),
        );
        return Err(missing.with_hint(format!("run `{PROJECTS}` to see the project ids")));
    };
    let named = match project.get("name").and_then(Value::as_str) {
        Some(name) => format!("{} ({shown_id})", phrase::quote(name)),
        None => shown_id,
    };
    let from = if member.is_some() {
        String::new()
    } else {
        let id = own_account_id(&ctx.send(&client, &accounts)?.json()?)?;
        // A state `hipc` read that rules the write out (tenet 4): `conflict`.
        if id == args.to_account {
            let same = Error::new(Code::Conflict, same_account(&args.to_account));
            return Err(same.with_hint(SAME_ACCOUNT_HINT));
        }
        format!("{own} {}", phrase::quote(&id))
    };
    let request = write(&named, &from);
    ctx.confirm_write(
        checked,
        &request.action(),
        Some("the project moves to that account once it accepts"),
    )?;
    ctx.send(&client, &request)?
        .document()
        .map(Printed::Document)
}

/// The token's own account's id, from the account the answer marks with `is_owner`
/// ([`account::own_account`]), not from its place in the list.
///
/// # Errors
///
/// `internal` when no one account is marked, or the one marked has no string `id`: the
/// offer's source is then unknown, so the offer is not sent, since the check against
/// `--to-account` and the question would rest on a guess.
fn own_account_id(answer: &Value) -> Result<String> {
    let accounts = answer
        .get("accounts")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let own = account::own_account(accounts).ok_or_else(account::no_own_account)?;
    accounts
        .get(own)
        .and_then(|account| account.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::internal("the account the API marks as the token's own has no `id`"))
}

/// The message of the refusal of an offer to the account the project is already in.
fn same_account(account: &str) -> String {
    format!("`--to-account {account}` is the account the project is in")
}

/// The hint of that refusal.
const SAME_ACCOUNT_HINT: &str = "pass the id of the account to offer the project to";

/// The most of a project's resources a refused archive names; the rest are counted.
const MAX_HELD: usize = 5;

/// A refused archive, `409` with the console's `{code: project_not_empty, resources}`,
/// reworded to name what the project still holds, each as the console names it, `{kind,
/// id, name}`, so the caller knows what to remove; `details` keeps the whole body. The rest
/// are counted, those without a kind or an id among them. Any other error, a `409` with
/// another code, or one whose resources `hipc` can name none of, is returned as it is.
fn still_holds(error: Error, project_id: &str) -> Error {
    let details = error.details();
    let code = details
        .and_then(|details| details.get("code"))
        .and_then(Value::as_str);
    if error.http_status() != Some(409) || code != Some("project_not_empty") {
        return error;
    }
    let resources = details
        .and_then(|details| details.get("resources"))
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let mut shown: Vec<String> = resources
        .iter()
        .filter_map(resource)
        .take(MAX_HELD)
        .collect();
    if shown.is_empty() {
        return error;
    }
    let rest = resources.len() - shown.len();
    if rest > 0 {
        shown.push(format!("{rest} more"));
    }
    let message = format!(
        "cannot archive the project `{project_id}`: it still holds {}",
        phrase::join(&shown, "and")
    );
    error
        .with_message(message)
        .with_hint("delete or move what the project holds, then archive it again")
}

/// One resource of a `project_not_empty` answer, as "the VM `web` (`vm-1`)": its kind in
/// words where the console names it, else quoted, then its name and id. `None` for one
/// without a kind or an id.
fn resource(resource: &Value) -> Option<String> {
    let kind = resource.get("kind")?.as_str()?;
    let id = match resource.get("id")? {
        Value::String(id) => id.clone(),
        Value::Number(id) => id.to_string(),
        _ => return None,
    };
    let noun = match kind {
        "vm" => "VM".to_owned(),
        "database" => "database".to_owned(),
        "kubernetes" => "Kubernetes cluster".to_owned(),
        "public_ip" => "public IP".to_owned(),
        "cdn_zone" => "CDN zone".to_owned(),
        "bucket" => "bucket".to_owned(),
        "s3_master_token" => "S3 master token".to_owned(),
        "s3_sub_token" => "S3 sub-token".to_owned(),
        "runner_installation" => "runner installation".to_owned(),
        "runner_job" => "runner job".to_owned(),
        other => format!("resource of kind {}", phrase::quote(other)),
    };
    Some(match resource.get("name").and_then(Value::as_str) {
        Some(name) => format!(
            "the {noun} {} ({})",
            phrase::quote(name),
            phrase::quote(&id)
        ),
        None => format!("the {noun} {}", phrase::quote(&id)),
    })
}

/// `/api/accounts/projects/<id>/budget/`: `GET`, `PUT`, and `DELETE`.
fn budget(ctx: &Context<'_>, command: &Budget) -> Result<Printed> {
    match command {
        Budget::Get { project_id } => {
            let request = Request::get(format!("/accounts/projects/{project_id}/budget/"))
                .listed_by(PROJECTS)
                .about(format!("get the budget of the project `{project_id}`"));
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        Budget::Update(args) => budget_update(ctx, args),
        Budget::Delete { project_id } => {
            let request = Request::new(
                Method::Delete,
                format!("/accounts/projects/{project_id}/budget/"),
            )
            .listed_by(PROJECTS)
            .about(format!(
                "remove the monthly budget of the project `{project_id}`, with its alerts and \
                 any launch block"
            ));
            let done = json!({ "project": project_id, "deleted": true });
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

/// The budget `PUT`, the console's `{monthly_limit, thresholds, block_launch_over}`, whole:
/// it replaces the budget, so nothing is read or merged. The thresholds are sent sorted, as
/// the console sends them, after the console's checks: at most five, each once.
fn budget_update(ctx: &Context<'_>, args: &BudgetUpdate) -> Result<Printed> {
    let mut thresholds = args.alert_at.clone();
    thresholds.sort_unstable();
    if thresholds.len() > MAX_ALERTS {
        return Err(Error::usage(
            format!(
                "`--alert-at` is given {} times, and a budget takes at most {MAX_ALERTS}",
                thresholds.len()
            ),
            format!("pass `--alert-at` at most {MAX_ALERTS} times"),
        ));
    }
    if let Some(&[twice, _]) = thresholds.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(Error::usage(
            format!("`--alert-at {twice}` is given more than once"),
            "pass each `--alert-at` value once",
        ));
    }
    let limit = args.monthly_limit.to_string();
    let shown: Vec<String> = thresholds.iter().map(|t| format!("`{t}`")).collect();
    let launches = if args.block_launches {
        "refusing new launches over it"
    } else {
        "allowing new launches over it"
    };
    let request = Request::new(
        Method::Put,
        format!("/accounts/projects/{}/budget/", args.project_id),
    )
    .json(&json!({
        "monthly_limit": limit,
        "thresholds": thresholds,
        "block_launch_over": args.block_launches,
    }))
    .listed_by(PROJECTS)
    .about(format!(
        "set the monthly budget of the project `{}` to `{limit}` USD, mailing at {} percent \
         of it and {launches}",
        args.project_id,
        phrase::join(&shown, "and")
    ));
    print_body(
        ctx,
        &ctx.load_authed_client()?,
        &request,
        Gate::Confirm { consequence: None },
    )
}

/// `/api/accounts/projects/<id>/quota/`: `GET`, and the `PATCH` of `{limits}` with only the
/// limits given, which the server merges, as the console sends it.
fn quota(ctx: &Context<'_>, command: &Quota) -> Result<Printed> {
    match command {
        Quota::Get { project_id } => {
            let request = Request::get(format!("/accounts/projects/{project_id}/quota/"))
                .listed_by(PROJECTS)
                .about(format!("get the quota of the project `{project_id}`"));
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        Quota::Update(args) => quota_update(ctx, args),
    }
}

fn quota_update(ctx: &Context<'_>, args: &QuotaUpdate) -> Result<Printed> {
    let mut limits = Map::new();
    let mut shown = Vec::new();
    for (key, limit) in args.limits() {
        let (value, text) = match limit {
            QuotaLimit::At(n) => (Value::from(n), format!("`{key}` to `{n}`")),
            QuotaLimit::None => (Value::Null, format!("`{key}` to none of its own")),
        };
        limits.insert(key.to_owned(), value);
        shown.push(text);
    }
    let request = Request::new(
        Method::Patch,
        format!("/accounts/projects/{}/quota/", args.project_id),
    )
    .json(&json!({ "limits": limits }))
    .listed_by(PROJECTS)
    .about(format!(
        "set the compute limits of the project `{}`: {}",
        args.project_id,
        phrase::join(&shown, "and")
    ));
    print_body(
        ctx,
        &ctx.load_authed_client()?,
        &request,
        Gate::Confirm { consequence: None },
    )
}

/// `hipc account project-transfers`. Every route is personal, so a membership sends no
/// header and the question names the token's own account.
pub(crate) fn project_transfers(ctx: &Context<'_>, command: &ProjectTransfers) -> Result<Printed> {
    let client = ctx.load_authed_client()?;
    match command {
        ProjectTransfers::List => {
            print_with(ctx, &client, &transfers_read(), Gate::Open, |response| {
                let envelope = transfers_envelope(response.json()?)?;
                Ok(match ctx.mode() {
                    Mode::Text => transfers_for_a_person(envelope),
                    Mode::Json => envelope,
                })
            })
        }
        ProjectTransfers::Accept {
            transfer_id,
            bill_to,
        } => {
            let billing = match bill_to {
                Some(text) => BillingId::parse(text, &Source::Argument)?,
                None => None,
            };
            let (body, billed) = match &billing {
                Some(id) => (
                    json!({ "billing": id.as_str() }),
                    format!("the billing account `{}`", id.as_str()),
                ),
                None => (
                    json!({ "billing": null }),
                    "the Default billing account".to_owned(),
                ),
            };
            let write = |named: &str| {
                Request::new(
                    Method::Post,
                    format!("/accounts/project-transfers/{transfer_id}/accept/"),
                )
                .json(&body)
                .about(format!(
                    "accept the project transfer {named} into the token's own account, billed \
                     to {billed}"
                ))
            };
            decide(ctx, &client, (*transfer_id, Side::Incoming), write, None)
        }
        ProjectTransfers::Decline { transfer_id } => {
            let write = |named: &str| {
                Request::new(
                    Method::Post,
                    format!("/accounts/project-transfers/{transfer_id}/decline/"),
                )
                .about(format!("decline the project transfer {named}"))
            };
            let done = json!({ "id": transfer_id, "declined": true });
            decide(
                ctx,
                &client,
                (*transfer_id, Side::Incoming),
                write,
                Some(done),
            )
        }
        ProjectTransfers::Cancel { transfer_id } => {
            let write = |named: &str| {
                Request::new(
                    Method::Delete,
                    format!("/accounts/project-transfers/{transfer_id}/"),
                )
                .about(format!("cancel the project transfer {named}"))
            };
            let done = json!({ "id": transfer_id, "cancelled": true });
            decide(
                ctx,
                &client,
                (*transfer_id, Side::Outgoing),
                write,
                Some(done),
            )
        }
    }
}

/// `GET /api/accounts/project-transfers/`: the list, and the read before a decision.
fn transfers_read() -> Request {
    Request::get(TRANSFERS_PATH).about("list the project transfers")
}

/// The project transfers route. Swagger documents no page for it, so it is read whole.
const TRANSFERS_PATH: &str = "/accounts/project-transfers/";

/// Which of the token's own account's transfers a decision acts on: an accept or a decline
/// one offered to it, a cancel one it offered.
#[derive(Debug, Clone, Copy)]
enum Side {
    Incoming,
    Outgoing,
}

/// A decision on the transfer `id`: the transfers are read first (tenet 5 allows a read
/// that only names the target), so the question names the project and the other account,
/// and an id that is not on `side` is refused there, with nothing else sent. `write` builds
/// the request with the transfer as the question names it. A dry run sends neither and lists
/// both. `done` is printed when the server answers with no body; without it the answer is
/// printed as sent.
fn decide(
    ctx: &Context<'_>,
    client: &Client,
    (id, side): (u64, Side),
    write: impl Fn(&str) -> Request,
    done: Option<Value>,
) -> Result<Printed> {
    let read = transfers_read();
    let draft = write(&format!("`{id}`")).listed_by(TRANSFERS);
    let checked = match ctx.check_write(client, &draft)? {
        Write::DryRun(mut doc) => {
            doc["requests"] = ctx.describe_each(client, &[&read, &draft])?;
            return Ok(Printed::Document(doc));
        }
        Write::Checked(checked) => checked,
    };
    let lists = transfer_lists(ctx.send(client, &read)?.json()?)?;
    let (list, other, missing) = match side {
        Side::Incoming => (
            lists.incoming,
            ("from", "from_account"),
            format!("the project transfer `{id}` is not one offered to the token's own account"),
        ),
        Side::Outgoing => (
            lists.outgoing,
            ("to", "to_account"),
            format!("the project transfer `{id}` is not one the token's own account offered"),
        ),
    };
    let Some(transfer) = list
        .iter()
        .find(|transfer| transfer.get("id").and_then(Value::as_u64) == Some(id))
    else {
        // Missing from a page that is not the last says nothing about a later page.
        lists.links.into_complete(TRANSFERS_PATH)?;
        // Decided by what the server listed, not by the command line (tenet 4).
        let missing = Error::new(Code::NotFound, missing);
        return Err(missing.with_hint(format!("run `{TRANSFERS}` to see the transfer ids")));
    };
    let mut named = format!("`{id}`");
    if let Some(project) = principal(transfer.get("project")) {
        named.push_str(&format!(" of the project {project}"));
    }
    if let Some(account) = principal(transfer.get(other.1)) {
        named.push_str(&format!(" {} the account {account}", other.0));
    }
    let request = write(&named).listed_by(TRANSFERS);
    ctx.confirm_write(checked, &request.action(), None)?;
    let response = ctx.send(client, &request)?;
    match done {
        Some(done) => {
            let body = response.json()?;
            Ok(Printed::Document(if body.is_null() { done } else { body }))
        }
        None => response.document().map(Printed::Document),
    }
}

/// A project or account as a transfer names it, `{id, name}`, for a message: its name and
/// id in backticks, or whichever of the two it has; `None` with neither.
fn principal(value: Option<&Value>) -> Option<String> {
    principal_as(value, |text| phrase::quote(text))
}

/// [`principal`] with each part written by `shown`: in backticks for a message, as is for a
/// text-mode table cell, which the output layer escapes itself.
fn principal_as(value: Option<&Value>, shown: impl Fn(&str) -> String) -> Option<String> {
    let field = |key| value.and_then(|v| v.get(key)).and_then(Value::as_str);
    match (field("name"), field("id")) {
        (Some(name), Some(id)) => Some(format!("{} ({})", shown(name), shown(id))),
        (Some(text), None) | (None, Some(text)) => Some(shown(text)),
        (None, None) => None,
    }
}

/// The transfers answer, `{incoming, outgoing}`, as one list envelope: the incoming ones,
/// then the outgoing ones, each as sent plus `direction`, `incoming` or `outgoing`, which is
/// `hipc`'s own field, so a reader of the one list still knows which way each goes. The
/// answer's other fields stay beside `items`. A transfer that is not an object is kept as
/// sent, without the field. Should the API send a `direction` of its own, `hipc`'s value
/// wins: it comes from the list the transfer was in, which is what the field promises.
///
/// # Errors
///
/// `internal` unless the answer is an object holding both lists, so a transfer is never left
/// out without saying so.
fn transfers_envelope(answer: Value) -> Result<Value> {
    let Transfers {
        incoming,
        outgoing,
        mut links,
    } = transfer_lists(answer)?;
    let rest = std::mem::take(&mut links.fields);
    // The list is printed whole, so a `next` link is refused rather than dropped.
    links.into_complete(TRANSFERS_PATH)?;
    let marked = |list: Vec<Value>, direction: &'static str| {
        list.into_iter().map(move |mut transfer| {
            if let Value::Object(fields) = &mut transfer {
                fields.insert("direction".to_owned(), json!(direction));
            }
            transfer
        })
    };
    let items = marked(incoming, "incoming")
        .chain(marked(outgoing, "outgoing"))
        .collect();
    Ok(hippius::envelope_beside(items, rest))
}

/// The transfers list for a person: each transfer's `project`, `from_account`, and
/// `to_account`, objects in the answer, as a plain `name (id)`, so the table shows them
/// rather than raw JSON, and without a message's backticks. One that names neither stays as
/// sent.
fn transfers_for_a_person(mut envelope: Value) -> Value {
    let items = envelope.get_mut("items").and_then(Value::as_array_mut);
    for transfer in items.into_iter().flatten().filter_map(Value::as_object_mut) {
        for key in ["project", "from_account", "to_account"] {
            if let Some(shown) = principal_as(transfer.get(key), str::to_owned) {
                transfer.insert(key.to_owned(), Value::String(shown));
            }
        }
    }
    envelope
}

/// The transfers answer: the transfers offered to the token's own account, those it
/// offered, and the answer's `next` link and other fields, as a page without items.
#[derive(Debug)]
struct Transfers {
    incoming: Vec<Value>,
    outgoing: Vec<Value>,
    links: hippius::Page,
}

/// The `incoming` and `outgoing` lists of the transfers answer, and its other fields.
///
/// # Errors
///
/// `internal` unless the answer is an object holding both lists, or for a `next` that is
/// not a link. A `next` link is the caller's to refuse: the list refuses it, and a decision
/// refuses it only when the transfer is not on the page read.
fn transfer_lists(answer: Value) -> Result<Transfers> {
    let Value::Object(mut body) = answer else {
        return Err(unexpected_transfers());
    };
    match (body.shift_remove("incoming"), body.shift_remove("outgoing")) {
        (Some(Value::Array(incoming)), Some(Value::Array(outgoing))) => Ok(Transfers {
            incoming,
            outgoing,
            links: hippius::page_beside(Vec::new(), body)?,
        }),
        _ => Err(unexpected_transfers()),
    }
}

fn unexpected_transfers() -> Error {
    Error::internal(
        "cannot list the project transfers: the API answered without `incoming` and \
         `outgoing` lists",
    )
}
