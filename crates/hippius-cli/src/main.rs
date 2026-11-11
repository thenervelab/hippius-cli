//! The `hipc` command: the Hippius console control plane, for agents and people.

// Every other rustdoc lint, broken links included, still applies to the grammar, and these two
// to the rest of the crate.
#[expect(
    clippy::allow_attributes,
    reason = "an expect of a rustdoc lint is never fulfilled under rustc or clippy, which do \
              not run rustdoc lints"
)]
#[allow(
    rustdoc::invalid_html_tags,
    rustdoc::bare_urls,
    reason = "the doc comments on the clap grammar are the help text, written for a terminal: \
              `<name>` there is a placeholder, and a URL is printed as written"
)]
mod cli;
mod commands;
mod context;
mod hidden;
mod output;
mod run;
mod spec;
mod units;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    run::run(&args)
}
