//! Managed databases.

use serde::{Deserialize, Serialize};

use crate::routes::Route;
use crate::view::{field, nested};

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

/// The body of [`DATABASE_CREATE`]: the tier, size, and engine version, and `name` when given
/// (swagger: "nothing else").
#[derive(Debug, Serialize)]
pub struct DatabaseCreateBody<'a> {
    /// The tier's code.
    pub tier: &'a str,
    /// The size's code.
    pub size: &'a str,
    /// The PostgreSQL major version.
    pub engine_version: &'a str,
    /// The database's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<&'a str>,
}

/// The body of [`DATABASE_RESTORE`] to a point in time; a restore to the end of the backups
/// sends no body.
#[derive(Debug, Serialize)]
pub struct DatabaseRestoreBody<'a> {
    /// The time restored to, RFC 3339.
    pub target_time: &'a str,
}

/// The answer of [`DATABASE_CREATE`] and [`DATABASE_RESTORE`], a new database, as far as the
/// files `--credentials-dir` writes and the note decide on it. Its `Debug` shows which fields
/// it holds, never their values: `credentials` holds the one-time secrets.
#[derive(Default, Deserialize)]
pub struct CreatedDatabase {
    /// The new database's id.
    #[serde(default, deserialize_with = "field")]
    pub id: Option<u64>,
    /// The one-time secrets.
    #[serde(default, deserialize_with = "nested")]
    pub credentials: Option<Credentials>,
    /// Where the database listens.
    #[serde(default, deserialize_with = "nested")]
    pub connection: Option<Connection>,
}

impl std::fmt::Debug for CreatedDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreatedDatabase")
            .field("id", &self.id)
            .field("credentials", &self.credentials)
            .field("connection", &self.connection.is_some())
            .finish()
    }
}

/// The `credentials` of a [`CreatedDatabase`], which no route shows again. Its `Debug` says
/// which fields it holds, never their values.
#[derive(Default, Deserialize)]
pub struct Credentials {
    /// The role to connect as.
    #[serde(default, deserialize_with = "field")]
    pub username: Option<String>,
    /// The role's password.
    #[serde(default, deserialize_with = "field")]
    pub password: Option<String>,
    /// The database to connect to.
    #[serde(default, deserialize_with = "field")]
    pub database: Option<String>,
    /// The TLS client certificate, its key, and the CA.
    #[serde(default, deserialize_with = "nested")]
    pub tls: Option<Tls>,
}

/// The `credentials.tls` of a [`CreatedDatabase`], each a PEM text. Its `Debug` says which
/// fields it holds, never their values.
#[derive(Default, Deserialize)]
pub struct Tls {
    /// The client certificate.
    #[serde(default, deserialize_with = "field")]
    pub client_cert: Option<String>,
    /// The client certificate's private key.
    #[serde(default, deserialize_with = "field")]
    pub client_key: Option<String>,
    /// The CA that signed the server's certificate.
    #[serde(default, deserialize_with = "field")]
    pub ca_cert: Option<String>,
}

impl Tls {
    /// The PEM text of `field`, one of `client_cert`, `client_key`, and `ca_cert`; `None` for
    /// a field it does not have.
    pub fn pem(&self, field: &str) -> Option<&str> {
        match field {
            "client_cert" => self.client_cert.as_deref(),
            "client_key" => self.client_key.as_deref(),
            "ca_cert" => self.ca_cert.as_deref(),
            _ => None,
        }
    }
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username.is_some())
            .field("password", &self.password.is_some())
            .field("database", &self.database.is_some())
            .field("tls", &self.tls)
            .finish()
    }
}

impl std::fmt::Debug for Tls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tls")
            .field("client_cert", &self.client_cert.is_some())
            .field("client_key", &self.client_key.is_some())
            .field("ca_cert", &self.ca_cert.is_some())
            .finish()
    }
}

/// The `connection` of a [`CreatedDatabase`].
#[derive(Debug, Default, Deserialize)]
pub struct Connection {
    /// The host to connect to.
    #[serde(default, deserialize_with = "field")]
    pub host: Option<String>,
    /// The port to connect to.
    #[serde(default, deserialize_with = "field")]
    pub port: Option<u64>,
    /// The libpq `sslmode` the database expects.
    #[serde(default, deserialize_with = "field")]
    pub sslmode: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::view::view_of;

    /// Each TLS field is read by its name, and a name the answer does not have is none.
    #[test]
    fn a_tls_field_is_read_by_its_name() {
        let created: CreatedDatabase = view_of(&json!({
            "credentials": {"tls": {"client_cert": "c", "client_key": "k", "ca_cert": "a"}},
        }));
        let tls = created
            .credentials
            .and_then(|credentials| credentials.tls)
            .unwrap();
        let read: Vec<Option<&str>> = ["client_cert", "client_key", "ca_cert", "ca"]
            .into_iter()
            .map(|field| tls.pem(field))
            .collect();
        assert_eq!(read, [Some("c"), Some("k"), Some("a"), None]);
    }

    /// The `Debug` of a new database names what it holds and never a secret.
    #[test]
    fn a_new_database_debug_shows_no_secret() {
        let created: CreatedDatabase = view_of(&json!({
            "id": 7,
            "credentials": {"password": "hunter2", "tls": {"client_key": "pem-key"}},
            "connection": {"host": "h"},
        }));
        let shown = format!("{created:?}");
        assert!(
            !shown.contains("hunter2") && !shown.contains("pem-key"),
            "{shown}"
        );
        assert!(shown.contains("id: Some(7)"), "{shown}");
    }
}
