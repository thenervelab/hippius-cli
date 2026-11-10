//! Support tickets, and the status of the Hippius services.

use crate::routes::Route;

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
