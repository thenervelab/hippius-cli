//! A `multipart/form-data` body (RFC 7578) that carries one file, for the one route `hipc`
//! uploads a file to: a support ticket message's attachment.
//!
//! The body is one part and its closing delimiter:
//!
//! ```text
//! --<boundary>\r\n
//! Content-Disposition: form-data; name="<field>"; filename="<name>"\r\n
//! Content-Type: <type>\r\n
//! \r\n
//! <the file's bytes>\r\n
//! --<boundary>--\r\n
//! ```
//!
//! The boundary is `hipc-` and a random UUID, drawn again until the file's bytes do not hold
//! it, so the delimiter never appears inside the part (RFC 2046 section 5.1.1). The file name
//! is written inside a quoted string, so a `"`, a `\`, or a control character in it, which
//! would end the string or the header line, is written as `_`, and so is a character that
//! changes how the name reads ([`changes_how_text_reads`]: a bidi override that shows
//! `exe.png` as `gnp.exe`, a zero-width joiner), since a person reads the name in the support
//! tool and in the question. RFC 7578 section 4.2 leaves
//! the name's encoding to the sender, and a name in UTF-8 is sent as it is, as browsers send
//! one. Built by hand rather than with a crate: it is one part with three header lines, and
//! ureq 3.5 has no multipart builder.

use std::fmt;

use serde_json::{Value, json};

use crate::error::Result;
use crate::ids::random_uuid;
use crate::phrase::changes_how_text_reads;

/// The name a file is sent under when its own has no character left, such as an empty one.
const UNNAMED: &str = "attachment";

/// A form of one file field, as it is sent. `Debug` shows the field, the type and the sizes,
/// never the file's name or bytes.
#[derive(Clone)]
pub struct FileForm {
    /// The form field the file is sent under, such as `file`.
    field: &'static str,
    /// The file name the part carries, with what would break its quoted string replaced.
    filename: String,
    content_type: &'static str,
    /// The file's size in bytes, without the framing.
    size: usize,
    /// Never inside the file's bytes.
    boundary: String,
    /// The whole body: the part's headers, the file, and the closing delimiter.
    body: Vec<u8>,
}

impl FileForm {
    /// The form that sends `contents` as the file `filename` of type `content_type` under the
    /// form field `field`. The file name is made safe for the part's header (see the module
    /// docs); [`FileForm::filename`] gives it as sent.
    ///
    /// # Errors
    ///
    /// `internal` when the system gives no random bytes for the boundary.
    pub fn new(
        field: &'static str,
        filename: &str,
        content_type: &'static str,
        contents: &[u8],
    ) -> Result<Self> {
        let boundaries = std::iter::repeat_with(|| random_uuid("a multipart boundary"));
        Self::with_boundaries(field, filename, content_type, contents, boundaries)
    }

    /// [`FileForm::new`] with the boundary's random part drawn from `boundaries`, an endless
    /// iterator, so a test can give one the file holds.
    fn with_boundaries(
        field: &'static str,
        filename: &str,
        content_type: &'static str,
        contents: &[u8],
        mut boundaries: impl Iterator<Item = Result<String>>,
    ) -> Result<Self> {
        let boundary = loop {
            let random = boundaries
                .next()
                .expect("the boundaries are an endless iterator")?;
            let boundary = format!("hipc-{random}");
            if !holds(contents, boundary.as_bytes()) {
                break boundary;
            }
        };
        let filename = header_safe(filename);
        let head = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; \
             filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n"
        );
        let tail = format!("\r\n--{boundary}--\r\n");
        let mut body = Vec::with_capacity(head.len() + contents.len() + tail.len());
        body.extend_from_slice(head.as_bytes());
        body.extend_from_slice(contents);
        body.extend_from_slice(tail.as_bytes());
        Ok(Self {
            field,
            filename,
            content_type,
            size: contents.len(),
            boundary,
            body,
        })
    }

    /// The file name as the part sends it.
    pub fn filename(&self) -> &str {
        &self.filename
    }

    /// The request's `Content-Type`, which names the boundary.
    pub(crate) fn content_type(&self) -> String {
        format!("multipart/form-data; boundary={}", self.boundary)
    }

    /// The bytes sent.
    pub(crate) fn body(&self) -> &[u8] {
        &self.body
    }

    /// The body as a dry run shows it: the form field, with the file's name, type and size,
    /// and never its bytes.
    pub(crate) fn shown(&self) -> Value {
        json!({
            self.field: {
                "filename": self.filename,
                "content_type": self.content_type,
                "bytes": self.size,
            }
        })
    }
}

impl fmt::Debug for FileForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileForm")
            .field("field", &self.field)
            .field("filename_bytes", &self.filename.len())
            .field("content_type", &self.content_type)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// Whether `bytes` holds `needle` anywhere.
fn holds(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|window| window == needle)
}

/// `name` with each character that would end a quoted header string or its line, a `"`, a
/// `\`, or a control character, or that changes how the name reads, written as `_`, and
/// [`UNNAMED`] for an empty name.
fn header_safe(name: &str) -> String {
    if name.is_empty() {
        return UNNAMED.to_owned();
    }
    name.chars()
        .map(|c| {
            if c == '"' || c == '\\' || c.is_control() || changes_how_text_reads(c) {
                '_'
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The boundaries a test draws: `x`, then `y`, then `z` for ever.
    fn fixed() -> impl Iterator<Item = Result<String>> {
        ["x", "y"]
            .into_iter()
            .chain(std::iter::repeat("z"))
            .map(|random| Ok(random.to_owned()))
    }

    #[test]
    fn the_body_is_one_part_and_its_closing_delimiter_byte_for_byte() {
        let form = FileForm::with_boundaries("file", "shot.png", "image/png", b"\x89PNG", fixed())
            .unwrap();
        let expected: &[u8] = b"--hipc-x\r\n\
            Content-Disposition: form-data; name=\"file\"; filename=\"shot.png\"\r\n\
            Content-Type: image/png\r\n\
            \r\n\
            \x89PNG\r\n\
            --hipc-x--\r\n";
        assert_eq!(form.body(), expected);
        assert_eq!(form.content_type(), "multipart/form-data; boundary=hipc-x");
        assert_eq!(form.filename(), "shot.png");
        assert_eq!(
            form.shown(),
            json!({"file": {"filename": "shot.png", "content_type": "image/png", "bytes": 4}})
        );
    }

    /// A file that holds a boundary gets another, so the delimiter never appears inside it,
    /// at its start, its end, or in between.
    #[test]
    fn a_boundary_the_file_holds_is_drawn_again() {
        for contents in [&b"hipc-x"[..], b"a hipc-x b", b"--hipc-x--", b"ahipc-y"] {
            let form = FileForm::with_boundaries("file", "a.txt", "text/plain", contents, fixed())
                .unwrap();
            let boundary = form.content_type().rsplit('=').next().unwrap().to_owned();
            assert!(!holds(contents, boundary.as_bytes()), "{boundary}");
            let expected = if contents.ends_with(b"hipc-y") {
                "hipc-x"
            } else {
                "hipc-y"
            };
            assert_eq!(boundary, expected);
        }
        // Drawn until one fits: `x` and `y` both inside.
        let form = FileForm::with_boundaries("file", "a", "text/plain", b"hipc-x hipc-y", fixed())
            .unwrap();
        assert_eq!(form.content_type(), "multipart/form-data; boundary=hipc-z");
    }

    #[test]
    fn a_failed_draw_is_the_error() {
        let failing = std::iter::repeat_with(|| Err(crate::Error::internal("no random bytes")));
        let error =
            FileForm::with_boundaries("file", "a", "text/plain", b"a", failing).unwrap_err();
        assert_eq!(error.code(), crate::Code::Internal);
    }

    #[test]
    fn a_random_boundary_is_a_uuid_the_file_does_not_hold() {
        let form = FileForm::new("file", "a.txt", "text/plain", b"hello").unwrap();
        let header = form.content_type();
        let boundary = header
            .strip_prefix("multipart/form-data; boundary=")
            .unwrap();
        assert_eq!(boundary.len(), "hipc-".len() + 36);
        assert!(
            form.body()
                .starts_with(format!("--{boundary}\r\n").as_bytes())
        );
        assert!(
            form.body()
                .ends_with(format!("\r\n--{boundary}--\r\n").as_bytes())
        );
    }

    /// A quote, a backslash, or a control character cannot end the quoted name or its line;
    /// other characters, UTF-8 included, are sent as they are.
    #[test]
    fn the_file_name_cannot_break_out_of_its_quoted_string() {
        for (name, sent) in [
            ("a\"b.txt", "a_b.txt"),
            ("a\\b.txt", "a_b.txt"),
            ("a\r\nContent-Type: x.txt", "a__Content-Type: x.txt"),
            ("tab\there\u{7f}\u{85}", "tab_here__"),
            ("résumé 1.pdf", "résumé 1.pdf"),
            // A right-to-left override, a zero-width joiner, a soft hyphen, a word joiner.
            ("a\u{202e}gnp.exe", "a_gnp.exe"),
            ("in\u{200d}voice\u{ad}.pdf\u{2060}", "in_voice_.pdf_"),
            ("", "attachment"),
        ] {
            let form =
                FileForm::with_boundaries("file", name, "text/plain", b"a", fixed()).unwrap();
            assert_eq!(form.filename(), sent, "{name:?}");
            let disposition = format!("filename=\"{sent}\"\r\n");
            assert!(holds(form.body(), disposition.as_bytes()), "{name:?}");
        }
    }

    #[test]
    fn debug_shows_the_sizes_not_the_name_or_the_bytes() {
        let form =
            FileForm::with_boundaries("file", "a.txt", "text/plain", b"my words", fixed()).unwrap();
        let shown = format!("{form:?}");
        assert!(!shown.contains("my words"), "{shown}");
        assert!(shown.contains("size: 8"), "{shown}");
        assert!(!shown.contains("a.txt"), "{shown}");
        assert!(shown.contains("filename_bytes: 5"), "{shown}");
    }
}
