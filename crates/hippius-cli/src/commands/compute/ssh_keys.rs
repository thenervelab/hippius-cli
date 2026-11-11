//! `hipc compute ssh-keys`: the stored SSH keys, and the public key file a launch installs.

use std::path::Path;

use hippius::routes::compute;
use hippius::{Error, Request, Result, phrase};
use serde_json::json;

use crate::cli::SshKeys;
use crate::commands::{Input, Printed, list, print_body, print_body_or};
use crate::context::{Context, Gate};

pub(super) fn ssh_keys(ctx: &Context<'_>, command: &SshKeys) -> Result<Printed> {
    match command {
        SshKeys::List(args) => list::run(
            ctx,
            &ctx.load_authed_client()?,
            &Request::to(&compute::SSH_KEYS, []).about("list the SSH keys"),
            args,
        ),
        SshKeys::Get { key_id } => {
            let request = Request::to(&compute::SSH_KEY, [key_id])
                .about(format!("get the SSH key `{key_id}`"))
                .listed_by("hipc compute ssh-keys list");
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        SshKeys::Create {
            name,
            public_key_file,
        } => {
            let public_key = read_public_key(public_key_file, "--public-key-file")?;
            let request =
                Request::to(&compute::SSH_KEY_CREATE, []).json(&compute::SshKeyCreateBody {
                    name,
                    public_key: &public_key,
                });
            let request = request.about(format!("store the SSH key {}", phrase::quote(name)));
            let client = ctx.load_authed_client()?;
            print_body(ctx, &client, &request, Gate::Confirm { consequence: None })
        }
        SshKeys::Delete { key_id } => {
            let request = Request::to(&compute::SSH_KEY_DELETE, [key_id])
                .listed_by("hipc compute ssh-keys list")
                .about(format!("delete the SSH key `{key_id}`"));
            let gate = Gate::Confirm {
                consequence: Some(SSH_KEY_KEPT),
            };
            let done = json!({ "id": key_id, "deleted": true });
            print_body_or(ctx, &ctx.load_authed_client()?, &request, gate, done)
        }
    }
}

/// The public key in the file `flag` names (or stdin for `-`), for `ssh-keys create` and
/// `vms create`. A private key is refused before anything is sent: the obvious mistake is
/// passing `~/.ssh/id_ed25519` instead of `~/.ssh/id_ed25519.pub`, and that would hand the
/// private key to the server. Errors name the file and the flag, and never quote what the
/// file holds. Only a line in the OpenSSH public key format ([`is_public_key`]) is sent: any
/// other one-line file, such as a token, would otherwise reach the server, whose refusal
/// echoes it on stderr. A symlink is followed: dotfile managers link `~/.ssh/*.pub`. Its
/// target must be a regular file other than the config or an AWS file, and the format check
/// refuses whatever else a link may point to.
pub(super) fn read_public_key(path: &Path, flag: &'static str) -> Result<String> {
    let input = Input {
        path,
        flag,
        what: "public key",
    };
    let text = input.read_text_following_links(MAX_PUBLIC_KEY_FILE_BYTES)?;
    let key = text.trim();
    if key.contains("PRIVATE KEY") {
        return Err(Error::usage(
            format!("{} holds a private key, which is never sent", input.name()),
            format!(
                "pass the public key file next to it to `{flag}`, such as `~/.ssh/id_ed25519.pub`"
            ),
        ));
    }
    if key.lines().count() != 1 || !is_public_key(key) {
        return Err(Error::usage(
            format!(
                "{} does not hold a public key: a public key is one line in the OpenSSH format, \
                 such as `ssh-ed25519 AAAA... user@host`",
                input.name()
            ),
            format!("pass the `.pub` file to `{flag}`, such as `~/.ssh/id_ed25519.pub`"),
        ));
    }
    Ok(key.to_owned())
}

/// The largest public key file read, in bytes: an OpenSSH RSA key of 16384 bits is under
/// 3 KiB.
const MAX_PUBLIC_KEY_FILE_BYTES: u64 = 64 * 1024;

/// The key types OpenSSH writes as the first word of a `.pub` file (`ssh-keygen -t`, and the
/// `-sk` types of a security key).
const PUBLIC_KEY_TYPES: [&str; 8] = [
    "ssh-ed25519",
    "ssh-rsa",
    "ssh-dss",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "sk-ssh-ed25519@openssh.com",
    "sk-ecdsa-sha2-nistp256@openssh.com",
];

/// Whether `line` is an OpenSSH public key: a key type of [`PUBLIC_KEY_TYPES`], then the key
/// in base64, then an optional comment.
fn is_public_key(line: &str) -> bool {
    let mut words = line.split_ascii_whitespace();
    let (Some(kind), Some(blob)) = (words.next(), words.next()) else {
        return false;
    };
    PUBLIC_KEY_TYPES.contains(&kind)
        && blob
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
}

/// What an SSH key delete leaves, for its question and `hipc api`'s.
pub(crate) const SSH_KEY_KEPT: &str = "a VM that has it keeps it";
