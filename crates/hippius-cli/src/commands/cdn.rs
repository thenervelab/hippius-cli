//! `hipc cdn`: CDN zones, their spend cap, custom hostnames, purges, cache rules, and
//! traffic. The WAF and the WAF lists are in [`waf`].
//!
//! Every `/api/cdn/` route these commands call is account-scoped, so a membership sends
//! `X-Hippius-Account`. CDN is usage, not a plan: there is no subscription command, and
//! the prices are `GET /api/cdn/prices/`, never a constant here.
//!
//! `estimate` is a POST that changes nothing, marked so (`Request::changes_nothing`): a
//! lost answer is a read's, with no side effects and safe to retry. `hostnames verify` checks
//! the DNS records again, which `hipc schema` calls idempotent, not read-only, so it stays a
//! write.

pub(crate) mod waf;

use std::cmp::Ordering;

use hippius::{Client, DnsCredentials, Error, Method, Request, Result, phrase};
use serde_json::{Map, Value, json};

use crate::cli::{
    Cdn, CdnCap, CdnEstimate, CdnHostnames, CdnPurge, CdnPurges, CdnRules, CdnStats, CdnZones,
    DnsSetup, StatsStep, ZoneCreate, ZoneSettings, ZoneUpdate,
};
use crate::commands::credits::{DECIMALS, amount, compare};
use crate::commands::replace::{self, Entry, Found, Replace, replace_list};
use crate::commands::{
    Input, Printed, print_body, print_body_or, print_keyed_list, print_with, require_public,
};
use crate::context::{self, Context, Gate, Write};

/// The largest `--cors-file` read, in bytes. Arbitrary: swagger's limits (32 origins, and
/// 32 headers each way) fit in a few kilobytes, and a wrong path is refused rather than sent.
const MAX_CORS_BYTES: u64 = 64 * 1024;

/// The largest `--rules-file` read, in bytes. Arbitrary: 50 rules, the server's limit, fit
/// in a few kilobytes, and a wrong path is refused rather than sent.
const MAX_RULES_BYTES: u64 = 1024 * 1024;

pub(crate) fn run(ctx: &Context<'_>, command: &Cdn) -> Result<Printed> {
    match command {
        // Swagger documents the price list as public, so it sends a token only when one is
        // configured.
        Cdn::Prices => print_body(
            ctx,
            &ctx.client(&context::config()?)?,
            &Request::get("/cdn/prices/").about("get the CDN prices"),
            Gate::Open,
        ),
        Cdn::Regions => print_body(
            ctx,
            &ctx.load_authed_client()?,
            &Request::get("/cdn/regions/").about("list the CDN regions"),
            Gate::Open,
        ),
        Cdn::Estimate(args) => estimate(ctx, args),
        Cdn::Zones(command) => zones(ctx, command),
        Cdn::Cap(command) => cap(ctx, &ctx.load_authed_client()?, command),
        Cdn::Hostnames(command) => hostnames(ctx, command),
        Cdn::Purges(command) => purges(ctx, &ctx.load_authed_client()?, command),
        Cdn::Rules(command) => rules(ctx, command),
        Cdn::Stats(args) => stats(ctx, &ctx.load_authed_client()?, args),
        Cdn::Waf(command) => waf::waf(ctx, command),
        Cdn::Lists(command) => waf::lists(ctx, command),
    }
}

/// The command that lists the zone ids, for the hint of a 404 on a zone route.
const ZONES: &str = "hipc cdn zones list";

fn zone_path(zone_id: &str) -> String {
    format!("/cdn/zones/{zone_id}/")
}

fn cap(ctx: &Context<'_>, client: &Client, command: &CdnCap) -> Result<Printed> {
    match command {
        CdnCap::Get { zone_id } => {
            let request = Request::get(format!("{}cap/", zone_path(zone_id)))
                .about(format!("get the spend cap of the CDN zone `{zone_id}`"))
                .listed_by(ZONES);
            print_body(ctx, client, &request, Gate::Open)
        }
        CdnCap::Update {
            zone_id,
            monthly_cap,
        } => cap_update(ctx, client, zone_id, monthly_cap),
    }
}

/// `credits`, an amount of credits, in base units (`10^18` per credit) as a decimal string
/// without leading zeros, or `None` when it is not an amount or has more than 18 decimals.
fn base_units(credits: &str) -> Option<String> {
    let (whole, fraction) = amount(credits)?;
    let places = DECIMALS as usize;
    if fraction.len() > places {
        return None;
    }
    let digits = format!("{whole}{fraction:0<places$}");
    let digits = digits.trim_start_matches('0');
    Some(if digits.is_empty() { "0" } else { digits }.to_owned())
}

/// `PATCH .../cap/ {"monthly_cap"}`, after a read of the cap in force. Only a change that can
/// raise spend is confirmed (tenet 5), as every spend cap is: a higher cap than the read's
/// `monthly_cap` (base units) or, without that, its `monthly_cap_credits`, or any cap hipc
/// cannot compare. A lower cap, or
/// the same one, is sent unasked. The dry run lists the read, then the write.
fn cap_update(
    ctx: &Context<'_>,
    client: &Client,
    zone_id: &str,
    monthly_cap: &str,
) -> Result<Printed> {
    let path = format!("{}cap/", zone_path(zone_id));
    let set =
        format!("set the monthly spend cap of the CDN zone `{zone_id}` to `{monthly_cap}` credits");
    // The cap is the one writable field: the server refuses any other key.
    let patch = |about: &str| {
        Request::new(Method::Patch, &path)
            .listed_by(ZONES)
            .json(&json!({ "monthly_cap": monthly_cap }))
            .about(about)
    };
    let read = Request::get(&path)
        .about(format!("get the spend cap of the CDN zone `{zone_id}`"))
        .listed_by(ZONES);
    let draft = patch(&set);
    let checked = match ctx.check_write(client, &draft)? {
        Write::DryRun(mut doc) => {
            doc["requests"] = ctx.describe_each(client, &[&read, &draft])?;
            return Ok(Printed::Document(doc));
        }
        Write::Checked(checked) => checked,
    };
    let current = ctx.send(client, &read)?.document()?;
    let old = match current.get("monthly_cap_credits") {
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Number(number)) => Some(number.to_string()),
        _ => None,
    };
    // `monthly_cap_credits` is rounded to a few decimals, so a cap just above the one in force
    // could read as equal; `monthly_cap` holds it exactly, in base units, and decides when it
    // is a digit string. The parser refuses a new cap with more decimals than base units
    // hold, so the new cap always has base units, and the order is unknown only when the read
    // holds no amount to compare with.
    let base = current
        .get("monthly_cap")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()));
    let order = match (base, base_units(monthly_cap)) {
        (Some(base), Some(new)) => compare(&new, base),
        _ => old.as_deref().and_then(|old| compare(monthly_cap, old)),
    };
    let question = match (order, old.as_deref()) {
        (Some(Ordering::Less | Ordering::Equal), _) => None,
        (Some(Ordering::Greater), Some(old)) => Some(format!(
            "raise the monthly spend cap of the CDN zone `{zone_id}` from {} to \
             `{monthly_cap}` credits",
            phrase::quote(old)
        )),
        (Some(Ordering::Greater), None) => Some(format!(
            "raise the monthly spend cap of the CDN zone `{zone_id}` to `{monthly_cap}` credits"
        )),
        (None, Some(old)) => Some(format!(
            "{set}, where `monthly_cap_credits` is {}, which is not an amount of credits",
            phrase::quote(old)
        )),
        (None, None) => Some(format!(
            "{set}, where the cap in force is unknown: the cap has no amount in \
             `monthly_cap_credits`"
        )),
    };
    let request = match question {
        Some(action) => {
            let request = patch(&action);
            ctx.confirm_write(
                checked,
                &request.action(),
                Some("the zone may then spend more of the balance each month"),
            )?;
            request
        }
        None => draft,
    };
    ctx.send(client, &request)?
        .document()
        .map(Printed::Document)
}

fn purges(ctx: &Context<'_>, client: &Client, command: &CdnPurges) -> Result<Printed> {
    match command {
        CdnPurges::Create(args) => purge(ctx, client, args),
        // Swagger gives this list no shape, and no zone was on the account to check it: a
        // bare array or an object holding `purges` is read.
        CdnPurges::List { zone_id } => {
            let path = format!("{}purges/", zone_path(zone_id));
            let request = Request::get(path)
                .about(format!("list the purges of the CDN zone `{zone_id}`"))
                .listed_by(ZONES);
            print_keyed_list(ctx, client, &request, "purges")
        }
        CdnPurges::Get { zone_id, purge_id } => {
            let request = Request::get(format!("{}purges/{purge_id}/", zone_path(zone_id)))
                .about(format!(
                    "get the purge `{purge_id}` of the CDN zone `{zone_id}`"
                ))
                .listed_by(format!("hipc cdn purges list {zone_id}"));
            print_body(ctx, client, &request, Gate::Open)
        }
    }
}

fn rules(ctx: &Context<'_>, command: &CdnRules) -> Result<Printed> {
    match command {
        CdnRules::Get { zone_id } => print_body(
            ctx,
            &ctx.load_authed_client()?,
            &rules_get(zone_id),
            Gate::Open,
        ),
        CdnRules::Update {
            zone_id,
            rules_file,
            remove_others,
        } => {
            let rules = Input {
                path: rules_file,
                flag: "--rules-file",
                what: "cache rules",
            };
            let (body, parsed) = rules.read_json(MAX_RULES_BYTES)?;
            check_cache_rules(&parsed, &rules.name())?;
            let Some(entries) = cache_rule_entries(&parsed) else {
                return Err(Error::usage(
                    format!(
                        "the cache rules from {} are not a JSON object with a `rules` array, \
                         or an array, of rules that each have a `match` object",
                        rules.name()
                    ),
                    "write the rules as `{\"rules\": [...]}`, the shape `hipc cdn rules get` \
                     prints",
                ));
            };
            let read = rules_get(zone_id);
            let read_action = read.action();
            let replace = Replace {
                write: Request::new(Method::Put, format!("{}rules/", zone_path(zone_id)))
                    .listed_by(ZONES)
                    .raw_json(body)
                    .about(format!(
                        "replace every cache rule of the CDN zone `{zone_id}` with the rules \
                         from {}",
                        rules.name()
                    )),
                consequence: None,
                entries,
                remove_others: *remove_others,
                read,
                draft_body: None,
                // The rules route has no version: swagger gives `If-Match` only for a zone's
                // WAF version, so this PUT cannot be made conditional, and a rule another
                // client saves between the read and the PUT is written over unseen. The help
                // says so; send `If-Match` here once the route takes one.
                draft_if_match: None,
                noun: "cache rule",
                owner: format!("the CDN zone `{zone_id}`"),
            };
            let current = |answer: Value| {
                cache_rule_entries(&answer)
                    .map(Found::new)
                    .ok_or_else(|| crate::commands::unreadable(&read_action))
            };
            replace_list(ctx, &ctx.load_authed_client()?, &replace, current)
        }
    }
}

fn rules_get(zone_id: &str) -> Request {
    Request::get(format!("{}rules/", zone_path(zone_id)))
        .about(format!("get the cache rules of the CDN zone `{zone_id}`"))
        .listed_by(ZONES)
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

/// `POST /api/cdn/estimate/` with only the keys given. It changes nothing, so it is not
/// confirmed.
fn estimate(ctx: &Context<'_>, args: &CdnEstimate) -> Result<Printed> {
    let mut body = Map::new();
    if !args.traffic.is_empty() {
        let mut gb = Map::new();
        for entry in &args.traffic {
            let repeated = gb
                .insert(entry.region.clone(), Value::Number(entry.gb.clone()))
                .is_some();
            if repeated {
                return Err(Error::usage(
                    format!(
                        "`--traffic` names the region `{}` more than once",
                        entry.region
                    ),
                    "pass each region once",
                ));
            }
        }
        body.insert("gb".to_owned(), Value::Object(gb));
    }
    for (key, value) in [
        ("requests", args.requests),
        ("custom_domains", args.custom_hostnames),
    ] {
        if let Some(value) = value {
            body.insert(key.to_owned(), value.into());
        }
    }
    if !args.waf.is_empty() {
        let mut waf = Map::new();
        for entry in &args.waf {
            let tier = entry.tier.code();
            if waf.insert(tier.to_owned(), entry.zones.into()).is_some() {
                return Err(Error::usage(
                    format!("`--waf` names the tier `{tier}` more than once"),
                    "pass each tier once",
                ));
            }
        }
        body.insert("waf".to_owned(), Value::Object(waf));
    }
    let request = Request::new(Method::Post, "/cdn/estimate/")
        .json(&Value::Object(body))
        .about("estimate the CDN cost")
        .changes_nothing();
    print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
}

fn zones(ctx: &Context<'_>, command: &CdnZones) -> Result<Printed> {
    match command {
        // The API answers `{zones, beta}`; `beta` is kept.
        CdnZones::List => print_keyed_list(
            ctx,
            &ctx.load_authed_client()?,
            &Request::get("/cdn/zones/").about("list the CDN zones"),
            "zones",
        ),
        CdnZones::Get { zone_id } => {
            let request = Request::get(zone_path(zone_id))
                .about(format!("get the CDN zone `{zone_id}`"))
                .listed_by(ZONES);
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        CdnZones::Create(args) => create_zone(ctx, args),
        CdnZones::Update(args) => update_zone(ctx, args),
        CdnZones::Delete { zone_id } => {
            let request = Request::new(Method::Delete, zone_path(zone_id))
                .listed_by(ZONES)
                .about(format!("delete the CDN zone `{zone_id}`"));
            let gate = Gate::Confirm {
                consequence: Some(ZONE_DELETED),
            };
            let done = json!({ "zone_id": zone_id, "deleted": true });
            print_body_or(ctx, &ctx.load_authed_client()?, &request, gate, done)
        }
    }
}

/// `POST /api/cdn/zones/`. The answer's `origin.private` says whether the zone made a
/// private bucket public; text mode says so on stderr.
///
/// Whether the bucket is private is known only from that answer, after the zone exists:
/// swagger gives a bucket list with names only, and a bucket ACL read returns an S3 policy
/// `hipc` would have to judge, which a member with CDN admin but not S3 admin may not be
/// allowed to read. So every create needs `--public`, the safe side.
fn create_zone(ctx: &Context<'_>, args: &ZoneCreate) -> Result<Printed> {
    check_settings(&args.settings)?;
    let cors = read_cors(&args.settings)?;
    let published = published(&args.bucket, args.settings.prefix.as_deref());
    require_public(
        ctx,
        args.public,
        &format!(
            "the CDN zone would make `{published}` readable by anyone if the bucket `{}` is \
             private",
            args.bucket
        ),
    )?;
    let client = &ctx.load_authed_client()?;
    let mut origin = Map::new();
    origin.insert("kind".to_owned(), "s3".into());
    origin.insert("bucket".to_owned(), args.bucket.as_str().into());
    let mut settings = Map::new();
    settings_into(&args.settings, &mut origin, &mut settings);
    let mut body = Map::new();
    body.insert("name".to_owned(), args.name.as_str().into());
    body.insert("origin".to_owned(), Value::Object(origin));
    body.extend(settings);
    if let Some(cap) = &args.monthly_cap {
        body.insert("monthly_cap".to_owned(), cap.as_str().into());
    }
    let with_cors = match cors {
        Some(Cors { policy, file }) => {
            let with = if policy.is_null() {
                ", with no CORS policy".to_owned()
            } else {
                format!(", with the whole CORS policy in {file}")
            };
            body.insert("cors".to_owned(), policy);
            with
        }
        None => String::new(),
    };
    // That a private bucket's prefix becomes public is part of the action, so the refusal an
    // agent reads says it too, not only the prompt.
    let request = Request::new(Method::Post, "/cdn/zones/")
        .json(&Value::Object(body))
        .about(format!(
            "create the CDN zone `{}` serving `{published}`, which makes that prefix public if \
             the bucket is private{with_cors}",
            args.name,
        ));
    let gate = Gate::Confirm {
        consequence: Some("it is billed by traffic"),
    };
    print_with(ctx, client, &request, gate, |response| {
        let zone = response.document()?;
        if zone.pointer("/origin/private") == Some(&Value::Bool(true)) {
            ctx.warning(&format!(
                "this zone makes the prefix `{published}` public: anyone can read it through the \
                 zone's hostnames"
            ));
        }
        Ok(zone)
    })
}

/// `PATCH /api/cdn/zones/<id>/` with only the keys given. It is always confirmed, because
/// every field it sets but `--name` changes what the zone serves or how, and a new origin
/// also purges the zone and may publish a private bucket; a rename alone is confirmed too,
/// rather than add a condition for the one field that changes nothing the zone serves.
/// A new origin also needs `--public`, as a create does ([`create_zone`]): whether the
/// bucket is private is known only from the answer. A change that keeps the origin
/// publishes nothing new.
fn update_zone(ctx: &Context<'_>, args: &ZoneUpdate) -> Result<Printed> {
    check_settings(&args.settings)?;
    let cors = read_cors(&args.settings)?;
    let mut origin = Map::new();
    if let Some(bucket) = &args.bucket {
        origin.insert("bucket".to_owned(), bucket.as_str().into());
    }
    let mut body = Map::new();
    if let Some(name) = &args.name {
        body.insert("name".to_owned(), name.as_str().into());
    }
    if let Some(redirect) = args.redirect_https {
        body.insert("redirect_https".to_owned(), redirect.into());
    }
    settings_into(&args.settings, &mut origin, &mut body);
    if body.is_empty() && origin.is_empty() && cors.is_none() {
        // An empty PATCH changes nothing and only returns the zone.
        return Err(Error::usage(
            "`hipc cdn zones update` was given nothing to change",
            "pass at least one field to change, such as `--name`",
        ));
    }
    // The question names every change, so the refusal an agent reads says what would change
    // and which bucket and prefix would be published.
    let set: Vec<String> = body
        .iter()
        .map(|(key, value)| format!("`{key}` to {}", shown(value)))
        .collect();
    let set = match set.as_slice() {
        [] => String::new(),
        set => format!("set {}", phrase::join(set, "and")),
    };
    // The policy is named by its file, not by its keys: it replaces the whole policy, which
    // is what `--yes` agrees to.
    let set = match cors {
        Some(Cors { policy, file }) => {
            let cors = if policy.is_null() {
                "remove its CORS policy".to_owned()
            } else {
                format!("replace its whole CORS policy with the one in {file}")
            };
            body.insert("cors".to_owned(), policy);
            if set.is_empty() {
                cors
            } else {
                format!("{set}, and {cors}")
            }
        }
        None => set,
    };
    let action = if origin.is_empty() {
        format!("update the CDN zone `{}`: {set}", args.zone_id)
    } else {
        require_public(
            ctx,
            args.public,
            &format!(
                "the new origin of the CDN zone `{}` would be readable by anyone if its bucket is \
                 private",
                args.zone_id
            ),
        )?;
        let to: Vec<String> = origin
            .iter()
            .map(|(key, value)| format!("the {key} {}", shown(value)))
            .collect();
        let and_set = if set.is_empty() {
            String::new()
        } else {
            format!(", and {set}")
        };
        body.insert("origin".to_owned(), Value::Object(origin));
        format!(
            "move the CDN zone `{}` to {}, which purges the whole zone and makes what it then \
             serves public if the bucket is private{and_set}",
            args.zone_id,
            phrase::join(&to, "and")
        )
    };
    let request = Request::new(Method::Patch, zone_path(&args.zone_id))
        .listed_by(ZONES)
        .json(&Value::Object(body))
        .about(action);
    let client = ctx.load_authed_client()?;
    print_body(ctx, &client, &request, Gate::Confirm { consequence: None })
}

/// The CORS policy `--cors-file` holds, and the file as a question names it.
#[derive(Debug)]
struct Cors {
    /// A JSON object, or `null`, which removes the policy. It replaces the whole policy
    /// (swagger `cors`: "absent keys take their defaults"); its keys are the server's to
    /// judge (400 `invalid_cors`).
    policy: Value,
    file: String,
}

/// Reads `--cors-file` when it is given.
fn read_cors(settings: &ZoneSettings) -> Result<Option<Cors>> {
    let Some(path) = &settings.cors_file else {
        return Ok(None);
    };
    let input = Input {
        path,
        flag: "--cors-file",
        what: "CORS policy",
    };
    let policy = input.read_value(MAX_CORS_BYTES)?;
    if !(policy.is_object() || policy.is_null()) {
        return Err(Error::usage(
            format!(
                "the CORS policy from {} is not a JSON object or null",
                input.name()
            ),
            "write the policy as a JSON object with `allow_origins`, or `null` to remove it, in \
             the file `--cors-file` names",
        ));
    }
    Ok(Some(Cors {
        policy,
        file: input.name(),
    }))
}

/// The most SPA fallback exclusions a zone keeps (swagger, `spa_fallback_exclude` of `POST
/// /api/cdn/zones/`: `maxItems` 16).
const MAX_SPA_EXCLUDES: usize = 16;

/// The most paths or prefixes one purge takes (swagger, `POST .../purge/`: `maxItems` 1000).
const MAX_PURGE_ITEMS: usize = 1000;

/// Refuses a count the server would refuse after the confirmation; each value's shape is
/// the parser's (`cli/cdn.rs`).
fn check_settings(settings: &ZoneSettings) -> Result<()> {
    check_count(
        "--spa-fallback-exclude",
        settings.spa_fallback_exclude.len(),
        MAX_SPA_EXCLUDES,
        "a zone keeps",
    )
}

/// `usage` when `flag` is given more than `max` times. `holder` ends the sentence, such as
/// "a purge takes".
fn check_count(flag: &str, given: usize, max: usize, holder: &str) -> Result<()> {
    if given <= max {
        return Ok(());
    }
    Err(Error::usage(
        format!("`{flag}` is given {given} times, and {holder} at most {max}"),
        format!("pass `{flag}` at most {max} times"),
    ))
}

/// Adds the fields `create` and `update` share: `prefix` into the origin, the rest into
/// the body, each only when given.
fn settings_into(
    settings: &ZoneSettings,
    origin: &mut Map<String, Value>,
    body: &mut Map<String, Value>,
) {
    if let Some(prefix) = &settings.prefix {
        origin.insert("prefix".to_owned(), prefix.as_str().into());
    }
    for (key, value) in [
        ("shield_region", &settings.shield_region),
        ("index_document", &settings.index_document),
    ] {
        if let Some(value) = value {
            body.insert(key.to_owned(), value.as_str().into());
        }
    }
    if let Some(fallback) = settings.spa_fallback {
        body.insert("spa_fallback".to_owned(), fallback.into());
    }
    if !settings.spa_fallback_exclude.is_empty() {
        body.insert(
            "spa_fallback_exclude".to_owned(),
            settings.spa_fallback_exclude.clone().into(),
        );
    }
}

/// A field's new value as a question names it: in backticks, `""` for an empty string,
/// and a list as its items.
fn shown(value: &Value) -> String {
    match value {
        Value::String(text) if text.is_empty() => "`\"\"`".to_owned(),
        Value::String(text) => format!("`{text}`"),
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(shown).collect();
            phrase::join(&items, "and")
        }
        other => format!("`{other}`"),
    }
}

/// The part of a bucket a zone publishes, as `bucket/prefix`.
fn published(bucket: &str, prefix: Option<&str>) -> String {
    format!("{bucket}/{}", prefix.unwrap_or(""))
}

fn hostnames(ctx: &Context<'_>, command: &CdnHostnames) -> Result<Printed> {
    // Built in each arm, so `dns-setup` checks its credential file before the config file is
    // read (tenet 5).
    let client = || ctx.load_authed_client();
    let hostname_path =
        |zone_id: &str, hostname: &str| format!("{}hostnames/{hostname}/", zone_path(zone_id));
    match command {
        // Swagger gives this list no shape, and no zone was on the account to check it: a
        // bare array or an object holding `hostnames` is read.
        CdnHostnames::List { zone_id } => print_keyed_list(
            ctx,
            &client()?,
            &Request::get(format!("{}hostnames/", zone_path(zone_id)))
                .about(format!("list the hostnames of the CDN zone `{zone_id}`"))
                .listed_by(ZONES),
            "hostnames",
        ),
        CdnHostnames::Create { zone_id, hostname } => {
            let request = Request::new(Method::Post, format!("{}hostnames/", zone_path(zone_id)))
                .listed_by(ZONES)
                .json(&json!({ "hostname": hostname }))
                .about(format!(
                    "add the custom hostname `{hostname}` to the CDN zone `{zone_id}`"
                ));
            let gate = Gate::Confirm {
                consequence: Some("it is billed monthly"),
            };
            print_body(ctx, &client()?, &request, gate)
        }
        // A check of the DNS records: a repeat changes nothing, so it is not confirmed.
        CdnHostnames::Verify { zone_id, hostname } => {
            let path = format!("{}verify/", hostname_path(zone_id, hostname));
            let request = Request::new(Method::Post, path)
                .about(format!("verify the custom hostname `{hostname}`"))
                .listed_by(format!("hipc cdn hostnames list {zone_id}"));
            print_body(ctx, &client()?, &request, Gate::Open)
        }
        CdnHostnames::DnsSetup(args) => dns_setup(ctx, args),
        // Swagger documents no answer shape. It is printed as sent; when it holds the link as
        // a string `url`, `next_step` names the read that shows when the records are
        // published: hipc does not wait for a person in a browser, as with a card checkout
        // (tenet 3). Swagger: a 404 while the feature flag is off, and the console offers the
        // link only when the provider has the template, so a 404 says both.
        CdnHostnames::DomainConnect { zone_id, hostname } => {
            let path = format!("{}domain-connect/", hostname_path(zone_id, hostname));
            let request = Request::get(path)
                .about(format!(
                    "get the Domain Connect link of the custom hostname `{hostname}`"
                ))
                .listed_by(format!("hipc cdn hostnames list {zone_id}"));
            print_with(ctx, &client()?, &request, Gate::Open, |response| {
                let mut link = response.document()?;
                crate::commands::drop_next_step(&mut link);
                if let Value::Object(fields) = &mut link
                    && fields.get("url").is_some_and(Value::is_string)
                {
                    // The URL is not quoted in the note: it is on stdout, and a quoted URL
                    // would lose its query values (tenet 9). The next command carries this
                    // run's origin and selectors, and is left out when one of them is not a
                    // plain word a shell would keep whole.
                    let open = "open the `url` in the output to publish the records";
                    match ctx.next_command(&format!("cdn hostnames list {zone_id}")) {
                        Some(next) => {
                            ctx.note(&format!("{open}; `{next}` shows the hostname's status"));
                            fields.insert("next_step".to_owned(), Value::from(next));
                        }
                        None => ctx.note(open),
                    }
                }
                Ok(link)
            })
            .map_err(|error| {
                if error.http_status() == Some(404) {
                    error.with_hint(format!(
                        "check the hostname with `hipc cdn hostnames list {zone_id}`; if it is \
                         there, its DNS provider has no Domain Connect template, or Domain \
                         Connect is off on this server"
                    ))
                } else {
                    error
                }
            })
        }
        CdnHostnames::Delete { zone_id, hostname } => {
            let request = Request::new(Method::Delete, hostname_path(zone_id, hostname))
                .listed_by(format!("hipc cdn hostnames list {zone_id}"))
                .about(format!(
                    "remove the custom hostname `{hostname}` from the CDN zone `{zone_id}`"
                ));
            let gate = Gate::Confirm {
                consequence: Some(HOSTNAME_DELETED),
            };
            let done = json!({ "zone_id": zone_id, "hostname": hostname, "deleted": true });
            print_body_or(ctx, &client()?, &request, gate, done)
        }
    }
}

/// `POST /api/cdn/zones/<id>/hostnames/<hostname>/dns-setup/` with `{provider, credentials}`
/// (swagger `cdn_zones_hostnames_dns-setup_create`). The credential comes from a private file
/// only, checked before anything else (`hippius::DnsCredentials`), and a dry run shows its
/// field names with a placeholder for each value. Swagger's `mode`, `replace`, and `confirm`
/// are not sent: the server's default writes every record, and overwriting a record that
/// conflicts is left to the console.
///
/// The server never echoes the credential, but if an answer or an error did, it would reach
/// stdout or stderr (tenet 9), in forms no list of spellings can foresee (base64, a Python
/// repr, `\uXXXX` escapes, a truncated copy). So nothing the server wrote as text is printed:
/// the answer keeps only values hipc can check against what it sent or against swagger's enums
/// ([`dns_setup_answer`]), and an error says only its status, not the server's words
/// ([`dns_setup_error`]). `--debug` prints no body.
fn dns_setup(ctx: &Context<'_>, args: &DnsSetup) -> Result<Printed> {
    let DnsSetup {
        zone_id,
        hostname,
        provider,
        credentials_file,
    } = args;
    let provider = provider.provider();
    let credentials = DnsCredentials::load(credentials_file, "--credentials-file", provider)?;
    let path = format!("{}hostnames/{hostname}/dns-setup/", zone_path(zone_id));
    let request = |credentials: Value| {
        Request::new(Method::Post, path.as_str())
            .json(&json!({ "provider": provider.code(), "credentials": credentials }))
            .listed_by(format!("hipc cdn hostnames list {zone_id}"))
            .about(format!(
                "write the DNS records of the custom hostname `{hostname}` at `{}`",
                provider.code()
            ))
    };
    let client = ctx.load_authed_client()?;
    let draft = request(credentials.placeholder_body());
    let checked = match ctx.check_write(&client, &draft)? {
        Write::DryRun(doc) => return Ok(Printed::Document(doc)),
        Write::Checked(checked) => checked,
    };
    let consequence = "it adds or changes records in the domain's DNS zone at the provider";
    ctx.confirm_write(checked, &draft.action(), Some(consequence))?;
    let action = draft.action();
    ctx.send(&client, &request(credentials.body()))
        .and_then(|response| response.document())
        .map(|done| Printed::Document(dns_setup_answer(done, provider.code())))
        .map_err(|error| dns_setup_error(error, &credentials, &action))
}

/// The values of `mode` swagger lists for the DNS setup.
const DNS_SETUP_MODES: [&str; 2] = ["all", "verification"];

/// The DNS setup's answer as printed. Swagger documents it as the request's serializer:
/// `provider`, `mode`, `credentials`, `replace`, and `confirm`. What is kept holds no text the
/// server chose: `provider` only when it is the code that was sent, `mode` only when it is one
/// of swagger's values, and `replace` only as a boolean. `confirm` is a string the server
/// writes (swagger: "The confirmation string a 409 asked for"), so it is dropped, as are
/// `credentials` and every other key at any depth. An answer that is not an object prints as
/// an empty one.
fn dns_setup_answer(answer: Value, provider: &str) -> Value {
    let Value::Object(fields) = answer else {
        return Value::Object(Map::new());
    };
    Value::Object(
        fields
            .into_iter()
            .filter(|(name, value)| match name.as_str() {
                "provider" => value.as_str() == Some(provider),
                "mode" => value
                    .as_str()
                    .is_some_and(|mode| DNS_SETUP_MODES.contains(&mode)),
                "replace" => value.is_boolean(),
                _ => false,
            })
            .collect(),
    )
}

/// A failed DNS setup, without the server's words. Tenet 15 quotes the server's text in a
/// message and keeps its body in `details`; here the body may repeat the credential in a form
/// hipc cannot recognise, so the message says only the status and `details` is dropped. The
/// code, the hint, `retryable`, and `side_effects` stay as the status set them, and
/// `request_id`, a header, is kept once each spelling of the secrets is redacted from it. An
/// error with no status (a refused connection, a timeout) holds no server text and is kept.
fn dns_setup_error(error: Error, credentials: &DnsCredentials, action: &str) -> Error {
    let error = error.redacted_all(&credentials.redactions());
    match error.http_status() {
        Some(status) => error.without_server_words(format!(
            "cannot {action}: the API answered with status `{status}`; its words are not shown, \
             since they may repeat the credential"
        )),
        None => error,
    }
}

/// `POST /api/cdn/zones/<id>/purge/` with exactly one of `all`, `paths`, and `prefixes`
/// (the parser requires exactly one). A purge spends nothing, so it is not confirmed; the
/// cost is that the next requests fetch from the bucket again.
fn purge(ctx: &Context<'_>, client: &Client, args: &CdnPurge) -> Result<Printed> {
    check_count("--path", args.paths.len(), MAX_PURGE_ITEMS, "a purge takes")?;
    check_count(
        "--prefix",
        args.prefixes.len(),
        MAX_PURGE_ITEMS,
        "a purge takes",
    )?;
    let body = if args.everything {
        json!({ "all": true })
    } else if args.paths.is_empty() {
        json!({ "prefixes": args.prefixes })
    } else {
        json!({ "paths": args.paths })
    };
    let request = Request::new(Method::Post, format!("{}purge/", zone_path(&args.zone_id)))
        .json(&body)
        .about(format!("purge the CDN zone `{}`", args.zone_id))
        .listed_by(ZONES);
    print_with(ctx, client, &request, Gate::Open, |response| {
        let body = response.document()?;
        // A zone id starts with `z`, so the printed command cannot read it as a flag.
        if let Some(id) = body.get("id").and_then(Value::as_u64)
            && let Some(command) =
                ctx.next_command(&format!("cdn purges get {} {id}", args.zone_id))
        {
            ctx.note(&format!("`{command}` shows its progress"));
        }
        Ok(body)
    })
}

/// `GET /api/cdn/zones/<id>/stats/` with the query parameters the console sends (`from`,
/// `to`, `step`, `region`), each only when given. Swagger names none, and no zone was on
/// the account to check them.
fn stats(ctx: &Context<'_>, client: &Client, args: &CdnStats) -> Result<Printed> {
    let request = Request::get(format!("{}stats/", zone_path(&args.zone_id)))
        .about(format!(
            "get the traffic of the CDN zone `{}`",
            args.zone_id
        ))
        .listed_by(ZONES);
    let query = [
        ("from", args.from.as_deref()),
        ("to", args.to.as_deref()),
        ("step", args.step.map(StatsStep::code)),
        ("region", args.region.as_deref()),
    ];
    print_body(ctx, client, &with_query(request, &query), Gate::Open)
}

/// `request` with each query parameter of `query` that has a value, in order.
fn with_query(mut request: Request, query: &[(&str, Option<&str>)]) -> Request {
    for (name, value) in query {
        if let Some(value) = value {
            request = request.query(name, value);
        }
    }
    request
}

/// What a CDN zone delete ends, for its question and `hipc api`'s.
pub(crate) const ZONE_DELETED: &str = "its hostnames stop serving, and its billing stops";

/// What a CDN hostname delete ends, for its question and `hipc api`'s.
pub(crate) const HOSTNAME_DELETED: &str = "it stops serving, and its billing stops";

#[cfg(test)]
mod tests {
    /// Base units hold 18 decimals; the `--monthly-cap` parser refuses a finer cap, so only
    /// these direct calls reach the `None`.
    #[test]
    fn base_units_hold_18_decimals() {
        assert_eq!(
            super::base_units("20").as_deref(),
            Some("20000000000000000000")
        );
        assert_eq!(
            super::base_units("0.000000000000000001").as_deref(),
            Some("1")
        );
        assert_eq!(super::base_units("0").as_deref(), Some("0"));
        assert_eq!(super::base_units("1.0000000000000000001"), None);
        assert_eq!(super::base_units("twenty"), None);
    }
}
