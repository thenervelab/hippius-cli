//! The CDN: zones, hostnames, caching rules, purges, the cap, the WAF, and IP lists.

use crate::routes::{Guard, Route, RouteFlag};

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
