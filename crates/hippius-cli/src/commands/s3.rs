//! `hipc s3`: S3 plans, buckets, tokens, and an AWS CLI profile.
//!
//! Object bytes never pass through `hipc`; they go to `https://s3.hippius.com` with the AWS
//! CLI, boto3, or rclone. S3 is part of a shared account, so a membership sends the account
//! header on every S3 route, and the server answers `owner_only` for a plan purchase.

use std::path::{Path, PathBuf};

use hippius::{
    AccessKeyId, AwsFiles, AwsProfile, Client, Error, Method, Request, Response, Result, SecretKey,
    phrase, without_fields,
};
use serde_json::{Map, Value, json};

use crate::cli::values::{Charset, Dash, is_word};
use crate::cli::{
    AclUpdate, BucketAcl, BucketObjects, Buckets, Configure, MAX_CONTINUATION_TOKEN_BYTES,
    ObjectsList, Presign, S3, S3Subscription, TokenCreate, TokenKind, TokenRotate, TokenTarget,
    Tokens, continuation_token,
};
use crate::commands::files::{save_secret, shown_once};
use crate::commands::{
    Cleared, Input, Pages, Printed, dry_run_requests, list, local_dry_run, plan_setting,
    print_body, print_body_or, print_envelope, print_keyed_list, print_with, require_public,
    require_reveal, require_reveal_or_file, subscribed,
};
use crate::context::{self, Context, Gate, Question, Step};

pub(crate) fn run(ctx: &Context<'_>, command: &S3) -> Result<Printed> {
    match command {
        S3::Configure(args) => configure(ctx, args),
        // The catalog needs no token, and sends one only when it is configured.
        S3::Plans => print_body(
            ctx,
            &ctx.client(&context::config()?)?,
            &Request::get("/s3/plans/").about("list the S3 plans"),
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
            let request = Request::get("/s3/subscription/").about("get the S3 plan");
            print_body(ctx, client, &request, Gate::Open)
        }
        S3Subscription::Checkout { intent_id } => {
            let request = Request::get(format!("/s3/subscription/checkout/{intent_id}/"))
                .about(format!("get the card checkout `{intent_id}`"));
            print_body(ctx, client, &request, Gate::Open)
        }
        S3Subscription::History(args) => {
            let request =
                Request::get("/s3/subscription/history/").about("list the S3 plan history");
            list::run(ctx, client, &request, args)
        }
        S3Subscription::Subscribe(args) => {
            let body = json!({ "plan": args.plan.code(), "rail": args.pay_from.code() });
            let request = Request::new(Method::Post, "/s3/subscription/")
                .json(&body)
                .about(format!("buy the S3 plan `{}`", args.plan.code()));
            let gate = Gate::Confirm {
                consequence: Some(args.pay_from.how()),
            };
            print_with(ctx, client, &request, gate, |r| subscribed(ctx, &r, "s3"))
        }
        S3Subscription::Change(args) => {
            let (request, gate) = match args.plan {
                Some(plan) => (
                    Request::new(Method::Patch, "/s3/subscription/")
                        .json(&json!({ "plan": plan.code() }))
                        .about(format!("switch the S3 plan to `{}`", plan.code())),
                    Gate::Confirm {
                        consequence: Some("a plan below the current usage pauses uploads"),
                    },
                ),
                None => plan_setting("/s3/subscription/", "S3", &args.setting)?,
            };
            print_body(ctx, client, &request, gate)
        }
        S3Subscription::Cancel => {
            let request =
                Request::new(Method::Delete, "/s3/subscription/").about("cancel the S3 plan");
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
            &Request::get("/objectstore/buckets/").about("list the buckets"),
            "buckets",
        ),
        Buckets::Acl(command) => acl(ctx, command),
        Buckets::Objects(BucketObjects::List(args)) => list_objects(ctx, args),
        Buckets::Objects(BucketObjects::Presign(args)) => presign(ctx, args),
        Buckets::Create {
            bucket_name,
            region,
        } => {
            check_bucket(bucket_name)?;
            let mut body = Map::new();
            body.insert("name".to_owned(), bucket_name.as_str().into());
            if let Some(region) = region {
                body.insert("region".to_owned(), region.as_str().into());
            }
            let request = Request::new(Method::Post, "/objectstore/buckets/")
                .json(&Value::Object(body))
                .about(format!("create the bucket `{bucket_name}`"));
            let client = ctx.load_authed_client()?;
            print_body(ctx, &client, &request, Gate::Confirm { consequence: None })
        }
        Buckets::Delete { bucket_name } => {
            check_bucket(bucket_name)?;
            let request = Request::new(
                Method::Delete,
                format!("/objectstore/buckets/{bucket_name}/"),
            )
            .listed_by("hipc s3 buckets list")
            .about(format!("delete the bucket `{bucket_name}`"));
            let done = json!({ "name": bucket_name, "deleted": true });
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
    check_bucket(name)?;
    let limit = args.limit.to_string();
    let page_request = |token: Option<&str>| {
        let mut request = Request::get(format!("/objectstore/buckets/{name}/objects/"))
            .listed_by("hipc s3 buckets list")
            .about(format!("list the objects of the bucket `{name}`"))
            .query("max_keys", &limit);
        if let Some(prefix) = &args.prefix {
            request = request.query("prefix", prefix);
        }
        match token {
            Some(token) => request.query("continuation_token", token),
            None => request,
        }
    };
    let client = ctx.load_authed_client()?;
    let mut items = Vec::new();
    // The first page has no token; `""` stands for it, as `--page-token` takes no empty one.
    let mut pages = Pages::starting_at(args.page_token.as_deref().unwrap_or(""));
    let mut token = args.page_token.clone();
    loop {
        let answer = match ctx.call(&client, &page_request(token.as_deref()), Gate::Open)? {
            Step::Sent(response) => {
                pages.read(response.bytes().len())?;
                response.json()?
            }
            Step::DryRun(doc) => return Ok(Printed::Document(doc)),
        };
        let page = objects_page(answer)?;
        if !args.all {
            let mut doc = hippius::envelope_beside(page.items, page.fields);
            doc["next_page_token"] = page.next.map_or(Value::Null, Value::String);
            return Ok(Printed::List(doc));
        }
        items.extend(page.items);
        let Some(next) = page.next else {
            return Ok(Printed::List(Value::Array(items)));
        };
        pages.admit(&next)?;
        token = Some(next);
    }
}

/// One page of a bucket's objects.
#[derive(Debug)]
struct ObjectsPage {
    /// `Contents`: the objects.
    items: Vec<Value>,
    /// `NextContinuationToken`, or `None` on the last page.
    next: Option<String>,
    /// The answer's other fields, such as `KeyCount` and `IsTruncated`.
    fields: Map<String, Value>,
}

/// Reads an S3 `ListObjectsV2` answer. `Contents` may be missing, as boto3 leaves it out of
/// an empty page. An empty `NextContinuationToken` is no token, as the AWS SDK paginators
/// read it, so a loop never sends it and fetches the first page again.
///
/// # Errors
///
/// `internal` for an answer that is not an object, a `Contents` that is not an array, a
/// token that is not text or that `--page-token` would refuse, and an `IsTruncated: true`
/// page with no token or an `IsTruncated` that is neither a boolean nor `null`: reading that
/// as the last page would print a short list as if it were complete. A page with
/// `IsTruncated: false` is the last, whatever token it carries.
fn objects_page(answer: Value) -> Result<ObjectsPage> {
    let not_a_list =
        || Error::internal("the API answered a list request with something other than a list");
    let Value::Object(mut fields) = answer else {
        return Err(not_a_list());
    };
    let items = match fields.shift_remove("Contents") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items,
        Some(_) => return Err(not_a_list()),
    };
    let next = match fields.shift_remove("NextContinuationToken") {
        None | Some(Value::Null) => None,
        Some(Value::String(token)) => Some(token).filter(|token| !token.is_empty()),
        Some(_) => {
            return Err(Error::internal(
                "the API's `NextContinuationToken` is not text, so the next page cannot be read",
            ));
        }
    };
    let next = match (fields.get("IsTruncated"), next) {
        // S3 sends a token only on a truncated page; one beside `IsTruncated: false` leads
        // nowhere, and following it would fetch a page the server says does not exist.
        (Some(Value::Bool(false)), _) => None,
        (Some(Value::Bool(true)), None) => {
            return Err(Error::internal(
                "the API says the list goes on (`IsTruncated`) but gives no \
                 `NextContinuationToken`, so the next page cannot be read",
            ));
        }
        // A string `"true"` or a number `1` is no boolean to trust either way: read as absent
        // it would make a page without a token the last, a short list printed as complete.
        (Some(truncated), _) if !truncated.is_null() && !truncated.is_boolean() => {
            return Err(Error::internal(
                "the API's `IsTruncated` is not `true` or `false`, so whether the list goes on \
                 cannot be read",
            ));
        }
        (_, next) => next,
    };
    // Held to the rule `--page-token` checks, so a printed `next_page_token` is always one
    // the next call accepts. The token is not quoted: it is the server's, and may be long.
    if let Some(token) = &next
        && continuation_token(token).is_err()
    {
        return Err(Error::internal(format!(
            "the API's `NextContinuationToken` is not one `--page-token` takes: it is empty, \
             holds a space or a control character, or is longer than \
             {MAX_CONTINUATION_TOKEN_BYTES} bytes"
        )));
    }
    Ok(ObjectsPage {
        items,
        next,
        fields,
    })
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
    check_bucket(bucket_name)?;
    if key.is_empty() {
        return Err(Error::usage(
            "the object key is empty",
            "pass the object's key, such as `2026/cat.jpg`",
        ));
    }
    require_reveal(ctx, *reveal)?;
    let request = Request::get(format!("/objectstore/buckets/{bucket_name}/presigned-url/"))
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
            &Request::get("/objectstore/acl/").about("list the sub-token grants of the buckets"),
        ),
        // Swagger `objectstore_buckets_acl_list`: `{bucket, policy}`.
        BucketAcl::Get { bucket_name } => {
            check_bucket(bucket_name)?;
            let request = Request::get(format!("/objectstore/buckets/{bucket_name}/acl/"))
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
    check_bucket(name)?;
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
    // A dry run sends nothing, so it removes nothing (TENETS 5).
    if !args.remove_others && !ctx.dry_run() {
        return Err(Error::remove_others_unread(
            format!(
                "the policy replaces the whole policy of the bucket `{name}`, removing every \
                 statement it leaves out, and `--remove-others` was not passed"
            ),
            ctx.args(),
        ));
    }
    let request = Request::new(Method::Put, format!("/objectstore/buckets/{name}/acl/set/"))
        .listed_by("hipc s3 buckets list")
        .json(&json!({ "policy": policy }))
        .about(format!(
            "replace the whole policy of the bucket `{name}` with the one in {}, which can make \
             it readable by anyone",
            input.name()
        ));
    // Swagger documents the 200 as "Updated" with no body.
    let done = json!({ "bucket": name, "updated": true });
    let gate = Gate::Confirm {
        consequence: Some("every statement the file leaves out is removed"),
    };
    print_body_or(ctx, &ctx.load_authed_client()?, &request, gate, done)
}

/// S3 bucket naming: 3 to 63 lowercase letters, digits, `.`, and `-`, starting and ending
/// with a letter or digit, with no `..`. Checked before sending because the name becomes a
/// path segment.
fn check_bucket(name: &str) -> Result<()> {
    let bytes = name.as_bytes();
    // A name that starts with a letter or digit starts with no `-`.
    let valid = is_word(name, 3..=63, Charset::LowerDotDash, Dash::Refused)
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && !name.contains("..");
    if valid {
        Ok(())
    } else {
        Err(Error::usage(
            format!(
                "`{name}` is not a bucket name: a name is 3 to 63 lowercase letters, digits, \
                 `.`, and `-`, starts and ends with a letter or a digit, and has no `..`"
            ),
            "pass a bucket name such as `my-bucket.2`",
        ))
    }
}

fn tokens(ctx: &Context<'_>, command: &Tokens) -> Result<Printed> {
    match command {
        Tokens::List => list_tokens(ctx, &ctx.load_authed_client()?),
        Tokens::Create(args) => create_token(ctx, args),
        Tokens::Revoke(target) => revoke_token(ctx, target),
        Tokens::Rotate(args) => rotate_token(ctx, args),
    }
}

/// The collection path of a token kind.
fn collection(kind: TokenKind) -> &'static str {
    match kind {
        TokenKind::Master => "/objectstore/master-tokens/",
        TokenKind::Sub => "/objectstore/sub-tokens/",
    }
}

/// Both collections, every page of each, as one list. Swagger gives the sub-token list
/// `page` and `page_size`, so its `next` links are followed; the first request carries no
/// parameters, and the server picks the page size.
fn list_tokens(ctx: &Context<'_>, client: &Client) -> Result<Printed> {
    let mut items = Vec::new();
    let mut dry_runs = Vec::new();
    for kind in [TokenKind::Master, TokenKind::Sub] {
        let mut pages = Pages::starting_at("1");
        let mut page_number: Option<String> = None;
        loop {
            let request = Request::get(collection(kind))
                .about(format!("list the S3 `{}` tokens", kind.code()));
            let request = match &page_number {
                Some(number) => request.query("page", number),
                None => request,
            };
            let page = match ctx.call(client, &request, Gate::Open)? {
                Step::Sent(response) => {
                    pages.read(response.bytes().len())?;
                    hippius::page(response.json()?)?
                }
                Step::DryRun(doc) => {
                    dry_runs.push(doc);
                    break;
                }
            };
            // A server that ignores `page` and answers page 1 again, with a `next` that
            // keeps rising, would repeat the list on every page; a later page that names
            // another `page` than the one asked is refused.
            let start_over = page_number.as_ref().map(|_| list::CHANGED_WHILE_READ);
            list::answered_as_asked(&page, page_number.as_deref().unwrap_or("1"), start_over)?;
            let next = page.next_page_token()?;
            for token in page.items {
                let mut token = token_document(without_fields(token, &is_s3_secret), kind);
                // Tenet 9: a listing shows `secret: null`, whatever the server sent.
                if let Value::Object(fields) = &mut token {
                    fields.insert("secret".to_owned(), Value::Null);
                }
                items.push(token);
            }
            let Some(number) = next else { break };
            pages.admit(&number)?;
            page_number = Some(number);
        }
    }
    if dry_runs.is_empty() {
        Ok(Printed::List(hippius::envelope(items)))
    } else {
        Ok(Printed::Document(dry_run_requests(dry_runs)))
    }
}

fn create_token(ctx: &Context<'_>, args: &TokenCreate) -> Result<Printed> {
    let body = token_body(args)?;
    require_reveal_or_file(ctx, args.reveal, args.secret_file.path.is_some())?;
    let request = Request::new(Method::Post, collection(args.kind))
        .json(&body)
        .about(format!("create an S3 `{}` token", args.kind.code()));
    let secret_file = args.secret_file.path.as_deref();
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
fn token_body(args: &TokenCreate) -> Result<Value> {
    let mut body = Map::new();
    if let Some(name) = &args.name {
        body.insert("name".to_owned(), name.as_str().into());
    }
    if let Some(expires_at) = &args.expires_at {
        body.insert("expires_at".to_owned(), expires_at.as_str().into());
    }
    let sub_flags = args.scope_type.is_some()
        || !args.buckets.is_empty()
        || !args.actions.is_empty()
        || !args.prefixes.is_empty()
        || !args.ips.is_empty()
        || args.not_before.is_some();
    match args.kind {
        TokenKind::Master if sub_flags => {
            return Err(Error::usage(
                "`--scope-type`, `--bucket`, `--action`, `--prefix`, `--ip`, and \
                 `--not-before` are for sub tokens, and `--kind` is `master`",
                "pass `--kind sub` to create a token with those limits",
            ));
        }
        TokenKind::Master => {}
        TokenKind::Sub => {
            if let Some(scope) = args.scope_type {
                body.insert("scope_type".to_owned(), scope.code().into());
            }
            for (field, values) in [
                ("bucket_names", &args.buckets),
                ("actions", &args.actions),
                ("allowed_prefixes", &args.prefixes),
                ("ip_allowlist", &args.ips),
            ] {
                if !values.is_empty() {
                    body.insert(field.to_owned(), values.clone().into());
                }
            }
            if let Some(not_before) = &args.not_before {
                body.insert("not_before".to_owned(), not_before.as_str().into());
            }
        }
    }
    Ok(Value::Object(body))
}

fn revoke_token(ctx: &Context<'_>, target: &TokenTarget) -> Result<Printed> {
    check_uuid(&target.token_id)?;
    let path = format!("{}{}/revoke/", collection(target.kind), target.token_id);
    let request = Request::new(Method::Post, path)
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
            doc.insert("id".to_owned(), target.token_id.as_str().into());
            // A revoke issues no secret; one the server sent anyway is not printed (tenet 9).
            if let Value::Object(body) = without_fields(response.json()?, &is_s3_secret) {
                doc.extend(body);
            }
            Ok(Value::Object(doc))
        },
    )
}

fn rotate_token(ctx: &Context<'_>, args: &TokenRotate) -> Result<Printed> {
    check_uuid(&args.target.token_id)?;
    require_reveal_or_file(ctx, args.reveal, args.secret_file.path.is_some())?;
    let path = format!(
        "{}{}/rotate/",
        collection(args.target.kind),
        args.target.token_id
    );
    let request = Request::new(Method::Post, path)
        .listed_by("hipc s3 tokens list")
        .about(format!(
            "rotate the S3 `{}` token `{}`",
            args.target.kind.code(),
            args.target.token_id
        ));
    let secret_file = args.secret_file.path.as_deref();
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
    let file = secret_file.map_or("<path>".to_owned(), |path| {
        phrase::shell_word(&path.to_string_lossy())
    });
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

/// A token id is a UUID. Checked before sending because it becomes a path segment.
fn check_uuid(id: &str) -> Result<()> {
    let groups: Vec<&str> = id.split('-').collect();
    let valid = groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(group, len)| group.len() == len && group.bytes().all(|b| b.is_ascii_hexdigit()));
    if valid {
        Ok(())
    } else {
        Err(Error::usage(
            format!("`{id}` is not a token id: a token id is a UUID"),
            "run `hipc s3 tokens list` to see the token ids",
        ))
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
    let parsed = serde_json::from_slice::<Value>(&bytes).map(Cleared);
    let Some(document) = parsed.as_ref().ok().and_then(|parsed| parsed.0.as_object()) else {
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
        json!({
            "aws_profile": profile.as_str(),
            "config_path": files.config.display().to_string(),
            "credentials_path": files.credentials.display().to_string(),
            "secret_written": written,
        })
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
    Ok(Printed::Document(doc(true)))
}

/// What an S3 plan cancel leaves, for its question and `hipc api`'s.
pub(crate) const CANCELLED: &str = "the plan stays until the end of the month paid for, with no \
                                    refund, and then the account is on pay as you go";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_names_follow_s3_rules() {
        for good in ["abc", "my-bucket.2", &"a".repeat(63)] {
            assert!(check_bucket(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "..",
            "ab",
            &"a".repeat(64),
            "Abc",
            "a_b",
            "-ab",
            "ab-",
            "a..b",
            "a/b",
            "a b",
            ".ab",
        ] {
            assert!(check_bucket(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn token_ids_are_uuids() {
        assert!(check_uuid("0f8fad5b-d9cb-469f-a165-70867728950e").is_ok());
        for bad in [
            "",
            "0f8fad5b-d9cb-469f-a165",
            "0f8fad5b-d9cb-469f-a165-70867728950g",
            "0f8fad5bd9cb469fa16570867728950e",
            "../../x",
            "0f8fad5b-d9cb-469f-a165-70867728950e-",
        ] {
            assert!(check_uuid(bad).is_err(), "{bad}");
        }
    }

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
