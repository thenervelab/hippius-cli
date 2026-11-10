//! The grammar of `hipc compute firewall`.

use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::cli::{examples, vm_id};

#[derive(Debug, Subcommand)]
pub(crate) enum Firewall {
    /// A VM's inbound rules, their revision, and whether the edge enforces them yet
    #[command(after_help = examples(&[
        "hipc compute firewall get vm-1a2b",
    ]))]
    Get {
        /// The vm_id from hipc compute vms list
        #[arg(value_parser = vm_id)]
        vm_id: String,
    },
    /// Replace every inbound rule of a VM with the rules in a file; needs --yes, and
    /// --remove-others to drop a rule or change its protocol, ports, sources, or enabled flag
    ///
    /// The file is a JSON object with a rules array, such as the document hipc compute firewall get
    /// prints, whose other fields are not sent, or a bare array of rules. A rule key the API
    /// does not list is refused. A rule without ports accepts every port, and one without
    /// source_cidrs every address. A port the rules do not accept is closed, and an empty array
    /// removes every rule. A rule the VM has now that the file leaves out, or whose protocol,
    /// ports, sources, or enabled flag it changes, is refused unless --remove-others is passed.
    /// Rules may be written before a public IP is attached. The edge applies a change within
    /// about 15 seconds.
    ///
    /// A file that opens SSH to one address, and the web to every address:
    /// {"rules": [{"protocol": "tcp", "ports": ["22"], "source_cidrs": ["203.0.113.7/32"],
    /// "description": "ssh"}, {"protocol": "tcp", "ports": ["80", "443"], "description":
    /// "web"}]}. A port is 1 to 65535 or a range such as 8000-8080, a source is an IPv4 CIDR,
    /// the protocol is tcp, udp, or icmp (which takes no ports), and enabled is true when
    /// left out.
    #[expect(
        clippy::doc_link_with_quotes,
        reason = "the help shows an example file, whose JSON arrays of strings look like links"
    )]
    #[command(after_help = examples(&[
        "hipc compute firewall update vm-1a2b --rules-file rules.json --yes",
        "hipc compute firewall update vm-1a2b --rules-file rules.json --remove-others --yes",
    ]))]
    Update(FirewallUpdate),
}

#[derive(Debug, Args)]
pub(crate) struct FirewallUpdate {
    /// The vm_id from hipc compute vms list
    #[arg(value_parser = vm_id)]
    pub(crate) vm_id: String,
    /// The JSON file with the rules, or - for stdin
    ///
    /// It is read only as a regular file, never through a symlink; /dev/stdin and <(...) are
    /// refused, so pass - for stdin.
    #[arg(long, value_name = "PATH")]
    pub(crate) rules_file: PathBuf,
    /// The revision from hipc compute firewall get: the server refuses the update with conflict
    /// when the rules changed since
    #[arg(long, value_name = "REVISION")]
    pub(crate) base_revision: Option<String>,
    /// Remove the VM's rules the file leaves out
    ///
    /// Without it the current rules are read first, and the update is refused when the file
    /// leaves out a rule the VM has now, or changes its protocol, ports, sources, or enabled
    /// flag in either direction (a missing enabled counts as true); its description may change.
    /// Without --base-revision the update carries the revision that read returned, so a change
    /// made since is refused with conflict. With --remove-others and no --base-revision, the
    /// rules are still read, after the question, for that revision.
    #[arg(long)]
    pub(crate) remove_others: bool,
}
