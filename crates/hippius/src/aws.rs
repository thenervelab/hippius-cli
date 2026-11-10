//! `hipc s3 configure`: an AWS CLI profile that points at Hippius S3.
//!
//! Object bytes never pass through `hipc`. This module writes the profile that lets the AWS
//! CLI, boto3, or rclone reach `https://s3.hippius.com` instead: the endpoint, the region
//! `decentralized`, path-style addressing, and the key pair.
//!
//! The two files are INI. One section is replaced, and every other byte of both files is
//! kept. The profile name, the key id, and the secret are checked before they are written,
//! so none of them can close the section and start another, or add a line.

use std::fmt;
use std::fs;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::phrase::{quote, quote_path};
use crate::private_file::{Guarded, NamedBy, PrivateFile};
use crate::source::Source;

/// The Hippius S3 endpoint.
const ENDPOINT: &str = "https://s3.hippius.com";

/// The region every Hippius bucket reports.
const REGION: &str = "decentralized";

/// The variable that moves the AWS config file, as the AWS CLI reads it.
const AWS_CONFIG_FILE: &str = "AWS_CONFIG_FILE";

/// The variable that moves the AWS credentials file, as the AWS CLI reads it.
const AWS_SHARED_CREDENTIALS_FILE: &str = "AWS_SHARED_CREDENTIALS_FILE";

/// The AWS config file. It is held to the checks of the credentials file: someone who can
/// write it can point the endpoint at their own host, and the write that replaces it keeps only
/// the owner's bits, so it would otherwise narrow a wider mode behind the user's back.
const AWS_CONFIG: Guarded = Guarded {
    what: "the AWS config file",
    holds: "the S3 endpoint",
    named_by: NamedBy::Variable(AWS_CONFIG_FILE),
};

/// The AWS credentials file.
const AWS_CREDENTIALS: Guarded = Guarded {
    what: "the AWS credentials file",
    holds: "S3 keys",
    named_by: NamedBy::Variable(AWS_SHARED_CREDENTIALS_FILE),
};

/// The AWS CLI's two files.
#[derive(Debug)]
pub struct AwsFiles {
    /// `AWS_CONFIG_FILE`, else `~/.aws/config`.
    pub config: PathBuf,
    /// `AWS_SHARED_CREDENTIALS_FILE`, else `~/.aws/credentials`.
    pub credentials: PathBuf,
}

impl AwsFiles {
    /// The paths the AWS CLI reads: its two variables, else `~/.aws/`. The caller passes
    /// the three variables as the environment holds them: `None` for a variable that is
    /// unset, and `Some("")` for one that is set but empty, which is refused, so the caller
    /// must not filter empty values out of the two AWS variables.
    ///
    /// # Errors
    ///
    /// `usage` when a path needs `HOME` and it is not set, and when a variable is set but
    /// empty, holds a `$`, or starts with `~` (`literal`).
    pub fn locate(
        config: Option<PathBuf>,
        credentials: Option<PathBuf>,
        home: Option<&Path>,
    ) -> Result<Self> {
        let config = literal(config, AWS_CONFIG_FILE)?;
        let credentials = literal(credentials, AWS_SHARED_CREDENTIALS_FILE)?;
        let aws = || {
            home.filter(|h| !h.as_os_str().is_empty())
                .map(|h| h.join(".aws"))
                .ok_or_else(|| {
                    Error::usage(
                        "`HOME` is not set, so the AWS files have no default path",
                        "set `AWS_CONFIG_FILE` and `AWS_SHARED_CREDENTIALS_FILE` to the AWS \
                             file paths",
                    )
                })
        };
        let config = match config {
            Some(path) => path,
            None => aws()?.join("config"),
        };
        let credentials = match credentials {
            Some(path) => path,
            None => aws()?.join("credentials"),
        };
        Ok(Self {
            config,
            credentials,
        })
    }

    /// The refusals [`configure`] makes before it writes, for a dry run, which must refuse
    /// what a real run would (tenet 5): a directory that is a symlink, belongs to another
    /// user, or others can write; a file that is a symlink, not a regular file, another
    /// user's, open to the group or others, larger than 1 MiB, or not UTF-8; and both
    /// variables naming one file. Creates nothing, locks nothing, and asks nothing, so a file
    /// or directory that is missing passes, as the real run creates it.
    ///
    /// # Errors
    ///
    /// The `usage` and `internal` errors of [`configure`], but for what `confirm` returns.
    pub fn check(&self) -> Result<()> {
        let config = PrivateFile::existing(&self.config, AWS_CONFIG)?;
        let credentials = PrivateFile::existing(&self.credentials, AWS_CREDENTIALS)?;
        refuse_one_file(self)?;
        for file in [config, credentials].iter().flatten() {
            read(file)?;
        }
        Ok(())
    }
}

/// `path`, the value of `variable`, when the AWS CLI reads it as written.
///
/// The AWS CLI runs both variables through `os.path.expandvars` and then
/// `os.path.expanduser` (botocore 1.35.0, `configloader.py` lines 142-143, in
/// `raw_config_parse`, which the config and the shared-credentials loaders both call).
/// `hipc` refuses a value those could change instead of copying them: a `$` anywhere, or a
/// `~` at the start. Python's rules for which `$NAME` it expands, and for `~user`, would be
/// a second implementation to keep in step, and a value either refusal misses would put the
/// secret where the AWS CLI does not look, such as `./~/.aws/credentials`. A refused value
/// that Python would leave alone (a `$` before a name that is not set) costs the caller one
/// edit. A `~` later in the path is a plain character to both.
///
/// A variable set to `""` is refused too, not read as unset. botocore 1.35.0's
/// `EnvironmentProvider.provide` (`configprovider.py` lines 797-801) returns the value
/// whenever the name is in the environment, the empty string included. `raw_config_parse`
/// then finds no file at `""` and raises `ConfigNotFound` (`configloader.py` lines 144-145),
/// which the session reads as no profiles (`session.py` lines 440-441), with no fall back to
/// `~/.aws/`. So a profile `hipc` wrote to the default path would not be read.
///
/// # Errors
///
/// `usage`, naming the variable.
fn literal(path: Option<PathBuf>, variable: &str) -> Result<Option<PathBuf>> {
    let Some(path) = path else {
        return Ok(None);
    };
    if path.as_os_str().is_empty() {
        return Err(Error::usage(
            format!(
                "`{variable}` is set but empty, which the AWS CLI reads as a path to no file, \
                 so `hipc` would write a file the AWS CLI does not read"
            ),
            format!("unset `{variable}`, or set it to the file's path"),
        ));
    }
    let bytes = path.as_os_str().as_encoded_bytes();
    if bytes.starts_with(b"~") || bytes.contains(&b'$') {
        return Err(Error::usage(
            format!(
                "`{variable}` is {}, which the AWS CLI expands and `hipc` does not, so `hipc` \
                 would write a file the AWS CLI does not read",
                quote_path(&path)
            ),
            format!("set `{variable}` to the path with `~` and every `$` variable written out"),
        ));
    }
    Ok(Some(path))
}

/// An AWS profile name. Letters, digits, `.`, `_`, and `-`, at most 64 characters.
#[derive(Debug)]
pub struct AwsProfile(String);

impl AwsProfile {
    /// Checks a profile name.
    ///
    /// # Errors
    ///
    /// `usage` for an empty, too long, or otherwise unsafe name. A `]` or a newline would
    /// let the name end its section header and start another.
    pub fn new(name: &str) -> Result<Self> {
        if crate::config::is_new_profile_name(name) {
            Ok(Self(name.to_owned()))
        } else {
            Err(Error::usage(
                format!(
                    "{} is not an AWS profile name: a name is 1 to 64 letters, digits, `.`, \
                     `_`, and `-`",
                    quote(name)
                ),
                "pass an `--aws-profile` of 1 to 64 letters, digits, `.`, `_`, and `-`",
            ))
        }
    }

    /// The section name in the config file: `default`, or `profile <name>`.
    fn config_name(&self) -> String {
        if self.0 == "default" {
            "default".to_owned()
        } else {
            format!("profile {}", self.0)
        }
    }

    fn config_header(&self) -> String {
        format!("[{}]", self.config_name())
    }

    fn credentials_header(&self) -> String {
        format!("[{}]", self.0)
    }

    /// The name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A Hippius S3 access key id: `hip_` and then letters and digits.
#[derive(Debug)]
pub struct AccessKeyId(String);

impl AccessKeyId {
    /// Checks an access key id.
    ///
    /// # Errors
    ///
    /// `usage` when it does not start with `hip_`, or holds anything but letters and digits
    /// after it.
    pub fn new(text: &str) -> Result<Self> {
        let valid = text.strip_prefix("hip_").is_some_and(|rest| {
            !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric())
        });
        if valid {
            Ok(Self(text.to_owned()))
        } else {
            Err(Error::usage(
                format!(
                    "{} is not an S3 access key id: an id is `hip_` and letters or digits",
                    quote(text)
                ),
                "run `hipc s3 tokens list` to see the access key ids",
            ))
        }
    }
}

/// An S3 secret key, cleared when dropped. `Debug` never prints it.
pub struct SecretKey(Zeroizing<String>);

impl SecretKey {
    /// Wraps a secret read from `source`, a file or stdin. Surrounding whitespace is trimmed.
    ///
    /// # Errors
    ///
    /// `usage` when it is empty or holds whitespace or a control character, which would end
    /// the `aws_secret_access_key` line. The message names the source and never quotes it.
    pub fn new(text: &str, source: &Source) -> Result<Self> {
        let secret = text.trim();
        let from = source.suffix();
        if secret.is_empty() {
            return Err(Error::usage(
                format!("the secret key{from} is empty"),
                "write the secret key into the file `--secret-key-file` names",
            ));
        }
        if secret.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(Error::usage(
                format!("the secret key{from} holds whitespace or a control character"),
                "leave only the secret key in the file `--secret-key-file` names: one word",
            ));
        }
        Ok(Self(Zeroizing::new(secret.to_owned())))
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretKey(redacted)")
    }
}

/// Writes the profile into both files, replacing any section of the same name and keeping
/// everything else, and asks `confirm` first when either file already has that profile.
///
/// The read, the question, and the write happen under the locks on both files, so a
/// concurrent `hipc s3 configure` that shares either file cannot be lost, and the answer is
/// about the files as they are written. Each lock is `flock` on `.<name>.lock` beside its
/// file, created on the first run and never removed (`PrivateFile::lock_all`). A new file gets
/// mode `0600` and a file
/// that exists keeps its owner bits (`PrivateFile::write`), through a temporary file and a
/// rename. Both directories are checked, since
/// `AWS_CONFIG_FILE` can name a different one than the credentials file, and someone who can
/// replace the config file can point the endpoint at their own host.
///
/// The config file is written first. If the credentials write then fails, a new profile
/// names an endpoint but no key, which fails loudly rather than reaching AWS; a profile
/// that already existed keeps its old key, now aimed at the Hippius endpoint. `SigV4` signs
/// requests and never sends the secret, so that leaks the old key's id, not the key.
///
/// # Errors
///
/// What `confirm` returns. `usage` for a file or a directory that is a symlink, a file that
/// is not UTF-8, belongs to another user, or has a permission bit for the group or others,
/// a directory others can write, or when `AWS_CONFIG_FILE` and
/// `AWS_SHARED_CREDENTIALS_FILE` name one file, and when the system denies a read or a
/// write. `internal` when a file cannot be read or written for another reason.
pub fn configure(
    files: &AwsFiles,
    profile: &AwsProfile,
    access_key_id: &AccessKeyId,
    secret: &SecretKey,
    confirm: impl FnOnce(&str) -> Result<()>,
) -> Result<()> {
    let config_file = PrivateFile::ensure(&files.config, AWS_CONFIG)?;
    let credentials_file = PrivateFile::ensure(&files.credentials, AWS_CREDENTIALS)?;
    // Both texts are read before either is written, so one file named by both variables
    // would be written twice, and the second write would drop the endpoint section. Files
    // that exist are compared now, before any question; a missing one only once the write
    // is certain, so declining the question creates neither file. It does not leave the
    // disk as it was: `ensure` above has already created a missing directory, and the lock
    // below create `.<config name>.lock` and `.<credentials name>.lock` beside the two
    // files, which stay, as every lock file does. A refusal at that later point leaves the empty file it created.
    refuse_one_file(files)?;
    // Both files are locked, so two runs that share only one of them cannot lose each
    // other's section. The locks also remove each file's leftover temporary files, which for
    // the credentials file hold the secret. Both variables can name one missing file; the
    // locks then share one lock file, which is locked once (`PrivateFile::lock_all`).
    let _locks = PrivateFile::lock_all(&[&config_file, &credentials_file])?;
    let config = read(&config_file)?;
    let credentials = read(&credentials_file)?;
    if has_section(&config, Kind::Config, &profile.0)
        || has_section(&credentials, Kind::Credentials, &profile.0)
    {
        confirm(&format!("replace the AWS profile `{}`", profile.0))?;
    }
    let config = set_section(
        &config,
        Kind::Config,
        &profile.0,
        &profile.config_header(),
        &[
            &format!("endpoint_url = {ENDPOINT}"),
            &format!("region = {REGION}"),
            "s3 =",
            "    addressing_style = path",
        ],
    );
    // Built at its final size, so no growth leaves a copy of the secret in freed memory.
    let secret_line = |key: &str, value: &str| {
        let mut line = Zeroizing::new(String::with_capacity(key.len() + 3 + value.len()));
        line.push_str(key);
        line.push_str(" = ");
        line.push_str(value);
        line
    };
    let id_line = secret_line("aws_access_key_id", &access_key_id.0);
    let secret_line = secret_line("aws_secret_access_key", &secret.0);
    let credentials = set_section(
        &credentials,
        Kind::Credentials,
        &profile.0,
        &profile.credentials_header(),
        &[id_line.as_str(), secret_line.as_str()],
    );
    let created = [
        (&config_file, config_file.create_if_missing()?),
        (&credentials_file, credentials_file.create_if_missing()?),
    ];
    refuse_or_take_back(files, created)?;
    config_file.write(config.as_bytes())?;
    credentials_file.write(credentials.as_bytes())
}

/// Refuses when both variables name one file that exists (see [`same_file`]).
fn refuse_one_file(files: &AwsFiles) -> Result<()> {
    if same_file(&files.config, &files.credentials) {
        return Err(Error::usage(
            format!(
                "`AWS_CONFIG_FILE` and `AWS_SHARED_CREDENTIALS_FILE` both name {}",
                quote_path(&files.config)
            ),
            "set `AWS_CONFIG_FILE` and `AWS_SHARED_CREDENTIALS_FILE` to two different files",
        ));
    }
    Ok(())
}

/// Whether `a` and `b` are one file: the same name in the same directory (once the
/// directories are resolved), or, when both exist, the same device and inode. The inode
/// covers the names only the file system calls equal: case on macOS and Windows volumes,
/// Unicode normalization on APFS (`café` composed and decomposed), and hard links.
///
/// While either file is missing there is no inode to compare, so two names in one directory
/// that differ only in ASCII case count as one file: on a case-insensitive volume they become
/// one once created, and a dry run, which creates nothing, must refuse what the real run
/// refuses after it creates them. On a case-sensitive volume this
/// refuses two files a person would not tell apart, which costs one rename. Names equal
/// only under Unicode normalization are not caught while a file is missing: that needs a
/// normalization table, which neither the standard library nor a dependency of this crate
/// has, and the real run still refuses them once both exist.
fn same_file(a: &Path, b: &Path) -> bool {
    // A bare name is in the working directory, as the real run's open finds it.
    let place = |path: &Path| {
        let dir = crate::private_file::directory(path);
        Some((fs::canonicalize(dir).ok()?, path.file_name()?.to_owned()))
    };
    if let (Some((dir_a, name_a)), Some((dir_b, name_b))) = (place(a), place(b))
        && dir_a == dir_b
    {
        let missing = fs::symlink_metadata(a).is_err() || fs::symlink_metadata(b).is_err();
        if name_a == name_b || (missing && name_a.eq_ignore_ascii_case(&name_b)) {
            return true;
        }
    }
    match (fs::symlink_metadata(a), fs::symlink_metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// [`refuse_one_file`] once the missing files exist. A refused run takes back the empty
/// files it made (each file with whether this run created it), so it leaves nothing
/// behind. Best effort: an empty file left after a failed removal is harmless to the AWS
/// CLI.
fn refuse_or_take_back(files: &AwsFiles, created: [(&PrivateFile, bool); 2]) -> Result<()> {
    let refused = refuse_one_file(files);
    if refused.is_err() {
        for (file, made) in created {
            if made {
                file.remove();
            }
        }
    }
    refused
}

/// Reads an AWS file, cleared when dropped: the credentials file holds every profile's key.
/// A missing file is empty. A symlink is refused, because the rename that writes it would
/// replace the link with a file and quietly change the user's setup.
fn read(file: &PrivateFile) -> Result<Zeroizing<String>> {
    let Some(bytes) = file.read()? else {
        return Ok(Zeroizing::default());
    };
    let path = file.path();
    String::from_utf8(bytes)
        .map(Zeroizing::new)
        .map_err(|refused| {
            // The bytes the error holds are the file's, keys included.
            drop(Zeroizing::new(refused.into_bytes()));
            Error::usage(
                format!("the AWS file {} is not UTF-8 text", quote_path(path)),
                format!("save {} as UTF-8 text", quote_path(path)),
            )
        })
}

/// Which AWS file a text is: botocore names a profile's section differently in each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// The config file: `[default]` or `[profile <name>]`, the name split as a shell would.
    Config,
    /// The credentials file: `[<name>]`, the text between the brackets as it is.
    Credentials,
}

/// Each line of `text`, with its line ending, beside the section header it is, as Python's
/// `configparser` reads the file for botocore: a header is a line that is `[`, at least one
/// character, and `]` once surrounding whitespace is stripped, and the header is the text up
/// to the last `]`. A line indented deeper than the option above it in a section continues that
/// option's value, as `addressing_style` continues `s3 =`, so it is never a header, however
/// it looks; blank lines and comments in between do not end the value.
fn lines_and_headers(text: &str) -> impl Iterator<Item = (&str, Option<&str>)> {
    // Whether a section has started, whether its last option line named an option, and the
    // indent of the last line that was not a continuation.
    let mut in_section = false;
    let mut option = false;
    let mut indent = 0;
    text.split_inclusive('\n').map(move |line| {
        let value = line.trim();
        if value.is_empty() || value.starts_with(['#', ';']) {
            return (line, None);
        }
        let this_indent = line.chars().take_while(|c| c.is_whitespace()).count();
        if in_section && option && this_indent > indent {
            return (line, None);
        }
        indent = this_indent;
        let header = value
            .strip_prefix('[')
            .and_then(|rest| rest.rsplit_once(']'))
            .map(|(header, _)| header)
            .filter(|header| !header.is_empty());
        if header.is_some() {
            in_section = true;
            option = false;
        } else if let Some(at) = value.find(['=', ':']) {
            // `configparser` takes `=` or `:` as the separator; a line with no name before it
            // is an error that names no option. A line with neither leaves the option as it was.
            option = !value[..at].trim_end().is_empty();
        }
        (line, header)
    })
}

/// The profile the section `header` holds in a file of `kind`, as botocore reads it, or `None`
/// for a section that holds none. In the config file that is `default`, or `profile` and one
/// more word, split as Python's `shlex.split` splits it (`[profile  x]` and `[profile "x"]` are
/// both `x`, and `[profile default]` is `default`); in the credentials file it is the header
/// as it is, so `[profile default]` there is a profile named `profile default`.
fn profile_named(kind: Kind, header: &str) -> Option<String> {
    match kind {
        Kind::Credentials => Some(header.to_owned()),
        Kind::Config if header == "default" => Some(header.to_owned()),
        // botocore passes over a profile whose name is empty.
        Kind::Config if header.starts_with("profile") => match &shell_words(header)?[..] {
            [_, name] if !name.is_empty() => Some(name.clone()),
            _ => None,
        },
        Kind::Config => None,
    }
}

/// `text` split into words as Python's `shlex.split` splits it (POSIX mode, no comments):
/// space, tab, and line breaks separate words, single quotes keep everything, double quotes
/// keep everything but a backslash before `"` or `\`, and a backslash outside quotes keeps
/// the next character. `None` for an unclosed quote or a trailing backslash, which
/// `shlex.split` refuses.
fn shell_words(text: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\r' | '\n' => words.extend(word.take()),
            '\'' => {
                let word = word.get_or_insert_with(String::new);
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => word.push(c),
                    }
                }
            }
            '"' => {
                let word = word.get_or_insert_with(String::new);
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => {
                            let next = chars.next()?;
                            if !matches!(next, '"' | '\\') {
                                word.push('\\');
                            }
                            word.push(next);
                        }
                        c => word.push(c),
                    }
                }
            }
            '\\' => word.get_or_insert_with(String::new).push(chars.next()?),
            c => word.get_or_insert_with(String::new).push(c),
        }
    }
    words.extend(word);
    Some(words)
}

/// Whether `text`, a file of `kind`, has a section for the profile `name`.
fn has_section(text: &str, kind: Kind, name: &str) -> bool {
    lines_and_headers(text)
        .filter_map(|(_, header)| header)
        .any(|header| profile_named(kind, header).as_deref() == Some(name))
}

/// `text`, a file of `kind`, with the section of the profile `name` holding exactly `body`,
/// under `header`, cleared when dropped, as the text it is built from is.
///
/// The first section of that profile is replaced in place, and any later section of it is
/// removed, so the AWS CLI cannot read a stale duplicate or refuse the file for holding one.
/// Without such a section, it is appended after a blank line. Every other line is kept as it
/// was, line endings included. Each string is built at its final size, so no growth leaves a
/// copy of a key in freed memory.
fn set_section(
    text: &str,
    kind: Kind,
    name: &str,
    header: &str,
    body: &[&str],
) -> Zeroizing<String> {
    let size = header.len() + 1 + body.iter().map(|line| line.len() + 1).sum::<usize>();
    let mut section = Zeroizing::new(String::with_capacity(size));
    section.push_str(header);
    section.push('\n');
    for line in body {
        section.push_str(line);
        section.push('\n');
    }
    // Two bytes to spare for the line breaks before an appended section.
    let mut out = Zeroizing::new(String::with_capacity(text.len() + 2 + section.len()));
    let mut skipping = false;
    let mut replaced = false;
    for (line, found) in lines_and_headers(text) {
        // The header that ends a section this replaced or removed follows an option of the
        // new body, at column 0; indented, it would read as that option's continuation, so
        // its indent is dropped. A header resets the option, so nothing after it changes.
        let ended = skipping;
        if let Some(found) = found {
            skipping = profile_named(kind, found).as_deref() == Some(name);
            if skipping && !replaced {
                out.push_str(&section);
                replaced = true;
            }
        }
        if !skipping {
            out.push_str(if ended { line.trim_start() } else { line });
        }
    }
    if !replaced {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str(&section);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};

    use super::*;

    fn lines<'a>(body: &[&'a str]) -> Vec<&'a str> {
        body.to_vec()
    }

    /// [`set_section`] of a credentials file, as a plain string.
    fn set(text: &str, name: &str, header: &str, body: &[&str]) -> String {
        set_section(text, Kind::Credentials, name, header, body).to_string()
    }

    #[test]
    fn a_new_section_is_appended_after_a_blank_line() {
        assert_eq!(set("", "a", "[a]", &lines(&["x = 1"])), "[a]\nx = 1\n");
        assert_eq!(
            set("[b]\ny = 2\n", "a", "[a]", &lines(&["x = 1"])),
            "[b]\ny = 2\n\n[a]\nx = 1\n"
        );
        assert_eq!(
            set("[b]\ny = 2", "a", "[a]", &lines(&["x = 1"])),
            "[b]\ny = 2\n\n[a]\nx = 1\n"
        );
        assert_eq!(
            set("[b]\ny = 2\n\n", "a", "[a]", &lines(&["x = 1"])),
            "[b]\ny = 2\n\n[a]\nx = 1\n"
        );
    }

    #[test]
    fn an_existing_section_is_replaced_in_place_and_the_rest_kept_byte_for_byte() {
        let text = "# top comment\r\n[b]\r\ny = 2\r\n[a]\nold = 1\n; note\n[c]\nz = 3 \n";
        assert_eq!(
            set(text, "a", "[a]", &lines(&["x = 1"])),
            "# top comment\r\n[b]\r\ny = 2\r\n[a]\nx = 1\n[c]\nz = 3 \n"
        );
    }

    #[test]
    fn a_later_duplicate_section_is_removed() {
        let text = "[a]\nold = 1\n[b]\ny = 2\n[a]  \nstale = 1\n[c]\n";
        assert_eq!(
            set(text, "a", "[a]", &lines(&["x = 1"])),
            "[a]\nx = 1\n[b]\ny = 2\n[c]\n"
        );
    }

    #[test]
    fn a_similar_header_is_not_the_same_section() {
        let text = "[profile hippius-old]\nx = 1\n";
        let out = set(
            text,
            "profile hippius",
            "[profile hippius]",
            &lines(&["y = 2"]),
        );
        assert!(out.starts_with("[profile hippius-old]\nx = 1\n"), "{out}");
        assert!(has_section(&out, Kind::Credentials, "profile hippius"));
        assert!(!has_section(text, Kind::Credentials, "profile hippius"));
    }

    #[test]
    fn profile_names_are_checked() {
        for good in ["hippius", "default", "a.b_c-1", &"a".repeat(64)] {
            assert!(AwsProfile::new(good).is_ok(), "{good}");
        }
        for bad in ["", "a b", "a]", "a\n[x]", "a/b", &"a".repeat(65)] {
            assert!(AwsProfile::new(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            AwsProfile::new("default").unwrap().config_header(),
            "[default]"
        );
        assert_eq!(AwsProfile::new("x").unwrap().config_header(), "[profile x]");
        assert_eq!(AwsProfile::new("x").unwrap().credentials_header(), "[x]");
        assert_eq!(AwsProfile::new("x").unwrap().as_str(), "x");
    }

    #[test]
    fn key_ids_and_secrets_are_checked_and_secrets_never_printed() {
        assert!(AccessKeyId::new("hip_AbC123").is_ok());
        for bad in ["", "hip_", "AKIA123", "hip_a b", "hip_a\nx", "hip_a=b"] {
            assert!(AccessKeyId::new(bad).is_err(), "{bad:?}");
        }
        let secret = SecretKey::new("  s3cr3t+/=\n", &Source::Argument).unwrap();
        assert_eq!(format!("{secret:?}"), "SecretKey(redacted)");
        for bad in ["", "  ", "QZ7 QZ7", "QZ7\nQZ7", "QZ7\u{7}QZ7"] {
            let error = SecretKey::new(bad, &Source::Argument).unwrap_err();
            assert!(!error.to_string().contains("QZ7"), "{error}");
        }
    }

    /// A fresh directory with mode 0700, whatever the umask. Ubuntu's default umask, 0002,
    /// would leave it writable by the group, which hipc refuses on purpose.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hipc-aws-{name}-{}", std::process::id()));
        drop(fs::remove_dir_all(&dir));
        fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    /// Writes `bytes` to `path` with mode `0600`, as the AWS CLI leaves its files.
    fn write_private_file(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn configure_new(files: &AwsFiles) -> Error {
        let profile = AwsProfile::new("p").unwrap();
        let key = AccessKeyId::new("hip_K").unwrap();
        let secret = SecretKey::new("s", &Source::Argument).unwrap();
        let never = |_: &str| -> Result<()> { panic!("refused before any question") };
        configure(files, &profile, &key, &secret, never).unwrap_err()
    }

    #[test]
    fn configure_writes_both_files_with_mode_0600_and_keeps_other_profiles() {
        let dir = temp_dir("write");
        let files = AwsFiles {
            config: dir.join(".aws").join("config"),
            credentials: dir.join(".aws").join("credentials"),
        };
        fs::create_dir_all(dir.join(".aws")).unwrap();
        fs::set_permissions(dir.join(".aws"), fs::Permissions::from_mode(0o700)).unwrap();
        write_private_file(&files.credentials, b"[work]\naws_access_key_id = AKIAX\n");
        // A temporary file a killed run left, a copy of the credentials with a secret in it,
        // is removed under the credentials lock.
        let leftovers = [".credentials.7.tmp"];
        for name in leftovers {
            write_private_file(&dir.join(".aws").join(name), b"aws_secret_access_key = old");
        }
        let profile = AwsProfile::new("hippius").unwrap();
        let key = AccessKeyId::new("hip_KEY1").unwrap();
        let never = |_: &str| -> Result<()> { panic!("a new profile needs no confirmation") };
        configure(
            &files,
            &profile,
            &key,
            &SecretKey::new("sekrit", &Source::Argument).unwrap(),
            never,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&files.config).unwrap(),
            "[profile hippius]\nendpoint_url = https://s3.hippius.com\nregion = decentralized\n\
             s3 =\n    addressing_style = path\n"
        );
        assert_eq!(
            fs::read_to_string(&files.credentials).unwrap(),
            "[work]\naws_access_key_id = AKIAX\n\n[hippius]\naws_access_key_id = hip_KEY1\n\
             aws_secret_access_key = sekrit\n"
        );
        for path in [&files.config, &files.credentials] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        for name in leftovers {
            assert!(!dir.join(".aws").join(name).exists(), "{name} is left");
        }
        fs::remove_dir_all(dir).unwrap();
    }

    /// The lines outside any section named `name`, joined, as the AWS CLI would read them.
    fn outside(text: &str, name: &str) -> String {
        let mut inside = false;
        lines_and_headers(text)
            .filter(|(_, header)| {
                if let Some(found) = header {
                    inside = *found == name;
                }
                !inside
            })
            .map(|(line, _)| line)
            .collect()
    }

    /// Every file of up to four lines from a small alphabet, each line ending in `\n`,
    /// `\r\n`, or (last only) nothing: `set_section` keeps every byte outside the
    /// section, leaves exactly one section with exactly the new body, and is idempotent.
    #[test]
    fn set_section_keeps_every_other_byte_for_every_small_file() {
        const LINES: [&str; 7] = ["[p]", "[ p ]", "[q]", "k = v", "", "# c", "[profile p]"];
        const ENDS: [&str; 3] = ["\n", "\r\n", ""];
        let body = ["a = 1"];
        let mut files = vec![String::new()];
        for _ in 0..4 {
            let longer: Vec<String> = files
                .iter()
                .filter(|f| f.is_empty() || f.ends_with('\n'))
                .flat_map(|f| {
                    LINES
                        .iter()
                        .flat_map(move |line| ENDS.iter().map(move |end| format!("{f}{line}{end}")))
                })
                .collect();
            files.extend(longer);
        }
        files.sort();
        files.dedup();
        assert!(files.len() > 10_000, "{}", files.len());
        for text in &files {
            let out = set(text, "p", "[p]", &body);
            let before = outside(text, "p");
            let after = outside(&out, "p");
            assert!(
                after.starts_with(&before),
                "a line changed: {text:?} -> {out:?}"
            );
            let added = &after[before.len()..];
            assert!(matches!(added, "" | "\n" | "\n\n"), "{text:?} -> {out:?}");
            let headers = lines_and_headers(&out)
                .filter(|(_, header)| *header == Some("p"))
                .count();
            assert_eq!(headers, 1, "{text:?} -> {out:?}");
            assert!(out.contains("[p]\na = 1\n"), "{text:?} -> {out:?}");
            assert_eq!(set(&out, "p", "[p]", &body), out, "{text:?}");
        }
    }

    /// `configure` waits for the lock of the config file, not only for the credentials
    /// file's, and says so first: two runs that share only the config file cannot lose each
    /// other's section.
    #[test]
    fn configure_waits_for_the_config_files_lock_and_says_so() {
        let dir = temp_dir("config-lock");
        let files = AwsFiles {
            config: dir.join("config"),
            credentials: dir.join("credentials"),
        };
        // The profile exists, so the run asks before it replaces it, under both locks.
        write_private_file(&files.credentials, b"[p]\nold = 1\n");
        let other = PrivateFile::ensure(&files.config, AWS_CONFIG).unwrap();
        let held = other.lock().unwrap();
        let (told, waiting) = std::sync::mpsc::channel();
        let run = {
            let files = AwsFiles {
                config: files.config.clone(),
                credentials: files.credentials.clone(),
            };
            std::thread::spawn(move || {
                crate::private_file::on_lock_wait(move |note| told.send(note.to_owned()).unwrap());
                let profile = AwsProfile::new("p").unwrap();
                let key = AccessKeyId::new("hip_K").unwrap();
                let secret = SecretKey::new("s", &Source::Argument).unwrap();
                configure(&files, &profile, &key, &secret, |_| Ok(()))
            })
        };
        let note = waiting
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("configure waits for the config file's lock");
        assert_eq!(
            note,
            format!(
                "waiting for another run of hipc to finish with the AWS config file {}",
                crate::phrase::quote_path(&files.config)
            )
        );
        assert_eq!(
            fs::read_to_string(&files.credentials).unwrap(),
            "[p]\nold = 1\n",
            "nothing is written while it waits"
        );
        drop(held);
        run.join().unwrap().unwrap();
        assert!(
            fs::read_to_string(&files.credentials)
                .unwrap()
                .contains("[p]")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    /// Both variables can name one missing file through two spellings the directory check
    /// does not tell apart; its one lock file is locked once, not twice, which would wait
    /// for this process forever. Two files are locked together, and a free lock is taken
    /// without a word.
    #[test]
    fn one_lock_file_named_twice_is_locked_once() {
        let dir = temp_dir("one-lock-file");
        // A lock file an older run left read-only still locks.
        fs::write(dir.join(".other.lock"), "").unwrap();
        fs::set_permissions(dir.join(".other.lock"), fs::Permissions::from_mode(0o400)).unwrap();
        let held = PrivateFile::ensure(&dir.join("other"), AWS_CREDENTIALS)
            .unwrap()
            .lock()
            .unwrap();
        let (told, notes) = std::sync::mpsc::channel();
        let run = {
            let dir = dir.clone();
            std::thread::spawn(move || {
                crate::private_file::on_lock_wait(move |note| told.send(note.to_owned()).unwrap());
                let a = PrivateFile::ensure(&dir.join("same"), AWS_CONFIG).unwrap();
                let b = PrivateFile::ensure(&dir.join(".").join("same"), AWS_CREDENTIALS).unwrap();
                let c = PrivateFile::ensure(&dir.join("other"), AWS_CREDENTIALS).unwrap();
                let once = PrivateFile::lock_all(&[&a, &b]).unwrap().len();
                // Then a lock another descriptor holds: this one waits, and says so.
                let both = PrivateFile::lock_all(&[&c, &a, &b]).unwrap().len();
                (once, both)
            })
        };
        // The first note is the wait for `other`: the lock of `same` was taken once, at once.
        let note = notes
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("the wait for the held lock");
        assert!(
            note.ends_with(&crate::phrase::quote_path(&dir.join("other"))),
            "{note}"
        );
        drop(held);
        assert_eq!(run.join().unwrap(), (1, 2));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn one_missing_path_named_twice_is_refused_and_nothing_is_created() {
        let dir = temp_dir("same-missing-path");
        let files = AwsFiles {
            config: dir.join("both"),
            credentials: dir.join(".").join("both"),
        };
        let never = |_: &str| -> Result<()> { panic!("refused before any question") };
        let error = configure(
            &files,
            &AwsProfile::new("p").unwrap(),
            &AccessKeyId::new("hip_K").unwrap(),
            &SecretKey::new("s", &Source::Argument).unwrap(),
            never,
        )
        .unwrap_err();
        assert_eq!(error.code(), crate::Code::Usage, "{error}");
        assert!(!dir.join("both").exists(), "nothing was left behind");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_refusal_after_creating_takes_the_created_file_back() {
        let dir = temp_dir("take-back");
        fs::write(dir.join("both"), "").unwrap();
        let files = AwsFiles {
            config: dir.join("both"),
            credentials: dir.join("both"),
        };
        let config = PrivateFile::ensure(&files.config, AWS_CONFIG).unwrap();
        let credentials = PrivateFile::ensure(&files.credentials, AWS_CREDENTIALS).unwrap();
        let created = [(&config, true), (&credentials, false)];
        let error = refuse_or_take_back(&files, created).unwrap_err();
        assert_eq!(error.code(), crate::Code::Usage);
        assert!(!dir.join("both").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_that_cannot_be_created_is_usage() {
        skip_if_root!();
        let dir = temp_dir("read-only-dir");
        let (locked, writable) = (dir.join("ro"), dir.join("rw"));
        fs::create_dir_all(&locked).unwrap();
        fs::create_dir_all(&writable).unwrap();
        fs::set_permissions(&writable, fs::Permissions::from_mode(0o700)).unwrap();
        // The owner may read and enter but not write: the directory check passes, the
        // create fails. The config file's lock file exists already, so its lock is taken.
        fs::write(locked.join(".config.lock"), "").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).unwrap();
        let files = AwsFiles {
            config: locked.join("config"),
            credentials: writable.join("credentials"),
        };
        let profile = AwsProfile::new("p").unwrap();
        let (key, secret) = (
            AccessKeyId::new("hip_K").unwrap(),
            SecretKey::new("s", &Source::Argument).unwrap(),
        );
        let never = |_: &str| -> Result<()> { panic!("no profile exists to replace") };
        let error = configure(&files, &profile, &key, &secret, never).unwrap_err();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_dir_all(dir).unwrap();
        // A mode the person changes, not a bug to report.
        assert_eq!(error.code(), crate::Code::Usage, "{error}");
        assert_eq!(
            error.to_string(),
            format!(
                "cannot create {}: permission is denied",
                crate::phrase::quote_path(&locked.join("config"))
            )
        );
        assert_eq!(
            error.hint(),
            Some(&*format!("run `chmod 700 {}`", locked.display()))
        );
    }

    /// Names the file system treats as one file: case, and composed and decomposed
    /// Unicode. Only macOS volumes do this by default; on Linux these are two files.
    #[cfg(target_os = "macos")]
    #[test]
    fn names_the_file_system_folds_together_are_one_file() {
        let dir = temp_dir("folded-names");
        let profile = AwsProfile::new("p").unwrap();
        let (key, secret) = (
            AccessKeyId::new("hip_K").unwrap(),
            SecretKey::new("s", &Source::Argument).unwrap(),
        );
        for (config, credentials) in [("config", "Config"), ("caf\u{e9}", "cafe\u{301}")] {
            let files = AwsFiles {
                config: dir.join(config),
                credentials: dir.join(credentials),
            };
            let never = |_: &str| -> Result<()> { panic!("refused before any question") };
            let error = configure(&files, &profile, &key, &secret, never).unwrap_err();
            assert_eq!(error.code(), crate::Code::Usage, "{config} {credentials}");
            assert!(
                !files.config.exists(),
                "the refused run takes back its empty file"
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }

    /// While a file is missing, names that differ only in ASCII case are one file to the
    /// check a dry run makes, on any volume, and nothing is created; two existing files
    /// are compared by inode.
    #[test]
    fn a_missing_name_that_differs_only_in_case_is_the_same_file() {
        let dir = temp_dir("case-only");
        let files = AwsFiles {
            config: dir.join("config"),
            credentials: dir.join("CONFIG"),
        };
        assert_eq!(files.check().unwrap_err().code(), crate::Code::Usage);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        let other = AwsFiles {
            config: dir.join("config"),
            credentials: dir.join("credentials"),
        };
        assert!(other.check().is_ok());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        fs::remove_dir_all(dir).unwrap();
    }

    /// A bare name is in the working directory, so the dry run's check finds one missing
    /// file named by both variables, as the real run does once it exists, and a bare name and
    /// the same name under `./` are one file too.
    #[test]
    fn a_bare_name_given_twice_is_one_file_in_the_working_directory() {
        let name = Path::new("hipc-test-aws-bare-name-not-created");
        assert!(same_file(name, name));
        assert!(same_file(name, &Path::new(".").join(name)));
        assert!(!same_file(name, Path::new("hipc-test-aws-another-name")));
        assert!(!name.exists());
    }

    #[test]
    fn one_file_named_by_both_variables_is_refused() {
        let dir = temp_dir("same-file");
        fs::write(dir.join("both"), "").unwrap();
        fs::hard_link(dir.join("both"), dir.join("linked")).unwrap();
        let profile = AwsProfile::new("p").unwrap();
        let (key, secret) = (
            AccessKeyId::new("hip_K").unwrap(),
            SecretKey::new("s", &Source::Argument).unwrap(),
        );
        for (config, credentials) in [
            (dir.join("both"), dir.join("both")),
            (dir.join("both"), dir.join(".").join("both")),
            (dir.join("both"), dir.join("linked")),
        ] {
            let files = AwsFiles {
                config,
                credentials,
            };
            let never = |_: &str| -> Result<()> { panic!("refused before any question") };
            let error = configure(&files, &profile, &key, &secret, never).unwrap_err();
            assert_eq!(error.code(), crate::Code::Usage, "{error}");
        }
        assert_eq!(fs::read_to_string(dir.join("both")).unwrap(), "");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_config_file_in_a_directory_others_can_write_is_refused() {
        let dir = temp_dir("open-config-dir");
        let open = dir.join("open");
        let private = dir.join("private");
        fs::create_dir_all(&open).unwrap();
        fs::create_dir_all(&private).unwrap();
        fs::set_permissions(&open, fs::Permissions::from_mode(0o777)).unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();
        let files = AwsFiles {
            config: open.join("config"),
            credentials: private.join("credentials"),
        };
        let profile = AwsProfile::new("p").unwrap();
        let (key, secret) = (
            AccessKeyId::new("hip_K").unwrap(),
            SecretKey::new("s", &Source::Argument).unwrap(),
        );
        let never = |_: &str| -> Result<()> { panic!("a new profile needs no confirmation") };
        let error = configure(&files, &profile, &key, &secret, never).unwrap_err();
        assert_eq!(error.code(), crate::Code::Usage, "{error}");
        assert!(!files.config.exists() && !files.credentials.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_symlink_or_a_non_utf8_file_is_refused() {
        let dir = temp_dir("refuse");
        fs::write(dir.join("real"), "").unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("link")).unwrap();
        write_private_file(&dir.join("binary"), &[0xff, 0xfe]);
        let profile = AwsProfile::new("p").unwrap();
        let key = AccessKeyId::new("hip_K").unwrap();
        let secret = SecretKey::new("s", &Source::Argument).unwrap();
        for (linked, variable) in [
            (
                AwsFiles {
                    config: dir.join("link"),
                    credentials: dir.join("c"),
                },
                "AWS_CONFIG_FILE",
            ),
            (
                AwsFiles {
                    config: dir.join("c"),
                    credentials: dir.join("link"),
                },
                "AWS_SHARED_CREDENTIALS_FILE",
            ),
        ] {
            let error = configure_new(&linked);
            assert!(error.to_string().contains("is a symlink"), "{error}");
            assert_eq!(error.code(), crate::Code::Usage, "{error}");
            assert_eq!(
                error.hint(),
                Some(format!("set `{variable}` to the file the symlink points to").as_str())
            );
            assert!(!dir.join("c").exists(), "nothing was created");
            assert!(fs::symlink_metadata(dir.join("link")).unwrap().is_symlink());
            assert_eq!(
                fs::read(dir.join("real")).unwrap(),
                b"",
                "the target is untouched"
            );
        }
        let binary = AwsFiles {
            config: dir.join("config"),
            credentials: dir.join("binary"),
        };
        let error = configure(&binary, &profile, &key, &secret, |_| Ok(())).unwrap_err();
        assert!(error.to_string().contains("UTF-8"), "{error}");
        assert_eq!(error.code(), crate::Code::Usage, "{error}");
        assert_eq!(
            fs::read(dir.join("binary")).unwrap(),
            [0xff, 0xfe],
            "nothing was written"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_existing_profile_is_replaced_only_when_confirmed() {
        let dir = temp_dir("confirm");
        let files = AwsFiles {
            config: dir.join("config"),
            credentials: dir.join("credentials"),
        };
        write_private_file(
            &files.credentials,
            b"[hippius] ; old\naws_secret_access_key = OLD\n",
        );
        let profile = AwsProfile::new("hippius").unwrap();
        let key = AccessKeyId::new("hip_K").unwrap();
        let secret = SecretKey::new("NEW", &Source::Argument).unwrap();
        let refused = configure(&files, &profile, &key, &secret, |what| {
            assert_eq!(what, "replace the AWS profile `hippius`");
            Err(Error::usage("no", "pass `--yes`"))
        });
        assert!(refused.is_err());
        assert!(
            fs::read_to_string(&files.credentials)
                .unwrap()
                .contains("OLD")
        );
        assert!(!files.config.exists());
        configure(&files, &profile, &key, &secret, |_| Ok(())).unwrap();
        let credentials = fs::read_to_string(&files.credentials).unwrap();
        assert!(
            credentials.contains("NEW") && !credentials.contains("OLD"),
            "{credentials}"
        );
        assert_eq!(credentials.matches("[hippius]").count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn headers_are_matched_as_the_aws_cli_reads_them() {
        let headers = |text| {
            lines_and_headers(text)
                .filter_map(|(_, header)| header)
                .collect::<Vec<_>>()
        };
        assert_eq!(headers("  [hippius] ; comment\n"), ["hippius"]);
        assert_eq!(headers("[a] # [b]\n[]\n[]]\n"), ["a] # [b", "]"]);
        assert_eq!(headers("key = [x]\n[unclosed\n"), Vec::<&str>::new());
        // A line indented under an option continues its value, past blank lines and
        // comments; one at the option's indent, or under no option, is a header.
        let text = "[a]\ns3 =\n  [b]\n\n# c\n    [c]\n[d]\n  [e]\nk = v\n[f]\n";
        assert_eq!(headers(text), ["a", "d", "e", "f"]);
        assert_eq!(
            headers("  k = v\n    [a]\n[b]\n  x\n    [c]\n"),
            ["a", "b", "c"]
        );
        assert_eq!(headers("[a]\n = v\n  [b]\n"), ["a", "b"]);
        assert_eq!(headers("[a]\n k: v\n  junk\n  [b]\n [c]\n"), ["a", "c"]);
        for (kind, header, profile) in [
            (Kind::Config, "default", Some("default")),
            (Kind::Config, "profile default", Some("default")),
            (Kind::Config, "profile  hippius", Some("hippius")),
            (Kind::Config, "profile \"hip pius\"", Some("hip pius")),
            (Kind::Config, "profile 'x'\\y\"\\\"\\n\"", Some("xy\"\\n")),
            (Kind::Config, "profile ''", None),
            (Kind::Config, "profilex y", Some("y")),
            (Kind::Config, "profile", None),
            (Kind::Config, "profile a b", None),
            (Kind::Config, "profile \"a", None),
            (Kind::Config, "profile 'a", None),
            (Kind::Config, "profile a\\", None),
            (Kind::Config, " profile a", None),
            (Kind::Config, "hippius", None),
            (Kind::Config, "sso-session x", None),
            (
                Kind::Credentials,
                "profile default",
                Some("profile default"),
            ),
            (Kind::Credentials, " hippius ", Some(" hippius ")),
            (Kind::Credentials, "default", Some("default")),
        ] {
            assert_eq!(
                profile_named(kind, header).as_deref(),
                profile,
                "{header:?}"
            );
        }
        let text = "[profile  hippius] # note\nold = 1\n[profile default]\nd = 1\n";
        let set_config = |name, header, body: &[&str]| {
            set_section(text, Kind::Config, name, header, body).to_string()
        };
        assert_eq!(
            set_config("hippius", "[profile hippius]", &["n = 2"]),
            "[profile hippius]\nn = 2\n[profile default]\nd = 1\n"
        );
        assert_eq!(
            set_config("default", "[default]", &["d = 2"]),
            "[profile  hippius] # note\nold = 1\n[default]\nd = 2\n"
        );
        // In the credentials file, `[profile default]` is not `default`, and an indented
        // header under an option is part of its value.
        let text = "[profile default]\nd = 1\n[x]\nk = v\n  [default]\n";
        assert!(!has_section(text, Kind::Credentials, "default"));
        assert!(has_section(text, Kind::Credentials, "profile default"));
        assert_eq!(
            set(text, "default", "[default]", &["d = 2"]),
            format!("{text}\n[default]\nd = 2\n")
        );
        // The indented header after a replaced section is moved to column 0, so it does not
        // read as a continuation of the new body's last option.
        let text = "[p]\n a = 1\n [work]\n k = W\n";
        let out = set(text, "p", "[p]", &["x = 1"]);
        assert_eq!(out, "[p]\nx = 1\n[work]\n k = W\n");
        let headers: Vec<_> = lines_and_headers(&out).filter_map(|(_, h)| h).collect();
        assert_eq!(headers, ["p", "work"]);
    }

    #[test]
    fn paths_follow_the_aws_variables_then_home() {
        let files = AwsFiles::locate(None, None, Some(Path::new("/h"))).unwrap();
        assert_eq!(files.config, PathBuf::from("/h/.aws/config"));
        assert_eq!(files.credentials, PathBuf::from("/h/.aws/credentials"));
        let files = AwsFiles::locate(Some("/c".into()), Some("/k".into()), None).unwrap();
        assert_eq!(
            (files.config, files.credentials),
            ("/c".into(), "/k".into())
        );
        assert!(AwsFiles::locate(None, Some("/k".into()), None).is_err());
        assert!(AwsFiles::locate(Some("/c".into()), None, Some(Path::new(""))).is_err());
    }

    #[test]
    fn a_path_the_aws_cli_would_expand_is_refused() {
        let home = Some(Path::new("/h"));
        for path in ["~/c", "~", "~u/c", "$HOME/c", "/a/${X}/c", "/a$"] {
            let error = AwsFiles::locate(Some(path.into()), None, home).unwrap_err();
            assert!(error.message().contains("`AWS_CONFIG_FILE`"), "{path}");
            let error = AwsFiles::locate(None, Some(path.into()), home).unwrap_err();
            assert!(
                error.message().contains("`AWS_SHARED_CREDENTIALS_FILE`"),
                "{path}"
            );
        }
        let files = AwsFiles::locate(Some("/a~/c".into()), Some("k~".into()), home).unwrap();
        assert_eq!(
            (files.config, files.credentials),
            ("/a~/c".into(), "k~".into())
        );
    }

    #[test]
    fn an_empty_aws_variable_is_refused_not_read_as_unset() {
        let home = Some(Path::new("/h"));
        for (config, credentials, variable) in [
            (Some(""), None, "AWS_CONFIG_FILE"),
            (None, Some(""), "AWS_SHARED_CREDENTIALS_FILE"),
        ] {
            let error = AwsFiles::locate(
                config.map(PathBuf::from),
                credentials.map(PathBuf::from),
                home,
            )
            .unwrap_err();
            assert_eq!(
                error.message(),
                format!(
                    "`{variable}` is set but empty, which the AWS CLI reads as a path to no \
                     file, so `hipc` would write a file the AWS CLI does not read"
                )
            );
            assert_eq!(
                error.hint(),
                Some(format!("unset `{variable}`, or set it to the file's path").as_str())
            );
        }
    }

    #[test]
    fn a_symlinked_aws_directory_is_refused_and_nothing_is_written() {
        let dir = temp_dir("linked-dir");
        let real = dir.join("real");
        fs::create_dir_all(&real).unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink(&real, dir.join(".aws")).unwrap();
        // A dangling symlink is refused the same way, not created through.
        std::os::unix::fs::symlink(dir.join("gone"), dir.join("dangling")).unwrap();
        for linked in [dir.join(".aws"), dir.join("dangling")] {
            let files = AwsFiles {
                config: linked.join("config"),
                credentials: real.join("credentials"),
            };
            let error = configure_new(&files);
            assert_eq!(error.code(), crate::Code::Usage, "{error}");
            assert!(error.to_string().contains("is a symlink"), "{error}");
            assert_eq!(
                error.hint(),
                Some("set `AWS_CONFIG_FILE` to a file in the directory the symlink points to")
            );
        }
        let files = AwsFiles {
            config: real.join("config"),
            credentials: dir.join(".aws").join("credentials"),
        };
        let error = configure_new(&files);
        assert!(error.to_string().contains("is a symlink"), "{error}");
        assert!(
            error
                .hint()
                .unwrap()
                .contains("AWS_SHARED_CREDENTIALS_FILE")
        );
        assert_eq!(
            fs::read_dir(&real).unwrap().count(),
            0,
            "nothing was written"
        );
        assert!(
            !dir.join("gone").exists(),
            "nothing was created through the link"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_aws_file_open_to_others_is_refused_and_kept() {
        let dir = temp_dir("open-file");
        let files = AwsFiles {
            config: dir.join("config"),
            credentials: dir.join("credentials"),
        };
        fs::write(&files.config, "[default]\nregion = x\n").unwrap();
        fs::set_permissions(&files.config, fs::Permissions::from_mode(0o644)).unwrap();
        let error = configure_new(&files);
        assert_eq!(error.code(), crate::Code::Usage, "{error}");
        assert!(error.to_string().contains("the S3 endpoint"), "{error}");
        assert!(
            error.hint().unwrap().starts_with("run `chmod 600 "),
            "{error}"
        );
        assert_eq!(
            fs::read_to_string(&files.config).unwrap(),
            "[default]\nregion = x\n"
        );
        assert!(!files.credentials.exists(), "nothing was written");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_fifo_or_an_aws_file_over_1_mib_is_refused_without_waiting() {
        let dir = temp_dir("fifo");
        let fifo = dir.join("fifo");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(made.success());
        // No writer ever opens the FIFO, so a blocking open would hang this test.
        let error = configure_new(&AwsFiles {
            config: fifo,
            credentials: dir.join("credentials"),
        });
        assert_eq!(error.code(), crate::Code::Usage, "{error}");
        assert!(error.to_string().contains("not a regular file"), "{error}");
        assert_eq!(
            error.hint(),
            Some("set `AWS_CONFIG_FILE` to a regular file")
        );

        let big = dir.join("big");
        write_private_file(&big, &vec![b'#'; (1 << 20) + 1]);
        let error = configure_new(&AwsFiles {
            config: dir.join("config"),
            credentials: big.clone(),
        });
        assert_eq!(error.code(), crate::Code::Usage, "{error}");
        assert!(error.to_string().contains("larger than 1 MiB"), "{error}");
        assert!(
            error
                .hint()
                .unwrap()
                .contains("AWS_SHARED_CREDENTIALS_FILE"),
            "{error}"
        );
        assert_eq!(fs::metadata(&big).unwrap().len(), (1 << 20) + 1);
        assert!(!dir.join("config").exists(), "nothing was written");
        fs::remove_dir_all(dir).unwrap();
    }
}
