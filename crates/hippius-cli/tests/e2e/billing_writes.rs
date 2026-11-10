//! `hipc billing profile get | update` and `hipc billing auto-reload update | card-setup`. The
//! profile update sends only the fields given; the auto-reload update reads the setting first,
//! sends it back whole, and asks only when it may charge the card more.

use serde_json::{Value, json};

use crate::common::{BILLING, Hipc, Home, MEMBER, Reply, Run, Server, draft, parsed, route};

const PROFILE: &str = "/api/billing/profile/";
const AUTO_RELOAD: &str = "/api/billing/auto-reload/";
const CARD_SETUP: &str = "/api/billing/auto-reload/card-setup/";

/// A saved profile, in the shape the console types it.
const SAVED_PROFILE: &str = concat!(
    r#"{"company_name":"Acme","address_line1":"1 Main St","address_line2":"","#,
    r#""city":"London","postal_code":"N1","region":"","country":"GB","tax_id":"GB123","#,
    r#""billing_email":"","updated_at":"2026-10-01T10:00:00Z"}"#
);

const CHECKOUT: &str = r#"{"checkout_url":"https://checkout.stripe.com/c/pay/cs_1?k=v"}"#;

fn hipc(server: &Server, args: &[&str]) -> Run {
    Hipc::against(&Home::new(), server).run(args)
}

fn methods(server: &Server) -> Vec<String> {
    server.requests().into_iter().map(|r| r.method).collect()
}

fn with_yes<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut argv = args.to_vec();
    argv.push("--yes");
    argv
}

/// A server whose `path` reads `answer` and that answers any PUT there with `answer`.
fn read_and_put(path: &str, answer: &str) -> Server {
    Server::start(vec![
        route("GET", path, Reply::json(200, answer)),
        route("PUT", path, Reply::json(200, answer)),
    ])
}

#[test]
fn profile_get_prints_the_profile_as_sent() {
    let server = Server::start(vec![route("GET", PROFILE, Reply::json(200, SAVED_PROFILE))]);
    assert_eq!(
        hipc(&server, &["billing", "profile", "get"]).ok(),
        parsed(SAVED_PROFILE)
    );
    assert_eq!(server.only_request().target, PROFILE);
    let run = hipc(&server, &["--output", "text", "billing", "profile", "get"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(run.stdout().contains("GB123"), "{}", run.stdout());
}

/// Swagger: a field the PUT leaves out keeps its value, so the update sends only the fields
/// given, with no read first; an empty value clears a field. It spends nothing, so it needs
/// no `--yes`.
#[test]
fn profile_update_sends_only_the_fields_given() {
    let server = Server::start(vec![route("PUT", PROFILE, Reply::json(200, SAVED_PROFILE))]);
    let args = [
        "billing",
        "profile",
        "update",
        "--city",
        "Paris",
        "--country",
        "fr",
        "--tax-id",
        "",
    ];
    assert_eq!(hipc(&server, &args).ok(), parsed(SAVED_PROFILE));
    let sent = server.only_request();
    assert_eq!(sent.method, "PUT");
    assert_eq!(sent.target, PROFILE);
    assert_eq!(
        sent.json(),
        json!({"city": "Paris", "country": "FR", "tax_id": ""})
    );
    // Every flag sets its own field. A new billing email reads the profile first and asks.
    let server = read_and_put(PROFILE, SAVED_PROFILE);
    let every = [
        "billing",
        "profile",
        "update",
        "--company-name",
        "B Ltd",
        "--address-line1",
        "a1",
        "--address-line2",
        "a2",
        "--city",
        "c",
        "--postal-code",
        "p",
        "--region",
        "r",
        "--country",
        "",
        "--tax-id",
        "t",
        "--billing-email",
        "e@example.com",
        "--yes",
    ];
    hipc(&server, &every).ok();
    assert_eq!(methods(&server), ["GET", "PUT"]);
    assert_eq!(
        server.requests()[1].json(),
        json!({
            "company_name": "B Ltd",
            "address_line1": "a1",
            "address_line2": "a2",
            "city": "c",
            "postal_code": "p",
            "region": "r",
            "country": "",
            "tax_id": "t",
            "billing_email": "e@example.com",
        })
    );
}

#[test]
fn profile_update_needs_a_field_and_a_country_code() {
    let server = Server::start(vec![]);
    let error = hipc(&server, &["billing", "profile", "update"]).fails(2, "usage");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("--company-name"),
        "{error}"
    );
    for country in ["FRA", "F", "1A", "é"] {
        let error = hipc(
            &server,
            &["billing", "profile", "update", "--country", country],
        )
        .fails(2, "usage");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("a country is a two-letter ISO 3166-1 code"),
            "{error}"
        );
    }
    assert!(server.requests().is_empty());
}

/// `hipc billing profile update --billing-email <to>`.
fn update(to: &str) -> Vec<&str> {
    vec!["billing", "profile", "update", "--billing-email", to]
}

/// A new billing email sends the invoices elsewhere, so the update reads the profile first and
/// asks unless it already holds that exact email. A profile that is not found, or holds no
/// email, counts as a change, but clearing an email it does not hold does not; any other
/// failed read stops the command before the PUT. The question names both addresses.
#[test]
fn a_billing_email_change_needs_yes() {
    let email = |address: &str| {
        let mut profile = parsed(SAVED_PROFILE);
        profile["billing_email"] = Value::from(address);
        profile.to_string()
    };
    // The same email: read, then sent, with no question.
    let server = read_and_put(PROFILE, &email("a@example.com"));
    hipc(&server, &update("a@example.com")).ok();
    assert_eq!(methods(&server), ["GET", "PUT"]);
    // Another email: refused after the read, then sent with `--yes`.
    let server = read_and_put(PROFILE, &email("a@example.com"));
    let error = hipc(&server, &update("b@example.com")).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        "this would change the billing email from `a@example.com` to `b@example.com`, and \
         `--yes` was not passed"
    );
    assert_eq!(methods(&server), ["GET"]);
    hipc(&server, &with_yes(&update("b@example.com"))).ok();
    assert_eq!(methods(&server), ["GET", "GET", "PUT"]);
    // Clearing the email asks too.
    let error = hipc(&server, &update("")).fails(2, "confirmation_required");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with("this would clear the billing email `a@example.com`"),
        "{error}"
    );
    // No profile yet, or one with no email: any email asks, an empty one does not.
    for answer in [
        Reply::not_found(),
        Reply::json(200, r#"{"billing_email":null}"#),
        Reply::json(200, r#"{"billing_email":""}"#),
    ] {
        let server = Server::start(vec![
            route("GET", PROFILE, answer),
            route("PUT", PROFILE, Reply::json(200, SAVED_PROFILE)),
        ]);
        let error = hipc(&server, &update("b@example.com")).fails(2, "confirmation_required");
        assert_eq!(
            error["message"],
            "this would set the billing email to `b@example.com`, and `--yes` was not passed"
        );
        hipc(&server, &update("")).ok();
        assert_eq!(methods(&server), ["GET", "GET", "PUT"]);
    }
    // A read that fails otherwise stops before the PUT, with nothing written.
    let server = Server::start(vec![route(
        "GET",
        PROFILE,
        Reply::json(503, r#"{"detail":"down"}"#),
    )]);
    let error = hipc(&server, &with_yes(&update("b@example.com"))).fails(1, "unavailable");
    assert_eq!(error["side_effects"], "none");
    assert_eq!(methods(&server), ["GET"]);
    // The dry run lists the read, then the PUT, and sends nothing.
    let server = Server::start(vec![]);
    let doc = Hipc::tokenless(&Home::new(), &server)
        .run(&[&["--dry-run"][..], &update("b@example.com")].concat())
        .ok();
    let shown: Vec<&str> = doc["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|request| request["method"].as_str().unwrap())
        .collect();
    assert_eq!(shown, ["GET", "PUT"]);
    assert!(server.requests().is_empty());
}

/// The dry run is the one PUT, with only the fields given. It needs no token and sends
/// nothing.
#[test]
fn profile_update_dry_run_is_one_put() {
    let server = Server::start(vec![]);
    let doc = Hipc::tokenless(&Home::new(), &server)
        .run(&[
            "--dry-run",
            "billing",
            "profile",
            "update",
            "--city",
            "Paris",
        ])
        .ok();
    assert_eq!(draft(&doc)["method"], "PUT");
    assert_eq!(draft(&doc)["url"], format!("{}{PROFILE}", server.url()));
    assert_eq!(draft(&doc)["body"], json!({"city": "Paris"}));
    let requests = doc["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(server.requests().is_empty());
}

/// A rejected write says what it did; one that may have reached the server, a 5xx, names the
/// read that shows whether it took.
#[test]
fn profile_update_reports_a_failed_write() {
    let args = ["billing", "profile", "update", "--city", "Paris"];
    let rejected = Server::start(vec![route(
        "PUT",
        PROFILE,
        Reply::json(400, r#"{"tax_id":["Enter a valid VAT number."]}"#),
    )]);
    let error = hipc(&rejected, &args).fails(1, "validation");
    assert_eq!(error["side_effects"], "none");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with("cannot update the billing profile's `city`"),
        "{error}"
    );
    assert!(
        !error["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("profile get")
    );
    let down = Server::start(vec![route("PUT", PROFILE, Reply::json(502, "{}"))]);
    let error = hipc(&down, &args).fails(1, "unavailable");
    assert_eq!(error["side_effects"], "partial");
    assert_eq!(
        error["hint"],
        "run `hipc billing profile get` to check whether it was saved before running the \
         command again"
    );
    let broken = Server::start(vec![route("PUT", PROFILE, Reply::json(500, "{}"))]);
    let error = hipc(&broken, &args).fails(1, "internal");
    assert_eq!(error["side_effects"], "partial");
    assert_eq!(
        error["hint"],
        "run `hipc billing profile get` to check whether it was saved before running the \
         command again"
    );
}

/// An auto-reload setting read, in the shape of swagger `auto_reload_get`: `cap` is a JSON
/// value, `null` for no monthly cap.
fn setting(enabled: bool, state: &str, threshold: &str, target: &str, cap: &str) -> String {
    format!(r#"{{"available":true,"can_edit":true,"enabled":{enabled},"threshold":"{threshold}","#)
        + &format!(r#""target":"{target}","monthly_cap":{cap},"state":"{state}","card":null,"#)
        + r#""month_spent":"0.00","consecutive_failures":0}"#
}

/// The usual setting: on and armed, below 10 back to 50, at most 200 a month.
fn armed() -> String {
    setting(true, "armed", "10.00", "50.00", r#""200.00""#)
}

fn reload(args: &[&str]) -> Vec<String> {
    ["billing", "auto-reload", "update"]
        .iter()
        .chain(args)
        .map(|w| (*w).to_owned())
        .collect()
}

fn words(argv: &[String]) -> Vec<&str> {
    argv.iter().map(String::as_str).collect()
}

/// Asserts that `args` over `answer` asks `what` after the read and sends nothing, then sends
/// `body` with `--yes`.
fn asks(answer: &str, args: &[&str], what: &str, body: &Value) {
    let server = read_and_put(AUTO_RELOAD, answer);
    let argv = reload(args);
    let error = hipc(&server, &words(&argv)).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!("this would {what}, and `--yes` was not passed"),
        "{answer} {args:?}"
    );
    assert_eq!(methods(&server), ["GET"], "{answer} {args:?}");
    hipc(&server, &with_yes(&words(&argv))).ok();
    assert_eq!(methods(&server), ["GET", "GET", "PUT"], "{answer} {args:?}");
    let sent = server.requests();
    assert_eq!(sent[2].target, AUTO_RELOAD);
    assert_eq!(&sent[2].json(), body, "{answer} {args:?}");
}

/// Asserts that `args` over `answer` sends `body` without `--yes`, after the read.
fn sends(answer: &str, args: &[&str], body: &Value) {
    let server = read_and_put(AUTO_RELOAD, answer);
    let doc = hipc(&server, &words(&reload(args))).ok();
    assert_eq!(doc, parsed(answer));
    assert_eq!(methods(&server), ["GET", "PUT"], "{answer} {args:?}");
    assert_eq!(&server.requests()[1].json(), body, "{answer} {args:?}");
}

/// The body a run sends; `cap` is `monthly_cap`, `Value::Null` for no cap.
fn body(enabled: bool, threshold: &str, target: &str, cap: impl Into<Value>) -> Value {
    let mut body = json!({"enabled": enabled, "threshold": threshold, "target": target});
    body["monthly_cap"] = cap.into();
    body
}

/// What the setting does once saved, as the question says it.
fn does(threshold: &str, target: &str, cap: &str) -> String {
    format!(
        "charge the card when the balance falls below `{threshold}` USD, back up to `{target}` \
         USD, with {cap}"
    )
}

/// Turning auto-reload on arms a recurring charge. A save while it is on checks the balance at
/// once (swagger), which may charge the card now, so a save in any state but `armed` asks too,
/// a lower amount included.
#[test]
fn auto_reload_update_needs_yes_to_turn_it_on_or_save_it_outside_armed() {
    let cap = "a monthly cap of `200.00` USD";
    asks(
        &setting(false, "off", "10.00", "50.00", r#""200.00""#),
        &["--enabled", "true"],
        &format!("turn on auto-reload, to {}", does("10.00", "50.00", cap)),
        &body(true, "10.00", "50.00", json!("200.00")),
    );
    for state in [
        "off",
        "disabled_failures",
        "retry_wait",
        "in_flight",
        "cooldown",
        "monthly_cap_reached",
    ] {
        asks(
            &setting(true, state, "10.00", "50.00", r#""200.00""#),
            &["--threshold", "5"],
            &format!(
                "save auto-reload on while its state is `{state}`, to {}",
                does("5.00", "50.00", cap)
            ),
            &body(true, "5.00", "50.00", json!("200.00")),
        );
    }
}

/// A higher threshold charges sooner, a higher target charges more each time, and a higher
/// cap, or none, lets more be charged each month. Amounts compare as cents.
#[test]
fn auto_reload_update_needs_yes_to_raise_an_amount() {
    let cases: [(&[&str], &str, Value); 6] = [
        (
            &["--threshold", "10.01"],
            &format!(
                "raise the auto-reload threshold, to {}",
                does("10.01", "50.00", "a monthly cap of `200.00` USD")
            ),
            body(true, "10.01", "50.00", json!("200.00")),
        ),
        (
            &["--target", "60"],
            &format!(
                "raise the auto-reload target, to {}",
                does("10.00", "60.00", "a monthly cap of `200.00` USD")
            ),
            body(true, "10.00", "60.00", json!("200.00")),
        ),
        (
            &["--monthly-cap", "200.1"],
            &format!(
                "raise the auto-reload monthly cap, to {}",
                does("10.00", "50.00", "a monthly cap of `200.10` USD")
            ),
            body(true, "10.00", "50.00", json!("200.10")),
        ),
        (
            &["--monthly-cap", "none"],
            &format!(
                "raise the auto-reload monthly cap, to {}",
                does("10.00", "50.00", "no monthly cap")
            ),
            body(true, "10.00", "50.00", Value::Null),
        ),
        (
            &["--threshold", "20", "--target", "100"],
            &format!(
                "raise the auto-reload threshold and target, to {}",
                does("20.00", "100.00", "a monthly cap of `200.00` USD")
            ),
            body(true, "20.00", "100.00", json!("200.00")),
        ),
        (
            &[
                "--enabled",
                "true",
                "--threshold",
                "20",
                "--target",
                "100",
                "--monthly-cap",
                "300",
            ],
            &format!(
                "raise the auto-reload threshold, target, and monthly cap, to {}",
                does("20.00", "100.00", "a monthly cap of `300.00` USD")
            ),
            body(true, "20.00", "100.00", json!("300.00")),
        ),
    ];
    for (args, what, sent) in cases {
        asks(&armed(), args, what, &sent);
    }
}

/// Fail closed: a setting in force that cannot be read as the fields compared asks, naming
/// the field and what it holds.
#[test]
fn auto_reload_update_needs_yes_when_the_setting_in_force_is_unknown() {
    let all = [
        "--enabled",
        "true",
        "--threshold",
        "5",
        "--target",
        "40",
        "--monthly-cap",
        "100",
    ];
    let sent = body(true, "5.00", "40.00", json!("100.00"));
    let unknown = |key: &str, shown: &str| {
        format!(
            "set auto-reload to {}, where the setting in force is unknown: its `{key}` is \
             {shown}",
            does("5.00", "40.00", "a monthly cap of `100.00` USD")
        )
    };
    let cases = [
        (
            r#"{"available":true}"#.to_owned(),
            unknown("enabled", "missing"),
        ),
        (
            armed().replace(r#""enabled":true"#, r#""enabled":"yes""#),
            unknown("enabled", "`yes`"),
        ),
        (
            armed().replace(r#""state":"armed""#, r#""state":"paused""#),
            unknown("state", "`paused`"),
        ),
        (
            armed().replace(r#","state":"armed""#, ""),
            unknown("state", "missing"),
        ),
        (
            armed().replace(r#""threshold":"10.00""#, r#""threshold":"10.000""#),
            unknown("threshold", "`10.000`"),
        ),
        (
            armed().replace(r#""threshold":"10.00""#, r#""threshold":null"#),
            unknown("threshold", "`null`"),
        ),
        (
            armed().replace(r#""target":"50.00""#, r#""target":50"#),
            unknown("target", "`50`"),
        ),
        (
            armed().replace(r#""monthly_cap":"200.00""#, r#""monthly_cap":"lots`""#),
            unknown("monthly_cap", "`lots\\u{60}`"),
        ),
        (
            armed().replace(r#""monthly_cap":"200.00","#, ""),
            unknown("monthly_cap", "missing"),
        ),
    ];
    for (answer, what) in cases {
        asks(&answer, &all, &what, &sent);
    }
}

/// Turning auto-reload off, leaving it off, or lowering or keeping every amount while it is on
/// charges no more, so it is sent without `--yes`, with the other fields as read.
#[test]
fn auto_reload_update_needs_no_yes_when_it_cannot_charge_more() {
    let cap = json!("200.00");
    sends(
        &armed(),
        &["--enabled", "false"],
        &body(false, "10.00", "50.00", cap.clone()),
    );
    // Off, whatever it was, even with a raise or an unreadable setting.
    sends(
        &armed(),
        &["--enabled", "false", "--target", "500"],
        &body(false, "10.00", "500.00", cap.clone()),
    );
    let all_off = [
        "--enabled",
        "false",
        "--threshold",
        "1",
        "--target",
        "2",
        "--monthly-cap",
        "none",
    ];
    sends(
        r#"{"enabled":"unknown"}"#,
        &all_off,
        &body(false, "1.00", "2.00", Value::Null),
    );
    sends(
        &setting(false, "off", "10.00", "50.00", "null"),
        &["--threshold", "99"],
        &body(false, "99.00", "50.00", Value::Null),
    );
    sends(
        &armed(),
        &[
            "--threshold",
            "9.99",
            "--target",
            "50",
            "--monthly-cap",
            "200",
        ],
        &body(true, "9.99", "50.00", cap.clone()),
    );
    // A cap where there was none limits the charges.
    sends(
        &setting(true, "armed", "10.00", "50.00", "null"),
        &["--monthly-cap", "1000"],
        &body(true, "10.00", "50.00", json!("1000.00")),
    );
    sends(
        &setting(true, "armed", "10.00", "50.00", "null"),
        &["--monthly-cap", "none"],
        &body(true, "10.00", "50.00", Value::Null),
    );
    sends(
        &setting(true, "armed", "10", "50.5", r#""200""#),
        &["--target", "50.50", "--enabled", "true"],
        &body(true, "10", "50.50", json!("200")),
    );
}

/// A field the read does not hold in the shape the route takes, such as the threshold of a
/// setting never saved, cannot be sent back unchanged: the caller gives it. One error names
/// every such field.
#[test]
fn auto_reload_update_refuses_the_fields_the_read_does_not_hold() {
    let cases = [
        (
            r#"{"available":true}"#.to_owned(),
            &["--threshold", "5"][..],
            "`enabled`, `target`, or `monthly_cap`",
            "`--enabled`, `--target`, and `--monthly-cap`",
        ),
        (
            r#"{"available":true,"enabled":false,"monthly_cap":null}"#.to_owned(),
            &["--enabled", "false"],
            "`threshold` or `target`",
            "`--threshold` and `--target`",
        ),
        (
            setting(false, "off", "10.00", "50.00", "null")
                .replace(r#""target":"50.00""#, r#""target":null"#),
            &["--enabled", "false"],
            "`target`",
            "`--target`",
        ),
        (
            armed().replace(r#""monthly_cap":"200.00""#, r#""monthly_cap":5"#),
            &["--enabled", "false"],
            "`monthly_cap`",
            "`--monthly-cap`",
        ),
        (
            armed().replace(r#""enabled":true"#, r#""enabled":null"#),
            &["--target", "5"],
            "`enabled`",
            "`--enabled`",
        ),
    ];
    for (answer, args, keys, flags) in cases {
        let server = read_and_put(AUTO_RELOAD, &answer);
        let error = hipc(&server, &with_yes(&words(&reload(args)))).fails(2, "usage");
        assert_eq!(
            error["message"],
            format!("the auto-reload setting holds no {keys} hipc can send back unchanged"),
            "{answer}"
        );
        let them = if flags.contains(" and ") {
            "them"
        } else {
            "it"
        };
        assert_eq!(error["hint"], format!("pass {flags} to set {them}"));
        assert_eq!(methods(&server), ["GET"], "{answer}");
    }
}

/// Swagger: the setting reads `available: false`, and nothing else, while the feature is off
/// on the server. No flag fixes that, so it is refused before any field is checked, with every
/// field given or none.
#[test]
fn auto_reload_update_refuses_when_the_feature_is_off() {
    for args in [
        &["--enabled", "false"][..],
        &[
            "--enabled",
            "true",
            "--threshold",
            "1",
            "--target",
            "60",
            "--monthly-cap",
            "none",
        ],
    ] {
        let server = read_and_put(AUTO_RELOAD, r#"{"available":false}"#);
        let error = hipc(&server, &with_yes(&words(&reload(args)))).fails(1, "conflict");
        assert_eq!(
            error["message"],
            "auto-reload is not available on this server: the setting reads `available` `false`"
        );
        assert_eq!(error["http_status"], Value::Null);
        assert_eq!(error["side_effects"], "none");
        assert_eq!(error["hint"], Value::Null);
        assert_eq!(methods(&server), ["GET"]);
    }
}

#[test]
fn auto_reload_update_needs_a_field_and_amounts_to_the_cent() {
    let server = Server::start(vec![]);
    let error = hipc(&server, &["billing", "auto-reload", "update"]).fails(2, "usage");
    assert!(
        error["message"].as_str().unwrap().contains("--enabled"),
        "{error}"
    );
    for args in [
        &["--threshold", "10.001"][..],
        &["--target", "-5"],
        &["--monthly-cap", "lots"],
        &["--monthly-cap", "1.234"],
        &["--enabled", "yes"],
    ] {
        hipc(&server, &words(&reload(args))).fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

/// The dry run lists the read, then the write, whose fields from the read are placeholders.
/// It needs no token and no `--yes`, and sends nothing.
#[test]
fn auto_reload_update_dry_run_lists_the_read_then_the_write() {
    let server = Server::start(vec![]);
    let update = reload(&["--enabled", "true", "--monthly-cap", "none"]);
    let mut argv = vec!["--dry-run"];
    argv.extend(words(&update));
    let doc = Hipc::tokenless(&Home::new(), &server).run(&argv).ok();
    let expected = json!({
        "enabled": true,
        "threshold": "<the setting's threshold>",
        "target": "<the setting's target>",
        "monthly_cap": null,
    });
    assert_eq!(draft(&doc)["method"], "PUT");
    assert_eq!(draft(&doc)["body"], expected);
    let requests = doc["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["method"], "GET");
    assert_eq!(requests[0]["url"], format!("{}{AUTO_RELOAD}", server.url()));
    assert_eq!(requests[1]["body"], expected);
    assert!(server.requests().is_empty());
}

#[test]
fn auto_reload_update_stops_on_a_failed_read_and_reports_a_rejected_write() {
    let off = Server::start(vec![route("GET", AUTO_RELOAD, Reply::not_found())]);
    let args = reload(&["--enabled", "false"]);
    hipc(&off, &with_yes(&words(&args))).fails(1, "not_found");
    assert_eq!(methods(&off), ["GET"]);
    let refused = Server::start(vec![
        route("GET", AUTO_RELOAD, Reply::json(200, &armed())),
        route(
            "PUT",
            AUTO_RELOAD,
            Reply::json(409, r#"{"error":"no wallet","code":"no_wallet"}"#),
        ),
    ]);
    let error = hipc(&refused, &words(&args)).fails(1, "conflict");
    assert_eq!(error["side_effects"], "none");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with("cannot save the auto-reload setting"),
        "{error}"
    );
    // A confirmed write that the server refuses names the action asked about.
    let raise = reload(&["--target", "60"]);
    let error = hipc(&refused, &with_yes(&words(&raise))).fails(1, "conflict");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with("cannot raise the auto-reload target"),
        "{error}"
    );
    assert!(
        !error["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("auto-reload get")
    );
    // A write that may have reached the server names the read that shows whether it took.
    let down = Server::start(vec![
        route("GET", AUTO_RELOAD, Reply::json(200, &armed())),
        route("PUT", AUTO_RELOAD, Reply::json(503, "{}")),
    ]);
    let error = hipc(&down, &words(&args)).fails(1, "unavailable");
    assert_eq!(error["side_effects"], "partial");
    assert_eq!(
        error["hint"],
        "run `hipc billing auto-reload get` to check whether it was saved before running the \
         command again"
    );
}

/// A member without a billing account is refused before anything is sent, a dry run included,
/// as a top-up is: the routes are not known to honour the account header alone, and an update
/// could change the caller's own profile or wallet.
#[test]
fn a_membership_needs_a_billing_account() {
    let server = Server::start(vec![]);
    for (args, what) in [
        (&["billing", "profile", "get"][..], "a billing profile read"),
        (
            &["billing", "profile", "update", "--city", "Paris"],
            "a billing profile update",
        ),
        (
            &["billing", "auto-reload", "update", "--enabled", "false"],
            "an auto-reload update",
        ),
        (&["billing", "auto-reload", "card-setup"], "a card setup"),
        (&["billing", "auto-reload", "get"], "an auto-reload read"),
        (
            &["billing", "auto-reload", "history"],
            "an auto-reload history read",
        ),
    ] {
        for dry in [false, true] {
            let mut argv = if dry { vec!["--dry-run"] } else { vec![] };
            argv.extend(args);
            let error = Hipc::against(&Home::new(), &server)
                .env("HIPPIUS_ACCOUNT", MEMBER)
                .run(&argv)
                .fails(2, "account_scope_unsupported");
            let message = error["message"].as_str().unwrap();
            assert!(
                message.starts_with(&format!(
                    "{what} in another account needs a billing account from `--billing`, \
                     `HIPPIUS_BILLING`, or `hipc account billing-accounts use`"
                )),
                "{message}"
            );
            assert!(error["hint"].as_str().unwrap().contains("--account self"));
        }
    }
    assert!(server.requests().is_empty());
}

/// The card setup prints the checkout URL and does not wait, charges nothing, and so needs no
/// `--yes`. `next_step` names the read that shows the card, with the run's selectors.
#[test]
fn card_setup_prints_the_checkout_and_names_the_next_step() {
    let server = Server::start(vec![route("POST", CARD_SETUP, Reply::json(200, CHECKOUT))]);
    let doc = hipc(&server, &["billing", "auto-reload", "card-setup"]).ok();
    let next = format!("hipc --api-url {} billing auto-reload get", server.url());
    assert_eq!(doc["checkout_url"], parsed(CHECKOUT)["checkout_url"]);
    assert_eq!(doc["next_step"], next.as_str());
    let sent = server.only_request();
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.json(), json!({}));
    let run = hipc(
        &server,
        &["--output", "text", "billing", "auto-reload", "card-setup"],
    );
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        format!(
            "note: nothing is charged: the card is saved once the checkout at `checkout_url` in \
             the output is completed; `{next}` then shows the card\n"
        )
    );
    // The billing account goes on the request and into the next step.
    let doc = hipc(
        &server,
        &["--billing", BILLING, "billing", "auto-reload", "card-setup"],
    )
    .ok();
    assert_eq!(
        doc["next_step"],
        format!(
            "hipc --api-url {} --billing {BILLING} billing auto-reload get",
            server.url()
        )
    );
    assert_eq!(
        server.requests()[2].header("x-hippius-billing"),
        Some(BILLING)
    );
}

/// A selector that is not one plain word gives no command to name, and an answer that is not
/// an object gains no field.
#[test]
fn card_setup_leaves_out_a_next_step_it_cannot_name() {
    let server = Server::start(vec![route("POST", CARD_SETUP, Reply::json(200, CHECKOUT))]);
    let home = Home::new();
    home.write_config("[profiles.\"-work\"]\n");
    let run = Hipc::against(&home, &server)
        .env("HIPPIUS_PROFILE", "-work")
        .run(&["--output", "text", "billing", "auto-reload", "card-setup"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        "note: nothing is charged: the card is saved once the checkout at `checkout_url` in the \
         output is completed\n"
    );
    let doc = Hipc::against(&home, &server)
        .env("HIPPIUS_PROFILE", "-work")
        .run(&["billing", "auto-reload", "card-setup"])
        .ok();
    assert!(doc.get("next_step").is_none(), "{doc}");
    let list = Server::start(vec![route("POST", CARD_SETUP, Reply::json(200, "[]"))]);
    assert_eq!(
        hipc(&list, &["billing", "auto-reload", "card-setup"]).ok(),
        json!([])
    );
    let failing = Server::start(vec![route(
        "POST",
        CARD_SETUP,
        Reply::json(502, r#"{"error":"stripe is down"}"#),
    )]);
    let error = hipc(&failing, &["billing", "auto-reload", "card-setup"]).fails(1, "unavailable");
    assert_eq!(error["side_effects"], "partial");
}

/// Every new route takes the billing account (tenet 12): it goes on each request, the read
/// before a write included, and on each request of a dry run. A membership sends the account
/// header too.
#[test]
fn the_billing_account_goes_on_every_request() {
    let server = Server::start(vec![
        route("GET", PROFILE, Reply::json(200, SAVED_PROFILE)),
        route("PUT", PROFILE, Reply::json(200, SAVED_PROFILE)),
        route("GET", AUTO_RELOAD, Reply::json(200, &armed())),
        route("PUT", AUTO_RELOAD, Reply::json(200, &armed())),
        route("POST", CARD_SETUP, Reply::json(200, CHECKOUT)),
        route(
            "GET",
            "/api/billing/auto-reload/history/",
            Reply::json(200, r#"{"results":[]}"#),
        ),
    ]);
    let commands: [&[&str]; 6] = [
        &["billing", "profile", "get"],
        &["billing", "profile", "update", "--city", "Paris"],
        &["billing", "auto-reload", "update", "--enabled", "false"],
        &["billing", "auto-reload", "card-setup"],
        &["billing", "auto-reload", "get"],
        &["billing", "auto-reload", "history"],
    ];
    for args in commands {
        let mut dry = vec!["--billing", BILLING, "--dry-run"];
        dry.extend(args);
        let doc = Hipc::against(&Home::new(), &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&dry)
            .ok();
        assert_eq!(draft(&doc)["billing_header"], BILLING, "{args:?}");
        assert_eq!(draft(&doc)["account_header"], MEMBER, "{args:?}");
        for request in doc["requests"].as_array().unwrap() {
            assert_eq!(request["billing_header"], BILLING, "{args:?}");
        }
        Hipc::against(&Home::new(), &server)
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .env("HIPPIUS_BILLING", BILLING)
            .run(args)
            .ok();
    }
    let sent = server.requests();
    assert_eq!(sent.len(), 7);
    for request in sent {
        assert_eq!(
            request.header("x-hippius-billing"),
            Some(BILLING),
            "{}",
            request.target
        );
        assert_eq!(
            request.header("x-hippius-account"),
            Some(MEMBER),
            "{}",
            request.target
        );
    }
    // A project is refused on every one, with nothing sent.
    let none = Server::start(vec![]);
    for args in commands {
        let mut argv = vec!["--project", "proj_0a1b2c3d4e5f60718293"];
        argv.extend(args);
        hipc(&none, &argv).fails(2, "project_scope_unsupported");
    }
    assert!(none.requests().is_empty());
}
