//! `hipc drive subscription change` and `hipc s3 subscription change`: the same command for two
//! plans, held to one table so the two cannot drift. A change switches the funding rail, sets
//! the overage cap, or moves to another plan, one kind at a time, and asks first.

use hippius::routes::{Route, drive, s3};
use serde_json::json;

use crate::common::{Hipc, Home, Recorded, Reply, Server, draft, route_of};

/// One service's change: its command group, the plan's name in a question, its route, and a
/// plan code it sells.
struct Service {
    group: &'static str,
    plan: &'static str,
    route: &'static Route<0>,
    code: &'static str,
}

const SERVICES: [Service; 2] = [
    Service {
        group: "drive",
        plan: "Drive",
        route: &drive::SUBSCRIPTION_CHANGE,
        code: "duo",
    },
    Service {
        group: "s3",
        plan: "S3",
        route: &s3::SUBSCRIPTION_CHANGE,
        code: "pro",
    },
];

/// Swagger `drive_subscription_partial_update` and its S3 twin: a PATCH is a plan change, a
/// funding switch, or an overage cap, one at a time. Each spends money, so each is confirmed,
/// and the refusal names what would change.
#[test]
fn change_switches_funding_or_sets_the_overage_cap_alone() {
    for service in SERVICES {
        let Service { group, plan, .. } = service;
        let server = Server::start(vec![route_of(
            service.route,
            [],
            Reply::json(200, r#"{"active":true}"#),
        )]);
        let home = Home::new();
        let change = |extra: &[&str]| {
            let args = [&[group, "subscription", "change"][..], extra].concat();
            Hipc::against(&home, &server).run(&args)
        };
        for (flags, action) in [
            (
                &["--pay-from", "card"][..],
                format!("charge each renewal of the {plan} plan to the card saved at Stripe"),
            ),
            (
                &["--pay-from", "credits"],
                format!("pay each renewal of the {plan} plan from the balance"),
            ),
            (
                &["--storage-cap", "2000000000000"],
                format!(
                    "bill the storage of the {plan} plan past its quota, up to `2000000000000` \
                     bytes"
                ),
            ),
            (
                &["--storage-cap", "default"],
                format!(
                    "bill the storage of the {plan} plan past its quota, up to the default cap \
                     of twice the plan's storage"
                ),
            ),
        ] {
            let error = change(flags).fails(2, "confirmation_required");
            assert_eq!(
                error["message"],
                format!("this would {action}, and `--yes` was not passed"),
                "{group}"
            );
        }
        assert!(server.requests().is_empty(), "{group}");

        change(&["--pay-from", "card", "--yes"]).ok();
        change(&["--storage-cap", "2000000000000", "--yes"]).ok();
        change(&["--storage-cap", "default", "--yes"]).ok();
        let bodies: Vec<_> = server.requests().iter().map(Recorded::json).collect();
        assert_eq!(
            bodies,
            [
                json!({"funding": "card"}),
                json!({"storage_cap_bytes": 2_000_000_000_000_u64}),
                json!({"storage_cap_bytes": null}),
            ],
            "{group}"
        );
    }
}

#[test]
fn change_takes_one_kind_of_change_and_a_period_only_with_a_plan() {
    for service in SERVICES {
        let Service { group, code, .. } = service;
        let server = Server::start(vec![]);
        for flags in [
            &["--plan", code, "--pay-from", "card"][..],
            &["--plan", code, "--storage-cap", "default"],
            &["--pay-from", "card", "--storage-cap", "1"],
            &["--period", "annual"],
            &["--period", "annual", "--pay-from", "card"],
            &["--pay-from", "paypal"],
            &["--storage-cap", "0"],
            &["--storage-cap", "-1"],
            &["--storage-cap", "9223372036854775808"],
            &["--storage-cap", "1.5"],
        ] {
            let args = [&[group, "subscription", "change"][..], flags, &["--yes"]].concat();
            Hipc::against(&Home::new(), &server)
                .run(&args)
                .fails(2, "usage");
        }
        let doc = Hipc::against(&Home::new(), &server)
            .run(&[
                group,
                "subscription",
                "change",
                "--storage-cap",
                "9223372036854775807",
                "--dry-run",
            ])
            .ok();
        assert_eq!(
            draft(&doc)["body"],
            json!({"storage_cap_bytes": 9_223_372_036_854_775_807_u64}),
            "{group}"
        );
        assert!(server.requests().is_empty(), "{group}");
    }
}
