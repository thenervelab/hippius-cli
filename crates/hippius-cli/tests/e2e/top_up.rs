//! `hipc billing top-up`: a card or Bitcoin checkout of the balance.

use serde_json::json;

use crate::common::{Hipc, Home, MEMBER, Reply, Server, draft, route};

const CARD: &str = "/api/billing/stripe/create-checkout-session/";
const BITCOIN: &str = "/api/billing/btcpay/create-invoice/";

/// The command line of a top-up, then `extra`.
fn top_up(amount: &str, method: &str, extra: &[&str]) -> Vec<String> {
    let words = ["billing", "top-up", "--amount", amount, "--method", method];
    words.iter().chain(extra).map(|w| (*w).to_owned()).collect()
}

fn args(words: &[String]) -> Vec<&str> {
    words.iter().map(String::as_str).collect()
}

#[test]
fn a_card_top_up_prints_the_server_body_with_the_checkout_url() {
    let body = r#"{"checkout_url":"https://checkout.stripe.com/c/pay/cs_live_1?k=v#frag"}"#;
    let server = Server::start(vec![route("POST", CARD, Reply::json(200, body))]);
    let words = top_up("50", "card", &["--yes"]);
    let run = Hipc::against(&Home::new(), &server).run(&args(&words));
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stdout(),
        format!("{body}\n"),
        "the server's bytes, unchanged"
    );
    assert_eq!(run.stderr(), "", "JSON mode has no note");
    let sent = server.only_request();
    // The console sends the amount as a decimal string to the cent.
    assert_eq!(sent.json(), json!({"amount": "50.00"}));
    assert_eq!(sent.header("x-hippius-account"), None);
}

#[test]
fn a_bitcoin_top_up_posts_to_btcpay() {
    let body = concat!(
        r#"{"checkout_url":"https://pay.example/i/inv_1","#,
        r#""transaction_id":"t","invoice_id":"inv_1"}"#
    );
    let server = Server::start(vec![route("POST", BITCOIN, Reply::json(201, body))]);
    let words = top_up("20.5", "bitcoin", &["--yes"]);
    let doc = Hipc::against(&Home::new(), &server).run(&args(&words)).ok();
    assert_eq!(doc["invoice_id"], "inv_1");
    assert_eq!(server.only_request().json(), json!({"amount": "20.50"}));
}

#[test]
fn without_yes_on_a_pipe_nothing_is_sent() {
    let server = Server::start(vec![]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&args(&top_up("50", "card", &[])))
        .fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        "this would start a card top-up of `50.00` USD, and `--yes` was not passed"
    );
    assert!(server.requests().is_empty());
}

#[test]
fn a_dry_run_shows_the_post_and_needs_no_token_or_yes() {
    let words = top_up("10000", "card", &["--dry-run"]);
    let post = json!({
        "method": "POST",
        "url": format!("http://127.0.0.1:9{CARD}"),
        "account_header": null,
        "project_header": null,
        "billing_header": null,
        "body": {"amount": "10000.00"}
    });
    let mut doc = json!({ "dry_run": true });
    doc["requests"] = json!([post]);
    assert_eq!(Hipc::new(&Home::new()).run(&args(&words)).dry_ok(), doc);
    let words = top_up("100000.00", "bitcoin", &["--dry-run"]);
    let doc = Hipc::new(&Home::new()).run(&args(&words)).dry_ok();
    assert_eq!(draft(&doc)["url"], format!("http://127.0.0.1:9{BITCOIN}"));
    assert_eq!(draft(&doc)["body"], json!({"amount": "100000.00"}));
    assert_eq!(doc["requests"].as_array().map(Vec::len), Some(1), "{doc}");
}

#[test]
fn each_method_takes_its_whole_range_and_nothing_past_it() {
    for (amount, method) in [
        ("10", "card"),
        ("10000", "card"),
        ("20", "bitcoin"),
        ("100000", "bitcoin"),
        ("0099.9", "card"),
    ] {
        let words = top_up(amount, method, &["--dry-run"]);
        Hipc::new(&Home::new()).run(&args(&words)).ok();
    }
    for (amount, method, range) in [
        ("9.99", "card", "`10.00` to `10000.00`"),
        ("10000.01", "card", "`10.00` to `10000.00`"),
        ("19.99", "bitcoin", "`20.00` to `100000.00`"),
        ("100000.01", "bitcoin", "`20.00` to `100000.00`"),
        ("0", "card", "`10.00` to `10000.00`"),
    ] {
        // The range is checked before the dry run and before the membership refusal.
        let words = top_up(amount, method, &["--dry-run"]);
        let error = Hipc::new(&Home::new())
            .env("HIPPIUS_ACCOUNT", MEMBER)
            .run(&args(&words))
            .fails(2, "usage");
        let message = error["message"].as_str().unwrap();
        assert!(message.contains(range), "{message}");
        assert!(message.contains(&format!("a {method} top-up")), "{message}");
        assert_eq!(error["hint"], format!("pass `--amount` from {range}"));
    }
}

/// The amount is argv, so it is checked before the config file is read (tenet 5): a config
/// that does not parse does not hide it, and a read of `hipc billing` still reports the
/// config.
#[test]
fn the_amount_is_checked_before_the_config_file() {
    let home = Home::new();
    home.write_config("this is not TOML [[");
    let words = top_up("5", "card", &["--yes"]);
    let error = Hipc::new(&home).run(&args(&words)).fails(2, "usage");
    let message = error["message"].as_str().unwrap();
    assert!(message.contains("`--amount 5.00` is outside"), "{message}");
    let run = Hipc::new(&home).run(&["billing", "balance"]);
    assert_ne!(run.code(), Some(0));
    assert!(run.stderr().contains("config.toml"), "{}", run.stderr());
}

#[test]
fn an_amount_that_is_not_dollars_to_the_cent_is_usage() {
    for amount in [
        "",
        "abc",
        "1.234",
        "-5",
        "+5",
        "1e3",
        "10.",
        ".5",
        "1,000",
        " 50",
        "50 ",
        "0x10",
        "99999999999999999999",
    ] {
        let words = top_up(amount, "card", &["--dry-run"]);
        let run = Hipc::new(&Home::new()).run(&args(&words));
        assert_eq!(run.code(), Some(2), "{amount:?}: {}", run.stderr());
        assert_eq!(run.stdout(), "");
        assert!(
            run.stderr().contains("\"usage\""),
            "{amount:?}: {}",
            run.stderr()
        );
    }
    let run = Hipc::new(&Home::new()).run(&args(&top_up("50", "usdc", &["--dry-run"])));
    assert_eq!(run.code(), Some(2), "{}", run.stderr());
}

#[test]
fn a_membership_is_refused_before_anything_is_sent_a_dry_run_included() {
    let server = Server::start(vec![]);
    for extra in ["--yes", "--dry-run"] {
        for method in ["card", "bitcoin"] {
            let words = top_up("50", method, &[extra]);
            let error = Hipc::against(&Home::new(), &server)
                .env("HIPPIUS_ACCOUNT", MEMBER)
                .run(&args(&words))
                .fails(2, "account_scope_unsupported");
            assert_eq!(
                error["message"],
                format!(
                    "a top-up in another account needs a billing account from `--billing`, \
                     `HIPPIUS_BILLING`, or `hipc account billing-accounts use`, and the selected \
                     account is `{MEMBER}` from `HIPPIUS_ACCOUNT`"
                )
            );
            assert_eq!(
                error["hint"],
                "pass `--account self` to act in the token's own account"
            );
        }
    }
    assert!(server.requests().is_empty());
}

#[test]
fn text_mode_notes_the_checkout_url_without_its_query_values() {
    let body = r#"{"checkout_url":"https://checkout.stripe.com/c/pay/cs_1?secret=s3cr3t#fid"}"#;
    let server = Server::start(vec![route("POST", CARD, Reply::json(200, body))]);
    let words = [
        vec!["--output".to_owned(), "text".to_owned()],
        top_up("50", "card", &["--yes"]),
    ]
    .concat();
    let run = Hipc::against(&Home::new(), &server).run(&args(&words));
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        "note: nothing has been paid yet: the balance grows by `50.00` USD once the checkout at \
         `https://checkout.stripe.com/c/pay/cs_1?secret=<redacted>` is paid\n"
    );
    let quiet = [words, vec!["--quiet".to_owned()]].concat();
    let run = Hipc::against(&Home::new(), &server).run(&args(&quiet));
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(run.stderr(), "");
}

#[test]
fn an_answer_without_a_checkout_url_notes_that_nothing_is_paid() {
    let server = Server::start(vec![route("POST", BITCOIN, Reply::json(201, r#"{"x":1}"#))]);
    let words = [
        vec!["--output".to_owned(), "text".to_owned()],
        top_up("25", "bitcoin", &["--yes"]),
    ]
    .concat();
    let run = Hipc::against(&Home::new(), &server).run(&args(&words));
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        "note: nothing has been paid yet: the balance grows by `25.00` USD once the checkout is \
         paid\n"
    );
}

#[test]
fn a_refused_top_up_changed_nothing() {
    let server = Server::start(vec![route(
        "POST",
        CARD,
        Reply::json(400, r#"{"error":"Minimum amount is $10"}"#),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&args(&top_up("50", "card", &["--yes"])))
        .fails(1, "validation");
    assert_eq!(error["side_effects"], "none");
    assert_eq!(
        error["message"],
        "cannot start a card top-up of `50.00` USD: the API says `Minimum amount is $10`"
    );
}

/// A top-up answered 502 or 503, or not at all, may have started the checkout or the
/// invoice, so each is `partial` and not retryable, whatever `Retry-After` says.
#[test]
fn a_top_up_answered_502_503_or_not_at_all_is_partial() {
    for (method, path) in [("card", CARD), ("bitcoin", BITCOIN)] {
        for (reply, code) in [
            (Reply::json(502, r#"{"error":"x"}"#), "unavailable"),
            (
                Reply::json(503, r#"{"error":"x"}"#).header("Retry-After", "4"),
                "unavailable",
            ),
            (Reply::never(), "timeout"),
        ] {
            let server = Server::start(vec![route("POST", path, reply)]);
            let words = top_up("50", method, &["--yes", "--request-timeout", "1"]);
            let error = Hipc::against(&Home::new(), &server)
                .run(&args(&words))
                .fails(1, code);
            assert_eq!(error["side_effects"], "partial", "{method}: {error}");
            assert_eq!(error["retryable"], false, "{method}: {error}");
            assert_eq!(server.requests().len(), 1);
        }
    }
}

#[test]
fn the_schema_calls_a_top_up_a_confirmed_spend() {
    let schema = Hipc::new(&Home::new()).run(&["schema"]).json();
    let row = schema["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["path"] == json!(["billing", "top-up"]))
        .expect("a schema row for billing top-up")
        .clone();
    assert_eq!(row["effect"], "non_idempotent", "{row}");
    assert_eq!(row["confirmation"], true, "{row}");
}
