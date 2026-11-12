//! The routes each command sends to, which `hipc schema` lists under `routes`, so a caller
//! and the release scripts learn them from the binary rather than keep their own copy.
//!
//! An end-to-end check holds every request a command sends in the test suite against the
//! command's routes here, so a command that starts sending to another route fails until it
//! is listed.

use hippius::routes::{
    Endpoint, Route, account, billing, cdn, compute, databases, drive, hub, kubernetes, network,
    operator, runners, s3, support,
};

/// The endpoints of the route constants given, as `routes` returns them.
macro_rules! sends {
    ($($route:path),* $(,)?) => {
        vec![$(endpoint(&$route)),*]
    };
}

/// `route`'s endpoint, for as long as the constant lives.
fn endpoint<const N: usize>(route: &'static Route<N>) -> &'static Endpoint {
    route.endpoint()
}

/// The routes the command at `path` may send to, each once: none for a command that sends
/// nothing. `hipc api` sends to the route it is given; it lists the routes `hipc` knows that
/// only it reaches, so every route `hipc` knows is listed by some command, and one no command
/// lists is one `hipc` does not know (`TENETS.md` tenet 7).
pub(super) fn routes(path: &[&str]) -> Vec<&'static Endpoint> {
    match path {
        ["api"] => sends![
            account::DELETION_REQUEST,
            account::DELETION_WITHDRAW,
            compute::APP_ENGINE_COMPOSE_UPDATE,
            network::ISOLATION_UPDATE,
            operator::NODE_STORAGE_UPDATE,
        ],
        ["compute", "app-engine", rest @ ..] => of_compute_app_engine(rest),
        ["compute", "runners", rest @ ..] => of_compute_runners(rest),
        ["compute", "databases", rest @ ..] => of_compute_databases(rest),
        ["compute", "kubernetes", rest @ ..] => of_compute_kubernetes(rest),
        ["status"] => sends![support::SERVICES_STATUS],
        ["auth", rest @ ..] => of_auth(rest),
        ["account", rest @ ..] => of_account(rest),
        ["billing", rest @ ..] => of_billing(rest),
        ["drive", rest @ ..] => of_drive(rest),
        ["s3", rest @ ..] => s3(rest),
        ["hub", rest @ ..] => of_hub(rest),
        ["compute", rest @ ..] => of_compute(rest),
        ["cdn", rest @ ..] => of_cdn(rest),
        ["support", rest @ ..] => of_support(rest),
        ["network", rest @ ..] => of_network(rest),
        _ => Vec::new(),
    }
}

/// The routes of `hipc auth ...`, by the rest of the path.
fn of_auth(path: &[&str]) -> Vec<&'static Endpoint> {
    match path {
        ["status"] => sends![account::ACCOUNTS, account::USER],
        _ => Vec::new(),
    }
}

/// The routes of `hipc account ...`, by the rest of the path.
fn of_account(path: &[&str]) -> Vec<&'static Endpoint> {
    match path {
        ["list" | "get"] => sends![account::ACCOUNTS],
        ["notifications", "get"] => sends![account::NOTIFICATIONS],
        ["notifications", "update"] => sends![account::NOTIFICATIONS_UPDATE],
        ["marketing-consent", "get"] => sends![account::MARKETING_CONSENT],
        ["marketing-consent", "update"] => sends![account::MARKETING_CONSENT_UPDATE],
        ["members", "list"] => sends![account::MEMBERS],
        ["members", "update"] => sends![account::MEMBER_UPDATE, account::MEMBERS],
        ["members", "delete"] => sends![account::MEMBER_DELETE],
        ["invites", "list"] => sends![account::INVITES],
        ["invites", "create"] => sends![account::INVITE_CREATE],
        ["invites", "revoke"] => sends![account::INVITE_REVOKE],
        ["invites", "preview"] => sends![account::INVITE_PREVIEW],
        ["invites", "accept"] => sends![account::INVITE_ACCEPT, account::INVITE_PREVIEW],
        ["projects", "list"] => sends![account::PROJECTS],
        ["projects", "create"] => sends![account::PROJECT_CREATE],
        ["projects", "update"] => sends![account::PROJECT_UPDATE],
        ["projects", "delete"] => sends![account::PROJECT_DELETE],
        ["projects", "transfer"] => sends![
            account::PROJECT_TRANSFER,
            account::PROJECTS,
            account::ACCOUNTS
        ],
        ["projects", "budget", "get"] => sends![account::PROJECT_BUDGET],
        ["projects", "budget", "update"] => sends![account::PROJECT_BUDGET_UPDATE],
        ["projects", "budget", "delete"] => sends![account::PROJECT_BUDGET_DELETE],
        ["projects", "quota", "get"] => sends![account::PROJECT_QUOTA],
        ["projects", "quota", "update"] => sends![account::PROJECT_QUOTA_UPDATE],
        ["project-transfers", "list"] => sends![account::PROJECT_TRANSFERS],
        ["project-transfers", "accept"] => {
            sends![account::PROJECT_TRANSFER_ACCEPT, account::PROJECT_TRANSFERS]
        }
        ["project-transfers", "decline"] => sends![
            account::PROJECT_TRANSFER_DECLINE,
            account::PROJECT_TRANSFERS
        ],
        ["project-transfers", "cancel"] => {
            sends![account::PROJECT_TRANSFER_CANCEL, account::PROJECT_TRANSFERS]
        }
        ["billing-accounts", "list"] => sends![account::BILLING_ACCOUNTS],
        ["billing-accounts", "create"] => sends![account::BILLING_ACCOUNT_CREATE],
        ["billing-accounts", "update"] => sends![account::BILLING_ACCOUNT_UPDATE],
        ["billing-accounts", "costs"] => sends![account::BILLING_ACCOUNT_COSTS],
        ["referrals", "list"] => sends![account::REFERRAL_CODES],
        ["referrals", "create"] => sends![account::REFERRAL_CODE_CREATE],
        ["referrals", "update"] => sends![account::REFERRAL_CODE_UPDATE],
        ["referrals", "analytics"] => sends![account::REFERRAL_ANALYTICS],
        _ => Vec::new(),
    }
}

/// The routes of `hipc billing ...`, by the rest of the path.
fn of_billing(path: &[&str]) -> Vec<&'static Endpoint> {
    match path {
        ["balance"] => sends![billing::BALANCE],
        ["subscriptions"] => sends![
            drive::SUBSCRIPTION,
            s3::SUBSCRIPTION,
            hub::SUBSCRIPTIONS,
            billing::STRIPE_SUBSCRIPTION
        ],
        ["transactions"] => sends![billing::TRANSACTIONS],
        ["stripe"] => sends![billing::STRIPE_SUBSCRIPTION],
        ["summary"] => sends![compute::BILLING_SUMMARY],
        ["usage"] => sends![compute::USAGE],
        ["invoices", "list"] => sends![billing::INVOICES],
        ["invoices", "get"] => sends![billing::INVOICE_PDF],
        ["auto-reload", "get"] => sends![billing::AUTO_RELOAD],
        ["auto-reload", "history"] => sends![billing::AUTO_RELOAD_HISTORY],
        ["auto-reload", "update"] => sends![billing::AUTO_RELOAD_UPDATE, billing::AUTO_RELOAD],
        ["auto-reload", "card-setup"] => sends![billing::AUTO_RELOAD_CARD_SETUP],
        ["profile", "get"] => sends![billing::PROFILE],
        ["profile", "update"] => sends![billing::PROFILE_UPDATE, billing::PROFILE],
        ["native-quote"] => sends![billing::NATIVE_QUOTE],
        ["gamma", "get"] => sends![billing::GAMMA],
        ["gamma", "hotkeys", "create"] => sends![
            billing::GAMMA_HOTKEY_CHALLENGE,
            billing::GAMMA_HOTKEY_CREATE
        ],
        ["gamma", "hotkeys", "delete"] => sends![billing::GAMMA_HOTKEY_DELETE],
        ["gamma", "conversions", "create"] => sends![billing::GAMMA_CONVERSION_CREATE],
        ["gamma", "conversions", "list"] => sends![billing::GAMMA_CONVERSIONS],
        ["gamma", "conversions", "get"] => sends![billing::GAMMA_CONVERSION],
        ["top-up"] => sends![
            billing::CARD_CHECKOUT,
            billing::NATIVE_BUY,
            billing::NATIVE_QUOTE,
            billing::NATIVE_PURCHASES,
            billing::BITCOIN_INVOICE
        ],
        _ => Vec::new(),
    }
}

/// The routes of `hipc drive ...`, by the rest of the path.
fn of_drive(path: &[&str]) -> Vec<&'static Endpoint> {
    match path {
        ["plans"] => sends![drive::PLANS],
        ["subscription", "get"] => sends![drive::SUBSCRIPTION],
        ["subscription", "subscribe"] => sends![drive::SUBSCRIBE],
        ["subscription", "change"] => sends![drive::SUBSCRIPTION_CHANGE],
        ["subscription", "cancel"] => sends![drive::SUBSCRIPTION_CANCEL],
        ["subscription", "checkout"] => sends![drive::CHECKOUT],
        ["subscription", "history"] => sends![drive::HISTORY],
        _ => Vec::new(),
    }
}

/// The routes of `hipc s3 ...`, by the rest of the path.
fn s3(path: &[&str]) -> Vec<&'static Endpoint> {
    match path {
        ["plans"] => sends![s3::PLANS],
        ["subscription", "get"] => sends![s3::SUBSCRIPTION],
        ["subscription", "subscribe"] => sends![s3::SUBSCRIBE],
        ["subscription", "change"] => sends![s3::SUBSCRIPTION_CHANGE],
        ["subscription", "cancel"] => sends![s3::SUBSCRIPTION_CANCEL],
        ["subscription", "checkout"] => sends![s3::CHECKOUT],
        ["subscription", "history"] => sends![s3::HISTORY],
        ["buckets", "list"] => sends![s3::BUCKETS],
        ["buckets", "acl", "list"] => sends![s3::ACLS],
        ["buckets", "acl", "get"] => sends![s3::BUCKET_ACL],
        ["buckets", "acl", "update"] => sends![s3::BUCKET_ACL_SET],
        ["buckets", "objects", "list"] => sends![s3::OBJECTS],
        ["buckets", "objects", "presign"] => sends![s3::PRESIGNED_URL],
        ["buckets", "create"] => sends![s3::BUCKET_CREATE],
        ["buckets", "delete"] => sends![s3::BUCKET_DELETE],
        ["tokens", "list"] => sends![s3::MASTER_TOKENS, s3::SUB_TOKENS],
        ["tokens", "create"] => sends![s3::MASTER_TOKEN_CREATE, s3::SUB_TOKEN_CREATE],
        ["tokens", "revoke"] => sends![s3::MASTER_TOKEN_REVOKE, s3::SUB_TOKEN_REVOKE],
        ["tokens", "rotate"] => sends![s3::MASTER_TOKEN_ROTATE, s3::SUB_TOKEN_ROTATE],
        _ => Vec::new(),
    }
}

/// The routes of `hipc hub ...`, by the rest of the path.
fn of_hub(path: &[&str]) -> Vec<&'static Endpoint> {
    match path {
        ["plans"] => sends![hub::PLANS],
        ["subscription", "get"] => sends![hub::SUBSCRIPTIONS],
        ["subscription", "subscribe"] => sends![
            hub::SUBSCRIBE,
            hub::PLANS,
            hub::SUBSCRIPTIONS,
            hub::SUBSCRIPTION_CANCEL
        ],
        ["subscription", "cancel"] => sends![hub::SUBSCRIPTION_CANCEL],
        ["namespace", "availability"] => sends![hub::NAMESPACE_CHECK],
        ["namespace", "create"] => sends![hub::NAMESPACE_CREATE],
        ["namespace", "status"] => sends![hub::NAMESPACE_STATUS],
        ["namespace", "get"] => sends![hub::NAMESPACE],
        ["namespace", "update"] => sends![hub::PUBLICITY_UPDATE],
        ["namespace", "robot", "get"] => sends![hub::ROBOT],
        ["namespace", "robot", "rotate"] => sends![hub::ROBOT_ROTATE],
        ["usage"] => sends![hub::USAGE, hub::USAGE_BY_REPOSITORY],
        ["keys", "list"] => sends![hub::KEYS],
        ["keys", "get"] => sends![hub::KEY],
        ["keys", "create"] => sends![hub::KEY_CREATE],
        ["keys", "rotate"] => sends![hub::KEY_ROTATE],
        ["keys", "delete"] => sends![hub::KEY_DELETE],
        ["repositories", "list" | "get"] => sends![hub::REPOSITORIES],
        ["repositories", "delete"] => sends![hub::REPOSITORY_DELETE],
        ["artifacts", "list"] => sends![hub::ARTIFACTS],
        ["artifacts", "get"] => sends![hub::ARTIFACT],
        ["artifacts", "delete"] => sends![hub::ARTIFACT_DELETE],
        ["events", "list"] => sends![hub::EVENTS],
        ["models", "list"] => sends![hub::MODELS],
        ["models", "formats"] => sends![hub::MODEL_FORMATS],
        ["models", "get"] => sends![hub::MODEL, hub::MODEL_VERSION],
        _ => Vec::new(),
    }
}

/// The routes of `hipc compute ...`, by the rest of the path.
fn of_compute(path: &[&str]) -> Vec<&'static Endpoint> {
    match path {
        ["prices"] => sends![compute::PRICES],
        ["public-prices"] => sends![compute::PUBLIC_PRICES],
        ["quota"] => sends![compute::QUOTA],
        ["charge-cap"] => sends![compute::CHARGE_CAP],
        ["flavors"] => sends![compute::FLAVORS],
        ["images"] => sends![compute::IMAGES],
        ["apps"] => sends![compute::APPS],
        ["regions"] => sends![compute::REGIONS],
        ["placement-groups"] => sends![compute::PLACEMENT_GROUPS],
        ["feasibility"] => sends![compute::FEASIBILITY],
        ["vms", "list"] => sends![compute::VMS],
        ["vms", "get"] | ["vms", "power-policy", "get"] => sends![compute::VM],
        ["vms", "attestation"] => sends![compute::VM_ATTESTATION],
        ["vms", "failovers"] => sends![compute::VM_FAILOVERS],
        ["vms", "create"] => sends![
            compute::VM_CREATE,
            compute::VM,
            compute::FIREWALL_UPDATE,
            compute::BACKUP_OPTIONS
        ],
        ["vms", "start"] => sends![compute::VM_START],
        ["vms", "stop"] => sends![compute::VM_STOP],
        ["vms", "reboot"] => sends![compute::VM_REBOOT],
        ["vms", "resize-options"] => sends![compute::VM_RESIZE_OPTIONS],
        ["vms", "resize"] => sends![compute::VM_RESIZE],
        ["vms", "resize-status"] => sends![compute::VM_RESIZE_STATUS],
        ["vms", "power-policy", "update"] => sends![compute::VM_POWER_POLICY_UPDATE],
        ["vms", "delete"] => sends![compute::VM_DELETE, compute::VM],
        ["public-ip", "availability"] => sends![compute::PUBLIC_IP_AVAILABILITY],
        ["public-ip", "get"] => sends![compute::PUBLIC_IP],
        ["public-ip", "create"] => sends![compute::PUBLIC_IP_CREATE],
        ["public-ip", "delete"] => sends![compute::PUBLIC_IP_DELETE],
        ["public-ip", "usage"] => sends![compute::PUBLIC_IP_USAGE],
        ["firewall", "get"] => sends![compute::FIREWALL],
        ["firewall", "update"] => sends![compute::FIREWALL_UPDATE, compute::FIREWALL],
        ["exposures", "list"] => sends![compute::EXPOSURES],
        ["exposures", "create"] => sends![compute::EXPOSURE_CREATE],
        ["exposures", "delete"] => sends![compute::EXPOSURE_DELETE],
        ["backups", "options"] => sends![compute::BACKUP_OPTIONS],
        ["backups", "policy", "get"] => sends![compute::BACKUP_POLICY],
        ["backups", "policy", "update"] => {
            sends![compute::BACKUP_POLICY_UPDATE, compute::BACKUP_OPTIONS]
        }
        ["backups", "policy", "delete"] => sends![compute::BACKUP_POLICY_DELETE],
        ["backups", "list"] => sends![compute::BACKUPS],
        ["backups", "restore"] => sends![compute::BACKUP_RESTORE, compute::VM],
        ["backups", "restore-status"] => sends![compute::BACKUP_RESTORE_STATUS],
        ["ssh-keys", "list"] => sends![compute::SSH_KEYS],
        ["ssh-keys", "get"] => sends![compute::SSH_KEY],
        ["ssh-keys", "create"] => sends![compute::SSH_KEY_CREATE],
        ["ssh-keys", "delete"] => sends![compute::SSH_KEY_DELETE],
        ["account-ssh-keys"] => sends![compute::ACCOUNT_SSH_KEYS],
        _ => Vec::new(),
    }
}

/// The routes of `hipc compute app-engine ...`, by the rest of the path.
fn of_compute_app_engine(path: &[&str]) -> Vec<&'static Endpoint> {
    match path {
        ["get"] => sends![compute::APP_ENGINE_INFO],
        ["validate"] => sends![compute::APP_ENGINE_VALIDATE],
        ["fix"] => sends![compute::APP_ENGINE_FIX],
        ["apps", "list"] => sends![compute::APP_ENGINE_APPS],
        ["apps", "get"] => sends![compute::APP_ENGINE_APP],
        ["apps", "create"] => sends![
            compute::APP_ENGINE_APP_CREATE,
            compute::APP_ENGINE_VALIDATE,
            compute::APP_ENGINE_APP,
            compute::BACKUP_OPTIONS
        ],
        ["apps", "runtime"] => sends![compute::APP_ENGINE_RUNTIME],
        ["guest-components", "get"] => sends![compute::APP_ENGINE_GUEST_COMPONENTS],
        ["guest-components", "update-status"] => sends![compute::APP_ENGINE_GUEST_UPGRADE],
        _ => Vec::new(),
    }
}

/// The routes of `hipc compute runners ...`, by the rest of the path.
fn of_compute_runners(path: &[&str]) -> Vec<&'static Endpoint> {
    match path {
        ["settings", "get"] => sends![runners::SETTINGS],
        ["settings", "update"] => sends![runners::SETTINGS_UPDATE, runners::SETTINGS],
        ["installations", "list"] => sends![runners::INSTALLATIONS],
        ["installations", "delete"] => sends![runners::INSTALLATION_DELETE],
        ["jobs"] => sends![runners::JOBS],
        ["connect"] => sends![runners::CONNECT],
        ["usage"] => sends![runners::USAGE],
        ["limit", "get"] => sends![runners::LIMIT],
        ["limit", "update"] => sends![runners::LIMIT_UPDATE, runners::LIMIT],
        _ => Vec::new(),
    }
}

/// The routes of `hipc compute databases ...`, by the rest of the path.
fn of_compute_databases(path: &[&str]) -> Vec<&'static Endpoint> {
    match path {
        ["list"] => sends![databases::DATABASES],
        ["get"] => sends![databases::DATABASE],
        ["create"] => sends![databases::DATABASE_CREATE],
        ["delete"] => sends![databases::DATABASE_DELETE],
        ["restart"] => sends![databases::DATABASE_RESTART],
        ["restore"] => sends![databases::DATABASE_RESTORE],
        ["exposure", "create"] => sends![databases::DATABASE_EXPOSE],
        ["exposure", "delete"] => sends![databases::DATABASE_EXPOSURE_DELETE],
        ["events"] => sends![databases::DATABASE_EVENTS],
        ["metrics"] => sends![databases::DATABASE_METRICS],
        ["refusals"] => sends![databases::DATABASE_REFUSALS],
        _ => Vec::new(),
    }
}

/// The routes of `hipc compute kubernetes ...`, by the rest of the path.
fn of_compute_kubernetes(path: &[&str]) -> Vec<&'static Endpoint> {
    match path {
        ["clusters", "list"] => sends![kubernetes::CLUSTERS],
        ["clusters", "get"] => sends![kubernetes::CLUSTER],
        ["clusters", "delete"] => sends![kubernetes::CLUSTER_DELETE, kubernetes::CLUSTER],
        ["clusters", "events"] => sends![kubernetes::CLUSTER_EVENTS],
        ["clusters", "refusals"] => sends![kubernetes::CLUSTER_REFUSALS],
        ["clusters", "kubeconfig"] => sends![kubernetes::CLUSTER, kubernetes::CLUSTER_SECRETS],
        ["backups", "list"] => sends![kubernetes::BACKUPS],
        ["node-pools", "list"] => sends![kubernetes::NODE_POOLS],
        ["node-pools", "create"] => sends![kubernetes::NODE_POOL_CREATE],
        ["node-pools", "scale"] => sends![
            kubernetes::NODE_POOL_SCALE,
            kubernetes::CLUSTER,
            kubernetes::NODE_POOLS,
            kubernetes::OPERATIONS
        ],
        ["operations", "list"] => sends![kubernetes::OPERATIONS],
        ["operations", "cancel"] => sends![kubernetes::OPERATION_CANCEL, kubernetes::OPERATIONS],
        ["operations", "resume"] => sends![kubernetes::OPERATION_RESUME, kubernetes::OPERATIONS],
        ["nodes", "delete"] => sends![kubernetes::NODE_REMOVE],
        ["nodes", "replace"] => sends![
            kubernetes::NODE_REPLACE,
            kubernetes::CLUSTER,
            kubernetes::NODE_POOLS,
            kubernetes::OPERATIONS,
            kubernetes::NODE_POOL_SCALE
        ],
        _ => Vec::new(),
    }
}

/// The routes of `hipc cdn ...`, by the rest of the path.
fn of_cdn(path: &[&str]) -> Vec<&'static Endpoint> {
    match path {
        ["prices"] => sends![cdn::PRICES],
        ["regions"] => sends![cdn::REGIONS],
        ["estimate"] => sends![cdn::ESTIMATE],
        ["zones", "list"] => sends![cdn::ZONES],
        ["zones", "get"] => sends![cdn::ZONE],
        ["zones", "create"] => sends![cdn::ZONE_CREATE],
        ["zones", "update"] => sends![cdn::ZONE_UPDATE, cdn::ZONE],
        ["zones", "delete"] => sends![cdn::ZONE_DELETE],
        ["cap", "get"] => sends![cdn::CAP],
        ["cap", "update"] => sends![cdn::CAP_UPDATE, cdn::CAP],
        ["hostnames", "list"] => sends![cdn::HOSTNAMES],
        ["hostnames", "create"] => sends![cdn::HOSTNAME_CREATE],
        ["hostnames", "verify"] => sends![cdn::HOSTNAME_VERIFY],
        ["hostnames", "dns-setup"] => sends![cdn::HOSTNAME_DNS_SETUP],
        ["hostnames", "domain-connect"] => sends![cdn::HOSTNAME_DOMAIN_CONNECT],
        ["hostnames", "delete"] => sends![cdn::HOSTNAME_DELETE],
        ["purges", "create"] => sends![cdn::PURGE_CREATE],
        ["purges", "list"] => sends![cdn::PURGES],
        ["purges", "get"] => sends![cdn::PURGE],
        ["rules", "get"] => sends![cdn::RULES],
        ["rules", "update"] => sends![cdn::RULES_UPDATE, cdn::RULES],
        ["stats"] => sends![cdn::STATS],
        ["waf", "get"] => sends![cdn::WAF],
        ["waf", "update"] => sends![cdn::WAF_UPDATE],
        ["waf", "status"] => sends![cdn::WAF_STATUS],
        ["waf", "analytics"] => sends![cdn::WAF_ANALYTICS],
        ["waf", "events"] => sends![cdn::WAF_EVENTS],
        ["waf", "test"] => sends![cdn::WAF_TEST],
        ["waf", "rules", "get"] => sends![cdn::WAF_RULES],
        ["waf", "rules", "update"] => sends![cdn::WAF_RULES_UPDATE, cdn::WAF_RULES],
        ["waf", "rate-limits", "get"] => sends![cdn::WAF_RATE_LIMITS],
        ["waf", "rate-limits", "update"] => {
            sends![cdn::WAF_RATE_LIMITS_UPDATE, cdn::WAF_RATE_LIMITS]
        }
        ["waf", "versions", "list"] => sends![cdn::WAF_VERSIONS],
        ["waf", "versions", "get"] => sends![cdn::WAF_VERSION],
        ["waf", "versions", "rollback"] => sends![cdn::WAF_ROLLBACK],
        ["lists", "list"] => sends![cdn::LISTS],
        ["lists", "get"] => sends![cdn::LIST],
        ["lists", "create"] => sends![cdn::LIST_CREATE],
        ["lists", "update"] => sends![cdn::LIST_UPDATE],
        ["lists", "delete"] => sends![cdn::LIST_DELETE],
        ["lists", "items", "create"] => sends![cdn::LIST_ITEMS_ADD],
        ["lists", "items", "update"] => sends![cdn::LIST_ITEMS_REPLACE, cdn::LIST],
        ["lists", "items", "delete"] => sends![cdn::LIST_ITEMS_REMOVE],
        _ => Vec::new(),
    }
}

/// The routes of `hipc support ...`, by the rest of the path.
fn of_support(path: &[&str]) -> Vec<&'static Endpoint> {
    match path {
        ["tickets", "list"] => sends![support::TICKETS],
        ["tickets", "get"] => sends![support::TICKET],
        ["tickets", "create"] => sends![support::TICKET_CREATE],
        ["tickets", "reply"] => sends![support::MESSAGE_CREATE],
        ["tickets", "update"] => sends![support::TICKET_UPDATE, support::TICKET],
        ["tickets", "messages", "list"] => sends![support::MESSAGES],
        _ => Vec::new(),
    }
}

/// The routes of `hipc network ...`, by the rest of the path.
fn of_network(path: &[&str]) -> Vec<&'static Endpoint> {
    match path {
        ["get"] => sends![network::ME],
        ["create"] => sends![network::PROVISION],
        ["peers", "list"] => sends![network::PEERS],
        ["peers", "get"] => sends![network::PEER],
        ["peers", "update"] => sends![network::PEER_UPDATE],
        ["peers", "delete"] => sends![network::PEER_DELETE],
        ["groups", "list"] => sends![network::GROUPS],
        ["policies", "list"] => sends![network::POLICIES],
        ["vpcs", "list"] => sends![network::VPCS],
        ["vpcs", "resources", "list"] => sends![network::VPC_RESOURCES],
        ["vpcs", "routers", "list"] => sends![network::VPC_ROUTERS],
        _ => Vec::new(),
    }
}
