//! `hipc support tickets messages attach`: upload one file to a message of a ticket.
//!
//! The file goes to a person on the support team, so it is held to what a ticket's text is
//! held to and to two local limits the API does not document (see [`MAX_BYTES`] and
//! [`content_type`]), all before the dry run, the token check, and the question.

use hippius::routes::support;
use hippius::{Error, FileForm, Request, Result, phrase};

use crate::cli::MessageAttach;
use crate::commands::support::{Secret, secret_in};
use crate::commands::{Input, Printed, print_body};
use crate::context::{self, Context, Gate};

/// The largest file attached, in bytes: 5 MB. Swagger documents no limit on the route; the
/// support chat's upload takes pictures of up to 5 MB (swagger's description of `POST
/// /api/chat/conversations/{ticket_id}/attachments/`), so the ticket route is assumed to take
/// as much, and a larger file is refused before it is read whole.
const MAX_BYTES: u64 = 5_000_000;

/// The form field the file goes in: swagger's `formData` parameter `file` of the route.
const FIELD: &str = "file";

pub(super) fn run(ctx: &Context<'_>, args: &MessageAttach) -> Result<Printed> {
    let input = Input {
        path: &args.file,
        flag: "--file",
        what: "attachment",
    };
    let contents = input.read_file(MAX_BYTES)?;
    if contents.is_empty() {
        return Err(Error::usage(
            format!("{} is empty", input.name()),
            "pass `--file` the file that holds what to attach",
        ));
    }
    let Some(kind) = content_type(&contents) else {
        return Err(Error::usage(
            format!(
                "{} is not a PNG, JPEG, GIF, WebP, or PDF file, nor UTF-8 text, the types \
                 hipc attaches",
                input.name()
            ),
            "save it as one of those, such as a screenshot as PNG or a log as plain text",
        ));
    };
    let client = client_without_secret_in(ctx, &input, &contents)?;
    let name = args.file.file_name().unwrap_or_default().to_string_lossy();
    let form = FileForm::new(FIELD, &name, kind, &contents)?;
    let request = Request::to(
        &support::ATTACHMENT_CREATE,
        [&args.ticket_id, &args.message_id],
    )
    .about(format!(
        "attach {} ({} bytes, {kind}) to the message `{}` of the ticket `{}`",
        phrase::quote(form.filename()),
        contents.len(),
        args.message_id,
        args.ticket_id
    ))
    .listed_by(format!(
        "hipc support tickets messages list {}",
        args.ticket_id
    ))
    .file_form(form);
    let gate = Gate::Confirm {
        consequence: Some("a person on the Hippius support team reads it"),
    };
    // Swagger answers 201 with the `TicketAttachment`, printed as sent.
    print_body(ctx, &client, &request, gate)
}

/// The type a file is sent as, read from its first bytes, or `None` for a type hipc does not
/// attach. The file name is not looked at: a name says nothing a person reading the ticket
/// can rely on, and the server reads the bytes as well (swagger: "refused file"). Pictures, a
/// PDF, and text cover what a ticket needs: a screenshot, an invoice, a log.
fn content_type(bytes: &[u8]) -> Option<&'static str> {
    // The signatures each format's specification opens a file with: PNG (ISO 15948 section
    // 5.2), JPEG's start-of-image marker and the next marker's first byte (ITU T.81 annex B),
    // GIF (GIF89a section 17), WebP's RIFF header (RFC 9649 section 2.4), and PDF (ISO 32000-2
    // section 7.5.2).
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if bytes.starts_with(b"%PDF-") {
        Some("application/pdf")
    } else if !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok() {
        // A NUL byte is valid UTF-8, but no text file holds one; a binary file of ASCII
        // bytes does. The charset is named because `text/plain` alone means US-ASCII (RFC
        // 2046 section 4.1.2).
        Some("text/plain; charset=utf-8")
    } else {
        None
    }
}

/// The client for an attachment whose bytes are `contents`, after a refusal of a file that
/// holds a secret ([`secret_in`]). The bytes are read as text, with what is not UTF-8 read as
/// a replacement character: a text file is scanned whole, and a picture or a PDF for the text
/// it carries as is, such as an image's metadata or a PDF's uncompressed text. A token drawn
/// in a screenshot, or held in a compressed stream, is not found; the check catches an
/// accident, such as a log or a config file attached by mistake, as it does for a ticket's
/// text. A run of BIP-39 words in binary data whose checksum holds is as unlikely as in prose.
fn client_without_secret_in(
    ctx: &Context<'_>,
    input: &Input<'_>,
    contents: &[u8],
) -> Result<hippius::Client> {
    let config = context::config()?;
    let text = String::from_utf8_lossy(contents);
    if let Some(secret) = secret_in(ctx, &config, &[&text])? {
        return Err(refused(input, secret));
    }
    ctx.authed_client(&config)
}

/// The refusal of a file that holds `secret`. It never quotes the file's bytes.
fn refused(input: &Input<'_>, secret: Secret) -> Error {
    Error::usage(
        format!(
            "{} holds {}, which a person on the Hippius support team would read",
            input.name(),
            secret.what()
        ),
        format!(
            "remove {} from the file; the support team never needs it",
            secret.it()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "text/plain; charset=utf-8";

    #[test]
    fn each_type_is_read_from_its_first_bytes() {
        let webp = b"RIFF\x10\0\0\0WEBPVP8 ";
        for (bytes, kind) in [
            (&b"\x89PNG\r\n\x1a\nrest"[..], Some("image/png")),
            (b"\xff\xd8\xff\xe0", Some("image/jpeg")),
            (b"GIF87a..", Some("image/gif")),
            (b"GIF89a..", Some("image/gif")),
            (webp, Some("image/webp")),
            (b"%PDF-1.7\n", Some("application/pdf")),
            (b"plain words\n", Some(TEXT)),
            ("r\u{e9}sum\u{e9}\r\n".as_bytes(), Some(TEXT)),
            // One byte short of each signature.
            (b"\x89PNG\r\n\x1a", None),
            (b"%PDF", Some(TEXT)),
            (b"\xff\xd8", None),
            (b"GIF88a", Some(TEXT)),
            (b"RIFF\x10\0\0\0WEBQ", None),
            (b"RIFF1234WEBQ", Some(TEXT)),
            (b"RIFF1234WEB", Some(TEXT)),
            (b"text with a NUL\0", None),
            (b"\xc3\x28 not UTF-8", None),
        ] {
            assert_eq!(content_type(bytes), kind, "{bytes:?}");
        }
    }
}
