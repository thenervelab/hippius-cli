//! `hipc s3`: S3 plans, buckets, tokens, and an AWS CLI profile.
//!
//! Object bytes never pass through `hipc`; they go to `https://s3.hippius.com` with the AWS
//! CLI, boto3, or rclone. S3 is part of a shared account, so a membership sends the account
//! header on every S3 route, and the server answers `owner_only` for a plan purchase.

use std::path::{Path, PathBuf};

use hippius::routes::{Paging, Route, s3};
use hippius::{
    AccessKeyId, AwsFiles, AwsProfile, Client, Error, Request, Response, Result, SecretKey,
    SecretValue, phrase, without_fields,
};
use serde_json::{Map, Value, json};

use crate::cli::{
    AclUpdate, BucketAcl, BucketObjects, Buckets, Configure, ObjectsList, Presign, S3,
    S3Subscription, TokenCreate, TokenKind, TokenRotate, TokenTarget, Tokens,
};
use crate::commands::files::{save_secret, shown_once};
use crate::commands::{
    Input, Printed, list, local_dry_run, object, plan_setting, print_body, print_body_or,
    print_envelope, print_keyed_list, print_with, require_public, require_reveal,
    require_reveal_or_file, subscribed,
};
use crate::context::{self, Context, Gate, Question};

pub(crate) fn run(ctx: &Context<'_>, command: &S3) -> Result<Printed> {
    match command {
        S3::Configure(args) => configure(ctx, args),
        // The catalog needs no token, and sends one only when it is configured.
        S3::Plans => print_body(
            ctx,
            &ctx.client(&context::config()?)?,
            &Request::to(&s3::PLANS, []).about("list the S3 plans"),
            Gate::Open,
        ),
        S3::Subscription(command) => subscription(ctx, &ctx.load_authed_client()?, command),
        S3::Buckets(command) => buckets(ctx, command),
        S3::Tokens(command) => tokens(ctx, command),
    }
}

fn subscription(ctx: &Context<'_>, client: &Client, command: &S3Subscription) -> Result<Printed> {
    match command {
        S3Subscription::Get => {
            let request = Request::to(&s3::SUBSCRIPTION, []).about("get the S3 plan");
            print_body(ctx, client, &request, Gate::Open)
        }
        S3Subscription::Checkout { intent_id } => {
            let request = Request::to(&s3::CHECKOUT, [intent_id])
                .about(format!("get the card checkout `{intent_id}`"));
            print_body(ctx, client, &request, Gate::Open)
        }
        S3Subscription::History(args) => {
            let request = Request::to(&s3::HISTORY, []).about("list the S3 plan history");
            list::run(ctx, client, &request, args)
        }
        S3Subscription::Subscribe(args) => {
            let body = s3::SubscribeBody {
                plan: &args.plan,
                rail: args.pay_from.code(),
            };
            // The server judges the plan code (owner decision D22): the help lists today's.
            let request = Request::to(&s3::SUBSCRIBE, [])
                .json(&body)
                .about(format!("buy the S3 plan `{}`", args.plan));
            let gate = Gate::Confirm {
                consequence: Some(args.pay_from.how()),
            };
            print_with(ctx, client, &request, gate, |response| {
                subscribed(ctx, &response, "s3")
            })
        }
        S3Subscription::Change(args) => {
            let (request, gate) = match &args.plan {
                Some(plan) => (
                    Request::to(&s3::SUBSCRIPTION_CHANGE, [])
                        .json(&s3::PlanChangeBody { plan })
                        .about(format!("switch the S3 plan to `{plan}`")),
                    Gate::Confirm {
                        consequence: Some("a plan below the current usage pauses uploads"),
                    },
                ),
                None => plan_setting(&s3::SUBSCRIPTION_CHANGE, "S3", &args.setting)?,
            };
            print_body(ctx, client, &request, gate)
        }
        S3Subscription::Cancel => {
            let request = Request::to(&s3::SUBSCRIPTION_CANCEL, []).about("cancel the S3 plan");
            let gate = Gate::Confirm {
                consequence: Some(CANCELLED),
            };
            print_with(ctx, client, &request, gate, |response| {
                let cancelled = response.document()?;
                ctx.note(
                    "once the cancel is confirmed, the plan stays until the end of the month paid \
                     for, with no refund, and then the account is on pay as you go",
                );
                Ok(cancelled)
            })
        }
    }
}

fn buckets(ctx: &Context<'_>, command: &Buckets) -> Result<Printed> {
    match command {
        // The API answers `{buckets, count, total_objects, total_size_bytes,
        // main_account_id}`, not the bare array its schema documents; a bare array is still
        // read.
        Buckets::List => print_keyed_list(
            ctx,
            &ctx.load_authed_client()?,
            &Request::to(&s3::BUCKETS, []).about("list the buckets"),
            "buckets",
        ),
        Buckets::Acl(command) => acl(ctx, command),
        Buckets::Objects(BucketObjects::List(args)) => list_objects(ctx, args),
        Buckets::Objects(BucketObjects::Presign(args)) => presign(ctx, args),
        Buckets::Create {
            bucket_name,
            region,
        } => {
            let body = s3::BucketCreateBody {
                name: bucket_name.as_str(),
                region: region.as_deref(),
            };
            let request = Request::to(&s3::BUCKET_CREATE, [])
                .json(&body)
                .about(format!("create the bucket `{bucket_name}`"));
            let client = ctx.load_authed_client()?;
            print_body(ctx, &client, &request, Gate::Confirm { consequence: None })
        }
        Buckets::Delete { bucket_name } => {
            let request = Request::to(&s3::BUCKET_DELETE, [bucket_name])
                .listed_by("hipc s3 buckets list")
                .about(format!("delete the bucket `{bucket_name}`"));
            let done = json!({ "name": bucket_name.as_str(), "deleted": true });
            let client = ctx.load_authed_client()?;
            print_body_or(
                ctx,
                &client,
                &request,
                Gate::Confirm { consequence: None },
                done,
            )
        }
    }
}

/// `GET /api/objectstore/buckets/<name>/objects/` (swagger `objectstore_buckets_objects_list`):
/// an S3 `ListObjectsV2` answer, paged by `max_keys` and `continuation_token`. One page prints
/// the envelope of its `Contents`, with `NextContinuationToken` as `next_page_token` and the
/// answer's other fields beside; `--all` sends each token back until none is left, and prints
/// one JSON array. Swagger's `delimiter` is left out: its `CommonPrefixes` are not objects,
/// and `--all` would drop them.
fn list_objects(ctx: &Context<'_>, args: &ObjectsList) -> Result<Printed> {
    let name = &args.bucket_name;
    let limit = args.limit.to_string();
    let paging = s3::OBJECTS
        .endpoint()
        .paging()
        .expect("the row of a bucket's objects says how they are paged");
    let page_request = |token: Option<&str>| {
        let request = Request::to(&s3::OBJECTS, [name])
            .listed_by("hipc s3 buckets list")
            .about(format!("list the objects of the bucket `{name}`"));
        let mut request = list::sized(request, paging, Some(&limit));
        if let Some(prefix) = &args.prefix {
            request = request.query("prefix", prefix);
        }
        list::at_page(request, paging, token)
    };
    let client = ctx.load_authed_client()?;
    list::run_with(ctx, &client, paging, args.into(), page_request)
}

/// `GET /api/objectstore/buckets/<name>/presigned-url/?key=<key>[&expires_in=<seconds>]`
/// (swagger `objectstore_buckets_presigned-url_list`), answered with `{url}`. The URL reads
/// the object with no other credential, so it needs `--reveal` and goes on stdout alone: no
/// note repeats it, and the `--debug` line of the request shows the query names only
/// (tenet 9). Swagger's `action=put`, an upload URL, is left to `hipc api`: `get`, its
/// default, is not sent.
fn presign(ctx: &Context<'_>, args: &Presign) -> Result<Printed> {
    let Presign {
        bucket_name,
        key,
        expires_in,
        reveal,
    } = args;
    if key.is_empty() {
        return Err(Error::usage(
            "the object key is empty",
            "pass the object's key, such as `2026/cat.jpg`",
        ));
    }
    require_reveal(ctx, *reveal)?;
    let request = Request::to(&s3::PRESIGNED_URL, [bucket_name])
        .query("key", key)
        .listed_by("hipc s3 buckets list")
        .about(format!(
            "presign a download from the bucket `{bucket_name}`"
        ));
    let request = match expires_in {
        Some(seconds) => request.query("expires_in", &seconds.to_string()),
        None => request,
    };
    print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
}

/// The largest `--policy-file` read, in bytes. Arbitrary: an S3 bucket policy is at most
/// 20 KB on AWS, and a wrong path is refused rather than sent.
const MAX_POLICY_BYTES: u64 = 1024 * 1024;

fn acl(ctx: &Context<'_>, command: &BucketAcl) -> Result<Printed> {
    match command {
        // Swagger `objectstore_acl_list`: a bare array of `{bucket, grants}`.
        BucketAcl::List => print_envelope(
            ctx,
            &ctx.load_authed_client()?,
            &Request::to(&s3::ACLS, []).about("list the sub-token grants of the buckets"),
        ),
        // Swagger `objectstore_buckets_acl_list`: `{bucket, policy}`.
        BucketAcl::Get { bucket_name } => {
            let request = Request::to(&s3::BUCKET_ACL, [bucket_name])
                .listed_by("hipc s3 buckets list")
                .about(format!("get the policy of the bucket `{bucket_name}`"));
            print_body(ctx, &ctx.load_authed_client()?, &request, Gate::Open)
        }
        BucketAcl::Update(args) => update_acl(ctx, args),
    }
}

/// `PUT /api/objectstore/buckets/<name>/acl/set/` with `{"policy": <the file>}` (swagger
/// `objectstore_buckets_acl_set_update`). Swagger's other body, `grants`, maps actions and
/// prefixes to a policy whose principal it does not name, so it stays on `hipc api`.
///
/// A policy can name any principal, `*` included, and it replaces the whole policy. Telling
/// whether a policy publishes the bucket, or which statements it drops, would mean judging
/// S3 policies, and a misjudged one would publish the bucket. So `--public` and
/// `--remove-others` are both required whatever the file holds, checked in that order, as
/// `hipc api` checks them on this route, and the current policy is not read first.
fn update_acl(ctx: &Context<'_>, args: &AclUpdate) -> Result<Printed> {
    let name = &args.bucket_name;
    let input = Input {
        path: &args.policy_file,
        flag: "--policy-file",
        what: "bucket policy",
    };
    let policy = input.read_value(MAX_POLICY_BYTES)?;
    if !policy.is_object() {
        return Err(Error::usage(
            format!(
                "the bucket policy from {} is not a JSON object",
                input.name()
            ),
            "write the policy alone, as the `policy` field of `hipc s3 buckets acl get` prints \
             it, in the file `--policy-file` names",
        ));
    }
    require_public(
        ctx,
        args.public,
        &format!("the policy may make the bucket `{name}` readable by anyone"),
    )?;
    // A dry run sends nothing, so it removes nothing (tenet 5).
    if !args.remove_others && !ctx.dry_run() {
        return Err(Error::remove_others_unread(
            format!(
                "the policy replaces the whole policy of the bucket `{name}`, removing every \
                 statement it leaves out, and `--remove-others` was not passed"
            ),
            ctx.args(),
        ));
    }
    let request = Request::to(&s3::BUCKET_ACL_SET, [name])
        .listed_by("hipc s3 buckets list")
        .json(&s3::BucketAclSetBody { policy: &policy })
        .about(format!(
            "replace the whole policy of the bucket `{name}` with the one in {}, which can make \
             it readable by anyone",
            input.name()
        ));
    // Swagger documents the 200 as "Updated" with no body.
    let done = json!({ "bucket": name.as_str(), "updated": true });
    let gate = Gate::Confirm {
        consequence: Some("every statement the file leaves out is removed"),
    };
    print_body_or(ctx, &ctx.load_authed_client()?, &request, gate, done)
}

fn tokens(ctx: &Context<'_>, command: &Tokens) -> Result<Printed> {
    match command {
        Tokens::List => list_tokens(ctx, &ctx.load_authed_client()?),
        Tokens::Create(args) => create_token(ctx, args),
        Tokens::Revoke(target) => revoke_token(ctx, target),
        Tokens::Rotate(args) => rotate_token(ctx, args),
    }
}

/// The routes of one kind of token.
#[derive(Debug)]
struct TokenRoutes {
    list: Route<0>,
    create: Route<0>,
    revoke: Route<1>,
    rotate: Route<1>,
}

impl TokenRoutes {
    const fn of(kind: TokenKind) -> &'static Self {
        match kind {
            TokenKind::Master => &Self {
                list: s3::MASTER_TOKENS,
                create: s3::MASTER_TOKEN_CREATE,
                revoke: s3::MASTER_TOKEN_REVOKE,
                rotate: s3::MASTER_TOKEN_ROTATE,
            },
            TokenKind::Sub => &Self {
                list: s3::SUB_TOKENS,
                create: s3::SUB_TOKEN_CREATE,
                revoke: s3::SUB_TOKEN_REVOKE,
                rotate: s3::SUB_TOKEN_ROTATE,
            },
        }
    }
}

/// Both collections, every page of each, as one list. Swagger gives the sub-token list
/// `page` and `page_size`, so its `next` links are followed; the first request carries no
/// parameters, and the server picks the page size.
fn list_tokens(ctx: &Context<'_>, client: &Client) -> Result<Printed> {
    let first_page = |kind: TokenKind| {
        Request::to(&TokenRoutes::of(kind).list, [])
            .about(format!("list the S3 `{}` tokens", kind.code()))
    };
    if ctx.dry_run() {
        let [master, sub] = [TokenKind::Master, TokenKind::Sub].map(first_page);
        return ctx.dry_run_of(client, &[&master, &sub]).map(Printed::from);
    }
    // Swagger gives the sub-token list DRF's `page`, but no command takes page flags on it,
    // so its route is not marked paged; it is read by number all the same.
    let paging = Paging::Numbered;
    let mut items = Vec::new();
    for kind in [TokenKind::Master, TokenKind::Sub] {
        let mut asked = None;
        list::walk(ctx, client, first_page(kind), "1", |answer| {
            let page = hippius::page(answer)?;
            // A server that ignores `page` and answers page 1 again, with a `next` that
            // keeps rising, would repeat the list on every page; a later page that names
            // another `page` than the one asked is refused.
            let start_over = asked.as_ref().map(|_| list::CHANGED_WHILE_READ);
            list::answered_as_asked(&page, asked.as_deref().unwrap_or("1"), start_over)?;
            let next = page.next_page_token()?;
            for token in page.items {
                let mut token = token_document(without_fields(token, &is_s3_secret), kind);
                // Tenet 9: a listing shows `secret: null`, whatever the server sent.
                if let Value::Object(fields) = &mut token {
                    fields.insert("secret".to_owned(), Value::Null);
                }
                items.push(token);
            }
            let Some(number) = next else {
                return Ok(list::Turn::Done(()));
            };
            let request = list::at_page(first_page(kind), paging, Some(&number));
            asked = Some(number.clone());
            Ok(list::Turn::Next {
                token: number,
                request,
            })
        })?;
    }
    Ok(Printed::List(hippius::envelope(items)))
}

fn create_token(ctx: &Context<'_>, args: &TokenCreate) -> Result<Printed> {
    let body = token_body(args)?;
    require_reveal_or_file(ctx, args.secret.reveal, args.secret.path.is_some())?;
    let request = Request::to(&TokenRoutes::of(args.kind).create, [])
        .json(&body)
        .about(format!("create an S3 `{}` token", args.kind.code()));
    let secret_file = args.secret.path.as_deref();
    let consequence = shown_once("its secret", secret_file);
    let gate = Gate::Confirm {
        consequence: Some(&consequence),
    };
    let client = ctx.load_authed_client()?;
    print_with(ctx, &client, &request, gate, |response| {
        issued_token(ctx, &response, args.kind, secret_file)
    })
}

/// The answer of a token create or rotate as `hipc` prints it ([`token_document`]), its
/// secret moved to `secret_file` when one is named ([`save_secret`]), and the text-mode note
/// that names the `hipc s3 configure` for it.
fn issued_token(
    ctx: &Context<'_>,
    response: &Response,
    kind: TokenKind,
    secret_file: Option<&Path>,
) -> Result<Value> {
    let token = token_document(response.document()?, kind);
    let Some(path) = secret_file else {
        configure_note(ctx, &token, None);
        return Ok(token);
    };
    let (token, written) = save_secret(path, token, &is_s3_secret)?;
    configure_note(ctx, &token, written.then_some(path));
    Ok(token)
}

/// The create body: only the flags the caller set. A sub-token flag on a master token is a
/// usage error, not a field the server would ignore.
fn token_body(args: &TokenCreate) -> Result<s3::TokenCreateBody<'_>> {
    let body = s3::TokenCreateBody {
        name: args.name.as_deref(),
        expires_at: args.expires_at.as_deref(),
        ..s3::TokenCreateBody::default()
    };
    let sub_flags = args.scope_type.is_some()
        || !args.buckets.is_empty()
        || !args.actions.is_empty()
        || !args.prefixes.is_empty()
        || !args.ips.is_empty()
        || args.not_before.is_some();
    match args.kind {
        TokenKind::Master if sub_flags => Err(Error::usage(
            "`--scope-type`, `--bucket`, `--action`, `--prefix`, `--ip`, and `--not-before` are \
             for sub tokens, and `--kind` is `master`",
            "pass `--kind sub` to create a token with those limits",
        )),
        TokenKind::Master => Ok(body),
        TokenKind::Sub => Ok(s3::TokenCreateBody {
            scope_type: args.scope_type.map(crate::cli::ScopeType::code),
            bucket_names: &args.buckets,
            actions: &args.actions,
            allowed_prefixes: &args.prefixes,
            ip_allowlist: &args.ips,
            not_before: args.not_before.as_deref(),
            ..body
        }),
    }
}

fn revoke_token(ctx: &Context<'_>, target: &TokenTarget) -> Result<Printed> {
    let request = Request::to(&TokenRoutes::of(target.kind).revoke, [&target.token_id])
        .listed_by("hipc s3 tokens list")
        .about(format!(
            "revoke the S3 `{}` token `{}`",
            target.kind.code(),
            target.token_id
        ));
    let client = ctx.load_authed_client()?;
    print_with(
        ctx,
        &client,
        &request,
        Gate::Confirm { consequence: None },
        |response| {
            let mut doc = Map::new();
            doc.insert("id".to_owned(), target.token_id.to_string().into());
            // A revoke issues no secret; one the server sent anyway is not printed (tenet 9).
            if let Value::Object(body) = without_fields(response.json()?, &is_s3_secret) {
                doc.extend(body);
            }
            Ok(Value::Object(doc))
        },
    )
}

fn rotate_token(ctx: &Context<'_>, args: &TokenRotate) -> Result<Printed> {
    require_reveal_or_file(ctx, args.secret.reveal, args.secret.path.is_some())?;
    let request = Request::to(
        &TokenRoutes::of(args.target.kind).rotate,
        [&args.target.token_id],
    )
    .listed_by("hipc s3 tokens list")
    .about(format!(
        "rotate the S3 `{}` token `{}`",
        args.target.kind.code(),
        args.target.token_id
    ));
    let secret_file = args.secret.path.as_deref();
    let consequence = format!(
        "the old token stops working, and {}",
        shown_once("the new secret", secret_file)
    );
    let gate = Gate::Confirm {
        consequence: Some(&consequence),
    };
    let client = ctx.load_authed_client()?;
    print_with(ctx, &client, &request, gate, |response| {
        issued_token(ctx, &response, args.target.kind, secret_file)
    })
}

/// The note after a token is created or rotated: the `hipc s3 configure` that writes an AWS
/// CLI profile for its key pair. It names the answer's `access_key_id` only when
/// `hipc s3 configure` accepts it, so the command runs as printed; without one there is no
/// note, nor without a `secret` to save or a `secret_file` it was written to. A secret on
/// stdout stays there: the command reads it from a file (tenet 9). A note is printed only in
/// text mode, whose output is not the JSON `--from-token-file` reads, so the note names the
/// flags that take the shown values, and the file `--secret-file` wrote.
/// It cannot fail: it runs after the write that issued the secret, which an error would
/// keep off stdout.
fn configure_note(ctx: &Context<'_>, token: &Value, secret_file: Option<&Path>) {
    let id = token
        .get("access_key_id")
        .and_then(Value::as_str)
        .filter(|id| AccessKeyId::new(id).is_ok());
    let has_secret = secret_file.is_some() || token.get("secret").and_then(Value::as_str).is_some();
    let Some(id) = id.filter(|_| has_secret) else {
        return;
    };
    let file = secret_file.map_or_else(
        || "<path>".to_owned(),
        |path| phrase::shell_word(&path.to_string_lossy()),
    );
    let words = format!("s3 configure --access-key-id {id} --secret-key-file {file}");
    if let Some(command) = ctx.next_command(&words) {
        ctx.note(&match secret_file {
            Some(path) => format!(
                "the secret is in {}; run `{command}` to write an AWS CLI profile for it, then \
                 delete the file",
                phrase::quote(path.display())
            ),
            None => format!(
                "save `secret` to a file, then run `{command}` to write an AWS CLI profile for it"
            ),
        });
    }
}

/// A token as `hipc` prints it: `snake_case` names, `kind` first, and `last4` as
/// `secret_preview`. The create and rotate answers use camelCase (`accessKeyId`,
/// `createdAt`, `expiresAt`); the list answers use `snake_case`. A `secret` field is kept
/// only where the server sent one, which is only on create and rotate.
fn token_document(token: Value, kind: TokenKind) -> Value {
    let Value::Object(fields) = token else {
        return token;
    };
    // Another spelling of the secret is printed as `secret`, which the list, the revoke,
    // `--secret-file`, and `--from-token-file` know, unless the answer has `secret` already,
    // or another spelling took the name: that one keeps its own, and `--secret-file` then
    // refuses an answer with two different secrets.
    let mut secret_named = fields.contains_key("secret");
    let mut out = Map::new();
    out.insert("kind".to_owned(), kind.code().into());
    for (name, value) in fields {
        let name = match name.as_str() {
            "accessKeyId" => "access_key_id".to_owned(),
            "createdAt" => "created_at".to_owned(),
            "expiresAt" => "expires_at".to_owned(),
            "last4" => "secret_preview".to_owned(),
            _ if !secret_named && is_s3_secret(&name) => {
                secret_named = true;
                "secret".to_owned()
            }
            _ => name,
        };
        out.insert(name, value);
    }
    Value::Object(out)
}

/// Whether `key` names an S3 token's secret key: `secret`, as the API answers it, or a
/// spelling of the secret access key another version of the answer could use
/// (`secret_key`, `secretKey`, `secret_access_key`, `SecretAccessKey`,
/// `aws_secret_access_key`), compared in lowercase
/// without `_` and `-`. The list and the revoke drop them all, and `--secret-file` moves
/// them all (tenet 9).
fn is_s3_secret(key: &str) -> bool {
    let key: String = key
        .chars()
        .filter(|c| !matches!(c, '_' | '-'))
        .map(|c| c.to_ascii_lowercase())
        .collect();
    matches!(
        key.as_str(),
        "secret" | "secretkey" | "secretaccesskey" | "awssecretaccesskey"
    )
}

/// The largest token document read, in bytes. Arbitrary: a create answer is a few hundred
/// bytes, and the cap stops a wrong path from being read whole.
const MAX_TOKEN_DOCUMENT_BYTES: u64 = 64 * 1024;

/// The key pair in the document `s3 tokens create --reveal` or `rotate --reveal` printed,
/// read from the file `path`: its `access_key_id` and `secret`, or, from a create or rotate
/// with `--secret-file`, the file its `secret_file` names ([`secret_from_file`]).
/// Messages name what is missing and never quote the document, which holds the secret.
fn key_pair_from_document(path: &Path) -> Result<(AccessKeyId, SecretKey)> {
    let input = Input {
        path,
        flag: "--from-token-file",
        what: "S3 token document",
    };
    // The document holds the secret: it is read, parsed, and dropped cleared.
    let bytes = input.read_cleared(MAX_TOKEN_DOCUMENT_BYTES)?;
    let name = input.name();
    let refused = |problem: &str, hint: &str| {
        Error::usage(format!("the S3 token document from {name} {problem}"), hint)
    };
    // Text mode prints a key-value block, not JSON, so the hint names `--json`.
    let print_one = "pass the JSON output of `hipc --json s3 tokens create --reveal` or \
                     `hipc --json s3 tokens rotate --reveal`";
    let parsed = serde_json::from_slice::<Value>(&bytes).map(SecretValue::new);
    let Some(document) = parsed
        .as_ref()
        .ok()
        .and_then(|parsed| parsed.as_value().as_object())
    else {
        return Err(refused("is not a JSON object", print_one));
    };
    let Some(id) = document.get("access_key_id").and_then(Value::as_str) else {
        return Err(refused("has no `access_key_id` text", print_one));
    };
    if let Some(secret) = document.get("secret").and_then(Value::as_str) {
        return Ok((
            AccessKeyId::new(id)?,
            SecretKey::new(secret, &input.source())?,
        ));
    }
    if let Some(file) = document.get("secret_file").and_then(Value::as_str) {
        return Ok((AccessKeyId::new(id)?, secret_from_file(&name, file)?));
    }
    // A list or get prints `secret: null`: the secret is shown once, at create or rotate.
    Err(refused(
        "has no `secret`: only a create or a rotate shows it",
        "run `hipc --json s3 tokens rotate --kind <kind> <token_id> --reveal --yes`, and \
         pass its output",
    ))
}

/// The secret in the file a token document's `secret_file` names, which `--secret-file` wrote.
/// The path is the one that create or rotate was given, so a relative one is read from the
/// current directory, as it was written, not from the document's directory. It is held to the
/// checks of `--secret-key-file` ([`Input::read`]): a regular file, never a symlink, a FIFO,
/// the hipc config file, or an AWS file. The path comes from the document, not from the
/// caller, so anything but a regular file is refused before it is opened, as a device's open
/// can act. `-` and an empty path name no file `--secret-file` writes, so they are refused
/// rather than read as stdin or the current directory. Every error names the document's
/// `secret_file`, and its hint the other way to pass the key pair.
fn secret_from_file(document: &str, file: &str) -> Result<SecretKey> {
    let hint = "run `hipc s3 configure` from the directory the token was created in, or pass \
                `--access-key-id` with `--secret-key-file` naming the secret file";
    let refused = |problem: String| {
        Error::usage(
            format!("the `secret_file` of the S3 token document from {document} {problem}"),
            hint,
        )
    };
    if file.is_empty() || file == "-" {
        return Err(refused("names no file".to_owned()));
    }
    let path = Path::new(file);
    let named = phrase::quote(path.display());
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(refused(format!(
                "is {named}, a symlink, and a symlink is never followed"
            )));
        }
        Ok(metadata) if !metadata.is_file() => {
            return Err(refused(format!("is {named}, which is not a regular file")));
        }
        Ok(_) => {}
        Err(e) => {
            return Err(refused(format!(
                "is {named}, which cannot be read: {}",
                phrase::io_reason(&e)
            )));
        }
    }
    let input = Input {
        path,
        flag: "--from-token-file",
        what: "secret key",
    };
    input
        .read_secret()
        .and_then(|secret| SecretKey::new(&secret, &input.source()))
        .map_err(|e| e.with_hint(hint))
}

/// The key pair from `--access-key-id` and the file `--secret-key-file` names.
fn key_pair_from_flags(args: &Configure) -> Result<(AccessKeyId, SecretKey)> {
    let id = args
        .access_key_id
        .as_deref()
        .expect("clap requires `--access-key-id` when `--from-token-file` is not given");
    let path = args
        .secret_key_file
        .as_deref()
        .expect("clap requires `--secret-key-file` with `--access-key-id`");
    let input = Input {
        path,
        flag: "--secret-key-file",
        what: "secret key",
    };
    let secret = SecretKey::new(&input.read_secret()?, &input.source())?;
    Ok((AccessKeyId::new(id)?, secret))
}

/// `hipc s3 configure`.
fn configure(ctx: &Context<'_>, args: &Configure) -> Result<Printed> {
    let profile = AwsProfile::new(&args.aws_profile)?;
    let (key, secret) = args
        .from_token_file
        .as_deref()
        .map_or_else(|| key_pair_from_flags(args), key_pair_from_document)?;
    // The two AWS variables are read without `env_os`'s empty filter: botocore reads a
    // variable set to `""` as that path, not as unset, so `locate` must see the empty value
    // to refuse it rather than write the default `~/.aws/` the AWS CLI then does not read.
    let files = AwsFiles::locate(
        std::env::var_os("AWS_CONFIG_FILE").map(PathBuf::from),
        std::env::var_os("AWS_SHARED_CREDENTIALS_FILE").map(PathBuf::from),
        context::env_os("HOME").map(PathBuf::from).as_deref(),
    )?;
    let doc = |written: bool| {
        object([
            ("aws_profile", profile.as_str().into()),
            ("config_path", files.config.display().to_string().into()),
            (
                "credentials_path",
                files.credentials.display().to_string().into(),
            ),
            ("secret_written", written.into()),
        ])
    };
    if ctx.dry_run() {
        files.check()?;
        return Ok(Printed::Document(local_dry_run(
            "write_aws_profile",
            doc(false),
        )));
    }
    hippius::configure(&files, &profile, &key, &secret, |action| {
        ctx.confirm(Question {
            action,
            consequence: Some("its endpoint, region, and key pair are replaced"),
        })
    })?;
    // `AwsProfile` allows a leading `-`, which the AWS CLI would read as a flag.
    if !profile.as_str().starts_with('-') {
        ctx.note(&format!(
            "run `aws --profile {} s3 ls` to list the buckets with it",
            profile.as_str()
        ));
    }
    Ok(Printed::Document(Value::Object(doc(true))))
}

/// What an S3 plan cancel leaves, for its question and `hipc api`'s.
pub(crate) const CANCELLED: &str = "the plan stays until the end of the month paid for, with no \
                                    refund, and then the account is on pay as you go";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_documents_are_snake_case_with_kind_first() {
        let created = json!({
            "accessKeyId": "hip_A",
            "secret": "s",
            "id": "i",
            "createdAt": "t",
            "expiresAt": null
        });
        assert_eq!(
            token_document(created, TokenKind::Master).to_string(),
            concat!(
                r#"{"kind":"master","access_key_id":"hip_A","secret":"s","id":"i","#,
                r#""created_at":"t","expires_at":null}"#
            )
        );
        let listed = json!({"id": "i", "access_key_id": "hip_A", "last4": "wxyz"});
        assert_eq!(
            token_document(listed, TokenKind::Sub),
            json!({"kind": "sub", "id": "i", "access_key_id": "hip_A", "secret_preview": "wxyz"})
        );
        assert_eq!(token_document(json!("x"), TokenKind::Sub), json!("x"));
    }
}
