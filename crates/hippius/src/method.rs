//! The HTTP method of a request, and whether the request may change state: the two facts every
//! route table and the client key on.

use std::fmt;

use crate::error::{Error, Result};
use crate::phrase::quote;

/// The HTTP methods `hipc` sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Reads a resource (RFC 9110, section 9.3.1).
    Get,
    /// Reads a resource's headers only (RFC 9110, section 9.3.2).
    Head,
    /// Creates a resource or runs an action (RFC 9110, section 9.3.3).
    Post,
    /// Replaces a resource whole (RFC 9110, section 9.3.4).
    Put,
    /// Changes part of a resource (RFC 5789).
    Patch,
    /// Removes a resource (RFC 9110, section 9.3.5).
    Delete,
}

impl Method {
    /// The names [`Method::parse`] accepts, in upper case.
    const NAMES: [&'static str; 6] = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"];

    /// Parses a method name, in any case.
    ///
    /// # Errors
    ///
    /// `usage` for any other name.
    pub fn parse(text: &str) -> Result<Self> {
        match text.to_ascii_uppercase().as_str() {
            "GET" => Ok(Self::Get),
            "HEAD" => Ok(Self::Head),
            "POST" => Ok(Self::Post),
            "PUT" => Ok(Self::Put),
            "PATCH" => Ok(Self::Patch),
            "DELETE" => Ok(Self::Delete),
            _ => {
                let names = crate::phrase::join(&Self::NAMES.map(crate::phrase::quote), "or");
                Err(Error::usage(
                    format!("{} is not an HTTP method: a method is {names}", quote(text)),
                    format!("pass the method as {names}"),
                ))
            }
        }
    }

    /// The upper-case name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }

    /// Whether the method only reads: GET and HEAD. Every other method may change state.
    pub fn is_read(self) -> bool {
        matches!(self, Self::Get | Self::Head)
    }

    /// Whether a request with this method always carries a body, an empty one when none is
    /// given. A DELETE carries one only when it is given.
    pub(crate) fn always_has_body(self) -> bool {
        matches!(self, Self::Post | Self::Put | Self::Patch)
    }
}

/// Whether a request may change state on the server, which decides the stored-selection
/// rule of the project and billing headers and the side effects of a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    /// It changes nothing.
    Read,
    /// It may change state.
    Write,
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;

    #[test]
    fn methods_parse_in_any_case_and_know_their_bodies() {
        for (text, method, read, body) in [
            ("get", Method::Get, true, false),
            ("HEAD", Method::Head, true, false),
            ("Post", Method::Post, false, true),
            ("put", Method::Put, false, true),
            ("PATCH", Method::Patch, false, true),
            ("delete", Method::Delete, false, false),
        ] {
            let parsed = Method::parse(text).unwrap();
            assert_eq!(parsed, method);
            assert_eq!(parsed.is_read(), read, "{text}");
            assert_eq!(parsed.always_has_body(), body, "{text}");
            assert_eq!(parsed.to_string(), text.to_ascii_uppercase());
        }
        assert_eq!(Method::parse("TRACE").unwrap_err().code(), Code::Usage);
    }
}
