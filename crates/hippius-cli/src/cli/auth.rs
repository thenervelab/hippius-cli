//! The grammar of `hipc auth`.

use std::path::PathBuf;

use clap::Subcommand;

use crate::cli::examples;

#[derive(Debug, Subcommand)]
pub(crate) enum Auth {
    /// Store a console API token in the profile; needs --yes at another API origin or to
    /// replace a stored token
    ///
    /// Create the token in the console at https://console.hippius.com/dashboard/settings; an
    /// account that signs in with a 12-word access key gets its token from the access-key
    /// sign-in described at https://docs.hippius.com/use/wallet-login, and hipc never takes the
    /// 12 words. At a terminal, run hipc auth login and paste the token at the prompt, which does not show it.
    /// Otherwise put it in a file, or pipe it in with --token-file -; without --token-file, the
    /// command asks only when stdin, stdout, and stderr are terminals, the output is text, and
    /// neither CI, AI_AGENT (unless it is off), nor --no-input is set. A login sends nothing.
    ///
    /// An agent asks its user to run hipc auth login in their own terminal rather than handle
    /// the token itself. For CI, keep the token on the runner as HIPPIUS_TOKEN. The token is
    /// never read from argv, because argv is visible to other users through ps, and never from
    /// HIPPIUS_TOKEN, which is already in use for the invocation that sets it; while
    /// HIPPIUS_TOKEN is set, later commands send it, not the token stored, and the login warns
    /// so. Needs --yes when the API origin is not https://api.hippius.com, and when the profile
    /// stores a different token, since every later command then acts as the new token's owner;
    /// the dry run says which in replaces_token (without --token-file, whether any token is
    /// stored). A dry run asks for no token. At a terminal, the question about another origin
    /// comes before the prompt for the token.
    #[command(after_help = examples(&[
        "hipc auth login --token-file token.txt",
    ]))]
    Login {
        /// The file holding the token, or - for stdin; without it, a terminal is asked
        ///
        /// It is read only as a regular file, never through a symlink; /dev/stdin and <(...) are
        /// refused, so pass - for stdin.
        #[arg(long, value_name = "PATH")]
        token_file: Option<PathBuf>,
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
