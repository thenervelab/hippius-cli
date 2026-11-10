//! A private file: the config file, an AWS file, or a file a flag names that holds a
//! secret, read and written through its directory's descriptor so nothing can swap it
//! between the check and the use. The config module's docs say what is checked and why.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{Read as _, Write as _};
use std::os::fd::OwnedFd;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use rustix::fs::{AtFlags, FileType, FlockOperation, Mode, OFlags};
use rustix::io::Errno;
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::phrase::{io_reason, quote_path};

thread_local! {
    /// What [`on_lock_wait`] set on this thread.
    static LOCK_WAIT: std::cell::RefCell<Option<LockWait>> = const { std::cell::RefCell::new(None) };
}

/// What a lock does before it waits ([`on_lock_wait`]).
type LockWait = Box<dyn Fn(&str)>;

/// Sets what a lock on this thread does before it waits for another process that holds the
/// lock of the config file or an AWS file: `note` is given a sentence that names the file,
/// such as ``waiting for another run of hipc to finish with the config file `/h/c.toml` ``.
/// `hipc s3 configure` holds its lock across its question, so without a word a second run
/// would seem to hang. The library never prints; the binary decides where the sentence goes.
/// Thread-local, so a test sees only its own.
pub fn on_lock_wait(note: impl Fn(&str) + 'static) {
    LOCK_WAIT.with(|hook| *hook.borrow_mut() = Some(Box::new(note)));
}

/// Creates `dir` and each missing directory above it, each with mode `0700`, and sets that
/// mode again on each one it created, since `mkdir` masks it with the umask: under `0277`
/// the directory would be `0500`, and no file could be created in it. A directory that
/// another process creates first is left as it is. The mode is set through a descriptor
/// opened without following a symlink, so a name swapped for a link after the create is
/// never followed; a umask that also clears the owner's read bit leaves a directory that
/// cannot be opened, and that fails as it did before.
fn create_private_dirs(dir: &Path) -> std::io::Result<()> {
    if dir.as_os_str().is_empty() || fs::symlink_metadata(dir).is_ok() {
        return Ok(());
    }
    dir.parent().map_or(Ok(()), create_private_dirs)?;
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        created => created?,
    }
    let fd = rustix::fs::open(dir, DIRECTORY_FLAGS, Mode::empty())?;
    Ok(rustix::fs::fchmod(&fd, Mode::RWXU)?)
}

/// The directory a config path lives in.
pub(crate) fn directory(path: &Path) -> &Path {
    path.parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// A private file `hipc` reads and writes, as its messages name it: what it is, what it
/// holds, and the variable or flag that names it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Guarded {
    pub(crate) what: &'static str,
    pub(crate) holds: &'static str,
    pub(crate) named_by: NamedBy,
}

/// What names a private file's path, which a hint tells the person to change.
#[derive(Debug, Clone, Copy)]
pub(crate) enum NamedBy {
    /// An environment variable, such as `HIPPIUS_CONFIG`.
    Variable(&'static str),
    /// A flag, such as `--cluster-key-file`.
    Flag(&'static str),
}

impl NamedBy {
    /// The variable or flag in backticks, as a message names it: `` `--secrets-file` ``.
    pub(crate) fn name(self) -> String {
        match self {
            Self::Variable(name) | Self::Flag(name) => format!("`{name}`"),
        }
    }

    /// The start of a hint that names another path, before the path: "set `HIPPIUS_CONFIG`
    /// to", or "pass `--cluster-key-file`".
    fn point_at(self) -> String {
        match self {
            Self::Variable(name) => format!("set `{name}` to"),
            Self::Flag(name) => format!("pass `{name}`"),
        }
    }
}
/// The flags of every directory open. `O_DIRECTORY` keeps the open from blocking on a FIFO
/// planted at the name, and `O_NOFOLLOW` refuses a symlink as the last component.
const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

/// The mode of every file `hipc` creates: read and write for the owner only.
const PRIVATE: Mode = Mode::RUSR.union(Mode::WUSR);

/// The largest private file `hipc` reads, in bytes: 1 MiB. Arbitrary: far above any config
/// or AWS file a person keeps, and small enough that a wrong path fails at once.
pub(crate) const MAX_FILE_BYTES: usize = 1 << 20;

/// The flags that create a file only when the name is free, and never through a symlink
/// planted at the name.
const CREATE_NEW: OFlags = OFlags::CREATE.union(OFlags::EXCL).union(OFlags::NOFOLLOW);

/// A private file: its directory, opened without following a symlink and checked on the
/// open descriptor, and its name in that directory. Every read, lock, create, and rename
/// goes through that descriptor, so nothing can swap the directory between the check and
/// the use, even when an ancestor directory is writable by someone else.
///
/// The directory must be a directory itself, not a symlink to one, owned by the effective
/// user, and writable by no one else: anyone who can write it could replace the file with
/// one that names their own token, API origin, or S3 endpoint. The file is opened without
/// following a symlink, and must be a regular file owned by the effective user with no
/// permission bits for the group or others, as OpenSSH requires of a private key.
///
/// The checks cover the owner and the mode bits only, not an access-control list; the
/// module docs say why.
#[derive(Debug)]
pub(crate) struct PrivateFile {
    dir: fs::File,
    /// The file's name in `dir`.
    name: OsString,
    /// The file's full path, for messages.
    path: PathBuf,
    guarded: Guarded,
}

impl PrivateFile {
    /// Opens and checks the directory of `path`, first creating it, and each missing
    /// directory above it, with mode `0700` when it is missing ([`create_private_dirs`]).
    pub(crate) fn ensure(path: &Path, guarded: Guarded) -> Result<Self> {
        let name = file_name(path, guarded)?;
        let dir = directory(path);
        let opened = match rustix::fs::open(dir, DIRECTORY_FLAGS, Mode::empty()) {
            Err(Errno::NOENT) => {
                create_private_dirs(dir)
                    .map_err(|e| file_error("create", dir, &e, Denied::Moved(guarded.named_by)))?;
                rustix::fs::open(dir, DIRECTORY_FLAGS, Mode::empty())
            }
            opened => opened,
        };
        let file = Self::opened(path, name, guarded, opened)?;
        file.check_directory()?;
        Ok(file)
    }

    /// Opens and checks the directory of `path` as [`PrivateFile::ensure`] does, or `None`
    /// when it is missing. Creates nothing, so a dry run can refuse what a write would.
    pub(crate) fn existing(path: &Path, guarded: Guarded) -> Result<Option<Self>> {
        let name = file_name(path, guarded)?;
        let file = match rustix::fs::open(directory(path), DIRECTORY_FLAGS, Mode::empty()) {
            Err(Errno::NOENT) => return Ok(None),
            opened => Self::opened(path, name, guarded, opened)?,
        };
        file.check_directory()?;
        Ok(Some(file))
    }

    /// Reads the file at `path`, or `None` when it or its directory does not exist. Creates
    /// nothing.
    ///
    /// The file is checked before its directory, so that when both would be refused, the
    /// message names the file. Both are checked before the bytes are returned. A missing
    /// file is not refused for its directory: there is nothing to trust yet, and the write
    /// that would create it goes through [`PrivateFile::ensure`], which checks it.
    pub(crate) fn load(path: &Path, guarded: Guarded) -> Result<Option<Vec<u8>>> {
        let name = file_name(path, guarded)?;
        let file = match rustix::fs::open(directory(path), DIRECTORY_FLAGS, Mode::empty()) {
            Err(Errno::NOENT) => return Ok(None),
            opened => Self::opened(path, name, guarded, opened)?,
        };
        let Some(bytes) = file.read()? else {
            return Ok(None);
        };
        // A refused directory drops the bytes cleared.
        let mut bytes = Zeroizing::new(bytes);
        file.check_directory()?;
        Ok(Some(std::mem::take(&mut *bytes)))
    }

    /// The file's full path.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Takes the exclusive lock for the file: `flock` on `.<name>.lock` in the directory,
    /// created with mode `0600` and opened without following a symlink. Waits while another
    /// process holds it, after telling the hook [`on_lock_wait`] set. The lock ends when the
    /// returned descriptor is dropped, or when the process exits.
    pub(crate) fn lock(&self) -> Result<OwnedFd> {
        let mut locks = Self::lock_all(&[self])?;
        Ok(locks.pop().expect("one file, one lock"))
    }

    /// [`PrivateFile::lock`] of each of `files`, all held together. Every lock file is opened
    /// first, and they are locked in the order of their device and inode, so two runs that
    /// lock the same files never each hold one and wait for the other. Two names whose lock is
    /// one file (the same name, or two spellings a case-insensitive volume or Unicode
    /// normalization makes one) are locked once: a second `flock` of the file through another
    /// descriptor would wait for this process forever.
    pub(crate) fn lock_all(files: &[&Self]) -> Result<Vec<OwnedFd>> {
        // Each lock file's device and inode, its descriptor, and its file.
        let mut opened = Vec::new();
        for &file in files {
            let fd = file.open_lock()?;
            // Without its identity a lock file could be locked twice, or out of order, so a
            // lock file that cannot be inspected is refused.
            let stat = rustix::fs::fstat(&fd).map_err(file.lock_error())?;
            let id = (stat.st_dev, stat.st_ino);
            if opened.iter().all(|(other, _, _)| *other != id) {
                opened.push((id, fd, file));
            }
        }
        opened.sort_by_key(|(id, ..)| *id);
        let mut locks = Vec::with_capacity(opened.len());
        for (_, fd, file) in opened {
            locks.push(file.flock(fd)?);
        }
        for file in files {
            file.remove_stale_temporaries();
        }
        Ok(locks)
    }

    /// Opens the file's lock file, `.<name>.lock`, creating it with mode `0600` (less under a
    /// strict umask, which does no harm: it is opened read-only).
    fn open_lock(&self) -> Result<OwnedFd> {
        let name = self.sibling("lock");
        // Read-only: `flock` needs no write access, so a lock file an older run left `0400`
        // under a strict umask still opens.
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        // Created with `O_EXCL`, then opened without `O_CREAT` when it exists. On macOS
        // (Darwin 24, APFS), `openat` with `O_CREAT` and without `O_EXCL` can fail with
        // `ENOENT` when another thread or process creates the same name at the same moment.
        // Nothing removes a lock file, so the open after `EEXIST` finds it.
        let opened = match rustix::fs::openat(&self.dir, &name, flags | CREATE_NEW, PRIVATE) {
            Err(Errno::EXIST) => rustix::fs::openat(&self.dir, &name, flags, Mode::empty()),
            opened => opened,
        };
        opened.map_err(self.lock_error())
    }

    /// Takes the exclusive `flock` of `fd`, the file's lock file. When another process holds
    /// it, the hook [`on_lock_wait`] set is told before the wait, so a run that waits for
    /// another one's question does not seem to hang.
    fn flock(&self, fd: OwnedFd) -> Result<OwnedFd> {
        let locked = match rustix::fs::flock(&fd, FlockOperation::NonBlockingLockExclusive) {
            Err(Errno::WOULDBLOCK) => {
                let Guarded { what, .. } = self.guarded;
                let waiting = format!(
                    "waiting for another run of hipc to finish with {what} {}",
                    quote_path(&self.path)
                );
                LOCK_WAIT.with(|hook| hook.borrow().as_ref().map(|hook| hook(&waiting)));
                rustix::fs::flock(&fd, FlockOperation::LockExclusive)
            }
            locked => locked,
        };
        locked.map(|()| fd).map_err(self.lock_error())
    }

    /// The error of a lock file that cannot be opened, inspected, or locked.
    fn lock_error(&self) -> impl Fn(Errno) -> Error + '_ {
        |error| {
            let dir = directory(&self.path);
            file_error(
                "lock",
                &dir.join(self.sibling("lock")),
                &error.into(),
                Denied::Chmod("700", dir),
            )
        }
    }

    /// Creates the file empty with mode `0600` when it is missing, never through a symlink,
    /// and returns whether it did. The mode is set again once it is created, since `open`
    /// masks it with the umask: under `0277` the file would be `0400`, and every later write,
    /// which keeps the owner bits a file has, would keep it so.
    pub(crate) fn create_if_missing(&self) -> Result<bool> {
        let flags = OFlags::WRONLY | CREATE_NEW;
        match rustix::fs::openat(&self.dir, &self.name, flags | OFlags::CLOEXEC, PRIVATE) {
            // Best effort: the file exists now, and the caller must know it made it.
            Ok(fd) => {
                let _best_effort = rustix::fs::fchmod(&fd, PRIVATE);
                Ok(true)
            }
            // It exists, or it is a symlink, which the read refuses with its own message.
            Err(Errno::EXIST) => Ok(false),
            Err(e) => Err(file_error(
                "create",
                &self.path,
                &e.into(),
                Denied::Chmod("700", directory(&self.path)),
            )),
        }
    }

    /// Removes the file. Best effort: the caller is already reporting another failure.
    pub(crate) fn remove(&self) {
        self.unlink(&self.name);
    }

    /// Writes `bytes` to the file and replaces the old file in one rename. A new file gets
    /// mode `0600`; a file that exists keeps its owner bits (a file the user made `0400`
    /// stays `0400`), and never gets a bit for the group or others, because `hipc` does not
    /// change a mode behind the user's back (tenet 8).
    ///
    /// The temporary file is created with `O_CREAT | O_EXCL | O_NOFOLLOW` in the same
    /// directory, so the write cannot follow a symlink someone planted at that name, and
    /// the rename stays on one file system. The data and then the directory are synced
    /// before success is reported, so a crash leaves the old file or the new one, never a
    /// mix.
    pub(crate) fn write(&self, bytes: &[u8]) -> Result<()> {
        let temporary = self.sibling(&format!("{}.tmp", std::process::id()));
        // A file left by a crashed process with this pid is ours to replace.
        self.unlink(&temporary);
        if let Err(e) = self.replace(&temporary, bytes, self.owner_mode()) {
            self.unlink(&temporary);
            // The directory sync is the last step, so a failure there leaves the new file
            // in place but not yet durable. "write or sync" covers both.
            let dir = directory(&self.path);
            return Err(file_error(
                "write or sync",
                &self.path,
                &e,
                Denied::Chmod("700", dir),
            ));
        }
        Ok(())
    }

    /// Removes the temporary files that writers killed before their rename left beside the
    /// file, `.<name>.<pid>.tmp`, each a full copy of it: of the config, every token. A later
    /// run has another pid, so [`PrivateFile::write`] would never replace them.
    ///
    /// [`PrivateFile::lock`] runs it once it holds the lock: every writer takes the lock
    /// before it creates its temporary file and keeps it past the rename, so while the lock
    /// is held, no such file is in use. Best effort, as [`PrivateFile::unlink`] is: a
    /// directory that cannot be listed leaves them, and the caller goes on.
    fn remove_stale_temporaries(&self) {
        let prefix = format!(".{}.", self.name.to_string_lossy());
        for entry in rustix::fs::Dir::read_from(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = entry.file_name().to_string_lossy();
            let pid = name
                .strip_prefix(&prefix)
                .and_then(|rest| rest.strip_suffix(".tmp"));
            if pid.is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit())) {
                self.unlink(entry.file_name());
            }
        }
    }

    /// Takes the directory that `opened` holds, the directory of `path`, without checking
    /// its owner or mode yet.
    fn opened(
        path: &Path,
        name: &OsStr,
        guarded: Guarded,
        opened: rustix::io::Result<OwnedFd>,
    ) -> Result<Self> {
        let dir = directory(path);
        let fd = match opened {
            Ok(fd) => fd,
            Err(Errno::LOOP | Errno::NOTDIR) => return Err(not_a_directory(dir, guarded)),
            Err(e) => return Err(file_error("open", dir, &e.into(), Denied::Enter(dir))),
        };
        Ok(Self {
            dir: fs::File::from(fd),
            name: name.to_owned(),
            path: path.to_owned(),
            guarded,
        })
    }

    /// Refuses a directory that another user owns, or that someone other than its owner
    /// can write.
    fn check_directory(&self) -> Result<()> {
        let Guarded { what, named_by, .. } = self.guarded;
        let dir = directory(&self.path);
        let metadata = self
            .dir
            .metadata()
            .map_err(|e| file_error("read", dir, &e, Denied::Enter(dir)))?;
        if metadata.uid() != rustix::process::geteuid().as_raw() {
            return Err(Error::usage(
                format!(
                    "the directory {} belongs to another user, so {what} in it cannot be trusted",
                    quote_path(dir)
                ),
                format!(
                    "{} a file in a directory of the current user",
                    named_by.point_at()
                ),
            ));
        }
        if metadata.permissions().mode() & 0o022 != 0 {
            return Err(Error::usage(
                format!(
                    "the directory {} can be written by other users, so {what} in it cannot be \
                     trusted",
                    quote_path(dir)
                ),
                format!(
                    "run `chmod 700 {}`",
                    crate::phrase::shell_word(&dir.to_string_lossy())
                ),
            ));
        }
        Ok(())
    }

    /// Opens the file without following a symlink, checks it on the open descriptor, and
    /// reads it, or `None` when it does not exist. [`PrivateFile::ensure`] has checked the
    /// directory already; [`PrivateFile::load`] checks it after this.
    pub(crate) fn read(&self) -> Result<Option<Vec<u8>>> {
        let Guarded {
            what,
            holds,
            named_by,
        } = self.guarded;
        let path = &self.path;
        // `O_NONBLOCK`: opening a FIFO planted at the name returns at once instead of
        // waiting for a writer, so the check below refuses it. A regular file ignores it.
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        let fd = match rustix::fs::openat(&self.dir, &self.name, flags, Mode::empty()) {
            Ok(fd) => fd,
            Err(Errno::NOENT) => return Ok(None),
            Err(Errno::LOOP) => {
                return Err(Error::usage(
                    format!(
                        "{what} {} is a symlink, and a symlink is never followed",
                        quote_path(path)
                    ),
                    format!("{} the file the symlink points to", named_by.point_at()),
                ));
            }
            Err(e) => {
                // The open is refused before the owner check below can run, so the owner is
                // read from the directory entry here.
                let owner = rustix::fs::statat(&self.dir, &self.name, AtFlags::SYMLINK_NOFOLLOW)
                    .ok()
                    .map(|stat| stat.st_uid);
                let user = rustix::process::geteuid().as_raw();
                return Err(open_error(path, &e.into(), owner, user, self.guarded));
            }
        };
        let file = fs::File::from(fd);
        let metadata = file
            .metadata()
            .map_err(|e| file_error("read", path, &e, Denied::Chmod("600", path)))?;
        if !metadata.is_file() {
            return Err(Error::usage(
                format!("{what} {} is not a regular file", quote_path(path)),
                format!("{} a regular file", named_by.point_at()),
            ));
        }
        if metadata.uid() != rustix::process::geteuid().as_raw() {
            return Err(another_users_file(path, self.guarded));
        }
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(Error::usage(
                format!(
                    "{what} {} has permission bits for the group or others, and it holds {holds}",
                    quote_path(path)
                ),
                format!(
                    "run `chmod 600 {}`",
                    crate::phrase::shell_word(&path.to_string_lossy())
                ),
            ));
        }
        // Sized from `fstat` with a byte to spare, so `read_to_end` sees the end without
        // growing the buffer: a growth would free a copy of the bytes that nothing clears.
        // That relies on std's `read_to_end` filling spare capacity before it reallocates,
        // which it does today but its docs do not promise, and on the file not growing
        // between the `fstat` and the read. Cleared on every refusal below; on success the
        // caller owns the bytes, and every caller clears them in turn: the config and AWS
        // readers, and the DNS credentials, app secrets, and cluster key files.
        let capacity = usize::try_from(metadata.len())
            .map_or(MAX_FILE_BYTES, |len| len.min(MAX_FILE_BYTES))
            + 1;
        let mut bytes = Zeroizing::new(Vec::with_capacity(capacity));
        file.take(MAX_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| file_error("read", path, &e, Denied::Chmod("600", path)))?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(Error::usage(
                format!("{what} {} is larger than 1 MiB", quote_path(path)),
                format!(
                    "check that {} is {what}, or {} another file",
                    quote_path(path),
                    named_by.point_at()
                ),
            ));
        }
        Ok(Some(std::mem::take(&mut *bytes)))
    }

    /// Whether the file at the name has another hard link now, never through a symlink:
    /// `false` when there is no file there.
    pub(crate) fn has_other_links(&self) -> bool {
        rustix::fs::statat(&self.dir, &self.name, AtFlags::SYMLINK_NOFOLLOW)
            .is_ok_and(|stat| stat.st_nlink > 1)
    }

    /// The mode a write gives the file: the owner bits of the regular file at the name now,
    /// or `0600` when there is none. The directory is checked, so only its owner can put a
    /// file there between this and the rename.
    fn owner_mode(&self) -> Mode {
        match rustix::fs::statat(&self.dir, &self.name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile => {
                Mode::from_raw_mode(stat.st_mode) & Mode::RWXU
            }
            _ => PRIVATE,
        }
    }

    /// Creates `temporary` with `mode`, writes and syncs `bytes`, renames it over the file,
    /// and syncs the directory.
    fn replace(&self, temporary: &str, bytes: &[u8], mode: Mode) -> std::io::Result<()> {
        let flags = OFlags::WRONLY | CREATE_NEW;
        let fd = rustix::fs::openat(&self.dir, temporary, flags | OFlags::CLOEXEC, PRIVATE)?;
        // The mode passed to open is masked by the umask. Setting it again makes it exact.
        // The descriptor was opened for writing, so a mode without the owner's write bit
        // still lets the write below go through.
        rustix::fs::fchmod(&fd, mode)?;
        let mut file = fs::File::from(fd);
        file.write_all(bytes)?;
        file.sync_all()?;
        rustix::fs::renameat(&self.dir, temporary, &self.dir, &self.name)?;
        self.dir.sync_all()
    }

    /// Removes `name` from the directory, never through a symlink (`unlinkat` removes the
    /// link itself). Every caller removes a file whose absence is the goal or that is
    /// already part of a failure being reported, so a failure here is not reported.
    fn unlink(&self, name: impl rustix::path::Arg) {
        let _best_effort = rustix::fs::unlinkat(&self.dir, name, AtFlags::empty());
    }

    /// The name `.<name>.<suffix>` beside the file, for its temporary file and its lock.
    fn sibling(&self, suffix: &str) -> String {
        format!(".{}.{suffix}", self.name.to_string_lossy())
    }
}

/// The name of the file `path` names, or `usage` for a path that names none (`/`, `.`, or a
/// path ending in `..`).
fn file_name(path: &Path, guarded: Guarded) -> Result<&OsStr> {
    let Guarded { what, named_by, .. } = guarded;
    path.file_name().ok_or_else(|| {
        Error::usage(
            format!("the path {} names no file for {what}", quote_path(path)),
            format!("{} a file path", named_by.point_at()),
        )
    })
}

/// The refusal of a directory that `O_DIRECTORY | O_NOFOLLOW` would not open: a symlink, or
/// not a directory at all.
///
/// Linux reports a symlink as `ENOTDIR` and macOS as `ELOOP`, so the errno cannot tell the
/// two apart, and `lstat` chooses the message. The refusal does not depend on it.
fn not_a_directory(dir: &Path, guarded: Guarded) -> Error {
    let Guarded { what, named_by, .. } = guarded;
    if fs::symlink_metadata(dir).is_ok_and(|m| m.is_symlink()) {
        return Error::usage(
            format!(
                "the directory {} is a symlink, and the directory of {what} is never reached \
                 through one",
                quote_path(dir)
            ),
            format!(
                "{} a file in the directory the symlink points to",
                named_by.point_at()
            ),
        );
    }
    Error::usage(
        format!(
            "{} is not a directory, so it cannot hold {what}",
            quote_path(dir)
        ),
        format!("{} a file in a directory", named_by.point_at()),
    )
}

/// The refusal of a private file that belongs to another user.
fn another_users_file(path: &Path, guarded: Guarded) -> Error {
    let Guarded {
        what,
        holds,
        named_by,
    } = guarded;
    Error::usage(
        format!(
            "{what} {} belongs to another user, and it holds {holds}",
            quote_path(path)
        ),
        format!("{} a file of the current user", named_by.point_at()),
    )
}

/// The error for a private file that could not be opened for reading: `owner` is its
/// owner's uid, when its directory entry could be read, and `user` the effective uid. A
/// denied open of another user's file is refused for its owner, as one that opens is, since a
/// `chmod` would not fix it; any other failure is [`file_error`]'s.
fn open_error(
    path: &Path,
    error: &std::io::Error,
    owner: Option<u32>,
    user: u32,
    guarded: Guarded,
) -> Error {
    let denied = error.kind() == std::io::ErrorKind::PermissionDenied;
    if denied && owner.is_some_and(|owner| owner != user) {
        return another_users_file(path, guarded);
    }
    file_error("read", path, error, Denied::Chmod("600", path))
}

/// What a person changes when the system denies `hipc` a file operation (see
/// [`file_error`]).
#[derive(Debug, Clone, Copy)]
enum Denied<'a> {
    /// The mode of a file or a directory of the current user: `chmod <mode> <path>`.
    Chmod(&'static str, &'a Path),
    /// A directory the current user may not enter, or one above it.
    Enter(&'a Path),
    /// A directory `hipc` cannot create: the variable or flag that moves the file elsewhere.
    Moved(NamedBy),
}

/// `action` on `path` failed with `error`, said as [`io_reason`] says it. A denied
/// permission is `usage`, with the fix `denied` names: the person changes a mode or a path,
/// and it is not a bug to report (tenet 4). Anything else is `internal`.
fn file_error(action: &str, path: &Path, error: &std::io::Error, denied: Denied<'_>) -> Error {
    let message = format!("cannot {action} {}: {}", quote_path(path), io_reason(error));
    if error.kind() != std::io::ErrorKind::PermissionDenied {
        return Error::internal(message);
    }
    let hint = match denied {
        Denied::Chmod(mode, path) => format!(
            "run `chmod {mode} {}`",
            crate::phrase::shell_word(&path.to_string_lossy())
        ),
        Denied::Enter(dir) => format!(
            "check that the current user can enter {} and each directory above it",
            quote_path(dir)
        ),
        Denied::Moved(named_by) => format!(
            "{} a file in a directory the current user can write",
            named_by.point_at()
        ),
    };
    Error::usage(message, hint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CONFIG_FILE;

    /// A file the current user cannot open is refused for its owner when another user owns
    /// it, since a `chmod` would not fix it, and gets the `chmod` when the current user owns
    /// it or its owner is unknown. The uids are the test's own, so this holds on every host,
    /// as root too: a test cannot create a file of another user.
    #[test]
    fn a_denied_open_names_the_fix_its_owner_allows() {
        let path = Path::new("/h/config.toml");
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let error = open_error(path, &denied, Some(0), 501, CONFIG_FILE);
        assert_eq!(error.code(), crate::Code::Usage);
        assert_eq!(
            error.to_string(),
            "the config file `/h/config.toml` belongs to another user, and it holds API tokens"
        );
        for owner in [Some(501), None] {
            let error = open_error(path, &denied, owner, 501, CONFIG_FILE);
            assert_eq!(
                error.hint(),
                Some("run `chmod 600 /h/config.toml`"),
                "{owner:?}"
            );
        }
        // Another failure is not about permissions, whoever owns the file.
        let other = std::io::Error::other("disk");
        let error = open_error(path, &other, Some(0), 501, CONFIG_FILE);
        assert_eq!(error.code(), crate::Code::Internal);
    }
}
