//! API origins and URLs as `hipc` checks and quotes them: the origin a token may go to, and a
//! URL with its query values withheld before any message repeats it.

use std::borrow::Cow;

use crate::error::{Error, Result};
use crate::phrase::quote;
use crate::source::Source;

/// An API origin: `https://host[:port]`, or `http://` to a loopback host.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Origin(String);

impl Origin {
    /// Parses an origin such as `https://api.hippius.com`. A trailing `/` is allowed.
    ///
    /// Plain `http` is refused unless the host is `127.0.0.1` or `[::1]`: the
    /// token travels in a header, and `HIPPIUS_API_URL=http://...` must not be a way to
    /// send it in the clear.
    ///
    /// # Errors
    ///
    /// `usage` for anything else: another scheme, a path, a query, credentials, no host.
    pub(crate) fn parse(text: &str) -> Result<Self> {
        Self::parse_from(text, &Source::Argument)
    }

    /// [`Origin::parse`] for a value from `source`, which the error names, with the fix
    /// that belongs to it.
    pub(crate) fn parse_from(text: &str, source: &Source) -> Result<Self> {
        let example = "`https://api.hippius.com`";
        let hint = match source {
            Source::Flag(flag) => format!("pass an `{flag}` such as {example}"),
            Source::Variable(name) => {
                format!("unset `{name}`, or set it to an origin such as {example}")
            }
            Source::Profile {
                field: "token_origin",
                ..
            } => "run `hipc auth login --token-file <path>` to store the token again".to_owned(),
            Source::Profile { .. } => format!("edit {source}"),
            Source::Argument | Source::File { .. } | Source::Default => {
                format!("use an origin such as {example}")
            }
        };
        let invalid = |why: &str| {
            Error::usage(
                format!(
                    "{}{} is not an API origin: {why}",
                    quote(without_query_values(text)),
                    source.suffix()
                ),
                hint.clone(),
            )
        };
        let (scheme, rest) = text
            .split_once("://")
            .ok_or_else(|| invalid("it has no scheme"))?;
        let scheme = scheme.to_ascii_lowercase();
        let authority = rest.strip_suffix('/').unwrap_or(rest);
        if authority.is_empty() {
            return Err(invalid("it has no host"));
        }
        if authority.contains(['/', '?', '#']) {
            return Err(invalid("it has a path, a query, or a fragment"));
        }
        if authority.contains('@') {
            return Err(invalid("it holds a user name or a password"));
        }
        if authority
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(invalid("it holds whitespace"));
        }
        // A host outside ASCII can spell a look-alike of the real one (`аpi` with a
        // Cyrillic `а`) in a confirmation question; IDN hosts are given in punycode.
        if !authority.is_ascii() {
            return Err(invalid(
                "its host is not ASCII, and an IDN host is written in punycode",
            ));
        }
        let host = host_of(authority).to_ascii_lowercase();
        // `https://:443` has a port and no host, and a bracket that does not hold an IPv6
        // address (`[`, `[]`, `[x]`) holds none; either would reach `send` as a URL with no
        // host to connect to.
        let not_ipv6 = host.strip_prefix('[').is_some_and(|inner| {
            inner
                .strip_suffix(']')
                .and_then(|address| address.parse::<std::net::Ipv6Addr>().ok())
                .is_none()
        });
        if host.is_empty() || not_ipv6 {
            return Err(invalid("it has no host"));
        }
        // After the host only `:port` may follow, so `[::1]evil.example` is not taken for
        // the loopback host and `host:abc` is not a port. A colon outside brackets belongs
        // to the port, so `host:abc:443` is refused too.
        if !host.starts_with('[') && host.contains(':') {
            return Err(invalid("it has more than one `:` outside an IPv6 address"));
        }
        // A host name is letters, digits, `-`, and `.` (RFC 1123, section 2.1). Anything else
        // (a backtick, a quote, `<`, `{`) names no host, would reach a confirmation question
        // as a broken literal, and is refused by the HTTP library only when a request is
        // built, after a dry run accepted it.
        let host_name = host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
        if !host.starts_with('[') && !host_name {
            return Err(invalid(
                "its host holds a character other than a letter, a digit, `-`, or `.`",
            ));
        }
        let after_host = &authority[host.len()..];
        let port = match after_host.strip_prefix(':') {
            None if after_host.is_empty() => None,
            Some(port) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => Some(
                port.parse::<u16>()
                    .map_err(|_| invalid("its port is above 65535"))?,
            ),
            _ => {
                return Err(invalid(
                    "it has something after the host that is not a port",
                ));
            }
        };
        match scheme.as_str() {
            "https" => {}
            // Literal loopback addresses only: `localhost` goes through the resolver, and a
            // container without it in its hosts file could resolve it to a remote host.
            "http" if matches!(host.as_str(), "127.0.0.1" | "[::1]") => {}
            "http" => {
                return Err(invalid(
                    "plain `http` is allowed only to `127.0.0.1` and `[::1]`",
                ));
            }
            _ => return Err(invalid("its scheme is not `https`")),
        }
        // The port is written as the number it is, and the default one is dropped, so
        // `https://api.hippius.com:0443` and `:443` are the same origin as
        // `https://api.hippius.com`, which ureq connects to, and a full URL to it still gets
        // the token.
        let default = if scheme == "https" { 443 } else { 80 };
        Ok(match port {
            Some(port) if port != default => Self(format!("{scheme}://{host}:{port}")),
            _ => Self(format!("{scheme}://{host}")),
        })
    }

    /// The origin as text, without a trailing slash, owned.
    pub(crate) fn into_string(self) -> String {
        self.0
    }

    /// The origin as text, without a trailing slash.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// Plain `http`, which `parse` allows only to a loopback host.
    pub(crate) fn is_plain_http(&self) -> bool {
        self.0.starts_with("http://")
    }
}

/// The host of `host[:port]` or `[v6]:port`.
fn host_of(authority: &str) -> &str {
    if authority.starts_with('[') {
        authority
            .find(']')
            .map_or(authority, |end| &authority[..=end])
    } else {
        authority
            .rsplit_once(':')
            .map_or(authority, |(host, _)| host)
    }
}

/// `url` with every query value replaced by `<redacted>`, for the `--debug` line
/// and for any message that quotes a URL. hipc
/// cannot know which parameter of a `hipc api` path is a secret, so it shows none of them
/// (tenet 9: `--debug` redacts tokens and secrets). The names stay, which is what a person
/// debugging a route needs.
pub fn without_query_values(url: &str) -> String {
    // A fragment is never sent and can carry anything; userinfo is a password in a URL.
    let url = url.split_once('#').map_or(url, |(before, _)| before);
    let url = match url.split_once("://") {
        Some((scheme, rest)) => {
            let end = rest.find(['/', '?']).unwrap_or(rest.len());
            match rest[..end].rsplit_once('@') {
                Some((_, host)) => format!("{scheme}://<redacted>@{host}{}", &rest[end..]),
                None => url.to_owned(),
            }
        }
        None => url.to_owned(),
    };
    let Some((base, query)) = url.split_once('?') else {
        return url;
    };
    // A query part without `=` can be a bare secret (`?s3cr3t`), so it is redacted too.
    let names: Vec<String> = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((name, _)) => format!("{name}=<redacted>"),
            None if pair.is_empty() => String::new(),
            None => "<redacted>".to_owned(),
        })
        .collect();
    format!("{base}?{}", names.join("&"))
}

/// `path` without its query.
pub(crate) fn without_query(path: &str) -> &str {
    path.split_once('?').map_or(path, |(path, _)| path)
}

/// `path` with a trailing `/`: every route table and row matches a path as if it had one.
pub(crate) fn with_trailing_slash(path: &str) -> Cow<'_, str> {
    if path.ends_with('/') {
        Cow::Borrowed(path)
    } else {
        Cow::Owned(format!("{path}/"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;

    #[test]
    fn urls_in_messages_show_query_names_but_no_values() {
        assert_eq!(without_query_values("https://h/api/x/"), "https://h/api/x/");
        assert_eq!(
            without_query_values("https://h/api/x/?token=s3cr3t&page=2&flag"),
            "https://h/api/x/?token=<redacted>&page=<redacted>&<redacted>"
        );
        assert_eq!(
            without_query_values("https://user:s3cr3t@h/x/?a=1#t=s3cr3t"),
            "https://<redacted>@h/x/?a=<redacted>"
        );
        assert_eq!(without_query_values("/x/?"), "/x/?");
    }

    #[test]
    fn only_a_port_may_follow_the_host() {
        for bad in [
            "http://[::1]evil.example",
            "http://[::1].attacker.example:9",
            "https://api.hippius.com:abc",
            "https://api.hippius.com:",
            "http://localhost:80x",
            "https://api.hippius.com:abc:443",
            "https://\u{430}pi.hippius.com",
        ] {
            assert_eq!(Origin::parse(bad).unwrap_err().code(), Code::Usage, "{bad}");
        }
        assert!(Origin::parse("http://[::1]:9").is_ok());
        assert!(Origin::parse("https://api.hippius.com:8443").is_ok());
        assert!(Origin::parse("https://api.hippius.com:65536").is_err());
        assert_eq!(
            Origin::parse("https://api.hippius.com:0443")
                .unwrap()
                .as_str(),
            "https://api.hippius.com"
        );
        assert_eq!(
            Origin::parse("http://127.0.0.1:09").unwrap().as_str(),
            "http://127.0.0.1:9"
        );
    }

    #[test]
    fn an_origin_without_a_host_is_refused() {
        for bad in [
            "https://:443",
            "https://:8443",
            "https://[",
            "https://[evil",
            "https://[]",
            "https://[]:443",
            "https://[x]",
            "https://[:]",
        ] {
            let error = Origin::parse(bad).unwrap_err();
            assert_eq!(error.code(), Code::Usage, "{bad}");
            assert_eq!(
                error.to_string(),
                format!("`{bad}` is not an API origin: it has no host"),
            );
        }
    }

    /// A host name is letters, digits, `-`, and `.` (RFC 1123, section 2.1): anything else
    /// would reach a confirmation as a broken literal, and a dry run would accept an origin
    /// the HTTP library refuses to build a request for.
    #[test]
    fn a_host_holds_only_letters_digits_hyphens_and_dots() {
        for bad in [
            "https://a\"b<c>{d}",
            "https://evil.example`,_named_by_the_default_`x",
            "https://a_b.example",
            "https://a|b",
            "https://a\\b",
            "https://a^b",
            "https://a'b",
            "https://a(b)",
            "https://a;b",
            "https://a!b:443",
        ] {
            let error = Origin::parse(bad).unwrap_err();
            assert_eq!(error.code(), Code::Usage, "{bad}");
            assert!(
                error.to_string().ends_with(
                    "is not an API origin: its host holds a character other than a letter, a \
                     digit, `-`, or `.`"
                ),
                "{error}"
            );
            // The message quotes the value through `phrase::quote`, so a backtick in it
            // cannot end the literal.
            assert!(error.to_string().starts_with(&quote(bad)), "{error}");
        }
        for good in [
            "https://api-2.hippius.com",
            "https://127.0.0.1:8443",
            "https://API.example.",
        ] {
            Origin::parse(good).unwrap();
        }
    }

    #[test]
    fn a_default_port_is_the_same_origin() {
        let plain = Origin::parse("https://api.hippius.com").unwrap();
        assert_eq!(
            Origin::parse("https://API.hippius.com:443/").unwrap(),
            plain
        );
        assert_eq!(
            Origin::parse("http://127.0.0.1:80").unwrap().as_str(),
            "http://127.0.0.1"
        );
        assert_eq!(
            Origin::parse("https://api.hippius.com:8443")
                .unwrap()
                .as_str(),
            "https://api.hippius.com:8443"
        );
        assert_eq!(
            Origin::parse("http://[::1]:80").unwrap().as_str(),
            "http://[::1]"
        );
        assert!(!plain.is_plain_http());
        assert!(Origin::parse("http://127.0.0.1:9").unwrap().is_plain_http());
    }

    #[test]
    fn origins_are_https_or_loopback_http() {
        for (text, origin) in [
            ("https://api.hippius.com", "https://api.hippius.com"),
            ("https://API.Hippius.com/", "https://api.hippius.com"),
            (
                "HTTPS://api.hippius.com:8443",
                "https://api.hippius.com:8443",
            ),
            ("http://127.0.0.1:9", "http://127.0.0.1:9"),
            ("http://[::1]:7", "http://[::1]:7"),
        ] {
            assert_eq!(Origin::parse(text).unwrap().as_str(), origin, "{text}");
        }
        for bad in [
            "api.hippius.com",
            "http://api.hippius.com",
            "http://127.0.0.2",
            "http://localhost:8080",
            "ftp://api.hippius.com",
            "https://",
            "https:///",
            "https://api.hippius.com/api",
            "https://api.hippius.com?x=1",
            "https://user:pass@api.hippius.com",
            "https://api.hippius .com",
            "https://api.hippius.com\n",
        ] {
            assert_eq!(
                Origin::parse(bad).unwrap_err().code(),
                Code::Usage,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn host_of_handles_ports_and_ipv6() {
        assert_eq!(host_of("api.hippius.com:443"), "api.hippius.com");
        assert_eq!(host_of("api.hippius.com"), "api.hippius.com");
        assert_eq!(host_of("[::1]:7"), "[::1]");
        assert_eq!(host_of("[::1"), "[::1");
    }
}
