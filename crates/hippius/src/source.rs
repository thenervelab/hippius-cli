//! Where a value came from, so a message about a bad value names it (tenet 15, "Phrasing"): a
//! refusal never reads the same for a flag and for a stored value, because the fix is in
//! different places.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::phrase::quote_path;

/// A value, and where it came from.
#[derive(Debug, Clone)]
pub struct Given {
    /// The value as given.
    pub value: String,
    /// Where it came from.
    pub source: Source,
}

/// Where a value came from.
#[derive(Debug, Clone)]
pub enum Source {
    /// A positional argument of the command. The message quotes the value, which is enough.
    Argument,
    /// A flag, such as `--account`.
    Flag(&'static str),
    /// An environment variable, such as `HIPPIUS_ACCOUNT`.
    Variable(&'static str),
    /// The file a flag names, or stdin for `-`.
    File {
        /// The flag, such as `--token-file`.
        flag: &'static str,
        /// The path as given.
        path: PathBuf,
    },
    /// A field of a profile in the config file.
    Profile {
        /// The field, such as `account`.
        field: &'static str,
        /// The profile name.
        profile: String,
        /// The config file.
        path: PathBuf,
    },
    /// Nothing set it: the built-in default.
    Default,
}

impl Source {
    /// The source as the end of a phrase about a value, with a leading space: ` from
    /// `--account``, ` from the `account` of the profile `default` in `/h/config.toml``, and
    /// nothing for an argument or the default.
    pub(crate) fn suffix(&self) -> String {
        match self {
            Self::Argument | Self::Default => String::new(),
            other => format!(" from {other}"),
        }
    }
}

/// The source as a message names it: `` `--account` ``, `` `HIPPIUS_ACCOUNT` ``, a path in
/// backticks or `stdin`, or `` the `account` of the profile `default` in `<path>` ``. Used
/// where the source is the subject of the phrase; `Source::suffix` where it trails the value.
impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Argument => f.write_str("the argument"),
            Self::Flag(flag) => write!(f, "`{flag}`"),
            Self::Variable(name) => write!(f, "`{name}`"),
            Self::File { path, .. } if path == Path::new("-") => f.write_str("stdin"),
            // A seed phrase passed in place of a path is not repeated, as on the command line.
            Self::File { flag, path } if path.to_str().is_some_and(crate::holds_phrase) => {
                write!(f, "the file `{flag}` names")
            }
            Self::File { path, .. } => f.write_str(&quote_path(path)),
            Self::Profile {
                field,
                profile,
                path,
            } => write!(
                f,
                "the `{field}` of the profile `{profile}` in {}",
                quote_path(path)
            ),
            Self::Default => f.write_str("the default"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_source_names_where_the_value_came_from() {
        let stored = Source::Profile {
            field: "account",
            profile: "work".into(),
            path: "/h/.hippius/config.toml".into(),
        };
        assert_eq!(
            stored.suffix(),
            " from the `account` of the profile `work` in `/h/.hippius/config.toml`"
        );
        assert_eq!(Source::Flag("--account").suffix(), " from `--account`");
        assert_eq!(
            Source::Variable("HIPPIUS_ACCOUNT").suffix(),
            " from `HIPPIUS_ACCOUNT`"
        );
        let stdin = Source::File {
            flag: "--token-file",
            path: "-".into(),
        };
        assert_eq!(stdin.suffix(), " from stdin");
        let file = Source::File {
            flag: "--token-file",
            path: "/t".into(),
        };
        assert_eq!(file.suffix(), " from `/t`");
        // A seed phrase passed in place of a path is named by its flag, not quoted.
        let phrase = Source::File {
            flag: "--body-file",
            path: "bottom drive obey lake curtain smoke basket hold race lonely fit walk".into(),
        };
        assert_eq!(phrase.to_string(), "the file `--body-file` names");
        assert_eq!(Source::Argument.suffix(), "");
        assert_eq!(Source::Default.suffix(), "");
        assert_eq!(Source::Argument.to_string(), "the argument");
        assert_eq!(Source::Default.to_string(), "the default");
    }
}
