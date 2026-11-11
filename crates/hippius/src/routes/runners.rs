//! GitHub Actions runners: the installations, jobs, settings, limit, and usage.

use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

use crate::routes::Route;
use crate::view::{Slot, TextOrNumber, field};

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

/// The body of [`LIMIT_UPDATE`]. Swagger types no body: the shape is the CDN zone cap's.
#[derive(Debug, Serialize)]
pub struct LimitUpdateBody<'a> {
    /// The monthly cap in credits, as a decimal string; `null` is no cap.
    pub monthly_cap: Option<&'a str>,
}

/// The body of [`SETTINGS_UPDATE`]: only the settings given, since the server keeps the ones
/// not sent.
#[derive(Debug, Default, Serialize)]
pub struct SettingsUpdateBody<'a> {
    /// Whether jobs run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// The most jobs at once.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent: Option<u32>,
    /// The runner sizes a job may ask for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_sizes: Option<&'a [&'a str]>,
    /// Where a job runs when it names no region; empty for anywhere.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_region: Option<&'a str>,
    /// Whether a job of a public repository may run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_public_repos: Option<bool>,
}

/// The settings of [`SETTINGS`], as far as whether an update may spend more decides on them.
#[derive(Debug, Default, Deserialize)]
pub struct Settings {
    /// Whether jobs run.
    #[serde(default, deserialize_with = "field")]
    pub enabled: Option<bool>,
    /// The most jobs at once.
    #[serde(default, deserialize_with = "field")]
    pub max_concurrent: Option<u64>,
    /// The runner sizes a job may ask for.
    #[serde(default, deserialize_with = "field")]
    pub allowed_sizes: Option<Vec<String>>,
    /// Whether a job of a public repository may run.
    #[serde(default, deserialize_with = "field")]
    pub allow_public_repos: Option<bool>,
}

/// The spend limit of [`LIMIT`], as far as an update decides on it. Both names are
/// unverified against the live API: the shape assumed is the CDN zone cap's.
#[derive(Debug, Default, Deserialize)]
pub struct Limit {
    /// The cap in credits, a string or a number.
    #[serde(default)]
    pub monthly_cap_credits: Slot<TextOrNumber>,
    /// The cap in base units; only whether it is `null` is read.
    #[serde(default)]
    pub monthly_cap: Slot<IgnoredAny>,
}
