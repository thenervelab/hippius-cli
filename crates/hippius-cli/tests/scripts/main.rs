//! Tests of the scripts in `scripts/` and of the workflows that run them: the curl
//! installer, the Homebrew formula generator, the release version check, the command
//! reference generator, the live release check, the release tarball's packing, and the
//! release workflow's guards. They run each script against local files, the built binary,
//! and a local HTTP server, with a PATH of chosen tools; nothing reaches the network.

mod brew_formula;
#[path = "../shared/dead_port.rs"]
mod dead_port;
mod docs_reference;
mod install;
mod package;
mod release_check;
mod release_version;
mod release_workflow;
mod support;
mod workflow;
