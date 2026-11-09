//! The grammar of `hipc api`.

use std::path::PathBuf;

use clap::Args;

use crate::cli::reveal_long_help;

#[derive(Debug, Args)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each bool is one independent command-line flag, which clap parses into a field"
)]
pub(crate) struct Api {
    /// GET, HEAD, POST, PUT, PATCH, or DELETE
    pub(crate) method: String,
    /// A path such as /accounts/ (sent under /api), or a full URL on the configured API origin
    pub(crate) path: String,
    /// A JSON request body, from this file or - for stdin; POST, PUT, PATCH, and DELETE only
    ///
    /// It is read only as a regular file, never through a symlink; /dev/stdin and <(...) are
    /// refused, so pass - for stdin.
    #[arg(long, value_name = "PATH")]
    pub(crate) body_file: Option<PathBuf>,
    /// Follow every page: one JSON array, or one table in text mode; GET only
    ///
    /// The same flag, and the same output, as --all on every list command.
    #[arg(long)]
    pub(crate) all: bool,
    /// Print a secret; required for the requests that return one, or nothing is sent
    #[arg(long, long_help = reveal_long_help())]
    pub(crate) reveal: bool,
    /// Change which peers of the network can reach each other; required for POST
    /// /network/isolation/
    ///
    /// The deny_intra mode deletes the catch-all policy, so --yes alone sends nothing. Any other
    /// write method to the route needs it too.
    #[arg(long)]
    pub(crate) change_isolation: bool,
    /// Let a write publish a bucket that may be private, or open the Hub namespace; required for
    /// POST /cdn/zones/, PUT /objectstore/buckets/<name>/acl/set/, a PATCH of
    /// /cdn/zones/<zone_id>/ whose body sets origin, and a PATCH of /registry/me/publicity/
    /// unless its body sets public to false
    ///
    /// A zone makes a private bucket readable by anyone, which is known only from the answer,
    /// and a bucket policy can name any principal, so --yes alone sends nothing, as with hipc
    /// cdn zones create. A public namespace lets anyone pull its images and models without
    /// logging in, as with hipc hub namespace update --visibility public. Each route named
    /// needs it for any write method, not only the one named.
    #[arg(long)]
    pub(crate) public: bool,
    /// Let a write that replaces a whole list or document remove the entries it leaves out;
    /// required for a PUT of /compute/vms/<vm_id>/firewall/, /cdn/zones/<zone_id>/rules/,
    /// /cdn/zones/<zone_id>/waf/rules/, /cdn/zones/<zone_id>/waf/rate-limits/,
    /// /cdn/lists/<name>/items/, or /objectstore/buckets/<name>/acl/set/, for a PATCH of
    /// /accounts/members/<id>/ whose body sets grants or of /cdn/lists/<name>/ whose body sets
    /// items, and for any other write hipc guards that replaces a whole list or document
    ///
    /// hipc api does not read the list first, so without the flag it sends nothing. hipc compute
    /// firewall update, hipc cdn rules update, and hipc account members update read the list,
    /// and refuse only a write that removes an entry. Each route named needs it for any write
    /// method, not only the one named, but a POST or DELETE of /cdn/lists/<name>/items/, which
    /// adds or removes the items it names.
    #[arg(long)]
    pub(crate) remove_others: bool,
}
