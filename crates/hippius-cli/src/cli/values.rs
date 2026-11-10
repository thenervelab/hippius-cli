//! The one rule of the name and id value parsers: a length, a set of characters, and whether
//! the value may start with `-`. Every parser of a name or an id that is one word of one set of
//! characters holds its value to [`word`] (or [`is_word`], for a check made after parsing), and
//! every numeric id to [`number`], so the rules cannot drift apart. A value with a grammar of
//! its own (a zone id, a hostname, an artifact reference, a repository or model name) keeps
//! its own parser.

use std::ops::RangeInclusive;

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

/// A parser for a numeric id, which goes into the URL path or the body as a number.
/// `refusal` says what the id is and which command prints it, in place of Rust's "invalid
/// digit found in string". The digits are read as `u64` parses them, so a `-` is refused.
pub(crate) fn number(refusal: &'static str) -> impl Fn(&str) -> Result<u64, String> + Clone {
    move |value| value.parse().map_err(|_| refusal.to_owned())
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
    fn a_number_is_a_u64_or_the_refusal() {
        let parse = number("an id is a whole number");
        assert_eq!(parse("0"), Ok(0));
        assert_eq!(parse(&u64::MAX.to_string()), Ok(u64::MAX));
        for bad in ["", "-1", "18446744073709551616", "..", "1.5", "a"] {
            assert_eq!(
                parse(bad),
                Err("an id is a whole number".to_owned()),
                "{bad}"
            );
        }
    }
}
