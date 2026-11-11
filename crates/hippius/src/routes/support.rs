//! Support tickets, and the status of the Hippius services.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::routes::Route;
use crate::view::field;

routes! {
    /// The caller's tickets.
    TICKETS: 0 = Route::get("/api/support/tickets/").personal().paged();

    /// Opens a ticket.
    TICKET_CREATE: 0 = Route::post("/api/support/tickets/").personal();

    /// One ticket.
    TICKET: 1 = Route::get("/api/support/tickets/{ticket_id}/").personal();

    /// Changes a ticket, such as closing it.
    TICKET_UPDATE: 1 = Route::patch("/api/support/tickets/{ticket_id}/").personal();

    /// A ticket's messages.
    MESSAGES: 1 = Route::get("/api/support/tickets/{ticket_id}/messages/").personal().paged();

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
