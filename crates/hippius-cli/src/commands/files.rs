//! New private files that `hipc` writes for the caller: the invoice PDF of `hipc billing
//! invoices get`, the kubeconfig of `hipc compute kubernetes clusters kubeconfig`, the fixed
//! compose of `hipc compute app-engine fix`, the credential files of `hipc compute databases
//! create` and `restore --credentials-dir`, and the one-time secret `--secret-file` names on
//! the S3 token and Hub key, namespace, and robot commands that issue one ([`save_secret`]).
//!
//! Each is created exclusively with mode 0600, so an existing file, a symlink included, is
//! never overwritten or followed. The path was checked at parse time to hold nothing, which
//! gives a `usage` error before any request; the exclusive create still refuses a file that
//! appeared after that check.
//!
//! The directory is held to [`private_dir`], at parse time and again on the open directory
//! when the file is created there: a directory another user can write lets them swap the file
//! for their own (a CA that a database connection trusts, a kubeconfig that names their
//! server) or delete it.

use std::fs::{File, Metadata, Permissions};
use std::io::Write as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::Path;

use hippius::{Error, SecretValue, phrase};
use rustix::fs::{AtFlags, Mode, OFlags};
use serde_json::Value;
use zeroize::Zeroizing;

/// [`write_new`] for the file `--output-file` names, after reads that changed nothing on the
/// server: a file that cannot be created is `usage`, since the caller names another path, and
/// one that cannot be written is `internal`.
pub(crate) fn write_output_file(path: &Path, bytes: &[u8]) -> hippius::Result<()> {
    write_new(path, bytes).map_err(|error| match error {
        NewFileError::Create(message) => Error::usage(
            message,
            "pass `--output-file` a new file in a directory that exists and no other user can write",
        ),
        NewFileError::Write(message) => Error::internal(message),
    })
}

/// Why [`write_new`] failed, as a message that names the file. The caller picks the code:
/// the invoice download makes a create failure `usage`, since nothing was changed and the
/// caller names another path, while the database create makes both `internal`, after a
/// write that cannot be repeated.
#[derive(Debug)]
pub(crate) enum NewFileError {
    /// The file could not be created, so nothing is at the path.
    Create(String),
    /// The file was created, and then could not be written; the message says whether it
    /// was removed.
    Write(String),
}

/// Why a directory is refused for new private files: the words that end a sentence naming it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unsafe {
    Foreign,
    Writable,
}

impl Unsafe {
    fn why(self) -> &'static str {
        match self {
            Self::Foreign => "belongs to another user",
            Self::Writable => "can be written by other users",
        }
    }

    /// The refusal of `dir` at parse time: why, and what to do. Only the owner can `chmod` a
    /// directory, so one of another user's is not told to.
    fn refusal(self, dir: &Path) -> String {
        let fix = match self {
            Self::Foreign => String::new(),
            Self::Writable => format!(
                "run `chmod go-w {}`, or ",
                phrase::shell_word(&dir.to_string_lossy())
            ),
        };
        format!(
            "the directory {} {}, who could swap or delete the file: {fix}name a directory of the \
             current user's that no one else can write",
            phrase::quote(dir.display()),
            self.why()
        )
    }
}

/// Whether files may be created in the directory `metadata` describes ([`refusal`]).
pub(crate) fn private_dir(metadata: &Metadata) -> Result<(), Unsafe> {
    let user = (
        rustix::process::geteuid().as_raw(),
        rustix::process::getegid().as_raw(),
    );
    let dir = (
        metadata.uid(),
        metadata.gid(),
        metadata.permissions().mode(),
    );
    refusal(dir, user).map_or(Ok(()), Err)
}

/// Why the directory `(owner, group, mode)` is unsafe to create a file in for the user `(euid,
/// egid)`, or `None`: the user (or root) must own it, and no other user may write it. Two kinds of
/// directory others can write pass too, since no file hipc writes there can be swapped or removed:
///
/// - a sticky one, such as `/tmp`: there only a file's owner may rename or delete it (POSIX
///   `rename`, `unlink`), and the exclusive create refuses a file someone made first;
/// - one only its group can write, when that group is the user's private group: the user's
///   primary group, with the user's own id, as `useradd` makes it (Ubuntu and Debian). With
///   their umask 002 every directory the user makes is 0775, and refusing those would refuse
///   the current directory of most users there. A primary group shared with others, such as
///   `staff` on macOS or `users`, has another id, so its write bit is refused.
///
/// ACL entries are not read: they are the caller's to keep. This is looser than the check of
/// the config directory in `hippius::private_file`, which refuses those two kinds and a
/// directory root owns.
fn refusal((owner, group, mode): (u32, u32, u32), (euid, egid): (u32, u32)) -> Option<Unsafe> {
    let private_group = group == egid && group == euid;
    let others_write = mode & 0o002 != 0 || (mode & 0o020 != 0 && !private_group);
    if owner != euid && owner != 0 {
        Some(Unsafe::Foreign)
    } else if others_write && mode & 0o1000 == 0 {
        Some(Unsafe::Writable)
    } else {
        None
    }
}

/// The parse-time refusal of a directory [`private_dir`] does not pass, as a value parser's
/// message.
pub(crate) fn private_dir_refusal(dir: &Path, metadata: &Metadata) -> Result<(), String> {
    private_dir(metadata).map_err(|refused| refused.refusal(dir))
}

/// Creates `path` with mode 0600, refusing one that exists (a symlink included), and
/// writes `bytes` to it. Its directory is opened first and held to [`private_dir`] on the
/// open descriptor, and the file is created in that descriptor (`openat`), so the directory
/// checked is the one written, whatever its path names by then. A write that returns an
/// error removes the file through the same descriptor (`unlinkat`), so a retry finds the
/// path free: in a directory that passed the check, no one else can have put another file at
/// the name. The directory is then synced, so the name lasts too, as far as its file system
/// allows. A process killed
/// during the write (hipc installs no signal handler) leaves the part already written.
pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> Result<(), NewFileError> {
    let name = phrase::quote(path.display());
    let create = |why: String| NewFileError::Create(format!("cannot create {name}: {why}"));
    let io = |e: rustix::io::Errno| phrase::io_reason(&e.into());
    // The parse-time checks refuse a path with no last name, or one that ends in `/`; an
    // empty name would only fail to open below.
    let file_name = path.file_name().unwrap_or_default();
    // A bare file name has the empty path as its parent, which is the current directory.
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let (dir, metadata) = rustix::fs::open(
        parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)
    .map(File::from)
    .and_then(|dir| dir.metadata().map(|metadata| (dir, metadata)))
    .map_err(|e| {
        create(format!(
            "its directory cannot be opened: {}",
            phrase::io_reason(&e)
        ))
    })?;
    private_dir(&metadata).map_err(|refused| create(format!("its directory {}", refused.why())))?;
    // `O_CREAT | O_EXCL` fails with `EEXIST` when the name is a symlink, a dangling one
    // included, and never follows it (POSIX `open`:
    // https://pubs.opengroup.org/onlinepubs/9799919799/functions/open.html); `O_NOFOLLOW`
    // says so twice.
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut file = rustix::fs::openat(&dir, file_name, flags, Mode::RUSR | Mode::WUSR)
        .map(File::from)
        .map_err(|e| create(io(e)))?;
    // The umask may have cleared the owner's bits of the mode passed to open; setting the mode
    // again restores `0600`. The file's bytes are synced, then its name, which lives in the
    // directory (POSIX `fsync`:
    // https://pubs.opengroup.org/onlinepubs/9799919799/functions/fsync.html), so a file that holds
    // a key that exists nowhere else lasts once hipc names it.
    file.set_permissions(Permissions::from_mode(0o600))
        .and_then(|()| file.write_all(bytes))
        .and_then(|()| file.sync_all())
        .map_err(|e| {
            let what = rustix::fs::unlinkat(&dir, file_name, AtFlags::empty())
                .map_or("and it could not be removed", |()| "so it was removed");
            NewFileError::Write(format!(
                "cannot write {name}, {what}: {}",
                phrase::io_reason(&e)
            ))
        })?;
    // Best effort: some file systems refuse `fsync` of a directory (`EINVAL` on some FUSE and
    // network mounts), and the file, whole and synced, must not be lost for that.
    drop(dir.sync_all());
    Ok(())
}

/// Moves the one-time secret of `answer`, the document of a command that issued one, to a new
/// file at `path` ([`write_new`]), and returns the document with each top-level field
/// `is_secret` names replaced by `<field>_file`, the path, and whether a file was written. The
/// file holds the secret's text alone, with no newline, which `docker login --password-stdin`
/// and `hipc s3 configure --secret-key-file` read as it is.
///
/// An answer with no secret writes nothing and is returned as it is (a Hub namespace create
/// whose namespace exists already issues none). The path was checked at parse time.
///
/// Tenet 9: a one-time secret goes on stdout, or into a new file the caller named, and
/// nowhere else, and is never lost. So every failure is `internal` after the write, with the
/// whole answer on stdout, as a database's `--credentials-dir` failure is: the file cannot be
/// written, or the answer holds a secret this cannot move whole, at any depth below the top
/// or under two top-level fields with different values. It fails closed: a document printed
/// without its secret holds no other.
pub(crate) fn save_secret(
    path: &Path,
    answer: Value,
    is_secret: &dyn Fn(&str) -> bool,
) -> hippius::Result<(Value, bool)> {
    let name = phrase::quote(path.display());
    let refused = |why: String, answer: Value| {
        Error::internal(format!("{why}; the secret is in the answer on stdout"))
            .with_hint("save the secret from stdout now, since no command shows it again")
            .after_write(answer)
    };
    // Each copy of the secret is cleared when dropped: the copy of the answer, the secrets
    // taken out of it, and, once the file holds the secret, the answer itself.
    let mut rest = SecretValue::new(answer.clone());
    let mut found = Vec::new();
    if let Value::Object(fields) = rest.as_value_mut() {
        let keys: Vec<String> = fields
            .iter()
            .filter(|(key, value)| is_secret(key) && value.is_string())
            .map(|(key, _)| key.clone())
            .collect();
        for key in keys {
            if let Some(Value::String(secret)) = fields.shift_remove(&key) {
                found.push((key, Zeroizing::new(secret)));
            }
        }
    }
    if holds_secret(rest.as_value(), is_secret) {
        return Err(refused(
            format!(
                "the answer holds a secret `hipc` cannot move to {name}, so no file was written"
            ),
            answer,
        ));
    }
    let Some((_, secret)) = found.first() else {
        return Ok((answer, false));
    };
    if found.iter().any(|(_, other)| other != secret) {
        return Err(refused(
            format!("the answer holds more than one secret, so none was written to {name}"),
            answer,
        ));
    }
    if let Err(NewFileError::Create(failed) | NewFileError::Write(failed)) =
        write_new(path, secret.as_bytes())
    {
        return Err(refused(failed, answer));
    }
    drop(SecretValue::new(answer));
    let fields = rest
        .as_value_mut()
        .as_object_mut()
        .expect("a secret was found, so the answer is an object");
    for (key, _) in found {
        fields.insert(format!("{key}_file"), path.to_string_lossy().into());
    }
    // What is left holds no secret: each was moved to the file above.
    Ok((std::mem::take(rest.as_value_mut()), true))
}

/// Where the one-time secret `what` goes, for the question of a command that issues one.
pub(crate) fn shown_once(what: &str, secret_file: Option<&Path>) -> String {
    match secret_file {
        Some(path) => format!(
            "{what} is written once to {}, and not printed",
            phrase::quote(path.display())
        ),
        None => format!("{what} is printed on stdout once"),
    }
}

/// Whether a key `is_secret` names holds anything but `null`, at any depth of `value`.
fn holds_secret(value: &Value, is_secret: &dyn Fn(&str) -> bool) -> bool {
    match value {
        Value::Object(fields) => fields.iter().any(|(key, value)| {
            (is_secret(key) && !value.is_null()) || holds_secret(value, is_secret)
        }),
        Value::Array(items) => items.iter().any(|item| holds_secret(item, is_secret)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER: (u32, u32) = (1000, 1000);

    /// The rule over every case its doc names, with the user `1000:1000`.
    #[test]
    fn a_directory_others_can_swap_files_in_is_refused() {
        for (dir, refused) in [
            ((1000, 1000, 0o700), None),
            ((1000, 1000, 0o755), None),
            ((0, 0, 0o755), None),
            ((0, 0, 0o1777), None),
            ((1000, 1000, 0o1777), None),
            // The user's private group may write it, a shared one may not.
            ((1000, 1000, 0o775), None),
            ((1000, 20, 0o775), Some(Unsafe::Writable)),
            ((1000, 100, 0o770), Some(Unsafe::Writable)),
            ((1000, 1000, 0o777), Some(Unsafe::Writable)),
            ((1000, 1000, 0o757), Some(Unsafe::Writable)),
            ((1000, 20, 0o1775), None),
            ((1001, 1000, 0o700), Some(Unsafe::Foreign)),
            ((1001, 1001, 0o1777), Some(Unsafe::Foreign)),
        ] {
            assert_eq!(refusal(dir, USER), refused, "{dir:?}");
        }
        // A user whose primary group has another id, such as `staff` on macOS, has no
        // private group.
        assert_eq!(refusal((501, 20, 0o775), (501, 20)), Some(Unsafe::Writable));
        assert_eq!(refusal((501, 20, 0o755), (501, 20)), None);
    }

    #[test]
    fn each_refusal_says_why_and_what_to_do() {
        let dir = Path::new("/srv/out");
        assert_eq!(
            Unsafe::Foreign.refusal(dir),
            "the directory `/srv/out` belongs to another user, who could swap or delete the \
             file: name a directory of the current user's that no one else can write"
        );
        assert_eq!(
            Unsafe::Writable.refusal(dir),
            "the directory `/srv/out` can be written by other users, who could swap or delete \
             the file: run `chmod go-w /srv/out`, or name a directory of the current user's that \
             no one else can write"
        );
    }
}
