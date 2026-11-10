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

use hippius::{Client, Error, Method, Request, Response, Result, SideEffects, phrase};
use serde_json::{Map, Value, json};

use crate::cli::{CdnLists, CdnWaf, GroupBy, ListItems, RuleAction, StatsStep, WafAnalytics};
use crate::cli::{WafEvents, WafMode, WafRateLimits, WafRules, WafTest, WafUpdate, WafVersions};
use crate::commands::cdn::{MAX_RULES_BYTES, ZONES, shown, with_query, zone_path};
use crate::commands::replace::{self, Entry, Found, Replace, replace_list};
use crate::commands::{Input, Printed, print_body, print_body_or, print_keyed_list};
use crate::context::{Context, Gate, Step, Write};

/// The command that lists the WAF lists, for the hint of a 404 on a list route.
const LISTS: &str = "hipc cdn lists list";

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
        CdnWaf::Get { zone_id } => {
            let request = Request::get(waf_path(zone_id))
                .about(format!("get the WAF settings of the CDN zone `{zone_id}`"))
                .listed_by(ZONES);
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        CdnWaf::Update(args) => update(ctx, args),
        CdnWaf::Status { zone_id } => {
            let request = Request::get(format!("{}status/", waf_path(zone_id)))
                .about(format!(
                    "get where the WAF of the CDN zone `{zone_id}` is deployed"
                ))
                .listed_by(ZONES);
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        CdnWaf::Analytics(args) => analytics(ctx, args),
        CdnWaf::Events(args) => events(ctx, args),
        CdnWaf::Test(args) => test(ctx, args),
        CdnWaf::Rules(WafRules::Get { zone_id }) => read_rules(ctx, zone_id, &CUSTOM_RULES),
        CdnWaf::RateLimits(WafRateLimits::Get { zone_id }) => {
            read_rules(ctx, zone_id, &RATE_LIMITS)
        }
        CdnWaf::Rules(WafRules::Update {
            zone_id,
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
            zone_id,
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

fn waf_path(zone_id: &str) -> String {
    format!("{}waf/", zone_path(zone_id))
}

/// The cost a mode other than `off`, or a tier, adds to the action. The price is the
/// server's, never a constant here (tenet 11), and no price is read before the question.
const BILLED: &str = ", which is billed from the balance while the mode is not `off`, at the \
                      price of the tier that `hipc cdn prices` lists under `waf`";

/// `PATCH /api/cdn/zones/<id>/waf/` with only the settings given. It is always confirmed:
/// every change publishes a version, a mode other than `off` is billed, and `off` stops
/// the protection.
fn update(ctx: &Context<'_>, args: &WafUpdate) -> Result<Printed> {
    let mut body = Map::new();
    if let Some(mode) = args.mode {
        body.insert("mode".to_owned(), mode.code().into());
    }
    if let Some(tier) = args.tier {
        body.insert("tier".to_owned(), tier.code().into());
    }
    if let Some(sampling) = args.event_sampling {
        body.insert("event_sampling".to_owned(), sampling.into());
    }
    let mut set: Vec<String> = body
        .iter()
        .map(|(key, value)| format!("`{key}` to {}", shown(value)))
        .collect();
    let mut block = Map::new();
    if let Some(status) = args.block_status {
        block.insert("status".to_owned(), status.code().into());
    }
    if let Some(content_type) = args.block_content_type {
        block.insert("content_type".to_owned(), content_type.code().into());
    }
    set.extend(
        block
            .iter()
            .map(|(key, value)| format!("`block_response.{key}` to {}", shown(value))),
    );
    if let Some(path) = &args.block_body_file {
        let page = Input {
            path,
            flag: "--block-body-file",
            what: "block page body",
        };
        block.insert(
            "body".to_owned(),
            page.read_text(MAX_BLOCK_BODY_BYTES)?.into(),
        );
        set.push(format!(
            "`block_response.body` to the text of {}",
            page.name()
        ));
    }
    if !block.is_empty() {
        body.insert("block_response".to_owned(), Value::Object(block));
    }
    if body.is_empty() {
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
    let request = Request::new(Method::Patch, waf_path(&args.zone_id))
        .listed_by(ZONES)
        .json(&Value::Object(body))
        .about(format!(
            "update the WAF of the CDN zone `{}`: set {}{billed}",
            args.zone_id,
            phrase::join(&set, "and")
        ));
    let client = ctx.load_authed_client()?;
    print_body(ctx, &client, &request, Gate::Confirm { consequence })
}

/// `GET .../waf/analytics/` with the query parameters the docs name, each only when given.
fn analytics(ctx: &Context<'_>, args: &WafAnalytics) -> Result<Printed> {
    let request = Request::get(format!("{}analytics/", waf_path(&args.zone_id)))
        .about(format!(
            "get the WAF matches of the CDN zone `{}`",
            args.zone_id
        ))
        .listed_by(ZONES);
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
    let request = Request::get(format!("{}events/", waf_path(&args.zone_id)))
        .about(format!(
            "get the sampled WAF matches of the CDN zone `{}`",
            args.zone_id
        ))
        .listed_by(ZONES);
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
    let mut body = Map::new();
    if let Some(expression) = &args.expression {
        body.insert("expression".to_owned(), expression.as_str().into());
    }
    body.insert("request".to_owned(), sample);
    let request = Request::new(Method::Post, format!("{}test/", waf_path(&args.zone_id)))
        .json(&Value::Object(body))
        .about(format!(
            "test a request against the WAF of the CDN zone `{}`",
            args.zone_id
        ))
        .listed_by(ZONES)
        .changes_nothing();
    print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
}

/// The flags of a rule list update: the zone, the file, `--remove-others`, and `--check`.
#[derive(Debug, Clone, Copy)]
struct ListUpdate<'a> {
    zone_id: &'a str,
    rules_file: &'a Path,
    remove_others: bool,
    check: bool,
}

/// A whole-list rule route of the WAF: the custom rules or the rate limits.
#[derive(Debug)]
struct RuleList {
    /// The route under `.../waf/`, such as `rules/`.
    segment: &'static str,
    /// The list, as a message names it, such as "WAF custom rules".
    what: &'static str,
    /// One rule, as a count names it: it takes a plain `s` in the plural.
    noun: &'static str,
    /// The keys swagger lists for a rule of the PUT.
    keys: &'static [&'static str],
    /// The keys swagger requires, which are also what a rule is compared by, with `enabled`.
    required: &'static [&'static str],
    /// Whether a rule's place is part of it.
    ordered: bool,
    /// The command that prints the list.
    get_command: &'static str,
}

/// The custom rules run top to bottom, and a block or an allow stops the rest (the docs), so
/// a rule's place is part of it.
const CUSTOM_RULES: RuleList = RuleList {
    segment: "rules/",
    what: "WAF custom rules",
    noun: "WAF custom rule",
    keys: &["id", "description", "enabled", "expression", "action"],
    required: &["expression", "action"],
    ordered: true,
    get_command: "hipc cdn waf rules get",
};

/// Each rate limit counts on its own, so its place is not part of it.
const RATE_LIMITS: RuleList = RuleList {
    segment: "rate-limits/",
    what: "WAF rate limits",
    noun: "WAF rate limit",
    keys: &[
        "id",
        "description",
        "enabled",
        "expression",
        "action",
        "key",
        "period",
        "requests",
        "timeout",
    ],
    required: &[
        "expression",
        "action",
        "key",
        "period",
        "requests",
        "timeout",
    ],
    ordered: false,
    get_command: "hipc cdn waf rate-limits get",
};

fn read_rules(ctx: &Context<'_>, zone_id: &str, list: &RuleList) -> Result<Printed> {
    let client = ctx.load_authed_client()?;
    print_body(ctx, &client, &rules_get(zone_id, list), Gate::Open)
}

fn rules_get(zone_id: &str, list: &RuleList) -> Request {
    Request::get(format!("{}{}", waf_path(zone_id), list.segment))
        .about(format!("get the {} of the CDN zone `{zone_id}`", list.what))
        .listed_by(ZONES)
}

/// `PUT .../waf/rules/` or `.../rate-limits/` with the file's bytes, under the whole-list
/// guard. A rule key swagger does not list is refused first, then a file that is not a
/// `rules` object of rules with every required key.
///
/// With `--check` the same PUT goes with `?dry_run=1` ([`check_rules`]).
fn update_rules(ctx: &Context<'_>, update: &ListUpdate<'_>, list: &RuleList) -> Result<Printed> {
    let ListUpdate {
        zone_id,
        rules_file,
        remove_others,
        check,
    } = *update;
    let file = Input {
        path: rules_file,
        flag: "--rules-file",
        what: list.what,
    };
    let (body, parsed) = file.read_json(MAX_RULES_BYTES)?;
    check_rule_keys(&parsed, &file.name(), list)?;
    let Some(entries) = rule_entries(&parsed, list) else {
        return Err(Error::usage(
            format!(
                "the {} from {} are not a JSON object with a `rules` array of rules that each \
                 have {}",
                list.what,
                file.name(),
                phrase::ticked(list.required, "and")
            ),
            format!(
                "write the rules as `{{\"rules\": [...]}}`, the shape `{}` prints",
                list.get_command
            ),
        ));
    };
    let read = rules_get(zone_id, list);
    let read_action = read.action();
    let replace = Replace {
        write: Request::new(
            Method::Put,
            format!("{}{}", waf_path(zone_id), list.segment),
        )
        .listed_by(ZONES)
        .raw_json(body)
        .about(format!(
            "replace every {} of the CDN zone `{zone_id}` with the rules from {}",
            list.noun,
            file.name()
        )),
        consequence: None,
        entries,
        remove_others,
        read,
        draft_body: None,
        draft_if_match: Some(VERSION_PLACEHOLDER),
        noun: list.noun,
        owner: format!("the CDN zone `{zone_id}`"),
    };
    // The PUT is made conditional on the rules read: it carries the read's `version` as
    // `If-Match`, and the server refuses it with 412 `waf_conflict`, changing nothing, when
    // another client published in between (https://docs.hippius.com/use/cdn/firewall).
    // Without that, a rule published after the read would be dropped unseen. With
    // `--remove-others` too: the flag lets the file remove the rules it leaves out, not
    // overwrite a version nobody read, so the rules are still read, after the question, for
    // the version. A read without a whole `version` is unreadable rather than a PUT without
    // the precondition. The docs give `0` before the first publish; whether such a zone answers
    // `0` or null is unverified, and a null would make every update `internal`, leaving
    // `hipc api PUT ... --remove-others`, which sends no `If-Match`.
    let current = |answer: Value| {
        let version = answer.get("version").and_then(Value::as_u64);
        match (rule_entries(&answer, list), version) {
            (Some(entries), Some(version)) => Ok(Found {
                entries,
                body: None,
                if_match: Some(version.to_string()),
            }),
            _ => Err(crate::commands::unreadable(&read_action)),
        }
    };
    let client = ctx.load_authed_client()?;
    let done = if check {
        let write = replace
            .write
            .clone()
            .query("dry_run", "1")
            .about(format!(
                "check the {} from {} against the CDN zone `{zone_id}`",
                list.what,
                file.name()
            ))
            .changes_nothing();
        match check_rules(ctx, &client, &replace, write, current)? {
            Step::Sent(response) => checked(&response, zone_id, list).map(Printed::Document),
            Step::DryRun(doc) => Ok(Printed::Document(doc)),
        }
    } else {
        replace_list(ctx, &client, &replace, current)
    };
    done.map_err(|error| {
        if error.http_status() != Some(412) {
            return error;
        }
        let again = phrase::command(ctx.args());
        let check = if remove_others {
            ""
        } else {
            " and checks the file against them"
        };
        error.with_hint(format!(
            "run `{again}` again, which reads the rules again{check}"
        ))
    })
}

/// `--check`: `write`, the PUT of [`update_rules`] with `?dry_run=1`, which the docs say checks
/// a list without publishing it and answers `{"valid": ..., "size": ..., "limit": ...}`
/// (<https://docs.hippius.com/use/cdn/firewall>). It is not confirmed, and a failure is a
/// read's. Since only the server keeps it from publishing, it goes as the update would: after
/// the read of the rules and the whole-list guard ([`replace::refuse_removal`]), with the
/// version read as `If-Match`. A server that ignored `dry_run` could then publish only a list
/// the update would publish without `--remove-others`, on top of the version read, and
/// [`checked`] would report it. A dry run shows the read and the PUT.
fn check_rules(
    ctx: &Context<'_>,
    client: &Client,
    replace: &Replace,
    write: Request,
    current: impl FnOnce(Value) -> Result<Found>,
) -> Result<Step> {
    let draft = write.clone().if_match(VERSION_PLACEHOLDER);
    if let Write::DryRun(mut doc) = ctx.check_write(client, &draft)? {
        doc["reads_first"] = ctx.describe(client, &replace.read)?;
        doc["requests"] = ctx.describe_each(client, &[&replace.read, &draft])?;
        return Ok(Step::DryRun(doc));
    }
    let found = current(ctx.send(client, &replace.read)?.document()?)?;
    replace::refuse_removal(ctx, replace, &found)?;
    let write = found.if_match.iter().fold(write, Request::if_match);
    ctx.send(client, &write).map(Step::Sent)
}

/// The answer of a `--check`: the server's `{"valid": ..., "size": ..., "limit": ...}`,
/// printed as it is when `valid` is `true`.
///
/// # Errors
///
/// `validation`, exit 1, with the answer in `details`, when `valid` is `false`, so a script
/// can trust the exit code. `internal` with `partial` side effects for an answer with no
/// boolean `valid`, or with the `rules` or `version` of a published list: a server that
/// ignored `dry_run` would have published the file, so the hint reads what is published.
fn checked(response: &Response, zone_id: &str, list: &RuleList) -> Result<Value> {
    let answer = response.document()?;
    let published = answer.get("rules").is_some() || answer.get("version").is_some();
    match answer.get("valid") {
        Some(Value::Bool(true)) if !published => Ok(answer),
        Some(Value::Bool(false)) if !published => Err(response
            .invalid("the API says the list is not valid")
            .with_hint("fix the list as `details` says, then run the check again")),
        _ => Err(response
            .refused(
                "the API answered without a boolean `valid`, or with a published list, so the \
                 file may have been published",
            )
            .with_side_effects(SideEffects::Partial)
            .with_hint(format!(
                "run `{} {zone_id}` to see what is published",
                list.get_command
            ))),
    }
}

/// What a dry run shows for the `If-Match` of a guarded rule PUT, which the read gives.
const VERSION_PLACEHOLDER: &str = "<the version the read returns>";

/// The rules of a rule file or answer: `{"rules": [...]}`, the only shape swagger gives the
/// PUT; `None` for any other.
fn rules_of(list: &Value) -> Option<&Vec<Value>> {
    list.as_object()?.get("rules")?.as_array()
}

/// Refuses a rule with a key swagger does not list, so a misspelled key cannot make the rule
/// do something other than what was written. The shape is left to [`rule_entries`].
fn check_rule_keys(parsed: &Value, file: &str, list: &RuleList) -> Result<()> {
    let Some(rules) = rules_of(parsed) else {
        return Ok(());
    };
    for (index, rule) in rules.iter().enumerate() {
        let unknown = rule
            .as_object()
            .and_then(|rule| rule.keys().find(|key| !list.keys.contains(&key.as_str())));
        if let Some(key) = unknown {
            return Err(Error::usage(
                format!(
                    "`rules[{index}]` from {file} has the key `{key}`, which a {} does not \
                     have: a rule has only {}",
                    list.noun,
                    phrase::ticked(list.keys, "and")
                ),
                format!("fix `rules[{index}]`; the zone keeps its rules until then"),
            ));
        }
    }
    Ok(())
}

/// The entries of a rule list, or `None` when it is not a `rules` object of objects that
/// each hold every required key.
///
/// A rule is what it matches and what it does: its required keys, and `enabled`, which
/// counts as `true` when absent (swagger's default), so turning a rule off is a removal.
/// Its `id` and `description` are left out, so changing them is an edit. A custom rule is
/// also its place ([`RuleList::ordered`]): moving one, or putting a new one ahead of it,
/// changes which rule a request meets first.
fn rule_entries(parsed: &Value, list: &RuleList) -> Option<Vec<Entry>> {
    rules_of(parsed)?
        .iter()
        .enumerate()
        .map(|(position, rule)| {
            let rule = rule.as_object()?;
            let mut key = Map::new();
            if list.ordered {
                key.insert("position".to_owned(), position.into());
            }
            for field in list.required {
                key.insert((*field).to_owned(), rule.get(*field)?.clone());
            }
            let enabled = rule.get("enabled").cloned().unwrap_or(Value::Bool(true));
            key.insert("enabled".to_owned(), enabled);
            Some(Entry::new(Value::Object(key)))
        })
        .collect()
}

fn versions(ctx: &Context<'_>, command: &WafVersions) -> Result<Printed> {
    let client = ctx.load_authed_client()?;
    let version_path =
        |zone_id: &str, number: u64| format!("{}versions/{number}/", waf_path(zone_id));
    match command {
        WafVersions::List { zone_id } => {
            let request = Request::get(format!("{}versions/", waf_path(zone_id)))
                .about(format!("list the WAF versions of the CDN zone `{zone_id}`"))
                .listed_by(ZONES);
            print_keyed_list(ctx, &client, &request, "versions")
        }
        WafVersions::Get {
            zone_id,
            version_number,
        } => {
            let request = Request::get(version_path(zone_id, *version_number))
                .about(format!(
                    "get the WAF version `{version_number}` of the CDN zone `{zone_id}`"
                ))
                .listed_by(format!("hipc cdn waf versions list {zone_id}"));
            print_body(ctx, &client, &request, Gate::Open)
        }
        // Swagger: "No input: the version is in the path"; the body is the empty object
        // its schema gives.
        WafVersions::Rollback {
            zone_id,
            version_number,
        } => {
            let path = format!("{}rollback/", version_path(zone_id, *version_number));
            let request = Request::new(Method::Post, path)
                .listed_by(format!("hipc cdn waf versions list {zone_id}"))
                .json(&json!({}))
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
            &Request::get("/cdn/lists/").about("list the WAF lists"),
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
            let mut body = Map::new();
            body.insert("name".to_owned(), list_name.as_str().into());
            body.insert("kind".to_owned(), kind.code().into());
            if let Some(description) = description {
                body.insert("description".to_owned(), description.as_str().into());
            }
            if let Some(path) = items_file {
                body.insert("items".to_owned(), read_items(path)?.into());
            }
            // A list costs nothing and filters nothing until a rule names it, so it is not
            // confirmed; a second create of the name is refused (`list_exists`).
            let request = Request::new(Method::Post, "/cdn/lists/")
                .json(&Value::Object(body))
                .about(format!("create the WAF list `{list_name}`"));
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        // The PATCH also takes `items`, which would replace them without the guard that
        // `items update` has, so only the description is sent here.
        CdnLists::Update {
            list_name,
            description,
        } => {
            let request = Request::new(Method::Patch, list_path(list_name))
                .listed_by(LISTS)
                .json(&json!({ "description": description }))
                .about(format!(
                    "change the description of the WAF list `{list_name}`"
                ));
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        CdnLists::Delete { list_name } => {
            let request = Request::new(Method::Delete, list_path(list_name))
                .listed_by(LISTS)
                .about(format!("delete the WAF list `{list_name}`"));
            let gate = Gate::Confirm { consequence: None };
            let done = json!({ "name": list_name, "deleted": true });
            print_body_or(ctx, &ctx.load_authed_client()?, &request, gate, done)
        }
        CdnLists::Items(command) => items(ctx, command),
    }
}

fn list_path(name: &str) -> String {
    format!("/cdn/lists/{name}/")
}

fn list_get(name: &str) -> Request {
    Request::get(list_path(name))
        .about(format!("get the WAF list `{name}`"))
        .listed_by(LISTS)
}

fn items(ctx: &Context<'_>, command: &ListItems) -> Result<Printed> {
    let items_path = |name: &str| format!("{}items/", list_path(name));
    match command {
        ListItems::Create {
            list_name,
            items_file,
        } => {
            let items = some_items(items_file)?;
            let request = Request::new(Method::Post, items_path(list_name))
                .listed_by(LISTS)
                .about(format!(
                    "add {} to the WAF list `{list_name}`",
                    phrase::count(items.len() as u64, "item")
                ))
                .json(&json!({ "items": items }));
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
            let request = Request::new(Method::Delete, items_path(list_name))
                .listed_by(LISTS)
                .about(format!(
                    "remove {} from the WAF list `{list_name}`",
                    phrase::count(items.len() as u64, "item")
                ))
                .json(&json!({ "items": items }));
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
                write: Request::new(Method::Put, items_path(list_name))
                    .listed_by(LISTS)
                    .about(format!(
                        "replace every item of the WAF list `{list_name}` with {}",
                        phrase::count(items.len() as u64, "item")
                    ))
                    .json(&json!({ "items": items })),
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
