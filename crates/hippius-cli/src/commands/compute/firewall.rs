//! `hipc compute firewall update`: the rules file, read and checked rule by rule, and the
//! guarded replace of a VM's inbound rules. The rules a file holds and the rules the server
//! answers are compared as entries ([`rule_entry`]), so a rule the file leaves out is seen.

use std::net::Ipv4Addr;
use std::path::Path;

use hippius::{Error, Method, Request, Result, phrase};
use serde_json::{Map, Value, json};

use super::{VMS, firewall_get, firewall_path};
use crate::cli::FirewallUpdate;
use crate::commands::replace::{self, Entry, Found, Replace, replace_list};
use crate::commands::{Input, Printed};
use crate::context::Context;

/// What a dry run shows for the firewall PUT's `base_revision` when `--base-revision` is not
/// given. A real run fills it with the `revision` its read of the rules returns, and a dry
/// run sends nothing.
const REVISION_PLACEHOLDER: &str = "<the revision the read returns>";

/// `PUT .../firewall/`: the file's `rules` array, unchanged, plus `base_revision`: the one
/// given, or else the `revision` the guard's read returned. With `--remove-others` that read
/// compares nothing and comes after the question, for the revision alone, so a rule another
/// client adds meanwhile is refused with 409 rather than deleted unseen. The PUT replaces the whole list, so a file that does not parse,
/// or any rule that is malformed, refuses the whole update before anything is sent: a list
/// cut short would close every port it no longer names. A rule the VM has now that the
/// file leaves out is refused unless `--remove-others` is passed ([`replace_list`]).
pub(super) fn update(ctx: &Context<'_>, args: &FirewallUpdate) -> Result<Printed> {
    let (rules, entries) = firewall_rules(&args.rules_file)?;
    let vm_id = &args.vm_id;
    let (action, consequence) = firewall_question(vm_id, &args.rules_file, &rules);
    let put_body = |revision: Option<&str>| {
        let mut body = Map::new();
        body.insert("rules".to_owned(), Value::Array(rules.clone()));
        if let Some(revision) = revision {
            body.insert("base_revision".to_owned(), revision.into());
        }
        Value::Object(body)
    };
    let read = firewall_get(vm_id);
    let read_action = read.action();
    let replace = Replace {
        write: Request::new(Method::Put, firewall_path(vm_id))
            .listed_by(VMS)
            .json(&put_body(args.base_revision.as_deref()))
            .about(action.as_str()),
        consequence: Some(consequence),
        entries,
        remove_others: args.remove_others,
        read,
        draft_body: args
            .base_revision
            .is_none()
            .then(|| put_body(Some(REVISION_PLACEHOLDER))),
        draft_if_match: None,
        noun: "inbound rule",
        owner: format!("the VM `{vm_id}`"),
    };
    // The guard compared the file with the rules it read, so the PUT is made conditional on
    // them: without `--base-revision`, it carries the `revision` that read returned, and the
    // server refuses it with 409 if another client changed the rules in between. Without
    // that, a rule added after the read would be deleted unseen. A `--base-revision` given
    // is sent as it is.
    let current = |answer: Value| {
        let unreadable = || crate::commands::unreadable(&read_action);
        let rules = answer.get("rules").and_then(Value::as_array);
        let entries = rules
            .ok_or_else(unreadable)?
            .iter()
            .map(firewall_entry)
            .collect::<Option<Vec<Entry>>>()
            .ok_or_else(unreadable)?;
        let body = if args.base_revision.is_some() {
            None
        } else {
            let revision = answer.get("revision").and_then(Value::as_str);
            Some(put_body(Some(revision.ok_or_else(unreadable)?)))
        };
        Ok(Found {
            entries,
            body,
            if_match: None,
        })
    };
    replace_list(ctx, &ctx.load_authed_client()?, &replace, current).map_err(|error| {
        // Swagger: a stale `base_revision` is refused with 409 `firewall-changed`. It
        // documents no other 409 on this PUT and no shape for the body, so the status alone
        // decides: matching a body field could drop the hint if the field is not the one
        // guessed. The GET before it documents no 409.
        // Without `--base-revision`, the revision came from the command's own read, so
        // running it again reads the rules again (and, without `--remove-others`, checks the
        // file against them). A `--base-revision` given would be sent again and refused
        // again, so it gets the get-and-edit hint.
        let read_revision = args.base_revision.is_none();
        match (error.http_status(), read_revision) {
            (Some(409), false) => error.with_hint(format!(
                "run `{}`, edit the rules it prints, and pass its `revision` as \
                 `--base-revision`",
                crate::cli::with_id("compute firewall get", vm_id, "")
            )),
            (Some(409), true) => {
                let again = phrase::command(ctx.args());
                let checks = if args.remove_others {
                    "reads the revision of the rules again"
                } else {
                    "reads the rules again and checks the file against them"
                };
                error.with_hint(format!("run `{again}` again, which {checks}"))
            }
            _ => error,
        }
    })
}

/// A firewall rule the server answered as one entry of the list ([`rule_entry`]).
///
/// `None` for `ports` or `source_cidrs` that are not arrays of strings, or an `enabled` that
/// is not a boolean: swagger reads an empty list as every port or any source, so reading a
/// malformed one as empty could match a rule that opens everything.
fn firewall_entry(rule: &Value) -> Option<Entry> {
    let rule = rule.as_object()?;
    let enabled = match rule.get("enabled") {
        None => true,
        Some(enabled) => enabled.as_bool()?,
    };
    let ports = strings(rule, "ports").ok()?;
    let sources = strings(rule, "source_cidrs").ok()?;
    Some(rule_entry(rule.get("protocol"), &ports, &sources, enabled))
}

/// A firewall rule as one entry of the list: what it accepts, which is its protocol, its
/// ports and its sources, each as a set ([`accepted_ports`], [`accepted_sources`]), and
/// whether it is enabled. A rule without `enabled` counts as enabled, so disabling one
/// closes what it accepted and is a removal. Its `id` (ignored on a PUT) and `description`
/// are left out, so changing them edits the rule. A rule whose ports or sources change
/// accepts something else, so it counts as removed.
///
/// Two rules have one entry only when they accept the same traffic, so the guard fails safe:
/// a spelling this does not recognize as equal (overlapping port ranges, a source written
/// with host bits) counts as a removal and is refused, never let through.
fn rule_entry(protocol: Option<&Value>, ports: &[&str], sources: &[&str], enabled: bool) -> Entry {
    Entry::new(json!({
        "protocol": protocol,
        "ports": accepted_ports(ports),
        "source_cidrs": accepted_sources(sources),
        "enabled": enabled,
    }))
}

/// `ports` as a set, each without the leading zeros the server's list may hold: `022` is
/// `22`, and `08000-08080` is `8000-8080`. A file cannot hold those, so they are the server
/// spelling the file's port. Nothing else is merged: the file is sent unchanged, and swagger
/// defines only `[]` as every port, so `1-65535` against `[]` or `22-22` against `22` is a
/// change the server may enforce differently, and counts as a removal.
/// A port that does not read as one (only the server's list can hold one) stays its text,
/// so it matches no port of a file.
fn accepted_ports(ports: &[&str]) -> Value {
    let unpadded = |text: &str| match text.split_once('-') {
        Some((low, high)) => port(low)
            .zip(port(high))
            .map_or_else(|| text.to_owned(), |(low, high)| format!("{low}-{high}")),
        None => port(text).map_or_else(|| text.to_owned(), |port| port.to_string()),
    };
    replace::set(
        ports
            .iter()
            .map(|&text| Value::from(unpadded(text)))
            .collect(),
    )
}

/// `sources` as the set of what they accept. Any source is `[]`: swagger reads an empty
/// `source_cidrs` as any source, and a list that holds a `/0` accepts every address too. The
/// two are one set because the firewall guards the VM's public IPv4 only, and its sources are
/// IPv4 CIDRs (swagger `compute_vms_firewall_update`: "Replace a VM's public-IP firewall
/// rules", "`source_cidrs` as IPv4 CIDRs (empty = any source)"), so no IPv6 source exists
/// for `[]` to admit and a `/0` to leave out.
fn accepted_sources(sources: &[&str]) -> Value {
    if any_source(sources) {
        Value::Array(Vec::new())
    } else {
        replace::set(sources.iter().copied().map(Value::from).collect())
    }
}

/// Whether `sources` accept every IPv4 address: none are listed, or one has a prefix length
/// of 0, which covers every address.
fn any_source(sources: &[&str]) -> bool {
    sources.is_empty()
        || sources.iter().any(|cidr| {
            cidr.rsplit_once('/').is_some_and(|(address, prefix)| {
                address.parse::<Ipv4Addr>().is_ok() && prefix.parse::<u8>() == Ok(0)
            })
        })
}

/// The action and the consequence a firewall update asks about. A rule that accepts every
/// IPv4 address is named in the action, so the refusal an agent reads says so too.
fn firewall_question(vm_id: &str, file: &Path, rules: &[Value]) -> (String, &'static str) {
    let source = Input {
        path: file,
        flag: "--rules-file",
        what: "firewall rule list",
    }
    .name();
    // Swagger reads an empty `source_cidrs` as any source and an empty `ports` as every
    // port, so a rule that leaves either out is named, beside how many rules do.
    let any_source = rules.iter().filter(|rule| from_any_source(rule)).count();
    let every_port = rules.iter().filter(|rule| on_every_port(rule)).count();
    let clauses: Vec<String> = [
        (
            any_source,
            "every IPv4 address (no `source_cidrs`, or a `/0`)",
        ),
        (every_port, "every port (no `ports`, or `[]`)"),
    ]
    .into_iter()
    .filter_map(|(count, what)| match (rules.len(), count) {
        (_, 0) => None,
        (1, _) => Some(format!("which accepts {what}")),
        (_, 1) => Some(format!("of which 1 accepts {what}")),
        (_, n) => Some(format!("of which {n} accept {what}")),
    })
    .collect();
    let exposure = match clauses.as_slice() {
        [] => String::new(),
        clauses => format!(", {}", clauses.join(", and ")),
    };
    match rules.len() {
        0 => (
            format!("remove every inbound rule of the VM `{vm_id}`"),
            "every port on its public IPv4 closes",
        ),
        1 => (
            format!(
                "replace every inbound rule of the VM `{vm_id}` with 1 rule from \
                 {source}{exposure}"
            ),
            "the rule opens what it accepts on its public IPv4, and every other port closes",
        ),
        n => (
            format!(
                "replace every inbound rule of the VM `{vm_id}` with {n} rules from \
                 {source}{exposure}"
            ),
            "the rules open what they accept on its public IPv4, and every other port closes",
        ),
    }
}

/// The most rules a VM's firewall holds (swagger, `PUT .../firewall/`).
const MAX_FIREWALL_RULES: usize = 50;

/// The largest rules file read, in bytes. Arbitrary: [`MAX_FIREWALL_RULES`] short rules are
/// far under this, and the cap stops a wrong path from being read whole.
const MAX_RULES_FILE_BYTES: u64 = 1024 * 1024;

/// The rule list in `path` (or stdin for `-`), each rule checked, and the entry of each
/// rule. The list is the `rules` array of a JSON object, the shape `firewall get` prints,
/// whose other keys are not sent, or a bare array, which `cdn rules update` takes too.
fn firewall_rules(path: &Path) -> Result<(Vec<Value>, Vec<Entry>)> {
    let input = Input {
        path,
        flag: "--rules-file",
        what: "firewall rule list",
    };
    let bytes = input.read(MAX_RULES_FILE_BYTES)?;
    let file = input.name();
    let not_rules = || {
        Error::usage(
            format!(
                "the firewall rule list from {file} is not a JSON object with a `rules` array, \
                 or an array of rules"
            ),
            "write the rules as `{\"rules\": [...]}`, the shape `hipc compute firewall get` \
             prints, or as a bare array",
        )
    };
    let rules = match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Array(rules)) => rules,
        Ok(Value::Object(mut document)) => match document.shift_remove("rules") {
            Some(Value::Array(rules)) => rules,
            _ => return Err(not_rules()),
        },
        _ => return Err(not_rules()),
    };
    // Swagger: "At most 50 rules". Refused here, so a list the server would reject is not
    // sent after the confirmation.
    if rules.len() > MAX_FIREWALL_RULES {
        return Err(Error::usage(
            format!(
                "the firewall rule list from {file} has {} rules, and a VM's firewall holds at \
             most {MAX_FIREWALL_RULES}",
                rules.len()
            ),
            "merge rules that share a protocol and ports",
        ));
    }
    let entries = rules
        .iter()
        .enumerate()
        .map(|(index, rule)| {
            check_rule(rule).map_err(|problem| {
                Error::usage(
                    format!("`rules[{index}]` from {file} {problem}"),
                    format!("fix `rules[{index}]`; the VM keeps its rules until then"),
                )
            })
        })
        .collect::<Result<Vec<Entry>>>()?;
    Ok((rules, entries))
}

/// The keys swagger lists for a firewall rule (`PUT .../firewall/`).
const RULE_KEYS: [&str; 6] = [
    "id",
    "protocol",
    "ports",
    "source_cidrs",
    "description",
    "enabled",
];

/// What is wrong with one rule, as the end of a sentence that starts with its place in the
/// file. A key swagger does not list is `usage`, so a misspelled key cannot quietly change
/// which traffic the rule lets in. `id` and `description` are the server's to judge. The rule
/// count is checked by [`firewall_rules`], against swagger's cap of [`MAX_FIREWALL_RULES`]. A rule
/// that passes is returned as its entry, so the file is read once.
fn check_rule(rule: &Value) -> std::result::Result<Entry, String> {
    let Value::Object(rule) = rule else {
        return Err("is not a JSON object".to_owned());
    };
    if let Some(key) = rule.keys().find(|key| !RULE_KEYS.contains(&key.as_str())) {
        return Err(format!(
            "has the key `{key}`, which a firewall rule does not have: a rule has only {}",
            phrase::ticked(RULE_KEYS, "and")
        ));
    }
    let Some(protocol @ ("tcp" | "udp" | "icmp")) = rule.get("protocol").and_then(Value::as_str)
    else {
        return Err("has no `protocol` of `tcp`, `udp`, or `icmp`".to_owned());
    };
    let ports = strings(rule, "ports")?;
    if protocol == "icmp" && !ports.is_empty() {
        return Err("is `icmp`, which has no ports, so its `ports` must be `[]`".to_owned());
    }
    if let Some(bad) = ports.iter().find(|p| !is_port_range(p)) {
        return Err(format!(
            "has the port `{bad}`: a port is `1` to `65535`, or a range such as `8000-8080`"
        ));
    }
    let sources = strings(rule, "source_cidrs")?;
    if let Some(bad) = sources.iter().find(|c| !is_ipv4_cidr(c)) {
        return Err(format!(
            "has the source `{bad}`: a source is an IPv4 CIDR, such as `203.0.113.0/24`"
        ));
    }
    let enabled = match rule.get("enabled") {
        None => true,
        Some(Value::Bool(enabled)) => *enabled,
        Some(_) => return Err("has an `enabled` that is not `true` or `false`".to_owned()),
    };
    Ok(rule_entry(rule.get("protocol"), &ports, &sources, enabled))
}

/// Whether `rule` accepts every IPv4 address: swagger says an empty `source_cidrs` is any
/// source, and a prefix length of 0 covers every address. Called on rules `check_rule`
/// passed, so `source_cidrs` is absent or an array of CIDRs `is_ipv4_cidr` accepted.
fn from_any_source(rule: &Value) -> bool {
    let sources = rule
        .as_object()
        .and_then(|rule| strings(rule, "source_cidrs").ok())
        .unwrap_or_default();
    any_source(&sources)
}

/// Whether `rule` is a TCP or UDP rule that accepts every port: swagger says an empty
/// `ports` is every port. An ICMP rule has no ports. Called on rules `check_rule` passed.
fn on_every_port(rule: &Value) -> bool {
    let tcp_or_udp = matches!(
        rule.get("protocol").and_then(Value::as_str),
        Some("tcp" | "udp")
    );
    let no_ports = rule
        .as_object()
        .and_then(|rule| strings(rule, "ports").ok())
        .unwrap_or_default()
        .is_empty();
    tcp_or_udp && no_ports
}

/// The strings of the array `rule[key]`, or an empty list when the key is absent.
fn strings<'a>(
    rule: &'a Map<String, Value>,
    key: &str,
) -> std::result::Result<Vec<&'a str>, String> {
    let Some(value) = rule.get(key) else {
        return Ok(Vec::new());
    };
    value
        .as_array()
        .and_then(|items| items.iter().map(Value::as_str).collect())
        .ok_or_else(|| format!("has a `{key}` that is not an array of strings"))
}

/// `22` or `8000-8080`: ports from 1 to 65535, the low end first, with no leading zero, so
/// each port has one spelling in a file (`022` is refused, as `/08` is in a source).
fn is_port_range(text: &str) -> bool {
    let unpadded = |port: &str| !port.starts_with('0');
    let spelled_once = match text.split_once('-') {
        Some((low, high)) => unpadded(low) && unpadded(high),
        None => unpadded(text),
    };
    spelled_once && port_bounds(text).is_some()
}

/// The low and high port of `22` (both 22) or `8000-8080`, the low end first; `None` for
/// anything else. Leading zeros are read; [`is_port_range`] refuses them in a file.
fn port_bounds(text: &str) -> Option<(u16, u16)> {
    match text.split_once('-') {
        Some((low, high)) => port(low).zip(port(high)).filter(|(low, high)| low <= high),
        None => port(text).map(|port| (port, port)),
    }
}

/// A port from 1 to 65535, in plain digits (no sign, no space).
fn port(text: &str) -> Option<u16> {
    if text.trim_start_matches('0').len() > 5 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok().filter(|&port| port != 0)
}

/// `203.0.113.0/24`: a dotted IPv4 address, then a prefix length from 0 to 32 with no
/// leading zero, so each prefix has one spelling (`/00` and `/08` are refused).
fn is_ipv4_cidr(text: &str) -> bool {
    let Some((address, prefix)) = text.split_once('/') else {
        return false;
    };
    let prefix_ok = (1..=2).contains(&prefix.len())
        && prefix.bytes().all(|b| b.is_ascii_digit())
        && (prefix == "0" || !prefix.starts_with('0'))
        && prefix.parse::<u8>().is_ok_and(|p| p <= 32);
    prefix_ok && address.parse::<Ipv4Addr>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_port_is_one_to_65535_in_plain_digits() {
        for (text, port_read) in [
            ("1", Some(1)),
            ("65535", Some(65_535)),
            ("022", Some(22)),
            ("0", None),
            ("65536", None),
            ("", None),
            ("+22", None),
            (" 22", None),
            ("000000022", Some(22)),
            ("1000000", None),
        ] {
            assert_eq!(port(text), port_read, "{text:?}");
        }
    }

    #[test]
    fn a_port_range_is_low_first_and_spelled_once() {
        for good in ["22", "1", "65535", "8000-8080", "22-22"] {
            assert!(is_port_range(good), "{good}");
        }
        for bad in [
            "022",
            "8080-8000",
            "08000-8080",
            "8000-08080",
            "0",
            "1-",
            "-1",
            "a",
            "1-2-3",
        ] {
            assert!(!is_port_range(bad), "{bad}");
        }
        assert_eq!(port_bounds("08000-08080"), Some((8000, 8080)));
        assert_eq!(port_bounds("9-8"), None);
    }

    #[test]
    fn a_source_is_an_ipv4_cidr_with_one_spelling() {
        for good in ["203.0.113.0/24", "0.0.0.0/0", "10.0.0.1/32", "10.0.0.0/8"] {
            assert!(is_ipv4_cidr(good), "{good}");
        }
        for bad in [
            "203.0.113.0",
            "10.0.0.0/08",
            "10.0.0.0/00",
            "10.0.0.0/33",
            "10.0.0.0/",
            "10.0.0.0/123",
            "::/0",
            "10.0.0/8",
            "10.0.0.0/+8",
        ] {
            assert!(!is_ipv4_cidr(bad), "{bad}");
        }
    }

    #[test]
    fn any_source_is_none_listed_or_a_slash_zero() {
        assert!(any_source(&[]));
        assert!(any_source(&["10.0.0.0/8", "0.0.0.0/0"]));
        assert!(any_source(&["192.0.2.1/0"]));
        assert!(!any_source(&["10.0.0.0/8"]));
        // Not a CIDR this module reads, so not any source.
        assert!(!any_source(&["x/0"]));
    }

    /// Two rules have one entry when they accept the same traffic: the server's padded
    /// ports and a `/0` beside other sources read as the file's spelling.
    #[test]
    fn a_rule_entry_is_what_it_accepts() {
        let entry = |rule: Value| format!("{:?}", firewall_entry(&rule));
        assert_eq!(
            entry(json!({"protocol": "tcp", "ports": ["022", "08000-08080"]})),
            entry(json!({"protocol": "tcp", "ports": ["8000-8080", "22"], "enabled": true})),
        );
        assert_eq!(
            entry(json!({"protocol": "tcp", "source_cidrs": ["10.0.0.0/8", "0.0.0.0/0"]})),
            entry(json!({"protocol": "tcp", "id": 7, "description": "x"})),
        );
        assert_ne!(
            entry(json!({"protocol": "tcp", "enabled": false})),
            entry(json!({"protocol": "tcp"})),
        );
        // A port the server spells that reads as no port stays its text.
        assert_ne!(
            entry(json!({"protocol": "tcp", "ports": ["x"]})),
            entry(json!({"protocol": "tcp", "ports": []})),
        );
        for malformed in [
            json!("tcp"),
            json!({"protocol": "tcp", "enabled": "yes"}),
            json!({"protocol": "tcp", "ports": "22"}),
            json!({"protocol": "tcp", "source_cidrs": [1]}),
        ] {
            assert!(firewall_entry(&malformed).is_none(), "{malformed}");
        }
    }

    #[test]
    fn check_rule_names_what_is_wrong() {
        let problem = |rule: Value| check_rule(&rule).err();
        assert_eq!(problem(json!({"protocol": "tcp", "ports": ["22"]})), None);
        assert_eq!(problem(json!({"protocol": "icmp"})), None);
        for (rule, says) in [
            (json!([]), "is not a JSON object"),
            (
                json!({"protocol": "tcp", "port": ["22"]}),
                "has the key `port`, which a firewall rule does not have: a rule has only `id`, \
                 `protocol`, `ports`, `source_cidrs`, `description`, and `enabled`",
            ),
            (
                json!({"protocol": "sctp"}),
                "has no `protocol` of `tcp`, `udp`, or `icmp`",
            ),
            (json!({}), "has no `protocol` of `tcp`, `udp`, or `icmp`"),
            (
                json!({"protocol": "icmp", "ports": ["22"]}),
                "is `icmp`, which has no ports, so its `ports` must be `[]`",
            ),
            (
                json!({"protocol": "tcp", "ports": ["022"]}),
                "has the port `022`: a port is `1` to `65535`, or a range such as `8000-8080`",
            ),
            (
                json!({"protocol": "tcp", "ports": 22}),
                "has a `ports` that is not an array of strings",
            ),
            (
                json!({"protocol": "udp", "source_cidrs": ["10.0.0.0"]}),
                "has the source `10.0.0.0`: a source is an IPv4 CIDR, such as `203.0.113.0/24`",
            ),
            (
                json!({"protocol": "udp", "enabled": 1}),
                "has an `enabled` that is not `true` or `false`",
            ),
        ] {
            assert_eq!(problem(rule.clone()).as_deref(), Some(says), "{rule}");
        }
    }

    #[test]
    fn every_port_and_any_source_are_told_apart() {
        assert!(on_every_port(&json!({"protocol": "tcp"})));
        assert!(on_every_port(&json!({"protocol": "udp", "ports": []})));
        assert!(!on_every_port(&json!({"protocol": "icmp"})));
        assert!(!on_every_port(&json!({"protocol": "tcp", "ports": ["22"]})));
        assert!(from_any_source(&json!({"protocol": "tcp"})));
        assert!(from_any_source(&json!({"source_cidrs": ["0.0.0.0/0"]})));
        assert!(!from_any_source(&json!({"source_cidrs": ["10.0.0.0/8"]})));
    }

    /// The question counts the rules, and names those that accept every address or port.
    #[test]
    fn the_question_names_what_the_rules_open() {
        let file = Path::new("rules.json");
        let tcp = json!({"protocol": "tcp", "ports": ["22"], "source_cidrs": ["10.0.0.0/8"]});
        let open = json!({"protocol": "tcp"});
        let (action, consequence) = firewall_question("vm-1", file, &[]);
        assert_eq!(action, "remove every inbound rule of the VM `vm-1`");
        assert_eq!(consequence, "every port on its public IPv4 closes");
        let (action, _) = firewall_question("vm-1", file, std::slice::from_ref(&tcp));
        assert_eq!(
            action,
            "replace every inbound rule of the VM `vm-1` with 1 rule from `rules.json`"
        );
        let (action, _) = firewall_question("vm-1", file, std::slice::from_ref(&open));
        assert!(
            action.ends_with(
                "with 1 rule from `rules.json`, which accepts every IPv4 \
                 address (no `source_cidrs`, or a `/0`), and which accepts every port (no \
                 `ports`, or `[]`)"
            ),
            "{action}"
        );
        let (action, consequence) = firewall_question("vm-1", file, &[open.clone(), open, tcp]);
        assert!(
            action.ends_with(
                "with 3 rules from `rules.json`, of which 2 accept every \
                 IPv4 address (no `source_cidrs`, or a `/0`), and of which 2 accept every port \
                 (no `ports`, or `[]`)"
            ),
            "{action}"
        );
        assert_eq!(
            consequence,
            "the rules open what they accept on its public IPv4, and every other port closes"
        );
    }
}
