//! The whole-list rule routes of a CDN zone: its cache rules, and its WAF custom rules and
//! rate limits. Each is read, and replaced whole from a file under the `--remove-others`
//! whole-list guard ([`replace_list`]); they differ in the shape of a rule, in what a rule is
//! compared by, and in whether the read gives a version the replace is conditional on.

use std::path::Path;

use hippius::routes::Route;
use hippius::routes::cdn;
use hippius::{Client, Error, Request, Response, Result, SideEffects, phrase, view_of};
use serde_json::{Map, Value, json};

use crate::cli::ZoneId;
use crate::commands::cdn::ZONES;
use crate::commands::replace::{self, Entry, Found, Replace, replace_list};
use crate::commands::{Input, Printed, print_body};
use crate::context::{Context, Gate, Step, Write, WritePlan};

/// The largest `--rules-file` read, in bytes. Arbitrary: 50 rules, the server's limit, fit
/// in a few kilobytes, and a wrong path is refused rather than sent.
const MAX_RULES_BYTES: u64 = 1024 * 1024;

/// The flags of a rule list update: the zone, the file, `--remove-others`, and `--check`,
/// which only the WAF lists take.
#[derive(Debug, Clone, Copy)]
pub(super) struct ListUpdate<'a> {
    pub(super) zone_id: &'a ZoneId,
    pub(super) rules_file: &'a Path,
    pub(super) remove_others: bool,
    pub(super) check: bool,
}

/// A whole-list rule route of a zone.
#[derive(Debug)]
pub(super) struct RuleList {
    /// The read of the list.
    read: &'static Route<1>,
    /// The PUT that replaces the list.
    update: &'static Route<1>,
    /// The list, as a message names it, such as "WAF custom rules".
    what: &'static str,
    /// One rule, as a count names it: it takes a plain `s` in the plural.
    noun: &'static str,
    /// The command that prints the list.
    get_command: &'static str,
    rules: Rules,
}

/// What a rule of a list is.
#[derive(Debug)]
enum Rules {
    /// A cache rule: its place and its `match` ([`cache_rule_entries`]). The route takes no
    /// version, so the replace is not conditional.
    Cache,
    /// A WAF rule: its required keys and `enabled` ([`rule_entries`]). The read gives the
    /// list's `version`, which the replace carries as `If-Match`.
    Waf {
        /// The keys swagger lists for a rule of the PUT.
        keys: &'static [&'static str],
        /// The keys swagger requires, which are also what a rule is compared by, with
        /// `enabled`.
        required: &'static [&'static str],
        /// Whether a rule's place is part of it.
        ordered: bool,
    },
}

/// The cache rules of a zone.
pub(super) const CACHE_RULES: RuleList = RuleList {
    read: &cdn::RULES,
    update: &cdn::RULES_UPDATE,
    what: "cache rules",
    noun: "cache rule",
    get_command: "hipc cdn rules get",
    rules: Rules::Cache,
};

/// The custom rules run top to bottom, and a block or an allow stops the rest (the docs), so
/// a rule's place is part of it.
pub(super) const CUSTOM_RULES: RuleList = RuleList {
    read: &cdn::WAF_RULES,
    update: &cdn::WAF_RULES_UPDATE,
    what: "WAF custom rules",
    noun: "WAF custom rule",
    get_command: "hipc cdn waf rules get",
    rules: Rules::Waf {
        keys: &["id", "description", "enabled", "expression", "action"],
        required: &["expression", "action"],
        ordered: true,
    },
};

/// Each rate limit counts on its own, so its place is not part of it.
pub(super) const RATE_LIMITS: RuleList = RuleList {
    read: &cdn::WAF_RATE_LIMITS,
    update: &cdn::WAF_RATE_LIMITS_UPDATE,
    what: "WAF rate limits",
    noun: "WAF rate limit",
    get_command: "hipc cdn waf rate-limits get",
    rules: Rules::Waf {
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
    },
};

impl RuleList {
    /// Refuses a rule of `parsed`, the file `file` names, with a key swagger does not list.
    fn check_keys(&self, parsed: &Value, file: &str) -> Result<()> {
        match self.rules {
            Rules::Cache => check_cache_rules(parsed, file),
            Rules::Waf { keys, .. } => check_rule_keys(parsed, file, self.noun, keys),
        }
    }

    /// The entries of `list`, a file or an answer, or `None` when it is not a list of rules.
    fn entries(&self, list: &Value) -> Option<Vec<Entry>> {
        match self.rules {
            Rules::Cache => cache_rule_entries(list),
            Rules::Waf {
                required, ordered, ..
            } => rule_entries(list, required, ordered),
        }
    }

    /// What a list is, as the refusal of a file of another shape says.
    fn shape(&self) -> String {
        match self.rules {
            Rules::Cache => "a JSON object with a `rules` array, or an array, of rules that each \
                             have a `match` object"
                .to_owned(),
            Rules::Waf { required, .. } => format!(
                "a JSON object with a `rules` array of rules that each have {}",
                phrase::ticked(required, "and")
            ),
        }
    }

    /// Whether the read gives a `version` the replace carries as `If-Match`.
    fn versioned(&self) -> bool {
        matches!(self.rules, Rules::Waf { .. })
    }
}

/// `GET` of the list, printed as it came.
pub(super) fn read_rules(ctx: &Context<'_>, zone_id: &ZoneId, list: &RuleList) -> Result<Printed> {
    let client = ctx.load_authed_client()?;
    print_body(ctx, &client, &rules_get(zone_id, list), Gate::Open)
}

fn rules_get(zone_id: &ZoneId, list: &RuleList) -> Request {
    Request::to(list.read, [zone_id])
        .about(format!("get the {} of the CDN zone `{zone_id}`", list.what))
        .listed_by(ZONES)
}

/// `PUT` of the list with the file's bytes, under the whole-list guard. A rule key swagger
/// does not list is refused first, then a file that is not a list of rules.
///
/// With `--check` the same PUT goes with `?dry_run=1` ([`check_rules`]).
pub(super) fn update_rules(
    ctx: &Context<'_>,
    update: &ListUpdate<'_>,
    list: &RuleList,
) -> Result<Printed> {
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
    list.check_keys(&parsed, &file.name())?;
    let Some(entries) = list.entries(&parsed) else {
        return Err(Error::usage(
            format!(
                "the {} from {} are not {}",
                list.what,
                file.name(),
                list.shape()
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
        write: Request::to(list.update, [zone_id])
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
        // The cache rules route has no version: swagger gives `If-Match` only for a zone's
        // WAF version, so that PUT cannot be made conditional, and a rule another client saves
        // between the read and the PUT is written over unseen. The help says so; send
        // `If-Match` there once the route takes one.
        draft_if_match: list.versioned().then_some(VERSION_PLACEHOLDER),
        noun: list.noun,
        owner: format!("the CDN zone `{zone_id}`"),
    };
    // A WAF PUT is made conditional on the rules read: it carries the read's `version` as
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
        let entries = list.entries(&answer);
        let found = if list.versioned() {
            let version = view_of::<cdn::WafRules>(&answer).version;
            entries.zip(version).map(|(entries, version)| Found {
                entries,
                body: None,
                if_match: Some(version.to_string()),
            })
        } else {
            entries.map(Found::new)
        };
        found.ok_or_else(|| crate::commands::unreadable(&read_action))
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
            Step::DryRun(doc) => Ok(doc.into()),
        }
    } else {
        replace_list(ctx, &client, &replace, current)
    };
    done.map_err(|error| {
        if !list.versioned() || error.http_status() != Some(412) {
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
    if let Write::DryRun(doc) =
        ctx.check_write(client, &WritePlan::new(&[&replace.read], &draft))?
    {
        return Ok(Step::DryRun(
            doc.reads_first(ctx.describe(client, &replace.read)?),
        ));
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
fn checked(response: &Response, zone_id: &ZoneId, list: &RuleList) -> Result<Value> {
    let answer = response.document()?;
    let check: cdn::WafCheck = view_of(&answer);
    match check.valid {
        Some(true) if !check.published() => Ok(answer),
        Some(false) if !check.published() => Err(response
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
fn check_rule_keys(parsed: &Value, file: &str, noun: &str, keys: &[&str]) -> Result<()> {
    let Some(rules) = rules_of(parsed) else {
        return Ok(());
    };
    for (index, rule) in rules.iter().enumerate() {
        let unknown = rule
            .as_object()
            .and_then(|rule| rule.keys().find(|key| !keys.contains(&key.as_str())));
        if let Some(key) = unknown {
            return Err(Error::usage(
                format!(
                    "`rules[{index}]` from {file} has the key `{key}`, which a {noun} does \
                     not have: a rule has only {}",
                    phrase::ticked(keys, "and")
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
/// also its place (`ordered`): moving one, or putting a new one ahead of it,
/// changes which rule a request meets first.
fn rule_entries(parsed: &Value, required: &[&str], ordered: bool) -> Option<Vec<Entry>> {
    rules_of(parsed)?
        .iter()
        .enumerate()
        .map(|(position, rule)| {
            let rule = rule.as_object()?;
            let mut key = Map::new();
            if ordered {
                key.insert("position".to_owned(), position.into());
            }
            for field in required {
                key.insert((*field).to_owned(), rule.get(*field)?.clone());
            }
            let enabled = rule.get("enabled").cloned().unwrap_or(Value::Bool(true));
            key.insert("enabled".to_owned(), enabled);
            Some(Entry::new(Value::Object(key)))
        })
        .collect()
}

/// The keys swagger lists for a cache rule (`PUT /api/cdn/zones/{zone_id}/rules/`).
const CACHE_RULE_KEYS: [&str; 2] = ["match", "actions"];

/// The keys swagger lists for a cache rule's `match`.
const MATCH_KEYS: [&str; 3] = ["path_prefix", "glob", "extensions"];

/// The keys swagger lists for a cache rule's `actions`.
const ACTION_KEYS: [&str; 5] = [
    "edge_ttl",
    "browser_ttl",
    "query_string",
    "bypass",
    "ignore_set_cookie",
];

/// Refuses a rule of the file that has a key swagger does not list, in the rule, its
/// `match`, or its `actions`, so a misspelled key cannot make the rule do something other
/// than what was written. The values, and the shape of the list itself, are left to
/// [`cache_rule_entries`] and the server.
fn check_cache_rules(list: &Value, file: &str) -> Result<()> {
    let Some(rules) = cache_rules(list) else {
        return Ok(());
    };
    for (index, rule) in rules.iter().enumerate() {
        if let Some(problem) = unknown_cache_key(rule) {
            return Err(Error::usage(
                format!("`rules[{index}]` from {file} {problem}"),
                format!("fix `rules[{index}]`; the zone keeps its rules until then"),
            ));
        }
    }
    Ok(())
}

/// The first key of `rule` that swagger does not list, as the end of a sentence that starts
/// with the rule's place in the file; `None` when every key is listed.
fn unknown_cache_key(rule: &Value) -> Option<String> {
    let listed = |keys: &[&str]| phrase::ticked(keys, "and");
    let unknown = |object: &Value, keys: &[&str]| {
        object
            .as_object()?
            .keys()
            .find(|key| !keys.contains(&key.as_str()))
            .cloned()
    };
    if let Some(key) = unknown(rule, &CACHE_RULE_KEYS) {
        return Some(format!(
            "has the key `{key}`, which a cache rule does not have: a rule has only {}",
            listed(&CACHE_RULE_KEYS)
        ));
    }
    for (part, keys, noun) in [
        ("match", &MATCH_KEYS[..], "a `match`"),
        ("actions", &ACTION_KEYS[..], "`actions`"),
    ] {
        if let Some(key) = rule.get(part).and_then(|object| unknown(object, keys)) {
            return Some(format!(
                "has the key `{key}` in its `{part}`: {noun} has only {}",
                listed(keys)
            ));
        }
    }
    None
}

/// The rules of a cache rule list: `{"rules": [...]}`, or a bare array, which swagger says
/// the PUT accepts too; `None` for any other shape.
fn cache_rules(list: &Value) -> Option<&Vec<Value>> {
    match list {
        Value::Array(rules) => Some(rules),
        list => list.get("rules")?.as_array(),
    }
}

/// The entries of a cache rule list: `{"rules": [...]}`, or a bare array, which swagger
/// says the PUT accepts too; `None` for any other shape, or for a rule without a `match`
/// object.
///
/// The list is ordered and the first match wins (swagger), so a rule is one entry by its
/// place and its `match`: `{"position", "match"}`. Moving a rule, or putting a new one ahead
/// of it, changes which rule a path meets first, so it counts as removing the rule from its
/// old place. Changing a rule's `actions` in place edits it. Of two rules that share a
/// `match` only the first ever applies, so the place tells them apart and their `actions`
/// need not. Swagger says a leading dot of an extension is dropped, so `extensions` is
/// compared without one, as a set. A rule without a `match` makes the list unreadable
/// rather than a shared `null` key.
fn cache_rule_entries(list: &Value) -> Option<Vec<Entry>> {
    let rules = cache_rules(list)?;
    let matches = rules
        .iter()
        .map(|rule| cache_match(rule.get("match")?))
        .collect::<Option<Vec<Value>>>()?;
    let entries = matches
        .into_iter()
        .enumerate()
        .map(|(position, key)| Entry::new(json!({ "position": position, "match": key })))
        .collect();
    Some(entries)
}

/// A rule's `match` as a key: the object with `extensions` as a set without leading dots;
/// `None` for a `match` that is not an object.
fn cache_match(found: &Value) -> Option<Value> {
    let mut key = found.as_object()?.clone();
    if let Some(Value::Array(extensions)) = key.get("extensions") {
        let bare = extensions
            .iter()
            .map(|e| match e.as_str() {
                Some(text) => text.strip_prefix('.').unwrap_or(text).into(),
                None => e.clone(),
            })
            .collect();
        key.insert("extensions".to_owned(), replace::set(bare));
    }
    Some(Value::Object(key))
}
