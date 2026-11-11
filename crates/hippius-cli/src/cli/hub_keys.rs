//! The grammar of `hipc hub keys`, the namespace's scoped CI keys.

use clap::{Args, Subcommand};

use crate::cli::values::{Charset, Dash, catalog, word};
use crate::cli::{OneTimeSecret, examples, number};

const KEY_ID: &str = "a key id is a whole number, from `hipc hub keys list`";

#[derive(Debug, Subcommand)]
pub(crate) enum HubKeys {
    /// The namespace's CI keys; never shows a secret
    #[command(after_help = examples(&["hipc hub keys list"]))]
    List,
    /// One CI key, by its id; never shows a secret
    #[command(after_help = examples(&["hipc hub keys get 7"]))]
    Get {
        /// The key id, from hipc hub keys list
        #[arg(value_parser = number(KEY_ID))]
        key_id: u64,
    },
    /// Create a CI key with a role; needs --yes, and --reveal or --secret-file
    ///
    /// The secret is shown once, so --reveal, which prints it on stdout, or --secret-file, which
    /// writes it only to a new private file, is required before anything is sent. The key logs
    /// in to the registry as the login in the answer, with only its role's rights.
    ///
    /// The answer holds the key's id, name, role, login, and secret (secret_file in its place
    /// with --secret-file), and docker_login_command, which reads the secret on stdin. To store
    /// the secret as a CI secret: hipc hub keys create --name ci --role push --reveal --yes |
    /// jq -r .secret | gh secret set HUB_SECRET.
    #[command(after_help = examples(&[
        "hipc hub keys create --name ci --role push --reveal --yes",
        "hipc hub keys create --name ci --role push --secret-file ci.secret --yes",
        "hipc hub keys create --name pull-only --role read --expires-in 90 --reveal --yes",
        "hipc --dry-run hub keys create --name ci --role push",
    ]))]
    Create(KeyCreate),
    /// Replace the secret of a CI key; needs --yes, and --reveal or --secret-file
    ///
    /// The new secret is shown once, so --reveal, which prints it on stdout, or --secret-file,
    /// which writes it only to a new private file, is required. Every CI job that uses the old
    /// secret stops working.
    #[command(after_help = examples(&[
        "hipc hub keys rotate 7 --reveal --yes",
        "hipc hub keys rotate 7 --secret-file ci-new.secret --yes",
    ]))]
    Rotate {
        /// The key id, from hipc hub keys list
        #[arg(value_parser = number(KEY_ID))]
        key_id: u64,
        #[command(flatten)]
        secret: OneTimeSecret,
    },
    /// Delete a CI key; needs --yes
    ///
    /// Every CI job that uses its secret stops working.
    #[command(after_help = examples(&["hipc hub keys delete 7 --yes"]))]
    Delete {
        /// The key id, from hipc hub keys list
        #[arg(value_parser = number(KEY_ID))]
        key_id: u64,
    },
}

#[derive(Debug, Args)]
pub(crate) struct KeyCreate {
    /// A short name for the key, one word; the registry login ends in it
    #[arg(long, value_parser = key_name)]
    pub(crate) name: String,
    /// What the key may do in the namespace: read, push, push-delete, or admin
    ///
    /// read pulls and lists images and artifacts; push does what read does, and pushes;
    /// push-delete does what push does, and deletes; admin is the whole namespace. These are the
    /// presets the Hub client offers; the API names the role field but not its values, so any
    /// other role is sent as given for the server to judge.
    #[arg(long, value_parser = catalog("a key role", &["read", "push", "push-delete", "admin"]))]
    pub(crate) role: String,
    /// Days until the key stops working, 1 or more; without it the key does not expire
    #[arg(long, value_name = "DAYS", value_parser = clap::value_parser!(u32).range(1..))]
    pub(crate) expires_in: Option<u32>,
    #[command(flatten)]
    pub(crate) secret: OneTimeSecret,
}

/// A key name. The registry makes the login `robot$<project>+<name>` from it (the Hub
/// client's help for `--name`), so a name is one word: an empty one, or one with a space or
/// a control character, is refused here rather than by the server after the question. A
/// leading `-` is refused too: it reads as a flag on a command line. The server judges the
/// rest.
fn key_name(value: &str) -> Result<String, String> {
    word(
        value,
        1..=usize::MAX,
        Charset::OneWord,
        Dash::Refused,
        "a key name is one word, with no spaces or control characters, that does not start \
         with `-`",
    )
}
