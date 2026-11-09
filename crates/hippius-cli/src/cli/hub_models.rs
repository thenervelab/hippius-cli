//! The grammar of `hipc hub events` and `hipc hub models`: the namespace's audit events,
//! and the model index.

use clap::Subcommand;

use crate::cli::{ListArgs, examples, reference};

#[derive(Debug, Subcommand)]
pub(crate) enum HubEvents {
    /// The namespace's audit events, read whole
    #[command(after_help = examples(&["hipc hub events list"]))]
    List,
}

#[derive(Debug, Subcommand)]
pub(crate) enum HubModels {
    /// The model index, one page at a time: each model with its format, size, and tags
    ///
    /// Without a token it lists the models in public namespaces; with one, the account's own
    /// private ones too.
    #[command(after_help = examples(&[
        "hipc hub models list",
        "hipc hub models list --limit 20 --page-token 2",
    ]))]
    List(ListArgs),
    /// The formats, architectures, and quantizations the model index knows
    #[command(after_help = examples(&["hipc hub models formats"]))]
    Formats,
    /// One model's indexed versions, newest first, one page at a time, or one version with
    /// --reference
    ///
    /// Without --reference the versions print as a list, paged with --limit, --page-token,
    /// and --all; with it, the one version prints as sent.
    #[command(after_help = examples(&[
        "hipc hub models get my-team/llama",
        "hipc hub models get my-team/llama --all",
        "hipc hub models get my-team/llama --reference v1",
        "hipc hub models get my-team/llama --reference sha256:0a1b2c",
    ]))]
    Get {
        /// The model as namespace/repository, such as my-team/llama, from hipc hub models list
        #[arg(value_parser = model)]
        model: String,
        /// One version: a tag, such as v1, or the digest, such as sha256:0a1b2c
        #[arg(
            long,
            value_parser = reference,
            conflicts_with_all = ["limit", "page_token", "all"]
        )]
        reference: Option<String>,
        #[command(flatten)]
        page: ListArgs,
    },
}

/// A model as the index names it: the namespace (`project` in the answer), `/`, and the
/// repository (`repo`). The route takes each as one path segment, so there is exactly one `/`,
/// and each part is ASCII letters, digits, `.`, `_`, and `-`, and is not `.` or `..`, so no
/// part can change the route. The server judges the rest.
fn model(value: &str) -> Result<String, String> {
    let part_ok = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    };
    match value.split_once('/') {
        Some((namespace, repository)) if part_ok(namespace) && part_ok(repository) => {
            Ok(value.to_owned())
        }
        _ => Err(
            "a model is named namespace/repository, such as `my-team/llama`, as `hipc hub models \
             list` prints its `project` and `repo`: two parts of letters, digits, `.`, `_`, and \
             `-`, neither of them `.` or `..`"
                .to_owned(),
        ),
    }
}
