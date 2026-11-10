//! `hipc compute databases`: managed PostgreSQL.
//!
//! Every route here is under `/api/databases/`, which is account-scoped, so a membership
//! sends `X-Hippius-Account`. The personal database routes (`/api/databases/metrics/`,
//! `<n>/backup-token/`, `<n>/members/`) are not wrapped. The series of one database,
//! `<n>/metrics/`, is account-scoped, and `metrics` reads it.
//!
//! The create answer is the one place the server shows a database's `credentials`: the
//! password, the backup passphrase, and the TLS client key. The server keeps no copy, and
//! no route shows them again. A restore makes a new database with its own credentials, so
//! it is treated the same way. Both commands need `--reveal`, print the answer as sent, and do
//! not wait, so no later step can lose it; every other command removes a `credentials` field at
//! any depth, should the server ever send one (tenet 9). The one change to the answer:
//! `--credentials-dir`, on either command, writes the credentials to new files (the TLS
//! certificate, key, and CA, the whole object as `credentials.json`, and the libpq settings as
//! `.env`) and prints their paths in place of every secret.

use std::path::Path;

use hippius::{Error, Method, Request, Response, Result, phrase, without_field};
use serde_json::{Map, Value, json};

use crate::cli::{
    CREDENTIALS_JSON, DatabaseCreate, DatabaseExposure, DatabaseRestore, Databases, ENV_FILE,
    TLS_FILES,
};
use crate::commands::compute::balance_hint;
use crate::commands::files::{NewFileError, write_new};
use crate::commands::{Printed, keyed_list, print_list_with, print_with, require_reveal_or_file};
use crate::context::{Context, Gate};

/// `DELETE /api/databases/<id>/`, which names in its question and a text-mode note what the
/// server keeps after it ([`left_after_delete`]).
fn delete(ctx: &Context<'_>, database_id: u64) -> Result<Printed> {
    let request = Request::new(Method::Delete, path(database_id))
        .about(format!(
            "decommission the database `{database_id}` and every VM behind it"
        ))
        .listed_by(DATABASES);
    let left = left_after_delete(database_id);
    let gate = Gate::Confirm {
        consequence: Some(&left),
    };
    print_with(
        ctx,
        &ctx.load_authed_client()?,
        &request,
        gate,
        |response| {
            let decommissioning = without_credentials(&response)?;
            ctx.note(&left);
            Ok(decommissioning)
        },
    )
}

/// What a database delete leaves behind, for its question and its note. A live run saw the
/// server keep the backup bucket and the backup writer token it made for the database after
/// the delete: the bucket is billed while it holds data, and the token is a
/// live credential, so both are named with the commands that remove them.
fn left_after_delete(database_id: u64) -> String {
    format!(
        "the server keeps the backup bucket `hippius-db-{database_id}-backups` and the S3 sub \
         token `db-{database_id}-backup-writer` it made for the database: empty the bucket with \
         an S3 client and run `hipc s3 buckets delete hippius-db-{database_id}-backups --yes`, \
         and revoke the token with `hipc s3 tokens revoke --kind sub` and its id from `hipc s3 \
         tokens list`"
    )
}

/// The field of the create answer that holds the one-time secrets.
const CREDENTIALS: &str = "credentials";

pub(super) fn run(ctx: &Context<'_>, command: &Databases) -> Result<Printed> {
    let print = |request: &Request, gate| {
        print_with(ctx, &ctx.load_authed_client()?, request, gate, |response| {
            without_credentials(&response)
        })
    };
    match command {
        // Swagger types the list as a bare array with no page parameters, so it is read
        // whole.
        Databases::List => {
            let request = Request::get("/databases/").about("list the databases");
            print_list_with(ctx, &ctx.load_authed_client()?, &request, |r| {
                let list = hippius::page(r.json()?)?.complete_envelope(request.target())?;
                Ok(without_field(list, CREDENTIALS))
            })
        }
        Databases::Get { database_id } => print(
            &Request::get(path(*database_id))
                .about(format!("get the database `{database_id}`"))
                .listed_by(DATABASES),
            Gate::Open,
        ),
        Databases::Create(args) => create(ctx, args),
        Databases::Delete { database_id } => delete(ctx, *database_id),
        Databases::Restart { database_id } => {
            let request = Request::new(Method::Post, format!("{}restart/", path(*database_id)))
                .about(format!(
                    "restart PostgreSQL on the database `{database_id}`"
                ))
                .listed_by(DATABASES);
            let gate = Gate::Confirm {
                consequence: Some("its connections end"),
            };
            let client = ctx.load_authed_client()?;
            print_with(ctx, &client, &request, gate, |response| {
                let restart = without_credentials(&response)?;
                // Swagger: the restart is "queued on the row and handed to the agent on its
                // next heartbeat", so the answer is not proof that PostgreSQL restarted.
                ctx.note(&format!(
                    "the restart is queued until the guest's next heartbeat; `restart.pending` \
                     in `hipc compute databases get {database_id}` clears once it is done"
                ));
                Ok(restart)
            })
        }
        Databases::Restore(args) => restore(ctx, args),
        Databases::Exposure(DatabaseExposure::Create { database_id }) => {
            // Who can reach it is part of the action, so an agent's refusal names it too.
            let request = Request::new(Method::Post, expose_path(*database_id))
                .about(format!(
                    "publish the primary of the database `{database_id}` on the public edge, \
                     where anyone can reach it"
                ))
                .listed_by(DATABASES);
            let gate = Gate::Confirm {
                consequence: Some("the password is still required to connect"),
            };
            print(&request, gate)
        }
        Databases::Exposure(DatabaseExposure::Delete { database_id }) => {
            let request = Request::new(Method::Delete, expose_path(*database_id))
                .about(format!(
                    "take the primary of the database `{database_id}` off the public edge"
                ))
                .listed_by(DATABASES);
            let gate = Gate::Confirm { consequence: None };
            // Swagger answers 204, so the document says what was done.
            let client = ctx.load_authed_client()?;
            print_with(ctx, &client, &request, gate, |response| {
                let body = without_field(response.json()?, CREDENTIALS);
                Ok(if body.is_null() {
                    json!({ "id": database_id, "exposed": false })
                } else {
                    body
                })
            })
        }
        // The console reads the journal as `{events: [...]}`; a bare array or a page is read
        // too, and either way it is printed as the list envelope, a `next` link refused.
        Databases::Events { database_id } => {
            let request = Request::get(format!("{}events/", path(*database_id)))
                .about(format!("list the events of the database `{database_id}`"))
                .listed_by(DATABASES);
            print_list(ctx, &request, "events")
        }
        // The metrics answer is an object: `samples`, `latest`, and `agent_last_seen`.
        Databases::Metrics { database_id } => {
            let request = Request::get(format!("{}metrics/", path(*database_id)))
                .about(format!("get the metrics of the database `{database_id}`"))
                .listed_by(DATABASES);
            print(&request, Gate::Open)
        }
        // Swagger: `{refusals: [...]}`.
        Databases::Refusals => print_list(
            ctx,
            &Request::get("/databases/refusals/").about("list the refused database requests"),
            "refusals",
        ),
    }
}

/// The command that lists the database ids, for the hint of a 404 on a database route.
const DATABASES: &str = "hipc compute databases list";

/// `request`'s answer, a list under `key` (or a bare array or a page), printed as the list
/// envelope with the answer's other fields beside `items`, and any `credentials` removed.
fn print_list(ctx: &Context<'_>, request: &Request, key: &str) -> Result<Printed> {
    print_list_with(ctx, &ctx.load_authed_client()?, request, |response| {
        let answer = without_credentials(&response)?;
        if answer.get(key).is_some() {
            keyed_list(answer, request, key)
        } else {
            hippius::page(answer)?.complete_envelope(request.target())
        }
    })
}

/// The answer's document with any `credentials` field removed: only create and restore
/// show them (module docs).
fn without_credentials(response: &Response) -> Result<Value> {
    Ok(without_field(response.document()?, CREDENTIALS))
}

fn path(id: u64) -> String {
    format!("/databases/{id}/")
}

fn expose_path(id: u64) -> String {
    format!("{}expose/", path(id))
}

/// `POST /api/databases/` with exactly `tier`, `size`, `engine_version`, and `name` when
/// given (swagger: "nothing else"). The 202 answer is printed as sent, credentials
/// included, or with the secrets replaced by their files under `--credentials-dir`
/// ([`save_credentials`]), and the command does not wait for `running`: operations that finish later
/// get a status read and no new wait, here `get` of the new id. A wait
/// would be worse: a run killed mid-wait would lose the one-time credentials.
fn create(ctx: &Context<'_>, args: &DatabaseCreate) -> Result<Printed> {
    require_reveal_or_file(ctx, args.reveal, args.credentials_dir.is_some())?;
    let mut body = Map::new();
    body.insert("tier".to_owned(), args.tier.code().into());
    body.insert("size".to_owned(), args.size.code().into());
    body.insert(
        "engine_version".to_owned(),
        args.engine_version.code().into(),
    );
    if let Some(name) = &args.name {
        body.insert("name".to_owned(), name.as_str().into());
    }
    let request = Request::new(Method::Post, "/databases/")
        .json(&Value::Object(body))
        .about(format!(
            "create a `{}` PostgreSQL {} database of size `{}`",
            args.tier.code(),
            args.engine_version.code(),
            args.size.code()
        ));
    let gate = Gate::Confirm {
        consequence: Some("it is billed until it is deleted"),
    };
    let client = ctx.load_authed_client()?;
    print_with(ctx, &client, &request, gate, |response| {
        credentials(ctx, args.credentials_dir.as_deref(), &response)
    })
    .map_err(balance_hint)
}

/// The answer of a create or a restore, a new database: printed as sent, credentials
/// included, or with the secrets replaced by their files under `dir` ([`save_credentials`]).
/// The text-mode note says where the credentials are and how to follow the new id to
/// `running`.
fn credentials(ctx: &Context<'_>, dir: Option<&Path>, response: &Response) -> Result<Value> {
    let created = document_or_raw(response)?;
    let (created, shown) = match dir {
        Some(dir) => {
            let (saved, files) = save_credentials(dir, created)?;
            let shown = format!(
                "the `credentials` are in {}, and only there: no command shows them again",
                phrase::join(&files, "and")
            );
            (saved, shown)
        }
        None if created.get(CREDENTIALS).is_some() => (
            created,
            "the `credentials` are shown only now, in this document".to_owned(),
        ),
        None => (created, "the answer holds no `credentials`".to_owned()),
    };
    match created.get("id").and_then(Value::as_u64) {
        Some(id) => ctx.note(&format!(
            "{shown}; run `hipc compute databases get {id}` to follow the database to \
             `running`"
        )),
        None => ctx.note(&shown),
    }
    Ok(created)
}

/// The fields of `credentials` the document keeps under `--credentials-dir`, which name the
/// database and hold no secret. Every other field, a new one the server adds included, goes
/// only to `credentials.json`: this fails closed.
const SHOWN_WITH_FILES: [&str; 3] = ["username", "database", "shown_once"];

/// Writes the credentials of the create or restore answer `created` to new files in `dir`:
/// the TLS client certificate, its key, and the CA ([`TLS_FILES`]), the whole `credentials`
/// object ([`CREDENTIALS_JSON`]), and the libpq settings ([`ENV_FILE`], [`env_file`]). It
/// returns the answer with `credentials` cut to [`SHOWN_WITH_FILES`] and the paths, each TLS
/// value replaced by its file (`client_key` becomes `client_key_file`), `json_file`, and
/// `env_file`, and the quoted paths. `dir` was checked at parse time.
///
/// Tenet 9: a one-time secret goes on stdout, or into new files the caller named, and
/// nowhere else. So every failure here is `internal` after the write with the whole answer,
/// every credential included, on stdout: the database exists, and a rerun would create a
/// second one. Nothing is written unless the answer holds all three TLS values and every
/// value `.env` takes can be written there. A file written before a later one failed stays,
/// since it holds a secret that is not shown again, and the message names it. [`write_new`]
/// syncs the directory after each file, so their names last.
fn save_credentials(dir: &Path, created: Value) -> Result<(Value, Vec<String>)> {
    let hint = "save the `credentials` from stdout now, since no command shows them again";
    let refused = |message: String, created: Value| {
        Err(Error::internal(message)
            .with_hint(hint)
            .after_write(created))
    };
    let mut files = Vec::new();
    for (file, field) in TLS_FILES {
        let path = dir.join(file);
        let pem = created
            .pointer(&format!("/{CREDENTIALS}/tls/{field}"))
            .and_then(Value::as_str);
        let Some(pem) = pem else {
            let message = format!(
                "the answer has no `credentials.tls.{field}` to write to {}, so no file was \
                 written; every credential is in the answer on stdout",
                phrase::quote(path.display())
            );
            return refused(message, created);
        };
        files.push((path, pem.as_bytes().to_vec()));
    }
    let object = &created[CREDENTIALS];
    let mut json = serde_json::to_vec_pretty(object).expect("a JSON value serializes");
    json.push(b'\n');
    files.push((dir.join(CREDENTIALS_JSON), json));
    let env = match env_file(dir, &created) {
        Ok(env) => env,
        Err(why) => {
            let message = format!(
                "{why}, so no file was written; every credential is in the answer on stdout"
            );
            return refused(message, created);
        }
    };
    files.push((dir.join(ENV_FILE), env.into_bytes()));
    let mut written = Vec::new();
    for (path, bytes) in &files {
        if let Err(NewFileError::Create(failed) | NewFileError::Write(failed)) =
            write_new(path, bytes)
        {
            let before = match written.len() {
                0 => String::new(),
                n => format!(
                    "; {} {} written",
                    phrase::join(&written, "and"),
                    if n == 1 { "was" } else { "were" }
                ),
            };
            let message = format!("{failed}{before}; every credential is in the answer on stdout");
            return refused(message, created);
        }
        written.push(phrase::quote(path.display()));
    }
    let named = |path: &Path| Value::from(path.to_string_lossy());
    let mut shown = Map::new();
    for field in SHOWN_WITH_FILES {
        if let Some(value) = object.get(field) {
            shown.insert(field.to_owned(), value.clone());
        }
    }
    let tls = TLS_FILES
        .iter()
        .map(|(file, field)| (format!("{field}_file"), named(&dir.join(file))))
        .collect();
    shown.insert("tls".to_owned(), Value::Object(tls));
    shown.insert("json_file".to_owned(), named(&dir.join(CREDENTIALS_JSON)));
    shown.insert("env_file".to_owned(), named(&dir.join(ENV_FILE)));
    let secrets = withheld(object);
    let mut saved = created;
    saved[CREDENTIALS] = Value::Object(shown);
    Ok((redacted(saved, &secrets), written))
}

/// Every text of `credentials` the document no longer shows, at any depth, longest first:
/// the password, the backup passphrase, the TLS values, and any new field.
fn withheld(object: &Value) -> Vec<String> {
    fn texts(value: &Value, into: &mut Vec<String>) {
        match value {
            Value::String(text) if !text.is_empty() => into.push(text.clone()),
            Value::Object(fields) => fields.values().for_each(|value| texts(value, into)),
            Value::Array(items) => items.iter().for_each(|item| texts(item, into)),
            _ => {}
        }
    }
    let mut secrets = Vec::new();
    for (field, value) in object.as_object().into_iter().flatten() {
        if !SHOWN_WITH_FILES.contains(&field.as_str()) {
            texts(value, &mut secrets);
        }
    }
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    secrets
}

/// `value` with every one of `secrets` in its strings replaced by `<redacted>`, all at once
/// ([`hippius::redact_all`]): a field outside `credentials`, such as a
/// `connection_string` that embeds the password, must not print what `--credentials-dir`
/// moved to its files.
fn redacted(value: Value, secrets: &[String]) -> Value {
    match value {
        Value::String(text) => Value::String(hippius::redact_all(text, secrets)),
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .map(|(key, value)| (key, redacted(value, secrets)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| redacted(item, secrets))
                .collect(),
        ),
        other => other,
    }
}

/// The `.env` `--credentials-dir` writes: the libpq settings a client such as psql reads from
/// the environment (<https://www.postgresql.org/docs/current/libpq-envars.html>), from the
/// answer's `credentials`, `PGHOST` and `PGPORT` from its `connection` when it names them, and
/// the TLS files in `dir` by their absolute paths, so the file works from any directory.
/// `PGSSLMODE` is the answer's `connection.sslmode`, or, when it names none, `verify-ca`, the
/// mode a database's `connection` names.
///
/// Each value is single-quoted, which a POSIX shell (`set -a; . .env`), Node's dotenv, and
/// docker compose take literally. A value that holds a single quote, a backslash (which
/// python-dotenv decodes inside single quotes), or a control character, such as a line break,
/// cannot be written that way, so the file is refused, with the reason. The directory's own
/// path was held to that rule at parse time.
fn env_file(dir: &Path, created: &Value) -> std::result::Result<String, String> {
    // `absolute` fails only when the current directory cannot be read, which the parse-time
    // check of a relative `dir` already needed: the path as given is then the best there is.
    let dir = std::path::absolute(dir).unwrap_or(dir.to_path_buf());
    let text = |pointer: &str| created.pointer(pointer).and_then(Value::as_str);
    let port = created
        .pointer("/connection/port")
        .and_then(Value::as_u64)
        .map(|port| port.to_string());
    let path = |file: &str| dir.join(file).to_string_lossy().into_owned();
    let settings = [
        ("PGHOST", text("/connection/host").map(str::to_owned)),
        ("PGPORT", port),
        ("PGUSER", text("/credentials/username").map(str::to_owned)),
        (
            "PGPASSWORD",
            text("/credentials/password").map(str::to_owned),
        ),
        (
            "PGDATABASE",
            text("/credentials/database").map(str::to_owned),
        ),
        (
            "PGSSLMODE",
            Some(
                text("/connection/sslmode")
                    .unwrap_or("verify-ca")
                    .to_owned(),
            ),
        ),
        ("PGSSLROOTCERT", Some(path(TLS_FILES[2].0))),
        ("PGSSLCERT", Some(path(TLS_FILES[0].0))),
        ("PGSSLKEY", Some(path(TLS_FILES[1].0))),
    ];
    let mut env = String::from(
        "# libpq settings for this database, written once by hipc; add PGHOST and PGPORT from\n\
         # `hipc compute databases get` when they are missing. Load with: set -a; . ./.env; set +a\n",
    );
    for (name, value) in settings {
        let Some(value) = value else { continue };
        if value.contains(['\'', '\\']) || value.chars().any(char::is_control) {
            return Err(format!(
                "the `{name}` value holds a single quote, a backslash, or a control character, \
                 which `.env` cannot hold quoted"
            ));
        }
        env.push_str(&format!("{name}='{value}'\n"));
    }
    Ok(env)
}

/// `POST /api/databases/<id>/restore/`, with `target_time` only when given. The answer is a
/// new database, whose credentials it may hold once, printed as a create's is
/// ([`credentials`]), `--credentials-dir` included. It does not
/// wait: operations that finish later get a status read, here `get` of the new id.
fn restore(ctx: &Context<'_>, args: &DatabaseRestore) -> Result<Printed> {
    require_reveal_or_file(ctx, args.reveal, args.credentials_dir.is_some())?;
    let id = args.database_id;
    let request = Request::new(Method::Post, format!("{}restore/", path(id))).listed_by(DATABASES);
    let (request, point) = match &args.target_time {
        Some(time) => (
            request.json(&json!({ "target_time": time })),
            format!("as it was at `{time}`"),
        ),
        None => (request, "up to the end of its backups".to_owned()),
    };
    let request = request.about(format!(
        "create a new database from the backups of the database `{id}`, {point}"
    ));
    let gate = Gate::Confirm {
        consequence: Some("it is billed until it is deleted"),
    };
    let client = ctx.load_authed_client()?;
    print_with(ctx, &client, &request, gate, |response| {
        credentials(ctx, args.credentials_dir.as_deref(), &response)
    })
    .map_err(balance_hint)
}

/// The most of an unreadable answer printed as `unparsed_answer`: far above any database
/// answer, and far below the 32 MiB a body may be.
const UNPARSED_CAP: usize = 1 << 20;

/// The answer of a create or a restore, which may hold the only copy of the new database's
/// credentials. When a 2xx body is not a JSON document, the failure stays `internal`, and the
/// body, read as UTF-8 with any invalid byte replaced and cut at [`UNPARSED_CAP`] bytes, goes
/// on stdout as `{"unparsed_answer": ...}`, so the credentials it may hold are not lost.
fn document_or_raw(response: &Response) -> Result<Value> {
    response.document().map_err(|error| {
        let body = response.bytes();
        if body.iter().all(u8::is_ascii_whitespace) {
            return error;
        }
        let cut = &body[..body.len().min(UNPARSED_CAP)];
        error
            .with_hint(
                "save `unparsed_answer` from stdout now: it may hold the new database's \
                 `credentials`, which no command shows again",
            )
            .after_write(json!({ "unparsed_answer": String::from_utf8_lossy(cut) }))
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    /// The secrets moved to files are redacted at once: a short one cuts neither a
    /// longer one that holds it nor the marker a longer one wrote.
    #[test]
    fn moved_secrets_are_redacted_in_one_pass() {
        let secrets = ["acted".to_owned(), "d".to_owned()];
        assert_eq!(
            super::redacted(json!({"a": ["acted d", 7], "acted": "x"}), &secrets),
            json!({"a": ["<redacted> <redacted>", 7], "acted": "x"})
        );
    }
}
