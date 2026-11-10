//! GitHub Actions runners: the installations, jobs, settings, limit, and usage.

use crate::routes::Route;

routes! {
    /// Starts linking a GitHub installation.
    CONNECT: 0 = Route::post("/api/runners/github/connect/").scoped().project();

    /// The linked GitHub installations.
    INSTALLATIONS: 0 = Route::get("/api/runners/installations/").scoped().project();

    /// Unlinks a GitHub installation.
    INSTALLATION_DELETE: 1 = Route::delete("/api/runners/installations/{installation_id}/")
        .scoped()
        .project();

    /// The runner jobs.
    JOBS: 0 = Route::get("/api/runners/jobs/").scoped().project().paged();

    /// The runner spend limit.
    LIMIT: 0 = Route::get("/api/runners/limit/").scoped().project();

    /// Replaces the runner spend limit.
    LIMIT_UPDATE: 0 = Route::put("/api/runners/limit/").scoped().project();

    /// The runner settings.
    SETTINGS: 0 = Route::get("/api/runners/settings/").scoped().project();

    /// Replaces the runner settings.
    SETTINGS_UPDATE: 0 = Route::put("/api/runners/settings/").scoped().project();

    /// The runner usage.
    USAGE: 0 = Route::get("/api/runners/usage/").scoped().project();
}
