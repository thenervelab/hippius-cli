use super::*;
use crate::account::{BILLING_NAMED_ONLY_PATHS, PERSONAL_PATHS, listed_class};
use crate::project::{BILLING_PATHS, PROJECT_EXCLUDED_PATHS, listed_billing, listed_project};

/// A path that fills each placeholder of `endpoint` with a value the policy tables read as
/// its kind: a project or billing account id where the template names one, else digits,
/// which every `{name}` and `{n}` of the tables matches.
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

/// What the header tables say of `path`, a path of `endpoint`, where they list it, side by
/// side with what the row says, as `(what, the row, the tables)`. A path the tables do not
/// list is the row's alone to decide, such as a new API version's.
fn listed_decisions(
    endpoint: &Endpoint,
    path: &str,
) -> Vec<(&'static str, String, Option<String>)> {
    let method = endpoint.method;
    vec![
        (
            "account class",
            format!("{:?}", endpoint.account()),
            listed_class(method, path).map(|class| format!("{class:?}")),
        ),
        (
            "account beside a billing account",
            endpoint.account_beside_billing.to_string(),
            BILLING_NAMED_ONLY_PATHS
                .contains(&path)
                .then(|| true.to_string()),
        ),
        (
            "project",
            format!("{:?}", endpoint.project),
            listed_project(method, path).map(|take| format!("{take:?}")),
        ),
        (
            "billing account",
            format!("{:?}", endpoint.billing),
            listed_billing(method, path).map(|take| format!("{take:?}")),
        ),
    ]
}

/// The paths the header tables name one by one, with no placeholder. A row whose
/// placeholder takes one of them decides it, so it must decide it as the tables do: were a
/// database row's `{database_id}` to take any segment, not digits alone, it would take
/// `/api/databases/metrics/`, a personal route.
fn literal_table_paths() -> impl Iterator<Item = &'static str> {
    PERSONAL_PATHS
        .iter()
        .chain(BILLING_NAMED_ONLY_PATHS)
        .chain(PROJECT_EXCLUDED_PATHS)
        .chain(BILLING_PATHS)
        .copied()
        .filter(|path| !path.contains('{'))
}

/// The header tables decide a request only when its path has no row, and they also decide
/// the other methods of a row's path, so where they list a path a row matches, its own or a
/// literal path a placeholder takes, they say what the row says: the account class, the
/// recurring card rule, and the project and billing handling. Each disagreement is listed,
/// so one run shows them all.
#[test]
fn the_header_tables_never_contradict_a_row() {
    let mut disagreements = Vec::new();
    for endpoint in all() {
        let own = sample(endpoint);
        let literal =
            literal_table_paths().filter(|path| row(endpoint.method, path) == Some(endpoint));
        let paths: Vec<String> = std::iter::once(own)
            .chain(literal.map(str::to_owned))
            .collect();
        for path in &paths {
            for (what, row, tables) in listed_decisions(endpoint, path) {
                if tables.as_ref().is_some_and(|tables| *tables != row) {
                    disagreements.push(format!(
                        "{} {path} ({}): {what} is {row} in the row and {} in the tables",
                        endpoint.method,
                        endpoint.template,
                        tables.unwrap_or_default()
                    ));
                }
            }
        }
    }
    assert!(disagreements.is_empty(), "{disagreements:#?}");
}

/// The policy of a row, without its method, template, and paging, which no request decision
/// reads.
fn policy(endpoint: &Endpoint) -> Endpoint {
    Endpoint {
        method: Method::Get,
        template: "",
        paging: None,
        ..*endpoint
    }
}

/// Each row is the row its own path reaches, or one with the same policy where two templates
/// match one path (a Hub model and a model with a reference, both under `{model..}`).
#[test]
fn every_row_is_reached_by_its_own_path() {
    for endpoint in all() {
        let path = sample(endpoint);
        let found = row(endpoint.method, &path).expect("a row matches its own path");
        assert!(
            found == endpoint || policy(found) == policy(endpoint),
            "{} {path} reaches {} instead of {}",
            endpoint.method,
            found.template,
            endpoint.template
        );
        // Slashless, as a server or a proxy that adds the slash delivers it.
        let slashless = path.strip_suffix('/').expect("a template ends with `/`");
        assert_eq!(row(endpoint.method, slashless), Some(found), "{slashless}");
    }
}

/// A literal segment beats a placeholder, as the server's router tries a literal path first,
/// a `HEAD` reaches the `GET` row, and a path or a method no row has reaches none.
#[test]
fn the_most_literal_row_wins_and_a_head_reaches_the_get() {
    let refusals = row(Method::Get, "/api/databases/refusals/").expect("a row");
    assert_eq!(refusals, databases::DATABASE_REFUSALS.endpoint());
    let one = row(Method::Get, "/api/databases/7/").expect("a row");
    assert_eq!(one, databases::DATABASE.endpoint());
    assert_eq!(
        row(Method::Head, "/api/network/me/"),
        Some(network::ME.endpoint())
    );
    assert_eq!(row(Method::Get, "/api/v2/compute/vms/"), None);
    assert_eq!(row(Method::Put, "/api/compute/vms/"), None);
    assert_eq!(
        matching("/api/compute/vms/")
            .map(|endpoint| endpoint.method)
            .collect::<Vec<_>>(),
        [Method::Get, Method::Post]
    );
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
    assert_eq!(ONE.path([&"sha256:0a+b=c@d"]), "/api/a/sha256:0a+b=c@d/b/");
    assert_eq!(ONE.path([&"a;b?c#d%e"]), "/api/a/a%3Bb%3Fc%23d%25e/b/");
    assert_eq!(TWO.path([&"team/app", &"v1"]), "/api/r/team/app/t/v1/");
    assert_eq!(TWO.path([&"a b/c", &"x/y"]), "/api/r/a%20b/c/t/x%2Fy/");
    assert_eq!(TWO.path([&"a:b/c", &"x"]), "/api/r/a:b/c/t/x/");
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
    for template in ["/api/", "/apix/", "/api/a", "/ap/a/", "api/a/", "/xpi/a/"] {
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
    assert!(
        !endpoint.undone_on_503 && endpoint.paging.is_none() && !endpoint.account_beside_billing
    );
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
        .paged(Paging::Cursor)
        .guard(Guard::Refused {
            why: "why",
            hint: "hint",
        });
    let endpoint = flagged.endpoint();
    assert_eq!(
        (endpoint.project, endpoint.billing),
        (Take::Send, Take::Send)
    );
    assert!(endpoint.reveals && endpoint.provisions && endpoint.permanent);
    assert!(
        endpoint.undone_on_503 && endpoint.paging().is_some() && endpoint.account_beside_billing
    );
    assert_eq!(endpoint.paging(), Some(Paging::Cursor));
    assert_eq!(
        endpoint.guard(),
        Guard::Refused {
            why: "why",
            hint: "hint"
        }
    );
    assert!(endpoint.reveals() && endpoint.provisions());
    assert!(endpoint.permanent() && endpoint.undone_on_503());
    assert!(!Guard::None.lends() && !Guard::Needs(&[]).lends());
    assert!(Guard::Needs(&[RouteFlag::Public]).lends());
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

/// Each paging style names the queries its pages are asked for with, and says whether its
/// pages are numbered.
#[test]
fn each_paging_style_names_its_queries() {
    let styles = [
        (Paging::Numbered, true, "page", Some("page_size")),
        (
            Paging::Totalled { items: "artifacts" },
            true,
            "page",
            Some("page_size"),
        ),
        (Paging::Cursor, false, "before", None),
        (
            Paging::Continuation,
            false,
            "continuation_token",
            Some("max_keys"),
        ),
    ];
    for (paging, numbered, token, size) in styles {
        assert_eq!(
            (paging.numbered(), paging.token_query(), paging.size_query()),
            (numbered, token, size),
            "{paging:?}"
        );
    }
}
