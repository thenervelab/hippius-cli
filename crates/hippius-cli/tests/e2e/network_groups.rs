//! `hipc network groups update`: a group's peers set as a whole list, one `POST .../members/`
//! per change, after the group list and the topology are read; a list that leaves out a
//! peer the group has needs `--remove-others`.

use hippius::routes::network;
use serde_json::{Value, json};

use crate::common::{Hipc, Home, MEMBER, PROJECT, Reply, Server, route_each_of, route_of};

/// The group list: the group `5`, `devs`, and the default group.
const GROUPS: &str = r#"[{"id":1,"label":"default","is_default":true},{"id":5,"label":"devs"}]"#;

/// A topology whose group `5` holds the peers `12` and `14`; `13` is in the default group
/// only. A group node and a resource node carry no peer meta.
const TOPOLOGY: &str = concat!(
    r#"{"nodes":["#,
    r#"{"id":"group:5","type":"group","label":"devs","meta":{"group_id":5}},"#,
    r#"{"id":"peer:12","type":"peer","label":"a","meta":{"peer_id":12,"groups":[1,5]}},"#,
    r#"{"id":"peer:13","type":"peer","label":"b","meta":{"peer_id":13,"groups":[1]}},"#,
    r#"{"id":"resource:3","type":"resource","label":"db","meta":{"resource_id":3}},"#,
    r#"{"id":"peer:14","type":"peer","label":"c","meta":{"peer_id":14,"groups":[5]}}"#,
    r#"],"edges":[],"isolation_mode":"allow_intra"}"#
);

/// The two reads every real run sends first, and a members route that answers `replies` in
/// order.
fn server(replies: Vec<Reply>) -> Server {
    Server::start(vec![
        route_of(&network::GROUPS, [], Reply::json(200, GROUPS)),
        route_of(&network::TOPOLOGY, [], Reply::json(200, TOPOLOGY)),
        route_each_of(&network::GROUP_MEMBERS_UPDATE, [&5], replies),
    ])
}

/// `hipc network groups update 5` with `extra`.
fn update(extra: &[&str]) -> Vec<String> {
    ["network", "groups", "update", "5"]
        .iter()
        .chain(extra)
        .map(|word| (*word).to_owned())
        .collect()
}

/// The bodies of the members writes `server` received, in order.
fn changes(server: &Server) -> Vec<Value> {
    server
        .requests()
        .iter()
        .filter(|request| request.method == "POST")
        .map(|request| {
            assert_eq!(request.target, "/api/network/groups/5/members/");
            request.json()
        })
        .collect()
}

fn run(home: &Home, server: &Server, args: &[String]) -> crate::common::Run {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    Hipc::against(home, server).run(&args)
}

/// A list that only adds sends one add per peer the group lacks, after the two reads, and
/// prints the group's new list with what changed. The body is the console's: an action and
/// a numeric peer id.
#[test]
fn an_add_sends_one_call_per_peer_the_group_lacks() {
    let server = server(vec![Reply::json(201, "{}")]);
    let doc = run(
        &Home::new(),
        &server,
        &update(&["--peer", "12", "--peer", "14", "--peer", "13", "--yes"]),
    )
    .ok();
    assert_eq!(
        doc,
        json!({"id": 5, "label": "devs", "peers": [12, 14, 13], "added": [13], "removed": []})
    );
    assert_eq!(
        server.calls(),
        [
            "GET /api/network/groups/",
            "GET /api/network/topology/",
            "GET /api/network/topology/",
            "POST /api/network/groups/5/members/",
        ]
    );
    let post = server.requests().pop().unwrap();
    assert_eq!(post.body, br#"{"action":"add","peer_id":13}"#);
}

/// A list that leaves out a peer the group has is refused after the reads, naming each peer
/// it would remove, and the hint repeats the command with `--remove-others`; nothing is
/// written.
#[test]
fn a_list_that_leaves_out_a_peer_needs_remove_others() {
    let server = server(vec![Reply::json(201, "{}")]);
    let home = Home::new();
    let error =
        run(&home, &server, &update(&["--peer", "13", "--yes"])).fails(2, "remove_others_required");
    assert_eq!(
        error["message"],
        "this would remove 2 peers that the network group `devs` (`5`) has now, and \
         `--remove-others` was not passed"
    );
    assert_eq!(
        error["hint"],
        "run `hipc network groups update 5 --peer 13 --yes --remove-others` to remove `12` \
         and `14`"
    );
    let error =
        run(&home, &server, &update(&["--peer", "12", "--yes"])).fails(2, "remove_others_required");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with("this would remove 1 peer that"),
        "{error}"
    );
    assert_eq!(changes(&server), Vec::<Value>::new());
}

/// With `--remove-others` the adds go first, then the removes, each its own call.
#[test]
fn remove_others_sends_the_adds_then_the_removes() {
    let server = server(vec![Reply::json(201, "{}")]);
    let doc = run(
        &Home::new(),
        &server,
        &update(&["--peer", "13", "--remove-others", "--yes"]),
    )
    .ok();
    assert_eq!(
        doc,
        json!({"id": 5, "label": "devs", "peers": [13], "added": [13], "removed": [12, 14]})
    );
    assert_eq!(
        changes(&server),
        [
            json!({"action": "add", "peer_id": 13}),
            json!({"action": "remove", "peer_id": 12}),
            json!({"action": "remove", "peer_id": 14}),
        ]
    );
}

/// The group's own list sends nothing and asks nothing, so it needs no `--yes`; a peer
/// given twice counts once.
#[test]
fn the_group_s_own_list_changes_nothing_and_asks_nothing() {
    let server = server(vec![Reply::json(201, "{}")]);
    let doc = run(
        &Home::new(),
        &server,
        &update(&["--peer", "14", "--peer", "12", "--peer", "14"]),
    )
    .ok();
    assert_eq!(
        doc,
        json!({"id": 5, "label": "devs", "peers": [12, 14], "added": [], "removed": []})
    );
    assert_eq!(changes(&server), Vec::<Value>::new());
}

/// A change without `--yes` on a pipe is refused after the reads, which name the group and
/// the change, and nothing is written.
#[test]
fn a_change_without_yes_is_refused_after_the_reads() {
    let server = server(vec![Reply::json(201, "{}")]);
    let error = run(
        &Home::new(),
        &server,
        &update(&["--peer", "13", "--remove-others"]),
    )
    .fails(2, "confirmation_required");
    assert_eq!(
        error["message"],
        "this would set the peers of the network group `devs` (`5`): add the peer `13` and \
         remove the peers `12` and `14`, and `--yes` was not passed"
    );
    assert_eq!(
        server.calls(),
        ["GET /api/network/groups/", "GET /api/network/topology/"]
    );
}

/// A dry run sends nothing and needs no token or `--yes`: it lists the reads, the topology
/// twice (before the question and after it), and the write, with placeholders for what only the
/// reads give, and the topology as the read the guard compares.
#[test]
fn a_dry_run_lists_the_reads_and_a_placeholder_write() {
    let server = Server::start(vec![]);
    let doc = Hipc::tokenless(&Home::new(), &server)
        .env_remove("HIPPIUS_TOKEN")
        .run(&[
            "--dry-run",
            "network",
            "groups",
            "update",
            "5",
            "--peer",
            "12",
        ])
        .dry_ok();
    let url = |path: &str| format!("{}{path}", server.url());
    let read = |path: &str| {
        json!({
            "method": "GET",
            "url": url(path),
            "account_header": null,
            "project_header": null,
            "billing_header": null,
            "body": null,
        })
    };
    assert_eq!(
        doc,
        json!({
            "dry_run": true,
            "requests": [
                read("/api/network/groups/"),
                read("/api/network/topology/"),
                read("/api/network/topology/"),
                {
                    "method": "POST",
                    "url": url("/api/network/groups/5/members/"),
                    "account_header": null,
                    "project_header": null,
                    "billing_header": null,
                    "body": {
                        "action": "<add or remove, from the reads>",
                        "peer_id": "<each peer whose membership changes>",
                    },
                },
            ],
            "reads_first": read("/api/network/topology/"),
        })
    );
    assert!(server.requests().is_empty());
}

/// A group the list does not show, or a peer the topology does not, is `not_found` before
/// any write, and the hint names the list that shows what exists.
#[test]
fn a_missing_group_or_peer_is_not_found_before_any_write() {
    let home = Home::new();
    let server = server(vec![Reply::json(201, "{}")]);
    let args = ["network", "groups", "update", "6", "--peer", "12", "--yes"].map(String::from);
    let error = run(&home, &server, &args).fails(1, "not_found");
    assert_eq!(
        error["message"],
        "cannot set the peers of the network group `6`: the network has no group `6`"
    );
    assert_eq!(
        error["hint"],
        "run `hipc network groups list` to see what exists"
    );
    assert_eq!(error["http_status"], Value::Null);
    assert_eq!(server.calls(), ["GET /api/network/groups/"]);
    let server = self::server(vec![Reply::json(201, "{}")]);
    let error = run(
        &home,
        &server,
        &update(&["--peer", "12", "--peer", "99", "--yes"]),
    )
    .fails(1, "not_found");
    assert_eq!(
        error["message"],
        "cannot set the peers of the network group `5`: the network has no peer `99`"
    );
    assert_eq!(
        error["hint"],
        "run `hipc network peers list` to see what exists"
    );
    assert_eq!(changes(&server), Vec::<Value>::new());
}

/// A topology whose peer nodes `hipc` cannot read fails closed: `internal`, before any
/// write, rather than a group taken as empty.
#[test]
fn a_topology_hipc_cannot_read_is_internal_and_writes_nothing() {
    let server = Server::start(vec![
        route_of(&network::GROUPS, [], Reply::json(200, GROUPS)),
        route_of(
            &network::TOPOLOGY,
            [],
            Reply::json(200, r#"{"nodes":[{"type":"peer","meta":{"peer_id":12}}]}"#),
        ),
    ]);
    let error =
        run(&Home::new(), &server, &update(&["--peer", "12", "--yes"])).fails(1, "internal");
    assert_eq!(
        error["message"],
        "cannot read the network's topology: the API answered with a shape `hipc` does not read"
    );
    assert_eq!(error["side_effects"], "none");
}

/// A first change the server refuses is that change's own error, with nothing done: a
/// managed group answers 409.
#[test]
fn a_first_change_refused_is_its_own_error() {
    let server = server(vec![Reply::json(409, r#"{"detail":"managed group"}"#)]);
    let error = run(
        &Home::new(),
        &server,
        &update(&["--peer", "12", "--peer", "14", "--peer", "13", "--yes"]),
    )
    .fails(1, "conflict");
    assert_eq!(
        error["message"],
        "cannot add the peer `13` to the network group `devs` (`5`): the API says `managed \
         group`"
    );
    assert_eq!(error["side_effects"], "none");
}

/// A change that fails after another went through is `partial_failure`: stdout holds the
/// group's peers as they now are and what was done, `details` the step that failed, and the
/// hint says that running the command again sends only what is left.
#[test]
fn a_change_that_fails_after_another_is_a_partial_failure() {
    let server = server(vec![
        Reply::json(201, "{}"),
        Reply::json(502, r#"{"detail":"bad gateway"}"#),
    ]);
    let run = run(
        &Home::new(),
        &server,
        &update(&["--peer", "13", "--remove-others", "--yes"]),
    );
    let error = run.fails_keeping_stdout(1, "partial_failure");
    assert_eq!(
        error["message"],
        "changed the peers of the network group `devs` (`5`) in part: added the peer `13`; \
         then cannot remove the peer `12` from the network group `devs` (`5`)"
    );
    assert_eq!(
        error["hint"],
        "run the same command again: it reads the group's peers first and sends only the \
         changes left"
    );
    assert_eq!(error["side_effects"], "partial");
    assert_eq!(
        error["details"][0]["step"],
        "remove the peer `12` from the network group `devs` (`5`)"
    );
    assert_eq!(error["details"][0]["code"], "unavailable");
    let doc: Value = serde_json::from_str(run.stdout()).unwrap();
    assert_eq!(
        doc,
        json!({"id": 5, "label": "devs", "peers": [12, 14, 13], "added": [13], "removed": []})
    );
    assert_eq!(changes(&server).len(), 2);
}

/// A group with no label is named by its id, and the document carries `null`.
#[test]
fn a_group_without_a_label_is_named_by_its_id() {
    let server = Server::start(vec![
        route_of(&network::GROUPS, [], Reply::json(200, r#"[{"id":"5"}]"#)),
        route_of(&network::TOPOLOGY, [], Reply::json(200, TOPOLOGY)),
    ]);
    let error =
        run(&Home::new(), &server, &update(&["--peer", "13"])).fails(2, "remove_others_required");
    assert_eq!(
        error["message"],
        "this would remove 2 peers that the network group `5` has now, and `--remove-others` \
         was not passed"
    );
    let doc = run(
        &Home::new(),
        &server,
        &update(&["--peer", "12", "--peer", "14"]),
    )
    .ok();
    assert_eq!(doc["label"], Value::Null);
}

/// A network never set up answers the group list 403, so the topology, whose first read
/// would create the network, is never sent.
#[test]
fn a_network_never_set_up_sends_no_topology_read() {
    let server = Server::start(vec![route_of(
        &network::GROUPS,
        [],
        Reply::json(
            403,
            r#"{"detail":"You must call /api/network/provision/ first."}"#,
        ),
    )]);
    let error =
        run(&Home::new(), &server, &update(&["--peer", "12", "--yes"])).fails(1, "permission");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .ends_with(": the private network is not set up for this account"),
        "{error}"
    );
    assert_eq!(server.calls(), ["GET /api/network/groups/"]);
}

/// Every request carries the membership's header, and a project is refused before any is
/// sent.
#[test]
fn a_membership_reaches_every_request_and_a_project_is_refused() {
    let server = server(vec![Reply::json(201, "{}")]);
    let home = Home::new();
    let args = [
        "--account",
        MEMBER,
        "network",
        "groups",
        "update",
        "5",
        "--peer",
        "13",
        "--peer",
        "12",
        "--peer",
        "14",
        "--yes",
    ];
    Hipc::against(&home, &server).run(&args).ok();
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    for request in requests {
        assert_eq!(request.header("x-hippius-account"), Some(MEMBER));
    }
    let refused = Server::start(vec![]);
    Hipc::against(&home, &refused)
        .run(&[
            "--project",
            PROJECT,
            "network",
            "groups",
            "update",
            "5",
            "--peer",
            "12",
        ])
        .fails(2, "project_scope_unsupported");
    assert!(refused.requests().is_empty());
}

/// `--peer` is required, and takes a numeric id.
#[test]
fn a_peer_is_required_and_numeric() {
    let server = Server::start(vec![]);
    let home = Home::new();
    Hipc::against(&home, &server)
        .run(&["network", "groups", "update", "5", "--yes"])
        .fails(2, "usage");
    let error = Hipc::against(&home, &server)
        .run(&["network", "groups", "update", "5", "--peer", "a", "--yes"])
        .fails(2, "usage");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("a peer id is a whole number, from `hipc network peers list`"),
        "{error}"
    );
    assert!(server.requests().is_empty());
}

/// A topology whose group `5` holds `12` and `14` and the managed database `20`.
const WITH_MANAGED: &str = concat!(
    r#"{"nodes":["#,
    r#"{"type":"peer","meta":{"peer_id":12,"groups":[5]}},"#,
    r#"{"type":"peer","meta":{"peer_id":14,"groups":[5]}},"#,
    r#"{"type":"peer","meta":{"peer_id":20,"groups":[5],"managed_service":true}},"#,
    r#"{"type":"peer","meta":{"peer_id":21,"groups":[],"managed_service":true}}"#,
    r#"]}"#
);

/// A managed database's membership is Hippius's: a list that would remove one, or add one, is
/// refused before the question with a hint that keeps the list as it must be, and nothing is
/// written; a list that keeps it in the group goes through.
#[test]
fn a_managed_database_is_neither_added_nor_removed() {
    let server = Server::start(vec![
        route_of(&network::GROUPS, [], Reply::json(200, GROUPS)),
        route_of(&network::TOPOLOGY, [], Reply::json(200, WITH_MANAGED)),
        route_each_of(
            &network::GROUP_MEMBERS_UPDATE,
            [&5],
            vec![Reply::json(201, "{}")],
        ),
    ]);
    let home = Home::new();
    for (extra, hint) in [
        (
            &["--peer", "12", "--peer", "14", "--remove-others", "--yes"][..],
            "add `--peer 20` to keep it in the group",
        ),
        (
            &[
                "--peer", "12", "--peer", "14", "--peer", "20", "--peer", "21", "--yes",
            ],
            "remove `--peer 21`",
        ),
    ] {
        let error = run(&home, &server, &update(extra)).fails(2, "usage");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .ends_with("is a managed database, whose groups Hippius keeps"),
            "{error}"
        );
        assert_eq!(error["hint"], hint);
    }
    assert_eq!(changes(&server), Vec::<Value>::new());
    let doc = run(
        &home,
        &server,
        &update(&["--peer", "20", "--remove-others", "--yes"]),
    )
    .ok();
    assert_eq!(doc["peers"], json!([20]));
    assert_eq!(
        changes(&server),
        [
            json!({"action": "remove", "peer_id": 12}),
            json!({"action": "remove", "peer_id": 14}),
        ]
    );
}

/// The topology is read again after the question, and the changes are decided from that
/// read: a peer that joined the group meanwhile is a removal nobody confirmed, so without
/// `--remove-others` it is `conflict` and nothing is written; with it, the list given stands
/// and the joined peer is removed. A peer added meanwhile is not added twice.
#[test]
fn the_topology_read_after_the_question_decides_the_changes() {
    let joined = TOPOLOGY.replace(r#""groups":[1]}"#, r#""groups":[1,5]}"#);
    let server = |remove_others_reply: Vec<Reply>| {
        Server::start(vec![
            route_of(&network::GROUPS, [], Reply::json(200, GROUPS)),
            route_each_of(
                &network::TOPOLOGY,
                [],
                vec![Reply::json(200, TOPOLOGY), Reply::json(200, &joined)],
            ),
            route_each_of(&network::GROUP_MEMBERS_UPDATE, [&5], remove_others_reply),
        ])
    };
    let home = Home::new();
    let quiet = server(vec![Reply::json(201, "{}")]);
    let error = run(
        &home,
        &quiet,
        &update(&["--peer", "12", "--peer", "14", "--peer", "15", "--yes"]),
    )
    .fails(1, "not_found");
    assert!(
        error["message"].as_str().unwrap().contains("no peer `15`"),
        "{error}"
    );
    let racing = server(vec![Reply::json(201, "{}")]);
    let args = update(&["--peer", "12", "--remove-others", "--yes"]);
    let doc = run(&home, &racing, &args).ok();
    // The first read showed `12` and `14`; the second shows `13` joined, so it goes too.
    assert_eq!(doc["removed"], json!([13, 14]));
    assert_eq!(doc["peers"], json!([12]));
    let racing = server(vec![Reply::json(201, "{}")]);
    let error = run(
        &home,
        &racing,
        &update(&["--peer", "12", "--peer", "14", "--peer", "13", "--yes"]),
    )
    .ok();
    // `13` joined between the reads, so the add the question named is not sent again.
    assert_eq!(error["added"], json!([]));
    assert_eq!(changes(&racing), Vec::<Value>::new());
    // With an add to make, `13` joining between the reads is a removal nobody confirmed.
    let with_15 = |topology: &str| {
        topology.replace(
            r#"{"id":"resource:3""#,
            r#"{"type":"peer","meta":{"peer_id":15,"groups":[]}},{"id":"resource:3""#,
        )
    };
    let racing = Server::start(vec![
        route_of(&network::GROUPS, [], Reply::json(200, GROUPS)),
        route_each_of(
            &network::TOPOLOGY,
            [],
            vec![
                Reply::json(200, &with_15(TOPOLOGY)),
                Reply::json(200, &with_15(&joined)),
            ],
        ),
    ]);
    let error = run(
        &home,
        &racing,
        &update(&["--peer", "12", "--peer", "14", "--peer", "15", "--yes"]),
    )
    .fails(1, "conflict");
    assert_eq!(
        error["message"],
        "cannot set the peers of the network group `5`: the peer `13` joined the group after \
         it was read, and `--remove-others` was not passed"
    );
    assert_eq!(error["http_status"], Value::Null);
    assert_eq!(changes(&racing), Vec::<Value>::new());
}

/// With `--remove-others` and no `--peer` the group is emptied; without either, the parser
/// refuses.
#[test]
fn remove_others_without_a_peer_empties_the_group() {
    let server = server(vec![Reply::json(201, "{}")]);
    let home = Home::new();
    let doc = run(&home, &server, &update(&["--remove-others", "--yes"])).ok();
    assert_eq!(
        doc,
        json!({"id": 5, "label": "devs", "peers": [], "added": [], "removed": [12, 14]})
    );
    run(&home, &server, &update(&["--yes"])).fails(2, "usage");
}

/// A failure during the adds, after one went through, is `partial_failure` too, and its
/// stdout holds the group's peers with the one add that was made.
#[test]
fn a_failure_during_the_adds_is_a_partial_failure() {
    let topology = TOPOLOGY.replace(
        r#"{"id":"resource:3""#,
        concat!(
            r#"{"id":"peer:15","type":"peer","label":"d","meta":{"peer_id":15,"groups":[]}},"#,
            r#"{"id":"resource:3""#
        ),
    );
    let server = Server::start(vec![
        route_of(&network::GROUPS, [], Reply::json(200, GROUPS)),
        route_of(&network::TOPOLOGY, [], Reply::json(200, &topology)),
        route_each_of(
            &network::GROUP_MEMBERS_UPDATE,
            [&5],
            vec![
                Reply::json(201, "{}"),
                Reply::json(409, r#"{"detail":"no"}"#),
            ],
        ),
    ]);
    let run = run(
        &Home::new(),
        &server,
        &update(&[
            "--peer", "12", "--peer", "14", "--peer", "13", "--peer", "15", "--yes",
        ]),
    );
    let error = run.fails_keeping_stdout(1, "partial_failure");
    assert_eq!(
        error["details"][0]["step"],
        "add the peer `15` to the network group `devs` (`5`)"
    );
    assert_eq!(error["details"][0]["code"], "conflict");
    let doc: Value = serde_json::from_str(run.stdout()).unwrap();
    assert_eq!(
        doc,
        json!({"id": 5, "label": "devs", "peers": [12, 14, 13], "added": [13], "removed": []})
    );
}
