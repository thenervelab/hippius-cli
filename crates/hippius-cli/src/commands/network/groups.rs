//! `hipc network groups update`: set which peers are in a group.
//!
//! The API moves one peer at a time (`POST .../members/` with `add` or `remove`; the console's
//! network types say there is no bulk form), so the command makes the group's list the one
//! given by reading who is in the group now and sending one call per change. A peer that
//! leaves a group loses the access its policies give, so a list that leaves out a peer the
//! group has is refused without `--remove-others`, as every list replace is (tenet 5).
//!
//! Before the question it reads the group list, which names the group and answers 403 when the
//! network was never set up, then the topology, whose peer nodes say which groups each peer
//! is in. The topology's first read for an account would create the network, but after the
//! group list answered the network exists, so it is sent as a read. Both reads fail closed:
//! an answer `hipc` cannot read is `internal`, never an empty group, so a shape it does not
//! expect cannot turn the guard off.
//!
//! The changes go out adds first, so a peer moved between lists is never left with less
//! access in between than the end state gives. A failure after one went through prints what
//! was done, as `partial_failure`; a run of the same command reads again and sends only what
//! is left.

use hippius::routes::network::{self, Group, GroupMemberBody, MemberAction, Topology};
use hippius::{Client, Error, Failure, Request, Result, SideEffects, phrase, view_of};
use serde_json::{Value, json};

use crate::commands::network::topology_reads;
use crate::commands::{Printed, failed_part, unreadable};
use crate::context::{Confirmed, Context, Write, WritePlan};

/// What a change of a group's peers does, for its question.
const MEMBERS_CHANGE: &str =
    "a peer added gains the access the group's policies give, and a peer removed loses it";

/// The hint of a failure after some changes went through.
const RUN_AGAIN: &str = "run the same command again: it reads the group's peers first and \
                         sends only the changes left";

/// Makes `peers` the whole list of the group `group_id`: the steps the module docs give.
///
/// # Errors
///
/// The refusals of [`Context::check_write`] and [`Context::confirm_write`]; a read's error;
/// `internal` for a read whose answer `hipc` cannot read; `not_found` for a group or a peer
/// the reads do not show; `remove_others_required`; and a change's error, as
/// `partial_failure` after another went through.
pub(super) fn set_peers(
    ctx: &Context<'_>,
    group_id: u64,
    peers: &[u64],
    remove_others: bool,
) -> Result<Printed> {
    let client = ctx.load_authed_client()?;
    let (groups, topology) = topology_reads();
    // The dry run shows the change with placeholders: which peers are added or removed is
    // known only from the reads.
    let draft = Request::to(&network::GROUP_MEMBERS_UPDATE, [&group_id])
        .json(&json!({
            "action": "<add or remove, from the reads>",
            "peer_id": "<each peer whose membership changes>",
        }))
        .about(unnamed_action(group_id));
    // One question covers every change, so the steps of `read_then_write` do not fit.
    let plan = WritePlan::new(&[&groups, &topology], &draft);
    let checked = match ctx.check_write(&client, &plan)? {
        Write::DryRun(doc) => {
            return Ok(doc.reads_first(ctx.describe(&client, &topology)?).into());
        }
        Write::Checked(checked) => checked,
    };
    let action = unnamed_action(group_id);
    let (name, label) = group_name(&ctx.send(&client, &groups)?.json()?, group_id, &action)?;
    let group = Target { id: group_id, name };
    let now = members(&ctx.send(&client, &topology)?.json()?, group_id)?;
    let change = Change::between(&now.members, peers);
    refuse(ctx, &change, &now, &group, remove_others)?;
    let document = |done: &Change| {
        json!({
            "id": group_id,
            "label": label,
            "peers": change.peers,
            "added": done.added,
            "removed": done.removed,
        })
    };
    if change.added.is_empty() && change.removed.is_empty() {
        // Nothing to send, so nothing to ask about (the row asks only when the list changes).
        let _nothing_sent = ctx.unasked(checked);
        return Ok(Printed::Document(document(&change)));
    }
    let name = &group.name;
    let question = format!(
        "set the peers of the network group {name}: {}",
        change.said()
    );
    let confirmed = ctx.confirm_write(checked, &question, Some(MEMBERS_CHANGE))?;
    let (done, stopped) = apply(ctx, &client, &confirmed, &group, &change);
    let Some((step, error)) = stopped else {
        return Ok(Printed::Document(document(&done)));
    };
    if done.added.is_empty() && done.removed.is_empty() {
        return Err(error);
    }
    Err(Error::partial_failure(
        format!(
            "changed the peers of the network group {name} in part: {}; then cannot {step}",
            done.said_done()
        ),
        document(&done),
        Value::Array(vec![failed_part("step", &step, &error)]),
        SideEffects::Partial,
    )
    .with_hint(RUN_AGAIN))
}

/// The group a command acts on: its id, and its name as a message gives it.
#[derive(Debug)]
struct Target {
    id: u64,
    /// The label and the id, or the id alone ([`group_name`]).
    name: String,
}

/// What the command does, as a refusal before the group list names the group.
fn unnamed_action(group_id: u64) -> String {
    format!("set the peers of the network group `{group_id}`")
}

/// Refuses `change` before the question: a peer it adds that the network does not have is
/// `not_found`, and without `remove_others` a peer it removes is `remove_others_required`.
fn refuse(
    ctx: &Context<'_>,
    change: &Change,
    now: &Members,
    group: &Target,
    remove_others: bool,
) -> Result<()> {
    if let Some(&missing) = change.added.iter().find(|peer| !now.peers.contains(peer)) {
        return Err(Error::new(
            Failure::NotFound,
            format!(
                "cannot {}: the network has no peer `{missing}`",
                unnamed_action(group.id)
            ),
        )
        .with_hint("run `hipc network peers list` to see what exists"));
    }
    if remove_others || change.removed.is_empty() {
        return Ok(());
    }
    let removed: Vec<String> = change
        .removed
        .iter()
        .map(|peer| format!("`{peer}`"))
        .collect();
    Err(Error::remove_others_required(
        format!(
            "this would remove {} that the network group {} has now, and \
             `--remove-others` was not passed",
            phrase::count(removed.len() as u64, "peer"),
            group.name,
        ),
        ctx.args(),
        &removed,
    ))
}

/// Sends each change of `change`, adds first, until one fails: what went through, and the
/// step that failed with its error, if one did.
fn apply(
    ctx: &Context<'_>,
    client: &Client,
    confirmed: &Confirmed,
    group: &Target,
    change: &Change,
) -> (Change, Option<(String, Error)>) {
    let steps = change
        .added
        .iter()
        .map(|&peer| (MemberAction::Add, peer))
        .chain(
            change
                .removed
                .iter()
                .map(|&peer| (MemberAction::Remove, peer)),
        );
    let mut done = Change {
        peers: Vec::new(),
        added: Vec::new(),
        removed: Vec::new(),
    };
    let name = &group.name;
    for (action, peer) in steps {
        let step = match action {
            MemberAction::Add => format!("add the peer `{peer}` to the network group {name}"),
            MemberAction::Remove => {
                format!("remove the peer `{peer}` from the network group {name}")
            }
        };
        let request = Request::to(&network::GROUP_MEMBERS_UPDATE, [&group.id])
            .json(&GroupMemberBody {
                action,
                peer_id: peer,
            })
            .listed_by("hipc network groups list")
            .about(step.clone());
        if let Err(error) = ctx.send_confirmed(client, &request, confirmed) {
            return (done, Some((step, error)));
        }
        match action {
            MemberAction::Add => done.added.push(peer),
            MemberAction::Remove => done.removed.push(peer),
        }
    }
    (done, None)
}

/// The group `group_id` as a message names it, from the group list's answer: its label and
/// its id, or its id alone when it has no label; and the label.
///
/// # Errors
///
/// `internal` when the answer is not a list read whole; `not_found` when no group has the id.
fn group_name(answer: &Value, group_id: u64, action: &str) -> Result<(String, Option<String>)> {
    let items = hippius::page(answer.clone())
        .map_err(|_| unreadable("list the network's groups"))?
        .into_complete("/network/groups/")?;
    let id = group_id.to_string();
    let Some(group) = items
        .iter()
        .map(view_of::<Group>)
        .find(|group| group.id.as_deref() == Some(id.as_str()))
    else {
        return Err(Error::new(
            Failure::NotFound,
            format!("cannot {action}: the network has no group `{group_id}`"),
        )
        .with_hint("run `hipc network groups list` to see what exists"));
    };
    let name = match &group.label {
        Some(label) => format!("{} (`{group_id}`)", phrase::quote(label)),
        None => format!("`{group_id}`"),
    };
    Ok((name, group.label))
}

/// Who is in the group now, from the topology.
#[derive(Debug, Default)]
struct Members {
    /// Every peer of the network, by its id.
    peers: Vec<u64>,
    /// The peers in the group, in the topology's order.
    members: Vec<u64>,
}

/// The network's peers, and those in the group `group_id`, from the topology's answer.
///
/// # Errors
///
/// `internal` when the answer has no list of nodes, or a peer node without its id or its
/// list of groups: the guard must see every peer's groups, so a node it cannot read is not
/// taken as a peer outside the group.
fn members(answer: &Value, group_id: u64) -> Result<Members> {
    let unread = || unreadable("read the network's topology");
    let topology: Topology = view_of(answer);
    let mut found = Members::default();
    for node in topology.nodes.ok_or_else(unread)? {
        if node.kind.as_deref() != Some("peer") {
            continue;
        }
        let meta = node.meta.unwrap_or_default();
        let (Some(peer), Some(groups)) = (meta.peer_id, meta.groups) else {
            return Err(unread());
        };
        found.peers.push(peer);
        if groups.contains(&group_id) {
            found.members.push(peer);
        }
    }
    Ok(found)
}

/// The change from one list of a group's peers to another.
#[derive(Debug, PartialEq, Eq)]
struct Change {
    /// The new list: each peer given, once, in the order given.
    peers: Vec<u64>,
    /// The peers given that the group does not have, in the order given.
    added: Vec<u64>,
    /// The peers the group has that are not given, in the group's order.
    removed: Vec<u64>,
}

impl Change {
    /// The change from `now` to `given`; a peer given twice counts once.
    fn between(now: &[u64], given: &[u64]) -> Self {
        let mut peers: Vec<u64> = Vec::with_capacity(given.len());
        for &peer in given {
            if !peers.contains(&peer) {
                peers.push(peer);
            }
        }
        let added = peers
            .iter()
            .copied()
            .filter(|peer| !now.contains(peer))
            .collect();
        let removed = now
            .iter()
            .copied()
            .filter(|peer| !peers.contains(peer))
            .collect();
        Self {
            peers,
            added,
            removed,
        }
    }

    /// The change as a question names it, such as: add the peer `12` and remove the peers
    /// `13` and `14`.
    fn said(&self) -> String {
        self.parts("add", "remove")
    }

    /// What went through, as a partial failure names it.
    fn said_done(&self) -> String {
        self.parts("added", "removed")
    }

    fn parts(&self, add: &str, remove: &str) -> String {
        let mut parts = Vec::new();
        for (verb, peers) in [(add, &self.added), (remove, &self.removed)] {
            if !peers.is_empty() {
                parts.push(format!(
                    "{verb} the {} {}",
                    phrase::plural(peers.len() as u64, "peer"),
                    phrase::ticked(peers, "and")
                ));
            }
        }
        phrase::join(&parts, "and")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_adds_what_is_given_and_removes_what_is_left_out() {
        let change = Change::between(&[1, 2, 3], &[4, 2, 4, 5]);
        assert_eq!(
            change,
            Change {
                peers: vec![4, 2, 5],
                added: vec![4, 5],
                removed: vec![1, 3],
            }
        );
        assert_eq!(
            change.said(),
            "add the peers `4` and `5` and remove the peers `1` and `3`"
        );
        assert_eq!(
            change.said_done(),
            "added the peers `4` and `5` and removed the peers `1` and `3`"
        );
        let same = Change::between(&[2, 1], &[1, 2]);
        assert!(same.added.is_empty() && same.removed.is_empty());
        assert_eq!(same.said(), "");
        assert_eq!(Change::between(&[], &[7]).said(), "add the peer `7`");
        assert_eq!(Change::between(&[7, 8], &[8]).said(), "remove the peer `7`");
    }

    #[test]
    fn the_members_are_the_peers_whose_groups_hold_the_group() {
        let answer = json!({"nodes": [
            {"type": "group", "meta": {"group_id": 5}},
            {"type": "peer", "meta": {"peer_id": 12, "groups": [1, 5]}},
            {"type": "peer", "meta": {"peer_id": 13, "groups": [1]}},
            {"type": "resource", "meta": {}},
            {"type": "peer", "meta": {"peer_id": 14, "groups": [5]}},
        ]});
        let found = members(&answer, 5).unwrap();
        assert_eq!(found.peers, [12, 13, 14]);
        assert_eq!(found.members, [12, 14]);
        let none = members(&json!({"nodes": []}), 5).unwrap();
        assert!(none.peers.is_empty() && none.members.is_empty());
    }

    #[test]
    fn a_topology_hipc_cannot_read_fails_closed() {
        for answer in [
            json!([]),
            json!({}),
            json!({"nodes": {}}),
            json!({"nodes": [{"type": "peer"}]}),
            json!({"nodes": [{"type": "peer", "meta": {"peer_id": 12}}]}),
            json!({"nodes": [{"type": "peer", "meta": {"groups": [5]}}]}),
            json!({"nodes": [{"type": "peer", "meta": {"peer_id": "12", "groups": [5]}}]}),
            json!({"nodes": [{"type": "peer", "meta": {"peer_id": 12, "groups": ["5"]}}]}),
        ] {
            let error = members(&answer, 5).unwrap_err();
            assert_eq!(error.code(), hippius::Code::Internal, "{answer}");
            assert_eq!(
                error.message(),
                "cannot read the network's topology: the API answered with a shape `hipc` \
                 does not read"
            );
        }
    }

    #[test]
    fn a_group_is_named_by_its_label_and_id_and_a_missing_one_is_not_found() {
        let answer = json!([{"id": 5, "label": "devs"}, {"id": "6"}]);
        assert_eq!(
            group_name(&answer, 5, "x").unwrap(),
            ("`devs` (`5`)".to_owned(), Some("devs".to_owned()))
        );
        assert_eq!(
            group_name(&answer, 6, "x").unwrap(),
            ("`6`".to_owned(), None)
        );
        let paged = json!({"count": 1, "next": null, "results": [{"id": 5, "label": "a`b"}]});
        assert_eq!(group_name(&paged, 5, "x").unwrap().0, r"`a\u{60}b` (`5`)");
        let error = group_name(&answer, 7, "set it").unwrap_err();
        assert_eq!(error.code(), hippius::Code::NotFound);
        assert_eq!(
            error.message(),
            "cannot set it: the network has no group `7`"
        );
        for unreadable in [json!({}), json!("x")] {
            let error = group_name(&unreadable, 5, "x").unwrap_err();
            assert_eq!(error.code(), hippius::Code::Internal, "{unreadable}");
        }
        let split = json!({"next": "https://x/api/network/groups/?page=2", "results": []});
        assert_eq!(
            group_name(&split, 5, "x").unwrap_err().code(),
            hippius::Code::Internal
        );
    }
}
