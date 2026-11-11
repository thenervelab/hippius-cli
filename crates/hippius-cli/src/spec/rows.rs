//! The schema row of each command, one function per command group, keyed by the path
//! after the group's name as the parser names it. `spec::tests::every_command_has_a_spec_row`
//! walks the parser and fails on a command with no row here, and the `hipc schema` snapshot
//! shows every row a change moves.
//!
//! Whether a command pages is not declared here: it pages exactly when it takes `--all`
//! (`spec::pages`).

use hippius::Code;

use super::{
    API_CONFIRMATION, API_GUARDS, API_REFUSED_WHEN, API_REVEAL_WHEN, Asks, CDN_CAP_RAISED, Effect,
    Guard, HUB_REPLACE_WHEN, NO_CREDENTIALS_DIR, NO_SECRET_FILE, NOT_DEFAULT_ORIGIN,
    NOTIFICATION_ALERT_OFF, READ_ONLY, RELOAD_CHARGES_MORE, RUNNER_CAP_RAISED,
    RUNNER_SETTINGS_SPEND_MORE, SPEND, Spec, TICKET_HOLDS_A_TOKEN, UNDO, UNGATED_WRITE, WAF_CHECK,
};

/// A write that spends nothing and is not gated, whose repeat does the work again.
const REPEATS: Spec = Spec {
    effect: Effect::NonIdempotent,
    ..READ_ONLY
};

/// A write the contract does not gate whose repeat is safe, confirmed all the same.
const CONFIRMED_UNGATED: Spec = Spec {
    confirmation: Asks::Always,
    ..UNGATED_WRITE
};

/// A write that answers with a secret the server shows once, which a file the caller names
/// for it asks for as plainly as `--reveal`, and keeps off stdout, so either flag lets the
/// request go.
const SPEND_TO_SECRET_FILE: Spec = Spec {
    reveal: Asks::When(NO_SECRET_FILE),
    ..SPEND
};

/// A guard that `--public` always gets past.
const PUBLIC_ALWAYS: Guard = Guard {
    code: Code::PublicRequired,
    when: None,
};

/// A guard that `--remove-others` always gets past.
const REMOVE_OTHERS_ALWAYS: Guard = Guard {
    code: Code::RemoveOthersRequired,
    when: None,
};

/// The rows of the commands outside the groups: none changes anything.
pub(super) fn of_top(path: &[&str]) -> Option<Spec> {
    match path {
        ["version" | "schema" | "completions" | "status"] => Some(READ_ONLY),
        _ => None,
    }
}

/// The rows of `hipc auth ...`, by the rest of the path.
pub(super) fn of_auth(path: &[&str]) -> Option<Spec> {
    match path {
        ["status"] => Some(READ_ONLY),
        // Replacing a different stored token is confirmed because later commands act as its
        // owner (tenet 8).
        ["login"] => Some(Spec {
            confirmation: Asks::When(
                "the API origin is not `https://api.hippius.com`, or the profile stores a \
                 different token",
            ),
            ..UNGATED_WRITE
        }),
        ["logout"] => Some(UNDO),
        _ => None,
    }
}

/// The rows of `hipc account ...`, by the rest of the path.
pub(super) fn of_account(path: &[&str]) -> Option<Spec> {
    match path {
        ["list" | "get"]
        | ["notifications" | "marketing-consent", "get"]
        | [
            "members" | "invites" | "projects" | "billing-accounts" | "project-transfers",
            "list",
        ]
        | ["billing-accounts", "costs"]
        | ["projects", "budget" | "quota", "get"]
        | ["referrals", "list" | "analytics"] => Some(READ_ONLY),
        // An invite preview is a POST that changes nothing. It sends the invite token, a
        // secret, so an origin but the default one is confirmed, as for a login (tenet 8).
        ["invites", "preview"] => Some(Spec {
            confirmation: Asks::When(NOT_DEFAULT_ORIGIN),
            ..READ_ONLY
        }),
        // The notification settings set the values given, so a repeat leaves them the same;
        // they spend nothing, but an alert turned off hides a low or empty balance.
        ["notifications", "update"] => Some(Spec {
            confirmation: Asks::When(NOTIFICATION_ALERT_OFF),
            ..UNGATED_WRITE
        }),
        ["use"]
        // A rename sends the name, so a repeat leaves the same name; it spends nothing.
        | ["projects" | "billing-accounts", "use" | "update"]
        // A referral rename changes a label stored on Hippius only; the code and slug stay.
        | ["referrals", "update"] => Some(UNGATED_WRITE),
        // A project or billing account is free (the free-create rule), so its create is not
        // gated; a repeat creates another. A referral link spends nothing: the server's
        // signer mints the user's code on first use and later links reuse it. A repeat of one
        // name is refused with 409.
        ["projects" | "billing-accounts" | "referrals", "create"]
        // A consent withdrawal spends nothing; swagger appends each one to the consent
        // history, so a repeat adds another entry.
        | ["marketing-consent", "update"] => Some(REPEATS),
        ["invites", "create"]
        // A repeat offers the project again. The project leaves the account once the other
        // account accepts, so it is confirmed.
        | ["projects", "transfer"] => Some(SPEND),
        // An invite is used once, as the console treats it (unverified against the API): a
        // repeat accept is refused and the caller stays a member.
        // Joining gives the token access to another account, so it is confirmed.
        ["invites", "revoke" | "accept"]
        // Swagger: a project delete archives an empty project, and a repeat finds it
        // archived.
        | ["members" | "projects", "delete"]
        // A budget PUT replaces the whole budget, a quota PATCH sets the limits given, and a
        // budget delete again finds none; each changes what the project may spend or run, so
        // each is confirmed.
        | ["projects", "budget", "update" | "delete"]
        | ["projects", "quota", "update"]
        // A transfer is decided once: a repeat accept, decline, or cancel is refused, and
        // the project stays where the first left it. An accept moves a project into the
        // token's own account, a decline ends an offer, and a cancel withdraws one.
        | ["project-transfers", "accept" | "decline" | "cancel"] => Some(UNDO),
        // A member update replaces the whole grant list, so a repeat leaves the same list.
        ["members", "update"] => Some(Spec {
            guards: &[Guard {
                code: Code::RemoveOthersRequired,
                when: Some("a `--grant` is given, and the grants leave out one the member has now"),
            }],
            ..UNDO
        }),
        _ => None,
    }
}

/// The rows of `hipc billing ...`, by the rest of the path.
pub(super) fn of_billing(path: &[&str]) -> Option<Spec> {
    match path {
        [
            "balance" | "subscriptions" | "transactions" | "stripe" | "summary" | "usage"
            | "native-quote",
        ]
        | ["invoices", "list"]
        | ["auto-reload", "get" | "history"]
        | ["gamma" | "profile", "get"]
        | ["gamma", "conversions", "get" | "list"] => Some(READ_ONLY),
        // An invoice download writes only a local file, and a repeat finds that file and stops.
        ["invoices", "get"] => Some(UNGATED_WRITE),
        // A profile update sends the fields given, so a repeat leaves the profile the same; it
        // spends nothing, but a new billing email sends the invoices elsewhere.
        ["profile", "update"] => Some(Spec {
            confirmation: Asks::When(
                "`--billing-email` is given and differs from the `billing_email` of the billing \
                 profile it reads first; a profile that is not found, or holds no email, \
                 differs from every email but an empty one",
            ),
            ..UNGATED_WRITE
        }),
        // A card setup charges nothing (swagger); a repeat starts another checkout.
        ["auto-reload", "card-setup"]
        // A hotkey link spends nothing; a repeat asks for, and spends, another challenge.
        | ["gamma", "hotkeys", "create"] => Some(REPEATS),
        // A conversion burns gamma on chain, which cannot be refunded; a repeat burns again.
        ["gamma", "conversions", "create"]
        // A repeat starts another checkout.
        | ["top-up"] => Some(SPEND),
        // An unlink leaves the gamma in its deposit; a repeat finds the hotkey unlinked (404).
        ["gamma", "hotkeys", "delete"] => Some(UNDO),
        // Auto-reload charges the card by itself, so a change that may charge more asks.
        ["auto-reload", "update"] => Some(Spec {
            confirmation: Asks::When(RELOAD_CHARGES_MORE),
            ..UNDO
        }),
        _ => None,
    }
}

/// The rows of `hipc drive ...` and of the plan commands `hipc s3 ...` shares with it, by
/// the rest of the path.
pub(super) fn of_drive(path: &[&str]) -> Option<Spec> {
    match path {
        ["plans"] | ["subscription", "get" | "checkout" | "history"] => Some(READ_ONLY),
        ["subscription", "subscribe" | "change"] => Some(SPEND),
        ["subscription", "cancel"] => Some(UNDO),
        _ => None,
    }
}

/// The rows of `hipc s3 ...`, by the rest of the path.
pub(super) fn of_s3(path: &[&str]) -> Option<Spec> {
    match path {
        ["plans"] | ["subscription", ..] => of_drive(path),
        ["buckets" | "tokens", "list"]
        | ["buckets", "acl", "list" | "get"]
        | ["buckets", "objects", "list"] => Some(READ_ONLY),
        // A presigned URL reads the object with no other credential until it expires.
        ["buckets", "objects", "presign"] => Some(Spec {
            reveal: Asks::Always,
            ..READ_ONLY
        }),
        // Whether the bucket is private is known only from the answer, after the write.
        ["buckets", "create"] => Some(SPEND),
        ["buckets", "delete"] | ["tokens", "revoke"] => Some(UNDO),
        ["tokens", "create" | "rotate"] => Some(SPEND_TO_SECRET_FILE),
        // A policy replaces the whole bucket policy, so a repeat leaves the same policy. Its
        // statements are not judged, so both guards always apply (`commands/s3.rs`).
        ["buckets", "acl", "update"] => Some(Spec {
            guards: &[PUBLIC_ALWAYS, REMOVE_OTHERS_ALWAYS],
            ..UNDO
        }),
        // Writing a new profile replaces nothing, so only an existing one is confirmed.
        ["configure"] => Some(Spec {
            confirmation: Asks::When("the AWS profile already exists"),
            ..UNDO
        }),
        _ => None,
    }
}

/// The rows of `hipc hub ...`, by the rest of the path.
pub(super) fn of_hub(path: &[&str]) -> Option<Spec> {
    match path {
        ["plans" | "usage"]
        | ["subscription", "get"]
        | ["namespace", "availability" | "status" | "get"]
        | ["namespace", "robot", "get"]
        | ["keys" | "repositories" | "artifacts", "list" | "get"]
        | ["events", "list"]
        | ["models", "formats" | "list" | "get"] => Some(READ_ONLY),
        // Hub lets an account hold a plan twice, so a second subscribe charges twice
        // (tenet 11).
        ["subscription", "subscribe"] => Some(Spec {
            guards: &[Guard {
                code: Code::ReplaceRequired,
                when: Some(HUB_REPLACE_WHEN),
            }],
            ..SPEND
        }),
        // A repeat leaves the namespace as one call did. Opening it to anonymous pulls needs
        // `--public` (tenet 5); closing it does not.
        ["namespace", "update"] => Some(Spec {
            guards: &[Guard {
                code: Code::PublicRequired,
                when: Some("`--visibility` is `public`"),
            }],
            ..UNDO
        }),
        ["subscription", "cancel"]
        // A repeat delete of a Hub key, repository, or artifact finds it gone. Hub data
        // deleted there is gone for good, which the question says; `--yes` is the one
        // confirmation (tenet 5).
        | ["keys" | "repositories" | "artifacts", "delete"] => Some(UNDO),
        ["keys", "create" | "rotate"] | ["namespace", "create"] | ["namespace", "robot", "rotate"] => {
            Some(SPEND_TO_SECRET_FILE)
        }
        _ => None,
    }
}

/// The rows of `hipc compute ...` outside its own groups, by the rest of the path.
pub(super) fn of_compute(path: &[&str]) -> Option<Spec> {
    match path {
        [
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
            "vms",
            "list" | "get" | "attestation" | "failovers" | "resize-options" | "resize-status",
        ]
        | ["vms", "power-policy", "get"]
        | ["public-ip", "availability" | "get" | "usage"]
        | ["firewall" | "ssh-keys", "get"]
        | ["ssh-keys" | "exposures", "list"]
        | ["backups", "options" | "list" | "restore-status"]
        | ["backups", "policy", "get"] => Some(READ_ONLY),
        // A stop takes down what the VM serves; a repeat leaves it stopped (tenet 7).
        ["vms", "stop"] => Some(CONFIRMED_UNGATED),
        // A repeat of a VM power change leaves the power state as one call did (tenet 7).
        ["vms", "start"]
        // A power policy sets one value, so a repeat leaves the same policy. Either value
        // spends nothing and is undone by setting the other, and it decides only whether a
        // VM shut down from inside is started again, which `start` does unasked.
        | ["vms", "power-policy", "update"] => Some(UNGATED_WRITE),
        ["vms", "create" | "reboot" | "resize"]
        // A repeat restores again once the first is done, losing what was written since.
        | ["backups", "restore"]
        | ["ssh-keys", "create"] => Some(SPEND),
        ["vms" | "ssh-keys", "delete"]
        // Swagger: a policy PUT sets the interval and the DELETE is "Idempotent". Stored
        // backups are billed, and turning them off ends new ones, so both are confirmed.
        | ["backups", "policy", "update" | "delete"]
        // Swagger: attaching again returns the address already attached, and exposing a port
        // and protocol again returns the exposure that exists, so a repeat is safe. An
        // attach starts a bill and an exposure opens a port to the internet, so both are
        // confirmed.
        | ["public-ip" | "exposures", "create" | "delete"] => Some(UNDO),
        // A rules file replaces the whole list, so a repeat leaves the same list.
        ["firewall", "update"] => Some(Spec {
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
        _ => None,
    }
}

/// The rows of `hipc compute app-engine ...`, by the rest of the path.
pub(super) fn of_compute_app_engine(path: &[&str]) -> Option<Spec> {
    match path {
        // `validate`, a compose check, is a POST that changes nothing.
        ["get" | "validate"]
        | ["apps", "list" | "get" | "runtime"]
        | ["guest-components", "get" | "update-status"] => Some(READ_ONLY),
        // A compose fix saves nothing on the server, but `--output-file` writes a new local
        // file, and a repeat finds that file and stops.
        ["fix"] => Some(UNGATED_WRITE),
        // An app is a VM with a public IPv4, so a repeat launches, and bills, a second one.
        ["apps", "create"] => Some(SPEND),
        _ => None,
    }
}

/// The rows of `hipc compute runners ...`, by the rest of the path.
pub(super) fn of_compute_runners(path: &[&str]) -> Option<Spec> {
    match path {
        ["settings" | "limit", "get"] | ["installations", "list"] | ["usage" | "jobs"] => {
            Some(READ_ONLY)
        }
        // A runner connect spends nothing and links nothing (the console finishes the link);
        // a repeat starts another install flow.
        ["connect"] => Some(REPEATS),
        // Unlinking an installation again finds it gone.
        ["installations", "delete"] => Some(UNDO),
        // A lower cap, or a first one, allows less spend, so only a raise or a removal asks.
        ["limit", "update"] => Some(Spec {
            confirmation: Asks::When(RUNNER_CAP_RAISED),
            ..UNDO
        }),
        // A settings update sets the keys given, so a repeat leaves the same settings; only a
        // change that may let jobs spend more of the balance asks.
        ["settings", "update"] => Some(Spec {
            confirmation: Asks::When(RUNNER_SETTINGS_SPEND_MORE),
            ..UNDO
        }),
        _ => None,
    }
}

/// The rows of `hipc compute databases ...`, by the rest of the path.
pub(super) fn of_compute_databases(path: &[&str]) -> Option<Spec> {
    match path {
        ["list" | "get" | "events" | "metrics" | "refusals"] => Some(READ_ONLY),
        // A restart is queued again on a repeat, and restarts PostgreSQL again.
        ["restart"] => Some(SPEND),
        // A repeat delete of a database answers 409 "Already decommissioned" (swagger).
        ["delete"]
        // Exposing a database publishes its primary and mints nothing, so a repeat leaves it
        // published; it makes the primary reachable from the internet, so it is confirmed.
        | ["exposure", "create" | "delete"] => Some(UNDO),
        // A create and a restore, which makes a new database, answer with its credentials,
        // which the server shows once.
        ["create" | "restore"] => Some(Spec {
            reveal: Asks::When(NO_CREDENTIALS_DIR),
            ..SPEND
        }),
        _ => None,
    }
}

/// The rows of `hipc compute kubernetes ...`, by the rest of the path.
pub(super) fn of_compute_kubernetes(path: &[&str]) -> Option<Spec> {
    match path {
        ["clusters", "list" | "get" | "events" | "refusals"]
        | ["node-pools" | "operations", "list"] => Some(READ_ONLY),
        // The backups answer holds presigned download links and sealed keys; the read
        // changes nothing.
        ["backups", "list"] => Some(Spec {
            reveal: Asks::Always,
            ..READ_ONLY
        }),
        // A kubeconfig goes only to a new file and never to stdout, so it needs no
        // `--reveal`, and a repeat finds that file and stops.
        ["clusters", "kubeconfig"] => Some(UNGATED_WRITE),
        // A node pool starts empty and spends nothing until it is scaled, which is confirmed;
        // a repeat of one name is refused.
        ["node-pools", "create"] => Some(REPEATS),
        // A scale up and a replacement launch billed VMs. A scale is to a size, but a repeat
        // after an add was rejected (its signature expired) adds again, so both count as
        // spending again.
        ["node-pools", "scale"] | ["nodes", "replace"] => Some(SPEND),
        // A cluster delete again finds the teardown running or over. A cancel moves the
        // operation out of `queued`, `running`, or `blocked`, and a resume out of `blocked`
        // or `queued`, so a repeat usually finds it moved and refuses before the question. A
        // node removal again finds the remove operation running.
        ["clusters" | "nodes", "delete"] | ["operations", "cancel" | "resume"] => Some(UNDO),
        _ => None,
    }
}

/// The rows of `hipc cdn ...`, by the rest of the path.
pub(super) fn of_cdn(path: &[&str]) -> Option<Spec> {
    match path {
        // An estimate is a POST that changes nothing.
        ["prices" | "regions" | "estimate" | "stats"]
        | ["zones" | "hostnames" | "purges", "list"]
        // A Domain Connect link is read; the records change only when a person approves
        // them at the provider.
        | ["hostnames", "domain-connect"]
        | ["zones" | "purges" | "cap" | "rules", "get"]
        // A WAF test is a POST that changes nothing and publishes nothing.
        | ["waf", "get" | "status" | "analytics" | "events" | "test"]
        | ["waf", "rules" | "rate-limits", "get"]
        | ["waf", "versions", "list" | "get"]
        | ["lists", "list" | "get"] => Some(READ_ONLY),
        // A WAF list's items added again leave the same list, which the server merges, but an
        // added item changes what every zone that uses the list lets through.
        ["lists", "items", "create"] => Some(CONFIRMED_UNGATED),
        // A hostname verify checks its DNS records again.
        ["hostnames", "verify"] | ["lists", "update"] => Some(UNGATED_WRITE),
        // A purge spends nothing, so it is not gated; a repeat purges again. A WAF list costs
        // nothing, so its create is not gated; tenet 7 calls a create non-idempotent, and a
        // repeat of one name is refused with 409 `list_exists`.
        ["purges" | "lists", "create"] => Some(REPEATS),
        ["hostnames", "create"] => Some(SPEND),
        // Whether the bucket is private is known only from the answer, after the write, so
        // every create needs `--public`, and every update that moves the origin.
        ["zones", "create"] => Some(Spec {
            guards: &[PUBLIC_ALWAYS],
            ..SPEND
        }),
        // A zone update can publish a private bucket. A repeat is safe.
        ["zones", "update"] => Some(Spec {
            guards: &[Guard {
                code: Code::PublicRequired,
                when: Some("`--bucket` or `--prefix` is given"),
            }],
            ..UNDO
        }),
        ["zones" | "hostnames" | "lists", "delete"]
        // A WAF settings change publishes a version, and a mode but `off` is billed. A repeat
        // is safe.
        | ["waf", "update"]
        // A rollback publishes a copy of that version again, which leaves the same WAF.
        | ["waf", "versions", "rollback"]
        // A DNS setup writes the hostname's records at the provider, and a repeat writes the
        // same records. It changes a DNS zone outside Hippius, so it is confirmed.
        | ["hostnames", "dns-setup"]
        | ["lists", "items", "delete"] => Some(UNDO),
        // Every spend cap asks the same way: a raise, not a cut. A repeat is safe.
        ["cap", "update"] => Some(Spec {
            confirmation: Asks::When(CDN_CAP_RAISED),
            ..UNDO
        }),
        ["rules", "update"] => Some(Spec {
            guards: &[Guard {
                code: Code::RemoveOthersRequired,
                when: Some("the rules file leaves out or moves a rule the zone has now"),
            }],
            ..UNDO
        }),
        ["waf", "rules", "update"] => Some(Spec {
            guards: &[Guard {
                code: Code::RemoveOthersRequired,
                when: Some(
                    "the rules file leaves out or moves a rule the zone has now, or changes its \
                     expression, action, or enabled flag; a missing enabled counts as true",
                ),
            }],
            confirmation: Asks::When(WAF_CHECK),
            ..UNDO
        }),
        ["waf", "rate-limits", "update"] => Some(Spec {
            guards: &[Guard {
                code: Code::RemoveOthersRequired,
                when: Some(
                    "the rules file leaves out a rate limit the zone has now, or changes its \
                     expression, action, key, period, requests, timeout, or enabled flag; a \
                     missing enabled counts as true",
                ),
            }],
            confirmation: Asks::When(WAF_CHECK),
            ..UNDO
        }),
        ["lists", "items", "update"] => Some(Spec {
            guards: &[Guard {
                code: Code::RemoveOthersRequired,
                when: Some("the items file leaves out an item the list stores now"),
            }],
            ..UNDO
        }),
        _ => None,
    }
}

/// The rows of `hipc support ...`, by the rest of the path.
pub(super) fn of_support(path: &[&str]) -> Option<Spec> {
    match path {
        ["tickets", "get" | "list"] | ["tickets", "messages", "list"] => Some(READ_ONLY),
        // A reply goes to a ticket that already exists, so it is not gated; a repeat posts the
        // message again.
        ["tickets", "reply"] => Some(Spec {
            refused_when: Some(TICKET_HOLDS_A_TOKEN),
            ..REPEATS
        }),
        // A ticket spends nothing, but a person on the support team reads it.
        ["tickets", "create"] => Some(Spec {
            refused_when: Some(TICKET_HOLDS_A_TOKEN),
            ..SPEND
        }),
        // A close again leaves the ticket closed. hipc refuses one of a ticket its read shows
        // closed, so a repeat of a close that worked exits `conflict`; it is still
        // `idempotent` on the assumption that the server's `PATCH` only sets the status, so
        // a second one, say after a close elsewhere between the read and the write, changes
        // nothing. It ends the support team's work on the ticket, as a cancel does, so it is
        // confirmed.
        ["tickets", "update"] => Some(UNDO),
        _ => None,
    }
}

/// The rows of `hipc network ...`, by the rest of the path.
pub(super) fn of_network(path: &[&str]) -> Option<Spec> {
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

/// The row of `hipc api`. A POST can spend money, so it reports the effect of its most
/// dangerous method. At run time GET and HEAD are reads and need no confirmation, except on a
/// route whose read creates something, which is confirmed as its curated command is. The
/// requests whose answer holds a secret need `--reveal`, reads included, as the curated
/// commands that reach some of them do. The routes of the curated guards need their flags,
/// and a route a curated command or the console is the one path to is refused.
pub(super) fn of_api() -> Spec {
    Spec {
        confirmation: Asks::When(&API_CONFIRMATION),
        reveal: Asks::When(API_REVEAL_WHEN),
        refused_when: Some(&API_REFUSED_WHEN),
        guards: &API_GUARDS,
        ..SPEND
    }
}
