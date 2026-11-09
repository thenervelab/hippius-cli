//! The grammar of `hipc network`, the account's private network.

use clap::Subcommand;

use crate::cli::{examples, slug_id};

#[derive(Debug, Subcommand)]
pub(crate) enum Network {
    /// The private network's summary: its status and counts; needs --yes
    ///
    /// The first call for an account creates its private network: a default group, a VPC
    /// network, and a policy that lets its peers reach each other. Which call is the first is
    /// not known before it is sent, so every call is confirmed. A suspended network answers 403.
    /// To set the network up on purpose, run hipc network create.
    #[command(after_help = examples(&["hipc network get --yes", "hipc --dry-run network get"]))]
    Get,
    /// Create the account's private network if it has none; needs --yes
    ///
    /// Sends POST /api/network/provision/ with no body and prints the network's summary. A new
    /// network has a default group, a VPC network, and a policy that lets its peers reach each
    /// other. A repeat finds the network and changes nothing. Until it exists, the network's
    /// lists answer that it is not set up.
    #[command(after_help = examples(&[
        "hipc network create --yes",
        "hipc --dry-run network create",
    ]))]
    Create,
    /// List, read, rename, and remove the devices and VMs in the network
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc network peers list",
        "hipc network peers update 12 --name build-box --yes",
        "hipc network peers delete 12 --yes",
    ]))]
    Peers(Peers),
    /// The network's groups of peers
    // One command until the group writes arrive (tenet 2's short-lived exception).
    #[command(subcommand)]
    #[command(after_help = examples(&["hipc network groups list"]))]
    Groups(Groups),
    /// The network's access policies
    // One command until the policy writes arrive (tenet 2's short-lived exception).
    #[command(subcommand)]
    #[command(after_help = examples(&["hipc network policies list"]))]
    Policies(Policies),
    /// The network's VPC networks, and the resources and routers of each
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc network vpcs list",
        "hipc network vpcs resources list 3",
    ]))]
    Vpcs(Vpcs),
}

#[derive(Debug, Subcommand)]
pub(crate) enum Peers {
    /// The peers in the network
    #[command(after_help = examples(&["hipc network peers list"]))]
    List,
    /// One peer
    #[command(after_help = examples(&["hipc network peers get 12"]))]
    Get {
        /// The peer id, from hipc network peers list
        #[arg(value_parser = peer_id)]
        peer_id: String,
    },
    /// Rename a peer; needs --yes
    #[command(after_help = examples(&[
        "hipc network peers update 12 --name build-box --yes",
        "hipc --dry-run network peers update 12 --name build-box",
    ]))]
    Update {
        /// The peer id, from hipc network peers list
        #[arg(value_parser = peer_id)]
        peer_id: String,
        /// The new name, 1 to 255 characters on one line
        #[arg(long, value_parser = peer_name)]
        name: String,
    },
    /// Remove a peer from the network; needs --yes
    ///
    /// The device or VM loses its access to the private network. Joining again takes a new
    /// enrollment, which stays on hipc api.
    #[command(after_help = examples(&[
        "hipc network peers delete 12 --yes",
        "hipc --dry-run network peers delete 12",
    ]))]
    Delete {
        /// The peer id, from hipc network peers list
        #[arg(value_parser = peer_id)]
        peer_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum Vpcs {
    /// The VPC networks, the default one included
    #[command(after_help = examples(&["hipc network vpcs list"]))]
    List,
    /// The resources of a VPC network: the hosts, subnets, and domains it reaches
    // One command until the resource writes arrive (tenet 2's short-lived exception).
    #[command(subcommand)]
    #[command(after_help = examples(&["hipc network vpcs resources list 3"]))]
    Resources(VpcResources),
    /// The routers of a VPC network: the peers that route its traffic
    // One command until the router writes arrive (tenet 2's short-lived exception).
    #[command(subcommand)]
    #[command(after_help = examples(&["hipc network vpcs routers list 3"]))]
    Routers(VpcRouters),
}

#[derive(Debug, Subcommand)]
pub(crate) enum Groups {
    /// The network's groups of peers, the default group included
    #[command(after_help = examples(&["hipc network groups list"]))]
    List,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Policies {
    /// The network's access policies, each one allowing traffic from one group to another
    #[command(after_help = examples(&["hipc network policies list"]))]
    List,
}

#[derive(Debug, Subcommand)]
pub(crate) enum VpcResources {
    /// The resources of a VPC network: the hosts, subnets, and domains it reaches
    #[command(after_help = examples(&["hipc network vpcs resources list 3"]))]
    List {
        /// The VPC network id, from hipc network vpcs list
        #[arg(value_parser = vpc_id)]
        vpc_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum VpcRouters {
    /// The routers of a VPC network: the peers that route its traffic
    #[command(after_help = examples(&["hipc network vpcs routers list 3"]))]
    List {
        /// The VPC network id, from hipc network vpcs list
        #[arg(value_parser = vpc_id)]
        vpc_id: String,
    },
}

/// A peer id. It goes into the URL path, and swagger types it as a string with no pattern,
/// so it is held to the rule of a VM id: no `/`, `?`, or `.` can change the route.
fn peer_id(value: &str) -> Result<String, String> {
    slug_id(
        value,
        64,
        "a peer id is 1 to 64 letters, digits, `-`, and `_`, from `hipc network peers list`",
    )
}

/// A VPC network id (swagger's `nid`, a string), held to the same rule as [`peer_id`].
fn vpc_id(value: &str) -> Result<String, String> {
    slug_id(
        value,
        64,
        "a VPC network id is 1 to 64 letters, digits, `-`, and `_`, from \
         `hipc network vpcs list`",
    )
}

/// The most characters a peer name may have. Swagger documents no body for the peer
/// `PATCH`; 255 is arbitrary, the limit hipc holds a support subject to, and keeps a pasted
/// file out of the request.
const MAX_PEER_NAME: usize = 255;

/// A new peer name: one line that is not blank, so an empty `--name` or a newline inside
/// it is refused here rather than by the server after the question. The server judges the
/// rest. The value is sent trimmed: the rename compares the answer's name with what it
/// sent, and a server that trims would otherwise answer a name hipc does not recognise.
fn peer_name(value: &str) -> Result<String, String> {
    let value = value.trim();
    let valid = !value.is_empty()
        && value.chars().count() <= MAX_PEER_NAME
        && !value.chars().any(char::is_control);
    if valid {
        Ok(value.to_owned())
    } else {
        Err(format!(
            "a peer name is one line of 1 to {MAX_PEER_NAME} characters"
        ))
    }
}
