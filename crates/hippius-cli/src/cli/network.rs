//! The grammar of `hipc network`, the account's private network.

use clap::{ArgAction, Args, Subcommand, ValueEnum};

use crate::cli::examples;
use crate::cli::values::{Charset, Dash, Spaces, label, number, word};

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
    /// The network's groups of peers: list, create, and delete them, and set their peers
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc network groups list",
        "hipc network groups create --name build-boxes",
        "hipc network groups update 5 --peer 12 --peer 13 --yes",
    ]))]
    Groups(Groups),
    /// The network's access policies, each one ruling the traffic from one group to another
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc network policies list",
        "hipc network policies create --source-group 5 --dest-group 6 --protocol tcp \
         --port 22 --yes",
        "hipc network policies delete 9 --yes",
    ]))]
    Policies(Policies),
    /// The network's VPC networks, and the resources and routers of each
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc network vpcs list",
        "hipc network vpcs create --name office",
        "hipc network vpcs resources list 3",
    ]))]
    Vpcs(Vpcs),
    /// Whether the peers of the network reach each other without a policy that says so
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc network isolation get",
        "hipc network isolation update --mode deny_intra --change-isolation --yes",
    ]))]
    Isolation(Isolation),
    /// The whole network as a graph: its groups, peers, resources, and routers, and the
    /// policies and routes between them
    ///
    /// The first read of the topology creates the account's network, as the first hipc
    /// network get does, so hipc reads the group list first, which creates nothing and
    /// answers that the network is not set up when it has none; only then does it read the
    /// topology, which then creates nothing either. The network's system groups and the
    /// policies between them are left out, as the console shows it, unless --system is given.
    /// Edge and node ids are kind:id, such as policy:9; the meta of each holds the plain id
    /// the other commands take.
    #[command(after_help = examples(&[
        "hipc network topology",
        "hipc network topology --system",
        "hipc --dry-run network topology",
    ]))]
    Topology {
        /// Also show the system groups, which Hippius manages for managed services, and the
        /// policies between them
        #[arg(long)]
        system: bool,
    },
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
pub(crate) enum Groups {
    /// The network's groups of peers, the default group included
    #[command(after_help = examples(&["hipc network groups list"]))]
    List,
    /// Create an empty group of peers, which a policy can then name
    ///
    /// Sends POST /api/network/groups/ with the name as label. A new group has no peers and
    /// no policy, so it changes no access; hipc network groups update sets its peers.
    #[command(after_help = examples(&[
        "hipc network groups create --name build-boxes",
        "hipc --dry-run network groups create --name build-boxes",
    ]))]
    Create {
        /// The group's name: 1 to 50 lowercase letters, digits, and -
        #[arg(long, value_parser = group_name)]
        name: String,
    },
    /// Set which peers are in a group; needs --yes to change them, and --remove-others to
    /// drop one
    ///
    /// The peers given become the group's whole list; --remove-others with no --peer empties
    /// the group. Before the question hipc reads the group list, to name the group, and the
    /// network's topology, to find the peers in the group now and to check that each peer
    /// given exists. A list that leaves out a peer the group has now is refused unless
    /// --remove-others is passed. A managed database's membership is kept by Hippius, so a
    /// list that would add or remove one is refused. After the question hipc reads the
    /// topology again and sends one POST /api/network/groups/GROUP_ID/members/ per peer it
    /// adds or removes, adds first; a list that is already the group's sends nothing and asks
    /// nothing. A peer added gains the access the group's policies give; one removed loses
    /// it. The default group and the groups Hippius keeps for each VM are refused by the
    /// server (409). When a step fails after others went through, stdout holds the group's
    /// peers as they now are and what changed; running the same command again sends only
    /// what is left.
    #[command(after_help = examples(&[
        "hipc network groups update 5 --peer 12 --peer 13 --yes",
        "hipc network groups update 5 --peer 12 --remove-others --yes",
        "hipc network groups update 5 --remove-others --yes",
        "hipc --dry-run network groups update 5 --peer 12",
    ]))]
    Update {
        /// The group id, from hipc network groups list
        #[arg(value_parser = group_id)]
        group_id: u64,
        /// A peer the group holds, by its id from hipc network peers list; repeat for each
        #[arg(
            long = "peer",
            value_name = "PEER_ID",
            required_unless_present = "remove_others",
            value_parser = body_peer_id
        )]
        peers: Vec<u64>,
        /// Let the list remove the group's peers it leaves out; with no --peer, every peer
        #[arg(long)]
        remove_others: bool,
    },
    /// Delete a group; needs --yes
    ///
    /// Sends DELETE /api/network/groups/GROUP_ID/. Its peers lose the access its policies
    /// give them, and a group made again gets a new id. The server refuses the default group.
    #[command(after_help = examples(&[
        "hipc network groups delete 5 --yes",
        "hipc --dry-run network groups delete 5",
    ]))]
    Delete {
        /// The group id, from hipc network groups list
        #[arg(value_parser = group_id)]
        group_id: u64,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum Policies {
    /// The network's access policies, each one ruling the traffic from one group to another
    #[command(after_help = examples(&["hipc network policies list"]))]
    List,
    /// Allow or drop the traffic from one group of peers to another; needs --yes
    ///
    /// Sends POST /api/network/policies/ with the groups and the fields given; the server
    /// fills the rest, and names the policy SOURCE-to-DEST without --name. A policy that
    /// accepts lets the source group's peers reach the destination group's on the protocol
    /// and ports given; one that drops stops that traffic. The server refuses a group that is
    /// not the account's (400).
    #[command(after_help = examples(&[
        "hipc network policies create --source-group 5 --dest-group 6 --protocol tcp \
         --port 22 --port 8000-8080 --yes",
        "hipc --dry-run network policies create --source-group 5 --dest-group 6",
    ]))]
    Create(PolicyCreate),
    /// Change a policy's groups, traffic, action, or name; needs --yes, and --remove-others to
    /// drop a port
    ///
    /// Sends PATCH /api/network/policies/POLICY_ID/ with only the fields given; the others
    /// stay. A change to what a policy allows changes who reaches whom. --port replaces the
    /// policy's whole list of ports, so before the question hipc reads the policy list and
    /// refuses a list that leaves out a port the policy has now unless --remove-others is
    /// passed. The server refuses a policy Hippius manages (409).
    #[command(after_help = examples(&[
        "hipc network policies update 9 --enabled false --yes",
        "hipc network policies update 9 --protocol tcp --port 443 --yes",
        "hipc --dry-run network policies update 9 --action drop",
    ]))]
    Update(PolicyUpdate),
    /// Delete a policy; needs --yes
    ///
    /// Sends DELETE /api/network/policies/POLICY_ID/. The traffic it allowed is no longer
    /// allowed by it, and traffic it dropped may pass. The server refuses a policy Hippius
    /// manages (409).
    #[command(after_help = examples(&[
        "hipc network policies delete 9 --yes",
        "hipc --dry-run network policies delete 9",
    ]))]
    Delete {
        /// The policy id, from hipc network policies list
        #[arg(value_parser = policy_id)]
        policy_id: u64,
    },
}

/// `hipc network policies create`.
#[derive(Debug, Args)]
pub(crate) struct PolicyCreate {
    /// The group the traffic comes from, by its id from hipc network groups list
    #[arg(long, value_name = "GROUP_ID", value_parser = group_id)]
    pub(crate) source_group: u64,
    /// The group the traffic goes to, by its id from hipc network groups list
    #[arg(long, value_name = "GROUP_ID", value_parser = group_id)]
    pub(crate) dest_group: u64,
    #[command(flatten)]
    pub(crate) fields: PolicyFields,
}

/// `hipc network policies update`: at least one field to change.
#[derive(Debug, Args)]
#[command(group(
    clap::ArgGroup::new("change").required(true).multiple(true).args([
        "source_group",
        "dest_group",
        "name",
        "protocol",
        "ports",
        "bidirectional",
        "action",
        "enabled",
    ])
))]
pub(crate) struct PolicyUpdate {
    /// The policy id, from hipc network policies list
    #[arg(value_parser = policy_id)]
    pub(crate) policy_id: u64,
    /// The group the traffic comes from, by its id from hipc network groups list
    #[arg(long, value_name = "GROUP_ID", value_parser = group_id)]
    pub(crate) source_group: Option<u64>,
    /// The group the traffic goes to, by its id from hipc network groups list
    #[arg(long, value_name = "GROUP_ID", value_parser = group_id)]
    pub(crate) dest_group: Option<u64>,
    #[command(flatten)]
    pub(crate) fields: PolicyFields,
    /// Let --port remove the policy's ports it leaves out
    #[arg(long)]
    pub(crate) remove_others: bool,
}

/// The fields of a policy besides its groups, each sent only when given.
#[derive(Debug, Args)]
pub(crate) struct PolicyFields {
    /// The policy's name, 1 to 255 characters on one line
    #[arg(long, value_parser = policy_name)]
    pub(crate) name: Option<String>,
    /// The protocol: all, tcp, udp, or icmp
    #[arg(long, value_enum)]
    pub(crate) protocol: Option<Protocol>,
    /// A port or a LOW-HIGH range, with --protocol tcp or udp; repeat for each
    #[arg(long = "port", value_name = "PORT", value_parser = port_spec)]
    pub(crate) ports: Vec<String>,
    /// Whether the destination group may start the traffic too: true or false
    #[arg(long, value_name = "BOOL", action = ArgAction::Set)]
    pub(crate) bidirectional: Option<bool>,
    /// What the policy does with the traffic: accept or drop
    #[arg(long, value_enum)]
    pub(crate) action: Option<PolicyAction>,
    /// Whether the policy applies: true or false
    #[arg(long, value_name = "BOOL", action = ArgAction::Set)]
    pub(crate) enabled: Option<bool>,
}

/// A policy's `protocol` (swagger `network_policies_create`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Protocol {
    All,
    Tcp,
    Udp,
    Icmp,
}

impl Protocol {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Tcp => "tcp",
            Self::Udp => "udp",
            Self::Icmp => "icmp",
        }
    }

    /// Whether the protocol has ports: `NetBird` ignores the ports of the others, as the
    /// console's network types say.
    pub(crate) fn has_ports(self) -> bool {
        matches!(self, Self::Tcp | Self::Udp)
    }
}

/// A policy's `action` (swagger `network_policies_create`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum PolicyAction {
    Accept,
    Drop,
}

impl PolicyAction {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Drop => "drop",
        }
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum Vpcs {
    /// The VPC networks, the default one included
    #[command(after_help = examples(&["hipc network vpcs list"]))]
    List,
    /// Create a VPC network, with no resources and no routers
    ///
    /// Sends POST /api/network/networks/ with the name. An account has a default VPC network
    /// from the start; a new one reaches nothing until a resource and a router are added.
    #[command(after_help = examples(&[
        "hipc network vpcs create --name office",
        "hipc --dry-run network vpcs create --name office",
    ]))]
    Create {
        /// The VPC network's name, 1 to 255 characters on one line
        #[arg(long, value_parser = vpc_name)]
        name: String,
    },
    /// Delete a VPC network; needs --yes
    ///
    /// Sends DELETE /api/network/networks/VPC_ID/. Peers lose the routes it gave them to its
    /// resources.
    #[command(after_help = examples(&[
        "hipc network vpcs delete 3 --yes",
        "hipc --dry-run network vpcs delete 3",
    ]))]
    Delete {
        /// The VPC network id, from hipc network vpcs list
        #[arg(value_parser = vpc_id)]
        vpc_id: String,
    },
    /// The resources of a VPC network: the hosts, subnets, and domains it reaches
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc network vpcs resources list 3",
        "hipc network vpcs resources create 3 --name db --address 10.0.0.5 --yes",
    ]))]
    Resources(VpcResources),
    /// The routers of a VPC network: the peers that route its traffic
    #[command(subcommand)]
    #[command(after_help = examples(&[
        "hipc network vpcs routers list 3",
        "hipc network vpcs routers create 3 --peer 12 --yes",
    ]))]
    Routers(VpcRouters),
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
    /// Add a host, subnet, or domain to a VPC network; needs --yes
    ///
    /// Sends POST /api/network/networks/VPC_ID/resources/ with the fields given. The peers of
    /// the groups given reach the resource through the VPC network's routers. Without --kind
    /// the server infers it from the address. The server refuses a group that is not the
    /// account's.
    #[command(after_help = examples(&[
        "hipc network vpcs resources create 3 --name db --address 10.0.0.5 --group 5 --yes",
        "hipc network vpcs resources create 3 --name lan --address 10.0.0.0/24 --yes",
        "hipc --dry-run network vpcs resources create 3 --name db --address 10.0.0.5",
    ]))]
    Create {
        /// The VPC network id, from hipc network vpcs list
        #[arg(value_parser = vpc_id)]
        vpc_id: String,
        /// The resource's name, 1 to 255 characters on one line
        #[arg(long, value_parser = resource_name)]
        name: String,
        /// An IP address, a CIDR subnet, or a domain
        #[arg(long, value_parser = address)]
        address: String,
        #[command(flatten)]
        fields: ResourceFields,
    },
    /// Change a resource's name, address, kind, or groups; needs --yes, and --remove-others to
    /// drop a group
    ///
    /// Sends PATCH /api/network/networks/VPC_ID/resources/RESOURCE_ID/ with only the fields
    /// given; the others stay. --group replaces the resource's whole list of groups, so
    /// before the question hipc reads the VPC network's resources and refuses a list that
    /// leaves out a group the resource is in now unless --remove-others is passed: that
    /// group's peers would lose their route to it. A new address changes who reaches what.
    #[command(after_help = examples(&[
        "hipc network vpcs resources update 3 7 --address 10.0.0.6 --yes",
        "hipc --dry-run network vpcs resources update 3 7 --group 5 --group 6",
    ]))]
    Update(ResourceUpdate),
    /// Remove a resource from a VPC network; needs --yes
    ///
    /// Sends DELETE /api/network/networks/VPC_ID/resources/RESOURCE_ID/. Peers lose their
    /// route to it.
    #[command(after_help = examples(&[
        "hipc network vpcs resources delete 3 7 --yes",
        "hipc --dry-run network vpcs resources delete 3 7",
    ]))]
    Delete {
        /// The VPC network id, from hipc network vpcs list
        #[arg(value_parser = vpc_id)]
        vpc_id: String,
        /// The resource id, from hipc network vpcs resources list
        #[arg(value_parser = resource_id)]
        resource_id: u64,
    },
}

/// `hipc network vpcs resources update`: at least one field to change.
#[derive(Debug, Args)]
#[command(group(
    clap::ArgGroup::new("change").required(true).multiple(true).args([
        "name",
        "address",
        "kind",
        "groups",
    ])
))]
pub(crate) struct ResourceUpdate {
    /// The VPC network id, from hipc network vpcs list
    #[arg(value_parser = vpc_id)]
    pub(crate) vpc_id: String,
    /// The resource id, from hipc network vpcs resources list
    #[arg(value_parser = resource_id)]
    pub(crate) resource_id: u64,
    /// The resource's name, 1 to 255 characters on one line
    #[arg(long, value_parser = resource_name)]
    pub(crate) name: Option<String>,
    /// An IP address, a CIDR subnet, or a domain
    #[arg(long, value_parser = address)]
    pub(crate) address: Option<String>,
    #[command(flatten)]
    pub(crate) fields: ResourceFields,
    /// Let --group remove the resource's groups it leaves out
    #[arg(long)]
    pub(crate) remove_others: bool,
}

/// The fields of a resource besides its name and address, each sent only when given.
#[derive(Debug, Args)]
pub(crate) struct ResourceFields {
    /// What the address is: host, subnet, or domain
    #[arg(long, value_enum)]
    pub(crate) kind: Option<ResourceKind>,
    /// A group whose peers reach the resource, by its id from hipc network groups list;
    /// repeat for each
    #[arg(long = "group", value_name = "GROUP_ID", value_parser = group_id)]
    pub(crate) groups: Vec<u64>,
}

/// A resource's `kind` (swagger `network_networks_resources_create`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ResourceKind {
    Host,
    Subnet,
    Domain,
}

impl ResourceKind {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Subnet => "subnet",
            Self::Domain => "domain",
        }
    }
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
    /// Make a peer route a VPC network's traffic to its resources; needs --yes
    ///
    /// Sends POST /api/network/networks/VPC_ID/routers/ with the peer and the fields given.
    /// The VPC network's traffic then goes through that peer. The server refuses a peer that
    /// is not the account's.
    #[command(after_help = examples(&[
        "hipc network vpcs routers create 3 --peer 12 --yes",
        "hipc network vpcs routers create 3 --peer 12 --masquerade true --metric 100 --yes",
        "hipc --dry-run network vpcs routers create 3 --peer 12",
    ]))]
    Create {
        /// The VPC network id, from hipc network vpcs list
        #[arg(value_parser = vpc_id)]
        vpc_id: String,
        /// The peer that routes the traffic, by its id from hipc network peers list
        #[arg(long = "peer", value_name = "PEER_ID", value_parser = body_peer_id)]
        peer: u64,
        /// Whether the router masks the source address of what it routes: true or false
        #[arg(long, value_name = "BOOL", action = ArgAction::Set)]
        masquerade: Option<bool>,
        /// The router's priority among the VPC network's routers; the lower number wins
        #[arg(long, value_name = "NUMBER", value_parser = number("a metric is a whole number"))]
        metric: Option<u64>,
    },
    /// Stop a peer routing a VPC network's traffic; needs --yes
    ///
    /// Sends DELETE /api/network/networks/VPC_ID/routers/ROUTER_ID/. The VPC network's
    /// traffic no longer goes through that peer; with no router left, peers cannot reach its
    /// resources.
    #[command(after_help = examples(&[
        "hipc network vpcs routers delete 3 4 --yes",
        "hipc --dry-run network vpcs routers delete 3 4",
    ]))]
    Delete {
        /// The VPC network id, from hipc network vpcs list
        #[arg(value_parser = vpc_id)]
        vpc_id: String,
        /// The router id, from hipc network vpcs routers list
        #[arg(value_parser = router_id)]
        router_id: u64,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum Isolation {
    /// The network's isolation mode: allow_intra or deny_intra
    #[command(after_help = examples(&["hipc network isolation get"]))]
    Get,
    /// Switch the isolation mode; needs --yes, and --change-isolation
    ///
    /// Sends POST /api/network/isolation/ with the mode and prints the network's summary. In
    /// allow_intra, the default, every peer of the network reaches every other, and policies
    /// can only widen that. In deny_intra only what a policy allows passes, so peers lose
    /// every connection no policy names. Either switch changes who reaches whom, so it needs
    /// --change-isolation besides --yes, as hipc api does on this route. A repeat leaves the
    /// mode as it is.
    #[command(after_help = examples(&[
        "hipc network isolation update --mode deny_intra --change-isolation --yes",
        "hipc --dry-run network isolation update --mode allow_intra",
    ]))]
    Update {
        /// The new mode: allow_intra or deny_intra
        #[arg(long, value_enum)]
        mode: IsolationMode,
        /// Let the switch change which peers can reach each other
        #[arg(long)]
        change_isolation: bool,
    },
}

/// The network's `isolation_mode` (swagger `network_isolation_create`), spelled as the API
/// spells it, so a value read from hipc network isolation get is typed as it was printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum IsolationMode {
    #[value(name = "allow_intra")]
    AllowIntra,
    #[value(name = "deny_intra")]
    DenyIntra,
}

impl IsolationMode {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::AllowIntra => "allow_intra",
            Self::DenyIntra => "deny_intra",
        }
    }
}

/// A peer id. It goes into the URL path, and swagger types it as a string with no pattern,
/// so it is held to the rule of a VM id: no `/`, `?`, or `.` can change the route.
fn peer_id(value: &str) -> Result<String, String> {
    word(
        value,
        1..=64,
        Charset::Slug,
        Dash::Taken,
        "a peer id is 1 to 64 letters, digits, `-`, and `_`, from `hipc network peers list`",
    )
}

/// A peer id that goes into a body, where the console's network types make it a number.
fn body_peer_id(value: &str) -> Result<u64, String> {
    number("a peer id is a whole number, from `hipc network peers list`")(value)
}

/// A group id: the console's network types make it a number, which a policy's body takes.
fn group_id(value: &str) -> Result<u64, String> {
    number("a group id is a whole number, from `hipc network groups list`")(value)
}

/// A policy id: a number in the console's network types.
fn policy_id(value: &str) -> Result<u64, String> {
    number("a policy id is a whole number, from `hipc network policies list`")(value)
}

/// A resource id: a number in the console's network types.
fn resource_id(value: &str) -> Result<u64, String> {
    number("a resource id is a whole number, from `hipc network vpcs resources list`")(value)
}

/// A router id: a number in the console's network types.
fn router_id(value: &str) -> Result<u64, String> {
    number("a router id is a whole number, from `hipc network vpcs routers list`")(value)
}

/// A VPC network id (swagger's `nid`, a string), held to the same rule as [`peer_id`].
fn vpc_id(value: &str) -> Result<String, String> {
    word(
        value,
        1..=64,
        Charset::Slug,
        Dash::Taken,
        "a VPC network id is 1 to 64 letters, digits, `-`, and `_`, from \
         `hipc network vpcs list`",
    )
}

/// The most characters a peer, policy, VPC network, or resource name may have. Swagger gives
/// none of them a limit; 255 is arbitrary, the limit hipc holds a support subject to, and
/// keeps a pasted file out of the request.
const MAX_NAME: usize = 255;

/// A new peer name: a [`label`], so an empty `--name` or a newline inside it is refused here rather
/// than by the server after the question. The server judges the rest. The value is sent trimmed:
/// the rename compares the answer's name with what it sent, and a server that trims would otherwise
/// answer a name hipc does not recognise.
fn peer_name(value: &str) -> Result<String, String> {
    label(value, 1..=MAX_NAME, Spaces::Trimmed, "a peer name")
}

/// A policy's name, a [`label`] sent trimmed, as a peer's is.
fn policy_name(value: &str) -> Result<String, String> {
    label(value, 1..=MAX_NAME, Spaces::Trimmed, "a policy name")
}

/// A VPC network's name, a [`label`] sent trimmed, as a peer's is.
fn vpc_name(value: &str) -> Result<String, String> {
    label(value, 1..=MAX_NAME, Spaces::Trimmed, "a VPC network name")
}

/// A resource's name, a [`label`] sent trimmed, as a peer's is.
fn resource_name(value: &str) -> Result<String, String> {
    label(value, 1..=MAX_NAME, Spaces::Trimmed, "a resource name")
}

/// A group's name, by swagger's rule for `label`: 1 to 50 lowercase letters, digits, and
/// `-`. One that starts with `-` is refused, since a command line would read it as a flag.
fn group_name(value: &str) -> Result<String, String> {
    word(
        value,
        1..=50,
        Charset::LowerDash,
        Dash::Refused,
        "a group name is 1 to 50 lowercase letters, digits, and `-`, not starting with `-`",
    )
}

/// A resource's address: one word of up to 253 characters, the longest domain name. Whether it
/// is an IP address, a CIDR subnet, or a domain is the server's to judge, which infers the
/// kind from it.
fn address(value: &str) -> Result<String, String> {
    word(
        value,
        1..=253,
        Charset::OneWord,
        Dash::Refused,
        "an address is an IP address, a CIDR subnet, or a domain, one word of up to 253 \
         characters",
    )
}

/// A port of a policy: a port, or an inclusive `low-high` range, each 1 to 65535, the low end
/// first, which the console holds a port to as the whole of what `NetBird` accepts. It is
/// sent as typed, the digits checked.
fn port_spec(value: &str) -> Result<String, String> {
    let port = |part: &str| {
        let digits = (1..=5).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_digit());
        digits
            .then(|| part.parse::<u32>().ok())
            .flatten()
            .filter(|port| (1..=65_535).contains(port))
    };
    let valid = match value.split_once('-') {
        None => port(value).is_some(),
        Some((low, high)) => {
            matches!((port(low), port(high)), (Some(low), Some(high)) if low <= high)
        }
    };
    if valid {
        Ok(value.to_owned())
    } else {
        Err("a port is a number from 1 to 65535, or a range LOW-HIGH of two, low first".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_port_is_one_number_or_an_ordered_range_within_1_to_65535() {
        for good in [
            "1",
            "22",
            "65535",
            "8000-8080",
            "1-65535",
            "443-443",
            "00022",
        ] {
            assert_eq!(port_spec(good).as_deref(), Ok(good), "{good}");
        }
        for bad in [
            "",
            "0",
            "65536",
            "99999",
            "123456",
            "-1",
            "1-",
            "-",
            "8080-8000",
            "1-2-3",
            "a",
            "22 ",
            "+22",
            "0-10",
            "10-65536",
            "1.5",
        ] {
            assert!(port_spec(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_group_name_is_lowercase_letters_digits_and_dashes_up_to_50() {
        let widest = "a".repeat(50);
        for good in ["a", "build-boxes", "x9", widest.as_str()] {
            assert_eq!(group_name(good).as_deref(), Ok(good));
        }
        let long = "a".repeat(51);
        for bad in ["", "Build", "a_b", "a b", "-a", "a.b", long.as_str()] {
            assert!(group_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn an_address_is_one_word_of_up_to_253_characters() {
        let widest = "a".repeat(253);
        for good in [
            "10.0.0.5",
            "10.0.0.0/24",
            "db.internal",
            "::1",
            widest.as_str(),
        ] {
            assert_eq!(address(good).as_deref(), Ok(good));
        }
        let long = "a".repeat(254);
        for bad in ["", "a b", "-x", "a\tb", long.as_str()] {
            assert!(address(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn numeric_ids_name_the_list_that_prints_them() {
        assert_eq!(group_id("5"), Ok(5));
        assert_eq!(policy_id("9"), Ok(9));
        assert_eq!(resource_id("7"), Ok(7));
        assert_eq!(router_id("4"), Ok(4));
        assert_eq!(body_peer_id("12"), Ok(12));
        for (parse, list) in [
            (group_id as fn(&str) -> Result<u64, String>, "groups list"),
            (policy_id, "policies list"),
            (resource_id, "vpcs resources list"),
            (router_id, "vpcs routers list"),
            (body_peer_id, "peers list"),
        ] {
            for bad in ["", "-1", "x", "1.0", "+1"] {
                let refusal = parse(bad).unwrap_err();
                assert!(
                    refusal.contains(&format!("`hipc network {list}`")),
                    "{refusal}"
                );
            }
        }
    }

    #[test]
    fn each_code_is_the_api_spelling() {
        let protocols = [Protocol::All, Protocol::Tcp, Protocol::Udp, Protocol::Icmp];
        let codes: Vec<&str> = protocols.iter().map(|p| p.code()).collect();
        assert_eq!(codes, ["all", "tcp", "udp", "icmp"]);
        let ported: Vec<bool> = protocols.iter().map(|p| p.has_ports()).collect();
        assert_eq!(ported, [false, true, true, false]);
        assert_eq!(PolicyAction::Accept.code(), "accept");
        assert_eq!(PolicyAction::Drop.code(), "drop");
        assert_eq!(ResourceKind::Host.code(), "host");
        assert_eq!(ResourceKind::Subnet.code(), "subnet");
        assert_eq!(ResourceKind::Domain.code(), "domain");
        assert_eq!(IsolationMode::AllowIntra.code(), "allow_intra");
        assert_eq!(IsolationMode::DenyIntra.code(), "deny_intra");
        for mode in [IsolationMode::AllowIntra, IsolationMode::DenyIntra] {
            let parsed = IsolationMode::from_str(mode.code(), false);
            assert_eq!(parsed, Ok(mode));
        }
    }
}
