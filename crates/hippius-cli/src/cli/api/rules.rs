//! What `hipc api` holds back, said from the library's route rows and guard table
//! (`hippius::api_routes`), so its help, its flags' help, and its row in `hipc schema` name
//! every guarded route and cannot drift from the rule that applies.
//!
//! The same rule is written in two voices ([`Voice`]): `hipc schema` quotes each literal in
//! backticks and names the whole path, and the help is plain text that names the path as
//! `hipc api` takes it, without `/api`.

use hippius::routes::Guard;
use hippius::{Method, RouteFlag};

/// The six methods `hipc api` sends, reads first, in the order a rule lists them.
const METHODS: [Method; 6] = [
    Method::Get,
    Method::Head,
    Method::Post,
    Method::Put,
    Method::Patch,
    Method::Delete,
];

/// The write methods: a rule of one lends to the others that have none (`hippius::api_guard`).
const WRITES: [Method; 4] = [Method::Post, Method::Put, Method::Patch, Method::Delete];

/// Routes the help leaves out: staff and operator routes no customer command sends. Their
/// guards still apply, and `hipc schema` names them.
const UNLISTED: [&str; 1] = ["/api/v1/operator/nodes/{node_id}/storage/"];

/// How a rule is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Voice {
    /// For `hipc schema`: literals in backticks, the whole path.
    Schema,
    /// For the help: plain text, the path as `hipc api` takes it, staff routes left out.
    Help,
}

impl Voice {
    /// `text` as a literal.
    fn literal(self, text: &str) -> String {
        match self {
            Self::Schema => format!("`{text}`"),
            Self::Help => text.to_owned(),
        }
    }

    /// `template` as a reader writes it: `<zone_id>` for `{zone_id}`, since clap's help
    /// renderer reads `{n}` as a line break, and without `/api` in the help.
    fn path(self, template: &str) -> String {
        let shown = template.replace('{', "<").replace('}', ">");
        match self {
            Self::Schema => self.literal(&shown),
            Self::Help => shown
                .strip_prefix(hippius::routes::API_BASE)
                .unwrap_or(&shown)
                .to_owned(),
        }
    }

    /// Whether the rule of `template` is said in this voice.
    fn lists(self, template: &str) -> bool {
        self == Self::Schema || !UNLISTED.contains(&template)
    }
}

/// Every route `hipc api` holds a rule for, each once, in the order the library lists them.
fn templates() -> Vec<&'static str> {
    let mut templates: Vec<&'static str> = Vec::new();
    for (_, template, _) in hippius::api_routes() {
        if !templates.contains(&template) {
            templates.push(template);
        }
    }
    templates
}

/// The methods of `template` whose guard `holds`.
fn methods_where(template: &str, holds: impl Fn(Guard) -> bool) -> Vec<Method> {
    METHODS
        .into_iter()
        .filter(|&method| holds(hippius::api_guard(method, template)))
        .collect()
}

/// `methods` as a rule names them: "any method", "a write", "a write other than `DELETE`",
/// "`GET` or `HEAD`", or the methods joined.
fn methods_phrase(voice: Voice, methods: &[Method]) -> String {
    let named = |methods: &[Method]| {
        let names: Vec<String> = methods.iter().map(|m| voice.literal(m.as_str())).collect();
        hippius::phrase::join(&names, "or")
    };
    let writes: Vec<Method> = WRITES
        .into_iter()
        .filter(|method| methods.contains(method))
        .collect();
    let reads: Vec<Method> = methods
        .iter()
        .copied()
        .filter(|method| method.is_read())
        .collect();
    if methods.len() == METHODS.len() {
        return "any method".to_owned();
    }
    let others: Vec<Method> = WRITES
        .into_iter()
        .filter(|method| !writes.contains(method))
        .collect();
    let writes = match (writes.len(), others.len()) {
        (0, _) => None,
        (_, 0) => Some("a write".to_owned()),
        (taken, left) if left < taken => Some(format!("a write other than {}", named(&others))),
        _ => Some(format!("a {}", named(&writes))),
    };
    let reads = (!reads.is_empty()).then(|| named(&reads));
    match (reads, writes) {
        (Some(reads), Some(writes)) => format!("{reads} or {writes}"),
        (Some(one), None) | (None, Some(one)) => one,
        (None, None) => String::new(),
    }
}

/// The routes `hipc api` never sends whatever the flags, each as "<methods> of <path>", and
/// those it refuses for a key of the body, as "<methods> of <path> ... whose body sets
/// `<key>`" with what that counts.
pub(crate) fn refused(voice: Voice) -> Vec<String> {
    let mut said = Vec::new();
    for template in templates().into_iter().filter(|t| voice.lists(t)) {
        let methods = methods_where(template, |guard| matches!(guard, Guard::Refused { .. }));
        if !methods.is_empty() {
            said.push(format!(
                "{} of {}",
                methods_phrase(voice, &methods),
                voice.path(template)
            ));
        }
    }
    let mut keys: Vec<&str> = Vec::new();
    for template in templates() {
        for method in METHODS {
            if let Guard::RefusedWhenSet { key, .. } = hippius::api_guard(method, template)
                && !keys.contains(&key)
            {
                keys.push(key);
            }
        }
    }
    for key in keys {
        let routes: Vec<String> = templates()
            .into_iter()
            .filter(|t| voice.lists(t))
            .filter_map(|template| {
                let methods = methods_where(
                    template,
                    |guard| matches!(guard, Guard::RefusedWhenSet { key: set, .. } if set == key),
                );
                (!methods.is_empty())
                    .then(|| (methods_phrase(voice, &methods), voice.path(template)))
            })
            .enumerate()
            .map(|(at, (methods, path))| {
                if at == 0 {
                    format!("{methods} of {path}")
                } else {
                    format!("of {path}")
                }
            })
            .collect();
        said.push(format!(
            "{} whose body sets {} (a body that is not a JSON object counts as setting it)",
            hippius::phrase::join(&routes, "or"),
            voice.literal(key)
        ));
    }
    said
}

/// The sentences of the help that say why each refused route is never sent and what to run
/// instead, from the reason and the hint its row carries.
pub(crate) fn refusals_explained() -> Vec<String> {
    let plain = |text: &str| text.replace('`', "");
    let mut explained = Vec::new();
    for template in templates().into_iter().filter(|t| Voice::Help.lists(t)) {
        let mut seen: Vec<Guard> = Vec::new();
        for method in METHODS {
            let guard = hippius::api_guard(method, template);
            let (why, hint, key) = match guard {
                Guard::Refused { why, hint } => (why, hint, None),
                Guard::RefusedWhenSet { key, why, hint } => (why, hint, Some(key)),
                Guard::None
                | Guard::Needs(_)
                | Guard::NeedsWhenSet(..)
                | Guard::NeedsUnlessFalse(..) => continue,
            };
            if seen.contains(&guard) {
                continue;
            }
            seen.push(guard);
            let methods = methods_where(template, |other| other == guard);
            let mut sentence = format!(
                "{} of {}",
                methods_phrase(Voice::Help, &methods),
                Voice::Help.path(template)
            );
            sentence[..1].make_ascii_uppercase();
            if let Some(key) = key {
                sentence.push_str(&format!(" whose body sets {key}"));
            }
            sentence.push_str(&format!(" is never sent: {why}; {}", plain(hint)));
            if let Some(key) = key {
                sentence.push_str(&format!("; without {key} it is sent as usual"));
            }
            sentence.push('.');
            explained.push(sentence);
        }
    }
    explained
}

/// When a guard that needs `flag` applies, as the clause that ends a route's phrase: empty for
/// a guard that always does, and `None` for one that does not need `flag`.
fn condition(voice: Voice, guard: Guard, flag: RouteFlag) -> Option<String> {
    match guard {
        Guard::Needs(flags) if flags.contains(&flag) => Some(String::new()),
        Guard::NeedsWhenSet(flags, key) if flags.contains(&flag) => {
            Some(format!(" whose body sets {}", voice.literal(key)))
        }
        Guard::NeedsUnlessFalse(flags, key) if flags.contains(&flag) => Some(format!(
            " unless its body sets {} to {}",
            voice.literal(key),
            voice.literal("false")
        )),
        Guard::None
        | Guard::Refused { .. }
        | Guard::RefusedWhenSet { .. }
        | Guard::Needs(_)
        | Guard::NeedsWhenSet(..)
        | Guard::NeedsUnlessFalse(..) => None,
    }
}

/// The routes `flag` gets past, each as "a <methods> of <path>" with the condition on its
/// body, from the entries of each route's own methods: a write method with no entry borrows
/// one, which [`flag_rules`] says once for all.
pub(crate) fn needs(voice: Voice, flag: RouteFlag) -> Vec<String> {
    let mut said = Vec::new();
    for template in templates().into_iter().filter(|t| voice.lists(t)) {
        let own: Vec<(Method, String)> = hippius::api_routes()
            .filter(|&(_, route, _)| route == template)
            .filter_map(|(method, _, guard)| {
                condition(voice, guard, flag).map(|when| (method, when))
            })
            .collect();
        let mut conditions: Vec<&String> = Vec::new();
        for (_, when) in &own {
            if !conditions.contains(&when) {
                conditions.push(when);
            }
        }
        for when in conditions {
            let methods: Vec<Method> = own
                .iter()
                .filter(|(_, other)| other == when)
                .map(|&(method, _)| method)
                .collect();
            let names: Vec<String> = methods.iter().map(|m| voice.literal(m.as_str())).collect();
            said.push(format!(
                "a {} of {}{when}",
                hippius::phrase::join(&names, "or"),
                voice.path(template)
            ));
        }
    }
    said
}

/// Whether the help leaves out a route `flag` gets past ([`UNLISTED`]), so it says that more
/// routes take the flag than it names.
pub(crate) fn leaves_out(flag: RouteFlag) -> bool {
    needs(Voice::Schema, flag).len() > needs(Voice::Help, flag).len()
}

/// What holds for every route of [`needs`]: the rule of a write method holds for the other
/// write methods of its route that have none of their own, a `PUT` that borrows a rule on a
/// key needs the flag whatever its body sets, and the write methods with a rule of their own
/// that needs nothing are named.
pub(crate) fn flag_rules(voice: Voice, flag: RouteFlag) -> String {
    let mut rules = Vec::new();
    let keyed = hippius::api_routes()
        .any(|(_, _, guard)| condition(voice, guard, flag).is_some_and(|when| !when.is_empty()));
    if keyed {
        rules.push("a body that is not a JSON object counts as setting the key".to_owned());
    }
    rules.push(
        "each rule holds for every write method of its route that has no rule of its own"
            .to_owned(),
    );
    let key_rule_lent_to_put = hippius::api_routes().any(|(method, template, guard)| {
        method != Method::Put
            && matches!(guard, Guard::NeedsWhenSet(flags, _) if flags.contains(&flag))
            && !hippius::api_routes()
                .any(|(other, route, _)| route == template && other == Method::Put)
    });
    if key_rule_lent_to_put {
        rules.push(format!(
            "a {} that borrows a rule on a key of the body needs the flag whatever its body sets",
            voice.literal("PUT")
        ));
    }
    for template in templates().into_iter().filter(|t| voice.lists(t)) {
        let guarded = hippius::api_routes()
            .any(|(_, route, guard)| route == template && condition(voice, guard, flag).is_some());
        if !guarded {
            continue;
        }
        let exempt: Vec<String> = hippius::api_routes()
            .filter(|&(method, route, guard)| {
                route == template && !method.is_read() && !guard.lends()
            })
            .map(|(method, ..)| voice.literal(method.as_str()))
            .collect();
        if !exempt.is_empty() {
            rules.push(format!(
                "a {} of {}, which the API documents as doing something else, needs none",
                hippius::phrase::join(&exempt, "or"),
                voice.path(template)
            ));
        }
    }
    rules.join("; ")
}

/// The routes whose read may create something, as "a `GET` or `HEAD` of <path>": a `HEAD`
/// reaches the `GET` row.
pub(crate) fn provisioning_reads(voice: Voice) -> Vec<String> {
    hippius::routes::all()
        .filter(|row| row.provisions())
        .map(|row| {
            format!(
                "a {} or {} of {}",
                voice.literal(row.method().as_str()),
                voice.literal(Method::Head.as_str()),
                voice.path(row.template())
            )
        })
        .collect()
}
