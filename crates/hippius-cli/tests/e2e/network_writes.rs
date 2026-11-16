//! The network writes that send one request: a group's create and delete, the policies, the
//! VPC networks with their resources and routers, and the isolation switch with its read;
//! and the topology, which reads the group list first. Every route acts in the selected
//! account, and a project or a billing account is refused.

use hippius::routes::network;
use serde_json::Value;

use crate::common::{BILLING, Hipc, Home, MEMBER, PROJECT, Reply, Server, draft, route, route_of};

/// A group, as the console's network types give one.
const GROUP: &str = r#"{"id":5,"label":"build-boxes","is_default":false,"peer_count":0}"#;

/// A policy, as the console's network types give one.
const POLICY: &str = concat!(
    r#"{"id":9,"name":"5-to-6","source_group":5,"dest_group":6,"protocol":"tcp","#,
    r#""ports":["22"],"bidirectional":true,"action":"accept","enabled":true,"is_managed":false}"#
);

/// A tenant summary with its isolation, as the switch answers it.
const SUMMARY: &str = r#"{"status":"active","groups":3,"isolation_mode":"deny_intra"}"#;

/// A topology, as swagger describes it.
const TOPOLOGY: &str = concat!(
    r#"{"nodes":[{"id":"group:5","type":"group","label":"build-boxes","meta":{"group_id":5}}],"#,
    r#""edges":[],"isolation_mode":"allow_intra"}"#
);

/// The live answer of a network read before the network exists (2026-11-03).
const NOT_SET_UP: &str = r#"{"detail":"You must call /api/network/provision/ first."}"#;

/// One command that sends one request: its arguments, with `--yes` when it is confirmed,
/// the request it sends, the status and body the server answers, the body it sends (`None`
/// for none), and what it prints after a 204 (`None` for the server's answer).
struct Single {
    args: &'static [&'static str],
    method: &'static str,
    path: &'static str,
    status: u16,
    answer: &'static str,
    sends: Option<&'static str>,
    done: Option<&'static str>,
}

/// Every command here that sends one request.
const SINGLE: [Single; 14] = [
    Single {
        args: &["network", "groups", "create", "--name", "build-boxes"],
        method: "POST",
        path: "/api/network/groups/",
        status: 201,
        answer: GROUP,
        sends: Some(r#"{"label":"build-boxes"}"#),
        done: None,
    },
    Single {
        args: &["network", "groups", "delete", "5", "--yes"],
        method: "DELETE",
        path: "/api/network/groups/5/",
        status: 204,
        answer: "",
        sends: None,
        done: Some(r#"{"id":5,"deleted":true}"#),
    },
    Single {
        args: &[
            "network",
            "policies",
            "create",
            "--source-group",
            "5",
            "--dest-group",
            "6",
            "--yes",
        ],
        method: "POST",
        path: "/api/network/policies/",
        status: 201,
        answer: POLICY,
        sends: Some(r#"{"source_group":5,"dest_group":6}"#),
        done: None,
    },
    Single {
        args: &[
            "network",
            "policies",
            "update",
            "9",
            "--enabled",
            "false",
            "--yes",
        ],
        method: "PATCH",
        path: "/api/network/policies/9/",
        status: 200,
        answer: POLICY,
        sends: Some(r#"{"enabled":false}"#),
        done: None,
    },
    Single {
        args: &["network", "policies", "delete", "9", "--yes"],
        method: "DELETE",
        path: "/api/network/policies/9/",
        status: 204,
        answer: "",
        sends: None,
        done: Some(r#"{"id":9,"deleted":true}"#),
    },
    Single {
        args: &["network", "vpcs", "create", "--name", "office"],
        method: "POST",
        path: "/api/network/networks/",
        status: 201,
        answer: r#"{"id":3,"name":"office"}"#,
        sends: Some(r#"{"name":"office"}"#),
        done: None,
    },
    Single {
        args: &["network", "vpcs", "delete", "3", "--yes"],
        method: "DELETE",
        path: "/api/network/networks/3/",
        status: 204,
        answer: "",
        sends: None,
        done: Some(r#"{"id":"3","deleted":true}"#),
    },
    Single {
        args: &[
            "network",
            "vpcs",
            "resources",
            "create",
            "3",
            "--name",
            "db",
            "--address",
            "10.0.0.5",
            "--yes",
        ],
        method: "POST",
        path: "/api/network/networks/3/resources/",
        status: 201,
        answer: r#"{"id":7,"name":"db","address":"10.0.0.5"}"#,
        sends: Some(r#"{"name":"db","address":"10.0.0.5"}"#),
        done: None,
    },
    Single {
        args: &[
            "network",
            "vpcs",
            "resources",
            "update",
            "3",
            "7",
            "--kind",
            "host",
            "--yes",
        ],
        method: "PATCH",
        path: "/api/network/networks/3/resources/7/",
        status: 200,
        answer: r#"{"id":7,"kind":"host"}"#,
        sends: Some(r#"{"kind":"host"}"#),
        done: None,
    },
    Single {
        args: &["network", "vpcs", "resources", "delete", "3", "7", "--yes"],
        method: "DELETE",
        path: "/api/network/networks/3/resources/7/",
        status: 204,
        answer: "",
        sends: None,
        done: Some(r#"{"id":7,"vpc_id":"3","deleted":true}"#),
    },
    Single {
        args: &[
            "network", "vpcs", "routers", "create", "3", "--peer", "12", "--yes",
        ],
        method: "POST",
        path: "/api/network/networks/3/routers/",
        status: 201,
        answer: r#"{"id":4,"peer":12}"#,
        sends: Some(r#"{"peer":12}"#),
        done: None,
    },
    Single {
        args: &["network", "vpcs", "routers", "delete", "3", "4", "--yes"],
        method: "DELETE",
        path: "/api/network/networks/3/routers/4/",
        status: 204,
        answer: "",
        sends: None,
        done: Some(r#"{"id":4,"vpc_id":"3","deleted":true}"#),
    },
    Single {
        args: &["network", "isolation", "get"],
        method: "GET",
        path: "/api/network/isolation/",
        status: 200,
        answer: r#"{"isolation_mode":"allow_intra"}"#,
        sends: None,
        done: None,
    },
    Single {
        args: &[
            "network",
            "isolation",
            "update",
            "--mode",
            "deny_intra",
            "--change-isolation",
            "--yes",
        ],
        method: "POST",
        path: "/api/network/isolation/",
        status: 201,
        answer: SUMMARY,
        sends: Some(r#"{"isolation_mode":"deny_intra"}"#),
        done: None,
    },
];

impl Single {
    /// A server that answers this command's request as scripted.
    fn server(&self) -> Server {
        Server::start(vec![route(
            self.method,
            self.path,
            Reply::json(self.status, self.answer),
        )])
    }
}

/// `text` as JSON.
fn parsed(text: &str) -> Value {
    serde_json::from_str(text).expect("a scripted body is JSON")
}

/// Each command sends its one request, with the body the flags give, in the wire order, and
/// prints the server's answer, or its own document after a 204.
#[test]
fn each_write_sends_its_request_and_prints_the_answer() {
    let home = Home::new();
    for case in &SINGLE {
        let server = case.server();
        let doc = Hipc::against(&home, &server).run(case.args).ok();
        let printed = parsed(case.done.unwrap_or(case.answer));
        assert_eq!(doc, printed, "{:?}", case.args);
        let request = server.only_request();
        assert_eq!(request.method, case.method, "{:?}", case.args);
        assert_eq!(request.target, case.path, "{:?}", case.args);
        assert_eq!(
            body_text(&request),
            case.sends.unwrap_or(""),
            "{:?}",
            case.args
        );
        assert_eq!(request.header("x-hippius-account"), None);
    }
}

/// The body of `request` as sent, as text, so a test pins the order of its fields.
fn body_text(request: &crate::common::Recorded) -> String {
    String::from_utf8(request.body.clone()).expect("a request body is UTF-8")
}

/// Every field a policy takes goes out as given, in the wire order, and a port only with a
/// protocol that has ports; `--port` repeats.
#[test]
fn a_policy_sends_every_field_given() {
    let server = Server::start(vec![
        route_of(&network::POLICY_CREATE, [], Reply::json(201, POLICY)),
        route_of(&network::POLICY_UPDATE, [&9], Reply::json(200, POLICY)),
    ]);
    let home = Home::new();
    Hipc::against(&home, &server)
        .run(&[
            "network",
            "policies",
            "create",
            "--source-group",
            "5",
            "--dest-group",
            "6",
            "--name",
            " ssh ",
            "--protocol",
            "tcp",
            "--port",
            "22",
            "--port",
            "8000-8080",
            "--bidirectional",
            "false",
            "--action",
            "drop",
            "--enabled",
            "true",
            "--yes",
        ])
        .ok();
    Hipc::against(&home, &server)
        .run(&[
            "network",
            "policies",
            "update",
            "9",
            "--dest-group",
            "7",
            "--source-group",
            "8",
            "--protocol",
            "udp",
            "--port",
            "53",
            "--yes",
        ])
        .ok();
    let requests = server.requests();
    assert_eq!(
        body_text(&requests[0]),
        concat!(
            r#"{"name":"ssh","source_group":5,"dest_group":6,"protocol":"tcp","#,
            r#""ports":["22","8000-8080"],"bidirectional":false,"action":"drop","enabled":true}"#
        )
    );
    assert_eq!(
        body_text(&requests[1]),
        r#"{"source_group":8,"dest_group":7,"protocol":"udp","ports":["53"]}"#
    );
}

/// A port with no protocol, or with one that has no ports, is refused before anything is
/// sent, a dry run included; an update, which reads nothing, must name the protocol too.
#[test]
fn a_port_without_a_protocol_that_has_ports_is_usage() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let create = [
        "network",
        "policies",
        "create",
        "--source-group",
        "5",
        "--dest-group",
        "6",
        "--port",
        "22",
        "--yes",
    ];
    let update = [
        "network", "policies", "update", "9", "--port", "22", "--yes",
    ];
    for args in [&create[..], &update[..]] {
        for protocol in [None, Some("all"), Some("icmp")] {
            let mut argv = args.to_vec();
            if let Some(protocol) = protocol {
                argv.extend(["--protocol", protocol]);
            }
            for dry in [false, true] {
                let mut argv = argv.clone();
                if dry {
                    argv.insert(0, "--dry-run");
                }
                let error = Hipc::against(&home, &server).run(&argv).fails(2, "usage");
                assert_eq!(
                    error["message"],
                    "`--port` needs `--protocol tcp` or `--protocol udp`: the other protocols \
                     have no ports"
                );
                assert_eq!(
                    error["hint"],
                    "pass `--protocol tcp` or `--protocol udp` with `--port`, or remove `--port`"
                );
            }
        }
    }
    assert!(server.requests().is_empty());
}

/// A resource takes its kind and groups; `--group` repeats, and the name is sent trimmed.
#[test]
fn a_resource_sends_every_field_given() {
    let server = Server::start(vec![
        route_of(
            &network::VPC_RESOURCE_CREATE,
            [&3],
            Reply::json(201, r#"{"id":7}"#),
        ),
        route_of(
            &network::VPC_RESOURCE_UPDATE,
            [&3, &7],
            Reply::json(200, r#"{"id":7}"#),
        ),
    ]);
    let home = Home::new();
    Hipc::against(&home, &server)
        .run(&[
            "network",
            "vpcs",
            "resources",
            "create",
            "3",
            "--name",
            " lan\n",
            "--address",
            "10.0.0.0/24",
            "--kind",
            "subnet",
            "--group",
            "5",
            "--group",
            "6",
            "--yes",
        ])
        .ok();
    Hipc::against(&home, &server)
        .run(&[
            "network",
            "vpcs",
            "resources",
            "update",
            "3",
            "7",
            "--name",
            "db",
            "--address",
            "db.internal",
            "--group",
            "5",
            "--yes",
        ])
        .ok();
    let requests = server.requests();
    assert_eq!(
        body_text(&requests[0]),
        r#"{"name":"lan","address":"10.0.0.0/24","kind":"subnet","groups":[5,6]}"#
    );
    assert_eq!(
        body_text(&requests[1]),
        r#"{"name":"db","address":"db.internal","groups":[5]}"#
    );
}

/// A router takes its masquerade and metric.
#[test]
fn a_router_sends_every_field_given() {
    let server = Server::start(vec![route_of(
        &network::VPC_ROUTER_CREATE,
        [&3],
        Reply::json(201, r#"{"id":4}"#),
    )]);
    Hipc::against(&Home::new(), &server)
        .run(&[
            "network",
            "vpcs",
            "routers",
            "create",
            "3",
            "--peer",
            "12",
            "--masquerade",
            "true",
            "--metric",
            "100",
            "--yes",
        ])
        .ok();
    assert_eq!(
        body_text(&server.only_request()),
        r#"{"peer":12,"masquerade":true,"metric":100}"#
    );
}

/// Without `--yes` on a pipe each confirmed write is refused, its refusal names what it
/// would do, and nothing is sent; a create of an empty group or VPC network is not gated.
#[test]
fn a_confirmed_write_without_yes_is_refused_and_sends_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    for (args, action) in [
        (
            vec!["network", "groups", "delete", "5"],
            "delete the network group `5`",
        ),
        (
            vec![
                "network",
                "policies",
                "create",
                "--source-group",
                "5",
                "--dest-group",
                "6",
            ],
            "create a network policy from the group `5` to the group `6`",
        ),
        (
            vec![
                "network", "policies", "update", "9", "--action", "drop", "--name", "x",
            ],
            "change the network policy `9`: `name` and `action`",
        ),
        (
            vec!["network", "policies", "delete", "9"],
            "delete the network policy `9`",
        ),
        (
            vec!["network", "vpcs", "delete", "3"],
            "delete the VPC network `3`",
        ),
        (
            vec![
                "network",
                "vpcs",
                "resources",
                "create",
                "3",
                "--name",
                "db",
                "--address",
                "a`b",
            ],
            r"add the resource `a\u{60}b` to the VPC network `3`",
        ),
        (
            vec![
                "network",
                "vpcs",
                "resources",
                "update",
                "3",
                "7",
                "--kind",
                "host",
            ],
            "change the resource `7` of the VPC network `3`: `kind`",
        ),
        (
            vec!["network", "vpcs", "resources", "delete", "3", "7"],
            "remove the resource `7` from the VPC network `3`",
        ),
        (
            vec!["network", "vpcs", "routers", "create", "3", "--peer", "12"],
            "make the peer `12` a router of the VPC network `3`",
        ),
        (
            vec!["network", "vpcs", "routers", "delete", "3", "4"],
            "remove the router `4` from the VPC network `3`",
        ),
        (
            vec![
                "network",
                "isolation",
                "update",
                "--mode",
                "allow_intra",
                "--change-isolation",
            ],
            "switch the private network's isolation to `allow_intra`",
        ),
    ] {
        let error = Hipc::against(&home, &server)
            .run(&args)
            .fails(2, "confirmation_required");
        assert_eq!(
            error["message"],
            format!("this would {action}, and `--yes` was not passed"),
            "{args:?}"
        );
        assert_eq!(error["hint"], "pass `--yes` to confirm");
    }
    assert!(server.requests().is_empty());
}

/// The isolation switch needs `--change-isolation` besides `--yes`, in either direction,
/// before the token is read; `--yes` does not imply it. Its refusal is `hipc api`'s on the
/// same route, and its hint repeats the command with the flag.
#[test]
fn the_isolation_switch_needs_change_isolation() {
    let server = Server::start(vec![]);
    let home = Home::new();
    for mode in ["allow_intra", "deny_intra"] {
        for yes in [&[][..], &["--yes"]] {
            let args = [&["network", "isolation", "update", "--mode", mode][..], yes].concat();
            let error = Hipc::tokenless(&home, &server)
                .env_remove("HIPPIUS_TOKEN")
                .run(&args)
                .fails(2, "change_isolation_required");
            assert_eq!(
                error["message"],
                "this would change which peers of the network can reach each other, and \
                 `--change-isolation` was not passed"
            );
            assert_eq!(
                error["hint"],
                format!("run `hipc {} --change-isolation`", args.join(" ")),
                "{args:?}"
            );
        }
    }
    assert!(server.requests().is_empty());
    // A mode the API does not name is refused by the parser.
    Hipc::against(&home, &server)
        .run(&[
            "network",
            "isolation",
            "update",
            "--mode",
            "deny-intra",
            "--change-isolation",
            "--yes",
        ])
        .fails(2, "usage");
}

/// A dry run needs no `--yes`, no token, and no `--change-isolation`, and lists the exact
/// request.
#[test]
fn dry_runs_list_the_exact_request_and_send_nothing() {
    let server = Server::start(vec![]);
    let home = Home::new();
    for case in &SINGLE {
        let (method, path) = (case.method, case.path);
        let args: Vec<&str> = case
            .args
            .iter()
            .copied()
            .filter(|arg| !matches!(*arg, "--yes" | "--change-isolation"))
            .collect();
        let doc = Hipc::tokenless(&home, &server)
            .env_remove("HIPPIUS_TOKEN")
            .run(&[&["--dry-run"], &args[..]].concat())
            .dry_ok();
        let requests = doc["requests"].as_array().unwrap();
        assert_eq!(requests.len(), 1, "{args:?}");
        let request = draft(&doc);
        assert_eq!(request["method"], method, "{args:?}");
        assert_eq!(
            request["url"],
            format!("{}{path}", server.url()),
            "{args:?}"
        );
    }
    let doc = Hipc::tokenless(&home, &server)
        .env_remove("HIPPIUS_TOKEN")
        .run(&["--dry-run", "network", "topology"])
        .dry_ok();
    let urls: Vec<&str> = doc["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|request| request["url"].as_str().unwrap())
        .collect();
    let url = |path: &str| format!("{}{path}", server.url());
    assert_eq!(
        urls,
        [url("/api/network/groups/"), url("/api/network/topology/")]
    );
    assert!(server.requests().is_empty());
}

/// The topology reads the group list first, which shows the network exists, then the
/// topology, and prints the topology as the server sent it.
#[test]
fn topology_reads_the_group_list_then_the_topology() {
    let server = Server::start(vec![
        route_of(&network::GROUPS, [], Reply::json(200, "[]")),
        route_of(&network::TOPOLOGY, [], Reply::json(200, TOPOLOGY)),
    ]);
    let doc = Hipc::against(&Home::new(), &server)
        .run(&["network", "topology"])
        .ok();
    assert_eq!(doc, serde_json::from_str::<Value>(TOPOLOGY).unwrap());
    assert_eq!(
        server.calls(),
        ["GET /api/network/groups/", "GET /api/network/topology/"]
    );
}

/// A network that was never set up answers the group list 403, so the topology, whose first
/// read would create the network, is never sent, and the error says how to set it up. A
/// failed topology read after the list is a read's failure: nothing was written.
#[test]
fn topology_of_a_network_never_set_up_sends_no_topology_read() {
    let server = Server::start(vec![route_of(
        &network::GROUPS,
        [],
        Reply::json(403, NOT_SET_UP),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["network", "topology"])
        .fails(1, "permission");
    assert_eq!(
        error["message"],
        "cannot list the network's groups: the private network is not set up for this account"
    );
    assert_eq!(
        error["hint"],
        format!(
            "run `hipc --api-url {} network create --yes` to create the account's private \
             network",
            server.url()
        )
    );
    assert_eq!(server.calls(), ["GET /api/network/groups/"]);
    let failing = Server::start(vec![
        route_of(&network::GROUPS, [], Reply::json(200, "[]")),
        route_of(
            &network::TOPOLOGY,
            [],
            Reply::json(502, r#"{"detail":"bad gateway"}"#),
        ),
    ]);
    let error = Hipc::against(&Home::new(), &failing)
        .run(&["network", "topology"])
        .fails(1, "unavailable");
    assert_eq!(error["side_effects"], "none");
    assert_eq!(error["retryable"], true);
}

/// A write the server refuses: its arguments, the request's method and path, the answer, and
/// the code and hint the refusal must have.
type Refusal = (
    &'static [&'static str],
    &'static str,
    &'static str,
    Reply,
    &'static str,
    Option<&'static str>,
);

/// The errors the API documents or the console names: a group of another account (400), a
/// managed policy or group (409), and a missing one (404, whose hint names the list).
#[test]
fn a_refused_write_keeps_its_code_and_names_the_list() {
    let home = Home::new();
    let cases: [Refusal; 8] = [
        (
            &[
                "network",
                "policies",
                "create",
                "--source-group",
                "5",
                "--dest-group",
                "99",
                "--yes",
            ],
            "POST",
            "/api/network/policies/",
            Reply::json(400, r#"{"dest_group":["not your group"]}"#),
            "validation",
            None,
        ),
        (
            &[
                "network",
                "policies",
                "update",
                "9",
                "--enabled",
                "false",
                "--yes",
            ],
            "PATCH",
            "/api/network/policies/9/",
            Reply::json(409, r#"{"detail":"managed"}"#),
            "conflict",
            None,
        ),
        (
            &["network", "groups", "delete", "1", "--yes"],
            "DELETE",
            "/api/network/groups/1/",
            Reply::json(409, r#"{"detail":"the default group"}"#),
            "conflict",
            None,
        ),
        (
            &["network", "policies", "delete", "9", "--yes"],
            "DELETE",
            "/api/network/policies/9/",
            Reply::not_found(),
            "not_found",
            Some("run `hipc network policies list` to see what exists"),
        ),
        (
            &["network", "vpcs", "resources", "delete", "3", "7", "--yes"],
            "DELETE",
            "/api/network/networks/3/resources/7/",
            Reply::not_found(),
            "not_found",
            Some("run `hipc network vpcs resources list 3` to see what exists"),
        ),
        (
            &["network", "vpcs", "routers", "delete", "3", "4", "--yes"],
            "DELETE",
            "/api/network/networks/3/routers/4/",
            Reply::not_found(),
            "not_found",
            Some("run `hipc network vpcs routers list 3` to see what exists"),
        ),
        (
            &["network", "vpcs", "delete", "3", "--yes"],
            "DELETE",
            "/api/network/networks/3/",
            Reply::not_found(),
            "not_found",
            Some("run `hipc network vpcs list` to see what exists"),
        ),
        (
            &["network", "groups", "delete", "5", "--yes"],
            "DELETE",
            "/api/network/groups/5/",
            Reply::not_found(),
            "not_found",
            Some("run `hipc network groups list` to see what exists"),
        ),
    ];
    for (args, method, path, reply, code, hint) in cases {
        let server = Server::start(vec![route(method, path, reply)]);
        let error = Hipc::against(&home, &server).run(args).fails(1, code);
        assert_eq!(error["side_effects"], "none", "{args:?}");
        if let Some(hint) = hint {
            assert_eq!(error["hint"], hint, "{args:?}");
        }
    }
}

/// A delete the server rejected changed nothing, so it is `none`; a 5xx after a write was
/// sent is `partial`.
#[test]
fn a_5xx_after_a_write_is_partial() {
    let server = Server::start(vec![route_of(
        &network::POLICY_DELETE,
        [&9],
        Reply::json(502, r#"{"detail":"bad gateway"}"#),
    )]);
    let error = Hipc::against(&Home::new(), &server)
        .run(&["network", "policies", "delete", "9", "--yes"])
        .fails(1, "unavailable");
    assert_eq!(error["side_effects"], "partial");
    assert_eq!(error["retryable"], false);
}

/// With a membership selected every route sends `X-Hippius-Account`; a project or a billing
/// account from a flag is refused before anything is sent; and a network never set up is
/// said plainly, with the command that sets it up.
#[test]
fn every_write_acts_in_the_selected_account_and_names_a_network_never_set_up() {
    let home = Home::new();
    for case in &SINGLE {
        let (args, method, path) = (case.args, case.method, case.path);
        let server = case.server();
        Hipc::against(&home, &server)
            .run(&[&["--account", MEMBER][..], args].concat())
            .ok();
        assert_eq!(
            server.only_request().header("x-hippius-account"),
            Some(MEMBER),
            "{args:?}"
        );
        for (flag, id, code) in [
            ("--project", PROJECT, "project_scope_unsupported"),
            ("--billing", BILLING, "billing_scope_unsupported"),
        ] {
            let refused = Server::start(vec![]);
            Hipc::against(&home, &refused)
                .run(&[&[flag, id][..], args].concat())
                .fails(2, code);
            assert!(refused.requests().is_empty());
        }
        let unset = Server::start(vec![route(method, path, Reply::json(403, NOT_SET_UP))]);
        let error = Hipc::against(&home, &unset)
            .run(args)
            .fails(1, "permission");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .ends_with(": the private network is not set up for this account"),
            "{error}"
        );
    }
}

/// A numeric id that is not plain digits, or a VPC network id that could change the route, is
/// refused before anything is sent, naming the list that prints the ids.
#[test]
fn an_id_outside_its_rule_is_usage() {
    let server = Server::start(vec![]);
    let home = Home::new();
    for (args, words) in [
        (
            vec!["network", "groups", "delete", "x5", "--yes"],
            "a group id is a whole number, from `hipc network groups list`",
        ),
        (
            vec!["network", "policies", "delete", "9x", "--yes"],
            "a policy id is a whole number",
        ),
        (
            vec!["network", "vpcs", "resources", "delete", "3", "7a", "--yes"],
            "a resource id is a whole number",
        ),
        (
            vec!["network", "vpcs", "routers", "delete", "3", "4.0", "--yes"],
            "a router id is a whole number",
        ),
        (
            vec![
                "network", "vpcs", "routers", "create", "3", "--peer", "p12", "--yes",
            ],
            "a peer id is a whole number",
        ),
        (
            vec!["network", "vpcs", "delete", "../3", "--yes"],
            "a VPC network id is 1 to 64",
        ),
    ] {
        let error = Hipc::against(&home, &server).run(&args).fails(2, "usage");
        assert!(
            error["message"].as_str().unwrap().contains(words),
            "{args:?}: {error}"
        );
    }
    assert!(server.requests().is_empty());
}

/// A name, an address, a port, or a metric outside its rule is refused before anything is
/// sent, naming the rule, and an update with nothing to change is refused by the parser.
#[test]
fn a_name_or_a_value_outside_its_rule_is_usage() {
    let server = Server::start(vec![]);
    let home = Home::new();
    let long = "a".repeat(51);
    for (args, words) in [
        (
            vec!["network", "groups", "create", "--name", "Build"],
            "a group name is 1 to 50 lowercase letters",
        ),
        (
            vec!["network", "groups", "create", "--name", long.as_str()],
            "a group name is 1 to 50",
        ),
        (
            vec!["network", "vpcs", "create", "--name", " "],
            "a VPC network name is one line of 1 to 255 characters",
        ),
        (
            vec![
                "network", "policies", "update", "9", "--name", "a\nb", "--yes",
            ],
            "a policy name holds no control character",
        ),
        (
            vec![
                "network",
                "vpcs",
                "resources",
                "create",
                "3",
                "--name",
                "",
                "--address",
                "x",
                "--yes",
            ],
            "a resource name is one line",
        ),
        (
            vec![
                "network",
                "vpcs",
                "resources",
                "create",
                "3",
                "--name",
                "db",
                "--address",
                "10.0.0.5 10.0.0.6",
                "--yes",
            ],
            "an address is an IP address, a CIDR subnet, or a domain",
        ),
        (
            vec![
                "network",
                "policies",
                "update",
                "9",
                "--protocol",
                "tcp",
                "--port",
                "70000",
                "--yes",
            ],
            "a port is a number from 1 to 65535",
        ),
        (
            vec![
                "network", "vpcs", "routers", "create", "3", "--peer", "12", "--metric", "1.5",
            ],
            "a metric is a whole number",
        ),
    ] {
        let error = Hipc::against(&home, &server).run(&args).fails(2, "usage");
        assert!(
            error["message"].as_str().unwrap().contains(words),
            "{args:?}: {error}"
        );
    }
    // An update with nothing to change is refused by the parser.
    for args in [
        &["network", "policies", "update", "9", "--yes"][..],
        &["network", "vpcs", "resources", "update", "3", "7", "--yes"],
    ] {
        Hipc::against(&home, &server).run(args).fails(2, "usage");
    }
    assert!(server.requests().is_empty());
}
