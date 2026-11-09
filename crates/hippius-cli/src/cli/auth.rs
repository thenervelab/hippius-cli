//! The grammar of `hipc auth`.

use std::path::PathBuf;

use clap::Subcommand;

use crate::cli::examples;

#[derive(Debug, Subcommand)]
pub(crate) enum Auth {
    /// Store a console API token in the profile, or log in with a 12-word access key; needs
    /// --yes at another API origin or to replace a stored token
    ///
    /// Copy the token from https://console.hippius.com/dashboard/settings, which shows it after a
    /// Google, GitHub, or Apple sign-in. At a terminal, run hipc auth login and paste the token
    /// at the prompt, which does not show it. Otherwise put it in a file, or pipe it in with
    /// --token-file -; without --token-file, the command asks only when stdin, stdout, and
    /// stderr are terminals, the output is text, and neither CI, AI_AGENT (unless it is off),
    /// nor --no-input is set. A token login sends nothing.
    ///
    /// An account that signs in with a 12-word access key has no token in the settings: run
    /// hipc auth login --access-key and type the 12 words at the prompt, which does not show
    /// them and asks under the same rules, or pass --access-key-file with a file of mode 0600
    /// (or - for stdin); a phrase of 15, 18, 21, or 24 words is read too. hipc derives the key's sr25519 address (SS58 prefix 42) and Ethereum
    /// address (m/44'/60'/0'/0/0), asks POST /api/auth/mnemonic/ for a challenge, signs it with
    /// both keys, sends the signatures to POST /api/auth/verify/, checks the token it answers
    /// with GET /api/accounts/, and stores only that token. The 12 words are never stored, sent,
    /// or printed. A 429 means too many logins or new accounts from this client address.
    /// --dry-run reads the key and prints the three requests, and signs, sends, and stores
    /// nothing; it cannot know whether the account is new, so it never shows the refusal below.
    ///
    /// A phrase the API has not seen opens a new, empty account, so one mistyped word opens an
    /// account that is not the one meant. When the verify answers is_new, the login is
    /// refused with usage and stores no token unless --new-account is passed. By then the
    /// server has already opened the empty account, so side_effects is partial; nothing is
    /// stored locally, and a later login with the same words logs in to that account without
    /// --new-account and answers is_new false, so the refusal guards one run only. A new account has no email and no free Drive plan, and S3 needs no
    /// subscription.
    ///
    /// An agent asks its user to run hipc auth login in their own terminal rather than handle
    /// the token or the 12 words itself. For CI, log in once on a machine you trust and keep
    /// only the token on the runner, as HIPPIUS_TOKEN, never the 12 words. Neither the token
    /// nor the 12 words are ever read from argv, because argv is visible to other users
    /// through ps, and the token is never read from HIPPIUS_TOKEN, which is already in use for
    /// the invocation that sets it. Needs --yes when the API origin is not
    /// https://api.hippius.com, and when the profile stores a different token, since every
    /// later command then acts as the new token's owner; the dry run says which in
    /// replaces_token (for an access key, whether any token is stored).
    #[command(after_help = examples(&[
        "hipc auth login --token-file token.txt",
        "hipc auth login --access-key-file access-key.txt",
        "hipc auth login --access-key-file access-key.txt --new-account",
    ]))]
    #[command(group(
        clap::ArgGroup::new("access_key_input").args(["access_key_file", "access_key"])
    ))]
    Login {
        /// The file holding the token, or - for stdin; without it, a terminal is asked
        ///
        /// It is read only as a regular file, never through a symlink; /dev/stdin and <(...) are
        /// refused, so pass - for stdin.
        #[arg(long, value_name = "PATH", conflicts_with_all = ["access_key_file", "access_key"])]
        token_file: Option<PathBuf>,
        /// The file holding the 12-word access key, or - for stdin
        ///
        /// A regular file of the current user at mode 0600, never through a symlink, as the
        /// config file is checked; it holds only the words.
        #[arg(long, value_name = "PATH", conflicts_with = "access_key")]
        access_key_file: Option<PathBuf>,
        /// Log in with a 12-word access key typed at a terminal prompt that does not show it
        #[arg(long)]
        access_key: bool,
        /// Store the token of an access-key login whose verify answers is_new, a new, empty
        /// account
        ///
        /// Without it, a login whose verify answers is_new is refused and stores no token,
        /// since a mistyped word also opens a new account. The server has opened the account
        /// by the time it answers, with or without this flag.
        #[arg(long, requires = "access_key_input")]
        new_account: bool,
    },
    /// Check the token against the API (GET /api/accounts/), and name the signed-in user
    ///
    /// The user comes from GET /api/users/me/. When that read fails the token check still
    /// stands: user is null, and user_error says what failed.
    #[command(after_help = examples(&[
        "hipc auth status",
    ]))]
    Status,
    /// Remove the stored token from the profile; does not revoke it on the server; needs --yes
    #[command(after_help = examples(&[
        "hipc auth logout --yes",
    ]))]
    Logout,
}
