use super::*;
use crate::account::{
    RouteRule, names_billing_only, not_done_on_503, permanent, provisions, returns_secret,
    route_class, route_rules,
};
use crate::project::{billing_take, project_take};

/// A path that fills each placeholder of `endpoint` with a value the policy tables read as
/// its kind: a project or billing account id where the template names one, else digits,
/// which every `{id}` and `{n}` of the tables matches.
fn sample(endpoint: &Endpoint) -> String {
    endpoint.sample(|name| {
        match name {
            "project_id" => "proj_0a1b2c3d4e5f60718293",
            "billing_account_id" => "bill_9f8e7d6c5b4a39281706",
            _ => "7",
        }
        .to_owned()
    })
}

/// The kind of what `route_rules` says of a request, as a row writes it.
fn guard_of(rules: &[RouteRule]) -> Guard {
    match rules {
        [] => Guard::None,
        [rule] => match *rule {
            RouteRule::Refused { .. } => Guard::Refused,
            RouteRule::RefusedWhenSet { key, .. } => Guard::RefusedWhenSet(key),
            RouteRule::Needs(flags) => Guard::Needs(flags),
            RouteRule::NeedsWhenSet(flags, key) => Guard::NeedsWhenSet(flags, key),
            RouteRule::NeedsUnlessFalse(flags, key) => Guard::NeedsUnlessFalse(flags, key),
        },
        more => panic!("a path without a format suffix has at most one rule: {more:?}"),
    }
}

/// What the policy tables decide for one row, side by side with what the row says, as
/// `(what, the row, the tables)`.
fn decisions(endpoint: &Endpoint) -> Vec<(&'static str, String, String)> {
    let (method, path) = (endpoint.method, sample(endpoint));
    let account = endpoint
        .account
        .expect("`routes!` checks that each row has a class");
    vec![
        (
            "account class",
            format!("{account:?}"),
            format!("{:?}", route_class(method, &path)),
        ),
        (
            "account beside a billing account",
            endpoint.account_beside_billing.to_string(),
            names_billing_only(&path).to_string(),
        ),
        (
            "project",
            format!("{:?}", endpoint.project),
            format!("{:?}", project_take(method, &path)),
        ),
        (
            "billing account",
            format!("{:?}", endpoint.billing),
            format!("{:?}", billing_take(method, &path)),
        ),
        (
            "reveals a secret",
            endpoint.reveals.to_string(),
            returns_secret(method, &path).to_string(),
        ),
        (
            "provisions",
            endpoint.provisions.to_string(),
            provisions(method, &path).to_string(),
        ),
        (
            "permanent",
            endpoint.permanent.to_string(),
            permanent(method, &path).to_string(),
        ),
        (
            "undone on a 503",
            endpoint.undone_on_503.to_string(),
            (method == Method::Post && not_done_on_503(&path)).to_string(),
        ),
        (
            "`hipc api` guard",
            format!("{:?}", endpoint.guard),
            format!("{:?}", guard_of(&route_rules(method, &path))),
        ),
    ]
}

/// Every row says what the policy tables decide for its path: the header classes, the
/// secret, provisioning, and permanent flags, the 503 rule, and `hipc api`'s guard. Each
/// disagreement is listed, so one run shows them all.
#[test]
fn every_row_agrees_with_the_policy_tables() {
    let mut disagreements = Vec::new();
    for endpoint in all() {
        for (what, row, tables) in decisions(endpoint) {
            if row != tables {
                disagreements.push(format!(
                    "{} {}: {what} is {row} in the row and {tables} in the tables",
                    endpoint.method, endpoint.template
                ));
            }
        }
    }
    assert!(disagreements.is_empty(), "{disagreements:#?}");
}

/// A method and template appear in one row only, so a command and a test that name a route
/// cannot reach two policies.
#[test]
fn no_route_has_two_rows() {
    let mut seen = std::collections::BTreeSet::new();
    for endpoint in all() {
        assert!(
            seen.insert((endpoint.method.as_str(), endpoint.template)),
            "{} {} has two rows",
            endpoint.method,
            endpoint.template
        );
    }
}

#[test]
fn placeholders_fill_one_segment_each_percent_encoded() {
    const ONE: Route<1> = Route::get("/api/a/{id}/b/").own_account();
    const TWO: Route<2> = Route::delete("/api/r/{repository..}/t/{tag}/").own_account();
    assert_eq!(ONE.path([&7]), "/api/a/7/b/");
    assert_eq!(ONE.path([&"x/../y z"]), "/api/a/x%2F..%2Fy%20z/b/");
    assert_eq!(TWO.path([&"team/app", &"v1"]), "/api/r/team/app/t/v1/");
    assert_eq!(TWO.path([&"a b/c", &"x/y"]), "/api/r/a%20b/c/t/x%2Fy/");
    assert_eq!(ONE.method(), Method::Get);
    assert_eq!(ONE.endpoint().template(), "/api/a/{id}/b/");
}

#[test]
fn a_template_matches_paths_with_a_value_in_each_placeholder() {
    const TWO: Route<2> = Route::delete("/api/r/{repository..}/t/{tag}/").own_account();
    let endpoint = TWO.endpoint();
    for (path, matches) in [
        ("/api/r/app/t/v1/", true),
        ("/api/r/team/app/t/v1/", true),
        ("/api/r/t/v1/", false),
        ("/api/r//t/v1/", false),
        ("/api/r/app/t//", false),
        ("/api/r/app/t/v1/x/", false),
        ("/api/r/app/u/v1/", false),
        ("/api/r/app/t/v1", false),
    ] {
        assert_eq!(endpoint.matches(path), matches, "{path}");
    }
}

#[test]
fn a_template_counts_only_whole_segment_placeholders() {
    assert_eq!(placeholders("/api/a/"), 0);
    assert_eq!(placeholders("/api/a/{id}/b/{n..}/"), 2);
    assert_eq!(placeholders("{id}/"), usize::MAX);
    assert_eq!(placeholders("/api/a{id}/"), usize::MAX);
    assert_eq!(placeholders("/api/{id}x/"), usize::MAX);
    assert_eq!(placeholders("/api/{id}"), usize::MAX);
    assert!(starts_with_base("/api/a/"));
    for template in ["/api/", "/apix/", "/api/a", "/ap/a/", "api/a/"] {
        assert!(!starts_with_base(template), "{template}");
    }
}

/// Each builder method sets its one part of the policy, and a route starts refusing every
/// selector and needing nothing beyond `--yes`. Run, not only evaluated as constants, so each
/// method's body runs under the coverage gate.
#[test]
fn each_builder_method_sets_its_part_of_the_policy() {
    let plain = Route::<0>::get("/api/a/").own_account().declared();
    let endpoint = plain.endpoint();
    assert_eq!(endpoint.account, Some(RouteClass::Unsupported));
    assert_eq!(
        (endpoint.project, endpoint.billing),
        (Take::Refuse, Take::Refuse)
    );
    assert!(!endpoint.reveals && !endpoint.provisions && !endpoint.permanent);
    assert!(!endpoint.undone_on_503 && !endpoint.paged && !endpoint.account_beside_billing);
    assert_eq!(endpoint.guard, Guard::None);

    let personal = Route::<0>::put("/api/a/").personal();
    assert_eq!(personal.endpoint().account, Some(RouteClass::Personal));
    assert_eq!(
        (personal.endpoint().project, personal.endpoint().billing),
        (Take::Skip, Take::Skip)
    );
    let named = Route::<1>::patch("/api/a/{id}/").scoped().names_one();
    assert_eq!(named.endpoint().account, Some(RouteClass::Scoped));
    assert_eq!(
        (named.endpoint().project, named.endpoint().billing),
        (Take::Named, Take::Named)
    );
    let flagged = Route::<0>::post("/api/a/")
        .own_account()
        .beside_billing()
        .project()
        .billing()
        .reveals()
        .provisions()
        .permanent()
        .undone_on_503()
        .paged()
        .guard(Guard::Refused);
    let endpoint = flagged.endpoint();
    assert_eq!(
        (endpoint.project, endpoint.billing),
        (Take::Send, Take::Send)
    );
    assert!(endpoint.reveals && endpoint.provisions && endpoint.permanent);
    assert!(endpoint.undone_on_503 && endpoint.paged() && endpoint.account_beside_billing);
    assert_eq!(endpoint.guard, Guard::Refused);
    assert_eq!(Route::<0>::delete("/api/a/").method(), Method::Delete);
}

/// A route that never says its class is refused, which stops the build of a constant.
#[test]
#[should_panic(expected = "a route says whether it is scoped")]
fn a_route_without_a_class_is_refused() {
    let _unclassed = Route::<0>::get("/api/a/").declared();
}

/// A template whose placeholders the type does not count, or that is outside the base, is
/// refused.
#[test]
#[should_panic(expected = "a route's type counts the placeholders of its template")]
fn a_route_with_the_wrong_count_is_refused() {
    let _miscounted = Route::<0>::get("/api/a/{id}/");
}

#[test]
#[should_panic(expected = "a route starts with `/api/`")]
fn a_route_outside_the_base_is_refused() {
    let _outside = Route::<0>::get("/v2/a/");
}
