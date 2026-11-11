//! `hipc hub keys`: the namespace's scoped CI keys.
//!
//! A key is a registry robot with one role. A create and a rotate answer its secret once,
//! so both need `--reveal` before anything is sent (tenet 9), and the list never prints a
//! secret. Each write is confirmed: a create issues a credential, and a rotate or a delete
//! stops every CI job that uses the old secret (tenet 5).

use std::path::Path;

use hippius::routes::hub;
use hippius::{Error, Request, Result, phrase};
use serde_json::{Map, Value, json};

use crate::cli::{HubKeys, KeyCreate};
use crate::commands::files::shown_once;
use crate::commands::hub::{
    Issued, docker_login_command, issued_robot, with_provision_hint, without_robot_secrets,
};
use crate::commands::{Printed, print_list_with, print_with, require_reveal_or_file};
use crate::context::{Context, Gate};

const LIST: &str = "hipc hub keys list";

pub(crate) fn run(ctx: &Context<'_>, command: &HubKeys) -> Result<Printed> {
    let printed = match command {
        HubKeys::List => list(ctx),
        HubKeys::Get { key_id } => get(ctx, *key_id),
        HubKeys::Create(args) => create(ctx, args),
        HubKeys::Rotate {
            key_id,
            reveal,
            secret_file,
        } => rotate(ctx, *key_id, *reveal, secret_file.path.as_deref()),
        HubKeys::Delete { key_id } => delete(ctx, *key_id),
    };
    printed.map_err(|error| with_provision_hint(ctx, error))
}

/// `GET /api/registry/keys/` (swagger `registry_keys_list`), read whole: swagger lists no
/// page parameter, so a `next` link is `internal` rather than a list cut short.
fn list(ctx: &Context<'_>) -> Result<Printed> {
    let request = Request::to(&hub::KEYS, []).about("list the Hub keys");
    print_list_with(ctx, &ctx.load_authed_client()?, &request, |response| {
        let mut page = hippius::page(response.json()?)?;
        page.items = page
            .items
            .into_iter()
            .map(|row| listed_key(row, "the Hub key list"))
            .collect::<Result<_>>()?;
        page.complete_envelope(request.target())
    })
}

/// `GET /api/registry/keys/<id>/` (swagger `registry_keys_read`, "show metadata"), printed
/// as the list prints a row: a preview with `secret: null`.
fn get(ctx: &Context<'_>, id: u64) -> Result<Printed> {
    let request = Request::to(&hub::KEY, [&id])
        .listed_by(LIST)
        .about(format!("get the Hub key `{id}`"));
    print_with(
        ctx,
        &ctx.load_authed_client()?,
        &request,
        Gate::Open,
        |response| listed_key(response.document()?, &format!("the Hub key `{id}`")),
    )
}

/// A key as the list and `get` print it ([`without_robot_secrets`]): `secret: null` (tenet
/// 9), and the `docker_login_command` `hipc` builds for the key's `login` (null without one).
/// Swagger says only the create and the rotate answer the secret. `what` names the answer in
/// the refusal.
///
/// # Errors
///
/// `internal`, with nothing printed, when the row holds a `docker login` command outside
/// the keys that are dropped.
fn listed_key(row: Value, what: &str) -> Result<Value> {
    let Some(mut row) = without_robot_secrets(row) else {
        return Err(Error::internal(format!(
            "{what} holds a `docker login` command, which may pass the secret, and `hipc` does \
             not print a key secret"
        ))
        .with_hint(
            "log in with `docker login --password-stdin`; `hipc hub keys rotate <key_id> --reveal \
             --yes` issues a new secret",
        ));
    };
    let login = row
        .get("login")
        .and_then(Value::as_str)
        .map(docker_login_command)
        .transpose()?;
    if let Value::Object(fields) = &mut row {
        fields.insert("secret".to_owned(), Value::Null);
        fields.insert("docker_login_command".to_owned(), login.into());
    }
    Ok(row)
}

/// `POST /api/registry/keys/` (swagger `registry_keys_create`), body `{name, role,
/// expires_days?}` as swagger's description gives it, `expires_days` only when the flag is.
fn create(ctx: &Context<'_>, args: &KeyCreate) -> Result<Printed> {
    require_reveal_or_file(ctx, args.reveal, args.secret_file.path.is_some())?;
    let mut body = Map::new();
    body.insert("name".to_owned(), args.name.as_str().into());
    body.insert("role".to_owned(), args.role.code().into());
    if let Some(days) = args.expires_in {
        body.insert("expires_days".to_owned(), days.into());
    }
    let request = Request::to(&hub::KEY_CREATE, [])
        .json(&Value::Object(body))
        .about(format!(
            "create the Hub key {} with the role `{}`",
            phrase::quote(&args.name),
            args.role.code()
        ));
    let secret_file = args.secret_file.path.as_deref();
    let consequence = shown_once("its secret", secret_file);
    let gate = Gate::Confirm {
        consequence: Some(&consequence),
    };
    print_with(
        ctx,
        &ctx.load_authed_client()?,
        &request,
        gate,
        |response| {
            let body = response.document()?;
            // The key's id, for the rotate that issues it a new secret.
            let rotate = body
                .get("id")
                .and_then(Value::as_u64)
                .map(|id| format!("hub keys rotate {id} --reveal --yes"));
            issued_robot(
                ctx,
                body,
                Issued {
                    what: "the Hub key create answer",
                    done: "the key was created",
                    again: rotate.as_deref(),
                    secret_file,
                },
            )
        },
    )
}

/// `POST /api/registry/keys/<id>/rotate/` (swagger `registry_keys_rotate_create`), no body.
fn rotate(ctx: &Context<'_>, id: u64, reveal: bool, secret_file: Option<&Path>) -> Result<Printed> {
    require_reveal_or_file(ctx, reveal, secret_file.is_some())?;
    let request = Request::to(&hub::KEY_ROTATE, [&id])
        .listed_by(LIST)
        .about(format!("rotate the secret of the Hub key `{id}`"));
    let consequence = format!(
        "every CI job that uses its old secret stops working, and {}",
        shown_once("the new secret", secret_file)
    );
    let gate = Gate::Confirm {
        consequence: Some(&consequence),
    };
    let again = format!("hub keys rotate {id} --reveal --yes");
    print_with(
        ctx,
        &ctx.load_authed_client()?,
        &request,
        gate,
        |response| {
            issued_robot(
                ctx,
                response.document()?,
                Issued {
                    what: "the Hub key rotate answer",
                    done: "the old secret no longer works",
                    again: Some(&again),
                    secret_file,
                },
            )
        },
    )
}

/// `DELETE /api/registry/keys/<id>/` (swagger `registry_keys_delete`), which documents a
/// 204. A body the server sends anyway is printed as the key list prints a row, without any
/// secret field or `docker login` command (tenet 9).
fn delete(ctx: &Context<'_>, id: u64) -> Result<Printed> {
    let request = Request::to(&hub::KEY_DELETE, [&id])
        .listed_by(LIST)
        .about(format!("delete the Hub key `{id}`"));
    let gate = Gate::Confirm {
        consequence: Some(DELETED),
    };
    print_with(
        ctx,
        &ctx.load_authed_client()?,
        &request,
        gate,
        |response| {
            let body = response.json()?;
            // The key is deleted either way, so a body that cannot be printed whole gives way
            // to the document `hipc` writes for no body, rather than an error for a done write.
            let done = json!({ "id": id, "deleted": true });
            Ok(if body.is_null() {
                done
            } else {
                without_robot_secrets(body).unwrap_or(done)
            })
        },
    )
}

/// What a Hub key delete ends, for its question and `hipc api`'s.
pub(crate) const DELETED: &str = "every CI job that uses its secret stops working";
