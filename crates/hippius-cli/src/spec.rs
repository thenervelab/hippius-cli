//! What each command does to the server, for `hipc schema` (`TENETS.md` tenet 7).
//!
//! The parser says which commands exist. This table says what they do. A test walks the
//! parser and fails when a command has no row here, so a new command cannot ship without
//! an effect.

use clap::CommandFactory as _;
use hippius::Code;
use serde_json::{Value, json};

use crate::cli::Cli;

/// The verb shelf of tenet 2: every word a leaf may end in as a verb, in the order tenet 2
/// lists them. A test holds every leaf to it.
#[cfg(test)]
const VERBS: [&str; 40] = [
    "list",
    "get",
    "create",
    "update",
    "delete",
    "subscribe",
    "change",
    "cancel",
    "start",
    "stop",
    "reboot",
    "resize",
    "restart",
    "restore",
    "resume",
    "revoke",
    "rotate",
    "scale",
    "replace",
    "verify",
    "connect",
    "test",
    "rollback",
    "presign",
    "dns-setup",
    "domain-connect",
    "use",
    "login",
    "logout",
    "accept",
    "reply",
    "configure",
    "card-setup",
    "top-up",
    "estimate",
    "preview",
    "transfer",
    "decline",
    "validate",
    "fix",
];

/// `schema_version` in the `hipc schema` document. Bump it when a field of a document `hipc`
/// prints is renamed, removed, or changes meaning (tenet 15). 2: `hipc api`'s route flags
/// moved into `guards`, `--change-isolation` got its own code, and the dry runs and `auth
/// status` lost `target` and `ok`. 3: a dry run holds its requests only in `requests`, the
/// VM and Kubernetes backups lists and the database events and refusals print the list
/// envelope, more commands are confirmed, and a database create or restore with
/// `--credentials-dir` prints the paths of its credential files in place of every secret.
const SCHEMA_VERSION: u32 = 3;

/// What a repeat of the command does to the server (tenet 7).
#[derive(Debug, Clone, Copy)]
enum Effect {
    /// Nothing changes.
    ReadOnly,
    /// A repeat leaves the server, or the config file, as one call did. The repeat may
    /// answer an error because the work is already done.
    Idempotent,
    /// A repeat does more work, or spends again.
    NonIdempotent,
}

impl Effect {
    fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Idempotent => "idempotent",
            Self::NonIdempotent => "non_idempotent",
        }
    }
}

/// When a command refuses with `confirmation_required` unless `--yes` is passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Confirmation {
    Never,
    Always,
    /// Only when this holds, as the end of a sentence that starts "it is confirmed when".
    When(&'static str),
}

/// When a command refuses with `reveal_required` unless `--reveal` is passed (tenet 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reveal {
    Never,
    Always,
    /// Only when this holds, as the end of a sentence that starts "it needs `--reveal`
    /// when".
    When(&'static str),
}

/// An exit-2 refusal of a write, beside the confirmation and `--reveal`, that the flag of
/// `code` gets past, except where `when` says it does not. For `hipc api` these are the
/// flags its routes need beyond `--yes`.
#[derive(Debug)]
struct Guard {
    code: Code,
    /// When the command refuses, as the end of a sentence that starts "it refuses when";
    /// `None` when it always does without the flag.
    when: Option<&'static str>,
}

/// One row of the schema.
#[derive(Debug)]
struct Spec {
    effect: Effect,
    confirmation: Confirmation,
    reveal: Reveal,
    /// The command returns one page and accepts `--page-token` and `--all`, or, for
    /// `hipc api`, follows pages with `--all`.
    pagination: bool,
    /// When the command refuses with `usage`, as the end of a sentence that starts "it
    /// refuses when": whatever the flags, or, for `auth login`, unless the flag it names is
    /// passed.
    refused_when: Option<&'static str>,
    /// The other exit-2 refusals the command makes unless a flag of its own is passed.
    guards: &'static [Guard],
}

const READ_ONLY: Spec = Spec {
    effect: Effect::ReadOnly,
    confirmation: Confirmation::Never,
    reveal: Reveal::Never,
    pagination: false,
    refused_when: None,
    guards: &[],
};

/// A write whose repeat is safe, and which the contract does not gate: a local config
/// change, or a VM power change.
const UNGATED_WRITE: Spec = Spec {
    effect: Effect::Idempotent,
    ..READ_ONLY
};

/// A write that spends or creates, and so runs only when confirmed.
const SPEND: Spec = Spec {
    effect: Effect::NonIdempotent,
    confirmation: Confirmation::Always,
    ..READ_ONLY
};

/// When a WAF rule or rate limit update is confirmed: `--check` publishes nothing.
const WAF_CHECK: &str = "`--check` is not passed: a check publishes nothing";

/// A write that undoes, replaces, or attaches what a repeat returns unchanged, and so runs
/// only when confirmed. A repeat is safe.
const UNDO: Spec = Spec {
    effect: Effect::Idempotent,
    confirmation: Confirmation::Always,
    ..READ_ONLY
};

/// The flags `hipc api` needs beyond `--yes`, on the routes `Client::route_flags` names,
/// each refused with the code `RouteFlag::code` gives it.
const API_GUARDS: &[Guard] = &[
    Guard {
        code: Code::ChangeIsolationRequired,
        when: Some(
            "the request writes to `/api/network/isolation/` (any method but `GET` and `HEAD`)",
        ),
    },
    Guard {
        code: Code::PublicRequired,
        when: Some(
            "the request is `POST /api/cdn/zones/`, \
             `PUT /api/objectstore/buckets/<name>/acl/set/`, a `PATCH` of \
             `/api/cdn/zones/<zone_id>/` whose body sets `origin`, or a \
             `PATCH /api/registry/me/publicity/` unless the body sets `public` to `false`; a \
             body that is not a JSON object counts as setting the key, and each rule holds for \
             every write method of its route, not only the one named",
        ),
    },
    Guard {
        code: Code::RemoveOthersRequired,
        when: Some(
            "the request is a `PUT` of `/api/compute/vms/<vm_id>/firewall/`, \
             `/api/cdn/zones/<zone_id>/rules/`, `/api/cdn/zones/<zone_id>/waf/rules/`, \
             `/api/cdn/zones/<zone_id>/waf/rate-limits/`, `/api/cdn/lists/<name>/items/`, \
             or `/api/objectstore/buckets/<name>/acl/set/`, a `PATCH` of \
             `/api/accounts/members/<id>/` whose body sets `grants`, a `PATCH` of \
             `/api/cdn/lists/<name>/` whose body sets `items`, or any other write `hipc` \
             guards that replaces a whole list or document; a body that is not a JSON \
             object counts as setting the key, and each rule holds for every write method of \
             its route, not only the one named; a `POST` or `DELETE` of \
             `/api/cdn/lists/<name>/items/`, which adds or removes the items it names, needs \
             none",
        ),
    },
];

/// When `hub subscription subscribe` refuses with `replace_required`. The window is
/// `MARKER_SECONDS` in `commands/hub.rs`.
const HUB_REPLACE_WHEN: &str = "a paid Hub plan is active, or this profile sent a Hub \
     subscribe with the same token to the same API origin in the last 10 minutes; inside that \
     window `--replace` gets past it only when the list shows a paid plan that subscribe did \
     not cancel";

/// When `compute runners limit update` asks: it reads the cap in force first.
const RUNNER_CAP_RAISED: &str = "`--monthly-cap` is higher than the limit's \
     `monthly_cap_credits`, or is `none` while that is set, or the limit has no decimal amount \
     there; `none` asks nothing while `monthly_cap` is `null` and `monthly_cap_credits` is `null` \
     or missing";

/// When `cdn cap update` asks: it reads the cap in force first.
const CDN_CAP_RAISED: &str = "`--monthly-cap` is higher than the cap's `monthly_cap` in base \
     units, or, when that is not a digit string, its `monthly_cap_credits`, or hipc cannot \
     compare the two";

/// When `compute runners settings update` asks: it reads the settings in force first.
const RUNNER_SETTINGS_SPEND_MORE: &str = "against the settings it reads first, `--enabled` \
     or `--allow-public-repos` is `true` where the setting is not, `--max-concurrent` is not `0` \
     and is higher than `max_concurrent`, or an `--allowed-size` is not in `allowed_sizes`; a \
     setting hipc cannot compare counts as higher";

/// When `billing auto-reload update` asks: it reads the setting in force first.
const RELOAD_CHARGES_MORE: &str = "the update leaves auto-reload on, and the setting it reads \
     first is off, is in a state other than `armed`, has a lower \
     `threshold`, `target`, or `monthly_cap` (`null` is no cap, the highest), or holds one of \
     those in a shape hipc cannot compare";

/// When a command that sends a secret to the API origin asks first (tenet 8).
/// When a support ticket or reply refuses with `usage`: a person reads it (tenet 5).
const TICKET_HOLDS_A_TOKEN: &str = "the subject or the message holds `HIPPIUS_TOKEN` or a \
     token a profile of the config file stores, as it is, quoted, or percent-encoded";

const NO_SECRET_FILE: &str = "`--secret-file` is not given, so the secret would go to stdout";
const NO_CREDENTIALS_DIR: &str =
    "`--credentials-dir` is not given, so the credentials would go to stdout";
const NOT_DEFAULT_ORIGIN: &str = "the API origin is not `https://api.hippius.com`";

/// When `account notifications update` is confirmed: it turns off an email that warns of a
/// problem. `commands::account::ALERTS` lists them.
const NOTIFICATION_ALERT_OFF: &str = "`--email-enabled`, `--low-credit-alerts`, \
     `--zero-balance-alerts`, or `--file-status-updates` is `false`";

/// The row for a command path such as `["auth", "status"]`, or `None` for an unknown path.
#[expect(
    clippy::too_many_lines,
    reason = "a lookup table, one arm per row kind; it grows with commands, not with logic"
)]
fn spec(path: &[&str]) -> Option<Spec> {
    match path {
        ["version" | "schema" | "completions" | "status"]
        | ["auth", "status"]
        | ["account", "list" | "get"]
        | ["account", "notifications" | "marketing-consent", "get"]
        | [
            "account",
            "members" | "invites" | "projects" | "billing-accounts" | "project-transfers",
            "list",
        ]
        | ["account", "billing-accounts", "costs"]
        | ["account", "projects", "budget" | "quota", "get"]
        | ["account", "referrals", "list" | "analytics"]
        | [
            "billing",
            "balance" | "subscriptions" | "stripe" | "summary" | "usage" | "native-quote",
        ]
        | ["billing", "auto-reload", "get" | "history"]
        | ["billing", "gamma" | "profile", "get"]
        | ["billing", "gamma", "conversions", "get"]
        | ["drive" | "s3" | "hub", "plans"]
        | ["drive" | "s3" | "hub", "subscription", "get"]
        | ["drive" | "s3", "subscription", "checkout"]
        | ["s3", "buckets" | "tokens", "list"]
        | ["s3", "buckets", "acl", "list" | "get"]
        | ["hub", "namespace", "availability" | "status" | "get"]
        | ["hub", "namespace", "robot", "get"]
        | ["hub", "usage"]
        | ["hub", "keys" | "repositories" | "artifacts", "list" | "get"]
        | ["hub", "events", "list"]
        | ["hub", "models", "formats"]
        | [
            "compute",
            "prices"
                | "public-prices"
                | "quota"
                | "charge-cap"
                | "flavors"
                | "images"
                | "apps"
                | "regions"
                | "placement-groups"
                | "feasibility"
                | "account-ssh-keys",
        ]
        | [
            "compute",
            "vms",
            "list" | "get" | "attestation" | "failovers" | "resize-options" | "resize-status",
        ]
        | ["compute", "vms", "power-policy", "get"]
        | ["compute", "public-ip", "availability" | "get" | "usage"]
        | ["compute", "firewall" | "ssh-keys", "get"]
        | ["compute", "exposures", "list"]
        | ["compute", "backups", "options" | "list" | "restore-status"]
        | ["compute", "backups", "policy", "get"]
        | ["compute", "databases", "list" | "get" | "events" | "metrics" | "refusals"]
        | [
            "compute",
            "kubernetes",
            "clusters",
            "list" | "get" | "events" | "refusals",
        ]
        | ["compute", "kubernetes", "node-pools" | "operations", "list"]
        | ["compute", "runners", "settings" | "limit", "get"]
        | ["compute", "runners", "installations", "list"]
        | ["compute", "runners", "usage"]
        // `validate`, a compose check, is a POST that changes nothing.
        | ["compute", "app-engine", "get" | "validate"]
        | ["compute", "app-engine", "apps", "list" | "get" | "runtime"]
        | ["compute", "app-engine", "guest-components", "get" | "update-status"]
        // An estimate is a POST that changes nothing.
        | ["cdn", "prices" | "regions" | "estimate" | "stats"]
        | ["cdn", "zones" | "hostnames" | "purges", "list"]
        // A Domain Connect link is read; the records change only when a person approves
        // them at the provider.
        | ["cdn", "hostnames", "domain-connect"]
        | ["cdn", "zones" | "purges" | "cap" | "rules", "get"]
        // A WAF test is a POST that changes nothing and publishes nothing.
        | ["cdn", "waf", "get" | "status" | "analytics" | "events" | "test"]
        | ["cdn", "waf", "rules" | "rate-limits", "get"]
        | ["cdn", "waf", "versions", "list" | "get"]
        | ["cdn", "lists", "list" | "get"]
        | ["support", "tickets", "get"] => Some(READ_ONLY),
        ["billing", "transactions"]
        | ["billing", "gamma", "conversions", "list"]
        | ["support", "tickets", "list"]
        | ["support", "tickets", "messages", "list"]
        | ["billing", "invoices", "list"]
        | ["drive" | "s3", "subscription", "history"]
        | ["s3", "buckets", "objects", "list"]
        | ["compute", "ssh-keys", "list"]
        | ["hub", "models", "list" | "get"]
        | ["compute", "runners", "jobs"] => Some(Spec {
            pagination: true,
            ..READ_ONLY
        }),
        // An invite preview is a POST that changes nothing. It sends the invite token, a
        // secret, so an origin but the default one is confirmed, as for a login (tenet 8).
        ["account", "invites", "preview"] => Some(Spec {
            confirmation: Confirmation::When(NOT_DEFAULT_ORIGIN),
            ..READ_ONLY
        }),
        // A token bound to any origin but the default one is confirmed (tenet 8), and so is
        // one that replaces a different stored token, since later commands act as its owner.
        ["auth", "login"] => Some(Spec {
            confirmation: Confirmation::When(
                "the API origin is not `https://api.hippius.com`, or the profile stores a \
                 different token",
            ),
            refused_when: Some(
                "an access-key login's verify answers `is_new`, a new, empty account the server \
                 has already opened (`side_effects` `partial`), and `--new-account` was not passed",
            ),
            ..UNGATED_WRITE
        }),
        // A stop takes down what the VM serves; a repeat leaves it stopped (tenet 7).
        // A WAF list's items added again leave the same list, which the server merges, but an
        // added item changes what every zone that uses the list lets through.
        ["compute", "vms", "stop"] | ["cdn", "lists", "items", "create"] => Some(Spec {
            confirmation: Confirmation::Always,
            ..UNGATED_WRITE
        }),
        // A profile update sends the fields given, so a repeat leaves the profile the same; it
        // spends nothing, but a new billing email sends the invoices elsewhere.
        ["billing", "profile", "update"] => Some(Spec {
            confirmation: Confirmation::When(
                "`--billing-email` is given and differs from the `billing_email` of the billing \
                 profile it reads first; a profile that is not found, or holds no email, \
                 differs from every email but an empty one",
            ),
            ..UNGATED_WRITE
        }),
        // The notification settings set the values given, so a repeat leaves them the same;
        // they spend nothing, but an alert turned off hides a low or empty balance.
        ["account", "notifications", "update"] => Some(Spec {
            confirmation: Confirmation::When(NOTIFICATION_ALERT_OFF),
            ..UNGATED_WRITE
        }),
        // A repeat of a VM power change leaves the power state as one call did (tenet 7).
        // A hostname verify checks its DNS records again.
        // An invoice download writes only a local file, and a repeat finds that file and stops.
        // So does a kubeconfig, which goes only to a new file and never to stdout, so it needs
        // no `--reveal`.
        ["account", "use"]
        // A rename sends the name, so a repeat leaves the same name; it spends nothing.
        | ["account", "projects" | "billing-accounts", "use" | "update"]
        | ["billing", "invoices", "get"]
        // A referral rename changes a label stored on Hippius only; the code and slug stay.
        | ["account", "referrals", "update"]
        | ["compute", "kubernetes", "clusters", "kubeconfig"]
        // A compose fix saves nothing on the server, but `--output-file` writes a new local
        // file, and a repeat finds that file and stops.
        | ["compute", "app-engine", "fix"]
        | ["compute", "vms", "start"]
        // A power policy sets one value, so a repeat leaves the same policy. Either value
        // spends nothing and is undone by setting the other, and it decides only whether a
        // VM shut down from inside is started again, which `start` does unasked.
        | ["compute", "vms", "power-policy", "update"]
        | ["cdn", "hostnames", "verify"]
        | ["cdn", "lists", "update"] => Some(UNGATED_WRITE),
        // A purge spends nothing, so it is not gated; a repeat purges again. A runner connect
        // spends nothing and links nothing (the console finishes the link); a repeat starts
        // another install flow. A reply goes to a ticket that already exists, so it is not
        // gated; a repeat posts the message again. A WAF list costs nothing, so its
        // create is not gated; tenet 7 calls a create non-idempotent, and a repeat of one name
        // is refused with 409 `list_exists`. A Kubernetes node pool starts empty and spends
        // nothing until it is scaled, which is confirmed; a repeat of one name is refused.
        ["cdn", "purges" | "lists", "create"]
        // A project or billing account is free (the free-create rule), so its create is not
        // gated; a repeat creates another. A referral link spends nothing: the server's
        // signer mints the user's code on first use and later links reuse it. A repeat of one
        // name is refused with 409.
        | ["account", "projects" | "billing-accounts" | "referrals", "create"]
        | ["compute", "kubernetes", "node-pools", "create"]
        | ["compute", "runners", "connect"]
        // A card setup charges nothing (swagger); a repeat starts another checkout.
        | ["billing", "auto-reload", "card-setup"]
        // A hotkey link spends nothing; a repeat asks for, and spends, another challenge.
        | ["billing", "gamma", "hotkeys", "create"]
        // A consent withdrawal spends nothing; swagger appends each one to the consent
        // history, so a repeat adds another entry.
        | ["account", "marketing-consent", "update"] => Some(Spec {
            effect: Effect::NonIdempotent,
            ..READ_ONLY
        }),
        ["support", "tickets", "reply"] => Some(Spec {
            effect: Effect::NonIdempotent,
            refused_when: Some(TICKET_HOLDS_A_TOKEN),
            ..READ_ONLY
        }),
        // A ticket spends nothing, but a person on the support team reads it.
        ["support", "tickets", "create"] => Some(Spec {
            refused_when: Some(TICKET_HOLDS_A_TOKEN),
            ..SPEND
        }),
        // A conversion burns gamma on chain, which cannot be refunded; a repeat burns again.
        ["billing", "gamma", "conversions", "create"]
        | ["drive" | "s3", "subscription", "subscribe" | "change"]
        | ["s3", "buckets", "create"]
        | ["compute", "vms", "create" | "reboot" | "resize"]
        // An app is a VM with a public IPv4, so a repeat launches, and bills, a second one.
        | ["compute", "app-engine", "apps", "create"]
        // A repeat restores again once the first is done, losing what was written since.
        | ["compute", "backups", "restore"]
        | ["compute", "ssh-keys", "create"]
        // A restart is queued again on a repeat, and restarts PostgreSQL again.
        | ["compute", "databases", "restart"]
        // A scale up and a replacement launch billed VMs. A scale is to a size, but a repeat
        // after an add was rejected (its signature expired) adds again, so both count as
        // spending again.
        | ["compute", "kubernetes", "node-pools", "scale"]
        | ["compute", "kubernetes", "nodes", "replace"]
        | ["cdn", "hostnames", "create"]
        | ["account", "invites", "create"]
        // A repeat offers the project again. The project leaves the account once the other
        // account accepts, so it is confirmed.
        | ["account", "projects", "transfer"]
        // A repeat starts another checkout.
        | ["billing", "top-up"] => Some(SPEND),
        // Hub lets an account hold a plan twice, so a second subscribe charges twice
        // (tenet 11).
        ["hub", "subscription", "subscribe"] => Some(Spec {
            guards: &[Guard {
                code: Code::ReplaceRequired,
                when: Some(HUB_REPLACE_WHEN),
            }],
            ..SPEND
        }),
        // Whether the bucket is private is known only from the answer, after the write, so
        // every create needs `--public`, and every update that moves the origin.
        // A repeat leaves the namespace as one call did. Opening it to anonymous pulls needs
        // `--public` (tenet 5); closing it does not.
        ["hub", "namespace", "update"] => Some(Spec {
            guards: &[Guard {
                code: Code::PublicRequired,
                when: Some("`--visibility` is `public`"),
            }],
            ..UNDO
        }),
        ["cdn", "zones", "create"] => Some(Spec {
            guards: &[Guard {
                code: Code::PublicRequired,
                when: None,
            }],
            ..SPEND
        }),
        ["auth", "logout"]
        | ["drive" | "s3" | "hub", "subscription", "cancel"]
        | ["s3", "buckets", "delete"]
        | ["s3", "tokens", "revoke"]
        // An unlink leaves the gamma in its deposit; a repeat finds the hotkey unlinked (404).
        | ["billing", "gamma", "hotkeys", "delete"]
        // A repeat delete of a Hub key, repository, or artifact finds it gone. Hub data
        // deleted there is gone for good, which the question says; `--yes` is the one
        // confirmation (tenet 5).
        | ["hub", "keys" | "repositories" | "artifacts", "delete"]
        // A repeat delete of a database answers 409 "Already decommissioned" (swagger).
        | ["compute", "vms" | "ssh-keys" | "databases", "delete"]
        // Swagger: a policy PUT sets the interval and the DELETE is "Idempotent". Stored
        // backups are billed, and turning them off ends new ones, so both are confirmed.
        | ["compute", "backups", "policy", "update" | "delete"]
        // Swagger: attaching again returns the address already attached, and exposing a port
        // and protocol again returns the exposure that exists, so a repeat is safe. An
        // attach starts a bill and an exposure opens a port to the internet, so both are
        // confirmed.
        | ["compute", "public-ip" | "exposures", "create" | "delete"]
        // Exposing a database publishes its primary and mints nothing, so a repeat leaves it
        // published; it makes the primary reachable from the internet, so it is confirmed.
        | ["compute", "databases", "exposure", "create" | "delete"]
        | ["cdn", "zones" | "hostnames" | "lists", "delete"]
        // An invite is used once, as the console treats it (unverified against the API): a
        // repeat accept is refused and the caller stays a member.
        // Joining gives the token access to another account, so it is confirmed.
        | ["account", "invites", "revoke" | "accept"]
        // Unlinking an installation again finds it gone.
        | ["compute", "runners", "installations", "delete"]
        // A cluster delete again finds the teardown running or over. A cancel moves the
        // operation out of `queued`, `running`, or `blocked`, and a resume out of `blocked`
        // or `queued`, so a repeat usually finds it moved and refuses before the question. A
        // node removal again finds the remove operation running.
        | ["compute", "kubernetes", "clusters" | "nodes", "delete"]
        | ["compute", "kubernetes", "operations", "cancel" | "resume"]
        // Swagger: a project delete archives an empty project, and a repeat finds it
        // archived.
        | ["account", "members" | "projects", "delete"]
        // A budget PUT replaces the whole budget, a quota PATCH sets the limits given, and a
        // budget delete again finds none; each changes what the project may spend or run, so
        // each is confirmed.
        | ["account", "projects", "budget", "update" | "delete"]
        | ["account", "projects", "quota", "update"]
        // A transfer is decided once: a repeat accept, decline, or cancel is refused, and
        // the project stays where the first left it. An accept moves a project into the
        // token's own account, a decline ends an offer, and a cancel withdraws one.
        | ["account", "project-transfers", "accept" | "decline" | "cancel"]
        // A WAF settings change publishes a version, and a mode but `off` is billed. A repeat
        // is safe.
        | ["cdn", "waf", "update"]
        // A rollback publishes a copy of that version again, which leaves the same WAF.
        | ["cdn", "waf", "versions", "rollback"]
        // A DNS setup writes the hostname's records at the provider, and a repeat writes the
        // same records. It changes a DNS zone outside Hippius, so it is confirmed.
        | ["cdn", "hostnames", "dns-setup"]
        // A close again leaves the ticket closed. hipc refuses one of a ticket its read shows
        // closed, so a repeat of a close that worked exits `conflict`; it is still
        // `idempotent` on the assumption that the server's `PATCH` only sets the status, so
        // a second one, say after a close elsewhere between the read and the write, changes
        // nothing. It ends the support team's work on the ticket, as a cancel does, so it is
        // confirmed.
        | ["support", "tickets", "update"]
        | ["cdn", "lists", "items", "delete"] => Some(UNDO),
        // A rules file replaces the whole list, so a repeat leaves the same list.
        ["compute", "firewall", "update"] => Some(Spec {
            guards: &[Guard {
                code: Code::RemoveOthersRequired,
                when: Some(
                    "the rules file leaves out a rule the VM has now, or changes its protocol, \
                     ports, sources, or enabled flag, in either direction; a missing enabled \
                     counts as true",
                ),
            }],
            ..UNDO
        }),
        ["cdn", "rules", "update"] => Some(Spec {
            guards: &[Guard {
                code: Code::RemoveOthersRequired,
                when: Some("the rules file leaves out or moves a rule the zone has now"),
            }],
            ..UNDO
        }),
        ["cdn", "waf", "rules", "update"] => Some(Spec {
            guards: &[Guard {
                code: Code::RemoveOthersRequired,
                when: Some(
                    "the rules file leaves out or moves a rule the zone has now, or changes its \
                     expression, action, or enabled flag; a missing enabled counts as true",
                ),
            }],
            confirmation: Confirmation::When(WAF_CHECK),
            ..UNDO
        }),
        ["cdn", "waf", "rate-limits", "update"] => Some(Spec {
            guards: &[Guard {
                code: Code::RemoveOthersRequired,
                when: Some(
                    "the rules file leaves out a rate limit the zone has now, or changes its \
                     expression, action, key, period, requests, timeout, or enabled flag; a \
                     missing enabled counts as true",
                ),
            }],
            confirmation: Confirmation::When(WAF_CHECK),
            ..UNDO
        }),
        ["cdn", "lists", "items", "update"] => Some(Spec {
            guards: &[Guard {
                code: Code::RemoveOthersRequired,
                when: Some("the items file leaves out an item the list stores now"),
            }],
            ..UNDO
        }),
        // A policy replaces the whole bucket policy, so a repeat leaves the same policy. Its
        // statements are not judged, so both guards always apply (`commands/s3.rs`).
        ["s3", "buckets", "acl", "update"] => Some(Spec {
            guards: &[
                Guard {
                    code: Code::PublicRequired,
                    when: None,
                },
                Guard {
                    code: Code::RemoveOthersRequired,
                    when: None,
                },
            ],
            ..UNDO
        }),
        // A member update replaces the whole grant list, so a repeat leaves the same list.
        ["account", "members", "update"] => Some(Spec {
            guards: &[Guard {
                code: Code::RemoveOthersRequired,
                when: Some("a `--grant` is given, and the grants leave out one the member has now"),
            }],
            ..UNDO
        }),
        // A zone update can publish a private bucket. A repeat is safe.
        ["cdn", "zones", "update"] => Some(Spec {
            guards: &[Guard {
                code: Code::PublicRequired,
                when: Some("`--bucket` or `--prefix` is given"),
            }],
            ..UNDO
        }),
        // Writing a new profile replaces nothing, so only an existing one is confirmed.
        ["s3", "configure"] => Some(Spec {
            confirmation: Confirmation::When("the AWS profile already exists"),
            ..UNDO
        }),
        // Auto-reload charges the card by itself, so a change that may charge more asks.
        ["billing", "auto-reload", "update"] => Some(Spec {
            confirmation: Confirmation::When(RELOAD_CHARGES_MORE),
            ..UNDO
        }),
        // A lower cap, or a first one, allows less spend, so only a raise or a removal asks.
        ["compute", "runners", "limit", "update"] => Some(Spec {
            confirmation: Confirmation::When(RUNNER_CAP_RAISED),
            ..UNDO
        }),
        // Every spend cap asks the same way: a raise, not a cut. A repeat is safe.
        ["cdn", "cap", "update"] => Some(Spec {
            confirmation: Confirmation::When(CDN_CAP_RAISED),
            ..UNDO
        }),
        // A settings update sets the keys given, so a repeat leaves the same settings; only a
        // change that may let jobs spend more of the balance asks.
        ["compute", "runners", "settings", "update"] => Some(Spec {
            confirmation: Confirmation::When(RUNNER_SETTINGS_SPEND_MORE),
            ..UNDO
        }),
        // The server shows the new secret once, so these also need `--reveal`. A database
        // create and a restore, which makes a new database, answer with its credentials.
        // The Kubernetes backups answer holds presigned download links and sealed keys; the
        // read changes nothing. A presigned URL reads the object with no other credential
        // until it expires.
        ["compute", "kubernetes", "backups", "list"]
        | ["s3", "buckets", "objects", "presign"] => Some(Spec {
            reveal: Reveal::Always,
            ..READ_ONLY
        }),
        // A file the caller names for the secret asks for it as plainly as `--reveal`, and
        // keeps it off stdout, so either flag lets the request go.
        ["s3", "tokens", "create" | "rotate"]
        | ["hub", "keys", "create" | "rotate"]
        | ["hub", "namespace", "create"]
        | ["hub", "namespace", "robot", "rotate"] => Some(Spec {
            reveal: Reveal::When(NO_SECRET_FILE),
            ..SPEND
        }),
        ["compute", "databases", "create" | "restore"] => Some(Spec {
            reveal: Reveal::When(NO_CREDENTIALS_DIR),
            ..SPEND
        }),
        // A POST can spend money, so `hipc api` reports the effect of its most dangerous
        // method. At run time GET and HEAD are reads and need no confirmation, except on
        // `/api/network/me/`, whose first call creates the account's network, so it is
        // confirmed as `hipc network get` is. The requests whose answer holds a secret need
        // `--reveal`, reads included, as the curated commands that reach some of them do. A
        // change to the network's isolation needs a flag that names the effect, the routes of
        // the curated `--public` and `--remove-others` guards need those flags, an account
        // deletion request is left to the console, and a cluster's sealed kubeconfig to `hipc
        // compute kubernetes clusters kubeconfig`.
        ["api"] => Some(Spec {
            confirmation: Confirmation::When(API_CONFIRMATION),
            reveal: Reveal::When("the route answers with a secret"),
            pagination: true,
            refused_when: Some(API_REFUSED_WHEN),
            guards: API_GUARDS,
            ..SPEND
        }),
        ["network", rest @ ..] => network_spec(rest),
        _ => None,
    }
}

/// When `hipc api` refuses whatever the flags, a dry run included: the console is the one
/// path to an account deletion, `hipc compute kubernetes clusters kubeconfig` the one
/// path to a cluster's sealed kubeconfig, and `hipc cdn hostnames dns-setup` the one path to a
/// DNS setup, whose body and answer hold a provider credential, and `hipc hub subscription
/// subscribe` the one path to a Hub subscribe, since it reads the current plans first. The
/// three App Engine routes
/// that take `secrets` are refused when the body sets it: `hipc compute app-engine apps
/// create` is the path for those values. A support write whose body holds an API token or an
/// access key is refused as `hipc support tickets create` and `reply` refuse one. It lists refused requests
/// only; what `hipc api` sends that a curated command refuses or never sends, the data-loss
/// resume and a ticket `PATCH` with any body, is in its help.
const API_REFUSED_WHEN: &str = "the target is a full URL off the configured API origin, or \
                                the request is `POST /api/account/deletion/`, any method of \
                                `/api/auth/token/`, `GET` or `HEAD` \
                                of `/api/kubernetes/clusters/<cluster_id>/secrets/`, a `POST`, \
                                `PUT`, or `PATCH` of \
                                `/api/cdn/zones/<zone_id>/hostnames/<hostname>/dns-setup/`, \
                                `POST /api/registry/subscribe/`, or a `POST \
                                /api/compute/app-engine/apps/`, a write of \
                                `/api/compute/app-engine/apps/<vm_id>/compose/`, or a `POST \
                                /api/compute/app-engine/validate/` whose body sets `secrets` \
                                (a body that is not a JSON object counts as setting it), or a \
                                `POST`, `PUT`, `PATCH`, or `DELETE` under `/api/support/` \
                                whose body holds `HIPPIUS_TOKEN` or a token a profile of the \
                                config file stores, as it is, quoted, percent-encoded, behind \
                                a JSON escape, or split by whitespace, or an access key \
                                whose checksum holds, with or without the trailing slash or a \
                                format suffix";

/// When `hipc api` asks: the one route it confirms on a read is the one the rows of
/// `hippius::PROVISIONING_ROUTES` name, written here so the text says which, in every form
/// the route tables match.
const API_CONFIRMATION: &str = "the method is not `GET` or `HEAD`, or the path is \
                                `/api/network/me/`, whose first call creates the network, \
                                with or without the trailing slash or a format suffix";

/// The rows of `hipc network`, by the path after `network`.
fn network_spec(path: &[&str]) -> Option<Spec> {
    match path {
        ["groups" | "policies" | "vpcs", "list"]
        | ["peers", "list" | "get"]
        | ["vpcs", "resources" | "routers", "list"] => Some(READ_ONLY),
        // Swagger: the first `GET /api/network/me/` for an account creates its network (a
        // default group, a VPC network, and the intra-network policy), and later calls find
        // it. A repeat leaves the server as one call did, and every call is confirmed because
        // which one is the first is not known before it is sent.
        // Swagger calls `POST /api/network/provision/` idempotent: a repeat finds the network
        // the first call created. It creates the network, so it is confirmed.
        // A rename sets the name, so a repeat leaves the same name; a repeat remove finds the
        // peer gone. A peer is a device or VM that someone reaches over the network, so both
        // are confirmed.
        ["get" | "create"] | ["peers", "update" | "delete"] => Some(UNDO),
        _ => None,
    }
}

/// Every leaf command path under `command`, in declaration order.
fn command_paths(command: &clap::Command) -> Vec<Vec<String>> {
    fn walk(command: &clap::Command, prefix: &[String], out: &mut Vec<Vec<String>>) {
        for sub in command.get_subcommands() {
            let mut path = prefix.to_vec();
            path.push(sub.get_name().to_owned());
            if sub.has_subcommands() {
                walk(sub, &path, out);
            } else {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(command, &[], &mut out);
    out
}

/// The name of an argument in the schema: a flag by its long form, `--kind`, and a
/// positional argument by its clap id, such as `vm_id`.
fn arg_name(arg: &clap::Arg) -> String {
    match arg.get_long() {
        Some(long) => format!("--{long}"),
        None => arg.get_id().to_string(),
    }
}

/// One argument or flag of a command, as clap declares it.
///
/// A switch takes no value, so its `value_name` and `default` are null and its
/// `possible_values` empty: clap gives a switch the parser values `true` and `false` and the
/// default `false`, which a caller never types. `required` is about the argument alone; a
/// set of which one is required is in the command's `required_groups`.
fn arg_entry(arg: &clap::Arg) -> Value {
    let takes_value = arg.get_action().takes_values();
    let value_name = arg
        .get_value_names()
        .and_then(<[_]>::first)
        .filter(|_| takes_value)
        .map(ToString::to_string);
    // No argument of `hipc` has more than one default; a spec test holds that, so the first
    // is the default.
    let default = arg
        .get_default_values()
        .first()
        .filter(|_| takes_value)
        .map(|value| value.to_string_lossy().into_owned());
    let possible_values: Vec<String> = if takes_value {
        arg.get_possible_values()
            .iter()
            .filter(|value| !value.is_hide_set())
            .map(|value| value.get_name().to_owned())
            .collect()
    } else {
        Vec::new()
    };
    let repeats = matches!(arg.get_action(), clap::ArgAction::Append)
        || arg
            .get_num_args()
            .is_some_and(|range| range.max_values() > 1);
    json!({
        "name": arg_name(arg),
        "value_name": value_name,
        "required": arg.is_required_set(),
        "repeats": repeats,
        "possible_values": possible_values,
        "default": default,
        "help": arg.get_help().map(ToString::to_string),
    })
}

/// The entries of the arguments of `command` that `keep` selects, in the order its help
/// lists them, leaving out the hidden ones.
///
/// The help orders flags by display order, then long name (`option_sort_key` in clap's
/// `output/help_template.rs`). The display order is fixed when an argument is added, so it
/// differs from the declaration order where `mut_arg` moves an argument to the end, as the
/// `history` commands do with `--limit`. Positional arguments have no display order and
/// keep the declaration order, which is their index; the sort is stable.
fn arg_entries(command: &clap::Command, keep: fn(&clap::Arg) -> bool) -> Vec<Value> {
    let mut args: Vec<&clap::Arg> = command
        .get_arguments()
        .filter(|arg| keep(arg) && !arg.is_hide_set())
        .collect();
    args.sort_by_key(|arg| (arg.get_display_order(), arg.get_long()));
    args.into_iter().map(arg_entry).collect()
}

/// The sets of arguments of `command` of which at least one is required, each with whether
/// more than one may be given. clap's derive also gives every `Args` struct a group of
/// all its fields, which requires nothing, so only a required group is listed.
fn required_groups(command: &clap::Command) -> Vec<Value> {
    command
        .get_groups()
        .filter(|group| group.is_required_set())
        .map(|group| {
            let names: Vec<String> = group
                .get_args()
                .map(|id| {
                    let arg = command
                        .get_arguments()
                        .find(|arg| arg.get_id() == id)
                        .expect("clap checks that a group names arguments of its command");
                    arg_name(arg)
                })
                .collect();
            // `ArgGroup::is_multiple` takes `&mut self`, so it reads a copy.
            json!({ "names": names, "multiple": group.clone().is_multiple() })
        })
        .collect()
}

/// The command at `path` under `root`, one of `command_paths(root)`.
fn leaf<'a>(root: &'a clap::Command, path: &[String]) -> &'a clap::Command {
    path.iter()
        .try_fold(root, |command, name| command.find_subcommand(name))
        .expect("a path from command_paths resolves")
}

/// The edit distance between `a` and `b`, in characters: the fewest insertions, deletions,
/// and substitutions that turn one into the other.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 1)
                .min(row[j] + 1)
                .min(diagonal + usize::from(ca != *cb));
            diagonal = above;
        }
    }
    row[b.len()]
}

/// How near a name is to a word: its edit distance, then whether it does not start with the
/// word, so at the same distance `apps` is nearer to `app` than `api` is.
type Nearness = (usize, bool);

/// The name in `names` nearest to `word`, case aside, with its [`Nearness`], when one is near
/// enough to be a likely typo or a short form: one edit, or up to a third of the longer
/// one's length, away, or, for a word of two or more characters, a name that starts with
/// it.
fn nearest<'a>(word: &str, names: &[&'a str]) -> Option<(Nearness, &'a str)> {
    let word = word.to_lowercase();
    let long = word.chars().count() >= 2;
    names
        .iter()
        .map(|name| (distance(&word, name), *name))
        .filter(|(far, name)| {
            let longer = word.chars().count().max(name.chars().count());
            *far <= (longer / 3).max(1) || (long && name.starts_with(&word))
        })
        .map(|(far, name)| ((far, !name.starts_with(&word)), name))
        .min_by_key(|(near, _)| *near)
}

/// The most paths [`deeper`] suggests at once; a name that ends more, such as `list`, is
/// too common to point anywhere.
const MAX_DEEPER: usize = 3;

/// The paths under `command`, deeper than its own subcommands, whose last name is the one
/// nearest to `word`, with its [`Nearness`], as `compute vms` is for `vms` under `hipc`: `None`
/// when no name is near, or when the nearest name ends more than [`MAX_DEEPER`] paths.
fn deeper(command: &clap::Command, word: &str) -> Option<(Nearness, Vec<Vec<String>>)> {
    let mut groups: Vec<Vec<String>> = Vec::new();
    for path in command_paths(command) {
        for end in 2..=path.len() {
            let group = path[..end].to_vec();
            if !groups.contains(&group) {
                groups.push(group);
            }
        }
    }
    let ends: Vec<&str> = groups.iter().map(|g| g[g.len() - 1].as_str()).collect();
    let (far, name) = nearest(word, &ends)?;
    let found: Vec<Vec<String>> = groups
        .iter()
        .filter(|g| g[g.len() - 1] == name)
        .cloned()
        .collect();
    (found.len() <= MAX_DEEPER).then_some((far, found))
}

/// The names of the commands `command` holds, hidden ones aside, each quoted and joined as
/// a message lists them: `` `prices`, `vms`, and `kubernetes` ``.
pub(crate) fn held(command: &clap::Command) -> String {
    let held: Vec<String> = command
        .get_subcommands()
        .filter(|sub| !sub.is_hide_set())
        .map(|sub| hippius::phrase::quote(sub.get_name()))
        .collect();
    hippius::phrase::join(&held, "and")
}

/// The command paths nearest to `word`, a name `command` does not hold, each from the root:
/// `path` is where `command` stands, and a path found is a command `command` holds or one
/// deeper, as `compute vms` is for `vm` under `hipc` ([`nearest`], [`deeper`]); at a tie, the
/// one `command` holds. `None` when no name is near.
pub(crate) fn near_commands(
    command: &clap::Command,
    path: &[String],
    word: &str,
) -> Option<Vec<Vec<String>>> {
    let names: Vec<&str> = command
        .get_subcommands()
        .filter(|sub| !sub.is_hide_set())
        .map(clap::Command::get_name)
        .collect();
    let here =
        nearest(word, &names).map(|(far, name)| (far, vec![[path, &[name.to_owned()]].concat()]));
    let below = deeper(command, word).map(|(far, found)| {
        let found = found
            .into_iter()
            .map(|below| [path, &below].concat())
            .collect();
        (far, found)
    });
    match (here, below) {
        (Some(here), Some(below)) if below.0 < here.0 => Some(below.1),
        (Some(here), _) => Some(here.1),
        (None, below) => below.map(|(_, found)| found),
    }
}

/// The command path `words` names under `root`, each word as the command's own name (an
/// alias resolves to it), so it compares with the paths of `command_paths`.
///
/// # Errors
///
/// `usage` for a word that is not a command where it stands, with the nearest command or
/// group, at that place or deeper, and the names there are, and for a word after a command
/// that has no subcommands.
fn resolve(root: &clap::Command, words: &[String]) -> hippius::Result<Vec<String>> {
    let quote = |text: &str| hippius::phrase::quote(text);
    let shown = |prefix: &str, path: &[String]| {
        let mut line = vec!["hipc", prefix];
        line.retain(|word| !word.is_empty());
        line.extend(path.iter().map(String::as_str));
        quote(&line.join(" "))
    };
    let mut command = root;
    let mut path: Vec<String> = Vec::new();
    for word in words {
        let place = shown("", &path);
        if !command.has_subcommands() {
            return Err(hippius::Error::usage(
                format!(
                    "{place} has no command {}: it is a command, not a group",
                    quote(word)
                ),
                format!("run {} for that command", shown("schema", &path)),
            ));
        }
        let Some(sub) = command.find_subcommand(word) else {
            let near = near_commands(command, &path, word);
            let held = held(command);
            let hint = match near {
                Some(near) => {
                    let near: Vec<String> = near.iter().map(|path| shown("schema", path)).collect();
                    format!("run {}", hippius::phrase::join(&near, "or"))
                }
                None => format!(
                    "run {} with one of the commands it holds",
                    shown("schema", &path)
                ),
            };
            let what = if word.is_empty() {
                "with an empty name".to_owned()
            } else {
                quote(word)
            };
            return Err(hippius::Error::usage(
                format!("{place} has no command {what}; it holds {held}"),
                hint,
            ));
        };
        path.push(sub.get_name().to_owned());
        command = sub;
    }
    Ok(path)
}

/// Adds to a dry run's document whether the real run asks for `--yes`, as `hipc schema` says
/// it for the command at `path`: `confirmation`, and `confirmation_when` when that depends on
/// what the real run reads or is given. A dry run sends nothing, so it cannot say more than
/// the schema: a spend cap that asks only for a raise cannot know the cap in force. The caller
/// calls it only for a run given `--dry-run`, and only a dry run that lists `requests` gets
/// them; a local change's dry run keeps its shape, `dry_run`
/// then the document a real run prints. Any other document is left as it is.
pub(crate) fn add_confirmation(path: &[String], document: &mut Value) {
    if let Value::Object(fields) = document
        && fields.get("dry_run") == Some(&Value::Bool(true))
        && fields.contains_key("requests")
    {
        let parts: Vec<&str> = path.iter().map(String::as_str).collect();
        let spec = spec(&parts).expect("every command has a spec row; spec tests enforce it");
        fields.insert(
            "confirmation".to_owned(),
            (spec.confirmation != Confirmation::Never).into(),
        );
        let when = match spec.confirmation {
            Confirmation::When(when) => Some(when),
            Confirmation::Never | Confirmation::Always => None,
        };
        fields.insert("confirmation_when".to_owned(), when.into());
    }
}

/// The `hipc schema` document, with only the commands under `prefix` when it names a
/// command path, such as `["compute", "app-engine"]`. Every other field is the same with or
/// without a prefix, so a filtered document has the shape of the whole one and the same
/// `schema_version`.
///
/// # Errors
///
/// `usage` for a prefix that is not a command path (see [`resolve`]).
pub(crate) fn schema(prefix: &[String]) -> hippius::Result<Value> {
    // Unbuilt, so a command holds only its own arguments: clap copies the global flags into
    // every subcommand, and adds `--help`, only when it builds the command for a parse.
    let root = Cli::command();
    let prefix = resolve(&root, prefix)?;
    let commands: Vec<Value> = command_paths(&root)
        .iter()
        .filter(|path| path.starts_with(&prefix))
        .map(|path| {
            let parts: Vec<&str> = path.iter().map(String::as_str).collect();
            let spec = spec(&parts).expect("every command has a spec row; spec tests enforce it");
            let command = leaf(&root, path);
            json!({
                "path": path,
                "effect": spec.effect.as_str(),
                "confirmation": spec.confirmation != Confirmation::Never,
                "reveal": spec.reveal != Reveal::Never,
                "pagination": spec.pagination,
                "confirmation_when": match spec.confirmation {
                    Confirmation::When(when) => Some(when),
                    Confirmation::Never | Confirmation::Always => None,
                },
                "reveal_when": match spec.reveal {
                    Reveal::When(when) => Some(when),
                    Reveal::Never | Reveal::Always => None,
                },
                "refused_when": spec.refused_when,
                "guards": spec.guards.iter().map(|guard| json!({
                    "code": guard.code.as_str(),
                    "flag": guard.code.flag(),
                    "when": guard.when,
                })).collect::<Vec<Value>>(),
                "args": arg_entries(command, clap::Arg::is_positional),
                "flags": arg_entries(command, |arg| !arg.is_positional()),
                "required_groups": required_groups(command),
            })
        })
        .collect();
    // The code table of tenet 4, so an agent learns every code and its exit code from the
    // binary.
    let errors: Vec<Value> = Code::ALL
        .iter()
        .map(|code| {
            json!({
                "code": code.as_str(),
                "exit": code.exit_code(),
                "meaning": code.meaning(),
                "flag": code.flag(),
            })
        })
        .collect();
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        // The root's own arguments are the global flags, which every command accepts.
        "global_flags": arg_entries(&root, |_| true),
        "commands": commands,
        "errors": errors,
    }))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// The schema row of `path`, one of `command_paths`.
    fn row(path: &[String]) -> Spec {
        let parts: Vec<&str> = path.iter().map(String::as_str).collect();
        spec(&parts).expect("every command has a spec row")
    }

    /// TENETS 2: `get` reads one resource, with or without an id, so no leaf is named or
    /// aliased `show` or `describe`.
    #[test]
    fn no_leaf_is_named_or_aliased_show_or_describe() {
        let root = Cli::command();
        let synonyms: Vec<String> = command_paths(&root)
            .into_iter()
            .filter(|path| {
                let leaf = leaf(&root, path);
                [leaf.get_name()]
                    .into_iter()
                    .chain(leaf.get_all_aliases())
                    .any(|name| name == "show" || name == "describe")
            })
            .map(|path| path.join(" "))
            .collect();
        assert!(
            synonyms.is_empty(),
            "these leaves use a synonym of get: {synonyms:?}"
        );
    }

    /// TENETS 2: a group exists because it groups several commands, so none holds one
    /// (so `billing stripe show` became `billing stripe`), apart from the exceptions tenet 2
    /// names.
    #[test]
    fn no_group_holds_one_command() {
        /// Every command under `command` with how many subcommands it has.
        fn sizes(command: &clap::Command, prefix: &str, out: &mut Vec<(String, usize)>) {
            for sub in command.get_subcommands() {
                let path = format!("{prefix} {}", sub.get_name());
                out.push((path.clone(), sub.get_subcommands().count()));
                sizes(sub, &path, out);
            }
        }
        let mut all = Vec::new();
        sizes(&Cli::command(), "hipc", &mut all);
        let lonely: Vec<_> = all.into_iter().filter(|(_, size)| *size == 1).collect();
        // The exceptions of tenet 2. The grammar is `hipc support tickets` by design, and
        // `support` holds the `tickets` group, not one command. The others hold only `list`,
        // because a list is `<noun> list`, and fill when their writes land (the writes of the
        // network groups, policies, and VPCs come in a later release, `compute kubernetes
        // backups` holds `list` alone until the backup writes are added, `hub events` until the
        // API has another events route, and `support tickets messages` until an attachment
        // upload joins it); a group not named here that holds one command fails, so this list
        // only shrinks.
        let one = |path: &str| (path.to_owned(), 1);
        assert_eq!(
            lonely,
            [
                one("hipc hub events"),
                one("hipc compute kubernetes backups"),
                one("hipc support"),
                one("hipc support tickets messages"),
                one("hipc network groups"),
                one("hipc network policies"),
                one("hipc network vpcs resources"),
                one("hipc network vpcs routers"),
            ]
        );
    }

    /// The leaves that end in a noun: each reads the thing it names and changes nothing
    /// (tenet 2), or is one of the top-level commands that are not resources. Two can write,
    /// as tenet 2 says: `api` and `kubeconfig`.
    const NOUN_LEAVES: [&str; 44] = [
        "api",
        "schema",
        "version",
        "status",
        "completions",
        "account-ssh-keys",
        "analytics",
        "apps",
        "attestation",
        "availability",
        "balance",
        "charge-cap",
        "checkout",
        "costs",
        "events",
        "failovers",
        "feasibility",
        "flavors",
        "formats",
        "history",
        "images",
        "jobs",
        "kubeconfig",
        "metrics",
        "native-quote",
        "options",
        "placement-groups",
        "plans",
        "prices",
        "public-prices",
        "quota",
        "refusals",
        "regions",
        "resize-options",
        "resize-status",
        "restore-status",
        "runtime",
        "stats",
        "stripe",
        "subscriptions",
        "summary",
        "transactions",
        "update-status",
        "usage",
    ];

    /// TENETS 2: the verb shelf is closed, so every leaf, and every alias of one, ends in a
    /// shelf verb or a noun this list allows. A new verb must be added to the shelf in
    /// TENETS.md and here, and a new noun leaf here, so neither slips in unreviewed.
    #[test]
    fn every_leaf_ends_in_a_shelf_verb_or_an_allowed_noun() {
        let root = Cli::command();
        let off_shelf: Vec<String> = command_paths(&root)
            .into_iter()
            .filter(|path| {
                let leaf = leaf(&root, path);
                [leaf.get_name()]
                    .into_iter()
                    .chain(leaf.get_all_aliases())
                    .any(|name| !VERBS.contains(&name) && !NOUN_LEAVES.contains(&name))
            })
            .map(|path| path.join(" "))
            .collect();
        assert!(
            off_shelf.is_empty(),
            "these leaves end in a word on neither the verb shelf nor the noun list: {off_shelf:?}"
        );
    }

    /// The shelf and the noun list hold only words a leaf uses, and no word is on both, so
    /// neither list grows stale and a word is a verb or a noun, never both.
    #[test]
    fn the_shelf_and_the_noun_list_hold_only_words_in_use() {
        let root = Cli::command();
        let used: BTreeSet<String> = command_paths(&root)
            .iter()
            .map(|path| leaf(&root, path).get_name().to_owned())
            .collect();
        let unused: Vec<&str> = VERBS
            .iter()
            .chain(&NOUN_LEAVES)
            .copied()
            .filter(|word| !used.contains(*word))
            .collect();
        assert!(unused.is_empty(), "no leaf ends in: {unused:?}");
        let both: Vec<&str> = VERBS
            .iter()
            .copied()
            .filter(|verb| NOUN_LEAVES.contains(verb))
            .collect();
        assert!(both.is_empty(), "both a verb and a noun: {both:?}");
    }

    /// TENETS 2: a noun leaf changes nothing, but for the two the tenet names, so a new noun
    /// that writes is either renamed to a verb or named in the tenet first.
    #[test]
    fn only_the_noun_leaves_tenet_2_names_can_write() {
        let root = Cli::command();
        let writes: Vec<String> = command_paths(&root)
            .into_iter()
            .filter(|path| NOUN_LEAVES.contains(&leaf(&root, path).get_name()))
            .filter(|path| {
                let words: Vec<&str> = path.iter().map(String::as_str).collect();
                let row = spec(&words).expect("every command has a spec row");
                !matches!(row.effect, Effect::ReadOnly)
            })
            .map(|path| path.join(" "))
            .collect();
        assert_eq!(writes, ["compute kubernetes clusters kubeconfig", "api"]);
        let tenet = include_str!("../../../TENETS.md")
            .lines()
            .find(|line| line.starts_with("A group exists because"))
            .expect("tenet 2 has the noun leaves");
        for named in [
            "the one exception among the resources is `hipc compute kubernetes clusters \
             kubeconfig <cluster_id>`",
            "`hipc api` is a noun too",
        ] {
            assert!(tenet.contains(named), "tenet 2 does not say: {named}");
        }
    }

    /// TENETS.md lists the same shelf as [`VERBS`], in the same order, so the written contract
    /// and the test cannot drift apart.
    #[test]
    fn the_tenets_list_the_verb_shelf() {
        let tenets = include_str!("../../../TENETS.md");
        let shelf = tenets
            .lines()
            .find(|line| line.starts_with("Verbs come from a closed shelf:"))
            .expect("tenet 2 has the verb shelf")
            .split(". ")
            .next()
            .expect("the shelf is the first sentence");
        let listed: Vec<&str> = shelf.split('`').skip(1).step_by(2).collect();
        assert_eq!(listed, VERBS);
    }

    #[test]
    fn every_command_has_a_spec_row() {
        let paths = command_paths(&Cli::command());
        let missing: Vec<&Vec<String>> = paths
            .iter()
            .filter(|path| {
                let parts: Vec<&str> = path.iter().map(String::as_str).collect();
                spec(&parts).is_none()
            })
            .collect();
        assert_eq!(missing, Vec::<&Vec<String>>::new());
    }

    /// Every command, with its effect and whether it asks, written out by hand so a
    /// change to a command's gate fails here.
    const PLAN_ROWS: [(&str, &str, bool); 273] = [
        ("version", "read_only", false),
        ("status", "read_only", false),
        ("schema", "read_only", false),
        ("completions", "read_only", false),
        ("auth login", "idempotent", true),
        ("auth status", "read_only", false),
        ("auth logout", "idempotent", true),
        ("account list", "read_only", false),
        ("account use", "idempotent", false),
        ("account get", "read_only", false),
        ("account notifications get", "read_only", false),
        ("account notifications update", "idempotent", true),
        ("account marketing-consent get", "read_only", false),
        ("account marketing-consent update", "non_idempotent", false),
        ("account members list", "read_only", false),
        ("account members update", "idempotent", true),
        ("account members delete", "idempotent", true),
        ("account invites list", "read_only", false),
        ("account invites create", "non_idempotent", true),
        ("account invites revoke", "idempotent", true),
        ("account invites preview", "read_only", true),
        ("account invites accept", "idempotent", true),
        ("account projects list", "read_only", false),
        ("account projects create", "non_idempotent", false),
        ("account projects update", "idempotent", false),
        ("account projects delete", "idempotent", true),
        ("account projects transfer", "non_idempotent", true),
        ("account projects budget get", "read_only", false),
        ("account projects budget update", "idempotent", true),
        ("account projects budget delete", "idempotent", true),
        ("account projects quota get", "read_only", false),
        ("account projects quota update", "idempotent", true),
        ("account projects use", "idempotent", false),
        ("account project-transfers list", "read_only", false),
        ("account project-transfers accept", "idempotent", true),
        ("account project-transfers decline", "idempotent", true),
        ("account project-transfers cancel", "idempotent", true),
        ("account billing-accounts list", "read_only", false),
        ("account billing-accounts create", "non_idempotent", false),
        ("account billing-accounts update", "idempotent", false),
        ("account billing-accounts costs", "read_only", false),
        ("account billing-accounts use", "idempotent", false),
        ("account referrals list", "read_only", false),
        ("account referrals create", "non_idempotent", false),
        ("account referrals update", "idempotent", false),
        ("account referrals analytics", "read_only", false),
        ("billing balance", "read_only", false),
        ("billing subscriptions", "read_only", false),
        ("billing transactions", "read_only", false),
        ("billing stripe", "read_only", false),
        ("billing summary", "read_only", false),
        ("billing usage", "read_only", false),
        ("billing invoices list", "read_only", false),
        ("billing invoices get", "idempotent", false),
        ("billing auto-reload get", "read_only", false),
        ("billing auto-reload history", "read_only", false),
        ("billing auto-reload update", "idempotent", true),
        ("billing auto-reload card-setup", "non_idempotent", false),
        ("billing profile get", "read_only", false),
        ("billing profile update", "idempotent", true),
        ("billing native-quote", "read_only", false),
        ("billing gamma get", "read_only", false),
        ("billing gamma hotkeys create", "non_idempotent", false),
        ("billing gamma hotkeys delete", "idempotent", true),
        ("billing gamma conversions create", "non_idempotent", true),
        ("billing gamma conversions list", "read_only", false),
        ("billing gamma conversions get", "read_only", false),
        ("billing top-up", "non_idempotent", true),
        ("drive plans", "read_only", false),
        ("drive subscription get", "read_only", false),
        ("drive subscription subscribe", "non_idempotent", true),
        ("drive subscription change", "non_idempotent", true),
        ("drive subscription cancel", "idempotent", true),
        ("drive subscription checkout", "read_only", false),
        ("drive subscription history", "read_only", false),
        ("s3 plans", "read_only", false),
        ("s3 subscription get", "read_only", false),
        ("s3 subscription subscribe", "non_idempotent", true),
        ("s3 subscription change", "non_idempotent", true),
        ("s3 subscription cancel", "idempotent", true),
        ("s3 subscription checkout", "read_only", false),
        ("s3 subscription history", "read_only", false),
        ("s3 buckets list", "read_only", false),
        ("s3 buckets acl list", "read_only", false),
        ("s3 buckets acl get", "read_only", false),
        ("s3 buckets acl update", "idempotent", true),
        ("s3 buckets objects list", "read_only", false),
        ("s3 buckets objects presign", "read_only", false),
        ("s3 buckets create", "non_idempotent", true),
        ("s3 buckets delete", "idempotent", true),
        ("s3 tokens list", "read_only", false),
        ("s3 tokens create", "non_idempotent", true),
        ("s3 tokens revoke", "idempotent", true),
        ("s3 tokens rotate", "non_idempotent", true),
        ("s3 configure", "idempotent", true),
        ("hub plans", "read_only", false),
        ("hub subscription get", "read_only", false),
        ("hub subscription subscribe", "non_idempotent", true),
        ("hub subscription cancel", "idempotent", true),
        ("hub namespace availability", "read_only", false),
        ("hub namespace create", "non_idempotent", true),
        ("hub namespace status", "read_only", false),
        ("hub namespace get", "read_only", false),
        ("hub namespace update", "idempotent", true),
        ("hub namespace robot get", "read_only", false),
        ("hub namespace robot rotate", "non_idempotent", true),
        ("hub usage", "read_only", false),
        ("hub keys list", "read_only", false),
        ("hub keys get", "read_only", false),
        ("hub keys create", "non_idempotent", true),
        ("hub keys rotate", "non_idempotent", true),
        ("hub keys delete", "idempotent", true),
        ("hub repositories list", "read_only", false),
        ("hub repositories get", "read_only", false),
        ("hub repositories delete", "idempotent", true),
        ("hub artifacts list", "read_only", false),
        ("hub artifacts get", "read_only", false),
        ("hub artifacts delete", "idempotent", true),
        ("hub events list", "read_only", false),
        ("hub models list", "read_only", false),
        ("hub models formats", "read_only", false),
        ("hub models get", "read_only", false),
        ("compute prices", "read_only", false),
        ("compute public-prices", "read_only", false),
        ("compute quota", "read_only", false),
        ("compute charge-cap", "read_only", false),
        ("compute flavors", "read_only", false),
        ("compute images", "read_only", false),
        ("compute apps", "read_only", false),
        ("compute app-engine get", "read_only", false),
        ("compute app-engine validate", "read_only", false),
        ("compute app-engine fix", "idempotent", false),
        ("compute app-engine apps list", "read_only", false),
        ("compute app-engine apps get", "read_only", false),
        ("compute app-engine apps create", "non_idempotent", true),
        ("compute app-engine apps runtime", "read_only", false),
        (
            "compute app-engine guest-components get",
            "read_only",
            false,
        ),
        (
            "compute app-engine guest-components update-status",
            "read_only",
            false,
        ),
        ("compute regions", "read_only", false),
        ("compute placement-groups", "read_only", false),
        ("compute feasibility", "read_only", false),
        ("compute vms list", "read_only", false),
        ("compute vms get", "read_only", false),
        ("compute vms attestation", "read_only", false),
        ("compute vms failovers", "read_only", false),
        ("compute vms create", "non_idempotent", true),
        ("compute vms start", "idempotent", false),
        ("compute vms stop", "idempotent", true),
        ("compute vms reboot", "non_idempotent", true),
        ("compute vms resize-options", "read_only", false),
        ("compute vms resize", "non_idempotent", true),
        ("compute vms resize-status", "read_only", false),
        ("compute vms power-policy get", "read_only", false),
        ("compute vms power-policy update", "idempotent", false),
        ("compute vms delete", "idempotent", true),
        ("compute public-ip availability", "read_only", false),
        ("compute public-ip get", "read_only", false),
        ("compute public-ip create", "idempotent", true),
        ("compute public-ip delete", "idempotent", true),
        ("compute public-ip usage", "read_only", false),
        ("compute firewall get", "read_only", false),
        ("compute firewall update", "idempotent", true),
        ("compute exposures list", "read_only", false),
        ("compute exposures create", "idempotent", true),
        ("compute exposures delete", "idempotent", true),
        ("compute backups options", "read_only", false),
        ("compute backups policy get", "read_only", false),
        ("compute backups policy update", "idempotent", true),
        ("compute backups policy delete", "idempotent", true),
        ("compute backups list", "read_only", false),
        ("compute backups restore", "non_idempotent", true),
        ("compute backups restore-status", "read_only", false),
        ("compute runners settings get", "read_only", false),
        ("compute runners settings update", "idempotent", true),
        ("compute runners installations list", "read_only", false),
        ("compute runners installations delete", "idempotent", true),
        ("compute runners jobs", "read_only", false),
        ("compute runners connect", "non_idempotent", false),
        ("compute runners usage", "read_only", false),
        ("compute runners limit get", "read_only", false),
        ("compute runners limit update", "idempotent", true),
        ("compute ssh-keys list", "read_only", false),
        ("compute ssh-keys get", "read_only", false),
        ("compute ssh-keys create", "non_idempotent", true),
        ("compute ssh-keys delete", "idempotent", true),
        ("compute account-ssh-keys", "read_only", false),
        ("compute databases list", "read_only", false),
        ("compute databases get", "read_only", false),
        ("compute databases create", "non_idempotent", true),
        ("compute databases delete", "idempotent", true),
        ("compute databases restart", "non_idempotent", true),
        ("compute databases restore", "non_idempotent", true),
        ("compute databases exposure create", "idempotent", true),
        ("compute databases exposure delete", "idempotent", true),
        ("compute databases events", "read_only", false),
        ("compute databases metrics", "read_only", false),
        ("compute databases refusals", "read_only", false),
        ("compute kubernetes clusters list", "read_only", false),
        ("compute kubernetes clusters get", "read_only", false),
        ("compute kubernetes clusters delete", "idempotent", true),
        ("compute kubernetes clusters events", "read_only", false),
        ("compute kubernetes clusters refusals", "read_only", false),
        (
            "compute kubernetes clusters kubeconfig",
            "idempotent",
            false,
        ),
        ("compute kubernetes backups list", "read_only", false),
        ("compute kubernetes node-pools list", "read_only", false),
        (
            "compute kubernetes node-pools create",
            "non_idempotent",
            false,
        ),
        (
            "compute kubernetes node-pools scale",
            "non_idempotent",
            true,
        ),
        ("compute kubernetes operations list", "read_only", false),
        ("compute kubernetes operations cancel", "idempotent", true),
        ("compute kubernetes operations resume", "idempotent", true),
        ("compute kubernetes nodes delete", "idempotent", true),
        ("compute kubernetes nodes replace", "non_idempotent", true),
        ("cdn prices", "read_only", false),
        ("cdn regions", "read_only", false),
        ("cdn estimate", "read_only", false),
        ("cdn zones list", "read_only", false),
        ("cdn zones get", "read_only", false),
        ("cdn zones create", "non_idempotent", true),
        ("cdn zones update", "idempotent", true),
        ("cdn zones delete", "idempotent", true),
        ("cdn cap get", "read_only", false),
        ("cdn cap update", "idempotent", true),
        ("cdn hostnames list", "read_only", false),
        ("cdn hostnames create", "non_idempotent", true),
        ("cdn hostnames verify", "idempotent", false),
        ("cdn hostnames dns-setup", "idempotent", true),
        ("cdn hostnames domain-connect", "read_only", false),
        ("cdn hostnames delete", "idempotent", true),
        ("cdn purges create", "non_idempotent", false),
        ("cdn purges list", "read_only", false),
        ("cdn purges get", "read_only", false),
        ("cdn rules get", "read_only", false),
        ("cdn rules update", "idempotent", true),
        ("cdn stats", "read_only", false),
        ("cdn waf get", "read_only", false),
        ("cdn waf update", "idempotent", true),
        ("cdn waf status", "read_only", false),
        ("cdn waf analytics", "read_only", false),
        ("cdn waf events", "read_only", false),
        ("cdn waf test", "read_only", false),
        ("cdn waf rules get", "read_only", false),
        ("cdn waf rules update", "idempotent", true),
        ("cdn waf rate-limits get", "read_only", false),
        ("cdn waf rate-limits update", "idempotent", true),
        ("cdn waf versions list", "read_only", false),
        ("cdn waf versions get", "read_only", false),
        ("cdn waf versions rollback", "idempotent", true),
        ("cdn lists list", "read_only", false),
        ("cdn lists get", "read_only", false),
        ("cdn lists create", "non_idempotent", false),
        ("cdn lists update", "idempotent", false),
        ("cdn lists delete", "idempotent", true),
        ("cdn lists items create", "idempotent", true),
        ("cdn lists items update", "idempotent", true),
        ("cdn lists items delete", "idempotent", true),
        ("support tickets list", "read_only", false),
        ("support tickets get", "read_only", false),
        ("support tickets create", "non_idempotent", true),
        ("support tickets reply", "non_idempotent", false),
        ("support tickets update", "idempotent", true),
        ("support tickets messages list", "read_only", false),
        ("network get", "idempotent", true),
        ("network create", "idempotent", true),
        ("network peers list", "read_only", false),
        ("network peers get", "read_only", false),
        ("network peers update", "idempotent", true),
        ("network peers delete", "idempotent", true),
        ("network groups list", "read_only", false),
        ("network policies list", "read_only", false),
        ("network vpcs list", "read_only", false),
        ("network vpcs resources list", "read_only", false),
        ("network vpcs routers list", "read_only", false),
        ("api", "non_idempotent", true),
    ];

    /// Twelve commands ask only sometimes, and the schema says when: `confirmation` is true
    /// and `confirmation_when` names the condition, so an agent passes `--yes` when it
    /// holds. Every other command asks always or never.
    #[test]
    fn a_conditional_confirmation_says_when() {
        let conditional: Vec<(String, &str)> = command_paths(&Cli::command())
            .iter()
            .filter_map(|path| {
                let parts: Vec<&str> = path.iter().map(String::as_str).collect();
                match spec(&parts)?.confirmation {
                    Confirmation::When(when) => Some((path.join(" "), when)),
                    Confirmation::Never | Confirmation::Always => None,
                }
            })
            .collect();
        assert_eq!(
            conditional,
            [
                (
                    "auth login".to_owned(),
                    "the API origin is not `https://api.hippius.com`, or the profile stores a \
                     different token"
                ),
                (
                    "account notifications update".to_owned(),
                    "`--email-enabled`, `--low-credit-alerts`, `--zero-balance-alerts`, or \
                     `--file-status-updates` is `false`"
                ),
                (
                    "account invites preview".to_owned(),
                    "the API origin is not `https://api.hippius.com`"
                ),
                (
                    "billing auto-reload update".to_owned(),
                    "the update leaves auto-reload on, and the setting it reads first is off, \
                     is in a state other than `armed`, has a lower \
                     `threshold`, `target`, or `monthly_cap` (`null` is no cap, the highest), \
                     or holds one of those in a shape hipc cannot compare"
                ),
                (
                    "billing profile update".to_owned(),
                    "`--billing-email` is given and differs from the `billing_email` of the \
                     billing profile it reads first; a profile that is not found, or holds no \
                     email, differs from every email but an empty one"
                ),
                ("s3 configure".to_owned(), "the AWS profile already exists"),
                (
                    "compute runners settings update".to_owned(),
                    "against the settings it reads first, `--enabled` or `--allow-public-repos` \
                     is `true` where the setting is not, `--max-concurrent` is not `0` and is \
                     higher than `max_concurrent`, or an `--allowed-size` is not in \
                     `allowed_sizes`; a setting hipc cannot compare counts as higher"
                ),
                (
                    "compute runners limit update".to_owned(),
                    "`--monthly-cap` is higher than the limit's `monthly_cap_credits`, or is \
                     `none` while that is set, or the limit has no decimal amount there; `none` \
                     asks nothing while `monthly_cap` is `null` and `monthly_cap_credits` is \
                     `null` or missing"
                ),
                (
                    "cdn cap update".to_owned(),
                    "`--monthly-cap` is higher than the cap's `monthly_cap` in base units, or, \
                     when that is not a digit string, its `monthly_cap_credits`, or hipc cannot \
                     compare the two"
                ),
                ("cdn waf rules update".to_owned(), WAF_CHECK),
                ("cdn waf rate-limits update".to_owned(), WAF_CHECK),
                (
                    "api".to_owned(),
                    "the method is not `GET` or `HEAD`, or the path is `/api/network/me/`, \
                     whose first call creates the network, with or without the trailing \
                     slash or a format suffix"
                ),
            ]
        );
        let api = schema(&[]).unwrap()["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["path"] == json!(["api"]))
            .cloned()
            .unwrap();
        assert_eq!(api["confirmation"], true);
        assert_eq!(
            api["confirmation_when"],
            "the method is not `GET` or `HEAD`, or the path is `/api/network/me/`, whose \
             first call creates the network, with or without the trailing slash or a format \
             suffix"
        );
    }

    #[test]
    fn rows_match_the_plan_tables() {
        let paths: Vec<String> = command_paths(&Cli::command())
            .iter()
            .map(|p| p.join(" "))
            .collect();
        let listed: Vec<&str> = PLAN_ROWS.iter().map(|(p, ..)| *p).collect();
        assert_eq!(paths, listed);
        for (path, effect, confirmation) in PLAN_ROWS {
            let parts: Vec<&str> = path.split(' ').collect();
            let row = spec(&parts).unwrap();
            assert_eq!(row.effect.as_str(), effect, "{path}");
            assert_eq!(
                row.confirmation != Confirmation::Never,
                confirmation,
                "{path}"
            );
        }
    }

    /// The `reveal` and `pagination` bits come from the flags each command takes, so a row
    /// cannot go stale when a flag is added or removed: `--reveal` is what `reveal` asks
    /// for, and a list pages with `--page-token`, or is `hipc api`, which has `--all` and no
    /// `--page-token`.
    #[test]
    fn reveal_and_pagination_follow_the_flags_each_command_takes() {
        let root = Cli::command();
        for path in command_paths(&root) {
            let leaf = leaf(&root, &path);
            let takes = |id: &str| leaf.get_arguments().any(|arg| arg.get_id() == id);
            let row = row(&path);
            assert_eq!(row.reveal != Reveal::Never, takes("reveal"), "{path:?}");
            let pages = takes("page_token") || path == ["api"];
            assert_eq!(row.pagination, pages, "{path:?}");
        }
    }

    /// `--all` means one thing to an agent reading `hipc schema`, "follow every page" (tenets
    /// 3 and 6), so exactly the paged lists and `hipc api` take it; a purge of a whole zone is
    /// `--everything`. A unit goes in the value name (`--storage-cap <BYTES>`, `--traffic
    /// REGION=GB`), never in the flag name, global flags included: no flag is named a unit or
    /// ends in one.
    #[test]
    fn all_means_every_page_and_no_flag_name_carries_a_unit() {
        const UNITS: [&str; 10] = [
            "bytes", "gb", "mb", "kb", "days", "hours", "minutes", "seconds", "secs", "ms",
        ];
        let root = Cli::command();
        let unit = |arg: &clap::Arg| {
            arg.get_long().is_some_and(|long| {
                UNITS.iter().any(|u| {
                    long == *u || long.strip_suffix(u).is_some_and(|rest| rest.ends_with('-'))
                })
            })
        };
        let global: Vec<&str> = root
            .get_arguments()
            .filter(|a| unit(a))
            .filter_map(clap::Arg::get_long)
            .collect();
        assert!(
            global.is_empty(),
            "a global flag names its unit: {global:?}"
        );
        for path in command_paths(&root) {
            let leaf = leaf(&root, &path);
            let all = leaf
                .get_arguments()
                .any(|arg| arg.get_long() == Some("all"));
            assert_eq!(
                all,
                row(&path).pagination,
                "{path:?}: `--all` must go with pages"
            );
            let named: Vec<&str> = leaf
                .get_arguments()
                .filter(|a| unit(a))
                .filter_map(clap::Arg::get_long)
                .collect();
            assert!(
                named.is_empty(),
                "{path:?}: a flag names its unit: {named:?}"
            );
        }
    }

    /// A command's help says `needs --yes` exactly when the schema says it is confirmed, so
    /// a person reading `--help` learns what an agent reads from `hipc schema`.
    #[test]
    fn the_help_says_needs_yes_exactly_when_the_command_is_confirmed() {
        let root = Cli::command();
        let wrong: Vec<String> = command_paths(&root)
            .into_iter()
            .filter(|path| {
                let leaf = leaf(&root, path);
                let help = [leaf.get_about(), leaf.get_long_about()]
                    .into_iter()
                    .flatten()
                    .map(|text| text.to_string().to_lowercase())
                    .collect::<Vec<_>>()
                    .join(" ");
                let confirmed = row(path).confirmation != Confirmation::Never;
                help.contains("needs --yes") != confirmed
            })
            .map(|path| path.join(" "))
            .collect();
        assert!(
            wrong.is_empty(),
            "the help and the schema disagree: {wrong:?}"
        );
        // The global flag points at that marker rather than listing kinds of command, a list
        // that left out reboot, resize, and the list replaces.
        let yes = root
            .get_arguments()
            .find(|arg| arg.get_id() == "yes")
            .and_then(clap::Arg::get_help)
            .map(ToString::to_string);
        assert_eq!(
            yes.as_deref(),
            Some("Confirm a command whose help says it needs --yes")
        );
    }

    /// A guard's flag is one the command takes, and a command that takes `--remove-others`,
    /// `--public`, `--replace`, or `--change-isolation` lists the guard that flag gets past,
    /// so a row cannot go stale.
    #[test]
    fn guards_follow_the_flags_each_command_takes() {
        let root = Cli::command();
        for path in command_paths(&root) {
            let leaf = leaf(&root, &path);
            let takes = |id: &str| leaf.get_arguments().any(|arg| arg.get_id() == id);
            let row = row(&path);
            let guarded = |code| row.guards.iter().any(|guard| guard.code == code);
            // The schema and the hint spell the flag from the code; the command reads it by
            // its clap name, so this ties the two.
            for guard in row.guards {
                let flag = guard.code.flag().unwrap();
                let long = flag.strip_prefix("--").unwrap();
                let takes = leaf.get_arguments().any(|arg| arg.get_long() == Some(long));
                assert!(takes, "{path:?} {flag}");
            }
            assert_eq!(
                guarded(Code::RemoveOthersRequired),
                takes("remove_others"),
                "{path:?}"
            );
            assert_eq!(guarded(Code::PublicRequired), takes("public"), "{path:?}");
            assert_eq!(guarded(Code::ReplaceRequired), takes("replace"), "{path:?}");
            assert_eq!(
                guarded(Code::ChangeIsolationRequired),
                takes("change_isolation"),
                "{path:?}"
            );
        }
    }

    /// `hipc api` needs `--reveal` only on the routes that return a secret, and
    /// the schema says so, as it says when the command is confirmed.
    #[test]
    fn a_conditional_reveal_says_when() {
        let reveal_when: Vec<(String, &str)> = command_paths(&Cli::command())
            .iter()
            .filter_map(|path| {
                let parts: Vec<&str> = path.iter().map(String::as_str).collect();
                match spec(&parts)?.reveal {
                    Reveal::When(when) => Some((path.join(" "), when)),
                    Reveal::Never | Reveal::Always => None,
                }
            })
            .collect();
        let when = "the route answers with a secret";
        let mut expected = vec![("api".to_owned(), when)];
        for path in [
            "s3 tokens create",
            "s3 tokens rotate",
            "hub keys create",
            "hub keys rotate",
            "hub namespace create",
            "hub namespace robot rotate",
        ] {
            expected.push((path.to_owned(), NO_SECRET_FILE));
        }
        for path in ["compute databases create", "compute databases restore"] {
            expected.push((path.to_owned(), NO_CREDENTIALS_DIR));
        }
        let mut reveal_when = reveal_when;
        reveal_when.sort();
        expected.sort();
        assert_eq!(reveal_when, expected);
        let commands = schema(&[]).unwrap()["commands"].as_array().unwrap().clone();
        let api = commands
            .iter()
            .find(|c| c["path"] == json!(["api"]))
            .unwrap();
        assert_eq!(api["reveal"], true);
        assert_eq!(api["reveal_when"], when);
        let tokens = commands
            .iter()
            .find(|c| c["path"] == json!(["s3", "tokens", "create"]))
            .unwrap();
        assert_eq!(tokens["reveal"], true);
        assert_eq!(tokens["reveal_when"], NO_SECRET_FILE);
        let presign = commands
            .iter()
            .find(|c| c["path"] == json!(["s3", "buckets", "objects", "presign"]))
            .unwrap();
        assert_eq!(presign["reveal"], true);
        assert_eq!(presign["reveal_when"], Value::Null);
    }

    /// Only `hipc api` and the two support writes, which refuse a text that holds a token, have
    /// a refusal whatever the flags, and `auth login` one that `--new-account` lifts; `hipc api`'s guards are the flags of every
    /// `RouteFlag`, each with the code it refuses with, and the schema prints them as
    /// `guards`, with no separate `route_flags`.
    #[test]
    fn the_api_route_flags_are_its_guards() {
        let root = Cli::command();
        for path in command_paths(&root) {
            let support_write = path.len() == 3
                && path[..2] == ["support", "tickets"]
                && ["create", "reply"].contains(&path[2].as_str());
            assert_eq!(
                row(&path).refused_when.is_some(),
                path == ["api"] || path == ["auth", "login"] || support_write,
                "{path:?}"
            );
        }
        assert_eq!(
            row(&["support".into(), "tickets".into(), "reply".into()]).refused_when,
            Some(TICKET_HOLDS_A_TOKEN)
        );
        let route_flags = hippius::RouteFlag::ALL.map(|flag| (flag.code(), flag.flag()));
        let guards: Vec<(Code, &str)> = API_GUARDS
            .iter()
            .map(|guard| (guard.code, guard.code.flag().unwrap()))
            .collect();
        assert_eq!(guards, route_flags);
        let commands = schema(&[]).unwrap()["commands"].as_array().unwrap().clone();
        assert!(commands.iter().all(|c| c.get("route_flags").is_none()));
        let api = commands
            .iter()
            .find(|c| c["path"] == json!(["api"]))
            .unwrap();
        let flags: Vec<(&str, &str)> = api["guards"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| (row["flag"].as_str().unwrap(), row["code"].as_str().unwrap()))
            .collect();
        assert_eq!(
            flags,
            [
                ("--change-isolation", "change_isolation_required"),
                ("--public", "public_required"),
                ("--remove-others", "remove_others_required"),
            ]
        );
        assert_eq!(
            api["guards"][0]["when"],
            "the request writes to `/api/network/isolation/` (any method but `GET` and `HEAD`)"
        );
        assert_eq!(
            api["refused_when"],
            "the target is a full URL off the configured API origin, or the request is `POST \
             /api/account/deletion/`, any method of `/api/auth/token/`, `GET` or `HEAD` of \
             `/api/kubernetes/clusters/<cluster_id>/secrets/`, a `POST`, `PUT`, or `PATCH` of \
             `/api/cdn/zones/<zone_id>/hostnames/<hostname>/dns-setup/`, `POST \
             /api/registry/subscribe/`, or a `POST /api/compute/app-engine/apps/`, a write of \
             `/api/compute/app-engine/apps/<vm_id>/compose/`, or a `POST \
             /api/compute/app-engine/validate/` whose body sets `secrets` (a body that is not a \
             JSON object counts as setting it), or a `POST`, `PUT`, `PATCH`, or `DELETE` under \
             `/api/support/` whose body holds `HIPPIUS_TOKEN` or a token a profile of the config \
             file stores, as it is, quoted, percent-encoded, behind a JSON escape, or split by \
             whitespace, or an access key whose checksum holds, with or without the trailing slash \
             or a format suffix"
        );
        for noun in ["repositories", "artifacts"] {
            let delete = commands
                .iter()
                .find(|c| c["path"] == json!(["hub", noun, "delete"]))
                .unwrap();
            assert_eq!(delete["guards"], json!([]), "{noun}");
            assert_eq!(delete["confirmation"], true, "{noun}");
        }
    }

    /// The `args` and `flags` entry of `name` on the command at `path`.
    fn arg_of(path: &[&str], name: &str) -> Value {
        let doc = schema(&[]).unwrap();
        let command = doc["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["path"] == json!(path))
            .unwrap_or_else(|| panic!("{path:?} is in the schema"))
            .clone();
        command["args"]
            .as_array()
            .unwrap()
            .iter()
            .chain(command["flags"].as_array().unwrap())
            .find(|entry| entry["name"] == name)
            .unwrap_or_else(|| panic!("{path:?} takes {name}"))
            .clone()
    }

    /// The short help clap gives the argument `id` of the command at `path`. The tests
    /// compare with it, not with the text, so a help edit in `cli/` does not break them.
    fn clap_help(path: &[&str], id: &str) -> String {
        let root = Cli::command();
        let path: Vec<String> = path.iter().map(|word| (*word).to_owned()).collect();
        let arg = leaf(&root, &path)
            .get_arguments()
            .find(|arg| arg.get_id() == id)
            .unwrap_or_else(|| panic!("{path:?} takes {id}"));
        arg.get_help()
            .expect("every argument has a help")
            .to_string()
    }

    /// A positional argument names its resource (`cluster_id`, `bucket_name`), so its name in
    /// the schema, the usage line, and an `invalid value` message says what to pass. A bare
    /// `id`, `name`, or `number` once stood for fourteen different resources, and `op_id` was
    /// the one abbreviation. The parser is walked rather than the schema, so a hidden argument
    /// counts, and the value name is checked as well as the id, so an explicit
    /// `value_name = "ID"` cannot undo the rename in usage lines.
    #[test]
    fn no_positional_argument_is_a_bare_id_name_or_number() {
        const BARE: [&str; 6] = ["id", "name", "number", "num", "op", "op_id"];
        let bare_word = |word: &str| BARE.contains(&word.to_lowercase().as_str());
        let root = Cli::command();
        let bare: Vec<String> = command_paths(&root)
            .into_iter()
            .flat_map(|path| {
                let command = leaf(&root, &path);
                let path = path.join(" ");
                command
                    .get_arguments()
                    .filter(|arg| arg.is_positional())
                    .filter_map(move |arg| {
                        let id = arg.get_id().as_str();
                        let value_names = arg.get_value_names().unwrap_or_default();
                        let value_names: Vec<&str> =
                            value_names.iter().map(clap::builder::Str::as_str).collect();
                        let shown = format!("{path} {id} {value_names:?}");
                        (bare_word(id) || value_names.iter().any(|v| bare_word(v))).then_some(shown)
                    })
            })
            .collect();
        assert_eq!(bare, Vec::<String>::new());
    }

    /// Each entry carries the name, value name, required and repeat bits, possible values,
    /// default, and short help of its clap argument.
    #[test]
    fn each_argument_says_how_it_is_passed() {
        assert_eq!(
            arg_of(&["compute", "vms", "get"], "vm_id"),
            json!({
                "name": "vm_id",
                "value_name": "VM_ID",
                "required": true,
                "repeats": false,
                "possible_values": [],
                "default": null,
                "help": clap_help(&["compute", "vms", "get"], "vm_id"),
            })
        );
        // Neither key flag is required alone; the command's `required_groups` lists the pair.
        let key = arg_of(&["compute", "vms", "create"], "--ssh-key-id");
        assert_eq!(
            (&key["value_name"], &key["repeats"]),
            (&json!("ID"), &json!(true))
        );
        assert_eq!(key["required"], false);
        let flavor = arg_of(&["compute", "vms", "create"], "--flavor");
        assert_eq!(
            (&flavor["required"], &flavor["repeats"]),
            (&json!(true), &json!(false))
        );
        let tier = arg_of(&["compute", "databases", "create"], "--tier");
        assert_eq!(
            tier["possible_values"],
            json!(["starter", "production", "ha"])
        );
        assert_eq!(tier["default"], Value::Null);
        let profile = arg_of(&["s3", "configure"], "--aws-profile");
        assert_eq!(profile["default"], "hippius");
        assert_eq!(
            profile["help"],
            clap_help(&["s3", "configure"], "aws_profile")
        );
    }

    /// A switch takes no value, so clap's `true` and `false` parser values and its `false`
    /// default are not listed: an agent passes the flag or leaves it out.
    #[test]
    fn a_switch_has_no_value_name_default_or_values() {
        assert_eq!(
            arg_of(&["api"], "--all"),
            json!({
                "name": "--all",
                "value_name": null,
                "required": false,
                "repeats": false,
                "possible_values": [],
                "default": null,
                "help": clap_help(&["api"], "all"),
            })
        );
        // A flag that takes `true` or `false` as a value lists them.
        let billing = arg_of(&["account", "members", "update"], "--billing-permission");
        assert_eq!(billing["possible_values"], json!(["true", "false"]));
        assert_eq!(billing["value_name"], "BOOL");
    }

    /// The global flags are listed once, at the top level, and no command repeats them.
    #[test]
    fn the_global_flags_are_listed_once_at_the_top() {
        let doc = schema(&[]).unwrap();
        let globals: Vec<&str> = doc["global_flags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|flag| flag["name"].as_str().unwrap())
            .collect();
        let root = Cli::command();
        let expected: Vec<String> = root
            .get_arguments()
            .map(|arg| format!("--{}", arg.get_long().unwrap()))
            .collect();
        assert_eq!(globals, expected);
        assert!(root.get_arguments().all(clap::Arg::is_global_set));
        for command in doc["commands"].as_array().unwrap() {
            for flag in command["flags"].as_array().unwrap() {
                let name = flag["name"].as_str().unwrap();
                assert!(!globals.contains(&name), "{command}");
            }
        }
        let global = |name: &str| {
            doc["global_flags"]
                .as_array()
                .unwrap()
                .iter()
                .find(|flag| flag["name"] == name)
                .unwrap()
                .clone()
        };
        let output = global("--output");
        assert_eq!(output["possible_values"], json!(["auto", "text", "json"]));
        assert_eq!(output["default"], "auto");
        assert_eq!(global("--request-timeout")["default"], "30");
    }

    /// A launch needs a key flag, a purge a target, `s3 configure` a key pair, a plan
    /// change one kind of change, and the billing profile, auto-reload, notification settings,
    /// and project quota updates at least one field, which no single flag says, so the schema
    /// lists each such set; no other command has one.
    #[test]
    fn a_set_of_which_one_is_required_is_listed() {
        let doc = schema(&[]).unwrap();
        let groups: Vec<(String, Value)> = doc["commands"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["required_groups"] != json!([]))
            .map(|c| (c["path"].to_string(), c["required_groups"].clone()))
            .collect();
        let change = json!([{
            "names": ["--plan", "--pay-from", "--storage-cap"],
            "multiple": false,
        }]);
        assert_eq!(
            groups,
            [
                (
                    json!(["account", "notifications", "update"]).to_string(),
                    json!([{
                        "names": [
                            "--email-enabled",
                            "--low-credit-alerts",
                            "--zero-balance-alerts",
                            "--file-status-updates",
                            "--marketing-emails",
                            "--provider-advice-emails",
                        ],
                        "multiple": true,
                    }])
                ),
                (
                    json!(["account", "projects", "quota", "update"]).to_string(),
                    json!([{
                        "names": [
                            "--vms",
                            "--databases",
                            "--clusters",
                            "--vcpus",
                            "--memory",
                            "--disk",
                            "--public-ips",
                            "--runners",
                        ],
                        "multiple": true,
                    }])
                ),
                (
                    json!(["billing", "auto-reload", "update"]).to_string(),
                    json!([{
                        "names": ["--enabled", "--threshold", "--target", "--monthly-cap"],
                        "multiple": true,
                    }])
                ),
                (
                    json!(["billing", "profile", "update"]).to_string(),
                    json!([{
                        "names": [
                            "--company-name",
                            "--address-line1",
                            "--address-line2",
                            "--city",
                            "--postal-code",
                            "--region",
                            "--country",
                            "--tax-id",
                            "--billing-email",
                        ],
                        "multiple": true,
                    }])
                ),
                (
                    json!(["drive", "subscription", "change"]).to_string(),
                    change.clone()
                ),
                (json!(["s3", "subscription", "change"]).to_string(), change),
                (
                    json!(["s3", "configure"]).to_string(),
                    json!([{"names": ["--from-token-file", "--access-key-id"], "multiple": false}])
                ),
                (
                    json!(["compute", "vms", "create"]).to_string(),
                    json!([{
                        "names": ["--ssh-key-id", "--ssh-public-key-file"],
                        "multiple": true,
                    }])
                ),
                (
                    json!(["cdn", "purges", "create"]).to_string(),
                    json!([{"names": ["--everything", "--path", "--prefix"], "multiple": false}])
                ),
            ]
        );
    }

    /// The schema lists the arguments of the leaves only, so no group may take one of its
    /// own; and it names a default as one string, so no argument may have two.
    #[test]
    fn groups_take_no_arguments_and_no_argument_has_two_defaults() {
        fn walk(command: &clap::Command, path: &str) {
            let defaults: Vec<usize> = command
                .get_arguments()
                .map(|arg| arg.get_default_values().len())
                .collect();
            assert!(defaults.iter().all(|&n| n <= 1), "{path}: {defaults:?}");
            for sub in command.get_subcommands() {
                if sub.has_subcommands() {
                    assert_eq!(sub.get_arguments().count(), 0, "{path} {}", sub.get_name());
                }
                walk(sub, &format!("{path} {}", sub.get_name()));
            }
        }
        walk(&Cli::command(), "hipc");
    }

    /// Hidden arguments and hidden values stay out of the schema, as they stay out of the
    /// help, and an argument that takes several values at once repeats.
    #[test]
    fn hidden_arguments_and_values_are_left_out() {
        let command = clap::Command::new("t")
            .arg(clap::Arg::new("secret").long("secret").hide(true))
            .arg(
                clap::Arg::new("mode")
                    .long("mode")
                    .value_parser([
                        clap::builder::PossibleValue::new("on"),
                        clap::builder::PossibleValue::new("legacy").hide(true),
                    ])
                    .num_args(1..),
            );
        assert_eq!(
            arg_entries(&command, |_| true),
            [json!({
                "name": "--mode",
                "value_name": null,
                "required": false,
                "repeats": true,
                "possible_values": ["on"],
                "default": null,
                "help": null,
            })]
        );
    }

    #[test]
    fn walk_reaches_nested_leaves_only() {
        let tree = clap::Command::new("t")
            .subcommand(
                clap::Command::new("s3").subcommand(
                    clap::Command::new("buckets")
                        .subcommand(clap::Command::new("list"))
                        .subcommand(clap::Command::new("create")),
                ),
            )
            .subcommand(clap::Command::new("version"));
        assert_eq!(
            command_paths(&tree),
            [
                vec!["s3", "buckets", "list"],
                vec!["s3", "buckets", "create"],
                vec!["version"],
            ]
        );
    }

    #[test]
    fn unknown_or_partial_paths_have_no_row() {
        assert!(spec(&["nope"]).is_none());
        assert!(spec(&[]).is_none());
        assert!(spec(&["version", "extra"]).is_none());
        assert!(spec(&["network"]).is_none());
        assert!(spec(&["network", "peers"]).is_none());
    }

    /// The window `HUB_REPLACE_WHEN` names is the one the Hub marker holds.
    #[test]
    fn the_hub_replace_window_matches_the_marker() {
        let window = hippius::phrase::count(crate::commands::hub::MARKER_SECONDS / 60, "minute");
        assert!(HUB_REPLACE_WHEN.contains(&format!("in the last {window}")));
    }

    #[test]
    fn distance_counts_edits() {
        assert_eq!(distance("", ""), 0);
        assert_eq!(distance("vm", "vms"), 1);
        assert_eq!(distance("vms", "vm"), 1);
        assert_eq!(distance("kitten", "sitting"), 3);
        assert_eq!(distance("", "abc"), 3);
        assert_eq!(distance("abc", "abc"), 0);
    }

    #[test]
    fn nearest_takes_a_close_name_or_a_prefix_only() {
        let names = ["apps", "app-engine", "api", "vms", "databases", "s3"];
        let name = |word: &str| nearest(word, &names).map(|(_, name)| name);
        assert_eq!(name("vm"), Some("vms"));
        // At one edit from both, the name that starts with the word wins.
        assert_eq!(name("app"), Some("apps"));
        assert_eq!(nearest("app", &names), Some(((1, false), "apps")));
        assert_eq!(name("app-engines"), Some("app-engine"));
        assert_eq!(name("databse"), Some("databases"));
        // One edit counts for a short name, and case does not.
        assert_eq!(name("s4"), Some("s3"));
        assert_eq!(name("VMS"), Some("vms"));
        assert_eq!(nearest("VMS", &names), Some(((0, false), "vms")));
        // One letter is no prefix to go on.
        assert_eq!(name("a"), None);
        assert_eq!(name("zzzz"), None);
        assert_eq!(name(""), None);
    }

    fn words(path: &[&str]) -> Vec<String> {
        path.iter().map(|w| (*w).to_owned()).collect()
    }

    #[test]
    fn resolve_names_each_command_by_its_own_name() {
        let root = clap::Command::new("hipc").subcommand(
            clap::Command::new("hub").subcommand(
                clap::Command::new("keys")
                    .alias("ci-keys")
                    .subcommand(clap::Command::new("list")),
            ),
        );
        assert_eq!(resolve(&root, &[]).unwrap(), Vec::<String>::new());
        assert_eq!(
            resolve(&root, &words(&["hub", "ci-keys", "list"])).unwrap(),
            ["hub", "keys", "list"]
        );
    }

    #[test]
    fn resolve_refuses_an_unknown_name_with_the_nearest_one() {
        let root = Cli::command();
        let refusal = |path: &[&str]| {
            let error = resolve(&root, &words(path)).unwrap_err();
            assert_eq!(error.code(), Code::Usage);
            (error.message().to_owned(), error.hint().unwrap().to_owned())
        };
        let (message, hint) = refusal(&["compute", "vm"]);
        assert!(
            message.starts_with("`hipc compute` has no command `vm`; it holds `prices`, "),
            "{message}"
        );
        assert!(message.contains("`app-engine`"), "{message}");
        assert!(message.ends_with(", and `kubernetes`"), "{message}");
        assert_eq!(hint, "run `hipc schema compute vms`");

        // Not near any name at its place, but the name of one deeper group.
        let (message, hint) = refusal(&["vms"]);
        assert!(
            message.starts_with("`hipc` has no command `vms`; it holds `version`, `status`,"),
            "{message}"
        );
        assert_eq!(hint, "run `hipc schema compute vms`");
        // A deeper name that starts with the word beats one at this place that does not.
        let (_, hint) = refusal(&["app"]);
        assert_eq!(
            hint,
            "run `hipc schema compute apps` or `hipc schema compute app-engine apps`"
        );
        // At the same nearness, the name at this place wins; case does not count.
        assert_eq!(refusal(&["COMPUTE"]).1, "run `hipc schema compute`");
        assert_eq!(refusal(&["s4"]).1, "run `hipc schema s3`");
        // A word that starts with a name is not that name.
        assert_eq!(
            refusal(&["compute vms"]).1,
            "run `hipc schema` with one of the commands it holds"
        );

        // A name that ends many paths suggests none of them.
        assert!(
            refusal(&[""])
                .0
                .starts_with("`hipc` has no command with an empty name; it holds `version`,")
        );
        for word in ["list", "zzzz", ""] {
            assert_eq!(
                refusal(&[word]).1,
                "run `hipc schema` with one of the commands it holds",
                "{word}"
            );
        }
        assert_eq!(
            refusal(&["compute", "zzzz"]).1,
            "run `hipc schema compute` with one of the commands it holds"
        );

        let (message, hint) = refusal(&["s3", "buckets", "create", "x"]);
        assert_eq!(
            message,
            "`hipc s3 buckets create` has no command `x`: it is a command, not a group"
        );
        assert_eq!(hint, "run `hipc schema s3 buckets create` for that command");
    }

    #[test]
    fn deeper_finds_a_few_paths_or_none() {
        let root = Cli::command();
        assert_eq!(
            deeper(&root, "app-engine"),
            Some(((0, false), vec![words(&["compute", "app-engine"])]))
        );
        assert_eq!(
            deeper(&root, "apps").map(|(_, found)| found),
            Some(vec![
                words(&["compute", "apps"]),
                words(&["compute", "app-engine", "apps"])
            ])
        );
        assert_eq!(deeper(&root, "list"), None);
        assert_eq!(deeper(&root, "zzzz"), None);
    }

    #[test]
    fn schema_with_a_prefix_keeps_every_other_field() {
        let whole = schema(&[]).unwrap();
        let part = schema(&words(&["compute", "app-engine"])).unwrap();
        for field in ["schema_version", "global_flags", "errors"] {
            assert_eq!(part[field], whole[field]);
        }
        let paths: Vec<&Value> = part["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| &c["path"])
            .collect();
        assert!(paths.len() > 1, "{paths:?}");
        assert!(
            paths
                .iter()
                .all(|p| p[0] == "compute" && p[1] == "app-engine")
        );
        // A prefix matches whole words: `apps` is not under `app`.
        assert!(schema(&words(&["compute", "app"])).is_err());
        // The `schema` row itself names its path argument.
        let own = schema(&words(&["schema"])).unwrap();
        assert_eq!(own["commands"][0]["args"][0]["name"], "path");
        assert_eq!(own["commands"][0]["args"][0]["repeats"], true);
    }

    #[test]
    fn schema_document_shape() {
        let doc = schema(&[]).unwrap();
        assert_eq!(doc["schema_version"], 3);
        assert_eq!(
            doc["commands"][0],
            json!({
                "path": ["version"],
                "effect": "read_only",
                "confirmation": false,
                "reveal": false,
                "pagination": false,
                "confirmation_when": null,
                "reveal_when": null,
                "refused_when": null,
                "guards": [],
                "args": [],
                "flags": [],
                "required_groups": [],
            })
        );
        let keys: Vec<&String> = doc.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            ["schema_version", "global_flags", "commands", "errors"]
        );
        let create = doc["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["path"] == json!(["cdn", "zones", "create"]))
            .cloned()
            .unwrap();
        assert_eq!(
            create["guards"],
            json!([{"code": "public_required", "flag": "--public", "when": null}])
        );
        let subscribe = doc["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["path"] == json!(["hub", "subscription", "subscribe"]))
            .cloned()
            .unwrap();
        assert_eq!(
            subscribe["guards"],
            json!([{"code": "replace_required", "flag": "--replace", "when": HUB_REPLACE_WHEN}])
        );
        let errors = doc["errors"].as_array().unwrap();
        assert_eq!(errors.len(), Code::ALL.len());
        assert_eq!(
            errors[1],
            json!({
                "code": "confirmation_required",
                "exit": 2,
                "meaning": "the command needs a confirmation, and `--yes` was not passed",
                "flag": "--yes",
            })
        );
        assert_eq!(errors[0]["flag"], Value::Null);
    }
}
