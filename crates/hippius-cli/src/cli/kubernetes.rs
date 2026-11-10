//! The grammar of `hipc compute kubernetes`.
//!
//! Opening a kubeconfig, scaling a pool up, and replacing a node take the key file the
//! console saved when it created the cluster, through `--cluster-key-file`; creating a
//! cluster, which generates that key, stays in the console.

use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::cli::values::{Charset, Dash, word};
use crate::cli::{examples, flavor, new_file, number};

const CLUSTER_ID: &str =
    "a cluster id is a whole number, from `hipc compute kubernetes clusters list`";
const OPERATION_ID: &str =
    "an operation id is a whole number, from `hipc compute kubernetes operations list`";
const POOL_ID: &str = "a pool id is a whole number, from `hipc compute kubernetes node-pools list`";

#[derive(Debug, Subcommand)]
pub(crate) enum Kubernetes {
    /// The account's clusters: read them, follow their journal, and delete them
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute kubernetes clusters list",
        "hipc compute kubernetes clusters get 7",
    ]))]
    Clusters(Clusters),
    /// A cluster's etcd snapshots in the account's S3
    // One command until the backup writes are added (tenet 2's short-lived exception).
    #[command(subcommand)]
    #[command(after_help = examples(&["hipc compute kubernetes backups list 7 --reveal"]))]
    Backups(KubernetesBackups),
    /// A cluster's node pools: list them, create an empty one for workers, and scale one
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute kubernetes node-pools list 7",
        "hipc compute kubernetes node-pools create 7 --name workers --flavor medium",
        "hipc compute kubernetes node-pools scale 7 3 --count 2 --cluster-key-file \
         hippius-cluster-prod.key --yes",
    ]))]
    NodePools(NodePools),
    /// A cluster's node operations: list them, and cancel or resume one
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute kubernetes operations list 7",
        "hipc compute kubernetes operations cancel 7 12 --yes",
    ]))]
    Operations(Operations),
    /// Remove a worker node from a cluster, or replace it with a new one
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc compute kubernetes nodes delete 7 work-3a5bf1ed --yes",
        "hipc compute kubernetes nodes replace 7 work-3a5bf1ed --cluster-key-file \
         hippius-cluster-prod.key --yes",
    ]))]
    Nodes(Nodes),
}

#[derive(Debug, Subcommand)]
pub(crate) enum KubernetesBackups {
    /// A cluster's etcd snapshots in the account's S3, with their signed manifests, sealed
    /// keys, and download links; needs --reveal
    ///
    /// Each download link fetches its snapshot without a token for a few minutes, so the answer
    /// is printed only with --reveal, on stdout. Opening a snapshot takes the cluster key file,
    /// in the console.
    #[command(after_help = examples(&["hipc compute kubernetes backups list 7 --reveal"]))]
    List {
        /// The cluster id from hipc compute kubernetes clusters list
        #[arg(value_parser = number(CLUSTER_ID))]
        cluster_id: u64,
        /// Print the download links and sealed keys; required: without it nothing is sent
        #[arg(long)]
        reveal: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum Clusters {
    /// Every cluster in the account, each with its status, endpoints, and master nodes
    #[command(after_help = examples(&["hipc compute kubernetes clusters list"]))]
    List,
    /// One cluster
    #[command(after_help = examples(&["hipc compute kubernetes clusters get 7"]))]
    Get {
        /// The cluster id from hipc compute kubernetes clusters list
        #[arg(value_parser = number(CLUSTER_ID))]
        cluster_id: u64,
    },
    /// Delete a cluster: every VM in it stops and is decommissioned, with the workloads and the
    /// data on it; needs --yes
    ///
    /// This cannot be undone. Before the question, hipc reads the cluster, whose name the
    /// question gives; --dry-run reads nothing and lists the read in requests. The command
    /// returns once the teardown has started. The cluster is closed only after every VM is
    /// decommissioned; hipc compute kubernetes clusters get shows its status.
    #[command(after_help = examples(&["hipc compute kubernetes clusters delete 7 --yes"]))]
    Delete {
        /// The cluster id from hipc compute kubernetes clusters list
        #[arg(value_parser = number(CLUSTER_ID))]
        cluster_id: u64,
    },
    /// The cluster's journal: what each node reported, and when
    #[command(after_help = examples(&["hipc compute kubernetes clusters events 7"]))]
    Events {
        /// The cluster id from hipc compute kubernetes clusters list
        #[arg(value_parser = number(CLUSTER_ID))]
        cluster_id: u64,
    },
    /// The cluster launches the platform refused in the last 7 days
    ///
    /// A refused launch leaves no cluster, only an entry here.
    #[command(after_help = examples(&["hipc compute kubernetes clusters refusals"]))]
    Refusals,
    /// Write a cluster's kubeconfig to a new file, opened with the cluster key file; needs
    /// --cluster-key-file
    ///
    /// hipc reads the cluster and its sealed secrets, checks that the key file is the one the
    /// cluster was created with, verifies that the cluster signed the kubeconfig, and only then
    /// opens it, all on this machine: neither the key nor the kubeconfig is sent anywhere. The
    /// kubeconfig goes only to the file --output-file names, created with mode 0600; an
    /// existing file is never overwritten. stdout is a JSON document that names the file. Only
    /// the account owner can read a cluster's secrets.
    #[command(after_help = examples(&[
        "hipc compute kubernetes clusters kubeconfig 7 --cluster-key-file \
         hippius-cluster-prod.key --output-file prod.kubeconfig",
    ]))]
    Kubeconfig(ClusterKubeconfig),
}

#[derive(Debug, Args)]
pub(crate) struct ClusterKubeconfig {
    /// The cluster id from hipc compute kubernetes clusters list
    #[arg(value_parser = number(CLUSTER_ID))]
    pub(crate) cluster_id: u64,
    /// The key file the console saved when it created the cluster, hippius-cluster-<name>.key;
    /// it must be a regular file only its owner can read (chmod 600)
    ///
    /// It is read only as a regular file of the current user, never through a symlink, with
    /// no bits for the group or others; there is no stdin form.
    #[arg(long, value_name = "PATH")]
    pub(crate) cluster_key_file: PathBuf,
    /// The file to create for the kubeconfig; it must not exist yet
    #[arg(long, value_name = "PATH", value_parser = new_kubeconfig_file)]
    pub(crate) output_file: PathBuf,
}

/// The file a kubeconfig goes to ([`new_file`]).
fn new_kubeconfig_file(text: &str) -> Result<PathBuf, String> {
    new_file(text, "the kubeconfig")
}

#[derive(Debug, Subcommand)]
pub(crate) enum NodePools {
    /// The cluster's node pools, the masters' control-plane pool included, each with its nodes
    #[command(after_help = examples(&["hipc compute kubernetes node-pools list 7"]))]
    List {
        /// The cluster id from hipc compute kubernetes clusters list
        #[arg(value_parser = number(CLUSTER_ID))]
        cluster_id: u64,
    },
    /// Create an empty worker node pool
    ///
    /// The pool has no node, and costs nothing, until hipc compute kubernetes node-pools scale
    /// adds nodes to it.
    #[command(after_help = examples(&[
        "hipc compute kubernetes node-pools create 7 --name workers --flavor medium",
        "hipc compute kubernetes node-pools create 7 --name gpu --flavor large --label tier=gpu \
         --taint gpu=true:NoSchedule",
    ]))]
    Create(NodePoolCreate),
    /// Scale a worker node pool to --count nodes; needs --yes, and --cluster-key-file to add
    /// nodes, which spends money
    ///
    /// Before the question, hipc reads the cluster, its pools, and its operations, so the
    /// question names the pool and how many nodes it gains or loses; the count includes adds
    /// still queued and leaves out nodes being removed. --dry-run reads nothing and lists the
    /// reads in requests. Each node added is a new VM of the pool's flavor, billed to the
    /// balance, and each is signed on this machine with the cluster key file: the masters
    /// refuse a node the key did not sign. After the answer, hipc reads the pools and operations
    /// again, and refuses as conflict, sending nothing, when the pool is no longer the size the
    /// question named, or, when nodes are added, when its signing params changed; a change
    /// after that read and before the request is not seen. Scaling down signs nothing and
    /// needs no key file:
    /// adds still queued are cancelled first, then the newest nodes are drained and their VMs
    /// decommissioned, and a node with local data blocks until its loss is accepted in the
    /// console.
    #[command(after_help = examples(&[
        "hipc compute kubernetes node-pools scale 7 3 --count 2 --cluster-key-file \
         hippius-cluster-prod.key --yes",
        "hipc compute kubernetes node-pools scale 7 3 --count 0 --yes",
    ]))]
    Scale(NodePoolScale),
}

#[derive(Debug, Args)]
pub(crate) struct NodePoolScale {
    /// The cluster id from hipc compute kubernetes clusters list
    #[arg(value_parser = number(CLUSTER_ID))]
    pub(crate) cluster_id: u64,
    /// The node pool id from hipc compute kubernetes node-pools list
    #[arg(value_parser = number(POOL_ID))]
    pub(crate) pool_id: u64,
    /// The number of nodes the pool should have, 0 to 1000
    #[arg(long, value_name = "N", value_parser = pool_size)]
    pub(crate) count: u64,
    /// The key file the console saved when it created the cluster, hippius-cluster-<name>.key;
    /// needed to add nodes, and read before anything is sent when given
    ///
    /// It is read only as a regular file of the current user, never through a symlink, with
    /// no bits for the group or others; there is no stdin form.
    #[arg(long, value_name = "PATH")]
    pub(crate) cluster_key_file: Option<PathBuf>,
}

/// The most nodes `--count` asks for: arbitrary, far above the backend's 50 workers per pool
/// today, so the server judges the size, while a mistyped count cannot make hipc sign thousands
/// of operations before the server refuses them.
const MAX_POOL_SIZE: u64 = 1000;

/// A `--count`: a whole number from 0 to [`MAX_POOL_SIZE`].
fn pool_size(text: &str) -> Result<u64, String> {
    text.parse()
        .ok()
        .filter(|count| *count <= MAX_POOL_SIZE)
        .ok_or_else(|| format!("a pool size is a whole number from 0 to {MAX_POOL_SIZE}"))
}

#[derive(Debug, Args)]
pub(crate) struct NodePoolCreate {
    /// The cluster id from hipc compute kubernetes clusters list
    #[arg(value_parser = number(CLUSTER_ID))]
    pub(crate) cluster_id: u64,
    /// The pool's name: lowercase letters, digits, and -, at most 20 characters, not starting
    /// with -
    #[arg(long, value_parser = pool_name)]
    pub(crate) name: String,
    /// The flavor of the pool's nodes, a code from hipc compute flavors
    #[arg(long, value_parser = flavor)]
    pub(crate) flavor: String,
    /// A Kubernetes label for the pool's nodes, as KEY=VALUE; repeat for more
    ///
    /// The value may be empty, as in KEY=.
    #[arg(long = "label", value_name = "KEY=VALUE", value_parser = label)]
    pub(crate) labels: Vec<Label>,
    /// A Kubernetes taint for the pool's nodes, as KEY[=VALUE]:EFFECT, such as
    /// gpu=true:NoSchedule; repeat for more
    #[arg(long = "taint", value_name = "TAINT", value_parser = taint)]
    pub(crate) taints: Vec<String>,
}

/// A node pool `--name`: 1 to 20 of `a-z`, `0-9`, and `-`, as its help says, not starting
/// with `-`, which a command line reads as a flag.
fn pool_name(text: &str) -> Result<String, String> {
    word(
        text,
        1..=20,
        Charset::LowerDash,
        Dash::Refused,
        "a node pool name is 1 to 20 of `a-z`, `0-9`, and `-`, and does not start with `-`",
    )
}

/// The effects a Kubernetes taint takes.
const TAINT_EFFECTS: [&str; 3] = ["NoSchedule", "PreferNoSchedule", "NoExecute"];

/// A `--taint`: `KEY[=VALUE]:EFFECT`, a key before any `=` and an effect after the `:` that
/// Kubernetes knows. Neither a key nor a value may hold `:` in Kubernetes, so `a:b:NoSchedule`
/// is refused here; the server judges the rest of the key and value against the Kubernetes
/// rules.
fn taint(text: &str) -> Result<String, String> {
    let valid = text.rsplit_once(':').is_some_and(|(pair, effect)| {
        let key = pair.split_once('=').map_or(pair, |(key, _)| key);
        !key.is_empty() && !pair.contains(':') && TAINT_EFFECTS.contains(&effect)
    });
    if valid {
        Ok(text.to_owned())
    } else {
        Err(
            "a taint is `KEY[=VALUE]:EFFECT`, with an effect of `NoSchedule`, \
             `PreferNoSchedule`, or `NoExecute`, such as `gpu=true:NoSchedule`"
                .to_owned(),
        )
    }
}

/// One `--label KEY=VALUE`.
#[derive(Debug, Clone)]
pub(crate) struct Label {
    pub(crate) key: String,
    pub(crate) value: String,
}

/// A `--label`: the key before the first `=`, which must not be empty, and the rest as the
/// value. The server judges the key and value against the Kubernetes rules.
fn label(text: &str) -> Result<Label, String> {
    match text.split_once('=') {
        Some((key, value)) if !key.is_empty() => Ok(Label {
            key: key.to_owned(),
            value: value.to_owned(),
        }),
        _ => Err("a label is `KEY=VALUE`, with a key before the `=`".to_owned()),
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum Operations {
    /// The cluster's node operations, each with its kind, state, and step
    #[command(after_help = examples(&["hipc compute kubernetes operations list 7"]))]
    List {
        /// The cluster id from hipc compute kubernetes clusters list
        #[arg(value_parser = number(CLUSTER_ID))]
        cluster_id: u64,
    },
    /// Cancel a queued, running, or blocked operation; needs --yes
    ///
    /// Before the question, hipc reads the cluster's operations, so the question names the
    /// operation's state and kind and what cancelling it leaves undone; --dry-run reads nothing
    /// and lists the read in requests. An operation that is done, failed, or cancelled has
    /// nothing left to cancel, so hipc refuses it as conflict (exit 1) and sends nothing after
    /// the read.
    #[command(after_help = examples(&["hipc compute kubernetes operations cancel 7 12 --yes"]))]
    Cancel(OperationTarget),
    /// Resume a blocked or queued operation; spends money when it launches a VM: needs --yes
    ///
    /// Before the question, hipc reads the cluster's operations, so the question names the
    /// operation's state and kind and what going on with it does, such as launching a VM billed
    /// to the balance; --dry-run reads nothing and lists the read in requests. A drain that
    /// stopped on a disruption budget or an unmanaged pod tries again. An operation in any
    /// state but blocked or queued, or with none, cannot be resumed, and one blocked because
    /// data on the node would be lost needs the signed acceptance only the console makes; hipc
    /// refuses both as conflict (exit 1) and sends nothing after the read.
    #[command(after_help = examples(&["hipc compute kubernetes operations resume 7 12 --yes"]))]
    Resume(OperationTarget),
}

#[derive(Debug, Args)]
pub(crate) struct OperationTarget {
    /// The cluster id from hipc compute kubernetes clusters list
    #[arg(value_parser = number(CLUSTER_ID))]
    pub(crate) cluster_id: u64,
    /// The operation id from hipc compute kubernetes operations list
    #[arg(value_parser = number(OPERATION_ID))]
    pub(crate) operation_id: u64,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Nodes {
    /// Remove a worker node from the cluster: its pods are drained to other nodes, then its VM
    /// is decommissioned; needs --yes
    ///
    /// The command returns the remove operation, which runs on the masters; hipc compute
    /// kubernetes operations list follows it. If data on the node would be lost, the operation
    /// blocks until that loss is accepted in the console.
    #[command(after_help = examples(&[
        "hipc compute kubernetes nodes delete 7 work-3a5bf1ed --yes",
    ]))]
    Delete {
        /// The cluster id from hipc compute kubernetes clusters list
        #[arg(value_parser = number(CLUSTER_ID))]
        cluster_id: u64,
        /// The node's name from hipc compute kubernetes node-pools list
        #[arg(value_parser = node_name)]
        node_name: String,
    },
    /// Replace a worker node with a new one of its node pool; spends money: needs --yes and
    /// --cluster-key-file
    ///
    /// A new VM of the pool's flavor, billed to the balance, joins first; once it is ready,
    /// the old node is drained and its VM decommissioned, and both are billed while they
    /// overlap. The new node is signed on this machine with the cluster key file. Before the
    /// question, hipc reads the cluster, its pools, and its operations, so the question names
    /// the pool; --dry-run reads nothing and lists the reads in requests. A node that an
    /// operation in flight already removes or replaces is refused, before the question and
    /// again before signing; an operation that starts after that read is not seen. The command
    /// returns the add operation; hipc compute kubernetes operations list follows it.
    #[command(after_help = examples(&[
        "hipc compute kubernetes nodes replace 7 work-3a5bf1ed --cluster-key-file \
         hippius-cluster-prod.key --yes",
    ]))]
    Replace(NodeReplace),
}

#[derive(Debug, Args)]
pub(crate) struct NodeReplace {
    /// The cluster id from hipc compute kubernetes clusters list
    #[arg(value_parser = number(CLUSTER_ID))]
    pub(crate) cluster_id: u64,
    /// The node's name from hipc compute kubernetes node-pools list
    #[arg(value_parser = node_name)]
    pub(crate) node_name: String,
    /// The key file the console saved when it created the cluster, hippius-cluster-<name>.key;
    /// it must be a regular file only its owner can read (chmod 600)
    ///
    /// It is read only as a regular file of the current user, never through a symlink, with
    /// no bits for the group or others; there is no stdin form.
    #[arg(long, value_name = "PATH")]
    pub(crate) cluster_key_file: PathBuf,
}

/// A node name: it goes into the URL path, so only the characters a name has are accepted.
/// The console names a worker `work-` and hex (`work-3a5bf1ed`); swagger types the name
/// as a string. The rule of a VM id, 1 to 64 letters, digits, `-`, and `_`, takes them, and
/// no `/`, `?`, or dot segment can change the route.
fn node_name(value: &str) -> Result<String, String> {
    word(
        value,
        1..=64,
        Charset::Slug,
        Dash::Taken,
        "a node name is 1 to 64 letters, digits, `-`, and `_`, from \
         `hipc compute kubernetes node-pools list`",
    )
}
