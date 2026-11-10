//! Managed databases.

use crate::routes::Route;

routes! {
    /// The databases.
    DATABASES: 0 = Route::get("/api/databases/").scoped().project();

    /// Creates a database.
    DATABASE_CREATE: 0 = Route::post("/api/databases/").scoped().project().reveals();

    /// One database.
    DATABASE: 1 = Route::get("/api/databases/{database_id}/").scoped().project();

    /// Deletes a database.
    DATABASE_DELETE: 1 = Route::delete("/api/databases/{database_id}/").scoped().project();

    /// A database's events.
    DATABASE_EVENTS: 1 = Route::get("/api/databases/{database_id}/events/").scoped().project();

    /// A database's metrics.
    DATABASE_METRICS: 1 = Route::get("/api/databases/{database_id}/metrics/").scoped().project();

    /// Restarts a database.
    DATABASE_RESTART: 1 = Route::post("/api/databases/{database_id}/restart/").scoped().project();

    /// Restores a database into a new one.
    DATABASE_RESTORE: 1 = Route::post("/api/databases/{database_id}/restore/")
        .scoped()
        .project()
        .reveals();

    /// Exposes a database.
    DATABASE_EXPOSE: 1 = Route::post("/api/databases/{database_id}/expose/").scoped().project();

    /// Stops exposing a database.
    DATABASE_EXPOSURE_DELETE: 1 = Route::delete("/api/databases/{database_id}/expose/")
        .scoped()
        .project();

    /// The refused database requests.
    DATABASE_REFUSALS: 0 = Route::get("/api/databases/refusals/").scoped().project();
}
