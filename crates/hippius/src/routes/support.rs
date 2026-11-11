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
}

impl fmt::Debug for TicketCreateBody<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TicketCreateBody")
            .field("subject_bytes", &self.subject.len())
            .field("description_bytes", &self.description.len())
            .field("priority", &self.priority)
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
}

impl fmt::Debug for MessageCreateBody<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MessageCreateBody")
            .field("body_bytes", &self.body.len())
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
        };
        let reply = MessageCreateBody { body: "my words" };
        for shown in [format!("{create:?}"), format!("{reply:?}")] {
            assert!(!shown.contains("my words"), "{shown}");
            assert!(shown.contains("_bytes: 8"), "{shown}");
        }
    }
}
