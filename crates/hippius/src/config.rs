//! The config file and the settings one invocation runs with (`TENETS.md` tenet 8).
//!
//! The file is `$HOME/.hippius/config.toml`, or the path in `HIPPIUS_CONFIG`. It holds API
//! tokens, so it is guarded the way OpenSSH guards a private key:
//!
//! - `hipc` creates the directory with mode `0700` and writes a new file with mode `0600`
//!   (a file that exists keeps its owner bits), through a new temporary file and a rename,
//!   so no reader ever sees a partial file or a wider mode. Each change rewrites the whole
//!   file, so comments are not kept: keeping them needs a format-preserving TOML editor, a
//!   dependency `hipc` does not have.
//! - `hipc` refuses to read the file when its mode gives the group or others any
//!   permission bit (`mode & 0o077`, execute included), and refuses to read or write it
//!   when the directory is writable by anyone but the owner, because then someone else
//!   could replace it. It does not change modes behind
//!   the user's back; the error says which `chmod` fixes it.
//! - `hipc` refuses a file or a directory that is a symlink, and a file larger than 1 MiB.
//!   The directory is opened with `O_NOFOLLOW` and checked on the open descriptor, and the
//!   file, its lock, its temporary file, and the rename are all relative to that
//!   descriptor, so nothing can swap either between the check and the use.
//!
//! The checks cover the owner and the mode bits only. An access-control list is not
//! checked, so an allow entry can still grant another user access the mode bits deny: on
//! macOS, a file created in a directory with an inheritable allow entry inherits it, and
//! `ls -le` shows it. `chmod -N <path>` removes the list. Rust's standard library has no ACL
//! API, the crates forbid `unsafe` code, and `hipc` adds no dependency for
//! it.
//!
//! Every change goes through [`ConfigFile::edit`], which holds an exclusive lock on a file
//! next to the config while it loads, changes, and saves. Two `hipc` processes that change
//! different profiles at once both keep their change.
//!
//! Precedence per field is flag, then environment, then the selected profile. The binary
//! reads flags and the environment and passes them in as [`Overrides`].

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{Read as _, Write as _};
use std::os::fd::OwnedFd;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use rustix::fs::{AtFlags, FileType, FlockOperation, Mode, OFlags};
use rustix::io::Errno;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::account::Selection;
use crate::client::Origin;
use crate::error::{Error, Result};
use crate::phrase::{io_reason, quote, quote_path};
use crate::project::{BillingId, ProjectId};
use crate::source::{Given, Source};

/// The API origin when nothing else is configured.
pub(crate) const DEFAULT_API_URL: &str = "https://api.hippius.com";

/// The profile used when neither a flag, `HIPPIUS_PROFILE`, nor `default_profile` names one.
pub(crate) const DEFAULT_PROFILE: &str = "default";

/// The shortest token whose last four characters `hipc auth status` shows. Console tokens are
/// 40 characters; 12 is arbitrary, chosen so a preview is at most a third of the token.
const PREVIEW_MIN_CHARS: usize = 12;

/// A console API token. `Debug` and `Display` never print it, and its memory is cleared when
/// it is dropped, so a token no longer in use is not left behind in a later crash dump.
#[derive(Clone)]
pub struct Token(Zeroizing<String>);

impl Token {
    /// Wraps a token read from `source`: a file, stdin, `HIPPIUS_TOKEN`, or a profile.
    /// Surrounding whitespace is trimmed.
    ///
    /// # Errors
    ///
    /// `usage` when nothing but whitespace is left, or when what is left is not one word
    /// of visible ASCII. A token goes into an HTTP header, and one with a line break or a
    /// space inside would be stored and then fail every request. The message names the
    /// source and never quotes the token.
    pub fn new(text: &str, source: &Source) -> Result<Self> {
        let token = text.trim();
        let from = source.suffix();
        let problem = if token.is_empty() && !text.is_empty() {
            "holds only whitespace"
        } else if token.is_empty() {
            "is empty"
        } else if !token.bytes().all(|b| b.is_ascii_graphic()) {
            "holds a space, a line break, or a character outside visible ASCII"
        } else {
            return Ok(Self(Zeroizing::new(token.to_owned())));
        };
        let hint = match source {
            Source::Variable(name) => format!("unset `{name}`, or set it to a console token"),
            Source::File { flag, path } if path == Path::new("-") => {
                format!("pipe only the console token, one word, to `{flag} -`")
            }
            Source::File { flag, .. } => {
                format!("leave only the console token, one word, in the file `{flag}` names")
            }
            Source::Profile { .. } | Source::Argument | Source::Flag(_) | Source::Default => {
                "run `hipc auth login --token-file <path>` to store a valid token".to_owned()
            }
        };
        Err(Error::usage(format!("the token{from} {problem}"), hint))
    }

    /// The last four characters, which `hipc auth status` shows, or `None` for a token
    /// shorter than `PREVIEW_MIN_CHARS`, whose last four would give too much of it away.
    pub fn preview(&self) -> Option<String> {
        // A token is visible ASCII (`Token::new`), so a byte is a character. Sliced, not
        // collected, so no copy of the whole token is left uncleared.
        let len = self.0.len();
        (len >= PREVIEW_MIN_CHARS).then(|| self.0[len - 4..].to_owned())
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }

    /// Whether `text` holds this token in any spelling an error redacts it in: as it is,
    /// quoted, or percent-encoded (tenet 9). For a refusal of text that would carry it to a
    /// person, such as a support message.
    pub fn is_in(&self, text: &str) -> bool {
        quoted_in(&self.0, text)
    }
}

/// Whether `text` holds `token` in any spelling [`crate::client::token_spellings`] lists.
/// The token is trimmed first, as [`Token::new`] trims it before it is sent, so a hand-edited
/// file's padded token is found as it goes out. A token shorter than [`PREVIEW_MIN_CHARS`]
/// is in no text: a console token is 40 characters, and a short one, such as `token`, would
/// match ordinary words and refuse a command line or a ticket for holding it.
fn quoted_in(token: &str, text: &str) -> bool {
    let token = token.trim();
    token.chars().count() >= PREVIEW_MIN_CHARS
        && zeroize::Zeroizing::new(crate::client::token_spellings(token))
            .iter()
            .any(|spelling| text.contains(spelling.as_str()))
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(redacted)")
    }
}

/// Values from flags and the environment, each overriding one field of the profile. `Debug`
/// does not print the token, because [`Token`] does not.
#[derive(Debug, Default)]
pub struct Overrides {
    /// `--profile`, else `HIPPIUS_PROFILE`, with which one it was. Checked against the file
    /// by [`ConfigFile::profile_name`].
    pub profile: Option<Given>,
    /// `--api-url`, else `HIPPIUS_API_URL`, with which one it was.
    pub api_url: Option<Given>,
    /// `--account`, else `HIPPIUS_ACCOUNT`, with which one it was.
    pub account: Option<Given>,
    /// `--project`, else `HIPPIUS_PROJECT`, with which one it was.
    pub project: Option<Given>,
    /// `--billing`, else `HIPPIUS_BILLING`, with which one it was.
    pub billing: Option<Given>,
    /// `HIPPIUS_TOKEN`.
    pub token: Option<Token>,
    /// `HIPPIUS_TOKEN_ORIGIN`: the one origin `HIPPIUS_TOKEN` may go to, when it is not the
    /// default one.
    pub token_origin: Option<Given>,
}

/// What one invocation runs with.
#[derive(Debug)]
#[expect(
    clippy::partial_pub_fields,
    reason = "the credential stays in the library, which alone decides what is sent; the \
              binary reads it only through `Settings::token`"
)]
pub struct Settings {
    /// The profile name, whether or not the file has it.
    pub profile: String,
    /// The API origin, without a trailing slash.
    pub api_url: String,
    /// Where `api_url` came from, which an error about it names.
    pub api_url_source: Source,
    /// The token this invocation may send, or why there is none.
    pub(crate) credential: Credential,
    /// The account commands act in.
    pub account: Selection,
    /// The project commands act in, `None` for the Default project.
    pub project: Option<ProjectId>,
    /// The billing account commands act on, `None` for the Default billing account.
    pub billing: Option<BillingId>,
}

impl Settings {
    /// The token to send, when there is one for this origin.
    pub fn token(&self) -> Option<&Token> {
        self.credential.token()
    }
}

/// The token an invocation may send. One value, so a withheld token cannot also be sent.
#[derive(Debug, Clone)]
pub(crate) enum Credential {
    /// No token is configured: neither `HIPPIUS_TOKEN` nor the profile holds one.
    None {
        /// The profile that was read, which the refusal names.
        profile: String,
        /// The config file it was read from.
        path: PathBuf,
    },
    /// `HIPPIUS_TOKEN`, which wins over a stored token.
    Variable(Token),
    /// `HIPPIUS_TOKEN` is set for another origin than this invocation targets, so it is not
    /// sent, and no stored token stands in for it.
    VariableWithheld {
        /// The origin the variable belongs to: `HIPPIUS_TOKEN_ORIGIN`, else the default.
        bound_to: String,
    },
    /// The token the profile stores for this origin.
    Stored(Token),
    /// No token is configured, and the profile `--profile` or `HIPPIUS_PROFILE` names is
    /// not in the config file.
    UnknownProfile {
        /// The profile name, and which of the two named it.
        profile: Given,
        /// The config file.
        path: PathBuf,
    },
    /// The profile stores a token for another origin, which this invocation does not get.
    Withheld {
        /// The origin the stored token belongs to.
        stored_for: String,
    },
}

impl Credential {
    /// The token to send, when there is one for this origin.
    pub(crate) fn token(&self) -> Option<&Token> {
        match self {
            Self::Variable(token) | Self::Stored(token) => Some(token),
            Self::None { .. }
            | Self::VariableWithheld { .. }
            | Self::UnknownProfile { .. }
            | Self::Withheld { .. } => None,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_profile: Option<String>,
    #[serde(default)]
    profiles: BTreeMap<String, Profile>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    api_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    /// The origin `token` was stored for. A hand-written profile without it holds a token
    /// for its own `api_url`, else the default origin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token_origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account: Option<String>,
    /// The selected project, an id of the selected account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    /// The selected billing account, as `project` is stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    billing: Option<String>,
    /// The last Hub subscribe sent from this profile: the double-subscribe marker
    /// (TENETS 11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hub_subscribe: Option<HubMarker>,
}

impl fmt::Debug for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Profile")
            .field("api_url", &self.api_url)
            .field("has_token", &self.token.is_some())
            .field("token_origin", &self.token_origin)
            .field("account", &self.account)
            .field("project", &self.project)
            .field("billing", &self.billing)
            .field("hub_subscribe", &self.hub_subscribe)
            .finish()
    }
}

impl Profile {
    /// Clears the stored token's memory and removes it, so a token replaced or removed is not
    /// left behind in freed memory.
    fn clear_token(&mut self) {
        if let Some(token) = &mut self.token {
            zeroize::Zeroize::zeroize(token);
        }
        self.token = None;
    }
}

/// The stored token is cleared when the profile is dropped, as [`Token`] is.
impl Drop for Profile {
    fn drop(&mut self) {
        self.clear_token();
    }
}

/// A Hub subscribe that was sent: when, as which origin and token, and which paid plans it
/// cancelled first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HubMarker {
    /// When it was sent, in Unix seconds.
    pub at: u64,
    /// Who it was sent as (`Client::identity`: the origin and a token fingerprint), so a
    /// marker from another origin or token on the same profile name does not block this one.
    pub key: String,
    /// The paid subscriptions that run cancels before it subscribes. Inside the window the
    /// list can still show them as active, so a later `--replace` that sees only these has
    /// nothing new to cancel. Left out of the file when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cancels: Vec<u64>,
}

/// The config file, loaded. Change it with [`ConfigFile::edit`].
#[derive(Debug)]
pub struct ConfigFile {
    path: PathBuf,
    document: Document,
}

/// The longest profile name, in characters. Arbitrary: far longer than any name a person
/// types, short enough to keep a mistake such as a pasted token out of the file.
const MAX_PROFILE_NAME_CHARS: usize = 64;

/// Whether `name` is one a new profile may take: 1 to [`MAX_PROFILE_NAME_CHARS`] ASCII
/// letters, digits, `.`, `_`, and `-`. A name goes into the config file as a table key, so one
/// with a line break, a quote, or a path, or an empty one, is refused before it gets there.
fn is_new_profile_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_PROFILE_NAME_CHARS
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The refusal of a profile name outside the rules of [`is_new_profile_name`].
fn invalid_profile_name(given: &Given) -> Error {
    let name = &given.value;
    let example = format!("`{DEFAULT_PROFILE}`");
    let hint = match &given.source {
        Source::Variable(variable) => {
            format!("unset `{variable}`, or set it to a profile name such as {example}")
        }
        // `--profile`, the one other source of a profile name.
        _ => format!("pass a `--profile` such as {example}"),
    };
    Error::usage(
        format!(
            "{}{} is not a profile name: a name is 1 to {MAX_PROFILE_NAME_CHARS} letters, \
             digits, `.`, `_`, or `-`",
            crate::phrase::quote(name),
            given.source.suffix()
        ),
        hint,
    )
}

/// The config path: `HIPPIUS_CONFIG` when it is set and not empty, else
/// `$HOME/.hippius/config.toml`. The caller passes both variables.
///
/// # Errors
///
/// `usage` when neither is set.
pub fn config_path(hippius_config: Option<PathBuf>, home: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = hippius_config.filter(|p| !p.as_os_str().is_empty()) {
        return Ok(path);
    }
    match home.filter(|h| !h.as_os_str().is_empty()) {
        Some(home) => Ok(home.join(".hippius").join("config.toml")),
        None => Err(Error::usage(
            "`HOME` is not set, so the config file has no default path",
            "set `HIPPIUS_CONFIG` to the config file path",
        )),
    }
}

thread_local! {
    /// The config file the last [`ConfigFile::edit`] on this thread rewrote while it had
    /// another hard link, until [`take_split_link`] takes it.
    static SPLIT_LINK: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };

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
    create_private_dirs(dir.parent().unwrap_or(Path::new("")))?;
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        created => created?,
    }
    let fd = rustix::fs::open(dir, DIRECTORY_FLAGS, Mode::empty())?;
    Ok(rustix::fs::fchmod(&fd, Mode::RWXU)?)
}

/// The config file a [`ConfigFile::edit`] on this thread rewrote while it had another hard
/// link, once, or `None`. An edit writes a new file and renames it over the old name, so the
/// other link keeps the old contents, the tokens a logout removed included; the binary warns
/// (tenet 8). Thread-local, so a test sees only its own edits.
pub fn take_split_link() -> Option<PathBuf> {
    SPLIT_LINK.with(|split| split.borrow_mut().take())
}

impl ConfigFile {
    /// Reads the file at `path`. A missing file is an empty config.
    ///
    /// # Errors
    ///
    /// `usage` when the path names no file; when the file is a symlink, not a regular file,
    /// larger than 1 MiB, not UTF-8, owned by another user, or open to the group or others;
    /// when its directory is a symlink, not a directory, owned by another user, or writable
    /// by others; when the file cannot be parsed; or when the system denies the read, with
    /// the `chmod` that fixes it. `internal` when it cannot be read for another reason. The
    /// message names the path and never quotes the file, which holds tokens.
    pub fn load(path: PathBuf) -> Result<Self> {
        let bytes = PrivateFile::load(&path, CONFIG_FILE)?;
        Self::parse(path, bytes)
    }

    /// The config in `bytes`, read from `path`; `None` is a missing file, an empty config.
    fn parse(path: PathBuf, bytes: Option<Vec<u8>>) -> Result<Self> {
        let Some(bytes) = bytes else {
            return Ok(Self {
                path,
                document: Document::default(),
            });
        };
        // Cleared when dropped: the text holds every stored token. The parsed document clears
        // its own copies ([`Profile`]'s `Drop`).
        let text = Zeroizing::new(String::from_utf8(bytes).map_err(|refused| {
            // The bytes the error holds are the file's, tokens included.
            zeroize::Zeroize::zeroize(&mut refused.into_bytes());
            Error::usage(
                format!("the config file {} is not UTF-8 text", quote_path(&path)),
                format!("save {} as UTF-8 text", quote_path(&path)),
            )
        })?);
        let document = toml::from_str(&text).map_err(|e: toml::de::Error| {
            let line = e
                .span()
                .map_or(1, |span| text[..span.start].matches('\n').count() + 1);
            Error::usage(
                format!(
                    "the config file {} is not valid: line {line} {}",
                    quote_path(&path),
                    config_problem(e.message())
                ),
                format!("fix line {line} of {}", quote_path(&path)),
            )
        })?;
        Ok(Self { path, document })
    }

    /// Loads the file at `path`, applies `change`, and saves the result, all under an
    /// exclusive lock, so a concurrent `hipc` cannot overwrite the change with a stale copy.
    /// The directory is created with mode `0700` when it is missing. Waits for the lock
    /// when another `hipc` holds it. The lock, the read, and the write go through one open
    /// descriptor of the checked directory.
    ///
    /// # Errors
    ///
    /// The errors of [`ConfigFile::load`], whatever `change` returns, and, when the
    /// directory cannot be created or the lock or the file cannot be written, `usage` if the
    /// system denied it and `internal` otherwise.
    pub fn edit<T>(path: PathBuf, change: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let file = PrivateFile::ensure(&path, CONFIG_FILE)?;
        let _lock = file.lock()?;
        let mut config = Self::parse(path, file.read()?)?;
        let changed = change(&mut config)?;
        let linked = file.has_other_links();
        config.save(&file)?;
        if linked {
            SPLIT_LINK.with(|split| *split.borrow_mut() = Some(config.path.clone()));
        }
        Ok(changed)
    }

    /// The file's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The profile name the overrides select.
    ///
    /// A name the file already has, as a profile or as its `default_profile`, is taken as it
    /// is, so a hand-edited profile stays usable. Any other name from the overrides must
    /// follow `is_new_profile_name`.
    ///
    /// # Errors
    ///
    /// `usage` for a name from `--profile` or `HIPPIUS_PROFILE` that the file does not have
    /// and that breaks the rules.
    pub fn profile_name(&self, overrides: &Overrides) -> Result<String> {
        let Some(given) = &overrides.profile else {
            return Ok(self
                .document
                .default_profile
                .clone()
                .unwrap_or_else(|| DEFAULT_PROFILE.to_owned()));
        };
        let known = self.document.profiles.contains_key(&given.value)
            || self.document.default_profile.as_ref() == Some(&given.value);
        if known || is_new_profile_name(&given.value) {
            Ok(given.value.clone())
        } else {
            Err(invalid_profile_name(given))
        }
    }

    /// The profile `hipc account projects use` and `hipc account billing-accounts use` store
    /// a selection in.
    ///
    /// # Errors
    ///
    /// The refusal of [`ConfigFile::profile_name`]; and `usage` when `--account` or
    /// `HIPPIUS_ACCOUNT` selects another account than the profile acts in. A stored project
    /// or billing account belongs to the profile's account and is read only in it, so it
    /// would be stored beside an account that does not have it, and this run would not read
    /// it.
    pub fn selection_profile(&self, overrides: &Overrides) -> Result<String> {
        let profile = self.profile_name(overrides)?;
        let Some(given) = &overrides.account else {
            return Ok(profile);
        };
        let account = Selection::parse(&given.value, &given.source)?;
        let stored = self
            .document
            .profiles
            .get(&profile)
            .and_then(|p| p.account.as_deref());
        if account.as_str() == stored.unwrap_or("self") {
            return Ok(profile);
        }
        let acts_in = match stored {
            Some(id) => format!("acts in the account {}", quote(id)),
            None => "acts in the token's own account".to_owned(),
        };
        Err(Error::usage(
            format!(
                "{} selects the account {}, and the profile {} {acts_in}: a project or billing \
                 account is stored for the profile's account",
                given.source,
                quote(account.as_str()),
                quote(&profile)
            ),
            format!("run `hipc account use {}` first", account.as_str()),
        ))
    }

    /// The API origin this invocation targets, and where it came from: the override, else
    /// the profile's `api_url`, else the default, without a trailing slash.
    ///
    /// # Errors
    ///
    /// The refusal of [`ConfigFile::profile_name`].
    pub fn api_url(&self, overrides: &Overrides) -> Result<Given> {
        let profile = self.profile_name(overrides)?;
        let stored = self
            .document
            .profiles
            .get(&profile)
            .and_then(|p| p.api_url.clone())
            .map(|value| Given {
                value,
                source: self.source("api_url", &profile),
            });
        let given = overrides
            .api_url
            .clone()
            .or(stored)
            .unwrap_or_else(|| Given {
                value: DEFAULT_API_URL.to_owned(),
                source: Source::Default,
            });
        // Only the slashes after the authority are trimmed: trimming `https://` whole would
        // leave `https:`, which reads as an origin with no scheme rather than no host.
        let value = match given.value.split_once("://") {
            Some((scheme, rest)) => format!("{scheme}://{}", rest.trim_end_matches('/')),
            None => given.value.trim_end_matches('/').to_owned(),
        };
        Ok(Given {
            value,
            source: given.source,
        })
    }

    /// The source of `field` in `profile` of this file.
    fn source(&self, field: &'static str, profile: &str) -> Source {
        Source::Profile {
            field,
            profile: profile.to_owned(),
            path: self.path.clone(),
        }
    }

    /// Resolves the settings for this invocation.
    ///
    /// # Errors
    ///
    /// `usage` for an account that is neither `self` nor an account id; when `HIPPIUS_TOKEN`
    /// is set, for a `HIPPIUS_TOKEN_ORIGIN` or a target `api_url` that is not an origin;
    /// and, when `HIPPIUS_TOKEN` is unset and the profile stores a token, for a stored `token_origin`
    /// or a target `api_url` that is not an origin, or, when the two origins match, a
    /// stored token that is empty or not visible ASCII. A withheld token is not checked.
    pub fn settings(&self, overrides: &Overrides) -> Result<Settings> {
        let profile = self.profile_name(overrides)?;
        let stored = self.document.profiles.get(&profile);
        let api_url = self.api_url(overrides)?;
        // A stored token goes only to the origin it was stored for, so a changed
        // HIPPIUS_API_URL or --api-url (a repository's .envrc, an agent told to use another
        // host) cannot carry it elsewhere. HIPPIUS_TOKEN is bound the same way, to the
        // default origin, or to the one HIPPIUS_TOKEN_ORIGIN names: `--api-url` alone must
        // not hand a CI secret to another host.
        let credential = match (&overrides.token, stored.and_then(|p| p.token.as_ref())) {
            (Some(env), _) => {
                // The target first, so a bad `--api-url` is named before the variable.
                let target = Origin::parse_from(&api_url.value, &api_url.source)?;
                let bound_to = match &overrides.token_origin {
                    Some(given) => Origin::parse_from(&given.value, &given.source)?,
                    None => Origin::parse(DEFAULT_API_URL)?,
                };
                if target == bound_to {
                    Credential::Variable(env.clone())
                } else {
                    Credential::VariableWithheld {
                        bound_to: bound_to.as_str().to_owned(),
                    }
                }
            }
            (None, Some(token)) => {
                // A hand-written profile without `token_origin` holds a token for its own
                // `api_url`, which sits in the same private file, else the default.
                let stored_for = match stored.and_then(|p| p.token_origin.as_deref()) {
                    Some(origin) => (origin, self.source("token_origin", &profile)),
                    None => match stored.and_then(|p| p.api_url.as_deref()) {
                        Some(origin) => (origin, self.source("api_url", &profile)),
                        None => (DEFAULT_API_URL, Source::Default),
                    },
                };
                let stored_for = Origin::parse_from(stored_for.0, &stored_for.1)?;
                if Origin::parse_from(&api_url.value, &api_url.source)? == stored_for {
                    Credential::Stored(Token::new(token, &self.source("token", &profile))?)
                } else {
                    Credential::Withheld {
                        stored_for: stored_for.as_str().to_owned(),
                    }
                }
            }
            // A profile the flag or the variable names that a file with other profiles
            // does not have is most likely misspelled, which the refusal then says. With
            // no profiles at all, the name is new, and the fix is a login.
            (None, None) => match &overrides.profile {
                Some(given)
                    if !self.document.profiles.is_empty()
                        && !self.document.profiles.contains_key(&given.value) =>
                {
                    Credential::UnknownProfile {
                        profile: given.clone(),
                        path: self.path.clone(),
                    }
                }
                _ => Credential::None {
                    profile: profile.clone(),
                    path: self.path.clone(),
                },
            },
        };
        let stored_account = stored.and_then(|p| p.account.clone()).map(|value| Given {
            value,
            source: self.source("account", &profile),
        });
        let account = match overrides.account.clone().or(stored_account) {
            Some(given) => Selection::parse(&given.value, &given.source)?,
            None => Selection::Own,
        };
        // The same precedence as the account: the flag, then the variable, then the profile.
        // A stored project and billing account belong to the stored account, so they are
        // read only while the run acts in it: under an `--account` or `HIPPIUS_ACCOUNT` that
        // names another, they would name a project that account does not have.
        let stored_in = stored.and_then(|p| p.account.as_deref()).unwrap_or("self");
        let in_stored_account = account.as_str() == stored_in;
        let stored_field = |field, value: Option<&String>| {
            value.filter(|_| in_stored_account).map(|value| Given {
                value: value.clone(),
                source: self.source(field, &profile),
            })
        };
        let project = match overrides
            .project
            .clone()
            .or_else(|| stored_field("project", stored.and_then(|p| p.project.as_ref())))
        {
            Some(given) => ProjectId::parse(&given.value, &given.source)?,
            None => None,
        };
        let billing = match overrides
            .billing
            .clone()
            .or_else(|| stored_field("billing", stored.and_then(|p| p.billing.as_ref())))
        {
            Some(given) => BillingId::parse(&given.value, &given.source)?,
            None => None,
        };
        Ok(Settings {
            profile,
            api_url: api_url.value,
            api_url_source: api_url.source,
            credential,
            account,
            project,
            billing,
        })
    }

    /// Whether `profile` has a stored token.
    pub fn has_token(&self, profile: &str) -> bool {
        self.stored_token(profile).is_some()
    }

    /// Stores `token` in `profile`, for the API origin `api_url`: it will be sent only
    /// there. Returns the origin as stored, normalized.
    ///
    /// # Errors
    ///
    /// `usage` when `api_url` is not a valid API origin.
    pub fn set_token(&mut self, profile: &str, token: &Token, api_url: &str) -> Result<String> {
        let origin = Origin::parse(api_url)?.as_str().to_owned();
        let stored = self.profile_mut(profile);
        stored.clear_token();
        stored.token = Some(token.expose().to_owned());
        stored.token_origin = Some(origin.clone());
        Ok(origin)
    }

    /// Whether `profile` stores a token other than `token`: a login that would replace it. An
    /// empty stored token, which a hand-edited file can hold, is no token, as in
    /// [`ConfigFile::holds_a_stored_token`].
    pub fn stores_other_token(&self, profile: &str, token: &Token) -> bool {
        self.stored_token(profile)
            .is_some_and(|stored| !stored.is_empty() && stored != token.expose())
    }

    /// Whether `text` holds a token any profile of the file stores, in any spelling
    /// [`Token::is_in`] knows, whatever origin it is stored for.
    pub fn holds_a_stored_token(&self, text: &str) -> bool {
        self.document
            .profiles
            .values()
            .filter_map(|profile| profile.token.as_deref())
            .any(|token| quoted_in(token, text))
    }

    /// Whether `profile` stores the same token here as in `earlier`, an older load of the
    /// file, or no token in both: for a change confirmed against `earlier` that must not
    /// touch a token stored since.
    pub fn same_token(&self, profile: &str, earlier: &Self) -> bool {
        self.stored_token(profile) == earlier.stored_token(profile)
    }

    fn stored_token(&self, profile: &str) -> Option<&str> {
        self.document
            .profiles
            .get(profile)
            .and_then(|p| p.token.as_deref())
    }

    /// Removes the token from `profile`. Other fields stay.
    pub fn clear_token(&mut self, profile: &str) {
        if let Some(stored) = self.document.profiles.get_mut(profile) {
            stored.clear_token();
            stored.token_origin = None;
        }
    }

    /// The last Hub subscribe sent from `profile`.
    pub fn hub_marker(&self, profile: &str) -> Option<&HubMarker> {
        self.document
            .profiles
            .get(profile)
            .and_then(|p| p.hub_subscribe.as_ref())
    }

    /// The last Hub subscribe sent from each profile that has one, with the profile's name:
    /// a subscribe guard reads them all, since two profiles can hold the same token.
    pub fn hub_markers(&self) -> impl Iterator<Item = (&str, &HubMarker)> {
        self.document
            .profiles
            .iter()
            .filter_map(|(name, p)| Some((name.as_str(), p.hub_subscribe.as_ref()?)))
    }

    /// Records the last Hub subscribe sent from `profile`, or clears it with `None`.
    pub fn set_hub_marker(&mut self, profile: &str, marker: Option<HubMarker>) {
        self.profile_mut(profile).hub_subscribe = marker;
    }

    /// Stores the account selection in `profile`. `self` removes the field. A project and a
    /// billing account belong to one account, so a stored one is removed when the account
    /// changes: kept, it would name a project the new account does not have.
    pub fn set_account(&mut self, profile: &str, account: &Selection) {
        let stored = self.profile_mut(profile);
        let account = match account {
            Selection::Own => None,
            Selection::Member(id) => Some(id.as_str().to_owned()),
        };
        if stored.account != account {
            stored.project = None;
            stored.billing = None;
        }
        stored.account = account;
    }

    /// Stores the project selection in `profile`. The Default project (`None`) removes the
    /// field.
    pub fn set_project(&mut self, profile: &str, project: Option<&ProjectId>) {
        self.profile_mut(profile).project = project.map(|id| id.as_str().to_owned());
    }

    /// Clears the project stored in `profile` when it is still `project`, and says whether it
    /// did. Inside [`ConfigFile::edit`] this is a compare-and-swap: a selection another run
    /// stored after `project` was read is kept, not undone.
    pub fn clear_project_if(&mut self, profile: &str, project: &str) -> bool {
        let Some(stored) = self.document.profiles.get_mut(profile) else {
            return false;
        };
        if stored.project.as_deref() != Some(project) {
            return false;
        }
        stored.project = None;
        true
    }

    /// Stores the billing account selection in `profile`. The Default billing account
    /// (`None`) removes the field.
    pub fn set_billing(&mut self, profile: &str, billing: Option<&BillingId>) {
        self.profile_mut(profile).billing = billing.map(|id| id.as_str().to_owned());
    }

    fn profile_mut(&mut self, profile: &str) -> &mut Profile {
        self.document
            .profiles
            .entry(profile.to_owned())
            .or_default()
    }

    /// Writes the document to `file` ([`PrivateFile::write`] says which mode it gets).
    fn save(&self, file: &PrivateFile) -> Result<()> {
        let text = toml::to_string(&self.document)
            .map(Zeroizing::new)
            .map_err(|e| Error::internal(format!("cannot encode the config file: {e}")))?;
        file.write(text.as_bytes())
    }
}

/// What is wrong on a line of the config file, from the parser's `message`, as the end of a
/// sentence that starts with the line: `has a key other than ...`.
///
/// The parser's own message quotes the key or the value it refused, either of which can be a
/// token pasted in the wrong place, so none of it is repeated. Only what serde writes from the
/// config types themselves is: the field names a table takes, and the name of a missing one.
/// A message this does not know is `does not parse`.
fn config_problem(message: &str) -> String {
    // The text between each pair of backticks: `` unknown field `x`, expected `a` or `b` ``
    // gives `x`, `a`, and `b`.
    let quoted = |text: &str| -> Vec<String> {
        text.split('`')
            .skip(1)
            .step_by(2)
            .map(crate::phrase::quote)
            .collect()
    };
    if let Some((_, expected)) = message
        .strip_prefix("unknown field ")
        .and_then(|rest| rest.rsplit_once(", expected "))
    {
        return format!(
            "has a key other than {}",
            crate::phrase::join(&quoted(expected), "or")
        );
    }
    if let Some((_, expected)) = message
        .strip_prefix("invalid type: ")
        .and_then(|rest| rest.rsplit_once(", expected "))
    {
        let kind = match expected {
            "a string" => "a string",
            "u64" => "a whole number",
            "a sequence" => "a list",
            // A table the config types hold, such as a profile.
            _ => "a table",
        };
        return format!("has a value that is not {kind}");
    }
    match message
        .strip_prefix("missing field ")
        .map(quoted)
        .as_deref()
    {
        Some([field]) => format!("lacks the key {field}"),
        _ => "does not parse".to_owned(),
    }
}

/// The directory a config path lives in.
pub(crate) fn directory(path: &Path) -> &Path {
    path.parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
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
    /// The start of a hint that names another path, before the path: "set `HIPPIUS_CONFIG`
    /// to", or "pass `--cluster-key-file`".
    fn point_at(self) -> String {
        match self {
            Self::Variable(name) => format!("set `{name}` to"),
            Self::Flag(name) => format!("pass `{name}`"),
        }
    }
}

/// The config file.
pub(crate) const CONFIG_FILE: Guarded = Guarded {
    what: "the config file",
    holds: "API tokens",
    named_by: NamedBy::Variable("HIPPIUS_CONFIG"),
};

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
const MAX_FILE_BYTES: usize = 1 << 20;

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

    /// A fresh directory with mode 0700, whatever the umask. Ubuntu's default umask, 0002,
    /// would leave it writable by the group, which hipc refuses on purpose.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hipc-config-{name}-{}", std::process::id()));
        drop(fs::remove_dir_all(&dir));
        fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    /// A regular file root owns, in a directory that is not a symlink, so that opening it
    /// is refused for its owner and not for its shape. `/usr/bin/env` is a symlink on some
    /// Linux hosts; `/etc` is one on macOS, where `/private/etc` is the directory.
    fn a_root_owned_regular_file() -> PathBuf {
        const CANDIDATES: [&str; 5] = [
            "/usr/bin/env",
            "/etc/passwd",
            "/etc/hosts",
            "/private/etc/passwd",
            "/private/etc/hosts",
        ];
        CANDIDATES
            .into_iter()
            .map(PathBuf::from)
            .find(|path| {
                fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.uid() == 0)
                    && fs::symlink_metadata(path.parent().unwrap()).is_ok_and(|m| m.is_dir())
            })
            .expect("no root-owned regular file in a real directory among CANDIDATES")
    }

    /// Saves `config` to its path without the lock, as one step of [`ConfigFile::edit`].
    fn save(config: &ConfigFile) -> Result<()> {
        config.save(&PrivateFile::ensure(&config.path, CONFIG_FILE)?)
    }

    #[test]
    fn a_stored_token_for_another_origin_is_withheld_not_sent() {
        let mut file = ConfigFile {
            path: PathBuf::from("unused"),
            document: Document::default(),
        };
        file.set_token(
            "default",
            &Token::new("stored-token", &Source::Argument).unwrap(),
            DEFAULT_API_URL,
        )
        .unwrap();
        let elsewhere = Overrides {
            api_url: Some(Given {
                value: "https://staging.example".into(),
                source: Source::Flag("--api-url"),
            }),
            ..Overrides::default()
        };
        let settings = file.settings(&elsewhere).unwrap();
        assert!(
            settings.token().is_none(),
            "the stored token must not go elsewhere"
        );
        assert!(matches!(
            settings.credential,
            Credential::Withheld { ref stored_for } if stored_for == DEFAULT_API_URL
        ));
    }

    #[test]
    fn a_token_must_be_one_word_of_visible_ascii() {
        assert_eq!(
            Token::new(" tok_abc \n", &Source::Argument)
                .unwrap()
                .expose(),
            "tok_abc"
        );
        for bad in ["tok_a\ntok_b", "tok a", "tok\u{7f}", "tök"] {
            assert_eq!(
                Token::new(bad, &Source::Argument).unwrap_err().code(),
                crate::Code::Usage,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn path_is_hippius_config_else_dot_hippius_in_home() {
        let explicit = config_path(Some("/x/c.toml".into()), Some("/home/u".into())).unwrap();
        assert_eq!(explicit, PathBuf::from("/x/c.toml"));
        let home = config_path(None, Some("/home/u".into())).unwrap();
        assert_eq!(home, PathBuf::from("/home/u/.hippius/config.toml"));
        let empty = config_path(Some(PathBuf::new()), Some("/home/u".into())).unwrap();
        assert_eq!(empty, home);
        assert!(config_path(None, None).is_err());
        assert!(config_path(None, Some(PathBuf::new())).is_err());
    }

    #[test]
    fn flag_then_env_then_profile() {
        let mut file = ConfigFile {
            path: PathBuf::from("unused"),
            document: Document::default(),
        };
        file.set_token(
            "default",
            &Token::new("stored-token", &Source::Argument).unwrap(),
            DEFAULT_API_URL,
        )
        .unwrap();
        let settings = file.settings(&Overrides::default()).unwrap();
        assert_eq!(settings.token().unwrap().expose(), "stored-token");
        assert_eq!(settings.api_url, DEFAULT_API_URL);
        assert!(matches!(settings.account, Selection::Own));

        let overrides = Overrides {
            token: Some(Token::new(" env-token\n", &Source::Argument).unwrap()),
            api_url: Some(Given {
                value: "http://127.0.0.1:9/".into(),
                source: Source::Flag("--api-url"),
            }),
            ..Overrides::default()
        };
        // Bound to the default origin, the variable is not sent to another, and the stored
        // token does not stand in for it.
        let settings = file.settings(&overrides).unwrap();
        assert!(settings.token().is_none());
        assert!(matches!(
            settings.credential,
            Credential::VariableWithheld { ref bound_to } if bound_to == DEFAULT_API_URL
        ));
        assert_eq!(settings.api_url, "http://127.0.0.1:9");
        // `HIPPIUS_TOKEN_ORIGIN` names the target, in any spelling of it.
        for spelling in [
            "http://127.0.0.1:9",
            "HTTP://127.0.0.1:9/",
            "http://127.0.0.1:09",
        ] {
            let bound = Overrides {
                token: overrides.token.clone(),
                api_url: overrides.api_url.clone(),
                token_origin: Some(Given {
                    value: spelling.into(),
                    source: Source::Variable("HIPPIUS_TOKEN_ORIGIN"),
                }),
                ..Overrides::default()
            };
            let settings = file.settings(&bound).unwrap();
            assert_eq!(
                settings.token().unwrap().expose(),
                "env-token",
                "{spelling}"
            );
        }
    }

    /// `HIPPIUS_TOKEN` goes only to the origin `HIPPIUS_TOKEN_ORIGIN` names, else the default
    /// one: another target, a port, a scheme, or a host off is withheld, and a value that is
    /// no origin is refused, naming the variable.
    #[test]
    fn the_token_variable_is_sent_only_to_the_origin_it_is_bound_to() {
        let file = ConfigFile {
            path: PathBuf::from("unused"),
            document: Document::default(),
        };
        let run = |target: Option<&str>, bound: Option<&str>| {
            file.settings(&Overrides {
                token: Some(Token::new("env-token", &Source::Argument).unwrap()),
                api_url: target.map(|value| given(value, "--api-url")),
                token_origin: bound.map(|value| Given {
                    value: value.to_owned(),
                    source: Source::Variable("HIPPIUS_TOKEN_ORIGIN"),
                }),
                ..Overrides::default()
            })
        };
        let sent = |target, bound| run(target, bound).unwrap().token().is_some();
        assert!(sent(None, None));
        assert!(sent(Some("https://API.hippius.com:443/"), None));
        assert!(sent(None, Some("https://api.hippius.com")));
        assert!(!sent(Some("https://collector.example"), None));
        assert!(!sent(
            Some("https://collector.example"),
            Some("https://other.example")
        ));
        assert!(!sent(
            Some("http://127.0.0.1:8"),
            Some("http://127.0.0.1:9")
        ));
        assert!(!sent(
            Some("http://127.0.0.1:9"),
            Some("https://127.0.0.1:9")
        ));
        // Bound elsewhere, the variable is withheld from the default origin too.
        assert!(!sent(None, Some("http://127.0.0.1:9")));
        assert!(sent(
            Some("https://collector.example/"),
            Some("HTTPS://Collector.example")
        ));
        let error = run(Some("https://collector.example"), Some("collector.example")).unwrap_err();
        assert_eq!(error.code(), crate::Code::Usage);
        let message = error.message();
        assert!(message.contains("`HIPPIUS_TOKEN_ORIGIN`"), "{message}");
    }

    const PROJECT: &str = "proj_0a1b2c3d4e5f60718293";
    const BILLING: &str = "bill_9f8e7d6c5b4a39281706";

    fn given(value: &str, flag: &'static str) -> Given {
        Given {
            value: value.to_owned(),
            source: Source::Flag(flag),
        }
    }

    /// The project and the billing account are picked as the account is: an override, then
    /// the profile, then the Default one; `default` from an override wins over a stored id.
    #[test]
    fn the_project_and_billing_account_come_from_an_override_then_the_profile() {
        let mut file = ConfigFile {
            path: PathBuf::from("unused"),
            document: Document::default(),
        };
        let settings = file.settings(&Overrides::default()).unwrap();
        assert!(settings.project.is_none() && settings.billing.is_none());

        let project = ProjectId::parse(PROJECT, &Source::Argument).unwrap();
        let billing = BillingId::parse(BILLING, &Source::Argument).unwrap();
        file.set_project("default", project.as_ref());
        file.set_billing("default", billing.as_ref());
        let settings = file.settings(&Overrides::default()).unwrap();
        assert_eq!(settings.project.unwrap().as_str(), PROJECT);
        assert_eq!(settings.billing.unwrap().as_str(), BILLING);

        let overrides = Overrides {
            project: Some(given("default", "--project")),
            billing: Some(given("default", "--billing")),
            ..Overrides::default()
        };
        let settings = file.settings(&overrides).unwrap();
        assert!(settings.project.is_none() && settings.billing.is_none());

        let other = "proj_ffffffffffffffffffff";
        let overrides = Overrides {
            project: Some(given(other, "--project")),
            ..Overrides::default()
        };
        assert_eq!(
            file.settings(&overrides).unwrap().project.unwrap().as_str(),
            other
        );

        file.set_project("default", None);
        file.set_billing("default", None);
        let settings = file.settings(&Overrides::default()).unwrap();
        assert!(settings.project.is_none() && settings.billing.is_none());
    }

    /// A malformed id is refused whether an override or the profile holds it, and the
    /// refusal names the one that did.
    #[test]
    fn a_malformed_project_or_billing_account_names_where_it_came_from() {
        let mut file = ConfigFile {
            path: PathBuf::from("/h/config.toml"),
            document: Document::default(),
        };
        let overrides = Overrides {
            billing: Some(given("bill_x", "--billing")),
            ..Overrides::default()
        };
        let error = file.settings(&overrides).unwrap_err();
        assert!(error.message().contains("from `--billing`"), "{error}");
        file.profile_mut("default").project = Some("proj_x".to_owned());
        let error = file.settings(&Overrides::default()).unwrap_err();
        assert!(
            error
                .message()
                .contains("from the `project` of the profile `default`"),
            "{error}"
        );
    }

    /// A project and a billing account belong to the account they were picked in, so a
    /// new account removes them, and picking the same account again keeps them.
    #[test]
    fn a_new_account_clears_the_stored_project_and_billing_account() {
        let mut file = ConfigFile {
            path: PathBuf::from("unused"),
            document: Document::default(),
        };
        let member = Selection::parse("acct_0123456789abcdef0123", &Source::Argument).unwrap();
        let project = ProjectId::parse(PROJECT, &Source::Argument).unwrap();
        let billing = BillingId::parse(BILLING, &Source::Argument).unwrap();
        let pick = |file: &mut ConfigFile| {
            file.set_project("default", project.as_ref());
            file.set_billing("default", billing.as_ref());
        };
        pick(&mut file);
        file.set_account("default", &Selection::Own);
        assert!(
            file.settings(&Overrides::default())
                .unwrap()
                .project
                .is_some()
        );
        file.set_account("default", &member);
        let settings = file.settings(&Overrides::default()).unwrap();
        assert!(settings.project.is_none() && settings.billing.is_none());
        pick(&mut file);
        file.set_account("default", &member);
        let settings = file.settings(&Overrides::default()).unwrap();
        assert!(settings.project.is_some() && settings.billing.is_some());
        file.set_account("default", &Selection::Own);
        let settings = file.settings(&Overrides::default()).unwrap();
        assert!(settings.project.is_none() && settings.billing.is_none());
    }

    /// A project is cleared only while the profile still stores it: another project, a
    /// profile without one, and a profile that does not exist are left as they are.
    #[test]
    fn a_project_is_cleared_only_while_it_is_still_stored() {
        let mut file = ConfigFile {
            path: PathBuf::from("unused"),
            document: Document::default(),
        };
        let stored = |file: &ConfigFile| file.document.profiles["default"].project.clone();
        assert!(!file.clear_project_if("default", PROJECT));
        file.set_project("default", None);
        assert!(!file.clear_project_if("default", PROJECT));
        let project = ProjectId::parse(PROJECT, &Source::Argument).unwrap();
        file.set_project("default", project.as_ref());
        assert!(!file.clear_project_if("default", "proj_ffffffffffffffffffff"));
        assert!(!file.clear_project_if("other", PROJECT));
        assert_eq!(stored(&file).as_deref(), Some(PROJECT));
        assert!(file.clear_project_if("default", PROJECT));
        assert_eq!(stored(&file), None);
    }

    /// The stored project and billing account are read only while the run acts in the
    /// stored account: an override that names another account leaves them unread, and one
    /// that names the stored account, `self` for none, keeps them.
    #[test]
    fn another_account_override_leaves_the_stored_project_unread() {
        let mut file = ConfigFile {
            path: PathBuf::from("unused"),
            document: Document::default(),
        };
        let project = ProjectId::parse(PROJECT, &Source::Argument).unwrap();
        let billing = BillingId::parse(BILLING, &Source::Argument).unwrap();
        file.set_project("default", project.as_ref());
        file.set_billing("default", billing.as_ref());
        let other = "acct_ffffffffffffffffffff";
        let run_in = |account: &str| {
            let overrides = Overrides {
                account: Some(given(account, "--account")),
                ..Overrides::default()
            };
            let settings = file.settings(&overrides).unwrap();
            (settings.project.is_some(), settings.billing.is_some())
        };
        assert_eq!(run_in("self"), (true, true));
        assert_eq!(run_in(other), (false, false));
        // A flag still selects a project in any account.
        let overrides = Overrides {
            account: Some(given(other, "--account")),
            project: Some(given(PROJECT, "--project")),
            ..Overrides::default()
        };
        assert!(file.settings(&overrides).unwrap().project.is_some());
    }

    /// A project or billing account is stored only for the account the profile acts in:
    /// an override that names another is refused, by flag or by variable, and one that names
    /// the profile's account, `self` for none, is not.
    #[test]
    fn a_selection_is_stored_only_for_the_profiles_account() {
        let mut file = ConfigFile {
            path: PathBuf::from("unused"),
            document: Document::default(),
        };
        let (member, other) = ("acct_0123456789abcdef0123", "acct_ffffffffffffffffffff");
        let with = |account: Option<Given>| Overrides {
            account,
            ..Overrides::default()
        };
        assert_eq!(file.selection_profile(&with(None)).unwrap(), "default");
        let own = with(Some(given("self", "--account")));
        assert_eq!(file.selection_profile(&own).unwrap(), "default");
        let error = file
            .selection_profile(&with(Some(given(member, "--account"))))
            .unwrap_err();
        assert_eq!(error.code().as_str(), "usage");
        assert_eq!(
            error.message(),
            format!(
                "`--account` selects the account `{member}`, and the profile `default` acts in \
                 the token's own account: a project or billing account is stored for the \
                 profile's account"
            )
        );
        assert_eq!(
            error.hint(),
            Some(format!("run `hipc account use {member}` first").as_str())
        );

        file.set_account(
            "default",
            &Selection::parse(member, &Source::Argument).unwrap(),
        );
        let same = with(Some(given(member, "--account")));
        assert_eq!(file.selection_profile(&same).unwrap(), "default");
        assert_eq!(file.selection_profile(&with(None)).unwrap(), "default");
        let variable = Given {
            value: other.to_owned(),
            source: Source::Variable("HIPPIUS_ACCOUNT"),
        };
        let error = file.selection_profile(&with(Some(variable))).unwrap_err();
        assert_eq!(
            error.message(),
            format!(
                "`HIPPIUS_ACCOUNT` selects the account `{other}`, and the profile `default` acts \
                 in the account `{member}`: a project or billing account is stored for the \
                 profile's account"
            )
        );
        let error = file.selection_profile(&own).unwrap_err();
        assert!(
            error
                .message()
                .starts_with("`--account` selects the account `self`"),
            "{error}"
        );
        assert_eq!(error.hint(), Some("run `hipc account use self` first"));
        // A malformed override is refused as `settings` refuses it.
        let error = file
            .selection_profile(&with(Some(given("acct_x", "--account"))))
            .unwrap_err();
        assert!(
            error.message().starts_with("`acct_x` from `--account`"),
            "{error}"
        );
    }

    #[test]
    fn save_writes_mode_0600_in_a_0700_directory_and_round_trips() {
        let dir = temp_dir("save");
        let path = dir.join("hipc").join("config.toml");
        let mut file = ConfigFile::load(path.clone()).unwrap();
        file.set_token(
            "work",
            &Token::new("t0ken", &Source::Argument).unwrap(),
            DEFAULT_API_URL,
        )
        .unwrap();
        save(&file).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let refused = ConfigFile::load(path.clone()).unwrap_err();
        assert_eq!(refused.code(), crate::Code::Usage);
        assert!(refused.hint().unwrap().contains("chmod 600"));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let mut file = ConfigFile::load(path.clone()).unwrap();
        assert!(file.has_token("work"));
        file.clear_token("work");
        save(&file).unwrap();
        assert_eq!(mode(&path), 0o600);
        assert!(!ConfigFile::load(path).unwrap().has_token("work"));
        fs::remove_dir_all(dir).unwrap();
    }

    /// A change keeps the owner's mode the file has now: a file the user made read-only
    /// stays read-only, and no change widens a mode.
    #[test]
    fn a_change_keeps_the_owner_mode_the_file_has() {
        let dir = temp_dir("keep-mode");
        let path = dir.join("hipc").join("config.toml");
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        save(&ConfigFile::load(path.clone()).unwrap()).unwrap();
        for kept in [0o400, 0o700, 0o600] {
            fs::set_permissions(&path, fs::Permissions::from_mode(kept)).unwrap();
            ConfigFile::edit(path.clone(), |config| {
                config.set_account("default", &Selection::Own);
                Ok(())
            })
            .unwrap();
            assert_eq!(mode(&path), kept, "{kept:o}");
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_directory_others_can_write_is_refused_for_reads_and_writes() {
        let dir = temp_dir("shared");
        let path = dir.join("config.toml");
        let mut file = ConfigFile::load(path.clone()).unwrap();
        file.set_token(
            "default",
            &Token::new("t", &Source::Argument).unwrap(),
            DEFAULT_API_URL,
        )
        .unwrap();
        save(&file).unwrap();
        for mode in [0o770, 0o707, 0o777] {
            fs::set_permissions(&dir, fs::Permissions::from_mode(mode)).unwrap();
            let read = ConfigFile::load(path.clone()).unwrap_err();
            assert!(read.hint().unwrap().contains("chmod 700"), "{mode:o}");
            let write = save(&file).unwrap_err();
            assert!(write.hint().unwrap().contains("chmod 700"), "{mode:o}");
        }
        // Readable by others is not a reason to refuse the directory, only writable.
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        ConfigFile::load(path.clone()).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn every_mode_bit_for_others_or_the_group_is_refused() {
        let dir = temp_dir("modes");
        let path = dir.join("config.toml");
        save(&ConfigFile::load(path.clone()).unwrap()).unwrap();
        for bit in [0o040, 0o020, 0o010, 0o004, 0o002, 0o001] {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600 | bit)).unwrap();
            assert!(ConfigFile::load(path.clone()).is_err(), "{:o}", 0o600 | bit);
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
        ConfigFile::load(path.clone()).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn save_replaces_a_stale_temporary_file_and_never_follows_a_symlink() {
        let dir = temp_dir("stale");
        let path = dir.join("config.toml");
        let temporary = dir.join(format!(".config.toml.{}.tmp", std::process::id()));
        let target = dir.join("elsewhere");
        std::os::unix::fs::symlink(&target, &temporary).unwrap();
        let mut file = ConfigFile::load(path.clone()).unwrap();
        file.set_token(
            "default",
            &Token::new("t", &Source::Argument).unwrap(),
            DEFAULT_API_URL,
        )
        .unwrap();
        save(&file).unwrap();
        assert!(!target.exists(), "the write followed the planted symlink");
        assert!(!temporary.exists());
        assert!(ConfigFile::load(path).unwrap().has_token("default"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_missing_file_is_an_empty_config_and_a_directory_is_refused() {
        let dir = temp_dir("missing");
        let file = ConfigFile::load(dir.join("nope.toml")).unwrap();
        assert!(!file.has_token("default"));
        assert_eq!(file.path(), dir.join("nope.toml"));
        // A directory that does not exist yet is the same empty config.
        let file = ConfigFile::load(dir.join("absent").join("nope.toml")).unwrap();
        assert!(!file.has_token("default"));
        fs::create_dir_all(dir.join("adir.toml")).unwrap();
        fs::set_permissions(dir.join("adir.toml"), fs::Permissions::from_mode(0o700)).unwrap();
        let error = ConfigFile::load(dir.join("adir.toml")).unwrap_err();
        assert_eq!(error.code(), crate::Code::Usage);
        assert!(error.to_string().contains("not a regular file"), "{error}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_symlinked_config_file_is_refused_and_its_target_is_not_read() {
        let dir = temp_dir("symlink");
        let target = dir.join("real.toml");
        fs::write(&target, "").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.join("config.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let error = ConfigFile::load(link).unwrap_err();
        assert!(error.to_string().contains("symlink"), "{error}");
        assert_eq!(error.code(), crate::Code::Usage, "{error}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_file_or_directory_owned_by_another_user_is_refused() {
        // A file root owns, chosen for the host, and `/usr`, which root owns on every macOS
        // and Linux host. The suite does not run as root: root owns them too, and root also
        // reads a mode-000 file.
        assert!(
            !rustix::process::geteuid().is_root(),
            "run the tests as a normal user"
        );
        let error = ConfigFile::load(a_root_owned_regular_file()).unwrap_err();
        assert!(error.to_string().contains("another user"), "{error}");
        assert_eq!(error.code(), crate::Code::Usage, "{error}");
        let mut file = ConfigFile {
            path: PathBuf::from("/usr/hipc-config-test.toml"),
            document: Document::default(),
        };
        file.set_token(
            "default",
            &Token::new("t", &Source::Argument).unwrap(),
            DEFAULT_API_URL,
        )
        .unwrap();
        let error = save(&file).unwrap_err();
        assert!(error.to_string().contains("another user"), "{error}");
        assert_eq!(error.code(), crate::Code::Usage, "{error}");
        assert!(!Path::new("/usr/hipc-config-test.toml").exists());
    }

    #[test]
    fn a_file_that_is_not_utf8_is_a_usage_error() {
        let dir = temp_dir("not-utf8");
        let path = dir.join("config.toml");
        fs::write(&path, b"[profiles.default]\ntoken = \"\xff\"\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let error = ConfigFile::load(path.clone()).unwrap_err();
        assert_eq!(error.code(), crate::Code::Usage);
        let quoted = format!("`{}`", path.display());
        assert_eq!(
            error.to_string(),
            format!("the config file {quoted} is not UTF-8 text")
        );
        assert_eq!(
            error.hint(),
            Some(format!("save {quoted} as UTF-8 text").as_str())
        );
        fs::remove_dir_all(dir).unwrap();
    }

    /// A writer killed between its temporary file and the rename leaves a full copy of the
    /// tokens beside the config, under its own pid, which no later run would reuse. The next
    /// edit removes it under the lock, and nothing else.
    #[test]
    fn an_edit_removes_the_temporary_files_a_crashed_writer_left() {
        let dir = temp_dir("stale-tmp");
        let stale = [".config.toml.1.tmp", ".config.toml.4294967295.tmp"];
        let kept = [
            ".config.toml.x.tmp",
            ".config.toml..tmp",
            ".other.toml.1.tmp",
            "config.toml.1.tmp",
            ".config.toml.1.tmp.bak",
        ];
        for name in stale.iter().chain(&kept) {
            fs::write(dir.join(name), "token = \"t0ken\"").unwrap();
        }
        store_token(&dir.join("config.toml")).unwrap();
        for name in stale {
            assert!(!dir.join(name).exists(), "{name} is left");
        }
        for name in kept {
            assert!(dir.join(name).exists(), "{name} was removed");
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_write_leaves_no_temporary_file_and_reports_the_path() {
        let dir = temp_dir("failed");
        fs::create_dir_all(dir.join("config.toml")).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        // The rename fails: the target is a directory that is not empty.
        fs::write(dir.join("config.toml").join("keep"), "").unwrap();
        let file = ConfigFile {
            path: dir.join("config.toml"),
            document: Document::default(),
        };
        let error = save(&file).unwrap_err();
        assert_eq!(error.code(), crate::Code::Internal);
        assert!(error.to_string().contains("config.toml"), "{error}");
        let leftovers: Vec<_> = fs::read_dir(&dir).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "only the directory remains");
        // The create fails: the directory is read-only, which a `chmod` fixes.
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        let other = ConfigFile {
            path: dir.join("other.toml"),
            document: Document::default(),
        };
        let error = save(&other).unwrap_err();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(error.code(), crate::Code::Usage);
        assert_eq!(
            error.hint(),
            Some(&*format!("run `chmod 700 {}`", dir.display()))
        );
        fs::remove_dir_all(dir).unwrap();
    }

    /// A key or a value the config types do not take is named for what is wrong with it,
    /// in `hipc`'s words, and never quoted: the key or the value can be a token.
    #[test]
    fn a_config_error_says_what_is_wrong_without_quoting_the_file() {
        for (text, problem) in [
            (
                "[profiles.default]\ntoken = \"x\"\nfoo = 1\n",
                "line 3 has a key other than `api_url`, `token`, `token_origin`, `account`, \
                 `project`, `billing`, or `hub_subscribe`",
            ),
            (
                "tok_s3cr3t = 1\n",
                "line 1 has a key other than `default_profile` or `profiles`",
            ),
            // A key that spells serde's own words cannot get its text into the list.
            (
                "\"x`, expected `s3cr3t\" = 1\n",
                "line 1 has a key other than `default_profile` or `profiles`",
            ),
            (
                "[profiles.default]\ntoken = 5\n",
                "line 2 has a value that is not a string",
            ),
            (
                "[profiles]\ndefault = \"s3cr3t\"\n",
                "line 2 has a value that is not a table",
            ),
            (
                "[profiles.default.hub_subscribe]\nat = \"s3cr3t\"\nkey = \"k\"\n",
                "line 2 has a value that is not a whole number",
            ),
            (
                "[profiles.default.hub_subscribe]\nat = 1\nkey = \"k\"\ncancels = 1\n",
                "line 4 has a value that is not a list",
            ),
            (
                "[profiles.default.hub_subscribe]\nat = 1\n",
                "line 1 lacks the key `key`",
            ),
            ("token = \"s3cr3t\" [[[", "line 1 does not parse"),
            (
                "default_profile = 3\n",
                "line 1 has a value that is not a string",
            ),
            (
                "default_profile = 1979-05-27\n",
                "line 1 has a value that is not a string",
            ),
        ] {
            let error = ConfigFile::parse(PathBuf::from("c.toml"), Some(text.into())).unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("the config file `c.toml` is not valid: {problem}"),
                "{text}"
            );
            assert!(!error.to_string().contains("s3cr3t"), "{error}");
        }
    }

    #[test]
    fn debug_never_prints_a_token() {
        let token = Token::new("super-secret-value", &Source::Argument).unwrap();
        assert!(!format!("{token:?}").contains("secret"));
        let mut file = ConfigFile {
            path: PathBuf::from("unused"),
            document: Document::default(),
        };
        file.set_token("default", &token, DEFAULT_API_URL).unwrap();
        assert!(!format!("{file:?}").contains("secret"));
        let overrides = Overrides {
            token: Some(token.clone()),
            ..Overrides::default()
        };
        assert!(!format!("{overrides:?}").contains("secret"));
        assert_eq!(token.preview().as_deref(), Some("alue"));
        assert_eq!(
            Token::new("12345678901", &Source::Argument)
                .unwrap()
                .preview(),
            None
        );
        assert_eq!(
            Token::new("123456789012", &Source::Argument)
                .unwrap()
                .preview()
                .as_deref(),
            Some("9012")
        );
    }

    #[test]
    fn a_bad_file_names_the_path_and_not_the_contents() {
        let dir = temp_dir("bad");
        let path = dir.join("config.toml");
        fs::write(&path, "token = \"leak-me\"\nnot toml [").unwrap();
        // Mode 0600, or the mode refusal fires first and the parse is never reached.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let error = ConfigFile::load(path).unwrap_err();
        assert!(error.to_string().contains("config.toml"));
        assert!(error.to_string().contains("not valid"), "{error}");
        assert!(!error.to_string().contains("leak-me"));
        fs::remove_dir_all(dir).unwrap();
    }

    /// Stores a token through [`ConfigFile::edit`], the path every command that writes
    /// takes.
    fn store_token(path: &Path) -> Result<()> {
        ConfigFile::edit(path.to_owned(), |config| {
            let token = Token::new("t0ken", &Source::Argument)?;
            config
                .set_token("default", &token, DEFAULT_API_URL)
                .map(drop)
        })
    }

    /// An edit of a file with another hard link is noted once, for the binary's warning: the
    /// rename leaves the old contents, the token included, in the other link. A file with one
    /// link, and a new file, are not noted.
    #[test]
    fn an_edit_of_a_hard_linked_config_is_noted_once() {
        let dir = temp_dir("hard-link");
        let path = dir.join("config.toml");
        drop(take_split_link());
        store_token(&path).unwrap();
        assert_eq!(take_split_link(), None, "a new file");
        store_token(&path).unwrap();
        assert_eq!(take_split_link(), None, "one link");
        let other = dir.join("dotfiles-copy.toml");
        fs::hard_link(&path, &other).unwrap();
        store_token(&path).unwrap();
        assert_eq!(take_split_link(), Some(path.clone()));
        assert_eq!(take_split_link(), None, "taken once");
        assert!(fs::read_to_string(&other).unwrap().contains("t0ken"));
        // The rename split the link, so the next edit has one again.
        store_token(&path).unwrap();
        assert_eq!(take_split_link(), None);
    }

    #[test]
    fn a_symlinked_config_directory_is_refused_for_reads_and_writes() {
        let dir = temp_dir("linked-dir");
        let real = dir.join("real");
        fs::create_dir_all(&real).unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
        store_token(&real.join("config.toml")).unwrap();
        let before = fs::read(real.join("config.toml")).unwrap();
        std::os::unix::fs::symlink(&real, dir.join(".hippius")).unwrap();
        let path = dir.join(".hippius").join("config.toml");
        let hint = "set `HIPPIUS_CONFIG` to a file in the directory the symlink points to";

        let read = ConfigFile::load(path.clone()).unwrap_err();
        assert_eq!(read.code(), crate::Code::Usage, "{read}");
        assert!(read.to_string().contains("is a symlink"), "{read}");
        assert_eq!(read.hint(), Some(hint));

        let write = store_token(&path).unwrap_err();
        assert_eq!(write.code(), crate::Code::Usage, "{write}");
        assert_eq!(write.hint(), Some(hint));
        assert_eq!(fs::read(real.join("config.toml")).unwrap(), before);
        let mut names: Vec<_> = fs::read_dir(&real)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [".config.toml.lock", "config.toml"],
            "nothing was written"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_config_directory_that_is_a_file_or_unreachable_is_refused() {
        let dir = temp_dir("not-a-dir");
        fs::write(dir.join("file"), "").unwrap();
        let error = store_token(&dir.join("file").join("config.toml")).unwrap_err();
        assert_eq!(error.code(), crate::Code::Usage, "{error}");
        assert!(error.to_string().contains("is not a directory"), "{error}");
        assert_eq!(
            error.hint(),
            Some("set `HIPPIUS_CONFIG` to a file in a directory")
        );

        // The owner may not search the parent, so the directory in it cannot be opened.
        fs::create_dir_all(dir.join("closed").join("hipc")).unwrap();
        fs::set_permissions(dir.join("closed"), fs::Permissions::from_mode(0o600)).unwrap();
        let error = store_token(&dir.join("closed").join("hipc").join("config.toml"));
        fs::set_permissions(dir.join("closed"), fs::Permissions::from_mode(0o700)).unwrap();
        let error = error.unwrap_err();
        // A mode the person changes, not a bug to report.
        assert_eq!(error.code(), crate::Code::Usage, "{error}");
        assert_eq!(
            error.to_string(),
            format!(
                "cannot open {}: permission is denied",
                quote_path(&dir.join("closed").join("hipc"))
            )
        );
        assert_eq!(
            error.hint(),
            Some(&*format!(
                "check that the current user can enter {} and each directory above it",
                quote_path(&dir.join("closed").join("hipc"))
            ))
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_path_that_names_no_file_is_refused_before_anything_is_written() {
        let dir = temp_dir("no-name");
        fs::create_dir_all(dir.join("sub")).unwrap();
        let error = store_token(&dir.join("sub").join("..")).unwrap_err();
        assert_eq!(error.code(), crate::Code::Usage, "{error}");
        assert!(error.to_string().contains("names no file"), "{error}");
        assert_eq!(error.hint(), Some("set `HIPPIUS_CONFIG` to a file path"));
        assert_eq!(fs::read_dir(dir.join("sub")).unwrap().count(), 0);
        // A missing directory is not created for such a path either.
        let error = store_token(&dir.join("new").join("..")).unwrap_err();
        assert!(error.to_string().contains("names no file"), "{error}");
        assert!(!dir.join("new").exists());
        let error = ConfigFile::load(dir.join("sub").join("..")).unwrap_err();
        assert!(error.to_string().contains("names no file"), "{error}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_fifo_or_a_file_over_1_mib_is_refused_without_waiting() {
        let dir = temp_dir("fifo");
        let fifo = dir.join("fifo.toml");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(made.success());
        // No writer ever opens the FIFO, so a blocking open would hang this test.
        let error = ConfigFile::load(fifo).unwrap_err();
        assert_eq!(error.code(), crate::Code::Usage, "{error}");
        assert!(error.to_string().contains("not a regular file"), "{error}");

        let big = dir.join("big.toml");
        let mut text = "# padding\n".repeat(MAX_FILE_BYTES / 10);
        fs::write(&big, &text).unwrap();
        fs::set_permissions(&big, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(text.len(), MAX_FILE_BYTES - MAX_FILE_BYTES % 10);
        ConfigFile::load(big.clone()).unwrap();
        text.push_str(&"#".repeat(MAX_FILE_BYTES % 10 + 1));
        fs::write(&big, &text).unwrap();
        let error = ConfigFile::load(big.clone()).unwrap_err();
        assert_eq!(error.code(), crate::Code::Usage, "{error}");
        assert!(error.to_string().contains("larger than 1 MiB"), "{error}");
        assert!(error.hint().unwrap().contains("HIPPIUS_CONFIG"), "{error}");
        let error = store_token(&big).unwrap_err();
        assert!(error.to_string().contains("larger than 1 MiB"), "{error}");
        assert_eq!(fs::metadata(&big).unwrap().len(), MAX_FILE_BYTES as u64 + 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_lock_that_cannot_be_created_is_usage_and_nothing_is_written() {
        let dir = temp_dir("no-lock");
        // The owner may read and enter but not write: the directory check passes, and the
        // lock file cannot be created.
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        let error = store_token(&dir.join("config.toml"));
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let error = error.unwrap_err();
        assert_eq!(error.code(), crate::Code::Usage, "{error}");
        assert_eq!(
            error.to_string(),
            format!(
                "cannot lock {}: permission is denied",
                quote_path(&dir.join(".config.toml.lock"))
            )
        );
        assert_eq!(
            error.hint(),
            Some(&*format!("run `chmod 700 {}`", dir.display()))
        );
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        fs::remove_dir_all(dir).unwrap();
    }

    /// A config the current user may not read is theirs to fix with a `chmod`, so it is
    /// `usage`, as an unreadable `--token-file` is.
    #[test]
    fn a_config_file_without_read_permission_is_usage() {
        let dir = temp_dir("unreadable");
        let path = dir.join("config.toml");
        fs::write(&path, "").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let error = ConfigFile::load(path.clone()).unwrap_err();
        assert_eq!(error.code(), crate::Code::Usage, "{error}");
        assert_eq!(
            error.to_string(),
            format!("cannot read {}: permission is denied", quote_path(&path))
        );
        assert_eq!(
            error.hint(),
            Some(&*format!("run `chmod 600 {}`", path.display()))
        );
        fs::remove_dir_all(dir).unwrap();
    }

    /// A directory that cannot be created names the variable that moves the file, since no
    /// `chmod` of a file `hipc` owns fixes it.
    #[test]
    fn a_config_directory_that_cannot_be_created_is_usage() {
        let dir = temp_dir("no-mkdir");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        let error = store_token(&dir.join("hipc").join("config.toml"));
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let error = error.unwrap_err();
        assert_eq!(error.code(), crate::Code::Usage, "{error}");
        assert_eq!(
            error.to_string(),
            format!(
                "cannot create {}: permission is denied",
                quote_path(&dir.join("hipc"))
            )
        );
        assert_eq!(
            error.hint(),
            Some("set `HIPPIUS_CONFIG` to a file in a directory the current user can write")
        );
        fs::remove_dir_all(dir).unwrap();
    }

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
