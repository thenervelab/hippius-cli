//! `hipc billing native-quote` and `hipc billing top-up --method halpha`: the hAlpha quote,
//! the custodial buy, and the follow of its purchase.

use hippius::routes::billing;
use serde_json::{Value, json};

use crate::common::{
    BILLING, Hipc, Home, MEMBER, PROJECT, Reply, Route, Server, route_each, route_of,
};

const QUOTE: &str = "/api/billing/native/quote/";
const BUY: &str = "/api/billing/native/buy/";
const PURCHASES: &str = "/api/billing/native/purchases/";

/// 1.5 hAlpha in planck.
const ALPHA: &str = "1500000000000000000";
/// 4.123456789012345678 credits per hAlpha.
const PRICE: &str = "4123456789012345678";
/// floor(1.5 * 4.123456789012345678) = 6.185185183518518517 credits.
const CREDITS: &str = "6185185183518518517";
/// floor(CREDITS * 9900 / 10000).
const MIN_CREDITS: &str = "6123333331683333331";
const KEY: &str = "3f2c9a1e-7b4d-4e8a-9c01-5d6e7f8a9b0c";
/// The hint of an account that signs its own buys: the console, where such a buy is made.
const SELF_SIGN_HINT: &str = "top up with hAlpha in the console at `https://console.hippius.com`";
/// The refusal of an account that signs its own buys: plain, with no promise of a `hipc` that
/// signs them.
const SELF_SIGN: &str = "the account signs its own hAlpha buys (`self_sign`), and those are \
                         made in the console; a card or Bitcoin top-up works with `hipc` too";
const ADDRESS: &str = "5DfhGyQdFobKM8NsWvEeAKk5EQQgYe9AydgJ7rMB6E1EqRzV";
/// The note a text-mode run prints once the buy is accepted, up to its random key.
const ACCEPTED: &str = "note: the buy was accepted as the purchase `p_1` under the key `";

/// `stderr` without the [`ACCEPTED`] line, which must come first.
fn after_accepted(stderr: &str) -> &str {
    assert!(stderr.starts_with(ACCEPTED), "{stderr}");
    let (line, rest) = stderr.split_once('\n').expect("the note ends its line");
    assert!(line.ends_with("`; following it"), "{line}");
    rest
}

const NOTE: &str = "note: `alpha_amount` and `alpha_price` are decimals here, in hAlpha or credits \
                    (1 credit = 1 USD); the JSON output keeps 18-decimal integers\n";

fn quote_body(changes: &Value) -> String {
    let mut quote = json!({
        "enabled": true,
        "alpha_price": PRICE,
        "alpha_price_usd": "4.12",
        "alpha_price_updated_block": 100,
        "current_block": 120,
        "max_age_blocks": 28_800,
        "stale": false,
        "address": "5DfhGyQdFobKM8NsWvEeAKk5EQQgYe9AydgJ7rMB6E1EqRzV",
        "custodial": true,
        "native_balance": "20000000000000000000",
        "decimals": 18,
        "min_alpha": "10000000000000000",
        "call": {"pallet": "Marketplace", "function": "buy_credits"},
    });
    for (key, value) in changes.as_object().expect("the changes are an object") {
        quote[key] = value.clone();
    }
    quote.to_string()
}

fn quote(changes: &Value) -> Route {
    route_of(
        &billing::NATIVE_QUOTE,
        [],
        Reply::json(200, &quote_body(changes)),
    )
}

fn buy(status: &str) -> Route {
    route_of(
        &billing::NATIVE_BUY,
        [],
        Reply::json(201, &json!({"id": "p_1", "status": status}).to_string()),
    )
}

fn row(status: &str) -> Value {
    json!({
        "id": "p_1",
        "alpha_amount": ALPHA,
        "credit_amount": if status == "completed" { Value::from(CREDITS) } else { Value::Null },
        "alpha_price": PRICE,
        "block_number": null,
        "extrinsic_hash": null,
        "status": status,
        "error": if status == "failed" { "SlippageExceeded" } else { "" },
        "source": "custodial",
        "created_at": "2026-11-03T10:00:00Z",
    })
}

/// A page of the purchases list: another purchase first, then `p_1` as `status`.
fn page(status: &str) -> Reply {
    let other = json!({"id": "p_0", "status": "completed"});
    let body = json!({"count": 2, "next": null, "previous": null, "results": [other, row(status)]});
    Reply::json(200, &body.to_string())
}

/// The purchases list with nothing in it, which the read before the question gets.
fn no_purchases() -> Reply {
    Reply::json(
        200,
        r#"{"count":0,"next":null,"previous":null,"results":[]}"#,
    )
}

/// The purchases route: an empty list for the read before the question, then `p_1` as each of
/// `statuses` in turn for the follow.
fn purchases(statuses: &[&str]) -> Route {
    let follow = statuses.iter().map(|s| page(s));
    route_each(
        "GET",
        PURCHASES,
        std::iter::once(no_purchases()).chain(follow).collect(),
    )
}

fn top_up(server: &Server, extra: &[&str]) -> crate::common::Run {
    top_up_with(Hipc::against(&Home::new(), server), extra)
}

/// [`top_up`] with the real 3-second poll interval instead of the suite's short one, for a
/// test that counts the reads a deadline leaves room for.
fn top_up_at_the_real_interval(server: &Server, extra: &[&str]) -> crate::common::Run {
    top_up_with(
        Hipc::against(&Home::new(), server).env_remove("HIPC_TEST_POLL_INTERVAL_MS"),
        extra,
    )
}

fn top_up_with(hipc: Hipc, extra: &[&str]) -> crate::common::Run {
    let mut args = vec!["billing", "top-up", "--method", "halpha", "--alpha", "1.5"];
    args.extend_from_slice(extra);
    hipc.run(&args)
}

#[test]
fn the_quote_is_printed_as_sent() {
    let server = Server::start(vec![quote(&json!({}))]);
    let run = Hipc::against(&Home::new(), &server).run(&["billing", "native-quote"]);
    assert_eq!(run.code(), Some(0));
    assert_eq!(run.stdout(), format!("{}\n", quote_body(&json!({}))));
    assert_eq!(server.only_request().header("x-hippius-account"), None);
}

#[test]
fn the_quote_shows_decimals_in_text_mode() {
    let server = Server::start(vec![quote(&json!({}))]);
    let run =
        Hipc::against(&Home::new(), &server).run(&["--output", "text", "billing", "native-quote"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    let out = run.stdout();
    for shown in ["4.123456789012345678", "0.01", "20"] {
        assert!(out.contains(shown), "{shown} in {out}");
    }
    assert!(!out.contains(PRICE), "{out}");
    assert_eq!(
        run.stderr(),
        "note: `alpha_price`, `min_alpha`, and `native_balance` are decimals here, in hAlpha or \
         credits (1 credit = 1 USD); the JSON output keeps 18-decimal integers\n"
    );
}

/// The hAlpha routes act on the signed-in user's own hAlpha, so every selector given by flag
/// is refused before anything is sent, in a dry run too, and one stored in the profile is
/// skipped on the quote, a read, and refused on the buy, a write.
#[test]
fn the_halpha_routes_refuse_another_account_project_or_billing_account() {
    let server = Server::start(vec![]);
    let quote_args = ["billing", "native-quote"];
    let buy_args = [
        "billing", "top-up", "--method", "halpha", "--alpha", "1", "--yes",
    ];
    for command in [&quote_args[..], &buy_args[..]] {
        for (flag, value, code) in [
            ("--account", MEMBER, "account_scope_unsupported"),
            ("--project", PROJECT, "project_scope_unsupported"),
            ("--billing", BILLING, "billing_scope_unsupported"),
        ] {
            for dry_run in [false, true] {
                let mut args = vec![flag, value];
                if dry_run {
                    args.push("--dry-run");
                }
                args.extend_from_slice(command);
                Hipc::against(&Home::new(), &server)
                    .run(&args)
                    .fails(2, code);
            }
        }
    }
    assert!(server.requests().is_empty());
    let home = Home::new();
    home.write_config(&format!(
        "[profiles.default]\nbilling = \"{BILLING}\"\nproject = \"{PROJECT}\"\n"
    ));
    let server = Server::start(vec![quote(&json!({}))]);
    let run = Hipc::against(&home, &server).run(&quote_args);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    let sent = server.only_request();
    assert_eq!(sent.header("x-hippius-billing"), None);
    assert_eq!(sent.header("x-hippius-project"), None);
    // On a write a stored project is refused, and a stored billing account is skipped
    // (tenet 12): the buy spends the signed-in user's own hAlpha either way.
    Hipc::against(&home, &server)
        .run(&buy_args)
        .fails(2, "project_scope_unsupported");
    let home = Home::new();
    home.write_config(&format!("[profiles.default]\nbilling = \"{BILLING}\"\n"));
    let server = Server::start(vec![quote(&json!({})), purchases(&[]), buy("completed")]);
    let run = Hipc::against(&home, &server).run(&buy_args);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    for sent in server.requests() {
        assert_eq!(sent.header("x-hippius-billing"), None);
        assert_eq!(sent.header("x-hippius-project"), None);
    }
}

#[test]
fn the_balance_must_cover_the_amount_and_the_fee() {
    for (balance, refused) in [
        ("1510000000000000000", false),
        ("1509999999999999999", true),
        ("0", true),
    ] {
        let server = Server::start(vec![
            quote(&json!({"native_balance": balance})),
            purchases(&[]),
            buy("completed"),
        ]);
        let run = top_up(&server, &["--yes"]);
        if refused {
            let error = run.fails(1, "conflict");
            assert!(
                error["message"]
                    .as_str()
                    .unwrap()
                    .starts_with("`--alpha 1.5` and the `0.01` hAlpha kept for the fee are more"),
                "{error}"
            );
            assert_eq!(server.calls(), [format!("GET {QUOTE}")]);
        } else {
            assert_eq!(run.code(), Some(0), "{}", run.stderr());
        }
    }
    let server = Server::start(vec![quote(
        &json!({"native_balance": "1000000000000000000"}),
    )]);
    let error = top_up(&server, &["--yes"]).fails(1, "conflict");
    assert_eq!(error["hint"], "pass `--alpha` of at most `0.99`");
}

#[test]
fn a_purchase_in_flight_blocks_a_second_buy() {
    let list = json!({"count": 1, "results": [{"id": "p_7", "status": "submitted"}]});
    let server = Server::start(vec![
        quote(&json!({})),
        route_of(
            &billing::NATIVE_PURCHASES,
            [],
            Reply::json(200, &list.to_string()),
        ),
    ]);
    let error = top_up(&server, &["--yes"]).fails(1, "conflict");
    assert_eq!(
        error["message"],
        "the purchase `p_7` is still `submitted`, and a second buy now could spend twice"
    );
    assert_eq!(
        error["hint"],
        "run `hipc api GET /billing/native/purchases/` until it is `completed` or `failed`, \
         then top up again"
    );
    assert_eq!(
        server.calls(),
        [format!("GET {QUOTE}"), format!("GET {PURCHASES}")]
    );
    // A purchase with no status yet is in flight too.
    let list = json!({"count": 1, "results": [{"id": "p_8"}]});
    let server = Server::start(vec![
        quote(&json!({})),
        route_of(
            &billing::NATIVE_PURCHASES,
            [],
            Reply::json(200, &list.to_string()),
        ),
    ]);
    let error = top_up(&server, &["--yes"]).fails(1, "conflict");
    assert_eq!(
        error["message"],
        "the purchase `p_8` is still without a status, and a second buy now could spend twice"
    );
}

/// The in-flight window is measured by the API's clock, the purchases read's `Date`: a
/// purchase created a minute before that `Date` blocks a second buy, though by the local
/// clock, which the server's `Date` here makes days fast, it is long out of the window.
#[test]
fn the_in_flight_window_is_measured_by_the_apis_clock() {
    let list = json!({"count": 1, "results": [
        {"id": "p_7", "status": "submitted", "created_at": "2026-11-03T10:00:00Z"}
    ]});
    let server = Server::start(vec![
        quote(&json!({})),
        route_of(
            &billing::NATIVE_PURCHASES,
            [],
            Reply::json(200, &list.to_string()).header("Date", "Tue, 03 Nov 2026 10:01:00 GMT"),
        ),
    ]);
    let error = top_up(&server, &["--yes"]).fails(1, "conflict");
    assert_eq!(
        error["message"],
        "the purchase `p_7` is still `submitted`, and a second buy now could spend twice"
    );
    // Sixteen minutes before the `Date` is out of the window by either clock.
    let server = Server::start(vec![
        quote(&json!({})),
        route_of(
            &billing::NATIVE_PURCHASES,
            [],
            Reply::json(200, &list.to_string()).header("Date", "Tue, 03 Nov 2026 10:16:00 GMT"),
        ),
        buy("completed"),
    ]);
    top_up(&server, &["--yes"]).ok();
}

/// A retry resends the first run's exact amounts under its key: it reads the quote but not
/// the list, and no refusal but the floor of its `--min-credits` and a self-signing account
/// applies, even a quote that is off or a balance the first buy spent.
#[test]
fn a_retry_resends_the_exact_buy_reading_only_the_quote() {
    let server = Server::start(vec![
        quote(&json!({"enabled": false, "stale": true, "native_balance": "0"})),
        buy("queued"),
        route_of(&billing::NATIVE_PURCHASES, [], page("completed")),
    ]);
    let doc = top_up(
        &server,
        &[
            "--yes",
            "--idempotency-key",
            KEY,
            "--min-credits",
            "6.123333331683333331",
        ],
    )
    .ok();
    assert_eq!(doc["idempotency_key"], KEY);
    let requests = server.requests();
    assert_eq!(requests[0].path(), QUOTE);
    assert_eq!(requests[1].path(), BUY);
    assert_eq!(requests[1].header("idempotency-key"), Some(KEY));
    assert_eq!(
        requests[1].json(),
        json!({"alpha_amount": ALPHA, "min_credits": MIN_CREDITS})
    );
    let server = Server::start(vec![quote(&json!({}))]);
    let error = top_up(&server, &["--idempotency-key", KEY, "--min-credits", "6"])
        .fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!(
            "this would buy credits with `1.5` hAlpha of `{ADDRESS}` again under the key \
             `{KEY}`, at least `6` credits; a key the API has not seen is a new buy, and \
             `--yes` was not passed"
        )
    );
    assert_eq!(server.calls(), [format!("GET {QUOTE}")]);
    let server = Server::start(vec![]);
    for (flags, message) in [
        (
            &["--idempotency-key", KEY][..],
            "a retry with `--idempotency-key` resends the first buy's exact amounts, and needs \
             its `--min-credits`",
        ),
        (
            &["--min-credits", "6"][..],
            "`--min-credits` is the minimum of a buy to retry, and needs that buy's \
             `--idempotency-key`",
        ),
    ] {
        let mut args = vec!["--yes"];
        args.extend_from_slice(flags);
        let error = top_up(&server, &args).fails(2, "usage");
        assert_eq!(error["message"], message);
    }
    let above_zero = "a buy accepts at least some credits, so the minimum is above `0`";
    for (bad, why) in [
        ("0", above_zero),
        ("0.0", above_zero),
        (
            "1.0000000000000000001",
            "credits has 18 decimals, so an amount has at most 18 digits after the point",
        ),
        (
            "x",
            "an amount is credits in digits, such as `6.12` or `990`",
        ),
    ] {
        let error = top_up(
            &server,
            &["--yes", "--idempotency-key", KEY, "--min-credits", bad],
        )
        .fails(2, "usage");
        assert_eq!(
            error["message"],
            format!("invalid value `{bad}` for `--min-credits <CREDITS>`: {why}")
        );
    }
    assert!(server.requests().is_empty());
}

#[test]
fn a_purchase_the_list_never_shows_says_so_at_the_deadline() {
    let server = Server::start(vec![
        quote(&json!({})),
        route_of(&billing::NATIVE_PURCHASES, [], no_purchases()),
        buy("queued"),
    ]);
    let run = top_up(
        &server,
        &["--yes", "--wait-timeout", "1", "--output", "text"],
    );
    assert_eq!(run.code(), Some(0));
    assert_eq!(
        after_accepted(run.stderr()),
        "warning: the purchase `p_1` was still `queued` after 1 second, and the purchases \
         list never showed it; `hipc billing balance` shows the credits once it completes\n"
    );
}

#[test]
fn a_custodial_buy_sends_the_exact_amounts_and_follows_the_purchase() {
    let server = Server::start(vec![
        quote(&json!({})),
        buy("queued"),
        purchases(&["completed"]),
    ]);
    let doc = top_up(&server, &["--yes"]).ok();
    assert_eq!(doc["status"], "completed");
    assert_eq!(doc["id"], "p_1");
    assert_eq!(doc["credit_amount"], CREDITS);
    assert_eq!(
        server.calls(),
        [
            format!("GET {QUOTE}"),
            format!("GET {PURCHASES}"),
            format!("POST {BUY}"),
            format!("GET {PURCHASES}")
        ]
    );
    let requests = server.requests();
    assert_eq!(
        requests[1].target,
        format!("{PURCHASES}?ordering=-created_at")
    );
    let buy = &requests[2];
    assert_eq!(
        buy.json(),
        json!({"alpha_amount": ALPHA, "min_credits": MIN_CREDITS})
    );
    let key = buy.header("idempotency-key").unwrap();
    assert_eq!(doc["idempotency_key"], key);
    let groups: Vec<usize> = key.split('-').map(str::len).collect();
    assert_eq!(groups, [8, 4, 4, 4, 12], "{key}");
    assert_eq!(&key[14..15], "4", "a v4 UUID: {key}");
    assert_eq!(buy.header("x-hippius-account"), None);
    assert_eq!(requests[0].header("idempotency-key"), None);
}

#[test]
fn each_run_sends_a_new_key_unless_one_is_given() {
    let server = Server::start(vec![
        quote(&json!({})),
        buy("queued"),
        purchases(&["completed"]),
    ]);
    let first = top_up(&server, &["--yes"]).ok()["idempotency_key"].clone();
    let second = top_up(&server, &["--yes"]).ok()["idempotency_key"].clone();
    assert_ne!(first, second);
    let doc = top_up(
        &server,
        &["--yes", "--idempotency-key", KEY, "--min-credits", "6.1"],
    )
    .ok();
    assert_eq!(doc["idempotency_key"], KEY);
    let last = server
        .requests()
        .into_iter()
        .rfind(|r| r.method == "POST")
        .unwrap();
    assert_eq!(last.header("idempotency-key"), Some(KEY));
}

#[test]
fn the_follow_reads_again_until_the_purchase_is_final() {
    let server = Server::start(vec![
        quote(&json!({})),
        buy("queued"),
        purchases(&["submitted", "completed"]),
    ]);
    let run = top_up(&server, &["--yes", "--output", "text"]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert!(
        run.stdout().contains("6.185185183518518517"),
        "{}",
        run.stdout()
    );
    assert!(
        run.stderr().contains(
            "note: the purchase `p_1` completed; `hipc billing balance` shows the credits"
        ),
        "{}",
        run.stderr()
    );
    assert_eq!(server.calls().len(), 5);
}

#[test]
fn the_follow_stops_at_the_wait_timeout_with_the_row_it_has() {
    let server = Server::start(vec![
        quote(&json!({})),
        buy("queued"),
        purchases(&["submitted"]),
    ]);
    let run = top_up_at_the_real_interval(&server, &["--yes", "--wait-timeout", "1"]);
    assert_eq!(
        run.code(),
        Some(0),
        "a stop is not an error: {}",
        run.stderr()
    );
    assert_eq!(run.stderr(), "", "JSON mode has no warning");
    let doc = run.json();
    assert_eq!(doc["status"], "submitted");
    assert!(doc["idempotency_key"].is_string());
    assert_eq!(
        server.calls().len(),
        5,
        "the quote, the list, the buy, a read now, and one after the wait"
    );
    let text = top_up(
        &server,
        &["--yes", "--wait-timeout", "1", "--output", "text"],
    );
    assert_eq!(text.code(), Some(0));
    assert_eq!(
        after_accepted(text.stderr()),
        format!(
            "warning: the purchase `p_1` was still `submitted` after 1 second; `hipc billing \
             balance` shows the credits once it completes\n{NOTE}"
        )
    );
}

#[test]
fn a_buy_answered_as_final_is_not_read_again() {
    let server = Server::start(vec![quote(&json!({})), purchases(&[]), buy("completed")]);
    let doc = top_up(&server, &["--yes"]).ok();
    assert_eq!(doc["status"], "completed");
    assert_eq!(server.calls().len(), 3);
}

#[test]
fn a_failed_purchase_is_an_error_with_the_row_on_stdout() {
    let server = Server::start(vec![
        quote(&json!({})),
        buy("queued"),
        purchases(&["failed"]),
    ]);
    let run = top_up(&server, &["--yes"]);
    assert_eq!(run.code(), Some(1));
    assert_eq!(run.json()["status"], "failed");
    let error = run.error();
    assert_eq!(error["code"], "partial_failure");
    assert_eq!(
        error["message"],
        "the purchase `p_1` failed on chain: the API says `SlippageExceeded`"
    );
    assert_eq!(
        error["details"],
        json!({"purchase": "p_1", "status": "failed", "error": "SlippageExceeded"})
    );
    assert_eq!(error["side_effects"], "partial");
    assert_eq!(error["retryable"], false);
}

#[test]
fn an_unknown_status_is_a_partial_failure_and_a_missing_id_a_warning() {
    let server = Server::start(vec![
        quote(&json!({})),
        buy("queued"),
        purchases(&["refunded"]),
    ]);
    let run = top_up(&server, &["--yes"]);
    assert_eq!(run.code(), Some(1));
    assert_eq!(run.json()["status"], "refunded");
    let error = run.error();
    assert_eq!(error["code"], "partial_failure");
    assert_eq!(
        error["message"],
        "the purchase `p_1` is `refunded`, a status `hipc` does not know"
    );
    assert_eq!(
        error["details"],
        json!({"purchase": "p_1", "status": "refunded"})
    );
    let no_id = route_of(
        &billing::NATIVE_BUY,
        [],
        Reply::json(201, r#"{"status":"queued"}"#),
    );
    let server = Server::start(vec![quote(&json!({})), purchases(&[]), no_id]);
    let run = top_up(&server, &["--yes", "--output", "text"]);
    assert_eq!(run.code(), Some(0));
    assert_eq!(
        run.stderr(),
        "warning: the buy answer names no purchase, so it was not followed\n"
    );
    assert_eq!(server.calls().len(), 3);
}

#[test]
fn a_numeric_id_is_found_in_the_list() {
    let started = route_of(
        &billing::NATIVE_BUY,
        [],
        Reply::json(201, r#"{"id":12,"status":"queued"}"#),
    );
    let list = json!({"count": 1, "results": [{"id": 12, "status": "completed"}]});
    let server = Server::start(vec![
        quote(&json!({})),
        started,
        route_of(
            &billing::NATIVE_PURCHASES,
            [],
            Reply::json(200, &list.to_string()),
        ),
    ]);
    assert_eq!(top_up(&server, &["--yes"]).ok()["status"], "completed");
}

#[test]
fn the_follow_waits_out_a_passing_outage_and_reports_a_lasting_one() {
    let unavailable = || Reply::json(503, r#"{"detail":"down"}"#);
    let server = Server::start(vec![
        quote(&json!({})),
        buy("queued"),
        route_each(
            "GET",
            PURCHASES,
            vec![no_purchases(), unavailable(), page("completed")],
        ),
    ]);
    let doc = top_up(&server, &["--yes"]).ok();
    assert_eq!(doc["status"], "completed");

    let server = Server::start(vec![
        quote(&json!({})),
        buy("queued"),
        route_each(
            "GET",
            PURCHASES,
            vec![no_purchases(), Reply::json(403, r#"{"detail":"no"}"#)],
        ),
    ]);
    let run = top_up(&server, &["--yes"]);
    assert_eq!(run.code(), Some(1));
    assert_eq!(
        run.json()["status"],
        "queued",
        "the last row seen is on stdout"
    );
    let error = run.error();
    assert_eq!(error["code"], "permission");
    assert_eq!(
        error["hint"],
        "run `hipc api GET /billing/native/purchases/` to see where the purchase `p_1` stands"
    );
}

#[test]
fn a_buy_answer_that_is_not_a_purchase_is_internal() {
    let server = Server::start(vec![
        quote(&json!({})),
        purchases(&[]),
        route_of(&billing::NATIVE_BUY, [], Reply::json(201, "[]")),
    ]);
    let run = top_up(&server, &["--yes"]);
    assert_eq!(run.code(), Some(1));
    assert_eq!(run.json()["idempotency_key"].as_str().unwrap().len(), 36);
}

/// Each refusal the quote decides is `conflict` with no HTTP status, after the read alone.
#[test]
fn the_quote_rules_out_a_buy_before_anything_is_bought() {
    let cases = [
        (
            json!({"enabled": false, "reason": "maintenance"}),
            "hAlpha top-ups are off (`enabled` is `false` in the quote): the API says \
             `maintenance`",
            "run `hipc billing native-quote` until it says `enabled` is `true`, then top up again",
        ),
        (
            json!({"enabled": false}),
            "hAlpha top-ups are off (`enabled` is `false` in the quote)",
            "run `hipc billing native-quote` until it says `enabled` is `true`, then top up again",
        ),
        (
            json!({"stale": true}),
            "the hAlpha price on chain is stale (`stale` is `true` in the quote), and the chain \
             refuses a buy at a stale price",
            "run `hipc billing native-quote` until it says `stale` is `false`, then top up again",
        ),
        (json!({"custodial": false}), SELF_SIGN, SELF_SIGN_HINT),
        (
            json!({"min_alpha": "2000000000000000000"}),
            "`--alpha 1.5` is below the quote's `min_alpha` of `2` hAlpha",
            "pass `--alpha` of at least `2`",
        ),
        (
            json!({"alpha_price": "0"}),
            "`--alpha 1.5` buys no credits at the quoted `alpha_price`",
            "pass a larger `--alpha`",
        ),
        (
            json!({"alpha_price": "1"}),
            "`--alpha 1.5` buys no credits at the quoted `alpha_price`",
            "pass a larger `--alpha`",
        ),
    ];
    for (changes, message, hint) in cases {
        let server = Server::start(vec![quote(&changes)]);
        let error = top_up(&server, &["--yes"]).fails(1, "conflict");
        assert_eq!(error["message"], message, "{changes}");
        assert_eq!(error["hint"], hint, "{changes}");
        assert_eq!(error["http_status"], Value::Null);
        assert_eq!(error["side_effects"], "none");
        assert_eq!(server.calls(), [format!("GET {QUOTE}")], "{changes}");
    }
}

#[test]
fn a_quote_hipc_cannot_read_or_price_is_internal() {
    for changes in [
        json!({"stale": "no"}),
        json!({"alpha_price": 4.12}),
        json!({"min_alpha": null}),
        json!({"custodial": null}),
        json!({"enabled": null}),
    ] {
        let server = Server::start(vec![quote(&changes)]);
        let error = top_up(&server, &["--yes"]).fails(1, "internal");
        assert_eq!(
            error["message"],
            "cannot get the hAlpha quote: the API answered with a shape `hipc` does not read",
            "{changes}"
        );
    }
    let huge = json!({"alpha_price": u128::MAX.to_string()});
    let server = Server::start(vec![quote(&huge)]);
    let error = top_up(&server, &["--yes"]).fails(1, "internal");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("larger than `hipc` counts"),
        "{error}"
    );
}

#[test]
fn a_409_self_sign_from_the_buy_is_refused_with_the_console_hint() {
    let refused = route_of(
        &billing::NATIVE_BUY,
        [],
        Reply::json(
            409,
            r#"{"code":"self_sign","detail":"This account signs its own buys."}"#,
        ),
    );
    let server = Server::start(vec![quote(&json!({})), purchases(&[]), refused]);
    let error = top_up(&server, &["--yes"]).fails(1, "conflict");
    assert_eq!(error["message"], SELF_SIGN);
    assert_eq!(error["hint"], SELF_SIGN_HINT);
    assert_eq!(error["http_status"], 409);
    assert_eq!(error["details"]["code"], "self_sign");
    // Another 409 keeps the API's own words.
    let other = route_of(
        &billing::NATIVE_BUY,
        [],
        Reply::json(409, r#"{"code":"busy","detail":"busy"}"#),
    );
    let server = Server::start(vec![quote(&json!({})), purchases(&[]), other]);
    let error = top_up(&server, &["--yes"]).fails(1, "conflict");
    assert!(
        !error["message"].as_str().unwrap().contains("self_sign"),
        "{error}"
    );
}

#[test]
fn a_buy_that_may_have_reached_the_server_names_its_key() {
    let failed = route_of(
        &billing::NATIVE_BUY,
        [],
        Reply::json(500, r#"{"detail":"boom"}"#),
    );
    let server = Server::start(vec![quote(&json!({})), purchases(&[]), failed]);
    let error = top_up(&server, &["--yes"]).fails(1, "internal");
    assert_eq!(error["side_effects"], "partial");
    let key = server.requests()[2]
        .header("idempotency-key")
        .unwrap()
        .to_owned();
    assert_eq!(
        error["hint"],
        format!(
            "run `hipc billing top-up --method halpha --alpha 1.5 --yes --min-credits \
             6.123333331683333331 --idempotency-key {key}` to retry this exact buy, which the \
             API answers as the first rather than buying again"
        )
    );
}

/// The retry hint is the run's own command line, with the profile it was given, so the retry
/// goes to the same account; a retry that fails again names itself unchanged.
#[test]
fn the_retry_hint_keeps_the_profile_and_is_not_doubled() {
    let home = Home::new();
    let failed = || {
        route_of(
            &billing::NATIVE_BUY,
            [],
            Reply::json(500, r#"{"detail":"boom"}"#),
        )
    };
    let server = Server::start(vec![quote(&json!({})), purchases(&[]), failed()]);
    let work = Hipc::against(&home, &server)
        .run(&[
            "--profile",
            "work",
            "billing",
            "top-up",
            "--method",
            "halpha",
            "--alpha",
            "1.5",
            "--yes",
        ])
        .fails(1, "internal");
    let hint = work["hint"].as_str().unwrap();
    assert!(
        hint.starts_with("run `hipc --profile work billing top-up --method halpha"),
        "{hint}"
    );
    let server = Server::start(vec![quote(&json!({})), failed()]);
    let args = [
        "billing",
        "top-up",
        "--method",
        "halpha",
        "--alpha",
        "1.5",
        "--min-credits",
        "6",
        "--idempotency-key",
        KEY,
        "--yes",
    ];
    let again = Hipc::against(&home, &server)
        .run(&args)
        .fails(1, "internal");
    assert_eq!(
        again["hint"],
        format!(
            "run `hipc {}` to retry this exact buy, which the API answers as the first rather \
             than buying again",
            args.join(" ")
        )
    );
}

/// A retry whose `--min-credits` is below 90 percent of what `--alpha` buys at the quote now
/// is refused before anything is bought: under a key the API has not seen it would be a new
/// buy at that price. A self-signing account is refused as for a first buy.
#[test]
fn a_retry_below_the_floor_or_for_a_self_signing_account_is_refused() {
    // 1.5 hAlpha buys 6.185185183518518517 credits; 90 percent is 5.566666665166666665.
    for (min, refused) in [
        ("5.566666665166666665", false),
        ("5.566666665166666664", true),
    ] {
        let server = Server::start(vec![quote(&json!({})), buy("completed")]);
        let run = top_up(
            &server,
            &["--yes", "--idempotency-key", KEY, "--min-credits", min],
        );
        if refused {
            let error = run.fails(1, "conflict");
            assert_eq!(
                error["message"],
                format!(
                    "`--min-credits {min}` is below 90 percent of the `6.185185183518518517` \
                     credits `--alpha 1.5` buys at the quoted price now, and under a key the API \
                     has not seen the retry would be a new buy at that price"
                )
            );
            assert_eq!(server.calls(), [format!("GET {QUOTE}")]);
        } else {
            assert_eq!(run.code(), Some(0), "{}", run.stderr());
        }
    }
    let server = Server::start(vec![quote(&json!({"custodial": false}))]);
    let error = top_up(
        &server,
        &["--yes", "--idempotency-key", KEY, "--min-credits", "6"],
    )
    .fails(1, "conflict");
    assert_eq!(error["message"], SELF_SIGN);
    assert_eq!(error["hint"], SELF_SIGN_HINT);
    assert_eq!(server.calls(), [format!("GET {QUOTE}")]);
}

#[test]
fn without_yes_the_question_names_the_credits_after_the_quote() {
    let server = Server::start(vec![quote(&json!({})), purchases(&[])]);
    let error = top_up(&server, &[]).fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        format!(
            "this would buy credits with `1.5` hAlpha of `{ADDRESS}`: about \
             `6.185185183518518517` credits at the quoted price, and at least \
             `6.123333331683333331`, for the signed-in user's own balance, and `--yes` was not \
             passed"
        )
    );
    assert_eq!(
        server.calls(),
        [format!("GET {QUOTE}"), format!("GET {PURCHASES}")]
    );
}

#[test]
fn a_dry_run_lists_the_three_requests_and_sends_nothing() {
    let server = Server::start(vec![]);
    let doc = Hipc::tokenless(&Home::new(), &server)
        .run(&[
            "--dry-run",
            "billing",
            "top-up",
            "--method",
            "halpha",
            "--alpha",
            "0.01",
        ])
        .ok();
    assert_eq!(doc["dry_run"], true);
    let requests = doc["requests"].as_array().unwrap();
    let lines: Vec<String> = requests
        .iter()
        .map(|r| {
            format!(
                "{} {}",
                r["method"].as_str().unwrap(),
                r["url"].as_str().unwrap()
            )
        })
        .collect();
    let base = server.url();
    assert_eq!(
        lines,
        [
            format!("GET {base}{QUOTE}"),
            format!("GET {base}{PURCHASES}?ordering=-created_at"),
            format!("POST {base}{BUY}"),
        ]
    );
    let requests = &requests[1..];
    assert_eq!(
        requests[1]["body"],
        json!({
            "alpha_amount": "10000000000000000",
            "min_credits": "<the credits the quote gives, less 1 percent>",
        })
    );
    assert_eq!(requests[1]["idempotency_key"], "<a new key for this run>");
    assert!(requests[0].get("idempotency_key").is_none());
    assert_eq!(doc["reads_first"], doc["requests"][0]);
    let doc = Hipc::new(&Home::new())
        .run(&[
            "--dry-run",
            "billing",
            "top-up",
            "--method",
            "halpha",
            "--alpha",
            "2",
            "--idempotency-key",
            KEY,
            "--min-credits",
            "7.5",
        ])
        .ok();
    // A retry reads the quote first, then sends the buy as it was sent; the follow's polls
    // are left out.
    let requests = doc["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["method"], "GET");
    assert_eq!(requests[1]["idempotency_key"], KEY);
    assert_eq!(
        requests[1]["body"],
        json!({"alpha_amount": "2000000000000000000", "min_credits": "7500000000000000000"})
    );
    assert_eq!(doc["reads_first"], doc["requests"][0]);
    assert!(server.requests().is_empty());
}

#[test]
fn the_amount_flags_belong_to_their_methods() {
    let server = Server::start(vec![]);
    let cases: [(&[&str], &str); 8] = [
        (
            &["--method", "card", "--amount", "50", "--min-credits", "6"],
            "`--min-credits` is for a `halpha` top-up, and a `card` top-up sends none",
        ),
        (
            &["--method", "halpha", "--amount", "50"],
            "`--amount` is US dollars, for a `card` or `bitcoin` top-up; a `halpha` top-up takes \
             `--alpha`",
        ),
        (
            &["--method", "halpha", "--amount", "50", "--alpha", "1"],
            "`--amount` is US dollars, for a `card` or `bitcoin` top-up; a `halpha` top-up takes \
             `--alpha`",
        ),
        (
            &["--method", "halpha"],
            "a `halpha` top-up needs `--alpha`, the hAlpha to spend",
        ),
        (
            &["--method", "card", "--alpha", "1"],
            "`--alpha` is hAlpha, for a `halpha` top-up; a `card` top-up takes `--amount` in US \
             dollars",
        ),
        (
            &["--method", "bitcoin"],
            "a `bitcoin` top-up needs `--amount`, in US dollars",
        ),
        (
            &[
                "--method",
                "card",
                "--amount",
                "50",
                "--idempotency-key",
                KEY,
            ],
            "`--idempotency-key` is for a `halpha` top-up, and a `card` top-up sends none",
        ),
        (
            &[
                "--method",
                "bitcoin",
                "--amount",
                "50",
                "--wait-timeout",
                "5",
            ],
            "`--wait-timeout` is for a `halpha` top-up, and a `bitcoin` top-up sends none",
        ),
    ];
    for (flags, message) in cases {
        let mut args = vec!["billing", "top-up", "--yes"];
        args.extend_from_slice(flags);
        let error = Hipc::against(&Home::new(), &server)
            .run(&args)
            .fails(2, "usage");
        assert_eq!(error["message"], message, "{flags:?}");
    }
    assert!(server.requests().is_empty());
}

#[test]
fn an_alpha_amount_hipc_cannot_send_exactly_is_usage() {
    let server = Server::start(vec![]);
    for alpha in ["0", "0.009", "1e2", "0.0000000000000000001", "abc", ".5"] {
        let error = Hipc::against(&Home::new(), &server)
            .run(&[
                "billing", "top-up", "--method", "halpha", "--alpha", alpha, "--yes",
            ])
            .fails(2, "usage");
        assert!(
            error["message"].as_str().unwrap().contains("--alpha"),
            "{alpha}: {error}"
        );
    }
    let error = Hipc::against(&Home::new(), &server)
        .run(&[
            "billing", "top-up", "--method", "halpha", "--alpha", "0.005", "--yes",
        ])
        .fails(2, "usage");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("the chain takes at least `0.01` hAlpha"),
        "{error}"
    );
    let halpha = ["billing", "top-up", "--method", "halpha", "--alpha", "1"];
    for timeout in ["0", "901"] {
        let argv = [&halpha[..], &["--wait-timeout", timeout, "--yes"]].concat();
        let error = Hipc::against(&Home::new(), &server)
            .run(&argv)
            .fails(2, "usage");
        assert_eq!(
            error["message"],
            format!(
                "invalid value `{timeout}` for `--wait-timeout <SECONDS>`: {timeout} is not in `1..=900`"
            )
        );
    }
    let argv = [&halpha[..], &["--idempotency-key", "not-a-key", "--yes"]].concat();
    let error = Hipc::against(&Home::new(), &server)
        .run(&argv)
        .fails(2, "usage");
    assert_eq!(
        error["message"],
        "invalid value `not-a-key` for `--idempotency-key <UUID>`: a key is a UUID such as \
         `3f2c9a1e-7b4d-4e8a-9c01-5d6e7f8a9b0c`, as a run prints it"
    );
    assert!(server.requests().is_empty());
    // The longest wait is 900 seconds, which a dry run takes.
    let argv = [&halpha[..], &["--wait-timeout", "900", "--dry-run"]].concat();
    assert_eq!(
        Hipc::against(&Home::new(), &server).run(&argv).ok()["dry_run"],
        true
    );
}

#[test]
fn a_failed_purchase_without_an_error_says_only_that_it_failed() {
    let mut failed = row("failed");
    failed["error"] = "".into();
    let list = json!({"count": 1, "results": [failed]});
    let server = Server::start(vec![
        quote(&json!({})),
        route_each(
            "GET",
            PURCHASES,
            vec![no_purchases(), Reply::json(200, &list.to_string())],
        ),
        buy("queued"),
    ]);
    let run = top_up(&server, &["--yes"]);
    assert_eq!(run.code(), Some(1));
    let error = run.error();
    assert_eq!(error["message"], "the purchase `p_1` failed on chain");
}
