//! Tests of the scripts in `scripts/`: the curl installer, the Homebrew formula generator,
//! the `PyPI` version check, the command reference generator, the live release check's
//! refusals, the release tarball's packing, and the release workflow's guards. They run each script against
//! local files, the built binary, and a local HTTP server, with a PATH of chosen tools;
//! nothing reaches the network.

mod brew_formula;
mod docs_reference;
mod install;
mod package;
mod pypi_version;
mod release_check;
mod release_workflow;
mod support;
