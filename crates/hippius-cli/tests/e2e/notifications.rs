//! `hipc account notifications update` and `hipc account marketing-consent get | update`: the
//! signed-in user's own email settings, on personal routes, so a membership sends no header.
//! Neither write spends anything; a notifications update that turns an alert off asks.

use serde_json::{Value, json};

use crate::common::{Hipc, Home, MEMBER, Reply, Run, Server, draft, parsed, route};

const SETTINGS: &str = "/api/notifications/settings/";
const CONSENT: &str = "/api/notifications/marketing-consent/";

/// The settings in the shape of swagger `NotificationPreferences`.
const SAVED: &str = concat!(
    r#"{"email_enabled":true,"low_credit_alerts":true,"zero_balance_alerts":true,"#,
    r#""file_status_updates":false,"marketing_emails":false,"provider_advice_emails":true}"#
);

/// The consent in the shape of swagger `MarketingConsentState`.
const WITHDRAWN: &str = r#"{"state":"no","receives_marketing":false}"#;
const GIVEN: &str = r#"{"state":"yes","receives_marketing":true}"#;

fn hipc(server: &Server, args: &[&str]) -> Run {
    let mut argv = vec!["account"];
    argv.extend(args);
    Hipc::against(&Home::new(), server).run(&argv)
}

/// The update is one `PATCH` of only the settings given, with no read first and no
/// confirmation, and prints the settings the server answers with. A membership sends no
/// header.
#[test]
fn notifications_update_patches_only_the_settings_given() {
    let server = Server::start(vec![route("PATCH", SETTINGS, Reply::json(200, SAVED))]);
    let doc = hipc(
        &server,
        &[
            "notifications",
            "update",
            "--low-credit-alerts",
            "true",
            "--marketing-emails=false",
            "--account",
            MEMBER,
        ],
    )
    .ok();
    assert_eq!(doc, parsed(SAVED));
    let sent = server.only_request();
    assert_eq!(sent.method, "PATCH");
    assert_eq!(sent.target, SETTINGS);
    assert_eq!(
        sent.json(),
        json!({"low_credit_alerts": true, "marketing_emails": false})
    );
    assert_eq!(sent.header("x-hippius-account"), None);

    // Every flag sets its own setting.
    let server = Server::start(vec![route("PATCH", SETTINGS, Reply::json(200, SAVED))]);
    hipc(
        &server,
        &[
            "notifications",
            "update",
            "--email-enabled",
            "false",
            "--low-credit-alerts",
            "false",
            "--zero-balance-alerts",
            "true",
            "--file-status-updates",
            "true",
            "--marketing-emails",
            "true",
            "--provider-advice-emails",
            "false",
            "--yes",
        ],
    )
    .ok();
    assert_eq!(
        server.only_request().json(),
        json!({
            "email_enabled": false,
            "low_credit_alerts": false,
            "zero_balance_alerts": true,
            "file_status_updates": true,
            "marketing_emails": true,
            "provider_advice_emails": false,
        })
    );
}

/// Turning off an email that warns of a problem needs `--yes`, and the question names each
/// one turned off; turning one on, or marketing and the provider digest off, does not.
#[test]
fn turning_an_alert_off_needs_yes() {
    let server = Server::start(vec![route("PATCH", SETTINGS, Reply::json(200, SAVED))]);
    for (flag, key) in [
        ("--email-enabled", "email_enabled"),
        ("--low-credit-alerts", "low_credit_alerts"),
        ("--zero-balance-alerts", "zero_balance_alerts"),
        ("--file-status-updates", "file_status_updates"),
    ] {
        let error = hipc(&server, &["notifications", "update", flag, "false"])
            .fails(2, "confirmation_required");
        assert_eq!(
            error["message"],
            format!(
                "this would update the notification settings `{key}`, and `--yes` was not passed"
            )
        );
    }
    assert!(server.requests().is_empty());
    for args in [
        &[
            "--marketing-emails",
            "false",
            "--provider-advice-emails",
            "false",
        ][..],
        &["--low-credit-alerts", "true"],
        &["--zero-balance-alerts", "false", "--yes"],
    ] {
        let mut argv = vec!["notifications", "update"];
        argv.extend(args);
        hipc(&server, &argv).ok();
    }
    assert_eq!(server.requests().len(), 3);
}

/// An update needs at least one setting, and each takes only `true` or `false`; nothing is
/// sent otherwise.
#[test]
fn notifications_update_needs_a_setting_and_a_boolean() {
    let server = Server::start(vec![]);
    let error = hipc(&server, &["notifications", "update"]).fails(2, "usage");
    let message = error["message"].as_str().unwrap();
    for flag in [
        "--email-enabled",
        "--low-credit-alerts",
        "--zero-balance-alerts",
        "--file-status-updates",
        "--marketing-emails",
        "--provider-advice-emails",
    ] {
        assert!(message.contains(flag), "{message}");
    }
    for value in ["on", "yes", "1", ""] {
        hipc(
            &server,
            &["notifications", "update", "--email-enabled", value],
        )
        .fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

/// The dry run is the one `PATCH`, needs no token, and sends nothing.
#[test]
fn notifications_update_dry_run_is_one_patch() {
    let server = Server::start(vec![]);
    let doc = Hipc::tokenless(&Home::new(), &server)
        .run(&[
            "--dry-run",
            "account",
            "notifications",
            "update",
            "--zero-balance-alerts",
            "false",
        ])
        .ok();
    assert_eq!(draft(&doc)["method"], "PATCH");
    assert_eq!(draft(&doc)["url"], format!("{}{SETTINGS}", server.url()));
    assert_eq!(draft(&doc)["body"], json!({"zero_balance_alerts": false}));
    assert_eq!(draft(&doc)["account_header"], Value::Null);
    assert_eq!(doc["requests"].as_array().map(Vec::len), Some(1));
    assert!(server.requests().is_empty());
}

/// A rejected update names the settings it sent.
#[test]
fn notifications_update_reports_a_rejected_write() {
    let server = Server::start(vec![route(
        "PATCH",
        SETTINGS,
        Reply::json(400, r#"{"email_enabled":["Must be a valid boolean."]}"#),
    )]);
    let error = hipc(
        &server,
        &[
            "notifications",
            "update",
            "--email-enabled",
            "true",
            "--file-status-updates",
            "false",
            "--yes",
        ],
    )
    .fails(1, "validation");
    assert!(
        error["message"].as_str().unwrap().starts_with(
            "cannot update the notification settings `email_enabled` and `file_status_updates`"
        ),
        "{error}"
    );
    assert_eq!(error["side_effects"], "none");
}

/// The settings read is a group's `get` now; the bare group, which was the read, is a usage
/// error whose hint is the group's help, in JSON mode and in text mode, where that help
/// follows.
#[test]
fn notifications_alone_is_a_usage_error() {
    let server = Server::start(vec![]);
    let error = hipc(&server, &["notifications"]).fails(2, "usage");
    assert_eq!(
        error["message"],
        "a subcommand is required; `hipc account notifications` holds `get` and `update`"
    );
    assert_eq!(error["hint"], "run `hipc account notifications --help`");
    let run = hipc(&server, &["--output", "text", "notifications"]);
    assert_eq!(run.code(), Some(2));
    assert!(
        run.stderr().starts_with(
            "error: a subcommand is required; `hipc account notifications` holds `get` and \
             `update`\nhint: run `hipc account notifications --help`\n\n"
        ),
        "{}",
        run.stderr()
    );
    assert_eq!(run.stdout(), "");
    // Another group alone gets the hint to its help, and so does `update` without a
    // setting.
    let error = hipc(&server, &["marketing-consent"]).fails(2, "usage");
    assert_eq!(error["hint"], "run `hipc account marketing-consent --help`");
    let error = hipc(&server, &["notifications", "update"]).fails(2, "usage");
    assert_eq!(
        error["hint"],
        "run `hipc account notifications update --help`"
    );
    assert!(server.requests().is_empty());
}

/// A flag of `update` given to the group without the subcommand is clap's unknown argument,
/// named in the message, and the hint sends the caller to the group's help, which lists
/// `update`. With or without `=`.
#[test]
fn an_update_flag_without_update_is_a_usage_error() {
    let server = Server::start(vec![]);
    for args in [
        &["notifications", "--low-credit-alerts", "true"][..],
        &["notifications", "--low-credit-alerts=true"],
    ] {
        let error = hipc(&server, args).fails(2, "usage");
        assert_eq!(
            error["message"],
            "unexpected argument `--low-credit-alerts` found"
        );
        assert_eq!(error["hint"], "run `hipc account notifications --help`");
    }
    assert!(server.requests().is_empty());
}

/// The consent read prints the server's answer as sent, on a personal route: a membership
/// sends no header and is not refused. `hipc api` reaches the route the same way.
#[test]
fn marketing_consent_get_reads_the_users_own_consent() {
    let server = Server::start(vec![route("GET", CONSENT, Reply::json(200, WITHDRAWN))]);
    for account in ["self", MEMBER] {
        let doc = hipc(&server, &["marketing-consent", "get", "--account", account]).ok();
        assert_eq!(doc, parsed(WITHDRAWN));
    }
    let raw = Hipc::against(&Home::new(), &server)
        .run(&[
            "api",
            "GET",
            "/notifications/marketing-consent/",
            "--account",
            MEMBER,
        ])
        .ok();
    assert_eq!(raw, parsed(WITHDRAWN));
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    for request in requests {
        assert_eq!(request.method, "GET");
        assert_eq!(request.target, CONSENT);
        assert_eq!(request.header("x-hippius-account"), None);
    }
}

/// The update withdraws: one `POST` of `granted` false with `source` `settings`, with no
/// confirmation, and prints the consent the server answers with.
#[test]
fn marketing_consent_update_posts_a_withdrawal() {
    let server = Server::start(vec![route("POST", CONSENT, Reply::json(200, WITHDRAWN))]);
    let doc = hipc(
        &server,
        &[
            "marketing-consent",
            "update",
            "--granted",
            "false",
            "--account",
            MEMBER,
        ],
    )
    .ok();
    assert_eq!(doc, parsed(WITHDRAWN));
    let sent = server.only_request();
    assert_eq!(sent.method, "POST");
    assert_eq!(sent.target, CONSENT);
    assert_eq!(
        sent.json(),
        json!({ "granted": false, "source": "settings" })
    );
    assert_eq!(sent.header("x-hippius-account"), None);
}

/// Giving the consent is refused: `hipc` never shows the wording it is to, so the hint sends
/// the caller to the console. It is `usage`, before the token and the dry run, so nothing is
/// sent; `hipc api` still sends the `POST`, with `--yes` as for any raw write.
#[test]
fn marketing_consent_cannot_be_given_from_hipc() {
    let server = Server::start(vec![route("POST", CONSENT, Reply::json(200, GIVEN))]);
    let given = [
        "account",
        "marketing-consent",
        "update",
        "--granted",
        "true",
    ];
    let mut dry_run = vec!["--dry-run"];
    dry_run.extend(given);
    let runs = [
        Hipc::against(&Home::new(), &server).run(&given),
        Hipc::against(&Home::new(), &server).run(&dry_run),
        Hipc::tokenless(&Home::new(), &server).run(&given),
    ];
    for run in runs {
        let error = run.fails(2, "usage");
        assert_eq!(
            error["message"],
            "`--granted true` cannot give the marketing consent from `hipc`: it never shows the \
             wording the consent is to"
        );
        assert_eq!(
            error["hint"],
            "give the consent in the console at `https://console.hippius.com/dashboard/settings`"
        );
        assert_eq!(error["side_effects"], "none");
    }
    assert!(server.requests().is_empty());
    let doc = Hipc::against(&Home::new(), &server)
        .stdin(br#"{"granted":true}"#)
        .run(&[
            "api",
            "POST",
            "/notifications/marketing-consent/",
            "--body-file",
            "-",
            "--yes",
        ])
        .ok();
    assert_eq!(doc, parsed(GIVEN));
    assert_eq!(server.only_request().json(), json!({ "granted": true }));
}

/// `--granted` is required and takes only `true` or `false`; nothing is sent otherwise.
#[test]
fn marketing_consent_update_needs_granted_as_a_boolean() {
    let server = Server::start(vec![]);
    let error = hipc(&server, &["marketing-consent", "update"]).fails(2, "usage");
    assert!(
        error["message"].as_str().unwrap().contains("--granted"),
        "{error}"
    );
    for args in [
        &["marketing-consent", "update", "--granted"][..],
        &["marketing-consent", "update", "--granted", "yes"],
        &[
            "marketing-consent",
            "update",
            "--granted",
            "true",
            "--granted",
            "false",
        ],
    ] {
        hipc(&server, args).fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}

/// The dry run is the one `POST`, and a failure names the withdrawal.
#[test]
fn marketing_consent_update_dry_run_and_failure() {
    let server = Server::start(vec![]);
    let doc = Hipc::tokenless(&Home::new(), &server)
        .run(&[
            "--dry-run",
            "account",
            "marketing-consent",
            "update",
            "--granted",
            "false",
        ])
        .ok();
    assert_eq!(draft(&doc)["method"], "POST");
    assert_eq!(draft(&doc)["url"], format!("{}{CONSENT}", server.url()));
    assert_eq!(
        draft(&doc)["body"],
        json!({ "granted": false, "source": "settings" })
    );
    assert!(server.requests().is_empty());

    let rejected = Server::start(vec![route(
        "POST",
        CONSENT,
        Reply::json(400, r#"{"detail":"invalid body"}"#),
    )]);
    let error = hipc(
        &rejected,
        &["marketing-consent", "update", "--granted", "false"],
    )
    .fails(1, "validation");
    assert_eq!(
        error["message"],
        "cannot withdraw the marketing consent: the API says `invalid body`"
    );
    let failed = Server::start(vec![route(
        "GET",
        CONSENT,
        Reply::json(401, r#"{"detail":"Invalid token."}"#),
    )]);
    let error = hipc(&failed, &["marketing-consent", "get"]).fails(1, "auth");
    assert_eq!(
        error["message"],
        "cannot read the marketing consent: the API says `Invalid token.`"
    );
}
