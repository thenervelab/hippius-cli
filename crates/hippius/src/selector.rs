//! The account, project, and billing account a run selects, from a flag, a variable, or the
//! profile: each a checked id and where it came from, which every refusal names (tenet 15).
//! Which routes take each one is decided in `account` and `project`.

use crate::error::{Error, Result};
use crate::phrase::quote;
use crate::source::Source;

/// An account id: `acct_` and 20 lowercase hex characters, and where it came from.
#[derive(Debug, Clone)]
pub struct AccountId(pub(crate) Selected);

impl AccountId {
    /// The id as sent in `X-Hippius-Account`.
    pub fn as_str(&self) -> &str {
        &self.0.id
    }

    /// The `account_scope_unsupported` refusal of something this account cannot do. `what`
    /// says what; the message goes on to name the account and where it came from.
    pub fn refusal(&self, what: &str) -> Error {
        self.0.refusal(what)
    }
}

/// The account commands act in.
#[derive(Debug, Clone)]
pub enum Selection {
    /// The caller's own account: no header is ever sent.
    Own,
    /// An account the caller is a member of. Any `acct_` id counts, the caller's own
    /// included: telling them apart would cost a request on every command.
    Member(AccountId),
}

impl Selection {
    /// Parses `self` or an account id, from `source`.
    ///
    /// # Errors
    ///
    /// `usage` when the text is neither; the message names the source, and the hint the fix
    /// that belongs to it.
    pub fn parse(text: &str, source: &Source) -> Result<Self> {
        Ok(parse_id(Selector::Account, text, source)?
            .map_or(Self::Own, |id| Self::Member(AccountId(id))))
    }

    /// The selection as text: `self`, or the account id.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Own => "self",
            Self::Member(id) => id.as_str(),
        }
    }
}

/// A project id: `proj_` and 20 lowercase hex characters, and where it came from.
#[derive(Debug, Clone)]
pub struct ProjectId(pub(crate) Selected);

impl ProjectId {
    /// Parses `default`, the Default project (`None`), or a project id, from `source`.
    ///
    /// # Errors
    ///
    /// `usage` when the text is neither; the message names the source, and the hint the fix
    /// that belongs to it.
    pub fn parse(text: &str, source: &Source) -> Result<Option<Self>> {
        parse_id(Selector::Project, text, source).map(|id| id.map(Self))
    }

    /// The id as sent in `X-Hippius-Project`.
    pub fn as_str(&self) -> &str {
        &self.0.id
    }

    /// Whether the id is the one stored in the profile, not one given for this run.
    pub fn is_stored(&self) -> bool {
        matches!(self.0.source, Source::Profile { .. })
    }
}

/// Two ids are equal when they name the same project, wherever each came from.
impl PartialEq for ProjectId {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for ProjectId {}

/// A billing account id: `bill_` and 20 lowercase hex characters, and where it came from.
#[derive(Debug, Clone)]
pub struct BillingId(pub(crate) Selected);

impl BillingId {
    /// Parses `default`, the Default billing account (`None`), or a billing account id,
    /// from `source`.
    ///
    /// # Errors
    ///
    /// `usage` when the text is neither; the message names the source, and the hint the fix
    /// that belongs to it.
    pub fn parse(text: &str, source: &Source) -> Result<Option<Self>> {
        parse_id(Selector::Billing, text, source).map(|id| id.map(Self))
    }

    /// The id as sent in `X-Hippius-Billing`.
    pub fn as_str(&self) -> &str {
        &self.0.id
    }
}

/// Two ids are equal when they name the same billing account, wherever each came from.
impl PartialEq for BillingId {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for BillingId {}

/// The text that selects the Default project or billing account: the console's own word
/// for it in the costs path.
pub const DEFAULT: &str = "default";

/// Which of the three selectors a value is for: `--account`, `--project`, or `--billing`.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Selector {
    Account,
    Project,
    Billing,
}

impl Selector {
    /// The id prefix. The project and billing ones are the prefixes the console uses for
    /// those ids.
    fn prefix(self) -> &'static str {
        match self {
            Self::Account => "acct_",
            Self::Project => "proj_",
            Self::Billing => "bill_",
        }
    }

    pub(crate) fn noun(self) -> &'static str {
        match self {
            Self::Account => "account",
            Self::Project => "project",
            Self::Billing => "billing account",
        }
    }

    /// The noun with its article: `an account`.
    fn a_noun(self) -> &'static str {
        match self {
            Self::Account => "an account",
            Self::Project => "a project",
            Self::Billing => "a billing account",
        }
    }

    /// The text that clears the selection.
    fn reset(self) -> &'static str {
        match self {
            Self::Account => "self",
            Self::Project | Self::Billing => DEFAULT,
        }
    }

    /// The `hipc` group that lists and stores the ids.
    fn group(self) -> &'static str {
        match self {
            Self::Account => "hipc account",
            Self::Project => "hipc account projects",
            Self::Billing => "hipc account billing-accounts",
        }
    }

    /// The refusal of a route that cannot use the selector, with `message`.
    pub(crate) fn refusal(self, message: String) -> Error {
        match self {
            Self::Account => Error::account_scope_unsupported(message),
            Self::Project => Error::project_scope_unsupported(message),
            Self::Billing => Error::billing_scope_unsupported(message),
        }
    }
}

/// A valid id of one selector, and where it came from, which a refusal names (tenet 15).
#[derive(Debug, Clone)]
pub(crate) struct Selected {
    pub(crate) selector: Selector,
    pub(crate) id: String,
    pub(crate) source: Source,
}

impl Selected {
    /// The refusal of a route that cannot use this selection. `route` says what the route
    /// cannot do; the message goes on to name the selection and where it came from.
    pub(crate) fn refusal(&self, route: &str) -> Error {
        let noun = self.selector.noun();
        // A stored value outlives the run, so the message names the command that clears it;
        // the hint stays the flag value that fixes this run (tenet 4).
        let clear = match &self.source {
            Source::Profile { .. } => format!(
                "; `{} use {}` clears it",
                self.selector.group(),
                self.selector.reset()
            ),
            Source::Argument
            | Source::Flag(_)
            | Source::Variable(_)
            | Source::File { .. }
            | Source::Default => String::new(),
        };
        self.selector.refusal(format!(
            "{route}, and the selected {noun} is {}{}{clear}",
            quote(&self.id),
            self.source.suffix()
        ))
    }
}

/// Whether `text` is an id of `selector`: its prefix and 20 lowercase hex characters.
pub(crate) fn is_id(selector: Selector, text: &str) -> bool {
    text.strip_prefix(selector.prefix()).is_some_and(|hex| {
        hex.len() == 20 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// Parses `text`, from `source`, as an id of `selector`, or `None` for the word that clears
/// the selection.
pub(crate) fn parse_id(
    selector: Selector,
    text: &str,
    source: &Source,
) -> Result<Option<Selected>> {
    let (reset, prefix) = (selector.reset(), selector.prefix());
    if text == reset {
        return Ok(None);
    }
    if is_id(selector, text) {
        return Ok(Some(Selected {
            selector,
            id: text.to_owned(),
            source: source.clone(),
        }));
    }
    let (noun, a_noun, group) = (selector.noun(), selector.a_noun(), selector.group());
    let hint = match source {
        Source::Flag(flag) => format!("pass `{flag} {reset}`, or an id from `{group} list`"),
        Source::Variable(name) => {
            format!("unset `{name}`, or set it to `{reset}` or an id from `{group} list`")
        }
        Source::Profile { .. } => format!("run `{group} use {reset}` to clear the stored {noun}"),
        Source::Argument | Source::File { .. } | Source::Default => {
            format!("run `{group} list` to see the {noun} ids the token can act in")
        }
    };
    Err(Error::usage(
        format!(
            "{}{} is not {a_noun} id: {a_noun} is `{reset}`, or `{prefix}` and 20 lowercase \
             hex characters",
            quote(text),
            source.suffix()
        ),
        hint,
    ))
}
