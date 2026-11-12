//! `hipc cdn waf` and `hipc cdn lists`: a zone's web application firewall, and the account's
//! WAF lists.
//!
//! Every route is under `/api/cdn/`, which is account-scoped. The routes and their bodies
//! are swagger's (`cdn-waf` tag); the answers of the reads have no schema there, and the
//! docs page (<https://docs.hippius.com/use/cdn/firewall>) gives their shapes: the rule
//! reads answer `{"rules": [...], "version": N}`.
//!
//! The WAF is billed while its mode is not `off`, at the price of its tier, which `hipc cdn
//! prices` lists under `waf`; `hipc` names no price (tenet 11). Every settings change
//! publishes a version, so every `waf update` is confirmed. The custom rules, the rate
//! limits, and a list's items are whole lists: their replace takes the `--remove-others`
//! whole-list guard ([`replace_list`]).

use std::path::Path;

use hippius::routes::EmptyBody;
use hippius::routes::cdn::{self, BlockResponse, WafUpdateBody};
use hippius::{Error, Request, Result, phrase};
use serde_json::{Value, json};

use crate::cli::{BlockContentType, BlockStatus, WafTier, ZoneTarget};
use crate::cli::{CdnLists, CdnWaf, GroupBy, ListItems, RuleAction, StatsStep, WafAnalytics};
use crate::cli::{WafEvents, WafMode, WafRateLimits, WafRules, WafTest, WafUpdate, WafVersions};
use crate::commands::cdn::rule_list::{
    CUSTOM_RULES, ListUpdate, RATE_LIMITS, read_rules, update_rules,
};
use crate::commands::cdn::{ZONES_LIST_COMMAND, fields_set, with_query};
use crate::commands::replace::{Entry, Found, Replace, replace_list};
use crate::commands::{Input, Printed, print_body, print_body_or, print_keyed_list};
use crate::context::{Context, Gate};

const WAF_LISTS_LIST_COMMAND: &str = "hipc cdn lists list";

/// The largest block page body, in bytes (swagger, `block_response.body`: "At most 4 KB").
const MAX_BLOCK_BODY_BYTES: u64 = 4096;

/// The largest `--request-file` read, in bytes. Arbitrary: the docs hold each field of a
/// test request to 2 KB, and ten fields fit well under this.
const MAX_TEST_BYTES: u64 = 64 * 1024;

/// The largest `--items-file` read, in bytes (swagger, `.../items/`: "at most 1 MiB").
const MAX_ITEMS_BYTES: u64 = 1024 * 1024;

/// What follows from a change to a list's items (the docs: "Changing a list republishes the
/// WAF of every zone that uses it").
pub(crate) const LIST_REPUBLISHES: &str = "every zone whose WAF uses the list publishes it again";

pub(super) fn waf(ctx: &Context<'_>, command: &CdnWaf) -> Result<Printed> {
    match command {
        CdnWaf::Get(ZoneTarget { zone_id }) => {
            let request = Request::to(&cdn::WAF, [zone_id])
                .about(format!("get the WAF settings of the CDN zone `{zone_id}`"))
                .listed_by(ZONES_LIST_COMMAND);
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        CdnWaf::Update(args) => update(ctx, args),
        CdnWaf::Status(ZoneTarget { zone_id }) => {
            let request = Request::to(&cdn::WAF_STATUS, [zone_id])
                .about(format!(
                    "get where the WAF of the CDN zone `{zone_id}` is deployed"
                ))
                .listed_by(ZONES_LIST_COMMAND);
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        CdnWaf::Analytics(args) => analytics(ctx, args),
        CdnWaf::Events(args) => events(ctx, args),
        CdnWaf::Test(args) => test(ctx, args),
        CdnWaf::Rules(WafRules::Get(ZoneTarget { zone_id })) => {
            read_rules(ctx, zone_id, &CUSTOM_RULES)
        }
        CdnWaf::RateLimits(WafRateLimits::Get(ZoneTarget { zone_id })) => {
            read_rules(ctx, zone_id, &RATE_LIMITS)
        }
        CdnWaf::Rules(WafRules::Update {
            zone: ZoneTarget { zone_id },
            rules_file,
            remove_others,
            check,
        }) => {
            let update = ListUpdate {
                zone_id,
                rules_file,
                remove_others: *remove_others,
                check: *check,
            };
            update_rules(ctx, &update, &CUSTOM_RULES)
        }
        CdnWaf::RateLimits(WafRateLimits::Update {
            zone: ZoneTarget { zone_id },
            rules_file,
            remove_others,
            check,
        }) => {
            let update = ListUpdate {
                zone_id,
                rules_file,
                remove_others: *remove_others,
                check: *check,
            };
            update_rules(ctx, &update, &RATE_LIMITS)
        }
        CdnWaf::Versions(command) => versions(ctx, command),
    }
}

/// The cost a mode other than `off`, or a tier, adds to the action. The price is the
/// server's, never a constant here (tenet 11), and no price is read before the question.
const BILLED: &str = ", which is billed from the balance while the mode is not `off`, at the \
                      price of the tier that `hipc cdn prices` lists under `waf`";

/// `PATCH /api/cdn/zones/<id>/waf/` with only the settings given. It is always confirmed:
/// every change publishes a version, a mode other than `off` is billed, and `off` stops
/// the protection.
fn update(ctx: &Context<'_>, args: &WafUpdate) -> Result<Printed> {
    let settings = WafUpdateBody {
        mode: args.mode.map(WafMode::code),
        tier: args.tier.map(WafTier::code),
        event_sampling: args.event_sampling,
        block_response: None,
    };
    let page = BlockResponse {
        status: args.block_status.map(BlockStatus::code),
        content_type: args.block_content_type.map(BlockContentType::code),
        body: None,
    };
    let mut set = fields_set(&settings, "");
    set.extend(fields_set(&page, "block_response."));
    let page_body = match &args.block_body_file {
        Some(path) => {
            let page = Input {
                path,
                flag: "--block-body-file",
                what: "block page body",
            };
            let text = page.read_text(MAX_BLOCK_BODY_BYTES)?;
            set.push(format!(
                "`block_response.body` to the text of {}",
                page.name()
            ));
            Some(text)
        }
        None => None,
    };
    let page = BlockResponse {
        body: page_body.as_deref(),
        ..page
    };
    let changes_page = page.status.is_some() || page.content_type.is_some() || page.body.is_some();
    let body = WafUpdateBody {
        block_response: changes_page.then_some(page),
        ..settings
    };
    if set.is_empty() {
        return Err(Error::usage(
            "`hipc cdn waf update` was given nothing to change",
            "pass at least one setting to change, such as `--mode`",
        ));
    }
    // A mode but `off`, or a tier, sets what the WAF costs, so the action names it: an agent
    // reads the refusal, not the prompt, as the VM launch names the public IPv4's price. A
    // tier set with `off` names it too: it is the price once the mode is turned back on.
    let names_billing =
        matches!(args.mode, Some(WafMode::Log | WafMode::On)) || args.tier.is_some();
    let billed = if names_billing { BILLED } else { "" };
    let consequence = (args.mode == Some(WafMode::Off))
        .then_some("the zone's WAF rules stop applying, and its WAF billing stops");
    let request = Request::to(&cdn::WAF_UPDATE, [&args.zone.zone_id])
        .listed_by(ZONES_LIST_COMMAND)
        .json(&body)
        .about(format!(
            "update the WAF of the CDN zone `{}`: set {}{billed}",
            args.zone.zone_id,
            phrase::join(&set, "and")
        ));
    let client = ctx.load_authed_client()?;
    print_body(ctx, &client, &request, Gate::Confirm { consequence })
}

/// `GET .../waf/analytics/` with the query parameters the docs name, each only when given.
fn analytics(ctx: &Context<'_>, args: &WafAnalytics) -> Result<Printed> {
    let request = Request::to(&cdn::WAF_ANALYTICS, [&args.zone.zone_id])
        .about(format!(
            "get the WAF matches of the CDN zone `{}`",
            args.zone.zone_id
        ))
        .listed_by(ZONES_LIST_COMMAND);
    let query = [
        ("from", args.from.as_deref()),
        ("to", args.to.as_deref()),
        ("step", args.step.map(StatsStep::code)),
        ("group_by", args.group_by.map(GroupBy::code)),
    ];
    let client = ctx.load_authed_client()?;
    print_body(ctx, &client, &with_query(request, &query), Gate::Open)
}

/// `GET .../waf/events/` with the filters the docs name, each only when given. The answer
/// is printed as sent: the docs page it by a cursor whose shape no zone was on the account
/// to check, so a later page is left to `hipc api`.
fn events(ctx: &Context<'_>, args: &WafEvents) -> Result<Printed> {
    let request = Request::to(&cdn::WAF_EVENTS, [&args.zone.zone_id])
        .about(format!(
            "get the sampled WAF matches of the CDN zone `{}`",
            args.zone.zone_id
        ))
        .listed_by(ZONES_LIST_COMMAND);
    let query = [
        ("from", args.from.as_deref()),
        ("to", args.to.as_deref()),
        ("rule", args.rule_id.as_deref()),
        ("action", args.action.map(RuleAction::code)),
        ("ip", args.ip.as_deref()),
    ];
    let client = ctx.load_authed_client()?;
    print_body(ctx, &client, &with_query(request, &query), Gate::Open)
}

/// The keys of a test request (the docs: "exactly these keys").
const TEST_REQUEST_KEYS: [&str; 10] = [
    "ip", "country", "asn", "host", "method", "path", "query", "headers", "version", "ssl",
];

/// `POST .../waf/test/`: a POST that changes nothing (`Request::changes_nothing`), so it is
/// not confirmed and a lost answer is a read's. A request key the docs do not list is
/// refused, as a rule key is, so a misspelled field cannot make the test run without it.
fn test(ctx: &Context<'_>, args: &WafTest) -> Result<Printed> {
    let file = Input {
        path: &args.request_file,
        flag: "--request-file",
        what: "test request",
    };
    let sample = file.read_value(MAX_TEST_BYTES)?;
    let Some(fields) = sample.as_object() else {
        return Err(Error::usage(
            format!("the test request from {} is not a JSON object", file.name()),
            "write the request as a JSON object, such as `{\"path\": \"/\"}`",
        ));
    };
    if let Some(key) = fields
        .keys()
        .find(|key| !TEST_REQUEST_KEYS.contains(&key.as_str()))
    {
        return Err(Error::usage(
            format!(
                "the test request from {} has the key `{key}`, which a test request does not \
                 have: it has only {}",
                file.name(),
                phrase::ticked(TEST_REQUEST_KEYS, "and")
            ),
            format!("remove `{key}` from the request"),
        ));
    }
    let request = Request::to(&cdn::WAF_TEST, [&args.zone.zone_id])
        .json(&cdn::WafTestBody {
            expression: args.expression.as_deref(),
            request: &sample,
        })
        .about(format!(
            "test a request against the WAF of the CDN zone `{}`",
            args.zone.zone_id
        ))
        .listed_by(ZONES_LIST_COMMAND)
        .changes_nothing();
    print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
}

fn versions(ctx: &Context<'_>, command: &WafVersions) -> Result<Printed> {
    let client = ctx.load_authed_client()?;
    match command {
        WafVersions::List(ZoneTarget { zone_id }) => {
            let request = Request::to(&cdn::WAF_VERSIONS, [zone_id])
                .about(format!("list the WAF versions of the CDN zone `{zone_id}`"))
                .listed_by(ZONES_LIST_COMMAND);
            print_keyed_list(ctx, &client, &request, "versions")
        }
        WafVersions::Get {
            zone: ZoneTarget { zone_id },
            version_number,
        } => {
            let request = Request::to(&cdn::WAF_VERSION, [zone_id, version_number])
                .about(format!(
                    "get the WAF version `{version_number}` of the CDN zone `{zone_id}`"
                ))
                .listed_by(format!("hipc cdn waf versions list {zone_id}"));
            print_body(ctx, &client, &request, Gate::Open)
        }
        // Swagger: "No input: the version is in the path"; the body is the empty object
        // its schema gives.
        WafVersions::Rollback {
            zone: ZoneTarget { zone_id },
            version_number,
        } => {
            let request = Request::to(&cdn::WAF_ROLLBACK, [zone_id, version_number])
                .listed_by(format!("hipc cdn waf versions list {zone_id}"))
                .json(&EmptyBody {})
                .about(format!(
                    "roll the WAF of the CDN zone `{zone_id}` back to version `{version_number}`"
                ));
            let gate = Gate::Confirm {
                consequence: Some(
                    "a copy of that version is published as the newest one, and the WAF is \
                     billed while the mode it then has is not `off`",
                ),
            };
            print_body(ctx, &client, &request, gate)
        }
    }
}

pub(super) fn lists(ctx: &Context<'_>, command: &CdnLists) -> Result<Printed> {
    match command {
        CdnLists::List => print_keyed_list(
            ctx,
            &ctx.load_authed_client()?,
            &Request::to(&cdn::LISTS, []).about("list the WAF lists"),
            "lists",
        ),
        CdnLists::Get { list_name } => print_body(
            ctx,
            &ctx.load_authed_client()?,
            &list_get(list_name),
            Gate::Open,
        ),
        CdnLists::Create {
            list_name,
            kind,
            description,
            items_file,
        } => {
            let items = items_file.as_deref().map(read_items).transpose()?;
            // A list costs nothing and filters nothing until a rule names it, so it is not
            // confirmed; a second create of the name is refused (`list_exists`).
            let request = Request::to(&cdn::LIST_CREATE, [])
                .json(&cdn::ListCreateBody {
                    name: list_name,
                    kind: kind.code(),
                    description: description.as_deref(),
                    items: items.as_deref(),
                })
                .about(format!("create the WAF list `{list_name}`"));
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        // The PATCH also takes `items`, which would replace them without the guard that
        // `items update` has, so only the description is sent here.
        CdnLists::Update {
            list_name,
            description,
        } => {
            let request = Request::to(&cdn::LIST_UPDATE, [list_name])
                .listed_by(WAF_LISTS_LIST_COMMAND)
                .json(&cdn::ListUpdateBody { description })
                .about(format!(
                    "change the description of the WAF list `{list_name}`"
                ));
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        CdnLists::Delete { list_name } => {
            let request = Request::to(&cdn::LIST_DELETE, [list_name])
                .listed_by(WAF_LISTS_LIST_COMMAND)
                .about(format!("delete the WAF list `{list_name}`"));
            let gate = Gate::Confirm { consequence: None };
            let done = json!({ "name": list_name, "deleted": true });
            print_body_or(ctx, &ctx.load_authed_client()?, &request, gate, done)
        }
        CdnLists::Items(command) => items(ctx, command),
    }
}

fn list_get(name: &str) -> Request {
    Request::to(&cdn::LIST, [&name])
        .about(format!("get the WAF list `{name}`"))
        .listed_by(WAF_LISTS_LIST_COMMAND)
}

fn items(ctx: &Context<'_>, command: &ListItems) -> Result<Printed> {
    match command {
        ListItems::Create {
            list_name,
            items_file,
        } => {
            let items = some_items(items_file)?;
            let request = Request::to(&cdn::LIST_ITEMS_ADD, [list_name])
                .listed_by(WAF_LISTS_LIST_COMMAND)
                .about(format!(
                    "add {} to the WAF list `{list_name}`",
                    phrase::count(items.len() as u64, "item")
                ))
                .json(&cdn::ItemsBody { items: &items });
            // An item changes what every zone whose WAF uses the list lets through or blocks.
            let gate = Gate::Confirm {
                consequence: Some(LIST_REPUBLISHES),
            };
            print_body(ctx, &ctx.load_authed_client()?, &request, gate)
        }
        // This route's DELETE carries a body, the items to remove.
        ListItems::Delete {
            list_name,
            items_file,
        } => {
            let items = some_items(items_file)?;
            let request = Request::to(&cdn::LIST_ITEMS_REMOVE, [list_name])
                .listed_by(WAF_LISTS_LIST_COMMAND)
                .about(format!(
                    "remove {} from the WAF list `{list_name}`",
                    phrase::count(items.len() as u64, "item")
                ))
                .json(&cdn::ItemsBody { items: &items });
            let gate = Gate::Confirm {
                consequence: Some(LIST_REPUBLISHES),
            };
            let done = json!({ "name": list_name, "removed": items });
            print_body_or(ctx, &ctx.load_authed_client()?, &request, gate, done)
        }
        ListItems::Update {
            list_name,
            items_file,
            remove_others,
        } => {
            let items = read_items(items_file)?;
            let entries = items.iter().map(|item| Entry::new(item.as_str().into()));
            let read = list_get(list_name);
            let read_action = read.action();
            let replace = Replace {
                write: Request::to(&cdn::LIST_ITEMS_REPLACE, [list_name])
                    .listed_by(WAF_LISTS_LIST_COMMAND)
                    .about(format!(
                        "replace every item of the WAF list `{list_name}` with {}",
                        phrase::count(items.len() as u64, "item")
                    ))
                    .json(&cdn::ItemsBody { items: &items }),
                consequence: Some(LIST_REPUBLISHES),
                entries: entries.collect(),
                remove_others: *remove_others,
                read,
                draft_body: None,
                // A list has no version: swagger and the docs give `If-Match` only for a
                // zone's WAF version, so this PUT cannot be made conditional, and an item
                // added between the read and the PUT is dropped unseen.
                draft_if_match: None,
                noun: "item",
                owner: format!("the WAF list `{list_name}`"),
            };
            let current = |answer: Value| {
                stored_items(&answer)
                    .map(Found::new)
                    .ok_or_else(|| crate::commands::unreadable(&read_action))
            };
            replace_list(ctx, &ctx.load_authed_client()?, &replace, current)
        }
    }
}

/// The items a list read answers, each as it is stored; `None` for an answer without an
/// `items` array of text.
fn stored_items(answer: &Value) -> Option<Vec<Entry>> {
    answer
        .get("items")?
        .as_array()?
        .iter()
        .map(|item| Some(Entry::new(item.as_str()?.into())))
        .collect()
}

/// The items of an `--items-file`, as the server reads its text form (the docs): one per
/// line, `#` starting a comment, and blank lines skipped. They are sent as JSON, because
/// every request `hipc` sends is JSON.
fn read_items(path: &Path) -> Result<Vec<String>> {
    let file = items_file(path);
    let items = file
        .read_text(MAX_ITEMS_BYTES)?
        .lines()
        .map(|line| line.split_once('#').map_or(line, |(item, _)| item).trim())
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect();
    Ok(items)
}

/// [`read_items`], refusing a file that holds no item: adding or removing nothing is not a
/// change.
fn some_items(path: &Path) -> Result<Vec<String>> {
    let items = read_items(path)?;
    if items.is_empty() {
        return Err(Error::usage(
            format!(
                "the list of items from {} holds no item",
                items_file(path).name()
            ),
            "write one item per line in the file passed to `--items-file`",
        ));
    }
    Ok(items)
}

fn items_file(path: &Path) -> Input<'_> {
    Input {
        path,
        flag: "--items-file",
        what: "list of items",
    }
}
