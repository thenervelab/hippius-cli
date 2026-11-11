//! The one rule of the name and id value parsers: a length, a set of characters, and whether
//! the value may start with `-`. Every parser of a name or an id that is one word of one set of
//! characters holds its value to [`word`] (or [`is_word`], for a check made after parsing), and
//! every numeric id to [`number`], so the rules cannot drift apart. A value with a grammar of
//! its own (a zone id, a hostname, an artifact reference, a repository or model name) keeps
//! its own parser. Every free-text label a person reads on one line, such as a project's name
//! or a ticket's subject, is held to [`label`].

use std::ops::RangeInclusive;

use crate::units::whole_number;

/// The characters a name or id may hold. Every set but [`Charset::OneWord`] is ASCII with no
/// `/`, `?`, `%`, or space. [`Charset::LowerDotDash`] holds `.`, so a check that puts its value
/// in a URL path refuses `..` itself, as the bucket check does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Charset {
    /// Letters of either case, digits, `-`, and `_`: an id the server issued.
    Slug,
    /// `a-z`, `0-9`, and `-`.
    LowerDash,
    /// `a-z`, `0-9`, and `_`.
    LowerUnderscore,
    /// `a-z`, `0-9`, `.`, and `-`.
    LowerDotDash,
    /// Any character but whitespace and control characters: one word, for a value that goes
    /// into a body, not a path.
    OneWord,
}

impl Charset {
    fn holds(self, c: char) -> bool {
        let lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        match self {
            Self::Slug => c.is_ascii_alphanumeric() || c == '-' || c == '_',
            Self::LowerDash => lower || c == '-',
            Self::LowerUnderscore => lower || c == '_',
            Self::LowerDotDash => lower || c == '.' || c == '-',
            Self::OneWord => !c.is_whitespace() && !c.is_control(),
        }
    }
}

/// Whether a value may start with `-`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dash {
    /// An id the server issued: the caller cannot choose it, so it is taken as the server
    /// gave it, and every command `hipc` prints puts such an id after `--`
    /// ([`crate::cli::with_id`]).
    Taken,
    /// A name the caller chooses, which a command line would read as a flag: refused, so no
    /// command that names it later has to work around it.
    Refused,
}

/// Whether `value` is `len` characters of `charset`, and starts with `-` only when `dash`
/// allows it.
pub(crate) fn is_word(
    value: &str,
    len: RangeInclusive<usize>,
    charset: Charset,
    dash: Dash,
) -> bool {
    len.contains(&value.chars().count())
        && value.chars().all(|c| charset.holds(c))
        && !(dash == Dash::Refused && value.starts_with('-'))
}

/// `value` when [`is_word`] holds, else `refusal`, which names the value and states its rule.
pub(crate) fn word(
    value: &str,
    len: RangeInclusive<usize>,
    charset: Charset,
    dash: Dash,
    refusal: &str,
) -> Result<String, String> {
    if is_word(value, len, charset, dash) {
        Ok(value.to_owned())
    } else {
        Err(refusal.to_owned())
    }
}

/// Whether `value` is a UUID in its text form: five groups of 8, 4, 4, 4, and 12 hex digits,
/// joined by `-`, in either case.
pub(crate) fn is_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(group, len)| group.len() == len && group.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// What a [`label`] does with the spaces around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Spaces {
    /// Sent as typed.
    Kept,
    /// Sent trimmed, as the console sends it, so a later read compares equal.
    Trimmed,
}

/// A label a person reads on one line, such as a name or a subject, checked before it is sent.
/// It is `len` characters as sent, and not blank when `len` starts above 0. It holds no control
/// character and no line break (a tab, a newline, or the line and paragraph separators), since
/// it is shown on one line where those would misalign or split it. Nor does it hold a character
/// that changes how text reads without being seen, such as a bidi override or a zero-width
/// space, with which two labels that look the same would differ and a person would pick the
/// wrong one. `what` names the label with its article (`a referral name`); the refusal names
/// the rule, never the value.
pub(crate) fn label(
    value: &str,
    len: RangeInclusive<usize>,
    spaces: Spaces,
    what: &str,
) -> Result<String, String> {
    let value = match spaces {
        Spaces::Kept => value,
        Spaces::Trimmed => value.trim(),
    };
    let blank = *len.start() > 0 && value.trim().is_empty();
    if blank || !len.contains(&value.chars().count()) {
        return Err(format!(
            "{what} is one line of {} to {} characters",
            len.start(),
            len.end()
        ));
    }
    if value
        .chars()
        .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
    {
        return Err(format!(
            "{what} holds no control character or line break, a tab or a newline included, \
             since it is shown on one line"
        ));
    }
    if value.chars().any(crate::output::changes_how_text_reads) {
        return Err(format!(
            "{what} holds no invisible character, such as a bidi override or a zero-width \
             space, since two that look the same would differ"
        ));
    }
    Ok(value.to_owned())
}

/// A parser for a code from a catalog the server owns, such as a database tier or an S3 plan:
/// one word of 1 to 64 lowercase letters, digits, `.`, and `-`, not starting with `-`. Which
/// codes exist is the server's to say, so a code it adds needs no new `hipc` (owner decision
/// D5); `today` lists the codes known when this `hipc` was built, for the refusal and the help.
/// The value goes into a body, never a path. `what` names it with its article.
pub(crate) fn catalog(
    what: &'static str,
    today: &'static [&'static str],
) -> impl Fn(&str) -> Result<String, String> + Clone {
    move |value| {
        if is_word(value, 1..=64, Charset::LowerDotDash, Dash::Refused) {
            Ok(value.to_owned())
        } else {
            Err(format!(
                "{what} is a code of lowercase letters, digits, `.`, and `-`, such as {}",
                hippius::phrase::ticked(today, "or")
            ))
        }
    }
}

/// A parser for a numeric id, which goes into the URL path or the body as a number.
/// `refusal` says what the id is and which command prints it, in place of Rust's "invalid
/// digit found in string". Only plain digits are an id ([`whole_number`]): a `-` or a `+` is
/// refused.
pub(crate) fn number(refusal: &'static str) -> impl Fn(&str) -> Result<u64, String> + Clone {
    move |value| whole_number(value).ok_or_else(|| refusal.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_charset_holds_its_own_characters_only() {
        let cases = [
            (Charset::Slug, "aZ09-_", ".~/ %é"),
            (Charset::LowerDash, "az09-", "A_./ "),
            (Charset::LowerUnderscore, "az09_", "A-./ "),
            (Charset::LowerDotDash, "az09.-", "A_/ "),
            (
                Charset::OneWord,
                "aZ-./é$",
                " \t\n\u{85}\u{a0}\u{2028}\u{1b}",
            ),
        ];
        for (charset, good, bad) in cases {
            for c in good.chars() {
                assert!(charset.holds(c), "{charset:?} {c:?}");
            }
            for c in bad.chars() {
                assert!(!charset.holds(c), "{charset:?} {c:?}");
            }
        }
    }

    #[test]
    fn a_word_holds_its_length_charset_and_dash_rule() {
        assert!(is_word("a", 1..=2, Charset::Slug, Dash::Refused));
        assert!(is_word("ab", 1..=2, Charset::Slug, Dash::Refused));
        assert!(!is_word("", 1..=2, Charset::Slug, Dash::Taken));
        assert!(!is_word("abc", 1..=2, Charset::Slug, Dash::Taken));
        assert!(is_word("-a", 1..=2, Charset::Slug, Dash::Taken));
        assert!(!is_word("-a", 1..=2, Charset::Slug, Dash::Refused));
        assert!(is_word("a-", 1..=2, Charset::Slug, Dash::Refused));
        assert!(!is_word("..", 1..=2, Charset::Slug, Dash::Taken));
        // The length counts characters.
        assert!(is_word("éé", 1..=2, Charset::OneWord, Dash::Refused));
        assert_eq!(
            word("x", 1..=1, Charset::Slug, Dash::Taken, "no"),
            Ok("x".to_owned())
        );
        assert_eq!(
            word("", 1..=1, Charset::Slug, Dash::Taken, "no"),
            Err("no".to_owned())
        );
    }

    #[test]
    fn a_label_is_one_visible_line_of_its_length() {
        let name = |value| label(value, 1..=3, Spaces::Kept, "a name");
        assert_eq!(name("abc"), Ok("abc".to_owned()));
        assert_eq!(name(" é "), Ok(" é ".to_owned()));
        let length = Err("a name is one line of 1 to 3 characters".to_owned());
        for bad in ["", "   ", "abcd"] {
            assert_eq!(name(bad), length, "{bad:?}");
        }
        for bad in ["a\tb", "a\nb", "a\u{2028}", "\u{1b}["] {
            assert!(
                name(bad)
                    .unwrap_err()
                    .contains("no control character or line break"),
                "{bad:?}"
            );
        }
        for bad in ["a\u{202e}b", "a\u{200b}"] {
            assert!(
                name(bad).unwrap_err().contains("no invisible character"),
                "{bad:?}"
            );
        }
        let trimmed = |value| label(value, 1..=3, Spaces::Trimmed, "a name");
        assert_eq!(trimmed("  abc  "), Ok("abc".to_owned()));
        assert!(trimmed(" ").is_err());
        let optional = |value| label(value, 0..=usize::MAX, Spaces::Kept, "a city");
        assert_eq!(optional(""), Ok(String::new()));
        assert!(optional("a\nb").is_err());
    }

    #[test]
    fn a_catalog_code_is_one_lowercase_word() {
        let tier = catalog("a tier", &["starter", "ha"]);
        for good in ["starter", "ha", "new-tier", "v1.2", "16"] {
            assert_eq!(tier(good), Ok(good.to_owned()), "{good}");
        }
        for bad in ["", "Starter", "-x", "a b", "a_b", &"a".repeat(65)] {
            assert_eq!(
                tier(bad),
                Err(
                    "a tier is a code of lowercase letters, digits, `.`, and `-`, such as \
                     `starter` or `ha`"
                        .to_owned()
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_number_is_a_u64_or_the_refusal() {
        let parse = number("an id is a whole number");
        assert_eq!(parse("0"), Ok(0));
        assert_eq!(parse(&u64::MAX.to_string()), Ok(u64::MAX));
        for bad in ["", "-1", "+1", "18446744073709551616", "..", "1.5", "a"] {
            assert_eq!(
                parse(bad),
                Err("an id is a whole number".to_owned()),
                "{bad}"
            );
        }
    }
}
