//! The config file and the settings one invocation runs with (tenet 8).
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
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::phrase::{quote, quote_path};
use crate::private_file::{Guarded, NamedBy, PrivateFile, utf8_text};
use crate::selector::{BillingId, ProjectId, Selection};
use crate::source::{Given, Source};
use crate::url::Origin;

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

/// Whether `text` holds `token` in any spelling [`crate::redact::token_spellings`] lists.
/// The token is trimmed first, as [`Token::new`] trims it before it is sent, so a hand-edited
/// file's padded token is found as it goes out. A token shorter than [`PREVIEW_MIN_CHARS`]
/// is in no text: a console token is 40 characters, and a short one, such as `token`, would
/// match ordinary words and refuse a command line or a ticket for holding it.
fn quoted_in(token: &str, text: &str) -> bool {
    let token = token.trim();
    token.chars().count() >= PREVIEW_MIN_CHARS
        && zeroize::Zeroizing::new(crate::redact::token_spellings(token))
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

/// The keys of a config table this `hipc` does not know, such as one a newer `hipc` wrote.
/// They are kept as they were and written back on every save, so two versions can share one
/// file (tenet 8). `Debug` counts them, since a value may be a token.
#[derive(Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(transparent)]
struct Unknown(BTreeMap<String, toml::Value>);

impl fmt::Debug for Unknown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Unknown({} keys)", self.0.len())
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Document {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_profile: Option<String>,
    #[serde(default)]
    profiles: BTreeMap<String, Profile>,
    #[serde(flatten)]
    unknown: Unknown,
}

impl Document {
    /// Where the document holds keys this `hipc` does not know, as a warning names them: `the
    /// top level`, `` the profile `work` ``, `` the `hub_subscribe` of the profile `work` ``.
    /// The keys are not named, since one may be a token pasted in the wrong place.
    fn unknown_places(&self) -> Vec<String> {
        let top = (!self.unknown.0.is_empty()).then(|| "the top level".to_owned());
        let profiles = self.profiles.iter().flat_map(|(name, profile)| {
            let named = format!("the profile {}", quote(name));
            let own = (!profile.unknown.0.is_empty()).then(|| named.clone());
            let marker = profile
                .hub_subscribe
                .as_ref()
                .filter(|marker| !marker.unknown.0.is_empty())
                .map(|_| format!("the `hub_subscribe` of {named}"));
            own.into_iter().chain(marker)
        });
        top.into_iter().chain(profiles).collect()
    }
}

/// One profile as the file holds it. The selections (`account`, `project`, `billing`) are
/// kept as text and parsed when a run reads them, so a hand edit is refused with its field and
/// its source, not as a file that does not deserialize; `hipc` writes only values it checked.
#[derive(Default, Serialize, Deserialize)]
struct Profile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    api_url: Option<String>,
    /// Cleared when the profile is dropped or the token replaced, as [`Token`] is.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "stored_token"
    )]
    token: Option<Zeroizing<String>>,
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
    /// (tenet 11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hub_subscribe: Option<HubMarker>,
    #[serde(flatten)]
    unknown: Unknown,
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
            .field("unknown", &self.unknown)
            .finish()
    }
}

/// The serde form of [`Profile`]'s token: a plain TOML string, held in memory as
/// `Zeroizing<String>`. The text the TOML parser and writer hold on the way are theirs and are
/// not cleared here; the config text read and written is (`ConfigFile::parse`).
mod stored_token {
    use serde::{Deserialize as _, Deserializer, Serialize as _, Serializer};
    use zeroize::Zeroizing;

    #[expect(
        clippy::ref_option,
        reason = "serde's `with` hands a field over by reference"
    )]
    pub(super) fn serialize<S: Serializer>(
        token: &Option<Zeroizing<String>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        token.as_deref().serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Zeroizing<String>>, D::Error> {
        Ok(Option::<String>::deserialize(deserializer)?.map(Zeroizing::new))
    }
}

/// A Hub subscribe that was sent: when, as which origin and token, and which paid plans it
/// cancelled first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HubMarker {
    /// When it was sent, in Unix seconds.
    at: u64,
    /// Who it was sent as (`Client::identity`: the origin and a token fingerprint), so a
    /// marker from another origin or token on the same profile name does not block this one.
    key: String,
    /// The paid subscriptions that run cancels before it subscribes. Inside the window the
    /// list can still show them as active, so a later `--replace` that sees only these has
    /// nothing new to cancel. Left out of the file when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    cancels: Vec<u64>,
    /// Keys this `hipc` does not know, kept as they were.
    #[serde(flatten)]
    unknown: Unknown,
}

impl HubMarker {
    /// A marker of a subscribe sent `at`, as `key`, that cancels `cancels` first.
    pub fn new(at: u64, key: String, cancels: Vec<u64>) -> Self {
        Self {
            at,
            key,
            cancels,
            unknown: Unknown::default(),
        }
    }

    /// When it was sent, in Unix seconds.
    pub fn at(&self) -> u64 {
        self.at
    }

    /// Who it was sent as: the origin and a token fingerprint.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The paid subscriptions that run cancels before it subscribes.
    pub fn cancels(&self) -> &[u64] {
        &self.cancels
    }
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
/// An AWS profile name is held to the same rule ([`crate::AwsProfile::new`]), where a `]` or a
/// line break would end its section header.
pub(crate) fn is_new_profile_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_PROFILE_NAME_CHARS
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The refusal of a profile name outside the rules of [`is_new_profile_name`].
fn invalid_profile_name(given: &Given) -> Error {
    let example = format!("`{DEFAULT_PROFILE}`");
    let hint = match &given.source {
        Source::Variable(variable) => {
            format!("unset `{variable}`, or set it to a profile name such as {example}")
        }
        // `--profile`, the one other source of a profile name.
        _ => format!("pass a `--profile` such as {example}"),
    };
    // The name is not repeated: it may be a secret given in its place (tenet 9).
    Error::usage(
        format!(
            "the profile name{} breaks the name rules: a name is 1 to \
             {MAX_PROFILE_NAME_CHARS} letters, digits, `.`, `_`, or `-`; it is not repeated \
             here, in case it is a secret",
            given.source.suffix()
        ),
        hint,
    )
}

/// The config path: `HIPPIUS_CONFIG` when it is set and not empty, else
/// `$HOME/.hippius/config.toml`. The caller passes both variables. An empty variable counts
/// as unset, as `hipc` reads every `HIPPIUS_*` variable; an empty AWS variable is refused
/// instead ([`crate::AwsFiles::locate`]), since the AWS CLI, not `hipc`, defines those.
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
}

/// The config file a [`ConfigFile::edit`] on this thread rewrote while it had another hard
/// link, once, or `None`. An edit writes a new file and renames it over the old name, so the
/// other link keeps the old contents, the tokens a logout removed included; the binary warns
/// (tenet 8). Thread-local, so a test sees only its own edits.
pub fn take_split_link() -> Option<PathBuf> {
    SPLIT_LINK.with(|split| split.borrow_mut().take())
}

thread_local! {
    /// The warning about the keys the last config file loaded on this thread holds and this
    /// `hipc` does not know, until [`take_unknown_keys_warning`] takes it.
    static UNKNOWN_KEYS: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// The warning that the config file holds keys this `hipc` does not know, once, or `None`. A
/// run loads the file more than once, and each load of a file with such keys sets it again, so
/// the binary takes it once at the end of a run and warns once (tenet 8). Thread-local, so a
/// test sees only its own loads.
pub fn take_unknown_keys_warning() -> Option<String> {
    UNKNOWN_KEYS.with(|warning| warning.borrow_mut().take())
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
    fn parse(path: PathBuf, bytes: Option<Zeroizing<Vec<u8>>>) -> Result<Self> {
        let Some(bytes) = bytes else {
            return Ok(Self {
                path,
                document: Document::default(),
            });
        };
        // Cleared when dropped: the text holds every stored token. The parsed document clears
        // its own copies (each `Profile`'s token is `Zeroizing`).
        let text = utf8_text(bytes).ok_or_else(|| {
            Error::usage(
                format!("the config file {} is not UTF-8 text", quote_path(&path)),
                format!("save {} as UTF-8 text", quote_path(&path)),
            )
        })?;
        let document: Document = toml::from_str(&text).map_err(|e: toml::de::Error| {
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
        let places = document.unknown_places();
        if !places.is_empty() {
            let warning = format!(
                "the config file {} holds keys this hipc does not know, in {}; they are kept as \
                 they are, and a newer hipc may use them",
                quote_path(&path),
                crate::phrase::join(&places, "and")
            );
            UNKNOWN_KEYS.with(|unknown| *unknown.borrow_mut() = Some(warning));
        }
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
    /// and, when `HIPPIUS_TOKEN` is unset and the profile stores a token, for a stored
    /// `token_origin` or a target `api_url` that is not an origin, or, when the two origins
    /// match, a stored token that is empty or not visible ASCII. A withheld token is not
    /// checked.
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
        stored.token = Some(Zeroizing::new(token.expose().to_owned()));
        stored.token_origin = Some(origin.clone());
        Ok(origin)
    }

    /// Whether `profile` stores a token other than `token`: a login that would replace it. A
    /// stored token is compared trimmed, as [`Token::new`] trims the one it sends, and an empty
    /// one, which a hand-edited file can hold, is no token, as in
    /// [`ConfigFile::holds_a_stored_token`].
    pub fn stores_other_token(&self, profile: &str, token: &Token) -> bool {
        self.stored_token(profile).is_some_and(|stored| {
            let stored = stored.trim();
            !stored.is_empty() && stored != token.expose()
        })
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
            .and_then(|p| p.token.as_deref().map(String::as_str))
    }

    /// Removes the token from `profile`. Other fields stay.
    pub fn clear_token(&mut self, profile: &str) {
        if let Some(stored) = self.document.profiles.get_mut(profile) {
            stored.token = None;
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
/// sentence that starts with the line: `has a value that is not a string`.
///
/// The parser's own message quotes the key or the value it refused, either of which can be a
/// token pasted in the wrong place, so none of it is repeated. Only what serde writes from the
/// config types themselves is: the name of a missing field.
/// A message this does not know is `does not parse`.
fn config_problem(message: &str) -> String {
    // The text between each pair of backticks: `` missing field `key` `` gives `key`.
    let quoted = |text: &str| -> Vec<String> {
        text.split('`')
            .skip(1)
            .step_by(2)
            .map(crate::phrase::quote)
            .collect()
    };
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

/// The config file.
pub(crate) const CONFIG_FILE: Guarded = Guarded {
    what: "the config file",
    holds: "API tokens",
    named_by: NamedBy::Variable("HIPPIUS_CONFIG"),
};

#[cfg(test)]
mod tests;
