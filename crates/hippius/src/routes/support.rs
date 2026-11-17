//! Support tickets, and the status of the Hippius services.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::routes::{Paging, Route};
use crate::view::field;

routes! {
    /// The caller's tickets.
    TICKETS: 0 = Route::get("/api/support/tickets/").personal().paged(Paging::Numbered);

    /// Opens a ticket.
    TICKET_CREATE: 0 = Route::post("/api/support/tickets/").personal();

    /// One ticket.
    TICKET: 1 = Route::get("/api/support/tickets/{ticket_id}/").personal();

    /// Changes a ticket, such as closing it.
    TICKET_UPDATE: 1 = Route::patch("/api/support/tickets/{ticket_id}/").personal();

    /// A ticket's messages.
    MESSAGES: 1 = Route::get("/api/support/tickets/{ticket_id}/messages/").personal().paged(Paging::Numbered);

    /// Replies on a ticket.
    MESSAGE_CREATE: 1 = Route::post("/api/support/tickets/{ticket_id}/messages/").personal();

    /// Attaches a file to a message of a ticket, as a `multipart/form-data` upload.
    ATTACHMENT_CREATE: 2 =
        Route::post("/api/support/tickets/{ticket_id}/messages/{message_id}/attachments/")
            .personal();

    /// The status of the Hippius services.
    SERVICES_STATUS: 0 = Route::get("/api/services/status/").own_account();
}

/// The body of [`TICKET_CREATE`]. `Debug` shows the sizes of the texts, not the texts.
#[derive(Serialize)]
pub struct TicketCreateBody<'a> {
    /// The ticket's subject.
    pub subject: &'a str,
    /// What the ticket asks.
    pub description: &'a str,
    /// Its priority, when one is chosen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<&'a str>,
    /// Its category, when one is given: free text, since swagger lists no values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<&'a str>,
    /// The kind of resource it is about, when one is given: free text, as `category` is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_type: Option<&'a str>,
    /// The id of the resource it is about, when one is given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<&'a str>,
}

impl fmt::Debug for TicketCreateBody<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TicketCreateBody")
            .field("subject_bytes", &self.subject.len())
            .field("description_bytes", &self.description.len())
            .field("priority", &self.priority)
            .field("category_bytes", &self.category.map(str::len))
            .field("resource_type_bytes", &self.resource_type.map(str::len))
            .field("resource_id_bytes", &self.resource_id.map(str::len))
            .finish()
    }
}

/// The body of [`TICKET_UPDATE`].
#[derive(Debug, Serialize)]
pub struct TicketUpdateBody<'a> {
    /// The new status, such as `closed`.
    pub status: &'a str,
}

/// The body of [`MESSAGE_CREATE`]. `Debug` shows the size of the text, not the text.
#[derive(Serialize)]
pub struct MessageCreateBody<'a> {
    /// The reply's text.
    pub body: &'a str,
    /// The id the client gives the reply. Swagger types it as a string of at most 64
    /// characters and says nothing of what the server does with a repeated one.
    pub client_msg_id: &'a str,
}

impl fmt::Debug for MessageCreateBody<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MessageCreateBody")
            .field("body_bytes", &self.body.len())
            .field("client_msg_id", &self.client_msg_id)
            .finish()
    }
}

/// A ticket of [`TICKET`], as far as `hipc` decides on it.
#[derive(Debug, Default, Deserialize)]
pub struct Ticket {
    /// Its status, such as `closed`.
    #[serde(default, deserialize_with = "field")]
    pub status: Option<String>,
    /// Its subject.
    #[serde(default, deserialize_with = "field")]
    pub subject: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `Debug` of a body that holds the user's text shows its size, not the text.
    #[test]
    fn a_text_body_debugs_as_its_sizes() {
        let create = TicketCreateBody {
            subject: "help",
            description: "my words",
            priority: None,
            category: Some("my words"),
            resource_type: Some("my words"),
            resource_id: Some("my words"),
        };
        let reply = MessageCreateBody {
            body: "my words",
            client_msg_id: "id",
        };
        for shown in [format!("{create:?}"), format!("{reply:?}")] {
            assert!(!shown.contains("my words"), "{shown}");
            assert!(shown.contains("_bytes: 8"), "{shown}");
        }
        let shown = format!("{create:?}");
        for field in ["category", "resource_type", "resource_id"] {
            assert!(
                shown.contains(&format!("{field}_bytes: Some(8)")),
                "{shown}"
            );
        }
    }
}
