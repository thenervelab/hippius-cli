//! The Kubernetes commands that take the cluster key file: `clusters kubeconfig`,
//! `node-pools scale`, and `nodes replace`.
//!
//! The library reads and checks the key file ([`hippius::ClusterKey`]), opens the kubeconfig,
//! and signs each `add_node`; this module decides the requests, their order, and what is
//! printed. The private keys never leave the library: no document, message, or `--debug` line
//! here holds one.
//!
//! The key file is read first, before the config file and any request, as a local file a
//! command reads is (tenet 5); a bad one is `usage`, and nothing is sent. A dry run reads it
//! too, and signs nothing: a signature depends on the pool's `signing_params` and the size of
//! the pool, which only the reads give, so the dry run shows a placeholder in its place.
//!
//! The kubeconfig is a secret, so it goes only to the new mode-0600 file `--output-file` names,
//! never to stdout, as the invoice PDF does. stdout names the file. It needs no `--reveal`,
//! since no output holds the secret, and it is not shown once and lost: the key file opens it
//! again.
//!
//! A scale and a replacement read the cluster first, to check the key file against it before
//! the question, then its pools and its operations, so the question names the pool and the
//! nodes it gains or loses. The pool's `signing_params` are checked against the pool before
//! the question ([`Target::signing_params`]): the signature is what the masters act on and
//! bill, so an answer whose `signing_params` would sign what the question does not name is
//! refused with nothing signed. The signatures are made after the question, so a declined
//! question uses no key. After the question, the pools and the operations are read again: a
//! scale of a pool that is no longer the size the question named (another `hipc`, or the
//! console, scaled the pool), a scale up whose pool signs other params now, and a
//! replacement of a node that another operation now removes or replaces, are refused with
//! nothing signed or sent. A scale up is refused since each signature the masters receive is
//! a node they add and bill; a scale down, which signs nothing, since its count is applied to
//! the pool as it is then: a pool that grew would lose more nodes than the question named,
//! and one that shrank below the count would be grown by a POST that signs nothing. That
//! narrows the window, it does not close it: a change between the second read and the POST
//! still passes here, and only a check by the server against its own count, if it makes one,
//! closes it. The signatures are dated by the server's clock, the `Date` of that second read,
//! when it is within five minutes of the local clock.

use std::cmp::Ordering;
use std::collections::HashSet;

use hippius::routes::kubernetes::{
    self, NodePoolScaleBody, NodeReplaceBody, Operation, OperationKind, Pool,
};
use hippius::{
    Client, ClusterKey, Error, Failure, MatchedKey, Request, Response, Result, phrase, view_of,
};
use serde_json::{Map, Value, json};

use crate::cli::{ClusterKubeconfig, NodePoolScale, NodeReplace};
use crate::commands::Printed;
use crate::commands::compute::kubernetes::{CLUSTERS, cluster_get, list_request, read_list};
use crate::commands::files::write_output_file;
use crate::context::{Context, Gate, Step, Write};

/// The flag that names the cluster key file.
const KEY_FILE: &str = "--cluster-key-file";

/// `GET .../<id>/` and `GET .../<id>/secrets/`, then the kubeconfig, opened with the key file,
/// written to a new file; prints `{id, name, output_file, bytes}`. Nothing is written unless the
/// kubeconfig opened, so a failed run leaves no file behind.
pub(super) fn kubeconfig(ctx: &Context<'_>, args: &ClusterKubeconfig) -> Result<Printed> {
    let key = ClusterKey::load(&args.key.path, KEY_FILE)?;
    let id = args.cluster_id;
    let client = ctx.load_authed_client()?;
    let read = cluster_get(id);
    let secrets = Request::to(&kubernetes::CLUSTER_SECRETS, [&id])
        .about(format!(
            "get the sealed secrets of the Kubernetes cluster `{id}`"
        ))
        .listed_by(CLUSTERS);
    let cluster = match ctx.call(&client, &read, Gate::Open)? {
        Step::DryRun(_) => {
            return ctx
                .dry_run_of(&client, &[&read, &secrets])
                .map(Printed::Document);
        }
        Step::Sent(response) => response.document()?,
    };
    let matched = key.matched(id, &cluster)?;
    let sealed = ctx.send(&client, &secrets)?.document()?;
    let kubeconfig = matched.open_kubeconfig(&sealed)?;
    let path = &args.output_file;
    write_output_file(path, kubeconfig.as_bytes())?;
    Ok(Printed::Document(json!({
        "id": id,
        "name": cluster.get("name"),
        "output_file": path.to_string_lossy(),
        "bytes": kubeconfig.len(),
    })))
}

/// `POST .../node-pools/<pool_id>/scale/` with `{count}`, and one signed `add_node` per node
/// added in `operations`, after the reads that size the pool (module docs). The pool's size is
/// projected as the console projects it ([`pool_size`]), and the request carries exactly the
/// difference in signatures: one per node added, none for a scale down.
pub(super) fn scale(ctx: &Context<'_>, args: &NodePoolScale) -> Result<Printed> {
    let key = args
        .cluster_key_file
        .as_deref()
        .map(|path| ClusterKey::load(path, KEY_FILE))
        .transpose()?;
    let (id, pool_id, count) = (args.cluster_id, args.pool_id, args.count);
    let client = ctx.load_authed_client()?;
    let post = |action: String| {
        Request::to(&kubernetes::NODE_POOL_SCALE, [&id, &pool_id])
            .about(action)
            .listed_by(format!("hipc compute kubernetes node-pools list {id}"))
    };
    let reads = [
        cluster_get(id),
        list_request(id, &kubernetes::NODE_POOLS, "node pools"),
        list_request(id, &kubernetes::OPERATIONS, "operations"),
    ];
    let draft = post(format!(
        "scale the pool `{pool_id}` of the Kubernetes cluster `{id}` to {count} nodes"
    ))
    .json(&NodePoolScaleBody {
        count,
        operations: Some(
            "<one add_node signed with the cluster key file per node added; left out when the \
             pool shrinks>",
        ),
    });
    let checked = match ctx.check_write(&client, &draft)? {
        Write::DryRun(_) => {
            // The reads before the question, the two after it, and the write (tenet 5).
            let [cluster, pools, operations] = &reads;
            let sends = [cluster, pools, operations, pools, operations, &draft];
            return ctx.dry_run_of(&client, &sends).map(Printed::Document);
        }
        Write::Checked(checked) => checked,
    };
    let cluster = ctx.send(&client, &reads[0])?.document()?;
    let matched = key
        .as_ref()
        .map(|key| key.matched(id, &cluster))
        .transpose()?;
    let pools = read_list(ctx, &client, &reads[1])?;
    let operations = operations_of(&read_list(ctx, &client, &reads[2])?);
    let Some(pool) = pools
        .iter()
        .find(|pool| view_of::<Pool>(pool).id == Some(pool_id))
    else {
        return Err(Error::new(
            Failure::NotFound,
            format!(
                "the Kubernetes cluster {} has no node pool `{pool_id}`",
                phrase::named(id, &cluster)
            ),
        )
        .with_hint(format!(
            "run `hipc compute kubernetes node-pools list {id}` to see what exists"
        )));
    };
    let target = Target {
        id,
        cluster: &cluster,
        pool,
    };
    let current = pool_size(&view_of(pool), &operations);
    let (action, adds) = target.scale(current, count, matched)?;
    let confirmed = ctx.confirm_write(checked, &action, None)?;
    // Read again after the question, both ways: the question named `count` as a change from
    // `current`, and the server applies `count` to the pool as it is then. A scale down signs
    // nothing, but a pool that grew since the question would lose more nodes than it named,
    // and one that shrank below `count` would be grown by a POST that signs nothing. Only a
    // scale up cares what a node of the pool signs. A queued add that finished during the
    // question passes: the pool is the same size, so a node is drained where the question
    // said an add is cancelled, and the pool still ends at `count`. The window from here to
    // the POST stays open (module docs). Signed after the question, so a declined one uses no
    // key, and each signature's window starts as late as it can.
    let (pools, operations, answered) = read_again(ctx, &client, &reads[1], &reads[2])?;
    let live = pools
        .iter()
        .find(|live| view_of::<Pool>(live).id == Some(pool_id));
    // The question named `count` as a change from `current`, up or down.
    let same_size = live.is_some_and(|live| pool_size(&view_of(live), &operations) == current);
    // Only the nodes a scale up adds are signed, so only a scale up cares what they sign.
    let signs_the_same = adds.is_none() || live.is_some_and(|live| same_signing(live, pool));
    if !(same_size && signs_the_same) {
        return Err(changed(ctx, &target));
    }
    let operations = match adds {
        None => None,
        Some(Adds { key, params, added }) => Some(
            (0..added)
                .map(|_| key.sign_add_node(params, &answered))
                .collect::<Result<Vec<_>>>()?,
        ),
    };
    let request = post(action).json(&NodePoolScaleBody { count, operations });
    ctx.send_confirmed(&client, &request, &confirmed)?
        .document()
        .map(Printed::Document)
}

/// The nodes a scale up adds, and what signs each of them.
#[derive(Debug)]
struct Adds<'a, 'k> {
    key: MatchedKey<'k>,
    /// The pool's `signing_params`.
    params: &'a Map<String, Value>,
    added: u64,
}

/// `POST .../nodes/<node_name>/replace/` with `{operation}`, one `add_node` signed over the
/// node's pool's `signing_params` with `replaces` added, after the reads that find the pool
/// (module docs).
pub(super) fn replace(ctx: &Context<'_>, args: &NodeReplace) -> Result<Printed> {
    let key = ClusterKey::load(&args.key.path, KEY_FILE)?;
    let (id, node) = (args.cluster_id, args.node_name.as_str());
    let client = ctx.load_authed_client()?;
    let post = |action: String| {
        Request::to(&kubernetes::NODE_REPLACE, [&id, &node])
            .about(action)
            .listed_by(format!("hipc compute kubernetes node-pools list {id}"))
    };
    let reads = [
        cluster_get(id),
        list_request(id, &kubernetes::NODE_POOLS, "node pools"),
        list_request(id, &kubernetes::OPERATIONS, "operations"),
    ];
    let draft = post(format!(
        "replace the node `{node}` of the Kubernetes cluster `{id}`"
    ))
    .json(&NodeReplaceBody {
        operation: "<an add_node signed with the cluster key file over the pool's \
                    signing_params, with `replaces`>",
    });
    let checked = match ctx.check_write(&client, &draft)? {
        Write::DryRun(_) => {
            // The reads before the question, the two after it, and the write (tenet 5).
            let [cluster, pools, operations] = &reads;
            let sends = [cluster, pools, operations, pools, operations, &draft];
            return ctx.dry_run_of(&client, &sends).map(Printed::Document);
        }
        Write::Checked(checked) => checked,
    };
    let cluster = ctx.send(&client, &reads[0])?.document()?;
    let matched = key.matched(id, &cluster)?;
    let pools = read_list(ctx, &client, &reads[1])?;
    let operations = operations_of(&read_list(ctx, &client, &reads[2])?);
    let Some(pool) = pools.iter().find(|pool| holds_node(&view_of(pool), node)) else {
        return Err(Error::new(
            Failure::NotFound,
            format!(
                "the Kubernetes cluster {} has no node `{node}` in its node pools",
                phrase::named(id, &cluster)
            ),
        )
        .with_hint(format!(
            "run `hipc compute kubernetes node-pools list {id}` to see what exists"
        )));
    };
    let target = Target {
        id,
        cluster: &cluster,
        pool,
    };
    refuse_busy(&target, node, &operations)?;
    let signing = target.signing_params()?;
    let action = format!(
        "replace the node `{node}` of {}: a new {} billed to the balance joins \
         first, then `{node}` is drained and its VM decommissioned, and both are billed while \
         they overlap",
        target.pool_named(),
        signed_vm(signing, "VM")
    );
    let confirmed = ctx.confirm_write(checked, &action, None)?;
    let (pools, operations, answered) = read_again(ctx, &client, &reads[1], &reads[2])?;
    let unchanged = pools
        .iter()
        .find(|now| holds_node(&view_of(now), node))
        .is_some_and(|now| same_signing(now, pool));
    if !unchanged {
        return Err(changed(ctx, &target));
    }
    refuse_busy(&target, node, &operations)?;
    // `signing_params` checked above hold no `replaces`, so this adds it rather than
    // overwriting one the API chose.
    let mut params = signing.clone();
    params.insert("replaces".to_owned(), node.into());
    let operation = matched.sign_add_node(&params, &answered)?;
    let request = post(action).json(&NodeReplaceBody { operation });
    ctx.send_confirmed(&client, &request, &confirmed)?
        .document()
        .map(Printed::Document)
}

/// The keys of a pool's `signing_params`, the only ones a node is signed over.
const SIGNED: [&str; 6] = ["pool", "flavor", "k8s_version", "image", "labels", "taints"];

/// The pool a scale or a replacement acts on, in its cluster, as the question names it.
#[derive(Debug)]
struct Target<'a> {
    id: u64,
    cluster: &'a Value,
    pool: &'a Value,
}

impl<'a> Target<'a> {
    /// The question of a scale of the pool from `current` nodes to `count`, and the nodes it
    /// adds, which `key` signs: none when it shrinks.
    ///
    /// # Errors
    ///
    /// `usage` when the pool already has `count` nodes, when it grows and no key file was
    /// given, or when it grows and it is not a worker pool; `internal` when it grows and the
    /// pool's `signing_params` are not the pool's own ([`Target::signing_params`]).
    fn scale<'k>(
        &self,
        current: u64,
        count: u64,
        key: Option<MatchedKey<'k>>,
    ) -> Result<(String, Option<Adds<'a, 'k>>)> {
        let nodes = phrase::count(count, "node");
        let pool = self.pool_named();
        match count.cmp(&current) {
            Ordering::Equal => Err(Error::usage(
                format!("{pool} already has {nodes}, counting adds still queued"),
                "pass `--count` another number of nodes",
            )),
            Ordering::Less => Ok((
                format!(
                    "scale {pool} from {current} to {nodes}: adds still queued are \
                     cancelled first, then the newest nodes are drained and their VMs \
                     decommissioned; a node with local data blocks until its loss is accepted \
                     in the console"
                ),
                None,
            )),
            Ordering::Greater => {
                let added = count - current;
                let Some(key) = key else {
                    return Err(Error::usage(
                        format!(
                            "adding {} to {pool} signs each one with the cluster key, \
                             and `{KEY_FILE}` was not passed",
                            phrase::count(added, "node")
                        ),
                        format!(
                            "pass `{KEY_FILE}` the `hippius-cluster-<name>.key` file the \
                             console saved when it created the cluster"
                        ),
                    ));
                };
                let params = self.signing_params()?;
                let action = format!(
                    "scale {pool} from {current} to {nodes}: {added} new {}, each \
                     billed to the balance until it is removed, signed with the cluster key file",
                    signed_vm(params, &phrase::plural(added, "VM"))
                );
                Ok((action, Some(Adds { key, params, added })))
            }
        }
    }

    /// `` the pool `workers` (`3`) of the Kubernetes cluster `prod` (`7`) ``, with what the
    /// answer gives of the pool's name and id: no id is made up for a pool that has none.
    fn pool_named(&self) -> String {
        let pool: Pool = view_of(self.pool);
        let label = match (pool.id, pool.name) {
            (Some(pool_id), _) => format!(" {}", phrase::named(pool_id, self.pool)),
            (None, Some(name)) => format!(" {}", phrase::quote(name)),
            (None, None) => String::new(),
        };
        format!(
            "the pool{label} of the Kubernetes cluster {}",
            phrase::named(self.id, self.cluster)
        )
    }

    /// The pool's `signing_params`, what a node of it signs, checked against the pool they
    /// came with: only a worker pool has them, and the question names the pool and its
    /// flavor, so the signature must add a node of that flavor to that pool, as the pool
    /// labels and taints it, and nothing else. Each key the pool has is checked whether the
    /// map has it or not: a map that leaves a key out signs a node without it, as much as
    /// one that changes it. The signature is what the masters act on and bill, and this runs
    /// before the question, so a refusal signs nothing and sends no node operation.
    ///
    /// The map is returned as the API gave it, since the backend verifies the canonical JSON
    /// of what the console would send, which is that map; once it passes, the map and the
    /// pool agree on every key the pool has.
    ///
    /// # Errors
    ///
    /// `usage` when the pool has no `signing_params`. `internal` when they hold a key other
    /// than [`SIGNED`] (`replaces` among them: a scale adds new nodes, and a replacement names
    /// the replaced node itself); when their `pool` is not the pool's `name`, as a string on
    /// both sides; when the pool has a `flavor` and they name another or none, or they name
    /// one that is not text; or when their `labels` or `taints` are not the pool's own, with
    /// a side that is missing or `null` read as none.
    fn signing_params(&self) -> Result<&'a Map<String, Value>> {
        let signing_value = self.pool.get("signing_params");
        let Some(signing) = signing_value.and_then(Value::as_object) else {
            return Err(Error::usage(
                format!(
                    "{} has no `signing_params`, so hipc cannot sign a node for \
                     it: only a worker pool gains nodes",
                    self.pool_named()
                ),
                format!(
                    "pick a worker pool from `hipc compute kubernetes node-pools list {}`",
                    self.id
                ),
            ));
        };
        let refused = |problem: String, why: &str| {
            Error::internal(format!(
                "{} {problem}, so no node was signed: {why}",
                self.pool_named()
            ))
        };
        if let Some(key) = signing.keys().find(|key| !SIGNED.contains(&key.as_str())) {
            return Err(refused(
                format!(
                    "has {} in its `signing_params`, which hold only {}",
                    phrase::quote(key),
                    phrase::join(&SIGNED.map(phrase::quote), "and")
                ),
                "the signature would hold what the question does not name",
            ));
        }
        // The pool's name and the signed `pool` must both be text and equal, since the
        // question names the pool by its name and the masters add the node to the pool the
        // signature names. A pool with a flavor must sign that flavor; one without may sign
        // none, or a name, which the question then shows. Anything but text on either side
        // is refused rather than shown or read as none: `node-pools list` shows the pool's
        // value as it is, and the question could not.
        for (what, key, noun, required) in [
            ("name", "pool", "pool", true),
            ("flavor", "flavor", "flavor", false),
        ] {
            let own = match self.pool.get(what) {
                None | Some(Value::Null) => None,
                Some(Value::String(own)) => Some(own.as_str()),
                Some(other) => {
                    return Err(refused(
                        format!("has a {what} that is not text, {}", literal(other)),
                        &format!(
                            "the question would not name the {noun} as `hipc compute \
                             kubernetes node-pools list {}` shows it",
                            self.id
                        ),
                    ));
                }
            };
            let signed = signing.get(key);
            let agreed = match (own, signed) {
                (Some(own), Some(Value::String(text))) => own == text,
                (None, Some(Value::String(_)) | None) => !required,
                _ => false,
            };
            if !agreed {
                return Err(refused(
                    mismatch(what, own, signed),
                    &format!("the question would name one {noun} and the signature another"),
                ));
            }
        }
        for (field, none) in [("labels", json!({})), ("taints", json!([]))] {
            let given = |object: &'a Value| object.get(field).filter(|value| !value.is_null());
            let own = given(self.pool).unwrap_or(&none);
            let signed = signing_value.and_then(given).unwrap_or(&none);
            if signed != own {
                return Err(refused(
                    format!(
                        "has the {field} {}, but its `signing_params` hold {}",
                        literal(own),
                        literal(signed)
                    ),
                    &format!("the signature would give the node {field} the pool does not have"),
                ));
            }
        }
        Ok(signing)
    }
}

/// What each node signed over `signing`, checked `signing_params`, is, as the question names
/// it: `` `medium` VMs with the image `ubuntu`, Kubernetes `v1.34.3+rke2r1`, the labels
/// `{"tier":"web"}`, and the taints `["spot:NoSchedule"]` ``, with `vms` the word for the VM
/// or VMs. Each part is left out when `signing` does not name it, and the labels and the
/// taints when they are empty, which a pool without any signs: the question names what is
/// signed, since the signature is what the masters act on and bill, and makes up nothing.
/// The labels and the taints are named even though they are checked against the pool's,
/// since the pool comes from the same answer, and only the person can tell what it should
/// hold.
fn signed_vm(signing: &Map<String, Value>, vms: &str) -> String {
    // Text when it is there: the check refused anything else.
    let flavor = signing
        .get("flavor")
        .and_then(Value::as_str)
        .map_or_else(String::new, |flavor| format!("{} ", phrase::quote(flavor)));
    let empty = |value: &&Value| match value {
        Value::Object(fields) => fields.is_empty(),
        Value::Array(items) => items.is_empty(),
        _ => false,
    };
    let with: Vec<String> = [
        ("image", "the image "),
        ("k8s_version", "Kubernetes "),
        ("labels", "the labels "),
        ("taints", "the taints "),
    ]
    .into_iter()
    .filter_map(|(field, what)| {
        signing
            .get(field)
            .filter(|value| !empty(value))
            .map(|value| format!("{what}{}", literal(value)))
    })
    .collect();
    if with.is_empty() {
        format!("{flavor}{vms}")
    } else {
        format!("{flavor}{vms} with {}", phrase::join(&with, "and"))
    }
}

/// `` has the flavor `medium`, but its `signing_params` name `gpu` ``: the problem of a key
/// the pool and its `signing_params` disagree on, with `own` the pool's text, or none, and
/// `signed` the value the map holds, or none.
fn mismatch(what: &str, own: Option<&str>, signed: Option<&Value>) -> String {
    let has = own.map_or_else(
        || format!("has no {what}"),
        |own| format!("has the {what} {}", phrase::quote(own)),
    );
    let names = signed.map_or_else(|| format!("no {what}"), literal);
    format!("{has}, but its `signing_params` name {names}")
}

/// `value` as a literal in a message: its text when it is a string, otherwise its JSON, so
/// a message shows what is signed whatever the API answered.
fn literal(value: &Value) -> String {
    match value {
        Value::String(text) => phrase::quote(text),
        other => phrase::quote(other),
    }
}

/// The pools and the operations read again after the question, and the answer to the last
/// read, whose `Date` the signatures are dated by.
fn read_again(
    ctx: &Context<'_>,
    client: &Client,
    pools: &Request,
    operations: &Request,
) -> Result<(Vec<Value>, Vec<Operation>, Response)> {
    let pools = read_list(ctx, client, pools)?;
    let answered = ctx.send(client, operations)?;
    let operations = hippius::page(answered.json()?)?.into_complete(operations.target())?;
    Ok((pools, operations_of(&operations), answered))
}

/// The operations of a list read, each as its view.
fn operations_of(list: &[Value]) -> Vec<Operation> {
    list.iter().map(view_of).collect()
}

/// Whether `now`, a pool read again, signs a node as `before` did: the same `signing_params`,
/// which hold the flavor the question named, or none on both.
fn same_signing(now: &Value, before: &Value) -> bool {
    now.get("signing_params") == before.get("signing_params")
}

/// The refusal of a pool that changed between the read the question was built from and the
/// read after it. Nothing is signed or sent, so running the command again asks about the pool
/// as it is then.
fn changed(ctx: &Context<'_>, target: &Target<'_>) -> Error {
    Error::new(
        Failure::Conflict,
        format!(
            "{} changed after hipc read it for the question: its nodes, its operations in \
             flight, or what a node of it signs are not what the question was about, so \
             nothing was signed or sent",
            target.pool_named()
        ),
    )
    .with_hint(format!(
        "run `{}` again: it reads the pool as it is now",
        phrase::command(ctx.args())
    ))
}

/// Refuses a replacement of `node` while an operation in flight already acts on it: a removal
/// of it, or an add that replaces it. A second replacement would
/// sign a second new node, added and billed.
fn refuse_busy(target: &Target<'_>, node: &str, operations: &[Operation]) -> Result<()> {
    let busy = operations
        .iter()
        .filter(|op| op.is_active())
        .find_map(|op| match op.kind {
            Some(OperationKind::RemoveNode)
                if op.node.value().map(String::as_str) == Some(node) =>
            {
                Some("removed")
            }
            Some(OperationKind::AddNode)
                if op
                    .params
                    .as_ref()
                    .and_then(|params| params.replaces.as_deref())
                    == Some(node) =>
            {
                Some("replaced")
            }
            _ => None,
        });
    match busy {
        None => Ok(()),
        Some(doing) => Err(Error::new(
            Failure::Conflict,
            format!(
                "the node `{node}` of {} is already being {doing}, so no node was signed",
                target.pool_named()
            ),
        )
        .with_hint(format!(
            "run `hipc compute kubernetes operations list {}` to follow that operation",
            target.id
        ))),
    }
}

/// Whether `pool` lists a node named `node` that is not removed: a node whose removal is done
/// stays in the list with the state `removed`, and has no VM left to replace.
fn holds_node(pool: &Pool, node: &str) -> bool {
    pool.nodes.as_ref().is_some_and(|nodes| {
        nodes
            .iter()
            .any(|n| n.name.as_deref() == Some(node) && !n.is_removed())
    })
}

/// The pool's size the way the console projects it for a scale, so the POST carries as many
/// signatures as the masters will add and bill (the server is not known to count it the same
/// way): its
/// nodes that are not `removed`, less the `node` of each in-flight `remove_node` and the
/// `node` of each in-flight `add_node` that `replaces` another (the replaced node keeps
/// counting until it is removed, so a replacement is size-neutral), plus the in-flight
/// `add_node`s of this pool that have no node yet and replace nothing. Scaling to N sends N
/// minus this many signed adds.
fn pool_size(pool: &Pool, operations: &[Operation]) -> u64 {
    let active: Vec<&Operation> = operations.iter().filter(|op| op.is_active()).collect();
    let leaving: HashSet<&str> = active
        .iter()
        .filter(|op| {
            op.kind == Some(OperationKind::RemoveNode)
                || op.kind == Some(OperationKind::AddNode) && op.replaces().is_some()
        })
        .filter_map(|op| op.node.value().map(String::as_str))
        .collect();
    let live = pool.nodes.iter().flatten().filter(|node| {
        !node.is_removed()
            && !node
                .name
                .as_deref()
                .is_some_and(|name| leaving.contains(name))
    });
    let pending = active.iter().filter(|op| {
        op.kind == Some(OperationKind::AddNode)
            && !op.node.is_set()
            && op.replaces().is_none()
            && pool.name.as_deref().is_some_and(|name| {
                op.params.as_ref().and_then(|params| params.pool.as_deref()) == Some(name)
            })
    });
    (live.count() + pending.count()) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`pool_size`] of a pool and operations as the API answers them.
    fn size(pool: &Value, operations: &[Value]) -> u64 {
        pool_size(&view_of(pool), &operations_of(operations))
    }

    #[test]
    fn the_pool_size_counts_as_the_console_projects_it() {
        let pool = json!({
            "name": "w",
            "nodes": [
                { "name": "w-1", "state": "ready" },
                { "name": "w-2", "state": "ready" },
                { "name": "w-3", "state": "removed" },
                { "name": "w-4", "state": "ready" },
                { "name": "w-5", "state": "joining" },
            ]
        });
        let operations = [
            // Being removed, and the new half of a replacement: not counted. The node it
            // replaces, `w-9`, is not listed, so leaving out `replaces` instead of `node`
            // would count `w-4` and give 6.
            json!({ "kind": "remove_node", "state": "running", "node": "w-1" }),
            json!({ "kind": "add_node", "state": "queued", "node": "w-4",
                    "params": { "pool": "w", "replaces": "w-9" } }),
            // Two adds with no node yet: counted.
            json!({ "kind": "add_node", "state": "queued", "node": null,
                    "params": { "pool": "w" } }),
            json!({ "kind": "add_node", "state": "blocked", "params": { "pool": "w" } }),
            // Settled, another pool's, or a replacement: not counted.
            json!({ "kind": "add_node", "state": "done", "node": null, "params": { "pool": "w" } }),
            json!({ "kind": "remove_node", "state": "failed", "node": "w-2" }),
            json!({ "kind": "add_node", "state": "queued", "node": null,
                    "params": { "pool": "x" } }),
            json!({ "kind": "add_node", "state": "queued", "node": null,
                    "params": { "pool": "w", "replaces": "w-2" } }),
            json!({ "kind": "add_node", "state": "queued", "node": null,
                    "params": { "pool": "w", "replaces": "" } }),
            json!({}),
        ];
        // w-2 and w-5 live, plus two adds, plus the add whose `replaces` is empty.
        assert_eq!(size(&pool, &operations), 5);
        assert_eq!(size(&json!({ "name": "w" }), &[]), 0);
        // A pool the answer gives without a name has no adds: an add names its pool.
        let unnamed = json!({ "nodes": [{ "name": "w-1", "state": "ready" }] });
        let add = json!({ "kind": "add_node", "state": "queued", "node": null, "params": {} });
        assert_eq!(size(&unnamed, &[add]), 1);
    }

    /// Two known-answer cases for the projected pool size: a pool with adds in flight, and
    /// one with a removal and replacements.
    #[test]
    fn the_pool_size_matches_the_known_answers() {
        let node = |name: &str| json!({ "name": name, "state": "ready" });
        let add = |id: u64, state: &str, node: Value, params: Value| {
            json!({ "id": id, "kind": "add_node", "state": state, "node": node,
                    "params": params })
        };
        let workers = json!({ "pool": "workers" });
        let pool = json!({ "name": "workers", "nodes": [node("workers-aaaaaaaa")] });
        let operations = [
            add(1, "queued", Value::Null, workers.clone()),
            add(2, "running", Value::Null, workers.clone()),
            // Another pool's add, a finished add, a cancelled one: not this pool's size.
            add(3, "queued", Value::Null, json!({ "pool": "gpu" })),
            add(4, "done", json!("workers-aaaaaaaa"), workers.clone()),
            add(5, "cancelled", Value::Null, workers),
        ];
        assert_eq!(size(&pool, &operations), 3);

        let nodes = ["aaaaaaaa", "bbbbbbbb", "cccccccc", "dddddddd"]
            .map(|suffix| node(&format!("workers-{suffix}")));
        let pool = json!({ "name": "workers", "nodes": nodes });
        let operations = [
            json!({ "id": 1, "kind": "remove_node", "state": "blocked",
                    "node": "workers-aaaaaaaa" }),
            // The new half of a replacement is left out; the node it replaces still counts.
            add(
                2,
                "running",
                json!("workers-cccccccc"),
                json!({ "pool": "workers", "replaces": "workers-bbbbbbbb" }),
            ),
            // A replacement with no node yet is net zero: its target still counts.
            add(
                3,
                "queued",
                Value::Null,
                json!({ "pool": "workers", "replaces": "workers-dddddddd" }),
            ),
        ];
        assert_eq!(size(&pool, &operations), 2);
    }

    /// Only an operation in flight counts, each state on its own.
    #[test]
    fn only_an_operation_in_flight_counts() {
        let pool = json!({ "name": "w", "nodes": [] });
        for (state, counted) in [
            ("queued", 1),
            ("running", 1),
            ("blocked", 1),
            ("done", 0),
            ("failed", 0),
            ("rejected", 0),
            ("cancelled", 0),
        ] {
            let add = json!({ "kind": "add_node", "state": state, "node": null,
                              "params": { "pool": "w" } });
            assert_eq!(size(&pool, &[add]), counted, "{state}");
        }
    }
}
