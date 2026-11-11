//! The CDN: zones, hostnames, caching rules, purges, the cap, the WAF, and IP lists.

use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::routes::{Guard, Route, RouteFlag};
use crate::view::{Slot, field, nested, text_or_number};

routes! {
    /// The CDN prices.
    PRICES: 0 = Route::get("/api/cdn/prices/").scoped().project();

    /// The CDN regions.
    REGIONS: 0 = Route::get("/api/cdn/regions/").scoped().project();

    /// A price estimate; changes nothing.
    ESTIMATE: 0 = Route::post("/api/cdn/estimate/").scoped().project();

    /// The CDN zones.
    ZONES: 0 = Route::get("/api/cdn/zones/").scoped().project();

    /// Creates a zone. `hipc api` needs `--public` on it: whether the bucket it serves is
    /// private is known only from the answer.
    ZONE_CREATE: 0 = Route::post("/api/cdn/zones/")
        .scoped()
        .project()
        .guard(Guard::Needs(&[RouteFlag::Public]));

    /// One zone.
    ZONE: 1 = Route::get("/api/cdn/zones/{zone_id}/").scoped().project();

    /// Changes a zone. One that sets `origin` may serve another bucket, so `hipc api` needs
    /// `--public` as on the create; the delete carries the guard too, as every write method
    /// of the route takes it.
    ZONE_UPDATE: 1 = Route::patch("/api/cdn/zones/{zone_id}/")
        .scoped()
        .project()
        .guard(Guard::NeedsWhenSet(&[RouteFlag::Public], "origin"));

    /// Deletes a zone.
    ZONE_DELETE: 1 = Route::delete("/api/cdn/zones/{zone_id}/")
        .scoped()
        .project()
        .guard(Guard::NeedsWhenSet(&[RouteFlag::Public], "origin"));

    /// A zone's spend cap.
    CAP: 1 = Route::get("/api/cdn/zones/{zone_id}/cap/").scoped().project();

    /// Changes a zone's spend cap.
    CAP_UPDATE: 1 = Route::patch("/api/cdn/zones/{zone_id}/cap/").scoped().project();

    /// A zone's purges.
    PURGES: 1 = Route::get("/api/cdn/zones/{zone_id}/purges/").scoped().project();

    /// One purge.
    PURGE: 2 = Route::get("/api/cdn/zones/{zone_id}/purges/{purge_id}/").scoped().project();

    /// Purges cached paths.
    PURGE_CREATE: 1 = Route::post("/api/cdn/zones/{zone_id}/purge/").scoped().project();

    /// A zone's caching rules.
    RULES: 1 = Route::get("/api/cdn/zones/{zone_id}/rules/").scoped().project();

    /// Replaces a zone's caching rules. `hipc api` needs `--remove-others` on a whole-list
    /// replace whatever the body holds: it reads no list to compare, since a second copy of
    /// each list's parser that misread a body would turn the guard off.
    RULES_UPDATE: 1 = Route::put("/api/cdn/zones/{zone_id}/rules/")
        .scoped()
        .project()
        .guard(Guard::Needs(&[RouteFlag::RemoveOthers]));

    /// A zone's traffic.
    STATS: 1 = Route::get("/api/cdn/zones/{zone_id}/stats/").scoped().project();

    /// A zone's hostnames.
    HOSTNAMES: 1 = Route::get("/api/cdn/zones/{zone_id}/hostnames/").scoped().project();

    /// Adds a hostname.
    HOSTNAME_CREATE: 1 = Route::post("/api/cdn/zones/{zone_id}/hostnames/").scoped().project();

    /// Removes a hostname.
    HOSTNAME_DELETE: 2 = Route::delete("/api/cdn/zones/{zone_id}/hostnames/{hostname}/")
        .scoped()
        .project();

    /// Checks a hostname's DNS.
    HOSTNAME_VERIFY: 2 = Route::post("/api/cdn/zones/{zone_id}/hostnames/{hostname}/verify/")
        .scoped()
        .project();

    /// The Domain Connect link of a hostname.
    HOSTNAME_DOMAIN_CONNECT: 2 = Route::get("/api/cdn/zones/{zone_id}/hostnames/{hostname}/domain-connect/")
        .scoped()
        .project();

    /// Sets a hostname's DNS records with a provider credential.
    HOSTNAME_DNS_SETUP: 2 = Route::post("/api/cdn/zones/{zone_id}/hostnames/{hostname}/dns-setup/")
        .scoped()
        .project()
        .guard(Guard::Refused {
            why: "its body and its answer hold a DNS provider credential",
            hint: "run `hipc cdn hostnames dns-setup`, which reads the credential from a private \
                   file and keeps it out of every output",
        });

    /// A zone's WAF settings.
    WAF: 1 = Route::get("/api/cdn/zones/{zone_id}/waf/").scoped().project();

    /// Changes a zone's WAF settings.
    WAF_UPDATE: 1 = Route::patch("/api/cdn/zones/{zone_id}/waf/").scoped().project();

    /// Whether a zone's WAF is published.
    WAF_STATUS: 1 = Route::get("/api/cdn/zones/{zone_id}/waf/status/").scoped().project();

    /// What a zone's WAF matched.
    WAF_ANALYTICS: 1 = Route::get("/api/cdn/zones/{zone_id}/waf/analytics/").scoped().project();

    /// A zone's WAF events.
    WAF_EVENTS: 1 = Route::get("/api/cdn/zones/{zone_id}/waf/events/").scoped().project();

    /// A zone's WAF rules.
    WAF_RULES: 1 = Route::get("/api/cdn/zones/{zone_id}/waf/rules/").scoped().project();

    /// Replaces a zone's WAF rules.
    WAF_RULES_UPDATE: 1 = Route::put("/api/cdn/zones/{zone_id}/waf/rules/")
        .scoped()
        .project()
        .guard(Guard::Needs(&[RouteFlag::RemoveOthers]));

    /// A zone's rate limits.
    WAF_RATE_LIMITS: 1 = Route::get("/api/cdn/zones/{zone_id}/waf/rate-limits/").scoped().project();

    /// Replaces a zone's rate limits.
    WAF_RATE_LIMITS_UPDATE: 1 = Route::put("/api/cdn/zones/{zone_id}/waf/rate-limits/")
        .scoped()
        .project()
        .guard(Guard::Needs(&[RouteFlag::RemoveOthers]));

    /// Runs a request through a zone's WAF; changes nothing.
    WAF_TEST: 1 = Route::post("/api/cdn/zones/{zone_id}/waf/test/").scoped().project();

    /// A zone's published WAF versions.
    WAF_VERSIONS: 1 = Route::get("/api/cdn/zones/{zone_id}/waf/versions/").scoped().project();

    /// One WAF version.
    WAF_VERSION: 2 = Route::get("/api/cdn/zones/{zone_id}/waf/versions/{version}/")
        .scoped()
        .project();

    /// Publishes an earlier WAF version again.
    WAF_ROLLBACK: 2 = Route::post("/api/cdn/zones/{zone_id}/waf/versions/{version}/rollback/")
        .scoped()
        .project();

    /// The IP lists.
    LISTS: 0 = Route::get("/api/cdn/lists/").scoped().project();

    /// Creates an IP list.
    LIST_CREATE: 0 = Route::post("/api/cdn/lists/").scoped().project();

    /// One IP list.
    LIST: 1 = Route::get("/api/cdn/lists/{list_name}/").scoped().project();

    /// Changes an IP list.
    LIST_UPDATE: 1 = Route::patch("/api/cdn/lists/{list_name}/")
        .scoped()
        .project()
        .guard(Guard::NeedsWhenSet(&[RouteFlag::RemoveOthers], "items"));

    /// Deletes an IP list.
    LIST_DELETE: 1 = Route::delete("/api/cdn/lists/{list_name}/")
        .scoped()
        .project()
        .guard(Guard::NeedsWhenSet(&[RouteFlag::RemoveOthers], "items"));

    /// Adds items to an IP list. Swagger: a `POST` adds the items it names and a `DELETE`
    /// removes them; neither replaces the list, so neither takes the `PUT`'s flag.
    LIST_ITEMS_ADD: 1 = Route::post("/api/cdn/lists/{list_name}/items/")
        .scoped()
        .project()
        .guard(Guard::Needs(&[]));

    /// Removes items from an IP list.
    LIST_ITEMS_REMOVE: 1 = Route::delete("/api/cdn/lists/{list_name}/items/")
        .scoped()
        .project()
        .guard(Guard::Needs(&[]));

    /// Replaces an IP list's items.
    LIST_ITEMS_REPLACE: 1 = Route::put("/api/cdn/lists/{list_name}/items/")
        .scoped()
        .project()
        .guard(Guard::Needs(&[RouteFlag::RemoveOthers]));
}

/// The body of [`ESTIMATE`]: only the keys given.
#[derive(Debug, Default, Serialize)]
pub struct EstimateBody {
    /// GB of traffic a month, by region code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gb: Option<Map<String, Value>>,
    /// Requests a month.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requests: Option<u64>,
    /// Custom hostnames.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_domains: Option<u64>,
    /// Zones with a WAF, by tier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waf: Option<Map<String, Value>>,
}

/// The body of [`ZONE_CREATE`]: the name and the origin, then only the settings given.
#[derive(Debug, Serialize)]
pub struct ZoneCreateBody<'a> {
    /// The zone's name.
    pub name: &'a str,
    /// What the zone serves.
    pub origin: Origin<'a>,
    /// The region the shield cache is in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shield_region: Option<&'a str>,
    /// The object served for a path that ends in `/`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_document: Option<&'a str>,
    /// Whether a missing path serves the index document, for a single-page app.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spa_fallback: Option<bool>,
    /// The paths the fallback leaves out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spa_fallback_exclude: Option<&'a [String]>,
    /// The monthly spend cap, credits as a decimal string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub monthly_cap: Option<&'a str>,
    /// The whole CORS policy: an object, or `null`, which sets none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cors: Option<&'a Value>,
}

/// The body of [`ZONE_UPDATE`]: only the keys given.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ZoneUpdateBody<'a> {
    /// The zone's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<&'a str>,
    /// Whether plain HTTP is redirected to HTTPS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect_https: Option<bool>,
    /// The region the shield cache is in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shield_region: Option<&'a str>,
    /// The object served for a path that ends in `/`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_document: Option<&'a str>,
    /// Whether a missing path serves the index document, for a single-page app.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spa_fallback: Option<bool>,
    /// The paths the fallback leaves out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spa_fallback_exclude: Option<&'a [String]>,
    /// The whole CORS policy: an object, or `null`, which removes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cors: Option<&'a Value>,
    /// The new origin: what changes of it, and, for a prefix alone, the `kind` and `bucket`
    /// the zone has, so the PATCH is right whether the server merges `origin` or replaces it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin<'a>>,
}

/// The `origin` of a zone's body: an S3 bucket, and the prefix in it the zone serves.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Origin<'a> {
    /// The origin's kind: `s3`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<&'a str>,
    /// The bucket.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bucket: Option<&'a str>,
    /// The prefix served; the whole bucket without one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<&'a str>,
}

/// The body of [`CAP_UPDATE`]: the cap is the one writable field, and the server refuses any
/// other key.
#[derive(Debug, Serialize)]
pub struct CapUpdateBody<'a> {
    /// The monthly cap, credits as a decimal string.
    pub monthly_cap: &'a str,
}

/// The body of [`PURGE_CREATE`]: exactly one of the three.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PurgeBody<'a> {
    /// The whole zone: `{"all": true}`; `false` is never sent.
    All(bool),
    /// These paths.
    Paths(&'a [String]),
    /// Every path under these prefixes.
    Prefixes(&'a [String]),
}

/// The body of [`HOSTNAME_CREATE`].
#[derive(Debug, Serialize)]
pub struct HostnameCreateBody<'a> {
    /// The custom hostname.
    pub hostname: &'a str,
}

/// The body of [`HOSTNAME_DNS_SETUP`]: the provider and the credential, and nothing else
/// swagger offers. Its `Debug` leaves the credential out.
#[derive(Serialize)]
pub struct DnsSetupBody<'a> {
    /// The DNS provider's code.
    pub provider: &'a str,
    /// The credential's fields, or their placeholders in a dry run.
    pub credentials: &'a Value,
}

impl std::fmt::Debug for DnsSetupBody<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DnsSetupBody")
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}

/// The body of [`WAF_UPDATE`]: only the settings given.
#[derive(Debug, Default, Serialize)]
pub struct WafUpdateBody<'a> {
    /// What the WAF does with a request a rule matches.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<&'a str>,
    /// The rule set and price tier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<&'a str>,
    /// Whether matched requests are sampled as events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_sampling: Option<bool>,
    /// The page a blocked request gets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_response: Option<BlockResponse<'a>>,
}

/// The `block_response` of a [`WafUpdateBody`]: only the parts given.
#[derive(Debug, Default, Serialize)]
pub struct BlockResponse<'a> {
    /// The HTTP status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// The page's content type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<&'a str>,
    /// The page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<&'a str>,
}

/// The body of [`WAF_TEST`].
#[derive(Debug, Serialize)]
pub struct WafTestBody<'a> {
    /// A rule expression to test in place of the zone's rules.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression: Option<&'a str>,
    /// The sample request, as the file holds it.
    pub request: &'a Value,
}

/// The body of [`LIST_CREATE`]: the name and kind, then only what is given.
#[derive(Debug, Serialize)]
pub struct ListCreateBody<'a> {
    /// The list's name.
    pub name: &'a str,
    /// What its items are.
    pub kind: &'a str,
    /// What it is for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<&'a str>,
    /// Its first items.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<&'a [String]>,
}

/// The body of [`LIST_UPDATE`]: the description alone, since the PATCH would replace `items`
/// without the guard `items update` has.
#[derive(Debug, Serialize)]
pub struct ListUpdateBody<'a> {
    /// What the list is for.
    pub description: &'a str,
}

/// The body of [`LIST_ITEMS_ADD`], [`LIST_ITEMS_REMOVE`], and [`LIST_ITEMS_REPLACE`].
#[derive(Debug, Serialize)]
pub struct ItemsBody<'a> {
    /// The items.
    pub items: &'a [String],
}

/// A zone of [`ZONE`] or [`ZONE_CREATE`], as far as a create's warning and a prefix update
/// decide on it.
#[derive(Debug, Default, Deserialize)]
pub struct Zone {
    /// What the zone serves.
    #[serde(default, deserialize_with = "nested")]
    pub origin: Option<ZoneOrigin>,
}

/// The `origin` of a [`Zone`].
#[derive(Debug, Default, Deserialize)]
pub struct ZoneOrigin {
    /// The origin's kind.
    #[serde(default, deserialize_with = "field")]
    pub kind: Option<String>,
    /// The bucket.
    #[serde(default, deserialize_with = "field")]
    pub bucket: Option<String>,
    /// Whether the bucket was private, which the zone made public.
    #[serde(default, deserialize_with = "field")]
    pub private: Option<bool>,
}

/// The cap of [`CAP`], as far as an update decides on it.
#[derive(Debug, Default, Deserialize)]
pub struct Cap {
    /// The cap in credits, rounded to a few decimals: a string or a number.
    #[serde(default, deserialize_with = "text_or_number")]
    pub monthly_cap_credits: Option<String>,
    /// The cap exactly, in base units (`10^18` per credit), a decimal string.
    #[serde(default, deserialize_with = "field")]
    pub monthly_cap: Option<String>,
}

/// A purge of [`PURGE_CREATE`], as far as its note decides on it.
#[derive(Debug, Default, Deserialize)]
pub struct Purge {
    /// The purge's id, which [`PURGE`] takes.
    #[serde(default, deserialize_with = "field")]
    pub id: Option<u64>,
}

/// The rules of [`WAF_RULES`] or [`WAF_RATE_LIMITS`], as far as a guarded update decides on
/// them.
#[derive(Debug, Default, Deserialize)]
pub struct WafRules {
    /// The published version, which the update sends as `If-Match`.
    #[serde(default, deserialize_with = "field")]
    pub version: Option<u64>,
}

/// The answer of a rule list update's `?dry_run=1` check: `{valid, size, limit}`, where a
/// `rules` or a `version` would mean the list was published.
#[derive(Debug, Default, Deserialize)]
pub struct WafCheck {
    /// Whether the list is valid.
    #[serde(default, deserialize_with = "field")]
    pub valid: Option<bool>,
    /// The rules of a published list.
    #[serde(default)]
    pub rules: Slot<IgnoredAny>,
    /// The version of a published list.
    #[serde(default)]
    pub version: Slot<IgnoredAny>,
}

impl WafCheck {
    /// Whether the answer is a published list's: it has `rules` or `version`, even `null`.
    pub fn published(&self) -> bool {
        self.rules != Slot::Missing || self.version != Slot::Missing
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::view::view_of;

    /// A purge sends exactly one of its three keys.
    #[test]
    fn a_purge_body_sends_one_key() {
        let paths = ["/a".to_owned()];
        for (body, sent) in [
            (PurgeBody::All(true), r#"{"all":true}"#),
            (PurgeBody::Paths(&paths), r#"{"paths":["/a"]}"#),
            (PurgeBody::Prefixes(&paths), r#"{"prefixes":["/a"]}"#),
        ] {
            assert_eq!(serde_json::to_string(&body).unwrap(), sent);
        }
    }

    /// The DNS setup body's `Debug` names the provider, never the credential.
    #[test]
    fn a_dns_setup_body_debug_shows_no_credential() {
        let credentials = json!({ "api_token": "cf-secret" });
        let body = DnsSetupBody {
            provider: "cloudflare",
            credentials: &credentials,
        };
        let shown = format!("{body:?}");
        assert!(
            shown.contains("cloudflare") && !shown.contains("cf-secret"),
            "{shown}"
        );
    }

    /// A check's answer is a published list's when it has `rules` or `version`, even `null`.
    #[test]
    fn a_check_with_rules_or_a_version_was_published() {
        let published = |answer: Value| view_of::<WafCheck>(&answer).published();
        assert!(!published(json!({"valid": true, "size": 1})));
        assert!(published(json!({"valid": true, "rules": []})));
        assert!(published(json!({"valid": true, "version": null})));
    }
}
